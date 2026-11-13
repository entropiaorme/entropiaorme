// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { Equipment, HealingTool } from '$lib/types';
import { shownMarkup } from '$lib/view/__fixtures__/markup';
import EquipmentListView from './EquipmentListView.svelte';
import type { LibraryModel } from './libraryModel.svelte';

function entry(id: string, name: string, overrides: Partial<Equipment> = {}): Equipment {
	return {
		id,
		name,
		type: 'weapon',
		enabled: true,
		amplifierName: null,
		costPerUse: 1.5,
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

const healer: HealingTool = {
	id: '3',
	name: 'FAP',
	enabled: false,
	costPerHeal: 3,
	isLimited: false,
	reloadSeconds: 2,
	profile: null,
};
const finders = [
	entry('4', 'Finder', { type: 'tool', amplifierName: 'F101' }),
	entry('5', 'Finder', { type: 'tool' }),
];
const weapons = [entry('1', 'Pistol'), entry('2', 'Rifle', { enabled: false })];

function model(toolReading: boolean) {
	return {
		loading: false,
		allEquipment: [...weapons, entry('3', 'FAP', { type: 'healing', enabled: false }), ...finders],
		sortedEquipment: weapons,
		consumables: [],
		healingTools: [healer],
		harvestingTools: finders,
		reloadSpeed: null,
		expandedId: null,
		detailCache: {},
		toolReading,
		indistinguishable: [['4', '5']],
		lookalikeLabels: { '4': 'Finder + F101', '5': 'Finder' },
		toggleExpand: vi.fn(),
		openAddModal: vi.fn(),
		openEditModal: vi.fn(),
		removeEquipment: vi.fn(),
		setEnabled: vi.fn(async () => {}),
	} as unknown as LibraryModel & { setEnabled: ReturnType<typeof vi.fn> };
}

describe('the equipment list', () => {
	it('is as it was while the tool in hand is not read off the screen', async () => {
		const { container } = render(EquipmentListView, { props: { model: model(false) } });
		expect(screen.queryAllByRole('switch')).toHaveLength(0);
		expect(screen.queryByTestId('lookalike-note')).toBeNull();
		expect(screen.queryByTestId('tool-reading-mark')).toBeNull();
		// The markup the list drew before reading the held tool existed.
		await expect(shownMarkup(container.innerHTML)).toMatchFileSnapshot(
			'./__fixtures__/EquipmentListView.off.html',
		);
	});

	it('switches healing and harvesting entries, mutes one that is off, and flags look-alikes', async () => {
		const reading = model(true);
		render(EquipmentListView, { props: { model: reading } });
		expect(screen.getByTestId('tool-reading-mark')).toBeTruthy();
		const fap = screen.getByRole('switch', { name: 'FAP in use' });
		expect(fap.getAttribute('aria-checked')).toBe('false');
		expect(screen.getByText('FAP').className).toContain('text-text-tertiary');
		await fireEvent.click(fap);
		expect(reading.setEnabled).toHaveBeenCalledWith('3', true);
		expect(screen.getByRole('switch', { name: 'Finder + F101 in use' })).toBeTruthy();
		const notes = screen.getAllByTestId('lookalike-note').map((note) => note.textContent ?? '');
		expect(
			notes.some((text) => text.includes("Looks the same as Finder + F101 on the game's bar.")),
		).toBe(true);
		expect(screen.getAllByTestId('lookalike-leave-one')).toHaveLength(2);
	});
});
