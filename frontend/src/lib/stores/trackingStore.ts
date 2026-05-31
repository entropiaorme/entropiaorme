/**
 * Consolidated tracking store: the dashboard's single source of live-session
 * render shape.
 *
 * Hydration-only and event-driven. `hydrate()` reads the consolidated
 * `/tracking/snapshot` once; `subscribeTracking()` listens the backend tracking
 * frames the event relay re-emits onto the Tauri bus and re-reads the snapshot
 * on each, so the dashboard updates by subscription rather than by polling the
 * three legacy tracking endpoints.
 *
 * Routing discipline (the load-bearing constraint): a relayed frame is a pure
 * trigger. We never fold a frame field into rendered state; every render-shaping
 * value comes from the snapshot read. That keeps the snapshot the single source
 * of shape and makes the store reconnect-safe by construction. The relay's
 * reconnect nudge carries no payload, and because we re-read rather than reduce,
 * an absent payload can never be mistaken for an idle session (which would blank
 * an active hunt on an EventSource reconnect). A session stop arrives as an
 * ordinary frame and re-reads to the idle snapshot, where the activity feed is
 * empty (the feed clears on idle).
 */
import { writable, type Writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import { getTrackingSnapshot, type TrackingSnapshot } from '$lib/api';

/**
 * The Tauri-bus topic the event relay re-emits each backend tracking frame on:
 * the colon form of the `tracking.session.updated` wire topic (Tauri event
 * names forbid dots). See `lib/realtime/eventRelay.ts`.
 */
const TRACKING_TOPIC = 'tracking:session:updated';

/** The consolidated readout, or `null` before the first hydration. */
export const trackingSnapshot: Writable<TrackingSnapshot | null> = writable(null);

let inFlight = false;
let refetchQueued = false;

/**
 * Re-read the consolidated snapshot and publish it. Overlapping calls coalesce:
 * a frame arriving mid-read queues exactly one follow-up read, so the store
 * always settles on the latest state and two reads can never race to write out
 * of order. A failed read leaves the last good snapshot in place; the next frame
 * (or the relay's reconnect nudge) re-reads.
 */
export async function hydrate(): Promise<void> {
	if (inFlight) {
		refetchQueued = true;
		return;
	}
	inFlight = true;
	try {
		do {
			refetchQueued = false;
			try {
				trackingSnapshot.set(await getTrackingSnapshot());
			} catch {
				// Transient read failure: keep the last good snapshot rather than
				// blanking the dashboard. The catch is INSIDE the loop so a re-read
				// a frame queued during this attempt is not abandoned: the do-while
				// still runs it (it may be the last transition, with no later frame
				// to re-trigger the read).
			}
		} while (refetchQueued);
	} finally {
		inFlight = false;
	}
}

/**
 * Subscribe to the relayed backend tracking frames and keep the snapshot
 * current. Returns a teardown that detaches the listener. Each frame (a live
 * update, a session start or stop, or the relay's payload-less reconnect nudge)
 * triggers one snapshot read; see the routing discipline in the module header.
 */
export function subscribeTracking(): Promise<UnlistenFn> {
	return listen(TRACKING_TOPIC, () => {
		void hydrate();
	});
}
