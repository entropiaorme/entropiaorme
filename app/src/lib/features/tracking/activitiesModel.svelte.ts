/**
 * The Activities control's view model: what the session offers, what is
 * standing on it, and the two writes that move between them.
 *
 * One control replaced the separate quest-focus picker and segment
 * field, so there is one model rather than two halves. It owns the
 * fetched offerings and the write state; the satellite-window plumbing
 * (anchors, popup lifecycle) stays with the overlay route, and the
 * strip-level readout (visible, the ready cue, the standing chips) comes
 * off the tracking snapshot rather than from here, so a chip never waits
 * on a round trip.
 *
 * A tap is the exclusive switch across BOTH kinds: it seals whatever was
 * standing, because the control offers quests and segments together and
 * "this is what I am doing now" means one thing. Co-activation is the
 * separate, deliberate gesture.
 */

import {
	type ActivityOption,
	type ActivityOptionsResult,
	ApiError,
	type QuestHandInState,
} from '$lib/api';

export interface ActivitiesModelDeps {
	/** Read what the control offers right now. */
	readOptions: () => Promise<ActivityOptionsResult>;
	/** Declare a quest's stretch: exclusive switch unless additive. */
	activateQuest: (questId: number, additive: boolean) => Promise<unknown>;
	/** Declare a named slice; the name is promoted into the session's
	 * roster, so it is a row of its own next time. */
	activateSegment: (label: string, additive: boolean) => Promise<unknown>;
	/** End one standing quest stretch. */
	deactivateQuest: (questId: number) => Promise<unknown>;
	/** End one standing segment, matched by its name. */
	deactivateSegment: (label: string) => Promise<unknown>;
	/** Begin or resume exact-clump review for a manual-hand-in quest. */
	beginHandIn: (questId: number) => Promise<QuestHandInState>;
	/** Re-read the snapshot after a successful write. */
	refresh: () => Promise<unknown>;
}

function describe(error: unknown, fallback: string): string {
	return error instanceof ApiError || error instanceof Error ? error.message : fallback;
}

export function createActivitiesModel(deps: ActivitiesModelDeps) {
	// The last fetched offerings. Null until the control is first
	// opened; the strip renders from the snapshot meanwhile.
	let options = $state<ActivityOptionsResult | null>(null);
	// The in-flight guard for a declaration. Enforced in `write`, not
	// merely reported: the menu is a satellite window whose rows stay
	// clickable while a write lands, so a second tap would otherwise
	// race the first on the tracker's standing set.
	let saving = $state(false);
	// One channel for every failure, surfaced beside the control rather
	// than swallowed.
	let error = $state<string | null>(null);
	// The free-text buffer, live only where the definition opts into
	// naming segments in play.
	let segmentDraft = $state('');

	/** Fetch the offerings. Returns them so the caller can size and
	 * present the menu in the same motion; null on a failed read, with
	 * the message on the error channel. */
	async function load(): Promise<ActivityOptionsResult | null> {
		try {
			options = await deps.readOptions();
			error = null;
			return options;
		} catch (e) {
			error = describe(e, 'Failed to read the activities');
			return null;
		}
	}

	/** Resolve the stable key the satellite emitted against the latest menu read. */
	function find(key: string): ActivityOption | null {
		return options?.options.find((option) => option.key === key) ?? null;
	}

	/** Run a write, then refresh the snapshot and the offerings so the
	 * menu re-presents with the standing set it produced. */
	async function write(action: () => Promise<unknown>, fallback: string): Promise<boolean> {
		if (saving) return false;
		saving = true;
		error = null;
		try {
			await action();
			await deps.refresh();
			await load();
			return true;
		} catch (e) {
			error = describe(e, fallback);
			return false;
		} finally {
			saving = false;
		}
	}

	/** A row's tap: end it when it is standing, declare it otherwise.
	 * Tapping a standing row to end it mirrors every other toggle in the
	 * app, and is the only way to record a stretch of nothing in
	 * particular. */
	async function toggle(option: ActivityOption): Promise<boolean> {
		if (option.active) {
			return option.kind === 'segment'
				? write(() => deps.deactivateSegment(option.name), 'Failed to end the activity')
				: write(() => deps.deactivateQuest(Number(option.questId)), 'Failed to end the activity');
		}
		return declare(option, false);
	}

	/** Declare a row, exclusively (the tap) or alongside what is already
	 * standing (the co-activate affordance). */
	async function declare(option: ActivityOption, additive: boolean): Promise<boolean> {
		if (option.kind === 'segment') {
			return write(
				() => deps.activateSegment(option.name, additive),
				'Failed to declare the activity',
			);
		}
		if (option.questId === null) return false;
		const questId = Number(option.questId);
		return write(() => deps.activateQuest(questId, additive), 'Failed to declare the activity');
	}

	/** Declare the typed name. There is no unnamed slice: a stretch worth
	 * recording is worth saying what it is, so a blank draft declares
	 * nothing rather than opening an auto-numbered one. */
	async function declareTyped(additive = false): Promise<boolean> {
		const label = segmentDraft.trim();
		if (!label) return false;
		const applied = await write(
			() => deps.activateSegment(label, additive),
			'Failed to declare the activity',
		);
		if (applied) segmentDraft = '';
		return applied;
	}

	/** Open the persisted hand-in review and refresh the live snapshot so its
	 * waiting state is immediately truthful on the parent overlay. */
	async function beginHandIn(option: ActivityOption): Promise<QuestHandInState | null> {
		if (saving || option.questId === null) return null;
		saving = true;
		error = null;
		try {
			const handIn = await deps.beginHandIn(Number(option.questId));
			await deps.refresh();
			return handIn;
		} catch (e) {
			error = describe(e, 'Failed to begin the quest hand-in');
			return null;
		} finally {
			saving = false;
		}
	}

	async function refresh(): Promise<void> {
		await deps.refresh();
		await load();
	}

	return {
		get options() {
			return options;
		},
		get saving() {
			return saving;
		},
		get error() {
			return error;
		},
		get segmentDraft() {
			return segmentDraft;
		},
		set segmentDraft(value: string) {
			segmentDraft = value;
		},

		load,
		find,
		toggle,
		declare,
		declareTyped,
		beginHandIn,
		refresh,
	};
}

export type ActivitiesModel = ReturnType<typeof createActivitiesModel>;
