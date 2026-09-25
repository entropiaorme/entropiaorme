/**
 * The stop flow for the tracking overlay: everything that happens between the
 * user asking to stop and the session actually stopping.
 *
 * A stop asks nothing. Armour costs are recorded when the player repairs or
 * scans, from the Cost popup, and spread back over the sessions they cover,
 * so the end of a session is no longer the moment to ask about them.
 *
 * The stop re-reads the snapshot once it lands, so the strip returns to idle
 * off the backend's own state rather than a stale frame.
 */

export interface PostSessionFlowOptions {
	/** Whether a session is currently active (gates the stop request). */
	isSessionActive(): boolean;
	/** Whether a start/stop toggle is already in flight (the stop request defers to it). */
	isBusy(): boolean;
	/** Re-read the tracking snapshot (after the stop). */
	refresh(): Promise<void>;
	/** Stop the session. */
	stopTracking(): Promise<unknown>;
}

export interface PostSessionFlow {
	/** A stop sequence is in flight. */
	readonly stopping: boolean;
	/** Ask to stop. */
	requestStop(): Promise<void>;
}

export function createPostSessionFlow(options: PostSessionFlowOptions): PostSessionFlow {
	let stopping = $state(false);

	return {
		get stopping() {
			return stopping;
		},
		async requestStop() {
			if (!options.isSessionActive() || options.isBusy()) return;
			stopping = true;
			try {
				await options.stopTracking();
				await options.refresh();
			} catch {
				// A refused stop leaves the session running and the strip showing
				// it; the user retries the stop.
			}
			stopping = false;
		},
	};
}
