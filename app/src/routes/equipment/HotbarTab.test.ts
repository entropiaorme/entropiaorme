// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { Equipment } from '$lib/types';
import { shownMarkup } from '$lib/view/__fixtures__/markup';
import HotbarTab from './HotbarTab.svelte';

function entry(id: string, name: string, overrides: Partial<Equipment> = {}): Equipment {
	return {
		id,
		name,
		type: 'weapon',
		enabled: true,
		amplifierName: null,
		costPerUse: 1,
		damageMin: 10,
		damageMax: 20,
		reloadSeconds: 1,
		isLimited: false,
		enrichmentLevel: 1,
		healingProfile: null,
		lifestealPercent: null,
		effectProfile: null,
		consumable: null,
		...overrides,
	};
}

const equipment = [
	entry('1', 'Pistol'),
	entry('2', 'Rifle', { enabled: false }),
	entry('3', 'Vita', { type: 'consumable' }),
	entry('4', 'Finder', { type: 'tool' }),
];

function mount(toolReading: boolean) {
	return render(HotbarTab, {
		props: {
			equipment,
			hotbar: { '1': 1, '2': 3 },
			hotbarPresets: [{ '1': 1, '2': 3 }, {}, {}],
			activeHotbarPreset: 1,
			carriedWeaponIds: [],
			hotbarHooksEnabled: true,
			toolReading,
			onassign: vi.fn(),
		},
	});
}

function slotOptions(slot: number): string[] {
	const select = screen.getAllByRole('combobox')[slot - 1] as HTMLSelectElement;
	return [...select.options].map((option) => option.textContent?.trim() ?? '');
}

describe('the hotbar tab', () => {
	it('is as it was while the tool in hand is not read off the screen', async () => {
		const { container } = mount(false);
		// The whole tab as it was drawn before reading the held tool existed.
		await expect(shownMarkup(container.innerHTML)).toMatchFileSnapshot(
			'./__fixtures__/HotbarTab.off.html',
		);
		expect(screen.getByTestId('hotbar-listener-state').textContent).toMatch(
			/a press tells the app which weapon is in hand/,
		);
		expect(slotOptions(3)).toContain('Pistol');
		expect(slotOptions(3)).toContain('Finder');
		expect(screen.queryByTestId('slot-idle')).toBeNull();
		expect(screen.getByRole('heading', { name: 'Carried without a hotkey' })).toBeTruthy();
	});

	it('says the keys start consumables, offers consumables only, and keeps a weapon already bound, idle', () => {
		mount(true);
		expect(screen.getByTestId('hotbar-listener-state').textContent?.trim()).toBe(
			'The held tool is read from the screen. Hotbar keys only start consumables and count heals.',
		);
		// Reading the held tool is not in development: the note is unmarked.
		expect(screen.queryByText('In development')).toBeNull();
		const empty = slotOptions(3);
		expect(empty).toContain('Vita');
		expect(empty).not.toContain('Pistol');
		expect(empty).not.toContain('Finder');
		// The weapon bound to slot 1 is kept, shown, and marked idle.
		const bound = screen.getAllByRole('combobox')[0] as HTMLSelectElement;
		expect(bound.value).toBe('1');
		expect(slotOptions(1)).toContain('Pistol');
		expect(screen.getAllByTestId('slot-idle')).toHaveLength(1);
		// A consumable's slot is not idle.
		expect((screen.getAllByRole('combobox')[1] as HTMLSelectElement).value).toBe('3');
	});

	it('hides the weapons carried without a hotkey: the weapons on are the carried ones', () => {
		mount(true);
		expect(screen.queryByRole('heading', { name: 'Carried without a hotkey' })).toBeNull();
	});
});
