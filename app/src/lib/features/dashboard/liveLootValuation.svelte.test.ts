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
		readSession: vi.fn(async () => ({ loot: LOOT, rewards: [] })),
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
			noTtValuePed: 0,
		});
	});

	it("adds a no-TT item's value at the player's per-unit entry, with no TT beneath it", () => {
		const loot: LootEntry[] = [
			{ name: 'Animal Oil Residue', quantity: 100, ttValue: 10 },
			{ name: 'Strongbox', quantity: 12, ttValue: 0 },
		];
		const feed: MarketHarvestData = {
			...FEED,
			items: [
				...FEED.items,
				{
					...FEED.items[0],
					itemName: 'Strongbox',
					markupPct: null,
					marketObservedAt: null,
					preferred: 'manual',
					manual: {
						kind: 'ped_per_unit',
						value: 0.7,
						unitTtPed: 0,
						markupPct: null,
						recordedAt: 1_700_000_100,
					},
				},
			],
		};
		const view = valueLiveLoot('s1', loot, feed, 'all');
		expect(view?.coverage.noTtValuePed).toBeCloseTo(8.4, 9);
		expect(view?.marketValuePed).toBeCloseTo(10.4 + 8.4, 9);
		expect(view?.lootMuFactor).toBeCloseTo((10.4 + 8.4) / 10, 9);
	});

	it('gives no figure without the market signals', () => {
		expect(valueLiveLoot('s1', LOOT, null, 'all')).toBeNull();
	});

	it('values the confirmed rewards apart from the loot', () => {
		const feed: MarketHarvestData = {
			...FEED,
			items: [
				...FEED.items,
				{ ...FEED.items[0], itemName: 'Blazar Fragment', markupPct: 30_000, readings: [] },
			],
		};
		const view = valueLiveLoot('s1', LOOT, feed, 'all', { live: true }, [
			{ itemName: 'Universal Ammo', quantity: 136_177, valuePed: 13.61, accounting: 'liquid' },
			{ itemName: 'Blazar Fragment', quantity: 103, valuePed: 0.001, accounting: 'stock' },
		]);
		// The loot's own figures are untouched by the reward.
		expect(view?.lootTt).toBe(12);
		expect(view?.reward?.ttPed).toBeCloseTo(13.611, 9);
		// Ammo at face value; the fragment at its 30,000% market.
		expect(view?.reward?.muPed).toBeCloseTo(13.61 + 0.3, 9);
		expect(valueLiveLoot('s1', LOOT, feed, 'all')?.reward).toBeNull();
	});
});

describe('createLiveLootValuation', () => {
	it('reads the loot and then the market for its items, once per change', async () => {
		const io = ports();
		const valuation = createLiveLootValuation(() => 'liquidMiddling', io);
		valuation.sync(active());
		await settle();
		expect(io.readSession).toHaveBeenCalledTimes(1);
		expect(io.readMarket).toHaveBeenCalledWith([
			'Animal Oil Residue',
			'Shrapnel',
			'Unlisted Thing',
		]);
		expect(valuation.view?.lootTt).toBe(12);

		// The same snapshot again changes nothing.
		valuation.sync(active());
		await settle();
		expect(io.readSession).toHaveBeenCalledTimes(1);

		// New loot re-reads the composition; the same items need no market read.
		valuation.sync(active({ returns: 13, kill_count: 4 }));
		await settle();
		expect(io.readSession).toHaveBeenCalledTimes(2);
		expect(io.readMarket).toHaveBeenCalledTimes(1);
	});

	it("reads the session's rewards with its loot and asks the market for their items", async () => {
		const reward = {
			completionId: 1,
			questName: 'AI Daily: Hanna Hendrix',
			completedAt: 1_794_762_846,
			items: [
				{
					itemName: 'HyperStim 10mg',
					quantity: 2,
					valuePed: 0.02,
					accounting: 'stock' as const,
				},
			],
		};
		const io = ports({ readSession: vi.fn(async () => ({ loot: LOOT, rewards: [reward] })) });
		const valuation = createLiveLootValuation(() => 'all', io);
		valuation.sync(active());
		await settle();
		expect(io.readMarket).toHaveBeenCalledWith([
			'Animal Oil Residue',
			'HyperStim 10mg',
			'Shrapnel',
			'Unlisted Thing',
		]);
		expect(valuation.rewardItems).toEqual(reward.items);
		expect(valuation.view?.reward?.ttPed).toBeCloseTo(0.02, 9);
	});

	it('re-reads the session when a quest completes, whatever the loot did', async () => {
		const io = ports();
		const valuation = createLiveLootValuation(() => 'all', io);
		valuation.sync(active());
		await settle();
		expect(io.readSession).toHaveBeenCalledTimes(1);
		valuation.sync(
			active({
				recentEvents: [
					{
						type: 'quest',
						description: 'Quest completed: AI Daily: Hanna Hendrix',
						value: 13.63,
						eventType: 'quest_completed',
						timestamp: '2026-11-15T17:14:06',
						id: 'ne-0',
					},
				],
			} as Partial<TrackingSnapshot>),
		);
		await settle();
		expect(io.readSession).toHaveBeenCalledTimes(2);
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
