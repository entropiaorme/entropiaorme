/**
 * Runes-native state for the after-session review of tools read off the
 * screen that are not in the library.
 *
 * The review is read for the session shown and re-read when the session
 * changes; a read or a settlement that lands after the player moved to
 * another session is dropped. A running session's sightings are shown as
 * they stand, without the offer to add, and read again as the tracker moves
 * on until the session has ended. Every settlement and undo answers with
 * the refreshed review, applied here, or with a refusal that changed
 * nothing, said in the player's words, after which the review is read
 * again (another window may have settled it). While one is being saved
 * every other is refused at once. Adding a tool whose entry is turned off
 * offers to turn it on.
 */

import {
	getToolSightings,
	setLibraryEnabled,
	settleToolSighting,
	type ToolSighting,
	type ToolSightingAction,
	type ToolSightingSettleResult,
	undoToolSighting,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { describeChange, describeRefusal, entryToTurnOn, sightingName } from './toolSighting';

export interface ToolSightingOptions {
	/** The session shown. */
	sessionId: () => string;
}

export function createToolSightingModel(options: ToolSightingOptions) {
	let sightings = $state<ToolSighting[]>([]);
	let loadedFor = $state<string | null>(null);
	let ended = $state(false);
	let error = $state<string | null>(null);
	let notice = $state<string | null>(null);
	let busy = $state<string | null>(null);
	/** A turned-off entry an addition linked to, offered to be turned on. */
	let offer = $state<{ id: string; label: string } | null>(null);
	let reading = false;
	let generation = 0;

	async function refresh() {
		const sessionId = options.sessionId();
		const run = ++generation;
		if (loadedFor !== sessionId) {
			error = null;
			notice = null;
			offer = null;
		}
		reading = true;
		try {
			const review = await getToolSightings(sessionId);
			if (run !== generation || options.sessionId() !== sessionId) return;
			sightings = review.sightings;
			ended = review.ended;
			loadedFor = sessionId;
		} catch (e) {
			if (run === generation && options.sessionId() === sessionId) {
				error = describeError(e, 'Could not load the detected tools.');
			}
		} finally {
			if (run === generation) reading = false;
		}
	}

	async function apply(
		sighting: ToolSighting,
		send: (sessionId: string) => Promise<ToolSightingSettleResult>,
	) {
		if (busy !== null) return;
		const sessionId = options.sessionId();
		busy = sighting.id;
		error = null;
		notice = null;
		offer = null;
		let refused = false;
		try {
			const result = await send(sessionId);
			if (options.sessionId() !== sessionId) return;
			if (result.kind === 'refused') {
				error = describeRefusal(result.reason);
				refused = true;
			} else {
				generation += 1;
				sightings = result.review.sightings;
				ended = result.review.ended;
				loadedFor = sessionId;
				notice = describeChange(result.change, sightingName(sighting));
				offer = entryToTurnOn(result.change);
			}
		} catch (e) {
			if (options.sessionId() === sessionId) {
				error = describeError(e, 'Could not record that.');
			}
		} finally {
			busy = null;
		}
		// The row stands otherwise than shown: show it as it stands.
		if (refused) await refresh();
	}

	function settle(sighting: ToolSighting, action: ToolSightingAction) {
		return apply(sighting, (sessionId) => settleToolSighting(sessionId, sighting.id, action));
	}

	return {
		/** The sightings of the session shown. */
		get sightings() {
			return loadedFor === options.sessionId() ? sightings : [];
		},
		/** The session shown has ended, so its tools can be added. */
		get ended() {
			return loadedFor === options.sessionId() && ended;
		},
		get error() {
			return error;
		},
		get notice() {
			return notice;
		},
		/** The sighting being saved, if any. */
		get busy() {
			return busy;
		},
		/** A turned-off entry an addition linked to, to turn on. */
		get offer() {
			return offer;
		},
		refresh,
		/** The tracker moved on: a running session's review is read again
		 * (shots still being booked, the session ending), one read at a
		 * time; an ended session's stands. */
		followRunning() {
			if (loadedFor !== options.sessionId() || ended || reading || busy !== null) return;
			return refresh();
		},
		/** Turn on the entry an addition linked to. */
		async turnOn() {
			const entry = offer;
			if (!entry || busy !== null) return;
			busy = entry.id;
			error = null;
			try {
				await setLibraryEnabled(entry.id, true);
				offer = null;
				notice = `${entry.label} is on.`;
			} catch (e) {
				error = describeError(e, 'Could not turn it on.');
			} finally {
				busy = null;
			}
		},
		/** Add the tool to equipment. */
		add: (sighting: ToolSighting) => settle(sighting, 'add'),
		/** It is not a tool the player tracks: never ask about it again. */
		dismiss: (sighting: ToolSighting) => settle(sighting, 'dismiss'),
		/** Ask no more about it for this session. */
		leave: (sighting: ToolSighting) => settle(sighting, 'leave'),
		/** Undo its settlement, so it is asked about again. */
		undo: (sighting: ToolSighting) =>
			apply(sighting, (sessionId) => undoToolSighting(sessionId, sighting.id)),
		dismissError() {
			error = null;
		},
	};
}

export type ToolSightingModel = ReturnType<typeof createToolSightingModel>;
