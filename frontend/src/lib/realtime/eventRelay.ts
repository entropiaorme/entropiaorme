/**
 * Backend to webview hydrate nudge.
 *
 * Real-time backend domain events reach every window over the Tauri event bus:
 * the native producer spine emits each typed envelope directly on its colon-form
 * Tauri topic (see `spawn_domain_event_bridge` in the shell), and the topic-aware
 * consumers (the tracking and scan stores, the overlay) subscribe through
 * `listen()`. This module owns only the HYDRATE NUDGE: a payload-less frame on
 * each forwarded topic that prompts every window to re-read its current state,
 * so a window cannot show stale data after it first mounts or after the native
 * spine is installed at startup.
 *
 * Only the main window nudges. Every window inherits the root layout that starts
 * this, but the nudge is a global emit (one reaches every window), so the main
 * window (guaranteed alive for the app's lifetime; closing it exits the app)
 * owns the single emitter.
 *
 * History: the real-event transport was once an `EventSource` over
 * `GET /api/events` relayed onto the bus. The Rust-native collapse moved that
 * transport into the shell (events are emitted natively), leaving this module the
 * hydrate half of the event spine; the webview never learns the transport changed.
 */

import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

/** Wire topics whose colon-form Tauri events the windows subscribe to. The
 * hydrate nudge fires a payload-less frame on each. Grows as more domain topics
 * are added (quests, ...); the Rust bridge forwards the live events on the same
 * topics. */
const FORWARDED_TOPICS = ['tracking.session.updated', 'scan.status.changed'] as const;

/**
 * Substrate ready signal. The shell composes the native service spine at startup
 * and publishes the backend substrate only once it is installed (the `api_request`
 * command errors until then); the native producer's events begin flowing from that
 * point. The shell emits this once composition completes so the relay can
 * re-hydrate every window onto the freshly-live native state.
 */
const SUBSTRATE_NATIVE_INSTALLED_EVENT = 'substrate:native-installed';

let started = false;
let unlistenHandover: UnlistenFn | undefined;

/**
 * Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots),
 * so the dotted wire topic is namespaced with colons for the Tauri bus. The Rust
 * bridge applies the same transform on the emit side.
 */
function toTauriEventName(topic: string): string {
	return topic.replaceAll('.', ':');
}

/**
 * Prompt every window to re-read its current state. A payload-less typed frame on
 * each forwarded topic drives the topic-aware consumers (the tracking and scan
 * stores, the overlay): each subscribes through its typed topic, so a
 * payload-less frame reads as "re-hydrate" rather than as an idle session.
 */
function hydrate(): void {
	for (const topic of FORWARDED_TOPICS) {
		void emit(toTauriEventName(topic), {});
	}
}

/**
 * Start the single backend to webview hydrate nudge. No-op outside the main
 * window or outside a Tauri webview. Idempotent. Returns a stop function (the
 * layout hands it back to Svelte for teardown on window close).
 */
export function startEventRelay(): () => void {
	if (typeof window === 'undefined') {
		return () => {};
	}
	let label: string;
	try {
		label = getCurrentWindow().label;
	} catch {
		// Not running inside a Tauri webview (e.g. a plain browser preview).
		return () => {};
	}
	if (label !== 'main' || started) {
		return stopEventRelay;
	}
	started = true;

	// Initial hydrate. The consumers mount and `listen()` before this layout-level
	// start runs (children mount before the parent layout's onMount), so this first
	// nudge cannot race ahead of their subscriptions.
	hydrate();

	// The substrate may hot-install the native service spine mid-session; when it
	// does, re-hydrate every window onto the now-live native state. The listener
	// attach stays independent of this promise.
	void listen(SUBSTRATE_NATIVE_INSTALLED_EVENT, () => {
		hydrate();
	}).then((unlisten) => {
		// Detach immediately if the relay was already stopped before the listener
		// attached (started is cleared on stop) rather than leaking it.
		if (!started) unlisten();
		else unlistenHandover = unlisten;
	});

	return stopEventRelay;
}

/** Stop the relay: detach the handover listener if open. */
export function stopEventRelay(): void {
	unlistenHandover?.();
	unlistenHandover = undefined;
	started = false;
}
