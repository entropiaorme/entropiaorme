// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it } from 'vitest';
import type { EquipmentExpectedReturn, ExpectedLooterSource } from '$lib/api/commands.gen';
import { looterBasis, setLooterBasis } from '$lib/looterBasis.svelte';
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

/** The fixture's expected return read at one looter basis. */
function expectedAt(
	looterSource: ExpectedLooterSource,
	looterLevel: number,
	lossPer100HpPec: number | null,
	rate: number,
): EquipmentExpectedReturn {
	return {
		modelVersion: 'community_v1',
		looterSource,
		looterLevel,
		weightedEfficiencyPct: 55,
		offensiveTtRecovery: rate,
		expectedTtRate: rate,
		effectiveEfficiency: { status: 'within_model_range', efficiencyPct: 55 },
		breakEvenLootMarkup: 1 / rate,
		modelledRawTtPerUse: 0.42,
		eligibleOffensiveCostPerUse: 0.42,
		consumedPremiumPerUse: 0,
		coverage: 1,
		incomplete: false,
		lossPer100HpPec,
	};
}

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
	expectedReturns: {
		threeLooterMean: expectedAt('three_looter_mean', 42.5, 1.2345, 0.928),
		animal: expectedAt('animal', 60, 1.0512, 0.941),
		mutant: expectedAt('mutant', 30, 1.3611, 0.92),
		robot: expectedAt('robot', 37.5, 1.2913, 0.925),
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

beforeEach(async () => {
	await setLooterBasis('three_looter_mean');
});

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

describe('the weapon detail economics', () => {
	it('shows loss per 100 HP beside Effective Efficiency', () => {
		render(WeaponRow, { props: { model: expandedWith(jesterDetail), item } });
		expect(screen.getByText('Loss / 100 HP')).toBeTruthy();
		expect(screen.getByText('1.23 PEC')).toBeTruthy();
	});

	it('shows no figure for a weapon it cannot place per HP', () => {
		const returns = jesterDetail.expectedReturns;
		if (!returns) throw new Error('the fixture carries expected returns');
		const detail: EquipmentDetail = {
			...jesterDetail,
			expectedReturns: {
				...returns,
				threeLooterMean: { ...returns.threeLooterMean, lossPer100HpPec: null },
			},
		};
		render(WeaponRow, { props: { model: expandedWith(detail), item } });
		expect(screen.getByText('Loss / 100 HP')).toBeTruthy();
		expect(screen.queryByText('1.23 PEC')).toBeNull();
	});
});

describe('the weapon detail looter basis', () => {
	it('names the basis its figures are read at, the mean by default', () => {
		render(WeaponRow, { props: { model: expandedWith(jesterDetail), item } });
		expect(screen.getByTestId('looter-basis-trigger').textContent).toContain('Looter mean 42.5');
		expect(screen.getByText('92.8%')).toBeTruthy();
	});

	it('reads every figure at the looter chosen, and remembers the choice', async () => {
		render(WeaponRow, { props: { model: expandedWith(jesterDetail), item } });
		await fireEvent.click(screen.getByTestId('looter-basis-trigger'));
		const options = screen.getAllByRole('menuitemradio');
		expect(options.map((option) => option.textContent?.replace(/\s+/g, ' ').trim())).toEqual([
			'Mean of the three 42.5',
			'Animal Looter 60.0',
			'Mutant Looter 30.0',
			'Robot Looter 37.5',
		]);
		expect(options[0].getAttribute('aria-checked')).toBe('true');

		await fireEvent.click(options[1]);
		expect(looterBasis.current).toBe('animal');
		expect(screen.getByTestId('looter-basis-trigger').textContent).toContain('Animal Looter 60.0');
		expect(screen.getByText('1.05 PEC')).toBeTruthy();
		expect(screen.getByText('94.1%')).toBeTruthy();
		expect(screen.queryByText('1.23 PEC')).toBeNull();
		expect(localStorage.getItem('looterBasis')).toBe('"animal"');
	});

	it('opens at the app-wide basis chosen elsewhere', async () => {
		await setLooterBasis('robot');
		render(WeaponRow, { props: { model: expandedWith(jesterDetail), item } });
		expect(screen.getByTestId('looter-basis-trigger').textContent).toContain('Robot Looter 37.5');
		expect(screen.getByText('1.29 PEC')).toBeTruthy();
	});
});
