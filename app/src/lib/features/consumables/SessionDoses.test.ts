// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConsumableDose } from '$lib/api';

vi.mock('$lib/api', () => ({
	getSessionDoses: vi.fn(),
	removeConsumableDose: vi.fn(),
	restoreConsumableDose: vi.fn(),
}));

import * as api from '$lib/api';
import SessionDoses from './SessionDoses.svelte';
import { createSessionDosesModel } from './sessionDosesModel.svelte';

const mocked = vi.mocked(api);

function dose(overrides: Partial<ConsumableDose> = {}): ConsumableDose {
	return {
		id: 'd1',
		equipmentId: 40,
		itemName: 'Adrenaline',
		source: 'hotbar',
		sessionId: 's1',
		startedAt: 1000,
		endsAt: 4600,
		replaced: false,
		costPed: 4.5,
		costTracked: true,
		effects: [],
		removedAt: null,
		removedBy: null,
		...overrides,
	};
}

async function renderWith(doses: ConsumableDose[]) {
	mocked.getSessionDoses.mockResolvedValue(doses);
	const model = createSessionDosesModel(() => 's1');
	await model.refresh();
	render(SessionDoses, { props: { model } });
	return model;
}

beforeEach(() => vi.clearAllMocks());

describe("a session's doses", () => {
	it('is absent for a session that took none', async () => {
		await renderWith([]);
		expect(screen.queryByTestId('session-doses')).toBeNull();
	});

	it('lists what each dose booked and removes one', async () => {
		const model = await renderWith([dose(), dose({ id: 'd2', costPed: 0, costTracked: false })]);
		expect(model.bookedPed).toBeCloseTo(4.5);
		expect(screen.getAllByTestId('session-dose')).toHaveLength(2);
		expect(screen.getByText('cost not tracked', { exact: false })).toBeTruthy();
		mocked.removeConsumableDose.mockResolvedValue({} as never);
		mocked.getSessionDoses.mockResolvedValue([dose({ removedAt: 2000, removedBy: 'player' })]);
		await fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0]);
		expect(mocked.removeConsumableDose).toHaveBeenCalledWith('d1');
	});

	it('restores a removed dose and leaves a heal buff to its heal', async () => {
		await renderWith([
			dose({ removedAt: 2000, removedBy: 'player' }),
			dose({ id: 'b', source: 'on_use', costPed: 0 }),
		]);
		mocked.restoreConsumableDose.mockResolvedValue({} as never);
		await fireEvent.click(screen.getByRole('button', { name: 'Restore' }));
		expect(mocked.restoreConsumableDose).toHaveBeenCalledWith('d1');
		// The buff offers no action of its own.
		expect(screen.getAllByRole('button')).toHaveLength(1);
	});
});
