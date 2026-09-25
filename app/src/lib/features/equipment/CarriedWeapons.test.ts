// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Equipment } from '$lib/types';
import CarriedWeapons from './CarriedWeapons.svelte';

vi.mock('$lib/api', () => ({
	updateSettings: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function weapon(id: string, name: string): Equipment {
	return {
		id,
		name,
		type: 'weapon',
		amplifierName: null,
		costPerUse: 1.5,
		damageMin: 5,
		damageMax: 10,
		reloadSeconds: null,
		isLimited: false,
		enrichmentLevel: 0,
		healingProfile: null,
		lifestealPercent: null,
	} as Equipment;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('carried weapons', () => {
	it('adds a weapon and reports the stored list', async () => {
		const onchange = vi.fn();
		mocked.updateSettings.mockResolvedValue({ carriedWeaponIds: [3, 4] } as never);
		render(CarriedWeapons, {
			props: {
				carried: [weapon('3', 'Marksman')],
				addable: [weapon('4', 'Opalo')],
				carriedIds: [3],
				onchange,
			},
		});
		expect(screen.getByText('Marksman')).toBeTruthy();
		const select = screen.getByLabelText('Carry a weapon without a hotkey') as HTMLSelectElement;
		await fireEvent.change(select, { target: { value: '4' } });
		expect(mocked.updateSettings).toHaveBeenCalledWith({ carried_weapon_ids: [3, 4] });
		await vi.waitFor(() => expect(onchange).toHaveBeenCalledWith([3, 4]));
	});

	it('removes a weapon, and says so when saving fails', async () => {
		mocked.updateSettings.mockRejectedValue(new Error('disk full'));
		render(CarriedWeapons, {
			props: { carried: [weapon('3', 'Marksman')], addable: [], carriedIds: [3] },
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Stop carrying Marksman' }));
		expect(mocked.updateSettings).toHaveBeenCalledWith({ carried_weapon_ids: [] });
		expect(await screen.findByText('disk full')).toBeTruthy();
	});

	it('offers nothing to add once every weapon is on the hotbar', () => {
		render(CarriedWeapons, {
			props: { carried: [], addable: [], carriedIds: [], enabled: true },
		});
		expect(screen.getByText('Every weapon in the library is already on the hotbar.')).toBeTruthy();
	});

	it('holds its controls while disabled', () => {
		render(CarriedWeapons, {
			props: {
				carried: [weapon('3', 'Marksman')],
				addable: [weapon('4', 'Opalo')],
				carriedIds: [3],
				enabled: false,
			},
		});
		expect(
			(screen.getByRole('button', { name: 'Stop carrying Marksman' }) as HTMLButtonElement)
				.disabled,
		).toBe(true);
		expect(
			(screen.getByLabelText('Carry a weapon without a hotkey') as HTMLSelectElement).disabled,
		).toBe(true);
	});
});
