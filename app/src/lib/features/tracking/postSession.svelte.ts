/**
 * The stop flow for the tracking overlay: everything that happens between the
 * user asking to stop and the session actually stopping.
 *
 * One prompt: when the end-of-session armour reminder is enabled, the stop
 * request does not stop: it arms a Record armour costs? decision.
 *
 * Record does not stop either. Armour cost is part of the session it was spent
 * in, and a reading taken after the session ended reads like an afterthought
 * about something already closed, so Record opens the armour-cost workflow
 * against the session still running and leaves it running. The user stops when
 * they have finished, answering Later that time.
 *
 * Later stops, and opens nothing: the user has just said the armour can wait.
 * A whole-session setup left unnamed is not lost by that; the armour-cost panel
 * offers every session still owed one the next time it opens.
 *
 * The stop re-reads the snapshot once it lands, so the strip returns to idle
 * off the backend's own state rather than a stale frame. (The post-stop
 * quest-link prompt this flow used to run retired with the curated link model:
 * the quest lifecycle records its own stretches now, so there is nothing left
 * to ask after the stop.)
 */

export interface PostSessionFlowOptions {
	/** Whether a session is currently active (gates the stop request). */
	isSessionActive(): boolean;
	/** Whether a start/stop toggle is already in flight (the stop request defers to it). */
	isBusy(): boolean;
	/** Whether the end-of-session armour reminder is enabled (arms the armour prompt). */
	armourReminderEnabled(): boolean;
	/** Re-read the tracking snapshot (after the stop). */
	refresh(): Promise<void>;
	/** Stop the session. */
	stopTracking(): Promise<unknown>;
	/** Open the armour workflow against the session still running. */
	showArmourWorkflowInSession(): Promise<boolean>;
}

export interface PostSessionFlow {
	/** The armour prompt is showing (the stop is parked on its answer). */
	readonly awaitingArmourDecision: boolean;
	/** A stop sequence is in flight. */
	readonly stopping: boolean;
	/** Ask to stop: arms the armour prompt when the reminder is on, else stops. */
	requestStop(): Promise<void>;
	/**
	 * Answer the armour prompt. Record opens the cost workflow and leaves the
	 * session running; Later stops it.
	 */
	decideArmourTrack(action: 'yes' | 'no'): Promise<void>;
}

export function createPostSessionFlow(options: PostSessionFlowOptions): PostSessionFlow {
	let awaitingArmourDecision = $state(false);
	let stopping = $state(false);

	async function stop(): Promise<void> {
		stopping = true;
		try {
			await options.stopTracking();
			await options.refresh();
		} catch {
			// A refused stop leaves the session running and the strip showing
			// it; the user retries the stop.
		}
		stopping = false;
	}

	return {
		get awaitingArmourDecision() {
			return awaitingArmourDecision;
		},
		get stopping() {
			return stopping;
		},
		async requestStop() {
			if (!options.isSessionActive() || options.isBusy()) return;
			if (options.armourReminderEnabled()) {
				awaitingArmourDecision = true;
				return;
			}
			await stop();
		},
		async decideArmourTrack(action: 'yes' | 'no') {
			if (!awaitingArmourDecision) return;
			awaitingArmourDecision = false;
			if (action === 'yes') {
				// Recording belongs to the session, so the session stays
				// running and the stop is the user's next move, not this one's.
				await options.showArmourWorkflowInSession();
				return;
			}
			await stop();
		},
	};
}
