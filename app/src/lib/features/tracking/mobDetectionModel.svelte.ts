/**
 * Runes-native state for the after-session review of detected mobs.
 *
 * The review is read for the session shown and re-read when the session
 * changes; a read that lands after the player moved to another session is
 * dropped. Only an ended session's review is shown. Settling a line answers
 * with the refreshed review, applied here, and tells the owner so the
 * session's own figures re-read; a name typed only loosely like a mob
 * changes nothing and offers that mob, to accept in one action. While
 * a settlement is being saved every other one is refused at once.
 */

import {
	type DetectedCreature,
	getMobDetections,
	type MobDetectionTarget,
	type ReviewedMob,
	settleMobDetection,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { describeSettled, mobKey } from './mobDetection';

export interface MobDetectionOptions {
	/** The session shown. */
	sessionId: () => string;
	/** A line was settled: the session's figures changed. */
	onSettled: () => void | Promise<void>;
}

export function createMobDetectionModel(options: MobDetectionOptions) {
	let mobs = $state<ReviewedMob[]>([]);
	let loadedFor = $state<string | null>(null);
	let error = $state<string | null>(null);
	let notice = $state<string | null>(null);
	let busy = $state<string | null>(null);
	let drafts = $state<Record<string, string>>({});
	let ended = $state(false);
	/** The mob offered for a line whose typed name read only loosely. */
	let offer = $state<{ key: string; creature: DetectedCreature } | null>(null);
	let generation = 0;

	function draftOf(mob: ReviewedMob): string {
		return drafts[mobKey(mob.mob)] ?? mob.name ?? '';
	}

	async function refresh() {
		const sessionId = options.sessionId();
		const run = ++generation;
		try {
			const review = await getMobDetections(sessionId);
			if (run !== generation || options.sessionId() !== sessionId) return;
			mobs = review.mobs;
			ended = review.ended;
			loadedFor = sessionId;
			drafts = {};
			offer = null;
		} catch (e) {
			if (run === generation) error = describeError(e, 'Could not load the detected mobs.');
		}
	}

	async function settle(mob: ReviewedMob, target: MobDetectionTarget) {
		if (busy !== null) return;
		const sessionId = options.sessionId();
		const key = mobKey(mob.mob);
		busy = key;
		error = null;
		notice = null;
		offer = null;
		try {
			const result = await settleMobDetection(sessionId, mob.mob, target);
			if (options.sessionId() !== sessionId) return;
			if (result.kind === 'didYouMean') {
				offer = { key, creature: result.creature };
				return;
			}
			generation += 1;
			mobs = result.review.mobs;
			ended = result.review.ended;
			const { [key]: _, ...rest } = drafts;
			drafts = rest;
			notice = describeSettled(result.settled);
			await options.onSettled();
		} catch (e) {
			error = describeError(e, 'Could not record that mob.');
		} finally {
			busy = null;
		}
	}

	return {
		/** The lines to show: only an ended session's. */
		get mobs() {
			return loadedFor === options.sessionId() && ended ? mobs : [];
		},
		/** The mob offered for `mob`, when its typed name read loosely. */
		offerFor(mob: ReviewedMob): DetectedCreature | null {
			return offer?.key === mobKey(mob.mob) ? offer.creature : null;
		},
		/** Accept the mob offered for `mob`. */
		acceptOffer(mob: ReviewedMob) {
			const offered = offer?.key === mobKey(mob.mob) ? offer.creature : null;
			if (!offered) return Promise.resolve();
			return settle(mob, { kind: 'creature', name: offered.name });
		},
		get error() {
			return error;
		},
		get notice() {
			return notice;
		},
		get busy() {
			return busy;
		},
		draft: draftOf,
		setDraft(mob: ReviewedMob, value: string) {
			drafts = { ...drafts, [mobKey(mob.mob)]: value };
		},
		refresh,
		/** Record every kill of the line as the mob typed for it. */
		name(mob: ReviewedMob) {
			return settle(mob, { kind: 'creature', name: draftOf(mob) });
		},
		/** Put back the declared mob each of the line's kills had. */
		restoreDeclared(mob: ReviewedMob) {
			return settle(mob, { kind: 'declared' });
		},
		dismissError() {
			error = null;
		},
	};
}

export type MobDetectionModel = ReturnType<typeof createMobDetectionModel>;
