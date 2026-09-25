/**
 * Consolidated tracking store: the dashboard's single source of live-session
 * render shape.
 *
 * A `createSnapshotStore` instance over the bridged tracking topic: hydration-only
 * and event-driven, with the coalesced re-read, keep-last-good-on-failure, and
 * pure-trigger frame semantics implemented (and tested) by the factory; see
 * `lib/realtime/snapshotStore.svelte.ts` for the routing discipline. A session
 * stop arrives as an ordinary frame and re-reads to the idle snapshot, where
 * the activity feed is empty (the feed clears on idle).
 */

import { getTrackingSnapshot, type TrackingSnapshot } from '$lib/api';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';

/**
 * The Tauri-bus topic the shell's event bridge emits each backend tracking
 * frame on: the colon form of the `tracking.session.updated` wire topic (Tauri
 * event names forbid dots). See `spawn_domain_event_bridge` in the shell.
 */
export const TRACKING_TOPIC = 'tracking:session:updated';

/** The consolidated readout; `current` is `null` before the first hydration. */
export const trackingSnapshot = createSnapshotStore<TrackingSnapshot>(
	TRACKING_TOPIC,
	getTrackingSnapshot,
);

/** Re-read the consolidated snapshot and publish it; overlapping calls coalesce. */
export const hydrate = trackingSnapshot.hydrate;

/**
 * Subscribe to the bridged backend tracking frames and keep the snapshot
 * current. Returns a teardown that detaches the listener.
 */
export const subscribeTracking = trackingSnapshot.subscribe;
