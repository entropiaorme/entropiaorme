/**
 * Backend to webview event relay.
 *
 * The backend pushes coarse domain events over a server-sent-events stream
 * (`GET /api/events`). This relay, running in the always-alive main window,
 * opens that stream once and re-emits each frame onto the Tauri event bus, so
 * every window (including the hidden overlays) receives backend state changes
 * by subscription rather than by polling.
 *
 * Only the main window relays. Every window inherits the root layout that
 * starts this, but a relay in each would open a duplicate stream and re-emit
 * every event several times over; the main window is the one guaranteed alive
 * for the app's lifetime (closing it exits the app), so it owns the single
 * relay.
 *
 * This is the frontend half of the event spine. At Rust-port time the backend
 * SSE producer is replaced wholesale, but the stream contract and this relay are
 * unchanged: the webview never learns the backend language changed.
 */

import { emit } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { EVENTS_STREAM_URL } from '$lib/api';

/** Wire topics forwarded from the SSE stream onto the Tauri bus. Grows as more
 * domain topics are added (quests, ...). Each is re-emitted on its colon-form
 * Tauri topic and covered by the reconnect nudge below. */
const FORWARDED_TOPICS = ['tracking.session.updated', 'scan.status.changed'] as const;

interface DomainEnvelope {
	type?: string;
	payload?: Record<string, unknown>;
}

let source: EventSource | null = null;

/**
 * Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots),
 * so the dotted wire topic is namespaced with colons for the Tauri bus. The SSE
 * wire contract keeps the dotted form; this is the relay's only transform.
 */
function toTauriEventName(topic: string): string {
	return topic.replaceAll('.', ':');
}

function forward(topic: string, raw: string): void {
	let envelope: DomainEnvelope;
	try {
		envelope = JSON.parse(raw) as DomainEnvelope;
	} catch {
		return;
	}
	// Re-emit the whole typed envelope onto the Tauri bus, so a topic-aware
	// consumer sees the full contract (type, event_version, occurred_at,
	// payload), not just the payload.
	void emit(toTauriEventName(topic), envelope);
}

/**
 * Start the single backend to webview relay. No-op outside the main window or
 * outside a Tauri webview. Idempotent. Returns a stop function (the layout hands
 * it back to Svelte for teardown on window close).
 */
export function startEventRelay(): () => void {
	if (typeof window === 'undefined' || typeof EventSource === 'undefined') {
		return () => {};
	}
	let label: string;
	try {
		label = getCurrentWindow().label;
	} catch {
		// Not running inside a Tauri webview (e.g. a plain browser preview).
		return () => {};
	}
	if (label !== 'main' || source !== null) {
		return stopEventRelay;
	}

	const stream = new EventSource(EVENTS_STREAM_URL);
	source = stream;

	stream.onopen = () => {
		// Hydrate on (re)connect: prompt every window to re-read its current
		// state, so an EventSource auto-reconnect cannot leave a window showing
		// stale data. A payload-less typed frame on each forwarded topic drives
		// the topic-aware consumers (the tracking and scan stores, the overlay):
		// each subscribes through its typed topic, so a reconnect re-reads through
		// it too, and a payload-less frame reads as "re-hydrate" rather than as an
		// idle session.
		for (const topic of FORWARDED_TOPICS) {
			void emit(toTauriEventName(topic), {});
		}
	};
	for (const topic of FORWARDED_TOPICS) {
		stream.addEventListener(topic, (event) => {
			forward(topic, (event as MessageEvent).data as string);
		});
	}

	return stopEventRelay;
}

/** Close the relay stream if open. */
export function stopEventRelay(): void {
	if (source !== null) {
		source.close();
		source = null;
	}
}
