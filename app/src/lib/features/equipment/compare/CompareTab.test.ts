// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { EquipmentSearchResult } from '$lib/api';
import {
	BC30,
	bc30AgainstGauss,
	comparison,
	component,
	economics,
	GAUSS,
	setup,
} from './__fixtures__/comparison';
import CompareTab from './CompareTab.svelte';
import { type Comparator, createComparator } from './compareModel.svelte';

vi.mock('$lib/api', () => ({
	compareLoadouts: vi.fn(),
	searchEquipmentItems: vi.fn(async () => []),
}));
vi.mock('$lib/api/character', () => ({
	getCharacterProfessions: vi.fn(async () => [
		{ name: 'Animal Looter', level: 30, anchorLevel: null, gainSinceAnchor: null, category: '' },
		{ name: 'Mutant Looter', level: 60, anchorLevel: null, gainSinceAnchor: null, category: '' },
		{ name: 'Robot Looter', level: 30, anchorLevel: null, gainSinceAnchor: null, category: '' },
	]),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function hit(catalogId: string, name: string, isLimited = true): EquipmentSearchResult {
	return {
		catalogId,
		name,
		decay: 0.01,
		ammoBurn: 10,
		absorptionPercent: null,
		isLimited,
		healMin: null,
		healMax: null,
		reloadSeconds: null,
		lifestealPercent: null,
		usesPerMinute: 60,
		consumable: null,
	};
}

function list(comparator: Comparator, item: EquipmentSearchResult) {
	comparator.blank?.pickers.weapon.select(item);
	comparator.commitBlank();
}

beforeEach(() => {
	vi.clearAllMocks();
	mocked.compareLoadouts.mockResolvedValue(bc30AgainstGauss());
});

describe('CompareTab', () => {
	it('shows the chart frame and an empty row before anything is listed', () => {
		render(CompareTab, { props: { comparator: createComparator() } });
		expect(screen.getByText('Loss per kill by mob HP')).toBeTruthy();
		expect(screen.getByText('Add a weapon to plot its loss per kill')).toBeTruthy();
		expect(screen.getByPlaceholderText('Add a weapon…')).toBeTruthy();
		// The empty row offers no parts until it has a weapon.
		expect(screen.queryByRole('button', { name: '+ Amplifier' })).toBeNull();
		expect(mocked.compareLoadouts).not.toHaveBeenCalled();
	});

	it('lists each setup as an editable row under the chart, with the next empty row below', async () => {
		const comparator = createComparator();
		list(comparator, hit('bc30', BC30));
		list(comparator, hit('gauss', GAUSS));
		render(CompareTab, { props: { comparator } });

		// Each setup's damage per PEC.
		expect(await screen.findByText('3.70', {}, { timeout: 2000 })).toBeTruthy();
		expect(screen.getByText('3.30')).toBeTruthy();
		expect(mocked.compareLoadouts).toHaveBeenCalledTimes(1);
		expect(vi.mocked(api.compareLoadouts).mock.calls[0][0]).toHaveLength(2);
		// Each row names its weapon, offers its parts and a markup, and can
		// be removed; the next empty row waits below.
		expect(screen.getByRole('button', { name: BC30 })).toBeTruthy();
		expect(screen.getAllByRole('button', { name: '+ Amplifier' })).toHaveLength(2);
		expect(screen.getAllByLabelText('Weapon markup percent')).toHaveLength(2);
		expect(screen.getByRole('button', { name: `Remove ${GAUSS}` })).toBeTruthy();
		expect(screen.getByPlaceholderText('Add a weapon…')).toBeTruthy();
		expect(screen.queryByText('Add a weapon to plot its loss per kill')).toBeNull();
		expect(screen.getByRole('button', { name: 'How these figures are estimated' })).toBeTruthy();
	});

	it('answers for the mobs killed, the mob HP, and the looter level', async () => {
		const comparator = createComparator();
		list(comparator, hit('bc30', BC30));
		list(comparator, hit('gauss', GAUSS));
		await comparator.compare();
		render(CompareTab, { props: { comparator } });

		expect(screen.getByRole('slider', { name: 'Mobs killed' })).toBeTruthy();
		expect(screen.getByRole('slider', { name: 'Mob HP' })).toBeTruthy();
		// The looter slider starts at the looter mean; its ticks mark the mean
		// and each looter, and looters at one level share a tick.
		expect(await screen.findByRole('slider', { name: 'Looter level' })).toBeTruthy();
		expect(screen.getByRole('button', { name: '40.0 · Looter mean' })).toBeTruthy();
		expect(
			screen.getByRole('button', { name: 'Slide to Animal Looter 30.0, Robot Looter 30.0' })
				.textContent,
		).toContain('A · R');
		await fireEvent.click(screen.getByRole('button', { name: 'Slide to Mutant Looter 60.0' }));
		expect(comparator.looterLevel).toBe(60);
		expect(screen.getByRole('button', { name: '60.0 · Mutant Looter' })).toBeTruthy();
		comparator.looterLevel = 70;
		await tick();
		expect(screen.getByRole('button', { name: '70.0' })).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Slide to three-looter mean 40.0' }));
		expect(comparator.looterLevel).toBeNull();
		// What the year's kills cycle with the reference setup.
		expect(screen.getByText((text) => text.includes(`PED cycled with ${BC30}`))).toBeTruthy();
	});
});

describe('ComparisonTable', () => {
	// At the default 10,000 kills of a 100 HP mob, the fixture curve takes
	// 4.33 uses a kill: 43,333 uses.
	function unlimitedSetup(index: number, name: string, houseEdgePec: number) {
		return setup(index, name, {
			components: [component({ catalogId: name, name, isLimited: false, maxTtPed: null })],
			economics: economics({ consumedPremiumPec: 0, houseEdgePec }),
		});
	}

	/** A limited reference, an unlimited setup that loses less per kill,
	 * and one that loses more. */
	async function withCapital() {
		mocked.compareLoadouts.mockResolvedValue(
			comparison({
				setups: [
					setup(0, BC30, {
						components: [component({ name: BC30, decayPecPerUse: 0.2, maxTtPed: 45 })],
					}),
					// 0.09 + 0.21 = 0.3 PEC a use against the reference's 0.4.
					unlimitedSetup(1, 'Scimitar', 0.21),
					// 0.09 + 0.41 = 0.5 PEC a use.
					unlimitedSetup(2, 'Saif', 0.41),
				],
			}),
		);
		const comparator = createComparator();
		list(comparator, hit('bc30', BC30));
		list(comparator, hit('scimitar', 'Scimitar', false));
		list(comparator, hit('saif', 'Saif', false));
		await comparator.compare();
		return comparator;
	}

	it('shows each setup for the kills and the mob HP in its columns', async () => {
		const comparator = createComparator();
		list(comparator, hit('bc30', BC30));
		mocked.compareLoadouts.mockResolvedValue(comparison({ setups: [setup(0, BC30)] }));
		await comparator.compare();
		render(CompareTab, { props: { comparator } });

		for (const heading of [
			'Damage / PEC',
			'Eff. Efficiency',
			'PED cycled',
			'Expected loss',
			'Time',
		]) {
			expect(screen.getByRole('columnheader', { name: heading })).toBeTruthy();
		}
		expect(screen.getByText('3.30')).toBeTruthy();
		expect(screen.getByText('75.9%')).toBeTruthy();
		// The premium is part of the loss, not of the TT cycled.
		expect(screen.getByText('3,454 PED')).toBeTruthy();
		expect(screen.getByText('173 PED')).toBeTruthy();
		expect(screen.getByText('10 h 47 min')).toBeTruthy();
		expect(screen.queryByText('Pays back at')).toBeNull();
	});

	it('names the limited parts a year of kills uses up', async () => {
		const comparator = await withCapital();
		render(CompareTab, { props: { comparator } });
		// 43,333 uses at 0.2 PEC wear out 86.7 PED of 45 PED weapons.
		expect(screen.getByText('Uses up ≈ 1.9 weapons')).toBeTruthy();
		expect(screen.getAllByText(/^Uses up/)).toHaveLength(1);
	});

	it('weighs capital in unlimited gear against the limited reference', async () => {
		const comparator = await withCapital();
		render(CompareTab, { props: { comparator } });
		expect(screen.queryByText('Pays back at')).toBeNull();

		comparator.draft(comparator.drafts[1].id)?.setCapitalText('weapon', '1000');
		comparator.draft(comparator.drafts[2].id)?.setCapitalText('weapon', '1000');
		await tick();
		expect(screen.getByText('Pays back at')).toBeTruthy();
		expect(screen.getByLabelText('Limited setup to weigh capital against')).toBeTruthy();
		expect(
			(screen.getByLabelText('Yearly rate on capital, percent') as HTMLInputElement).value,
		).toBe('10');

		// The reference row says so.
		expect(screen.getByText('reference')).toBeTruthy();
		// Saving 0.1 PEC a use, 0.0043 PED a kill, covers a 100 PED yearly
		// charge at about 23,100 kills; 10,000 kills save 43.3 of it.
		expect(screen.getByText(/^23\.1k mobs \/ yr$/i)).toBeTruthy();
		expect(screen.getByText('43.3')).toBeTruthy();
		expect(screen.getByText(/of 100 needed/)).toBeTruthy();
		// Losing more than the reference never pays back.
		expect(screen.getByText('Never')).toBeTruthy();
		expect(screen.getByText('43.3 more')).toBeTruthy();
		expect(screen.getByText(/than the reference/)).toBeTruthy();
	});
});
