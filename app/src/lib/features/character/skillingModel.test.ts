import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	ActivityRecommenderResult,
	HpOptimizerResult,
	PathOptimizerResult,
	SkillingForecastResult,
	SkillingForecastSource,
} from '$lib/api/commands.gen';
import type { ProfessionLevel } from '$lib/types/analytics';
import {
	createSkillingModel,
	defaultGoal,
	defaultSource,
	GOAL_DEBOUNCE_MS,
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

function source(overrides: Partial<SkillingForecastSource> = {}): SkillingForecastSource {
	return {
		definitionId: 7,
		name: 'Carabok Skilling',
		archived: false,
		sample: {
			sessions: 75,
			hours: 120,
			cycledPed: 45000,
			lootTt: 40800,
			pes: 3679,
			realisedMarkup: null,
			markupLift: null,
		},
		status: 'ready',
		cycledPed: 1240,
		hours: 3.3,
		lootTt: 1124,
		markup: null,
		ttCost: 116,
		netCost: null,
		skills: [],
		warnings: [],
		...overrides,
	};
}

function forecast(sources: SkillingForecastSource[] = [source()]): SkillingForecastResult {
	return { current: 42.37, goal: 43, sources };
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

	it('defaults to the quickest source that answers', () => {
		expect(defaultSource(null)).toBeNull();
		expect(defaultSource(forecast([]))).toBeNull();
		const blocked = source({ definitionId: 1, status: 'does_not_train' });
		const ready = source({ definitionId: 2 });
		expect(defaultSource(forecast([blocked, ready]))?.definitionId).toBe(2);
		expect(defaultSource(forecast([blocked]))?.definitionId).toBe(1);
	});
});

describe('setTarget', () => {
	it('answers a profession at its default goal across every facet', async () => {
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		expect(model.current).toBe(42.37);
		expect(model.goalInput).toBe('43');
		expect(model.goalActive).toBe(true);
		await settle();

		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'profession',
			professions: ['Laser Sniper (Hit)'],
		});
		expect(mocked.getProfessionPathOptimizer).toHaveBeenCalledWith(['Laser Sniper (Hit)'], {
			targetLevel: 43,
		});
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({
			target: 'profession',
			professions: ['Laser Sniper (Hit)'],
			goal: 43,
		});
		expect(model.path?.totalPed).toBe(120);
		expect(model.selectedSource?.name).toBe('Carabok Skilling');
		expect(model.pathLoading).toBe(false);
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
		expect(mocked.getHpOptimizer).toHaveBeenCalledTimes(1);
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({ target: 'hp', goal: 143 });
		expect(model.hpPath?.currentHp).toBe(142);
	});

	it('answers a family on its combined level across every facet', async () => {
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
		await settle();
		const looters = ['Animal Looter', 'Mutant Looter', 'Robot Looter'];
		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'profession',
			professions: looters,
		});
		expect(mocked.getProfessionPathOptimizer).toHaveBeenCalledWith(looters, { targetLevel: 51 });
		expect(mocked.getSkillingForecast).toHaveBeenCalledWith({
			target: 'profession',
			professions: looters,
			goal: 51,
		});
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
		mocked.getSkillingForecast.mockResolvedValueOnce(forecast([source({ name: 'Newer' })]));
		model.setTarget({ kind: 'profession', name: 'Animal Looter' });
		await settle();
		resolveFirst(forecast([source({ name: 'Stale' })]));
		await settle();
		expect(model.selectedSource?.name).toBe('Newer');
	});
});

describe('the goal', () => {
	it('reloads only the goal-bound facets once typing settles', async () => {
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
		expect(mocked.getProfessionPathOptimizer).toHaveBeenCalledWith(['Laser Sniper (Hit)'], {
			targetLevel: 45,
		});
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
		expect(model.path).toBeNull();
		expect(model.forecast).toBeNull();
		expect(mocked.getSkillingForecast).not.toHaveBeenCalled();
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
	});

	it('reuses the HP ranking, which no goal changes', async () => {
		vi.useFakeTimers();
		const model = makeModel();
		model.setTarget({ kind: 'hp' });
		await vi.runAllTimersAsync();
		model.setGoal('150');
		await vi.advanceTimersByTimeAsync(GOAL_DEBOUNCE_MS);
		expect(mocked.getHpOptimizer).toHaveBeenCalledTimes(1);
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith({ target: 'hp', goal: 150 });
	});
});

describe('the forecast', () => {
	it('lets the player choose another named session', async () => {
		mocked.getSkillingForecast.mockResolvedValue(
			forecast([
				source({ definitionId: 1, name: 'Quick' }),
				source({ definitionId: 2, name: 'Slow' }),
			]),
		);
		const model = makeModel();
		model.setTarget({ kind: 'profession', name: 'Laser Sniper (Hit)' });
		await settle();
		expect(model.selectedSource?.name).toBe('Quick');
		model.selectSource(2);
		expect(model.selectedSource?.name).toBe('Slow');
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
		expect(model.forecastError).toBe("Profession 'X' not found");
		expect(model.forecast).toBeNull();
		expect(model.pathError).toContain('boom');
		expect(model.activitiesError).toBeNull();
		expect(model.activities.result).not.toBeNull();
	});
});

describe('restore', () => {
	it('reopens the last target once', async () => {
		prefs.getPreference.mockResolvedValue({ kind: 'profession', name: 'Animal Looter' });
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
		prefs.getPreference.mockResolvedValue({ kind: 'family', key: 'nope' });
		const model = makeModel();
		await model.restore();
		expect(model.target).toEqual({ kind: 'none' });
		expect(mocked.getActivityRecommender).not.toHaveBeenCalled();
	});
});
