/**
 * Satellite-window factory: the one implementation of the on-demand popover
 * webview protocol (a hidden, transparent, always-on-top child window that a
 * host window sizes, positions, and drives over the Tauri event bus).
 *
 * The lifecycle it encapsulates:
 *
 * - **Get-by-label reuse.** A satellite is created at most once per label;
 *   `ensure()` first adopts a window that already exists (created by an
 *   earlier host instance in the same app run).
 * - **Readiness handshake.** The satellite route emits `readyEvent` (with
 *   `{ label }`) once its own show/hide listeners are attached. The host arms
 *   its ready listener BEFORE creating the window, so the ready signal cannot
 *   be lost in the gap; only after it arrives may a show payload be emitted
 *   (an earlier emit would vanish into a route with no listener yet).
 * - **Creation and readiness timeouts.** Window creation races the
 *   `tauri://created` / `tauri://error` pair against a timeout, then the
 *   ready event against its own timeout. Any failure fully resets the
 *   factory's state so a later `ensure()` retries creation from scratch.
 * - **Show protocol.** Size and position settle before the show payload goes
 *   out, and the window is revealed only after it, so the satellite never
 *   flashes at a stale location. Focus is optional and best-effort. A
 *   satellite route that sizes and reveals itself from the state payload
 *   opts out of the host-side reveal (`reveal: false`).
 * - **Tolerant hide, emit, and focus query.** `hide()`, `emitTo()`, and
 *   `isFocused()` address whichever window can be resolved (the pending
 *   creation, else get-by-label) and are no-ops (or false) when none can:
 *   the satellite may never have been created, or its creation may have
 *   failed.
 */

import { LogicalSize, PhysicalPosition } from '@tauri-apps/api/dpi';
import { listen } from '@tauri-apps/api/event';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

const CREATION_TIMEOUT_MS = 3000;
const READY_TIMEOUT_MS = 3000;

export interface SatelliteWindowOptions {
	/** Unique window label; also the `label` the ready event must carry. */
	label: string;
	/** Route the satellite webview loads. */
	url: string;
	/** Initial (and default) logical width. */
	width: number;
	/** Initial (and default) logical height. */
	height: number;
	/** Event the satellite route emits (with `{ label }`) once its listeners are attached. */
	readyEvent: string;
	/** Event `show()` emits the state payload on. */
	showEvent: string;
	/** Event `hide()` emits. */
	hideEvent: string;
	/**
	 * Overrides for the failure messages `ensure()` rejects with. These
	 * surface directly in the host's UI, so a host keeps its established
	 * wording; the defaults are the generic satellite phrasings.
	 */
	messages?: {
		/** Creation handshake timeout (default 'Satellite window creation timed out'). */
		creationTimeout?: string;
		/** `tauri://error` fired with an empty payload (default 'Unknown satellite window creation error'). */
		creationFailed?: string;
		/** Readiness timeout (default 'Satellite window did not become ready'). */
		readyTimeout?: string;
	};
}

export interface SatellitePosition {
	/** Physical screen x, e.g. from `anchorBelow` (see `lib/windows/anchor.ts`). */
	x: number;
	/** Physical screen y. */
	y: number;
	/** Logical width; defaults to the creation width. */
	width?: number;
	/** Logical height; defaults to the creation height. */
	height?: number;
}

export interface SatelliteShowOptions {
	/** Focus the satellite after revealing it (best-effort; failures are swallowed). */
	focus?: boolean;
	/**
	 * Reveal the window after emitting the state (default true). A satellite
	 * route that measures its content and reveals itself, to avoid a flash at
	 * a stale position, opts out; `show()` then only delivers the state
	 * payload (and focus is left to the satellite too).
	 */
	reveal?: boolean;
}

export interface SatelliteWindow {
	/** Resolve the satellite window, creating it (hidden) on first use. */
	ensure(): Promise<WebviewWindow>;
	/** Position, reveal, and hand the satellite its render state. */
	show(state: unknown, position?: SatellitePosition, opts?: SatelliteShowOptions): Promise<void>;
	/** Ask the satellite to hide; tolerates the window not existing. */
	hide(): Promise<void>;
	/**
	 * Emit an arbitrary event to the satellite when it exists (the pending
	 * creation, else get-by-label); a swallowed no-op when it does not or
	 * when the emit fails. Never creates the window.
	 */
	emitTo(event: string, payload?: unknown): Promise<void>;
	/**
	 * Whether the satellite holds keyboard focus (false when it does not
	 * exist or the query fails). Never creates the window. A host that
	 * dismisses a satellite when its own input blurs asks this first: a blur
	 * caused by the pointer landing in the satellite is not a dismissal.
	 */
	isFocused(): Promise<boolean>;
}

export function createSatelliteWindow(options: SatelliteWindowOptions): SatelliteWindow {
	let windowPromise: Promise<WebviewWindow> | null = null;
	let ready = false;
	let readyPromise: Promise<void> | null = null;

	function ensureReadyListener(): Promise<void> {
		if (ready || readyPromise) return readyPromise ?? Promise.resolve();

		readyPromise = new Promise((resolve) => {
			let unlisten: (() => void) | undefined;
			void listen<{ label?: string }>(options.readyEvent, (event) => {
				// The ready event is global on the bus; gate on the label so a
				// ready signal from another window cannot satisfy this handshake.
				if (event.payload?.label !== options.label) return;
				ready = true;
				readyPromise = null;
				unlisten?.();
				resolve();
			}).then((fn) => {
				unlisten = fn;
			});
		});

		return readyPromise;
	}

	function ensure(): Promise<WebviewWindow> {
		if (windowPromise) return windowPromise;

		windowPromise = (async () => {
			const existing = await WebviewWindow.getByLabel(options.label);
			if (existing) {
				ready = true;
				return existing;
			}

			const readyRace = ensureReadyListener();
			const satellite = new WebviewWindow(options.label, {
				url: options.url,
				width: options.width,
				height: options.height,
				visible: false,
				decorations: false,
				transparent: true,
				alwaysOnTop: true,
				skipTaskbar: true,
				shadow: false,
				resizable: false,
				focus: false,
			});

			await new Promise<void>((resolve, reject) => {
				const timeoutId = window.setTimeout(() => {
					reject(
						new Error(options.messages?.creationTimeout ?? 'Satellite window creation timed out'),
					);
				}, CREATION_TIMEOUT_MS);

				void satellite.once('tauri://created', () => {
					window.clearTimeout(timeoutId);
					resolve();
				});

				void satellite.once('tauri://error', (event) => {
					window.clearTimeout(timeoutId);
					const payload =
						typeof event.payload === 'string' ? event.payload : JSON.stringify(event.payload);
					reject(
						new Error(
							payload ||
								(options.messages?.creationFailed ?? 'Unknown satellite window creation error'),
						),
					);
				});
			});

			await Promise.race([
				readyRace,
				new Promise<never>((_, reject) => {
					window.setTimeout(() => {
						reject(
							new Error(options.messages?.readyTimeout ?? 'Satellite window did not become ready'),
						);
					}, READY_TIMEOUT_MS);
				}),
			]);
			return satellite;
		})().catch((error) => {
			// Full reset so a later call retries creation rather than adopting
			// a promise that will reject forever.
			windowPromise = null;
			ready = false;
			readyPromise = null;
			throw error;
		});

		return windowPromise;
	}

	async function show(
		state: unknown,
		position?: SatellitePosition,
		opts?: SatelliteShowOptions,
	): Promise<void> {
		const satellite = await ensure();
		if (position) {
			await satellite.setSize(
				new LogicalSize(position.width ?? options.width, position.height ?? options.height),
			);
			await satellite.setPosition(new PhysicalPosition(position.x, position.y));
		}
		await satellite.emit(options.showEvent, state);
		if (opts?.reveal === false) return;
		await satellite.show();
		if (opts?.focus) {
			await satellite.setFocus().catch(() => {});
		}
	}

	/** The window when it exists (pending creation, else get-by-label); never creates. */
	function resolveExisting(): Promise<WebviewWindow | null> {
		return windowPromise
			? windowPromise.catch(() => null)
			: WebviewWindow.getByLabel(options.label);
	}

	async function hide(): Promise<void> {
		const satellite = await resolveExisting();
		if (!satellite) return;
		await satellite.emit(options.hideEvent).catch(() => {});
	}

	async function emitTo(event: string, payload?: unknown): Promise<void> {
		const satellite = await resolveExisting();
		if (!satellite) return;
		await satellite.emit(event, payload).catch(() => {});
	}

	async function isFocused(): Promise<boolean> {
		const satellite = await resolveExisting();
		if (!satellite) return false;
		return satellite.isFocused().catch(() => false);
	}

	return { ensure, show, hide, emitTo, isFocused };
}
