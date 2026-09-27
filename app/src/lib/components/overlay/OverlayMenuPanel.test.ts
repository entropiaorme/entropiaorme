// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ConsumableOption } from '$lib/api';
import { buildConsumablesMenuState } from '$lib/windows/overlayMenu';
import OverlayMenuPanel from './OverlayMenuPanel.svelte';

function option(overrides: Partial<ConsumableOption> = {}): ConsumableOption {
	return {
		equipmentId: 40,
		name: 'Adrenaline',
		durationSeconds: 3600,
		doseCostPed: 4.5,
		costTracked: true,
		reloadSpeedPercent: 10,
		hotbarSlot: null,
		...overrides,
	};
}

function renderMenu(options: ConsumableOption[], runningIds: number[] = []) {
	const onSelect = vi.fn();
	render(OverlayMenuPanel, {
		props: {
			menuState: buildConsumablesMenuState(120, options, runningIds),
			onSelect,
			onActivitySelect: vi.fn(),
		},
	});
	return onSelect;
}

describe('the consumables menu', () => {
	it('takes a timed dose from the row', async () => {
		const onSelect = renderMenu([option()]);
		await fireEvent.click(screen.getByTitle('Take a dose of Adrenaline'));
		expect(onSelect).toHaveBeenCalledWith({ kind: 'consumables', equipmentId: 40, untimed: false });
	});

	it('adds an effect already in force, with no timer, from its own control', async () => {
		const onSelect = renderMenu([option()]);
		await fireEvent.click(screen.getByRole('button', { name: 'Adrenaline is already in effect' }));
		expect(onSelect).toHaveBeenCalledWith({ kind: 'consumables', equipmentId: 40, untimed: true });
	});

	it('offers it only for a lasting item not already running', () => {
		renderMenu(
			[
				option(),
				option({ equipmentId: 41, name: 'Heal Pill', durationSeconds: 0 }),
				option({ equipmentId: 42, name: 'Rush' }),
			],
			[42],
		);
		expect(screen.getAllByRole('button', { name: /is already in effect/ })).toHaveLength(1);
	});
});
