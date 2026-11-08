import { describe, expect, it, vi } from 'vitest';
import type { LootEntry, MarketHarvestData, TrackingSnapshot } from '$lib/api';
import {
	createLiveLootValuation,
	type LiveLootValuationPorts,
	valueLiveLoot,
} from './liveLootValuation.svelte';

const FEED: MarketHarvestData = {
	nanocubeMarkupPct: 100.6,
	items: [
		{
			itemName: 'Animal Oil Residue',
			markupPct: 104,
			marketObservedAt: 1_700_000_000,
			manual: null,
			preferred: 'market',
			nanocubeMarked: false,
			unitPricePed: null,
			horizon: 'week',
			salesPed: 50_000,
			recommendedPacketTt: null,
			readings: [{ horizon: 'week', markupPct: 104, salesPed: 50_000 }],
		},
	],
};

const LOOT: LootEntry[] = [
	{ name: 'Animal Oil Residue', quantity: 100, ttValue: 10 },
	{ name: 'Shrapnel', quantity: 10_000, ttValue: 1 },
	{ name: 'Unlisted Thing', quantity: 1, ttValue: 1 },
];

function active(overrides: Partial<TrackingSnapshot> = {}): TrackingSnapshot {
	return {
		status: 'active',
		session_id: 's1',
		returns: 12,
		kill_count: 3,
		cost: 10,
		...overrides,
	} as TrackingSnapshot;
}

function ports(overrides: Partial<LiveLootValuationPorts> = {}): LiveLootValuationPorts {
	return {
		readLoot: vi.fn(async () => LOOT),
		readMarket: vi.fn(async () => FEED),
		followMarket: vi.fn(async () => () => {}),
		...overrides,
	};
}

/** Let every pending read settle. */
async function settle() {
	for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

describe('valueLiveLoot', () => {
	it('values each item by its route and reads the markup factor over loot TT', () => {
		const view = valueLiveLoot('s1', LOOT, FEED, 'liquidMiddling');
		expect(view).not.toBeNull();
		// Oil at its 104% reading, Shrapnel at 101%, the unlisted item at Nanocube.
		expect(view?.marketValuePed).toBeCloseTo(10.4 + 1.01 + 1.006, 9);
		expect(view?.lootTt).toBe(12);
		expect(view?.lootMuFactor).toBeCloseTo((10.4 + 1.01 + 1.006) / 12, 9);
		expect(view?.coverage).toEqual({
			marketTt: 10,
			manualTt: 0,
			flooredTt: 1,
			markedTt: 0,
			shrapnelTt: 1,
		});
	});

	it('gives no figure without the market signals', () => {
		expect(valueLiveLoot('s1', LOOT, null, 'all')).toBeNull();
	});
});

describe('createLiveLootValuation', () => {
	it('reads the loot and then the market for its items, once per change', async () => {
		const io = ports();
		const valuation = createLiveLootValuation(() => 'liquidMiddling', io);
		valuation.sync(active());
		await settle();
		expect(io.readLoot).toHaveBeenCalledTimes(1);
		expect(io.readMarket).toHaveBeenCalledWith([
			'Animal Oil Residue',
			'Shrapnel',
			'Unlisted Thing',
		]);
		expect(valuation.view?.lootTt).toBe(12);

		// The same snapshot again changes nothing.
		valuation.sync(active());
		await settle();
		expect(io.readLoot).toHaveBeenCalledTimes(1);

		// New loot re-reads the composition; the same items need no market read.
		valuation.sync(active({ returns: 13, kill_count: 4 }));
		await settle();
		expect(io.readLoot).toHaveBeenCalledTimes(2);
		expect(io.readMarket).toHaveBeenCalledTimes(1);
	});

	it('re-reads the market on a market write', async () => {
		const io = ports();
		const valuation = createLiveLootValuation(() => 'all', io);
		valuation.sync(active());
		await settle();
		valuation.marketChanged();
		await settle();
		expect(io.readMarket).toHaveBeenCalledTimes(2);
	});

	it('starts afresh for a new session and clears when idle', async () => {
		const io = ports();
		const valuation = createLiveLootValuation(() => 'all', io);
		valuation.sync(active());
		await settle();
		valuation.sync(active({ session_id: 's2' }));
		expect(valuation.view).toBeNull();
		await settle();
		expect(valuation.view?.sessionId).toBe('s2');

		valuation.sync({ status: 'idle' } as TrackingSnapshot);
		expect(valuation.view).toBeNull();
		expect(valuation.loading).toBe(false);
	});

	it('reports a failed market read rather than valuing at TT', async () => {
		const io = ports({ readMarket: vi.fn(async () => Promise.reject(new Error('offline'))) });
		const valuation = createLiveLootValuation(() => 'all', io);
		valuation.sync(active());
		await settle();
		expect(valuation.marketFailed).toBe(true);
		expect(valuation.view).toBeNull();
		expect(valuation.loot).toEqual(LOOT);
	});
});
