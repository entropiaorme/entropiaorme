import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	HpOptimizerResult,
	PathOptimizerResult,
	ProfessionOptimizerResult,
} from '$lib/api/commands.gen';
import { createOptimizerModel, type PageErrorSlot } from './optimizerModel.svelte';

vi.mock('$lib/api', () => ({
	getProfessionOptimizer: vi.fn(),
	getHpOptimizer: vi.fn(),
	getProfessionPathOptimizer: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function professionResult(overrides: Partial<ProfessionOptimizerResult> = {}) {
	return { skills: [], attributes: [], currentLevel: 37.5, ...overrides };
}

function hpResult(overrides: Partial<HpOptimizerResult> = {}): HpOptimizerResult {
	return {
		currentHp: 132.4,
		skills: [
			{
				name: 'Athletics',
				hpIncrease: 80,
				currentLevel: 2400,
				levelsPerHp: 80,
				pedPerHp: 1.2,
				hpPerPed: 0.83,
				codexCategory: null,
				codexDivisor: null,
			},
		],
		attributes: [{ name: 'Stamina', hpIncrease: 1, currentLevel: 30, levelsPerHp: 1 }],
		...overrides,
	};
}

function pathResult(overrides: Partial<PathOptimizerResult> = {}): PathOptimizerResult {
	return {
		profession: 'Laser Sniper (Hit)',
		currentLevel: 40,
		endLevel: 50,
		professionLevelsGained: 10,
		totalPed: 250,
		allocations: [],
		excluded: [],
		attributes: [],
		...overrides,
	} as unknown as PathOptimizerResult;
}

function makeModel(): { model: ReturnType<typeof createOptimizerModel>; errors: PageErrorSlot } {
	const errors: PageErrorSlot = { error: null };
	return { model: createOptimizerModel(errors), errors };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('loadOptimizer', () => {
	it('resets the level to zero without an API call when no profession is named', async () => {
		const { model } = makeModel();
		mocked.getProfessionOptimizer.mockResolvedValue(professionResult());
		await model.loadOptimizer('Laser Sniper (Hit)');
		expect(model.profLevel).toBe(37.5);

		await model.loadOptimizer('');
		expect(model.profLevel).toBe(0);
		expect(mocked.getProfessionOptimizer).toHaveBeenCalledTimes(1);
	});

	it('treats a null current level as zero', async () => {
		const { model } = makeModel();
		mocked.getProfessionOptimizer.mockResolvedValue(professionResult({ currentLevel: null }));
		await model.loadOptimizer('Laser Sniper (Hit)');
		expect(model.profLevel).toBe(0);
	});

	it('surfaces a failure and still lands the level on zero', async () => {
		const { model, errors } = makeModel();
		mocked.getProfessionOptimizer.mockResolvedValue(professionResult());
		await model.loadOptimizer('Laser Sniper (Hit)');
		mocked.getProfessionOptimizer.mockRejectedValue(new Error('backend unreachable'));
		await model.loadOptimizer('Laser Sniper (Hit)');
		expect(model.profLevel).toBe(0);
		expect(errors.error).toBe('backend unreachable');
	});

	it('clears a stale error on entry', async () => {
		const { model, errors } = makeModel();
		errors.error = 'stale failure';
		mocked.getProfessionOptimizer.mockResolvedValue(professionResult());
		await model.loadOptimizer('Laser Sniper (Hit)');
		expect(errors.error).toBeNull();
	});
});

describe('loadHpOptimizer', () => {
	it('populates the skills, attributes and current HP on success', async () => {
		const { model, errors } = makeModel();
		mocked.getHpOptimizer.mockResolvedValue(hpResult());
		await model.loadHpOptimizer();
		expect(model.hpSkills.map((s) => s.name)).toEqual(['Athletics']);
		expect(model.hpAttributes.map((a) => a.name)).toEqual(['Stamina']);
		expect(model.hpCurrent).toBe(132.4);
		expect(model.hpLoading).toBe(false);
		expect(errors.error).toBeNull();
	});

	it('surfaces a failure and lands on empty lists rather than stale rows', async () => {
		const { model, errors } = makeModel();
		mocked.getHpOptimizer.mockResolvedValue(hpResult());
		await model.loadHpOptimizer();
		expect(model.hpSkills).toHaveLength(1);

		mocked.getHpOptimizer.mockRejectedValue(new Error('backend unreachable'));
		await model.loadHpOptimizer();
		expect(model.hpSkills).toEqual([]);
		expect(model.hpAttributes).toEqual([]);
		// The current-HP figure is deliberately left as loaded; only the lists reset.
		expect(model.hpCurrent).toBe(132.4);
		expect(model.hpLoading).toBe(false);
		expect(errors.error).toBe('backend unreachable');
	});
});

describe('loadPathOptimizer', () => {
	it('does nothing without a selected profession', async () => {
		const { model } = makeModel();
		model.pathTargetInput = '50';
		await model.loadPathOptimizer();
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
	});

	it('does nothing when the target is missing, non-numeric, zero or negative', async () => {
		const { model } = makeModel();
		model.selectedProfession = 'Laser Sniper (Hit)';
		for (const target of ['', 'abc', '0', '-5']) {
			model.pathTargetInput = target;
			await model.loadPathOptimizer();
		}
		expect(mocked.getProfessionPathOptimizer).not.toHaveBeenCalled();
	});

	it('loads the path for a valid target', async () => {
		const { model } = makeModel();
		mocked.getProfessionPathOptimizer.mockResolvedValue(pathResult());
		model.selectedProfession = 'Laser Sniper (Hit)';
		model.pathTargetInput = '50';
		await model.loadPathOptimizer();
		expect(mocked.getProfessionPathOptimizer).toHaveBeenCalledWith(['Laser Sniper (Hit)'], {
			targetLevel: 50,
		});
		expect(model.pathResult?.endLevel).toBe(50);
		expect(model.pathLoading).toBe(false);
	});

	it('surfaces a failure and lands the result on null', async () => {
		const { model, errors } = makeModel();
		mocked.getProfessionPathOptimizer.mockResolvedValue(pathResult());
		model.selectedProfession = 'Laser Sniper (Hit)';
		model.pathTargetInput = '50';
		await model.loadPathOptimizer();
		expect(model.pathResult).not.toBeNull();

		mocked.getProfessionPathOptimizer.mockRejectedValue(new Error('no data'));
		await model.loadPathOptimizer();
		expect(model.pathResult).toBeNull();
		expect(model.pathLoading).toBe(false);
		expect(errors.error).toBe('no data');
	});

	it('clears a stale error on entry even when validation stops the load', async () => {
		const { model, errors } = makeModel();
		errors.error = 'stale failure';
		await model.loadPathOptimizer();
		expect(errors.error).toBeNull();
	});
});
