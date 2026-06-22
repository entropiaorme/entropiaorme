/**
 * Visibility-gated polling: the single sanctioned home for `setInterval`.
 *
 * Some state is genuinely time-driven and cannot be pushed over the event spine
 * (a wall-clock cooldown tick, a quest-status poll with no backend push topic
 * yet). Those survivors poll through this helper
 * instead of a bare `setInterval`, so a backgrounded window does zero work: while
 * the visibility source reports hidden the timer is cleared (not merely skipped),
 * and on resume it re-arms and fires one catch-up tick.
 *
 * This module is the ONLY place a raw `setInterval` may appear; a lint enforces
 * that (every other timer-driven loop routes through here), so the
 * hidden-window-polling smell cannot grow back. At Rust-port time the
 * `VisibilitySource` abstraction maps onto a visibility signal feeding a
 * `tokio::time::interval` that is paused/resumed rather than always ticking.
 */

/** A surface whose visibility gates a poll. The poll body never branches on
 * window type: it reads `isVisible()` and reacts to `subscribe()` transitions,
 * so a new surface (a Tauri overlay window, say) is a new source, not a new
 * code path in the helper. */
export type VisibilitySource = {
	/** Whether the surface is visible right now. */
	isVisible: () => boolean;
	/** Subscribe to visible<->hidden transitions; returns an unsubscribe. */
	subscribe: (onChange: (visible: boolean) => void) => () => void;
};

export interface VisiblePollOptions {
	/** Poll period in milliseconds while visible. */
	readonly intervalMs: number;
	/** Run one tick immediately on start when visible (default true). */
	readonly immediate?: boolean;
	/** Run one catch-up tick on each hidden->visible resume (default true). */
	readonly tickOnResume?: boolean;
	/** Visibility source; defaults to {@link documentVisibility}. */
	readonly source?: VisibilitySource;
}

const noopUnsubscribe = (): void => {};

/**
 * Invoke a tick, surfacing an async rejection rather than letting `void` swallow
 * it. Current callers handle their own errors, but the helper accepts an async
 * tick, so a future one that does not should still be debuggable rather than
 * fail silently. A synchronous throw propagates as usual.
 */
function runTick(tick: () => void | Promise<void>, label: string): void {
	const result = tick();
	if (result instanceof Promise) {
		result.catch((err) => console.error(`${label}: tick failed`, err));
	}
}

/**
 * Default source: the Page Visibility API on the always-alive main window,
 * where every current poll survivor lives. Falls back to "always visible, no
 * transitions" when there is no DOM (non-browser context), so the helper is
 * safe to construct anywhere.
 */
export function documentVisibility(): VisibilitySource {
	if (typeof document === 'undefined') {
		return { isVisible: () => true, subscribe: () => noopUnsubscribe };
	}
	const visible = (): boolean => document.visibilityState !== 'hidden';
	return {
		isVisible: visible,
		subscribe: (onChange) => {
			const handler = (): void => onChange(visible());
			document.addEventListener('visibilitychange', handler);
			return () => document.removeEventListener('visibilitychange', handler);
		},
	};
}

/**
 * Start a visibility-gated poll. Returns a `stop()` that clears the timer and
 * detaches the visibility subscription: hand it straight back as the `$effect`
 * teardown.
 */
export function useVisiblePoll(
	tick: () => void | Promise<void>,
	options: VisiblePollOptions,
): () => void {
	const { intervalMs, immediate = true, tickOnResume = true } = options;
	const source = options.source ?? documentVisibility();

	let timer: ReturnType<typeof setInterval> | null = null;

	const run = (): void => runTick(tick, 'useVisiblePoll');

	const arm = (runNow: boolean): void => {
		if (timer !== null) {
			return;
		}
		if (runNow) {
			run();
		}
		timer = setInterval(run, intervalMs);
	};

	const disarm = (): void => {
		if (timer !== null) {
			clearInterval(timer);
			timer = null;
		}
	};

	if (source.isVisible()) {
		arm(immediate);
	}

	const unsubscribe = source.subscribe((visible) => {
		if (visible) {
			arm(tickOnResume);
		} else {
			disarm();
		}
	});

	return () => {
		disarm();
		unsubscribe();
	};
}

/**
 * The one sanctioned poll that must keep running while its window is hidden: the
 * HUD overlay persists its on-screen position on a fixed cadence, and the
 * overlay's hidden/shown state is not reliably observable from inside its own
 * webview (a pre-spawned Tauri window's document-visibility does not flip on
 * show/hide). It lives here, in the sanctioned `setInterval` home, rather than
 * as a scattered lint exemption at the call site. Deliberately narrow: there is
 * no pause knob, so "runs while hidden" cannot be reused to slip a network poll
 * past the visibility gate. Returns a `stop()` for the `$effect` teardown.
 */
export function windowGeometryPoll(
	tick: () => void | Promise<void>,
	intervalMs: number,
): () => void {
	const timer = setInterval(() => runTick(tick, 'windowGeometryPoll'), intervalMs);
	return () => clearInterval(timer);
}
