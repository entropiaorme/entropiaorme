// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CostPlacements } from '$lib/api/commands.gen';
import { exact } from './__fixtures__/measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getCostPlacements: vi.fn(),
	placeCostItem: vi.fn(),
	clearCostPlacement: vi.fn(),
}));

vi.mock('$lib/api/account', () => api);
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

import { createCostPlacementsModel, placementAnnouncement } from './costPlacementsModel.svelte';

const AWAITING: CostPlacements = {
	awaiting: [{ item: 'Odd Thing', sessions: 2, lost: exact(0.2), units: null }],
	placed: [],
};
const PLACED: CostPlacements = {
	awaiting: [],
	placed: [{ item: 'Odd Thing', category: 'weapon', placedAt: '2026-11-16T20:00:00Z' }],
};

/** A promise the test settles by hand. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('what a placement says it did', () => {
	it('names where the item went, or that its placement was cleared', () => {
		expect(placementAnnouncement('weapon')).toBe('Placed as Weapon');
		expect(placementAnnouncement('fieldLogistics')).toBe('Placed as Field logistics');
		expect(placementAnnouncement('notACost')).toBe('Placed as not a cost');
		expect(placementAnnouncement(null)).toBe('Placement cleared');
	});
});

describe('the placements model', () => {
	it('never lets a read begun before a placement put the placed item back among those awaiting', async () => {
		const stale = deferred<CostPlacements>();
		api.getCostPlacements.mockReturnValueOnce(stale.promise);
		api.placeCostItem.mockResolvedValue({ placements: PLACED, pendingSessions: [] });
		const model = createCostPlacementsModel();
		const read = model.refresh();

		expect(await model.place('Odd Thing', 'weapon')).toBe(true);
		expect(model.placedAs('Odd Thing')).toBe('weapon');
		// The read began before the placement answered: it lands, and is dropped.
		stale.resolve(AWAITING);
		await read;
		expect(model.placements).toEqual(PLACED);
		expect(model.announcement).toBe('Placed as Weapon');
		expect(model.lastPlacement).toEqual({ item: 'Odd Thing', category: 'weapon', seq: 1 });
	});

	it('takes one placement at a time, never a second choice made while one is saving', async () => {
		const answer = deferred<{ placements: CostPlacements; pendingSessions: string[] }>();
		api.placeCostItem.mockReturnValueOnce(answer.promise);
		const model = createCostPlacementsModel();

		const first = model.place('Odd Thing', 'weapon');
		expect(model.pending).toBe('Odd Thing');
		expect(await model.place('Other Thing', 'travel')).toBe(false);
		expect(api.placeCostItem).toHaveBeenCalledTimes(1);

		answer.resolve({ placements: PLACED, pendingSessions: [] });
		expect(await first).toBe(true);
		expect(model.pending).toBeNull();
	});

	it("keeps the backend's reason beside the item a placement failed for", async () => {
		api.placeCostItem.mockRejectedValue(new Error('That item is not one the readings hold'));
		const model = createCostPlacementsModel();
		expect(await model.place('Odd Thing', 'travel')).toBe(false);
		expect(model.failure).toEqual({
			item: 'Odd Thing',
			message: 'That item is not one the readings hold',
		});
		expect(model.announcement).toBe('');
	});

	it('reads the figures again before saying it is done', async () => {
		const order: string[] = [];
		api.clearCostPlacement.mockResolvedValue({ placements: AWAITING, pendingSessions: [] });
		const model = createCostPlacementsModel({
			onChanged: async () => {
				order.push(`read again (${model.announcement || 'nothing said'})`);
			},
		});
		expect(await model.clear('Odd Thing')).toBe(true);
		expect(order).toEqual(['read again (nothing said)']);
		expect(model.announcement).toBe('Placement cleared');
	});
});
