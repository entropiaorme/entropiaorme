// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import ExpectedMuTip from './ExpectedMuTip.svelte';

const basis = (level: number) => ({
	looterLevel: level,
	expectedLootTtPed: 99.835,
	lowPed: 99.45,
	highPed: 100.15,
	modelledExpectedLootTtPed: 90.835,
});

const cycle = {
	modelVersion: 'community_v1',
	cycledPed: 112,
	modelledCostPed: 100,
	unknownEfficiencyTtPed: 10,
	lostPed: 2,
	bases: {
		threeLooterMean: basis(37),
		animal: basis(80),
		mutant: basis(10),
		robot: basis(20),
	},
};

describe('ExpectedMuTip', () => {
	it('reads over Cycled while the costs are the tracked ones', () => {
		render(ExpectedMuTip, {
			cycle,
			basis: 'three_looter_mean',
			lootMuFactor: 1.08,
			cycledPed: 112,
		});
		expect(screen.getByText('Expected loot at its markup, over Cycled')).toBeTruthy();
		expect(screen.getByText(/112\.00 PED cycled\./)).toBeTruthy();
	});

	it('names the tracked cost it rests on once Cycled comes from the account', () => {
		render(ExpectedMuTip, {
			cycle,
			basis: 'three_looter_mean',
			lootMuFactor: 1.08,
			cycledPed: 112,
			trackedCost: true,
		});
		expect(screen.getByText('Expected loot at its markup, over the tracked cost')).toBeTruthy();
		expect(screen.getByText(/112\.00 PED of the tracked cost\./)).toBeTruthy();
		expect(screen.queryByText(/PED cycled/)).toBeNull();
	});
});
