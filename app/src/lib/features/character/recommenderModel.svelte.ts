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
import { getPreference, setPreference } from '$lib/preferences';
import { describeError } from '$lib/view/errorState';
import { type CodexRankingTarget, targetProfessions } from './codexRankingTarget';
import type { PageErrorSlot } from './errorSlot.svelte';

const HIDDEN_PREFERENCE = 'recommender_hidden_activities';

/** The ranking with hidden activities moved to the end, each part keeping
 * its ranked order. */
export function orderByVisibility(
	candidates: RecommenderActivity[],
	hidden: readonly string[],
): RecommenderActivity[] {
	const isHidden = (candidate: RecommenderActivity) => hidden.includes(candidate.activity);
	return [
		...candidates.filter((candidate) => !isHidden(candidate)),
		...candidates.filter(isHidden),
	];
}

export function createRecommenderModel(errors: PageErrorSlot) {
	let target = $state<CodexRankingTarget>({ kind: 'none' });
	let result = $state<ActivityRecommenderResult | null>(null);
	let selectedActivity = $state('');
	let loading = $state(false);
	let hidden = $state<string[]>([]);
	let hiddenLoaded: Promise<void> | null = null;

	const candidates = $derived(orderByVisibility(result?.candidates ?? [], hidden));
	const visibleCount = $derived(candidates.filter((c) => !hidden.includes(c.activity)).length);
	const selected = $derived<RecommenderActivity | null>(
		candidates.find((candidate) => candidate.activity === selectedActivity) ??
			candidates[0] ??
			null,
	);

	function loadHidden(): Promise<void> {
		hiddenLoaded ??= getPreference<unknown>(HIDDEN_PREFERENCE, []).then((saved) => {
			if (Array.isArray(saved)) hidden = saved.filter((name) => typeof name === 'string');
		});
		return hiddenLoaded;
	}

	/** The first activity worth showing: the top one not hidden. */
	function firstVisible(): string {
		return (
			candidates.find((c) => !hidden.includes(c.activity))?.activity ??
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
			await loadHidden();
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

	function isHidden(activity: string): boolean {
		return hidden.includes(activity);
	}

	/** Hide an activity from the ranking, for every target, persistently.
	 * Hiding the one on show moves the chart to the top visible one. */
	function hide(activity: string) {
		if (hidden.includes(activity)) return;
		hidden = [...hidden, activity];
		if (selected?.activity === activity) selectedActivity = firstVisible();
		void setPreference(HIDDEN_PREFERENCE, hidden);
	}

	function restore(activity: string) {
		if (!hidden.includes(activity)) return;
		hidden = hidden.filter((name) => name !== activity);
		void setPreference(HIDDEN_PREFERENCE, hidden);
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
			return hidden;
		},
		/** How many ranked activities are not hidden. */
		get visibleCount() {
			return visibleCount;
		},

		load,
		select,
		isHidden,
		hide,
		restore,
	};
}

export type RecommenderModel = ReturnType<typeof createRecommenderModel>;
