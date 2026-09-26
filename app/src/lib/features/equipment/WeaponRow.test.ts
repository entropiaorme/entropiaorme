// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { Equipment, EquipmentDetail } from '$lib/types';
import type { LibraryModel } from './libraryModel.svelte';
import WeaponRow from './WeaponRow.svelte';

const item: Equipment = {
	id: '1',
	name: 'Jester D-1',
	type: 'weapon',
	amplifierName: null,
	costPerUse: 0.42,
	damageMin: 13.0,
	damageMax: 22.5,
	reloadSeconds: 2.5,
	isLimited: false,
	enrichmentLevel: 2,
	healingProfile: null,
	lifestealPercent: null,
	effectProfile: null,
	consumable: null,
};

const jesterDetail: EquipmentDetail = {
	id: '1',
	type: 'weapon',
	weapon: {
		catalogId: 'jester-d1',
		name: 'Jester D-1',
		decay: 0.18,
		ammoBurn: 0.24,
		markupPercent: 100,
		isLimited: false,
		damageEnhancers: 0,
		efficiencyPct: 55,
	},
	amplifier: null,
	scope: null,
	absorber: null,
	implant: null,
	costBreakdown: [
		{
			component: 'Jester D-1 decay',
			costPec: 0.18,
			markupMultiplier: 1.0,
			effectiveCostPec: 0.18,
		},
		{
			component: 'Jester D-1 ammo',
			costPec: 0.24,
			markupMultiplier: 1.0,
			effectiveCostPec: 0.24,
		},
	],
	totalCostPerUse: 0.42,
	expectedReturn: {
		modelVersion: 'community_v1',
		looterSource: 'three_looter_mean',
		looterLevel: 42.5,
		weightedEfficiencyPct: 55,
		offensiveTtRecovery: 0.92825,
		expectedTtRate: 0.92825,
		effectiveEfficiency: { status: 'within_model_range', efficiencyPct: 55 },
		breakEvenLootMarkup: 1.077296,
		modelledRawTtPerUse: 0.42,
		eligibleOffensiveCostPerUse: 0.42,
		consumedPremiumPerUse: 0,
		coverage: 1,
		incomplete: false,
	},
	healingProfile: null,
	lifestealPercent: null,
	effectProfile: null,
	consumable: null,
	attackRate: {
		basePerMinute: 46,
		reloadSpeedPercent: 0,
		buffedPerMinute: 46,
		effectivePerMinute: 46,
		factor: 1,
	},
};

/** The row reads only the expansion and the detail cache from its model. */
function expandedWith(detail: EquipmentDetail): LibraryModel {
	return {
		expandedId: item.id,
		detailCache: { [item.id]: detail },
		toggleExpand: () => {},
	} as unknown as LibraryModel;
}

describe('the weapon detail attack rate', () => {
	it('states the catalogue rate within the server limit', () => {
		render(WeaponRow, { props: { model: expandedWith(jesterDetail), item } });
		expect(screen.getByTestId('weapon-attack-rate').textContent).toContain('46 a minute');
		expect(screen.queryByText(/damage per PEC is unchanged/)).toBeNull();
	});

	it('explains the per-attack factor when the limit holds the weapon back', () => {
		const detail: EquipmentDetail = {
			...jesterDetail,
			attackRate: {
				basePerMinute: 90,
				reloadSpeedPercent: 15,
				buffedPerMinute: 103.5,
				effectivePerMinute: 100,
				factor: 1.035,
			},
		};
		render(WeaponRow, { props: { model: expandedWith(detail), item } });
		expect(screen.getByTestId('weapon-attack-rate').textContent).toContain(
			'100 a minute, server limit',
		);
		expect(
			screen.getByText(
				'The 103.5 a minute this weapon would reach become ×1.035 damage and cost per attack; damage per PEC is unchanged.',
			),
		).toBeTruthy();
	});

	it('shows no attack-rate line for a weapon without a catalogue rate', () => {
		render(WeaponRow, {
			props: { model: expandedWith({ ...jesterDetail, attackRate: null }), item },
		});
		expect(screen.queryByTestId('weapon-attack-rate')).toBeNull();
	});
});
