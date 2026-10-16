/**
 * Skilling hub view model: one target ("skill up X") and one goal answered
 * four ways, one view at a time. What each route there costs (weapon and
 * mob pairs priced by the model), what your named sessions would take as
 * recorded (every skill they train, at their own pace and return), where
 * to train it (the activity recommender's modelled ranking), and the
 * cheapest skill path (the path optimiser for a profession or a family's
 * combined level, the HP optimiser for HP). A facet loads when its view is shown and what it answers has
 * changed since, so a target or goal change costs only the open view. Each
 * facet loads into its own error slot so one failure never blanks the
 * others; each load claims a generation so an out-of-order response can
 * never repopulate a newer target or goal.
 */

import { getHpOptimizer, getProfessionPathOptimizer, getSkillingForecast } from '$lib/api';
import type {
	HpOptimizerResult,
	PathOptimizerResult,
	SkillingForecastResult,
	SkillingForecastSource,
} from '$lib/api/commands.gen';
import { looterBasis, looterBasisLevel, looterLevelsFrom } from '$lib/looterBasis.svelte';
import { getPreference, setPreference } from '$lib/preferences';
import type { ProfessionLevel } from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { type CodexRankingTarget, familyByKey, targetProfessions } from './codexRankingTarget';
import { createErrorSlot } from './errorSlot.svelte';
import { createRecommenderModel } from './recommenderModel.svelte';
import {
	isStoredRoute,
	type RouteRow,
	routeKey,
	routePick,
	routeRows,
	type StoredRoute,
	sameRoute,
} from './routesView';

/** How long goal typing settles before the goal-bound facets reload. */
export const GOAL_DEBOUNCE_MS = 350;

const TARGET_PREFERENCE = 'skilling_hub_target';
const ROUTES_PREFERENCE = 'skilling_routes';
const BOOST_PREFERENCE = 'skilling_route_pill';
const VIEW_PREFERENCE = 'skilling_view';
const SESSION_WEAPON_PREFERENCE = 'skilling_session_weapon';
/** The game caps stacked skill boosts at 100%, whatever it displays. */
export const MAX_SKILL_BOOST = 100;
/** The boost a preference saved before the percent replaced the pills. */
const LEGACY_PILLS: Record<string, number> = { none: 0, fifty: 50, hundred: 100 };

/** The hub's views of the one target and goal. */
export type SkillingView = 'routes' | 'sessions' | 'activities' | 'optimiser';
export const SKILLING_VIEWS: readonly SkillingView[] = [
	'routes',
	'sessions',
	'activities',
	'optimiser',
];

/** What each view shows: Routes and Sessions read the one forecast. */
type Facet = 'forecast' | 'activities' | 'path';
const VIEW_FACET: Record<SkillingView, Facet> = {
	routes: 'forecast',
	sessions: 'forecast',
	activities: 'activities',
	optimiser: 'path',
};

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

/** A boost input's whole percent, capped at the game's 100%, or null when
 * it is not a number of zero or more. */
export function parseBoost(input: string): number | null {
	const value = Number.parseFloat(input);
	if (!Number.isFinite(value) || value < 0) return null;
	return Math.min(MAX_SKILL_BOOST, Math.round(value));
}

/** A goal input's value, or null when it is not a positive number. */
export function parseGoal(input: string): number | null {
	const value = Number.parseFloat(input);
	return Number.isFinite(value) && value > 0 ? value : null;
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
	let view = $state<SkillingView>('routes');
	/** The facets whose shown answer predates the current target or goal. */
	const stale = new Set<Facet>();

	let path = $state<PathOptimizerResult | null>(null);
	let hpPath = $state<HpOptimizerResult | null>(null);
	let pathLoading = $state(false);
	let pathError = $state<string | null>(null);

	let forecast = $state<SkillingForecastResult | null>(null);
	let forecastLoading = $state(false);
	let forecastError = $state<string | null>(null);
	let selectedKey = $state<string | null>(null);
	let selectedDefinition = $state<number | null>(null);
	/** The weapon the named sessions are narrowed to; null takes them whole. */
	let sessionWeapon = $state<string | null>(null);
	let routes = $state<StoredRoute[]>([]);
	/** The boost percent planned routes assume; null leaves it to the
	 * latest session's. */
	let skillBoost = $state<number | null>(null);
	/** The boost field while it is being typed in. */
	let boostDraft = $state<string | null>(null);

	let pathGeneration = 0;
	let forecastGeneration = 0;
	let goalTimer: ReturnType<typeof setTimeout> | undefined;
	let boostTimer: ReturnType<typeof setTimeout> | undefined;

	/** The family's member professions with their levels. */
	const members = $derived.by(() => {
		if (target.kind !== 'family') return [];
		const levels = inputs.professions();
		return (familyByKey(target.key)?.professions ?? []).map((name) => ({
			name,
			level: levels.find((prof) => prof.name === name)?.level ?? 0,
		}));
	});
	/** The target's current value: a profession's level, HP, or a family's
	 * summed level (the recommender's family target, so all three facets
	 * answer the same question). Null only with no target. */
	const current = $derived.by<number | null>(() => {
		if (target.kind === 'profession') {
			const name = target.name;
			return inputs.professions().find((prof) => prof.name === name)?.level ?? 0;
		}
		if (target.kind === 'hp') return inputs.hp();
		if (target.kind === 'family') return members.reduce((sum, member) => sum + member.level, 0);
		return null;
	});
	const goal = $derived(parseGoal(goalInput));
	/** A goal the goal-bound facets can answer: set, and above current. */
	const goalActive = $derived(current !== null && goal !== null && goal > current);
	const rows = $derived<RouteRow[]>(routeRows(forecast, routes));
	/** The chosen row, or the cheapest that answers. */
	const selectedRow = $derived<RouteRow | null>(
		rows.find((row) => row.key === selectedKey) ??
			rows.find((row) => row.premium === 0) ??
			rows.find((row) => row.status === 'ready') ??
			rows[0] ??
			null,
	);
	const sources = $derived<SkillingForecastSource[]>(forecast?.sources ?? []);
	/** The chosen named session, or the first that answers. */
	const selectedSource = $derived<SkillingForecastSource | null>(
		sources.find((source) => source.definitionId === selectedDefinition) ??
			sources.find((source) => source.status === 'ready') ??
			sources[0] ??
			null,
	);

	/** The looter level planned routes are priced at: null asks for the
	 * player's own mean, so only another basis sends its level. */
	function requestedLooterLevel(): number | null {
		if (looterBasis.current === 'three_looter_mean') return null;
		return looterBasisLevel(looterLevelsFrom(inputs.professions()), looterBasis.current);
	}

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
		const professions = targetProfessions(current);
		if (professions.length === 0 || !goalActive || goal === null) {
			pathLoading = false;
			return;
		}
		pathLoading = true;
		try {
			const loaded = await getProfessionPathOptimizer(professions, { targetLevel: goal });
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
		if (current.kind === 'none' || !goalActive || goal === null) {
			forecastLoading = false;
			return;
		}
		forecastLoading = true;
		try {
			const planned = {
				routes: routes.map(routePick),
				skillBoostPercent: skillBoost,
				sessionWeapon,
				looterLevel: requestedLooterLevel(),
			};
			const loaded = await getSkillingForecast(
				current.kind === 'hp'
					? { target: 'hp', goal, ...planned }
					: { target: 'profession', professions: targetProfessions(current), goal, ...planned },
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

	/** Load the open view's facet if what it answers has moved. */
	function refreshView() {
		const facet = VIEW_FACET[view];
		if (!stale.delete(facet)) return;
		if (facet === 'activities') void activities.load(target);
		else if (facet === 'path') void loadPath();
		else void loadForecast();
	}

	function loadGoalBound() {
		clearTimeout(goalTimer);
		goalTimer = undefined;
		stale.add('path');
		stale.add('forecast');
		refreshView();
	}

	/** Pick a target: each view answers it at its default goal when shown. */
	function setTarget(next: CodexRankingTarget) {
		target = next;
		selectedKey = null;
		hpPath = null;
		goalInput = current === null ? '' : String(defaultGoal(current));
		stale.add('activities');
		loadGoalBound();
		void setPreference(TARGET_PREFERENCE, next);
	}

	/** Show one view, loading it if it is behind the target or goal. */
	function setView(next: SkillingView) {
		view = next;
		void setPreference(VIEW_PREFERENCE, next);
		refreshView();
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

	function selectRow(key: string) {
		selectedKey = key;
	}

	function selectSource(definitionId: number) {
		selectedDefinition = definitionId;
	}

	/** Narrow the named sessions to one weapon's part, or take them whole. */
	function setSessionWeapon(weapon: string | null) {
		sessionWeapon = weapon;
		selectedDefinition = null;
		void setPreference(SESSION_WEAPON_PREFERENCE, weapon);
		void loadForecast();
	}

	/** Add a planned route (a repeat of one already listed is ignored) and
	 * show it. */
	function addRoute(route: StoredRoute) {
		const existing = routes.findIndex((other) => sameRoute(other, route));
		if (existing === -1) {
			routes = [...routes, route];
			void setPreference(ROUTES_PREFERENCE, routes);
		}
		selectedKey = routeKey(existing === -1 ? routes.length - 1 : existing);
		void loadForecast();
	}

	/** Swap a route's weapon or mob in place. */
	function updateRoute(index: number, route: StoredRoute) {
		routes = routes.map((existing, position) => (position === index ? route : existing));
		selectedKey = routeKey(index);
		void setPreference(ROUTES_PREFERENCE, routes);
		void loadForecast();
	}

	function removeRoute(index: number) {
		routes = routes.filter((_, position) => position !== index);
		selectedKey = null;
		void setPreference(ROUTES_PREFERENCE, routes);
		void loadForecast();
	}

	/** Typing a boost re-prices once it settles; a figure past the game's
	 * cap prices at the cap. */
	function setSkillBoost(input: string) {
		boostDraft = input;
		const percent = parseBoost(input);
		clearTimeout(boostTimer);
		boostTimer = undefined;
		if (percent === null || percent === skillBoost) return;
		boostTimer = setTimeout(() => applyBoost(percent), GOAL_DEBOUNCE_MS);
	}

	/** Settle the boost field (Enter, or leaving it) on the figure priced. */
	function commitSkillBoost() {
		const percent = boostDraft === null ? null : parseBoost(boostDraft);
		boostDraft = null;
		if (percent === null) return;
		if (boostTimer !== undefined || percent !== skillBoost) applyBoost(percent);
	}

	function applyBoost(percent: number) {
		clearTimeout(boostTimer);
		boostTimer = undefined;
		skillBoost = percent;
		void setPreference(BOOST_PREFERENCE, percent);
		void loadForecast();
	}

	/** Re-price after something the routes read changed (the looter basis). */
	function reloadForecast() {
		void loadForecast();
	}

	/** Restore the last target once per page, after the profession list
	 * (which the current level reads) has loaded. */
	async function restore() {
		if (restored) return;
		restored = true;
		const [savedRoutes, savedBoost, savedView, savedWeapon] = await Promise.all([
			getPreference<unknown>(ROUTES_PREFERENCE, []),
			getPreference<unknown>(BOOST_PREFERENCE, null),
			getPreference<unknown>(VIEW_PREFERENCE, null),
			getPreference<unknown>(SESSION_WEAPON_PREFERENCE, null),
		]);
		if (typeof savedWeapon === 'string' && savedWeapon !== '') sessionWeapon = savedWeapon;
		if (Array.isArray(savedRoutes)) routes = savedRoutes.filter(isStoredRoute);
		if (typeof savedBoost === 'number') skillBoost = parseBoost(String(savedBoost));
		else if (typeof savedBoost === 'string') skillBoost = LEGACY_PILLS[savedBoost] ?? null;
		if (SKILLING_VIEWS.includes(savedView as SkillingView)) view = savedView as SkillingView;
		const saved = await getPreference<unknown>(TARGET_PREFERENCE, null);
		if (target.kind === 'none' && isTarget(saved) && saved.kind !== 'none') setTarget(saved);
	}

	function dispose() {
		clearTimeout(goalTimer);
		goalTimer = undefined;
		clearTimeout(boostTimer);
		boostTimer = undefined;
	}

	return {
		activities,
		get activitiesError() {
			return activitiesErrors.error;
		},
		get target() {
			return target;
		},
		get view() {
			return view;
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
		get rows() {
			return rows;
		},
		get selectedRow() {
			return selectedRow;
		},
		get routes() {
			return routes;
		},
		/** Whether Routes prices this target: HP, or a profession a weapon
		 * trains. Taken as yes until the forecast says otherwise. */
		get huntingTarget() {
			return forecast?.huntingTarget ?? true;
		},
		get sources() {
			return sources;
		},
		get sessionWeapon() {
			return sessionWeapon;
		},
		/** The weapons the named sessions used, most used first. */
		get sessionWeapons() {
			return forecast?.sessionWeapons ?? [];
		},
		get selectedSource() {
			return selectedSource;
		},
		/** The boost field: what is being typed, else the percent priced. */
		get boostInput() {
			return boostDraft ?? String(skillBoost ?? forecast?.skillBoostPercent ?? '');
		},

		setTarget,
		setView,
		setGoal,
		commitGoal,
		selectRow,
		selectSource,
		setSessionWeapon,
		addRoute,
		updateRoute,
		removeRoute,
		setSkillBoost,
		commitSkillBoost,
		reloadForecast,
		restore,
		dispose,
	};
}

export type SkillingModel = ReturnType<typeof createSkillingModel>;
