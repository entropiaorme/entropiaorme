/**
 * Skilling hub view model: one target ("skill up X") and one goal answered
 * three ways at once. Where to train it (the activity recommender's
 * modelled ranking), what your own named sessions say it takes (the
 * skilling forecast, from recorded play and its realised markup), and the
 * cheapest skill path (the path optimiser for a profession, the HP
 * optimiser for HP). Each facet loads into its own error slot so one
 * failure never blanks the others; each load claims a generation so an
 * out-of-order response can never repopulate a newer target or goal.
 */

import { getHpOptimizer, getProfessionPathOptimizer, getSkillingForecast } from '$lib/api';
import type {
	HpOptimizerResult,
	PathOptimizerResult,
	SkillingForecastResult,
	SkillingForecastSource,
} from '$lib/api/commands.gen';
import { getPreference, setPreference } from '$lib/preferences';
import type { ProfessionLevel } from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { type CodexRankingTarget, familyByKey } from './codexRankingTarget';
import { createErrorSlot } from './errorSlot.svelte';
import { createRecommenderModel } from './recommenderModel.svelte';

/** How long goal typing settles before the goal-bound facets reload. */
export const GOAL_DEBOUNCE_MS = 350;

const TARGET_PREFERENCE = 'skilling_hub_target';

export interface SkillingInputs {
	/** The calibrated profession levels (the character surface's list). */
	professions: () => ProfessionLevel[];
	/** Current HP as the Stats panel reads it. */
	hp: () => number;
}

/** The default goal: the next whole level (or HP point) above `current`. */
export function defaultGoal(current: number): number {
	return Math.floor(current) + 1;
}

/** A goal input's value, or null when it is not a positive number. */
export function parseGoal(input: string): number | null {
	const value = Number.parseFloat(input);
	return Number.isFinite(value) && value > 0 ? value : null;
}

/** The source a forecast shows by default: the quickest one that answers. */
export function defaultSource(
	result: SkillingForecastResult | null,
): SkillingForecastSource | null {
	if (!result || result.sources.length === 0) return null;
	return result.sources.find((source) => source.status === 'ready') ?? result.sources[0];
}

function isTarget(value: unknown): value is CodexRankingTarget {
	if (typeof value !== 'object' || value === null || !('kind' in value)) return false;
	const target = value as { kind: unknown; name?: unknown; key?: unknown };
	switch (target.kind) {
		case 'none':
		case 'hp':
			return true;
		case 'profession':
			return typeof target.name === 'string';
		case 'family':
			return typeof target.key === 'string' && familyByKey(target.key) !== undefined;
		default:
			return false;
	}
}

export function createSkillingModel(inputs: SkillingInputs) {
	const activitiesErrors = createErrorSlot();
	const activities = createRecommenderModel(activitiesErrors);

	let target = $state<CodexRankingTarget>({ kind: 'none' });
	let goalInput = $state('');
	let restored = $state(false);

	let path = $state<PathOptimizerResult | null>(null);
	let hpPath = $state<HpOptimizerResult | null>(null);
	let pathLoading = $state(false);
	let pathError = $state<string | null>(null);

	let forecast = $state<SkillingForecastResult | null>(null);
	let forecastLoading = $state(false);
	let forecastError = $state<string | null>(null);
	let selectedSourceId = $state<number | null>(null);

	let pathGeneration = 0;
	let forecastGeneration = 0;
	let goalTimer: ReturnType<typeof setTimeout> | undefined;

	/** The target's current value; null for a family (no single level). */
	const current = $derived.by<number | null>(() => {
		if (target.kind === 'profession') {
			const name = target.name;
			return inputs.professions().find((prof) => prof.name === name)?.level ?? 0;
		}
		if (target.kind === 'hp') return inputs.hp();
		return null;
	});
	const goal = $derived(parseGoal(goalInput));
	/** A goal the goal-bound facets can answer: set, and above current. */
	const goalActive = $derived(current !== null && goal !== null && goal > current);
	/** The family's member professions with their levels. */
	const members = $derived.by(() => {
		if (target.kind !== 'family') return [];
		const levels = inputs.professions();
		return (familyByKey(target.key)?.professions ?? []).map((name) => ({
			name,
			level: levels.find((prof) => prof.name === name)?.level ?? 0,
		}));
	});
	const sources = $derived(forecast?.sources ?? []);
	const selectedSource = $derived<SkillingForecastSource | null>(
		sources.find((source) => source.definitionId === selectedSourceId) ?? defaultSource(forecast),
	);

	async function loadPath() {
		const claimed = ++pathGeneration;
		pathError = null;
		const current = target;
		if (current.kind === 'hp') {
			if (hpPath) {
				pathLoading = false;
				return;
			}
			pathLoading = true;
			try {
				const loaded = await getHpOptimizer();
				if (claimed === pathGeneration) hpPath = loaded;
			} catch (e) {
				if (claimed === pathGeneration) {
					pathError = describeError(e, 'Failed to load the cheapest HP skills');
				}
			} finally {
				if (claimed === pathGeneration) pathLoading = false;
			}
			return;
		}
		path = null;
		if (current.kind !== 'profession' || !goalActive || goal === null) {
			pathLoading = false;
			return;
		}
		pathLoading = true;
		try {
			const loaded = await getProfessionPathOptimizer(current.name, { targetLevel: goal });
			if (claimed !== pathGeneration) return;
			if (loaded.error) {
				pathError = loaded.error;
				return;
			}
			path = loaded;
		} catch (e) {
			if (claimed === pathGeneration) {
				pathError = describeError(e, 'Failed to compute the skill path');
			}
		} finally {
			if (claimed === pathGeneration) pathLoading = false;
		}
	}

	async function loadForecast() {
		const claimed = ++forecastGeneration;
		forecastError = null;
		forecast = null;
		const current = target;
		if ((current.kind !== 'profession' && current.kind !== 'hp') || !goalActive || goal === null) {
			forecastLoading = false;
			return;
		}
		forecastLoading = true;
		try {
			const loaded = await getSkillingForecast(
				current.kind === 'hp'
					? { target: 'hp', goal }
					: { target: 'profession', profession: current.name, goal },
			);
			if (claimed !== forecastGeneration) return;
			if (loaded.error) {
				forecastError = loaded.error;
				return;
			}
			forecast = loaded;
		} catch (e) {
			if (claimed === forecastGeneration) {
				forecastError = describeError(e, 'Failed to forecast from your sessions');
			}
		} finally {
			if (claimed === forecastGeneration) forecastLoading = false;
		}
	}

	function loadGoalBound() {
		clearTimeout(goalTimer);
		goalTimer = undefined;
		void loadPath();
		void loadForecast();
	}

	/** Pick a target: every facet reloads against it at its default goal. */
	function setTarget(next: CodexRankingTarget) {
		target = next;
		selectedSourceId = null;
		hpPath = null;
		goalInput = current === null ? '' : String(defaultGoal(current));
		void activities.load(next);
		loadGoalBound();
		void setPreference(TARGET_PREFERENCE, next);
	}

	/** Typing a goal reloads the goal-bound facets once it settles. */
	function setGoal(input: string) {
		goalInput = input;
		clearTimeout(goalTimer);
		goalTimer = setTimeout(loadGoalBound, GOAL_DEBOUNCE_MS);
	}

	/** Reload immediately (Enter, or leaving the field). */
	function commitGoal() {
		if (goalTimer !== undefined) loadGoalBound();
	}

	function selectSource(definitionId: number) {
		selectedSourceId = definitionId;
	}

	/** Restore the last target once per page, after the profession list
	 * (which the current level reads) has loaded. */
	async function restore() {
		if (restored) return;
		restored = true;
		const saved = await getPreference<unknown>(TARGET_PREFERENCE, null);
		if (target.kind === 'none' && isTarget(saved) && saved.kind !== 'none') setTarget(saved);
	}

	function dispose() {
		clearTimeout(goalTimer);
		goalTimer = undefined;
	}

	return {
		activities,
		get activitiesError() {
			return activitiesErrors.error;
		},
		get target() {
			return target;
		},
		get goalInput() {
			return goalInput;
		},
		get current() {
			return current;
		},
		get goal() {
			return goal;
		},
		get goalActive() {
			return goalActive;
		},
		get members() {
			return members;
		},

		get path() {
			return path;
		},
		get hpPath() {
			return hpPath;
		},
		get pathLoading() {
			return pathLoading;
		},
		get pathError() {
			return pathError;
		},

		get forecast() {
			return forecast;
		},
		get forecastLoading() {
			return forecastLoading;
		},
		get forecastError() {
			return forecastError;
		},
		get sources() {
			return sources;
		},
		get selectedSource() {
			return selectedSource;
		},

		setTarget,
		setGoal,
		commitGoal,
		selectSource,
		restore,
		dispose,
	};
}

export type SkillingModel = ReturnType<typeof createSkillingModel>;
