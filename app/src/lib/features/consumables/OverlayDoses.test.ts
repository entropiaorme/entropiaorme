// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConsumableDose, ConsumableDoses } from '$lib/api';

vi.mock('$lib/api', () => ({
	CONSUMABLES_TOPIC: 'consumables:updated',
	getConsumableDoses: vi.fn(),
	removeConsumableDose: vi.fn(),
	restoreConsumableDose: vi.fn(),
	startConsumableDose: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
}));

import * as api from '$lib/api';
import { createDosesModel } from './dosesModel.svelte';
import OverlayDoses from './OverlayDoses.svelte';

const mocked = vi.mocked(api);

function dose(overrides: Partial<ConsumableDose> = {}): ConsumableDose {
	return {
		id: 'd1',
		equipmentId: 40,
		itemName: 'Adrenaline',
		source: 'hotbar',
		sessionId: 's1',
		startedAt: 1000,
		endsAt: 1600,
		replaced: false,
		costPed: 4.5,
		costTracked: true,
		effects: [{ name: 'Reload Speed Increased', strength: 10, unit: '%', reloadSpeedPercent: 10 }],
		removedAt: null,
		removedBy: null,
		...overrides,
	};
}

function readout(doses: ConsumableDose[], withOptions = true): ConsumableDoses {
	return {
		now: 1000,
		doses,
		reloadSpeed: {
			equippedPercent: 0,
			consumedPercent: 0,
			inEffectPercent: 0,
			itemLimitPercent: 15,
			consumedLimitPercent: 20,
			totalLimitPercent: 30,
		},
		options: withOptions
			? [
					{
						equipmentId: 40,
						name: 'Adrenaline',
						durationSeconds: 600,
						doseCostPed: 4.5,
						costTracked: true,
						reloadSpeedPercent: 10,
						hotbarSlot: null,
					},
				]
			: [],
	};
}

async function renderWith(data: ConsumableDoses, clock: () => number, onStartTrigger = vi.fn()) {
	mocked.getConsumableDoses.mockResolvedValue(data);
	const model = createDosesModel({ includeOnUse: false, clock });
	model.connect();
	for (let i = 0; i < 5; i++) await Promise.resolve();
	render(OverlayDoses, { props: { model, onStartTrigger } });
	return model;
}

beforeEach(() => vi.clearAllMocks());

describe('the overlay doses', () => {
	it('is absent for play without consumables', async () => {
		await renderWith(readout([], false), () => 1100);
		expect(screen.queryByTestId('overlay-doses')).toBeNull();
	});

	it('counts a running dose down and removes it on the X', async () => {
		mocked.removeConsumableDose.mockResolvedValue(readout([]));
		await renderWith(readout([dose()]), () => 1100);
		expect(screen.getByTestId('overlay-dose').textContent).toContain('8:20');
		await fireEvent.click(screen.getByRole('button', { name: 'Remove this dose of Adrenaline' }));
		expect(mocked.removeConsumableDose).toHaveBeenCalledWith('d1');
	});

	it('offers a re-dose once a dose has ended', async () => {
		mocked.startConsumableDose.mockResolvedValue(readout([]));
		await renderWith(readout([dose()]), () => 1620);
		expect(screen.getByTestId('overlay-dose').textContent).toContain('Ended');
		await fireEvent.click(screen.getByRole('button', { name: 'Take another dose of Adrenaline' }));
		expect(mocked.startConsumableDose).toHaveBeenCalledWith(40);
	});

	it('opens the menu of consumables from its start control', async () => {
		const trigger = vi.fn();
		await renderWith(readout([]), () => 1100, trigger);
		await fireEvent.click(screen.getByRole('button', { name: 'Take a dose' }));
		expect(trigger).toHaveBeenCalledTimes(1);
	});

	it('never shows a heal buff, which has no misclick to undo', async () => {
		await renderWith(readout([dose({ source: 'on_use' })]), () => 1100);
		expect(screen.queryByTestId('overlay-dose')).toBeNull();
	});
});
