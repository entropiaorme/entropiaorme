import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ActivityRecommenderResult, RecommenderActivity } from '$lib/api/commands.gen';
import type { PageErrorSlot } from './errorSlot.svelte';
import { createRecommenderModel, withDirect } from './recommenderModel.svelte';

vi.mock('$lib/api', () => ({
	getActivityRecommender: vi.fn(),
}));

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import * as api from '$lib/api';
import * as preferences from '$lib/preferences';

const mocked = vi.mocked(api);
const prefs = vi.mocked(preferences);

function activity(overrides: Partial<RecommenderActivity> = {}): RecommenderActivity {
	return {
		activity: 'Resource Gatherer',
		professions: ['Resource Gatherer'],
		pesToPlusOne: 29.2,
		gainAtCap: 6.39,
		series: [0, 0.9, 6.39],
		contributors: [{ name: 'Analysis', currentLevel: 12, levelGain: 615, targetGain: 2.46 }],
		...overrides,
	};
}

function recommenderResult(
	overrides: Partial<ActivityRecommenderResult> = {},
): ActivityRecommenderResult {
	return {
		pesCap: 1000,
		sampleStep: 20,
		direct: activity({ activity: 'Animal Looter', pesToPlusOne: 8.1, gainAtCap: 15.8 }),
		candidates: [
			activity(),
			activity({ activity: 'Gardener', pesToPlusOne: 31.4, gainAtCap: 5.06 }),
		],
		...overrides,
	};
}

function makeModel(): { model: ReturnType<typeof createRecommenderModel>; errors: PageErrorSlot } {
	const errors: PageErrorSlot = { error: null };
	return { model: createRecommenderModel(errors), errors };
}

beforeEach(() => {
	vi.clearAllMocks();
	prefs.getPreference.mockResolvedValue([]);
	prefs.setPreference.mockResolvedValue(undefined);
});

describe('load', () => {
	it('clears the result without an API call for a none target', async () => {
		const { model } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(model.result).not.toBeNull();

		await model.load({ kind: 'none' });
		expect(model.result).toBeNull();
		expect(model.selected).toBeNull();
		expect(mocked.getActivityRecommender).toHaveBeenCalledTimes(1);
	});

	it('queries a single profession and selects the top candidate', async () => {
		const { model, errors } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'profession',
			professions: ['Animal Looter'],
		});
		expect(model.selected?.activity).toBe('Resource Gatherer');
		expect(model.loading).toBe(false);
		expect(errors.error).toBeNull();
	});

	it('expands a family target into its member professions', async () => {
		const { model } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult({ direct: null }));
		await model.load({ kind: 'family', key: 'looter' });
		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'profession',
			professions: ['Animal Looter', 'Mutant Looter', 'Robot Looter'],
		});
	});

	it('queries HP with no professions', async () => {
		const { model } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult({ direct: null }));
		await model.load({ kind: 'hp' });
		expect(mocked.getActivityRecommender).toHaveBeenCalledWith({
			target: 'hp',
			professions: [],
		});
	});

	it('routes a soft error to the error slot and keeps the result empty', async () => {
		const { model, errors } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(
			recommenderResult({ error: "Profession 'Nonsense' not found", candidates: [] }),
		);
		await model.load({ kind: 'profession', name: 'Nonsense' });
		expect(model.result).toBeNull();
		expect(errors.error).toBe("Profession 'Nonsense' not found");
	});

	it('surfaces a thrown failure and lands the result on null', async () => {
		const { model, errors } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(model.result).not.toBeNull();

		mocked.getActivityRecommender.mockRejectedValue(new Error('backend unreachable'));
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(model.result).toBeNull();
		expect(model.loading).toBe(false);
		expect(errors.error).toBe('backend unreachable');
	});
});

describe('overlapping loads', () => {
	function deferred<T>() {
		let resolve!: (value: T) => void;
		let reject!: (reason: unknown) => void;
		const promise = new Promise<T>((res, rej) => {
			resolve = res;
			reject = rej;
		});
		return { promise, resolve, reject };
	}

	it('discards a stale response that resolves after a newer load', async () => {
		const { model, errors } = makeModel();
		const first = deferred<ActivityRecommenderResult>();
		const second = deferred<ActivityRecommenderResult>();
		mocked.getActivityRecommender
			.mockReturnValueOnce(first.promise)
			.mockReturnValueOnce(second.promise);

		const firstLoad = model.load({ kind: 'profession', name: 'Animal Looter' });
		const secondLoad = model.load({ kind: 'hp' });
		second.resolve(recommenderResult({ direct: null, candidates: [activity()] }));
		await secondLoad;
		expect(model.selected?.activity).toBe('Resource Gatherer');

		first.resolve(recommenderResult({ candidates: [activity({ activity: 'Stale Winner' })] }));
		await firstLoad;
		// The stale resolution neither replaces the result nor clears the
		// newer load's state.
		expect(model.selected?.activity).toBe('Resource Gatherer');
		expect(model.loading).toBe(false);
		expect(errors.error).toBeNull();
	});

	it('ignores a stale failure after a newer load succeeded', async () => {
		const { model, errors } = makeModel();
		const first = deferred<ActivityRecommenderResult>();
		mocked.getActivityRecommender
			.mockReturnValueOnce(first.promise)
			.mockResolvedValueOnce(recommenderResult({ direct: null }));

		const firstLoad = model.load({ kind: 'profession', name: 'Animal Looter' });
		await model.load({ kind: 'hp' });
		first.reject(new Error('stale failure'));
		await firstLoad;
		expect(errors.error).toBeNull();
		expect(model.result).not.toBeNull();
	});

	it('selecting none while a load is pending keeps the cleared state', async () => {
		const { model, errors } = makeModel();
		const pending = deferred<ActivityRecommenderResult>();
		mocked.getActivityRecommender.mockReturnValueOnce(pending.promise);

		const pendingLoad = model.load({ kind: 'profession', name: 'Animal Looter' });
		await model.load({ kind: 'none' });
		expect(model.loading).toBe(false);

		pending.resolve(recommenderResult());
		await pendingLoad;
		expect(model.result).toBeNull();
		expect(model.selected).toBeNull();
		expect(model.loading).toBe(false);
		expect(errors.error).toBeNull();
	});
});

describe('select', () => {
	it('switches the selected candidate and falls back to the top for unknown names', async () => {
		const { model } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		await model.load({ kind: 'profession', name: 'Animal Looter' });

		model.select('Gardener');
		expect(model.selected?.activity).toBe('Gardener');

		model.select('Not A Candidate');
		expect(model.selected?.activity).toBe('Resource Gatherer');
	});
});

describe('hidden activities', () => {
	const names = (list: RecommenderActivity[]) => list.map((candidate) => candidate.activity);

	it('restores the saved list and never selects a hidden activity by default', async () => {
		prefs.getPreference.mockResolvedValue(['Resource Gatherer']);
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		const { model } = makeModel();
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(prefs.getPreference).toHaveBeenCalledWith('recommender_hidden_activities', []);
		expect(names(model.candidates)).toEqual(['Animal Looter', 'Gardener', 'Resource Gatherer']);
		expect(model.selected?.activity).toBe('Gardener');
		expect(model.visibleCount).toBe(2);
		expect(model.isHidden('Resource Gatherer')).toBe(true);
	});

	it('hides and restores persistently, moving the chart off a hidden activity', async () => {
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		const { model } = makeModel();
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(model.selected?.activity).toBe('Resource Gatherer');

		model.hide('Resource Gatherer');
		expect(names(model.candidates)).toEqual(['Animal Looter', 'Gardener', 'Resource Gatherer']);
		expect(model.selected?.activity).toBe('Gardener');
		expect(prefs.setPreference).toHaveBeenLastCalledWith('recommender_hidden_activities', [
			'Resource Gatherer',
		]);

		model.restore('Resource Gatherer');
		expect(names(model.candidates)).toEqual(['Animal Looter', 'Resource Gatherer', 'Gardener']);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('recommender_hidden_activities', []);
	});

	it('keeps the hidden list across targets and reads it once', async () => {
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		const { model } = makeModel();
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		model.hide('Gardener');
		await model.load({ kind: 'hp' });
		expect(names(model.candidates)).toEqual(['Animal Looter', 'Resource Gatherer', 'Gardener']);
		expect(prefs.getPreference).toHaveBeenCalledTimes(1);
	});

	it('ignores a malformed saved list', async () => {
		prefs.getPreference.mockResolvedValue('nope');
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		const { model } = makeModel();
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(model.hidden).toEqual([]);
	});
});

describe('the direct activity', () => {
	const rank = (activities: RecommenderActivity[]) => activities.map((a) => a.activity);

	it('takes its place in the ranking: fewer PES first, then more gain at the cap', () => {
		const candidates = [
			activity({ activity: 'Quick', pesToPlusOne: 10 }),
			activity({ activity: 'Slow', pesToPlusOne: 30 }),
			activity({ activity: 'Capped', pesToPlusOne: null, gainAtCap: 0.4 }),
		];
		expect(rank(withDirect(candidates, activity({ activity: 'Own', pesToPlusOne: 20 })))).toEqual([
			'Quick',
			'Own',
			'Slow',
			'Capped',
		]);
		expect(rank(withDirect(candidates, activity({ activity: 'Own', pesToPlusOne: 5 })))[0]).toBe(
			'Own',
		);
		// Short of +1 at the cap, it ranks among the others by gain.
		expect(
			rank(
				withDirect(candidates, activity({ activity: 'Own', pesToPlusOne: null, gainAtCap: 0.9 })),
			),
		).toEqual(['Quick', 'Slow', 'Own', 'Capped']);
		expect(withDirect(candidates, null)).toBe(candidates);
	});

	it('ranks among the candidates but is not the default choice', async () => {
		const { model } = makeModel();
		mocked.getActivityRecommender.mockResolvedValue(recommenderResult());
		await model.load({ kind: 'profession', name: 'Animal Looter' });
		expect(rank(model.candidates)).toEqual(['Animal Looter', 'Resource Gatherer', 'Gardener']);
		expect(model.isDirect('Animal Looter')).toBe(true);
		expect(model.isDirect('Gardener')).toBe(false);
		// The chart opens on the quickest other activity, drawn against it.
		expect(model.selected?.activity).toBe('Resource Gatherer');
	});
});
