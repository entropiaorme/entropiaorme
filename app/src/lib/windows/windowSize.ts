/**
 * Self-sizing for the overlay windows: keep a transparent, chromeless
 * window's OS footprint exactly the surfaces it draws.
 *
 * An overlay marks each surface it draws (a glass strip, a notice, a panel)
 * with `data-overlay-surface`. The window is sized to the surfaces' bounding
 * box, flush from its top-left corner, and its pointer input is cut to the
 * surfaces themselves (rounded corners included, through the shell's
 * `setWindowHitRegion`), so no transparent pixel the window covers swallows
 * a click meant for what lies beneath. Nothing is drawn outside the
 * surfaces: an overlay surface carries an edge, not an outer shadow, since
 * a shadow would be cut away (or sit in a margin that blocks clicks).
 *
 * `createWindowSizeSync` owns the loop: coalesce sync requests onto
 * animation frames, measure, dedupe against the last applied size and
 * region, and apply. A failed apply forgets what it tried, so the next
 * request retries rather than deduping against something that never landed.
 */

import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { type HitShape, setWindowHitRegion } from '$lib/api/shell';

/** The attribute that marks a drawn surface inside an overlay window. */
export const OVERLAY_SURFACE_ATTRIBUTE = 'data-overlay-surface';
const SURFACE_SELECTOR = `[${OVERLAY_SURFACE_ATTRIBUTE}]`;

export interface OverlaySurfaceLayout {
	/** Logical window width: the surfaces' right-most edge, ceiled. */
	width: number;
	/** Logical window height: the surfaces' bottom-most edge, ceiled. */
	height: number;
	/** The surfaces the window's input is cut to (empty: the whole window). */
	shapes: HitShape[];
}

/** The marked surfaces at or under `root`, in document order. */
export function overlaySurfaces(root: HTMLElement): HTMLElement[] {
	const nested = Array.from(root.querySelectorAll<HTMLElement>(SURFACE_SELECTOR));
	return root.matches(SURFACE_SELECTOR) ? [root, ...nested] : nested;
}

/** The smallest of `el`'s four corner radii (a percentage resolves against
 * its shorter side): cutting to the smallest never removes a drawn pixel. */
function cornerRadius(el: HTMLElement, width: number, height: number): number {
	const style = getComputedStyle(el);
	const radii = [
		style.borderTopLeftRadius,
		style.borderTopRightRadius,
		style.borderBottomRightRadius,
		style.borderBottomLeftRadius,
	].map((value) => {
		const amount = Number.parseFloat(value);
		if (!Number.isFinite(amount)) return 0;
		return value.trim().endsWith('%') ? (amount / 100) * Math.min(width, height) : amount;
	});
	return Math.max(0, Math.min(...radii));
}

/**
 * The window footprint for `root`'s current layout: the marked surfaces,
 * and the bounding size that holds them. A root with no visible marked
 * surface falls back to its own box with input across the whole window.
 */
export function measureOverlaySurfaces(root: HTMLElement): OverlaySurfaceLayout {
	const shapes: HitShape[] = [];
	for (const surface of overlaySurfaces(root)) {
		const rect = surface.getBoundingClientRect();
		if (rect.width <= 0 || rect.height <= 0) continue;
		shapes.push({
			x: rect.left,
			y: rect.top,
			width: rect.width,
			height: rect.height,
			radius: cornerRadius(surface, rect.width, rect.height),
		});
	}
	if (shapes.length === 0) {
		const rect = root.getBoundingClientRect();
		return {
			width: Math.max(1, Math.ceil(rect.right)),
			height: Math.max(1, Math.ceil(rect.bottom)),
			shapes,
		};
	}
	return {
		width: Math.max(1, Math.ceil(Math.max(...shapes.map((shape) => shape.x + shape.width)))),
		height: Math.max(1, Math.ceil(Math.max(...shapes.map((shape) => shape.y + shape.height)))),
		shapes,
	};
}

export interface WindowSizeSync {
	/** Request a sync on the next animation frame; repeat calls coalesce. */
	schedule(): void;
	/** Cancel a pending sync (teardown). */
	cancel(): void;
	/**
	 * Watch the root and every marked surface for size changes, and the
	 * root's subtree for surfaces coming and going, scheduling a sync on
	 * each. Returns the teardown. Schedules an initial sync.
	 */
	connect(): () => void;
}

export interface WindowSizeSyncOptions {
	/** Runs after each scheduled sync settles, applied or deduped, for work
	 * that must follow a possible window resize (e.g. re-anchoring a
	 * satellite). */
	afterSync?: () => void;
	/** Size the window to the surfaces (default true). A satellite whose host
	 * sizes and positions it only cuts its input region. */
	resize?: boolean;
}

/**
 * Build the sync loop over a `root` accessor (`null` while the element is
 * unmounted; `schedule()`, `connect()`, and the sync itself no-op then).
 */
export function createWindowSizeSync(
	root: () => HTMLElement | null,
	options: WindowSizeSyncOptions = {},
): WindowSizeSync {
	const resize = options.resize ?? true;
	let frame: number | null = null;
	let lastWidth: number | null = null;
	let lastHeight: number | null = null;
	let lastRegion: string | null = null;

	async function sync(): Promise<void> {
		const el = root();
		if (!el) return;

		const layout = measureOverlaySurfaces(el);
		if (resize && (layout.width !== lastWidth || layout.height !== lastHeight)) {
			lastWidth = layout.width;
			lastHeight = layout.height;
			try {
				await getCurrentWindow().setSize(new LogicalSize(layout.width, layout.height));
			} catch {
				lastWidth = null;
				lastHeight = null;
			}
		}

		const region = JSON.stringify(layout.shapes);
		if (region === lastRegion) return;
		lastRegion = region;
		try {
			await setWindowHitRegion(layout.shapes);
		} catch {
			lastRegion = null;
		}
	}

	function schedule() {
		if (!root() || frame != null) return;
		frame = window.requestAnimationFrame(() => {
			frame = null;
			void sync().then(() => {
				options.afterSync?.();
			});
		});
	}

	function cancel() {
		if (frame == null) return;
		window.cancelAnimationFrame(frame);
		frame = null;
	}

	function connect(): () => void {
		const el = root();
		if (!el) return () => {};

		const resizes = new ResizeObserver(() => schedule());
		const observeSurfaces = () => {
			resizes.disconnect();
			resizes.observe(el);
			for (const surface of overlaySurfaces(el)) {
				if (surface !== el) resizes.observe(surface);
			}
		};
		// Surfaces mount and unmount with the overlay's state (a notice
		// arriving, a panel switching view); each one is re-observed so its
		// own size changes move the region too.
		const mutations = new MutationObserver(() => {
			observeSurfaces();
			schedule();
		});
		observeSurfaces();
		mutations.observe(el, { childList: true, subtree: true });
		schedule();

		return () => {
			cancel();
			resizes.disconnect();
			mutations.disconnect();
		};
	}

	return { schedule, cancel, connect };
}
