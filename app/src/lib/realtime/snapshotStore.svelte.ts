/**
 * Snapshot-store factory: the one implementation of the app's realtime read
 * doctrine, for any window that renders a backend snapshot.
 *
 * Hydration-only and event-driven. `hydrate()` runs the snapshot read once;
 * `subscribe()` listens for the backend frames the shell's event bridge emits
 * on the given Tauri-bus topic and re-reads the snapshot on each, so a consumer
 * updates by subscription rather than by polling.
 *
 * Routing discipline (the load-bearing constraint): a bridged frame is a pure
 * trigger. We never fold a frame field into rendered state; every
 * render-shaping value comes from the snapshot read. That keeps the snapshot
 * the single source of shape: because we re-read rather than reduce, a
 * payload-less frame can never be mistaken for an idle state (which would
 * blank an active view). A state transition arrives as an ordinary frame and
 * re-reads to the new snapshot.
 *
 * Startup needs no special case here: the typed transport holds a read made
 * while the backend is still starting and sends it once the backend is ready
 * (`lib/api/readiness.svelte.ts`), so the initial `hydrate` lands on live
 * state rather than failing.
 *
 * Consumers attach the listener FIRST and hydrate after (`subscribe` then
 * `hydrate`), so a frame arriving during subscription setup is not lost: it
 * simply re-triggers a read. Each webview is its own JS context, so a window
 * that needs the same snapshot keeps its own store instance.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface SnapshotStore<T> {
	/** The latest snapshot, or `null` before the first successful hydration. */
	readonly current: T | null;
	/** Re-read the snapshot and publish it; overlapping calls coalesce. */
	hydrate(): Promise<void>;
	/** Listen for relayed frames on the topic; returns the detach function. */
	subscribe(): Promise<UnlistenFn>;
}

/**
 * Build a snapshot store over a Tauri-bus `topic` (the colon form of the wire
 * topic; Tauri event names forbid dots) and
 * a `read` that fetches the consolidated snapshot.
 */
export function createSnapshotStore<T>(topic: string, read: () => Promise<T>): SnapshotStore<T> {
	let current = $state<T | null>(null);
	let inFlight = false;
	let refetchQueued = false;

	/**
	 * Re-read the snapshot and publish it. Overlapping calls coalesce: a frame
	 * arriving mid-read queues exactly one follow-up read, so the store always
	 * settles on the latest state and two reads can never race to write out of
	 * order. A failed read leaves the last good snapshot in place; the next
	 * frame re-reads.
	 */
	async function hydrate(): Promise<void> {
		if (inFlight) {
			refetchQueued = true;
			return;
		}
		inFlight = true;
		try {
			do {
				refetchQueued = false;
				try {
					current = await read();
				} catch {
					// Transient read failure: keep the last good snapshot rather than
					// blanking the consumer. The catch is INSIDE the loop so a re-read
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
	 * Subscribe to the bridged backend frames and keep the snapshot current.
	 * Returns a teardown that detaches the listener. Each frame triggers one
	 * snapshot read; see the routing discipline in the module header.
	 */
	function subscribe(): Promise<UnlistenFn> {
		return listen(topic, () => {
			void hydrate();
		});
	}

	return {
		get current() {
			return current;
		},
		hydrate,
		subscribe,
	};
}
