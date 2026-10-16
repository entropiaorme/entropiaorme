import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	ActivityRecommenderResult,
	HpOptimizerResult,
	PathOptimizerResult,
	SkillingForecastResult,
} from '$lib/api/commands.gen';
import { setLooterBasis } from '$lib/looterBasis.svelte';
import type { ProfessionLevel } from '$lib/types/analytics';
import { forecastResult, sourceForecast } from './__fixtures__/skilling';
import {
	createSkillingModel,
	defaultGoal,
	GOAL_DEBOUNCE_MS,
	parseBoost,
	parseGoal,
} from './skillingModel.svelte';

vi.mock('$lib/api', () => ({
	getActivityRecommender: vi.fn(),
	getProfessionPathOptimizer: vi.fn(),
	getHpOptimizer: vi.fn(),
	getSkillingForecast: vi.fn(),
}));

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import * as api from '$lib/api';
import * as preferences from '$lib/preferences';

const mocked = vi.mocked(api);
const prefs = vi.mocked(preferences);

const PROFESSIONS: ProfessionLevel[] = [
	{
		name: 'Laser Sniper (Hit)',
		level: 42.37,
		anchorLevel: null,
		gainSinceAnchor: null,
		category: 'Combat',
	},
	{
		name: 'Animal Looter',
		level: 30.5,
		anchorLevel: null,
		gainSinceAnchor: null,
		category: 'Looting',
	},
	{
		name: 'Mutant Looter',
		level: 20.25,
		anchorLevel: null,
		gainSinceAnchor: null,
		category: 'Looting',
	},
];

function recommender(): ActivityRecommenderResult {
	return { pesCap: 1000, sampleStep: 20, direct: null, candidates: [] };
}

function path(overrides: Partial<PathOptimizerResult> = {}): PathOptimizerResult {
	return {
		allocations: [],
		attributes: [],
		profession: 'Laser Sniper (Hit)',
		mode: 'target',
		inputTargetLevel: 43,
		inputPedBudget: null,
		currentLevel: 42.37,
		endLevel: 43,
		professionLevelsGained: 0.63,
		totalPed: 120,
		excluded: [],
		...overrides,
	};
}

function hpResult(): HpOptimizerResult {
	return { currentHp: 142, skills: [], attributes: [] };
}

function forecast(sources = [sourceForecast()]): SkillingForecastResult {
	return forecastResult({ current: 42.37, goal: 43, sources });
}

function makeModel() {
	return createSkillingModel({ professions: () => PROFESSIONS, hp: () => 142 });
}

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getActivityRecommender.mockResolvedValue(recommender());
	mocked.getProfessionPathOptimizer.mockResolvedValue(path());
	mocked.getHpOptimizer.mockResolvedValue(hpResult());
	mocked.getSkillingForecast.mockResolvedValue(forecast());
	prefs.getPreference.mockResolvedValue(null);
	prefs.setPreference.mockResolvedValue(undefined);
});

afterEach(() => {
	vi.useRealTimers();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** What a forecast asks with no planned route, the boost left to the
 * latest session, the looter basis at the player's own mean, and the
 * named sessions taken whole. */
const UNPLANNED = { routes: [], skillBoostPercent: null, looterLevel: null, sessionWeapon: null };

const SWORD_ON_ROBBERS = {
	weaponId: 'sword',
	weaponName: 'Laconian Sword',
	species: 'Bank Robber',
	maturity: 'Default',
	display: 'Default Bank Robber',
};

describe('the pure helpers', () => {
	it('defaults the goal to the next whole value', () => {
		expect(defaultGoal(42.37)).toBe(43);
		expect(defaultGoal(142)).toBe(143);
	});

	it('parses only a positive goal', () => {
		expect(parseGoal('43.5')).toBe(43.5);
		expect(parseGoal('')).toBeNull();
		expect(parseGoal('0')).toBeNull();
		expect(parseGoal('-2')).toBeNull();
		expect(parseGoal('abc')).toBeNull();
	});

	it('reads a boost as a whole percent, capped at the game limit', () => {
		expect(parseBoost('0')).toBe(0);
		expect(parseBoost('37.6')).toBe(38);
		expect(parseBoost('150')).toBe(100);
		expect(parseBoost('-1')).toBeNull();
		expect(parseBoost('')).toBeNull();
	});
});

describe('setTarget', () => {
	it('answers a profession at its default goal in the open view only', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		expect(model.view).toBe('routes');
		expect(model.current).toBe(42.37);
		expect(model.goalInput).toBe('43');
		expect(model.goalActive).toBe(true);
		await settle();

		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({
			target: 'profession',
			professions: ['Laser Sniper (Hit)'],
			goal: 43,
			...UNPLANNED,
		});
		expect(mocked.getActivityRecommender).not.toHaveBeenCalled();
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
		expect(model.selectedSource?.name).toBe('Carabok Skilling');
		expect(model.forecastLoading).toBe(false);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_hub_target', {
			kind: 'profession',
			name: 'Laser Sniper (Hit)',
		});
	});

	it('answers HP from the HP optimiser and an HP forecast', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'hp' });
		expect(model.goalInput).toBe('143');
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({
			target: 'hp',
			goal: 143,
			...UNPLANNED,
		});
		model.setView('optimiser');
		await settle();
		expect(mocked.getHpOptimizer).toHaveBeenCalledTimes(1);
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
		expect(model.hpPath?.currentHp).toBe(142);
	});

	it('answers a family on its combined level in each view', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'family', key: 'looter' });
		// 30.5 + 20.25 + 0 (Robot Looter, uncalibrated) = 50.75.
		expect(model.current).toBe(50.75);
		expect(model.goalInput).toBe('51');
		expect(model.members.map((member) => [member.name, member.level])).toEqual([
			['Animal Looter', 30.5],
			['Mutant Looter', 20.25],
			['Robot Looter', 0],
		]);
		const looters = ['Animal Looter', 'Mutant Looter', 'Robot Looter'];
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({
			target: 'profession',
			professions: looters,
			goal: 51,
			...UNPLANNED,
		});
		model.setView('activities');
		model.setView('optimiser');
		await settle();
		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'profession',
			professions: looters,
		});
		expect(mocked.getProfessionPathOptimizer).toHaveBeenCalledWith(looters, { targetLevel: 51 });
	});

	it('discards a response that a newer target superseded', async () => {
		let resolveFirst!: (value: SkillingForecastResult) => void;
		mocked.getSkillingForecast.mockReturnValueOnce(
			new Promise((resolve) => {
				resolveFirst = resolve;
			}),
		);
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		mocked.getSkillingForecast.mockResolvedValueOnce(forecast([sourceForecast({ name: 'Newer' })]));
		model.setTarget({ kind: 'profession', name: 'Animal Looter' });
		await settle();
		resolveFirst(forecast([sourceForecast({ name: 'Stale' })]));
		await settle();
		expect(model.selectedSource?.name).toBe('Newer');
	});
});

describe('the views', () => {
	it('loads a view when it opens, once per target and goal', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		model.setView('activities');
		await settle();
		expect(mocked.getActivityRecommender).toHaveBeenCalledTimes(1);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_view', 'activities');

		// Back to Routes, and on to Sessions, which reads the same forecast:
		// nothing is asked again.
		model.setView('routes');
		model.setView('sessions');
		model.setView('activities');
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenCalledTimes(1);
		expect(mocked.getActivityRecommender).toHaveBeenCalledTimes(1);
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
	});

	it('defers a goal change until a view that answers it opens', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		model.setView('activities');
		await vi.runAllTimersAsync();
		vi.clearAllMocks();

		model.setGoal('45');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		// The ranking does not depend on the goal, and the forecast waits.
		expect(mocked.getActivityRecommender).not.toHaveBeenCalled();
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
		model.setView('sessions');
		await vi.runAllTimersAsync();
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith(expect.objectContaining({ goal: 45 }));
	});

	it('reopens the last view', async () => {
		prefs.getPreference.mockImplementation(async (key: string) =>
			key === 'skilling_view' ? 'optimiser' : null,
		);
		const model = makeModel();
		await model.restore();
		expect(model.view).toBe('optimiser');
	});
});

describe('the goal', () => {
	it('reloads the open view once typing settles', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await vi.runAllTimersAsync();
		vi.clearAllMocks();

		model.setGoal('4');
		model.setGoal('45');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS - 1);
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);

		expect(mocked.getSkillingForecast).toHaveBeenCalledTimes(1);
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith(expect.objectContaining({ goal: 45 }));
		expect(mocked.getActivityRecommender).not.toHaveBeenCalled();
	});

	it('commits at once on demand, and only when a change is pending', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await vi.runAllTimersAsync();
		vi.clearAllMocks();

		model.commitGoal();
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
		model.setGoal('44');
		model.commitGoal();
		expect(mocked.getSkillingForecast).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		expect(mocked.getSkillingForecast).toHaveBeenCalledTimes(1);
	});

	it('asks nothing of a goal at or below where you stand', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await vi.runAllTimersAsync();
		vi.clearAllMocks();

		model.setGoal('42');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		expect(model.goalActive).toBe(false);
		expect(model.forecast).toBeNull();
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
		model.setView('optimiser');
		await vi.runAllTimersAsync();
		expect(model.path).toBeNull();
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
	});

	it('reuses the HP ranking, which no goal changes', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'hp' });
		model.setView('optimiser');
		await vi.runAllTimersAsync();
		model.setGoal('150');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		expect(mocked.getHpOptimizer).toHaveBeenCalledTimes(1);
	});
});

describe('the named sessions', () => {
	it('selects the first that answers, and another on request', async () => {
		mocked.getSkillingForecast.mockResolvedValue(
			forecast([
				sourceForecast({ definitionId: 1, name: 'Out of reach', status: 'out_of_range' }),
				sourceForecast({ definitionId: 2, name: 'Quick' }),
				sourceForecast({ definitionId: 3, name: 'Slow' }),
			]),
		);
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		expect(model.selectedSource?.name).toBe('Quick');
		model.selectSource(3);
		expect(model.selectedSource?.name).toBe('Slow');
	});

	it('narrows to one weapon, remembers it, and takes them whole again', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		model.setSessionWeapon('Laconian Sword');
		await settle();
		expect(model.sessionWeapon).toBe('Laconian Sword');
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_session_weapon', 'Laconian Sword');
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ sessionWeapon: 'Laconian Sword' }),
		);
		model.setSessionWeapon(null);
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ sessionWeapon: null }),
		);
	});

	it('lands soft and thrown failures in their own facet only', async () => {
		mocked.getSkillingForecast.mockResolvedValueOnce({
			...forecast([]),
			error: "Profession 'X' not found",
		});
		mocked.getProfessionPathOptimizer.mockRejectedValueOnce(new Error('boom'));
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		model.setView('optimiser');
		model.setView('activities');
		await settle();
		expect(model.forecastError).toBe("Profession 'X' not found");
		expect(model.forecast).toBeNull();
		expect(model.pathError).toContain('boom');
		expect(model.activitiesError).toBeNull();
		expect(model.activities.result).not.toBeNull();
	});
});

describe('planned routes', () => {
	it('prices an added route, remembers it, and selects it', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		model.addRoute(SWORD_ON_ROBBERS);
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({
				routes: [
					{
						setup: { weapon: { catalogId: 'sword' } },
						species: 'Bank Robber',
						maturity: 'Default',
					},
				],
			}),
		);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_routes', [SWORD_ON_ROBBERS]);
		expect(model.routes).toEqual([SWORD_ON_ROBBERS]);
		expect(model.selectedRow?.key).toBe('route:0');

		// The same route again adds nothing.
		model.addRoute({ ...SWORD_ON_ROBBERS });
		expect(model.routes).toHaveLength(1);

		model.updateRoute(0, { ...SWORD_ON_ROBBERS, weaponId: 'chip', weaponName: 'Chip' });
		expect(model.routes[0].weaponName).toBe('Chip');

		model.removeRoute(0);
		expect(model.routes).toEqual([]);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('skilling_routes', []);
	});

	it('takes the target as priceable until the forecast says otherwise', async () => {
		mocked.getSkillingForecast.mockResolvedValue(forecastResult({ huntingTarget: false }));
		const model = makeModel();
		expect(model.huntingTarget).toBe(true);
		model.setTarget({ kind: 'profession', name: 'Animal Looter' });
		await settle();
		expect(model.huntingTarget).toBe(false);
	});

	it('re-prices once a typed boost settles, and at the looter basis in force', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await vi.runAllTimersAsync();
		// Unset, the field shows the boost the forecast priced at.
		expect(model.boostInput).toBe('50');
		vi.clearAllMocks();

		model.setSkillBoost('7');
		model.setSkillBoost('75');
		expect(model.boostInput).toBe('75');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		expect(mocked.getSkillingForecast).toHaveBeenCalledTimes(1);
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ skillBoostPercent: 75, looterLevel: null }),
		);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_route_pill', 75);

		// Past the cap, leaving the field settles on the cap.
		model.setSkillBoost('150');
		model.commitSkillBoost();
		expect(model.boostInput).toBe('100');
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ skillBoostPercent: 100 }),
		);

		// Another basis sends its level: Animal Looter 30.5.
		await setLooterBasis('animal');
		model.reloadForecast();
		await vi.runAllTimersAsync();
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ looterLevel: 30.5 }),
		);
		await setLooterBasis('three_looter_mean');
	});

	it('restores routes, boost, and session weapon, reading a pill saved before the percent', async () => {
		prefs.getPreference.mockImplementation(async (key: string) => {
			if (key === 'skilling_routes') return [SWORD_ON_ROBBERS, { weaponId: 3 }];
			if (key === 'skilling_route_pill') return 'hundred';
			if (key === 'skilling_session_weapon') return 'Laconian Sword';
			return null;
		});
		const model = makeModel();
		await model.restore();
		expect(model.routes).toEqual([SWORD_ON_ROBBERS]);
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ skillBoostPercent: 100, sessionWeapon: 'Laconian Sword' }),
		);
	});
});

describe('restore', () => {
	it('reopens the last target once', async () => {
		prefs.getPreference.mockImplementation(async (key: string) =>
			key === 'skilling_hub_target' ? { kind: 'profession', name: 'Animal Looter' } : null,
		);
		const model = makeModel();
		await model.restore();
		expect(model.target).toEqual({ kind: 'profession', name: 'Animal Looter' });
		expect(model.goalInput).toBe('31');
		await model.restore();
		const targetReads = prefs.getPreference.mock.calls.filter(
			([key]) => key === 'skilling_hub_target',
		);
		expect(targetReads).toHaveLength(1);
	});

	it('ignores a malformed or unknown saved target', async () => {
		prefs.getPreference.mockImplementation(async (key: string) =>
			key === 'skilling_hub_target' ? { kind: 'family', key: 'nope' } : null,
		);
		const model = makeModel();
		await model.restore();
		expect(model.target).toEqual({ kind: 'none' });
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
	});
});
