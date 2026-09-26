import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConsumableDose, ConsumableDoses } from '$lib/api';

vi.mock('$lib/api', () => ({
	CONSUMABLES_TOPIC: 'consumables:updated',
	getConsumableDoses: vi.fn(),
	removeConsumableDose: vi.fn(),
	restoreConsumableDose: vi.fn(),
	startConsumableDose: vi.fn(),
}));

const listeners = new Map<string, () => void>();
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		listeners.set(topic, handler);
		return () => listeners.delete(topic);
	}),
}));

import * as api from '$lib/api';
import { createDosesModel, UNDO_SECONDS } from './dosesModel.svelte';

const mocked = vi.mocked(api);

function dose(overrides: Partial<ConsumableDose> = {}): ConsumableDose {
	return {
		id: 'd1',
		equipmentId: 40,
		itemName: 'Nanobots - Adrenaline Boost',
		source: 'hotbar',
		sessionId: 's1',
		startedAt: 1000,
		endsAt: 1600,
		replaced: false,
		costPed: 4.5,
		costTracked: true,
		effects: [],
		removedAt: null,
		removedBy: null,
		...overrides,
	};
}

function readout(doses: ConsumableDose[]): ConsumableDoses {
	return {
		now: 1000,
		doses,
		reloadSpeed: {
			equippedPercent: 0,
			consumedPercent: 10,
			inEffectPercent: 10,
			itemLimitPercent: 15,
			consumedLimitPercent: 20,
			totalLimitPercent: 30,
		},
		options: [
			{
				equipmentId: 40,
				name: 'Nanobots - Adrenaline Boost',
				durationSeconds: 600,
				doseCostPed: 4.5,
				costTracked: true,
				reloadSpeedPercent: 10,
				hotbarSlot: '5',
			},
		],
	};
}

async function settle() {
	for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
	vi.clearAllMocks();
	listeners.clear();
});

describe('the live dose readout', () => {
	it('reads on connect and re-reads on every doses frame', async () => {
		let clock = 1100;
		mocked.getConsumableDoses.mockResolvedValue(readout([dose()]));
		const model = createDosesModel({ includeOnUse: false, clock: () => clock });
		const detach = model.connect();
		await settle();
		expect(model.rows.map((row) => row.id)).toEqual(['d1']);
		expect(model.ticking).toBe(true);

		mocked.getConsumableDoses.mockResolvedValue(readout([]));
		listeners.get('consumables:updated')?.();
		await settle();
		expect(model.rows).toEqual([]);
		detach();
		clock = 0;
	});

	it('counts down from the stored end with its own tick', async () => {
		let clock = 1100;
		mocked.getConsumableDoses.mockResolvedValue(readout([dose()]));
		const model = createDosesModel({ includeOnUse: false, clock: () => clock });
		model.connect();
		await settle();
		clock = 1700;
		model.tick();
		expect(model.now).toBe(1700);
		// Ended 100 s ago: past the re-dose window, so off the readout.
		expect(model.rows).toEqual([]);
	});

	it('offers a removal back for a short while', async () => {
		let clock = 1100;
		mocked.getConsumableDoses.mockResolvedValue(readout([dose()]));
		mocked.removeConsumableDose.mockResolvedValue(readout([]));
		mocked.restoreConsumableDose.mockResolvedValue(readout([dose()]));
		const model = createDosesModel({ includeOnUse: false, clock: () => clock });
		model.connect();
		await settle();

		await model.remove(dose());
		expect(mocked.removeConsumableDose).toHaveBeenCalledWith('d1');
		expect(model.undoable?.dose.id).toBe('d1');
		clock += UNDO_SECONDS + 1;
		model.tick();
		expect(model.undoable).toBeNull();

		clock = 1105;
		model.tick();
		await model.restore(dose());
		expect(mocked.restoreConsumableDose).toHaveBeenCalledWith('d1');
		expect(model.undoable).toBeNull();
	});

	it('says why an action failed and keeps the readout', async () => {
		mocked.getConsumableDoses.mockResolvedValue(readout([dose()]));
		mocked.startConsumableDose.mockRejectedValue(new Error('Consumable not found'));
		const model = createDosesModel({ includeOnUse: false, clock: () => 1100 });
		model.connect();
		await settle();
		expect(await model.start(40)).toBe(false);
		expect(model.error).toBeTruthy();
		expect(model.rows).toHaveLength(1);
		model.dismissError();
		expect(model.error).toBeNull();
	});
});
