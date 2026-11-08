import { describe, expect, it, vi } from 'vitest';
import type { LootEntry, MarketHarvestData, TrackingSnapshot } from '$lib/api';
import {
	createLifetimeLootValuation,
	type LifetimeLootValuationPorts,
	mergeLoot,
} from './lifetimeLootValuation.svelte';

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

const FAMILY: LootEntry[] = [{ name: 'Animal Oil Residue', quantity: 100, ttValue: 10 }];

function lifetime(instanceCount: number, lootTt = 10) {
	return { instanceCount, lootTt } as TrackingSnapshot['lifetime'];
}

function idle(overrides: Partial<TrackingSnapshot> = {}): TrackingSnapshot {
	return {
		status: 'idle',
		sessionDefinitionId: '7',
		lifetime: lifetime(2),
		...overrides,
	} as TrackingSnapshot;
}

function active(overrides: Partial<TrackingSnapshot> = {}): TrackingSnapshot {
	return { ...idle(), status: 'active', lifetime: lifetime(3, 12), ...overrides };
}

function ports(overrides: Partial<LifetimeLootValuationPorts> = {}): LifetimeLootValuationPorts {
	return {
		readFamilyLoot: vi.fn(async () => FAMILY),
		readMarket: vi.fn(async () => FEED),
		followMarket: vi.fn(async () => () => {}),
		...overrides,
	};
}

async function settle() {
	for (let i = 0; i < 6; i += 1) await Promise.resolve();
}

function subject(
	seams: LifetimeLootValuationPorts,
	running: () => LootEntry[] | null = () => null,
) {
	return createLifetimeLootValuation(() => 'all', running, seams);
}

describe('mergeLoot', () => {
	it('adds the running instance to the family line by item', () => {
		expect(
			mergeLoot(
				[{ name: 'Oil', quantity: 2, ttValue: 4 }],
				[
					{ name: 'Oil', quantity: 1, ttValue: 2 },
					{ name: 'Hide', quantity: 3, ttValue: 1 },
				],
			),
		).toEqual([
			{ name: 'Oil', quantity: 3, ttValue: 6 },
			{ name: 'Hide', quantity: 3, ttValue: 1 },
		]);
	});
});

describe('createLifetimeLootValuation', () => {
	it('reads nothing while no markup figure is on show', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), false);
		await settle();
		expect(seams.readFamilyLoot).not.toHaveBeenCalled();
		expect(valuation.view).toBeNull();
	});

	it('values the family at market once it is on show', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), true);
		await settle();
		expect(seams.readFamilyLoot).toHaveBeenCalledWith(7);
		expect(valuation.view?.lootTt).toBe(10);
		expect(valuation.view?.lootMuFactor).toBeCloseTo(1.04);
	});

	it('leaves the composition alone while kills land on a running session', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(active(), true);
		await settle();
		valuation.sync(active({ lifetime: lifetime(3, 40) }), true);
		valuation.sync(active({ lifetime: lifetime(3, 90) }), true);
		await settle();
		expect(seams.readFamilyLoot).toHaveBeenCalledTimes(1);
	});

	it('reads again when another instance ends or a past one is edited', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), true);
		await settle();
		valuation.sync(idle({ lifetime: lifetime(3) }), true);
		await settle();
		valuation.sync(idle({ lifetime: lifetime(3, 8) }), true);
		await settle();
		expect(seams.readFamilyLoot).toHaveBeenCalledTimes(3);
	});

	it("values the running instance's loot with the family's", async () => {
		const valuation = subject(ports(), () => [
			{ name: 'Animal Oil Residue', quantity: 10, ttValue: 1 },
		]);
		valuation.sync(active(), true);
		await settle();
		expect(valuation.view?.lootTt).toBe(11);
	});

	it('fetches the market afresh when the figures come back into view', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), true);
		await settle();
		expect(seams.readMarket).toHaveBeenCalledTimes(1);
		valuation.sync(idle(), false);
		valuation.sync(idle(), true);
		await settle();
		expect(seams.readMarket).toHaveBeenCalledTimes(2);
	});

	it('follows a market write only while on show', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), true);
		await settle();
		valuation.marketChanged();
		await settle();
		expect(seams.readMarket).toHaveBeenCalledTimes(2);
		valuation.sync(idle(), false);
		valuation.marketChanged();
		await settle();
		expect(seams.readMarket).toHaveBeenCalledTimes(2);
	});

	it('gives no figure when the market cannot be read', async () => {
		const valuation = subject(
			ports({
				readMarket: vi.fn(async () => {
					throw new Error('down');
				}),
			}),
		);
		valuation.sync(idle(), true);
		await settle();
		expect(valuation.view).toBeNull();
	});

	it('starts over for another definition', async () => {
		const seams = ports();
		const valuation = subject(seams);
		valuation.sync(idle(), true);
		await settle();
		valuation.sync(idle({ sessionDefinitionId: '9' }), true);
		await settle();
		expect(seams.readFamilyLoot).toHaveBeenLastCalledWith(9);
	});

	it('applies no live Nanocube mark to the family', async () => {
		const marked: MarketHarvestData = {
			...FEED,
			items: [{ ...FEED.items[0], nanocubeMarked: true }],
		};
		const valuation = subject(ports({ readMarket: vi.fn(async () => marked) }));
		valuation.sync(idle(), true);
		await settle();
		expect(valuation.view?.lootMuFactor).toBeCloseTo(1.04);
	});
});
