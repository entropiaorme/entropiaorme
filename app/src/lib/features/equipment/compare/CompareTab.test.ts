// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { EquipmentSearchResult } from '$lib/api';
import { BC30, bc30AgainstGauss, GAUSS } from './__fixtures__/comparison';
import CompareTab from './CompareTab.svelte';
import { createComparator } from './compareModel.svelte';

vi.mock('$lib/api', () => ({
	compareLoadouts: vi.fn(),
	searchEquipmentItems: vi.fn(async () => []),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function hit(catalogId: string, name: string): EquipmentSearchResult {
	return {
		catalogId,
		name,
		decay: 0.01,
		ammoBurn: 10,
		absorptionPercent: null,
		isLimited: true,
		healMin: null,
		healMax: null,
		reloadSeconds: null,
		lifestealPercent: null,
		usesPerMinute: 60,
		consumable: null,
	};
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
		comparator.blank?.pickers.weapon.select(hit('bc30', BC30));
		comparator.commitBlank();
		comparator.blank?.pickers.weapon.select(hit('gauss', GAUSS));
		comparator.commitBlank();
		render(CompareTab, { props: { comparator } });

		// PEC lost per 100 HP, from each setup's loss per HP.
		expect(await screen.findByText('1.12', {}, { timeout: 2000 })).toBeTruthy();
		expect(screen.getByText('1.60')).toBeTruthy();
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
});
