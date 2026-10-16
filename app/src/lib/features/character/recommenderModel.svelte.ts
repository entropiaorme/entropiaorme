/**
 * Activity recommender view model: the ranking target, the ranked
 * arbitrage candidates with their projected gain series, and the
 * selected candidate the chart renders. The player can hide activities
 * they cannot pursue directly: a hidden activity trails the ranking,
 * is never the default selection, and stays hidden for every target
 * and across app restarts (a UI preference). Presentation lives in the
 * feature components; they compose over this state. Failures land in
 * the error slot the caller hands it (the skilling hub gives it one of
 * its own).
 */

import { getActivityRecommender } from '$lib/api';
import type { ActivityRecommenderResult, RecommenderActivity } from '$lib/api/commands.gen';
import { describeError } from '$lib/view/errorState';
import { createHiddenSet, orderByVisibility } from '$lib/view/hiddenSet.svelte';
import { type CodexRankingTarget, targetProfessions } from './codexRankingTarget';
import type { PageErrorSlot } from './errorSlot.svelte';

const HIDDEN_PREFERENCE = 'recommender_hidden_activities';

/** True when `a` ranks ahead of `b`: fewer PES to add one, then more gain
 * at the cap, then by name (the ranking's own order). */
function ranksBefore(a: RecommenderActivity, b: RecommenderActivity): boolean {
	if (a.pesToPlusOne !== null && b.pesToPlusOne !== null && a.pesToPlusOne !== b.pesToPlusOne) {
		return a.pesToPlusOne < b.pesToPlusOne;
	}
	if ((a.pesToPlusOne === null) !== (b.pesToPlusOne === null)) return a.pesToPlusOne !== null;
	if (a.gainAtCap !== b.gainAtCap) return a.gainAtCap > b.gainAtCap;
	return a.activity < b.activity;
}

/** The ranking with the target's own activity in its place, so how close
 * the others come to training it directly reads off the list. */
export function withDirect(
	candidates: RecommenderActivity[],
	direct: RecommenderActivity | null,
): RecommenderActivity[] {
	if (!direct) return candidates;
	const at = candidates.findIndex((candidate) => ranksBefore(direct, candidate));
	return at === -1
		? [...candidates, direct]
		: [...candidates.slice(0, at), direct, ...candidates.slice(at)];
}

export function createRecommenderModel(errors: PageErrorSlot) {
	let target = $state<CodexRankingTarget>({ kind: 'none' });
	let result = $state<ActivityRecommenderResult | null>(null);
	let selectedActivity = $state('');
	let loading = $state(false);
	const hidden = createHiddenSet(HIDDEN_PREFERENCE);

	const direct = $derived(result?.direct ?? null);
	const candidates = $derived(
		orderByVisibility(withDirect(result?.candidates ?? [], direct), (c) => hidden.has(c.activity)),
	);
	const visibleCount = $derived(candidates.filter((c) => !hidden.has(c.activity)).length);
	const selected = $derived<RecommenderActivity | null>(
		candidates.find((candidate) => candidate.activity === selectedActivity) ??
			candidates.find((candidate) => candidate.activity === firstVisible()) ??
			null,
	);

	/** The first activity worth showing: the top one not hidden, other than
	 * the target's own (the chart draws that beside it anyway). */
	function firstVisible(): string {
		const shown = candidates.filter((c) => !hidden.has(c.activity));
		return (
			shown.find((c) => c.activity !== direct?.activity)?.activity ??
			shown[0]?.activity ??
			candidates[0]?.activity ??
			''
		);
	}

	// Each load claims a generation; a resolution from a superseded load
	// (a newer target picked meanwhile, including 'none') is discarded so
	// out-of-order responses can never repopulate the current state.
	let generation = 0;

	async function load(next: CodexRankingTarget) {
		const claimed = ++generation;
		target = next;
		result = null;
		selectedActivity = '';
		errors.error = null;
		if (next.kind === 'none') {
			loading = false;
			return;
		}
		loading = true;
		try {
			await hidden.load();
			const loaded = await getActivityRecommender(
				next.kind === 'hp'
					? { target: 'hp', professions: [] }
					: { target: 'profession', professions: targetProfessions(next) },
			);
			if (claimed !== generation) return;
			if (loaded.error) {
				errors.error = loaded.error;
				return;
			}
			result = loaded;
			selectedActivity = firstVisible();
		} catch (e) {
			if (claimed !== generation) return;
			errors.error = describeError(e, 'Failed to load the activity recommender');
		} finally {
			if (claimed === generation) loading = false;
		}
	}

	function select(activity: string) {
		selectedActivity = activity;
	}

	/** Hide an activity from the ranking, for every target, persistently.
	 * Hiding the one on show moves the chart to the top visible one. */
	function hide(activity: string) {
		hidden.hide(activity);
		if (selected?.activity === activity) selectedActivity = firstVisible();
	}

	return {
		get target() {
			return target;
		},
		get result() {
			return result;
		},
		set result(value: ActivityRecommenderResult | null) {
			result = value;
		},
		get candidates() {
			return candidates;
		},
		/** The target's own activity, ranked among the others; null for HP
		 * and family targets, which have none. */
		get direct() {
			return direct;
		},
		isDirect(activity: string) {
			return direct?.activity === activity;
		},
		get selected() {
			return selected;
		},
		get selectedActivity() {
			return selectedActivity;
		},
		get loading() {
			return loading;
		},
		get hidden() {
			return hidden.names;
		},
		/** How many ranked activities are not hidden. */
		get visibleCount() {
			return visibleCount;
		},

		load,
		select,
		isHidden: hidden.has,
		hide,
		restore: hidden.restore,
	};
}

export type RecommenderModel = ReturnType<typeof createRecommenderModel>;
