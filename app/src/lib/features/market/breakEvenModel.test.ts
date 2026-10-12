import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MarketBreakEven } from '$lib/api';
import { setLooterBasis } from '$lib/looterBasis.svelte';
import { createBreakEvenModel } from './breakEvenModel.svelte';

vi.mock('$lib/api', () => ({
	getMarketBreakEven: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function payload(): MarketBreakEven {
	// The break-even falls as the looter level rises; Mutant is lowest here.
	const atBasis = (breakEvenLootMarkupPct: number, looterLevel: number) => ({
		looterLevel,
		expectedTtReturnPct: 10_000 / breakEvenLootMarkupPct,
		breakEvenLootMarkupPct,
	});
	const loadout = (name: string, mean: number | null, mutant: number | null = mean) => ({
		name,
		amplifierName: null,
		weightedEfficiencyPct: mean === null ? null : 70,
		offensiveTtRecoveryPct: mean === null ? null : 95,
		byLooter:
			mean === null || mutant === null
				? null
				: {
						threeLooterMean: atBasis(mean, 24.57),
						animal: atBasis(mean - 1, 37.2),
						mutant: atBasis(mutant, 12.0),
						robot: atBasis(mean + 0.5, 24.5),
					},
		coverage: mean === null ? null : 1,
		incomplete: mean === null,
		modelVersion: mean === null ? null : 'community_v1',
	});
	return {
		looters: [
			{ name: 'Animal Looter', level: 37.2 },
			{ name: 'Mutant Looter', level: 12.0 },
			{ name: 'Robot Looter', level: 24.5 },
		],
		weapons: [
			loadout('Unknown Blade', null),
			loadout('Low Efficiency Gun', 110.1, 111),
			loadout('High Efficiency Gun', 105.6, 112),
		],
	};
}

beforeEach(async () => {
	vi.clearAllMocks();
	await setLooterBasis('three_looter_mean');
});

describe('createBreakEvenModel', () => {
	it('sorts known weapons by best break-even and unknowns last', async () => {
		mocked.getMarketBreakEven.mockResolvedValue(payload());
		const model = createBreakEvenModel();
		await model.loadData();

		expect(model.looters.map((l) => l.name)).toEqual([
			'Animal Looter',
			'Mutant Looter',
			'Robot Looter',
		]);
		expect(model.weapons.map((w) => w.name)).toEqual([
			'High Efficiency Gun',
			'Low Efficiency Gun',
			'Unknown Blade',
		]);
	});

	it('reads each row at the chosen looter and reorders by it', async () => {
		mocked.getMarketBreakEven.mockResolvedValue(payload());
		const model = createBreakEvenModel();
		await model.loadData();
		expect(model.looterLevels).toEqual({ animal: 37.2, mutant: 12.0, robot: 24.5 });
		expect(model.weapons[0]).toMatchObject({
			name: 'High Efficiency Gun',
			breakEvenLootMarkupPct: 105.6,
			looterLevel: 24.57,
		});

		await setLooterBasis('mutant');
		expect(model.weapons.map((w) => [w.name, w.breakEvenLootMarkupPct, w.looterLevel])).toEqual([
			['Low Efficiency Gun', 111, 12.0],
			['High Efficiency Gun', 112, 12.0],
			['Unknown Blade', null, null],
		]);

		await setLooterBasis('animal');
		expect(model.weapons[0]).toMatchObject({
			name: 'High Efficiency Gun',
			breakEvenLootMarkupPct: 104.6,
		});
	});

	it('surfaces a load failure', async () => {
		mocked.getMarketBreakEven.mockRejectedValue(new Error('boom'));
		const model = createBreakEvenModel();
		await model.loadData();
		expect(model.error).not.toBeNull();
		expect(model.weapons).toHaveLength(0);
	});
});
