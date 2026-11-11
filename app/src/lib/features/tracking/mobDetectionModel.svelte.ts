/**
 * Runes-native state for the after-session review of detected creatures.
 *
 * The review is read for the session shown and re-read when the session
 * changes; a read that lands after the player moved to another session is
 * dropped. Only an ended session's review is shown. Settling a group answers
 * with the refreshed review, applied here, and tells the owner so the
 * session's own figures re-read; a name typed only loosely like a creature
 * changes nothing and offers that creature, to accept in one action. While
 * a settlement is being saved every other one is refused at once.
 */

import {
	type DetectedCreature,
	getMobDetections,
	type MobDetectionGroup,
	type MobDetectionTarget,
	settleMobDetection,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { describeSettled, groupKey } from './mobDetection';

export interface MobDetectionOptions {
	/** The session shown. */
	sessionId: () => string;
	/** A group was settled: the session's figures changed. */
	onSettled: () => void | Promise<void>;
}

export function createMobDetectionModel(options: MobDetectionOptions) {
	let groups = $state<MobDetectionGroup[]>([]);
	let loadedFor = $state<string | null>(null);
	let error = $state<string | null>(null);
	let notice = $state<string | null>(null);
	let busy = $state<string | null>(null);
	let drafts = $state<Record<string, string>>({});
	let ended = $state(false);
	/** The creature offered for a group whose typed name read only loosely. */
	let offer = $state<{ key: string; creature: DetectedCreature } | null>(null);
	let generation = 0;

	function draftOf(group: MobDetectionGroup): string {
		return drafts[groupKey(group.group)] ?? group.group.creature.name;
	}

	async function refresh() {
		const sessionId = options.sessionId();
		const run = ++generation;
		try {
			const review = await getMobDetections(sessionId);
			if (run !== generation || options.sessionId() !== sessionId) return;
			groups = review.groups;
			ended = review.ended;
			loadedFor = sessionId;
			drafts = {};
			offer = null;
		} catch (e) {
			if (run === generation) error = describeError(e, 'Could not load the detected creatures.');
		}
	}

	async function settle(group: MobDetectionGroup, target: MobDetectionTarget) {
		if (busy !== null) return;
		const sessionId = options.sessionId();
		const key = groupKey(group.group);
		busy = key;
		error = null;
		notice = null;
		offer = null;
		try {
			const result = await settleMobDetection(sessionId, group.group, target);
			if (options.sessionId() !== sessionId) return;
			if (result.kind === 'didYouMean') {
				offer = { key, creature: result.creature };
				return;
			}
			generation += 1;
			groups = result.review.groups;
			ended = result.review.ended;
			const { [key]: _, ...rest } = drafts;
			drafts = rest;
			notice = describeSettled(result.settled);
			await options.onSettled();
		} catch (e) {
			error = describeError(e, 'Could not record that creature.');
		} finally {
			busy = null;
		}
	}

	return {
		/** The groups to show: only an ended session's. */
		get groups() {
			return loadedFor === options.sessionId() && ended ? groups : [];
		},
		/** The creature offered for `group`, when its typed name read loosely. */
		offerFor(group: MobDetectionGroup): DetectedCreature | null {
			return offer?.key === groupKey(group.group) ? offer.creature : null;
		},
		/** Accept the creature offered for `group`. */
		acceptOffer(group: MobDetectionGroup) {
			const offered = offer?.key === groupKey(group.group) ? offer.creature : null;
			if (!offered) return Promise.resolve();
			return settle(group, { kind: 'creature', name: offered.name });
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
		setDraft(group: MobDetectionGroup, value: string) {
			drafts = { ...drafts, [groupKey(group.group)]: value };
		},
		refresh,
		/** Record every kill of the group as the creature typed for it. */
		name(group: MobDetectionGroup) {
			return settle(group, { kind: 'creature', name: draftOf(group) });
		},
		/** Put back the declared mob each of the group's kills had. */
		restoreDeclared(group: MobDetectionGroup) {
			return settle(group, { kind: 'declared' });
		},
		dismissError() {
			error = null;
		},
	};
}

export type MobDetectionModel = ReturnType<typeof createMobDetectionModel>;
