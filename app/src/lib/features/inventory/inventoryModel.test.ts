import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createInventoryModel } from './inventoryModel.svelte';

vi.mock('$lib/api/inventory', () => ({
	captureSaleWindow: vi.fn(),
	takeSaleWindowCapture: vi.fn(),
	confirmInventoryListing: vi.fn(),
	commitInventorySaleDraft: vi.fn(),
	convertLoot: vi.fn(),
	convertLootShrapnel: vi.fn(),
	deleteEquipmentHolding: vi.fn(),
	expireInventoryListing: vi.fn(),
	getEquipmentInventory: vi.fn(),
	getInventoryHistory: vi.fn(),
	getInventoryListings: vi.fn(),
	getLootInventory: vi.fn(),
	removeLoot: vi.fn(),
	revertInventorySale: vi.fn(),
	undoInventoryConversion: vi.fn(),
	undoInventoryListing: vi.fn(),
	undoInventoryRemoval: vi.fn(),
	undoInventoryTrade: vi.fn(),
}));

vi.mock('$lib/api/market', () => ({
	getMarketAuctionPacketThreshold: vi.fn(),
	getMarketItemMarkups: vi.fn(),
	MARKET_TOPIC: 'market:updated',
}));

import * as inventoryApi from '$lib/api/inventory';
import * as marketApi from '$lib/api/market';

const mocked = vi.mocked(inventoryApi);
const mockedMarket = vi.mocked(marketApi);

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getLootInventory.mockResolvedValue([
		{ itemName: 'Animal Oil Residue', quantity: 10, ttValue: 1, listedQuantity: 0 },
	]);
	mocked.getEquipmentInventory.mockResolvedValue([
		{
			id: 'equipment-1',
			name: 'Ares Ring, Improved',
			ttValue: 25,
			markupPaid: 75,
			notes: null,
			acquiredAt: '2026-08-01',
		},
	]);
	mocked.getInventoryListings.mockResolvedValue([]);
	mocked.getInventoryHistory.mockResolvedValue([]);
	mocked.commitInventorySaleDraft.mockResolvedValue(undefined);
	mockedMarket.getMarketItemMarkups.mockResolvedValue({
		nanocubeMarkupPct: 100.6,
		items: [
			{
				itemName: 'Animal Oil Residue',
				markupPct: 125,
				unitPricePed: null,
				marketObservedAt: 1_700_000_000,
				manual: null,
				preferred: 'market',
				nanocubeMarked: false,
				horizon: 'week',
				salesPed: 1_000,
				recommendedPacketTt: 39.2,
				readings: [
					{ horizon: 'day', markupPct: 126, salesPed: 100 },
					{ horizon: 'week', markupPct: 125, salesPed: 1_000 },
				],
			},
		],
	});
	mockedMarket.getMarketAuctionPacketThreshold.mockResolvedValue({
		maxFeeSharePct: 10,
		grossMarkupPed: 9.8,
	});
});

describe('inventory market valuation', () => {
	it('reads the market for exactly the items held', async () => {
		const model = createInventoryModel();
		await model.load();
		expect(mockedMarket.getMarketItemMarkups).toHaveBeenCalledWith(['Animal Oil Residue']);
	});

	it("values a holding at the player's own markup when it is the more recent", async () => {
		mockedMarket.getMarketItemMarkups.mockResolvedValue({
			nanocubeMarkupPct: 100.6,
			items: [
				{
					itemName: 'Animal Oil Residue',
					markupPct: 125,
					unitPricePed: null,
					marketObservedAt: 1_700_000_000,
					manual: {
						kind: 'ped_per_unit',
						value: 0.05,
						unitTtPed: 0.1,
						markupPct: 150,
						recordedAt: 1_700_000_100,
					},
					preferred: 'manual',
					nanocubeMarked: false,
					horizon: 'week',
					salesPed: 1_000,
					recommendedPacketTt: null,
					readings: [],
				},
			],
		});
		const model = createInventoryModel();
		await model.load();
		// 10 units at 0.1 PED TT each, plus 0.05 PED each: 150% of TT.
		expect(model.loot[0]).toMatchObject({
			markupBasis: 'manual',
			markupPct: 125,
			floored: false,
		});
		expect(model.loot[0].effectiveMarkupPct).toBeCloseTo(150, 6);
		expect(model.loot[0].marketItem?.manual?.kind).toBe('ped_per_unit');
	});

	it('re-reads the market when a market write lands', async () => {
		const model = createInventoryModel();
		await model.load();
		mockedMarket.getMarketItemMarkups.mockClear();
		await model.refreshMarket();
		expect(mockedMarket.getMarketItemMarkups).toHaveBeenCalledWith(['Animal Oil Residue']);
	});
});

describe('central inventory model', () => {
	it('loads pooled loot and equipment as separate holding families', async () => {
		const model = createInventoryModel();
		await model.load();

		expect(model.lootTt).toBe(1);
		expect(model.equipmentTt).toBe(25);
		expect(model.equipmentBasis).toBe(100);
		expect(model.loot[0]).toMatchObject({
			markupPct: 125,
			effectiveMarkupPct: 125,
			tier: 'liquid',
			markupHorizon: 'week',
			recommendedPacketTt: 39.2,
		});
		expect(model.loading).toBe(false);
	});

	it('recomputes packet TT from a selected listing-fee cap', async () => {
		const model = createInventoryModel();
		await model.load();
		mockedMarket.getMarketAuctionPacketThreshold.mockResolvedValueOnce({
			maxFeeSharePct: 5,
			grossMarkupPed: 126,
		});

		await model.setPacketFeeSharePct(5);

		expect(model.packetFeeSharePct).toBe(5);
		expect(model.loot[0].recommendedPacketTt).toBe(504);
	});

	it('clears a packet fee-cap error after a successful retry', async () => {
		const model = createInventoryModel();
		await model.load();
		mockedMarket.getMarketAuctionPacketThreshold
			.mockRejectedValueOnce(new Error('fee model unavailable'))
			.mockResolvedValueOnce({ maxFeeSharePct: 15, grossMarkupPed: 4.94 });

		await model.setPacketFeeSharePct(5);
		expect(model.error).toBe('fee model unavailable');

		await model.setPacketFeeSharePct(15);
		expect(model.error).toBeNull();
		expect(model.packetFeeSharePct).toBe(15);
	});

	it('turns a manual loot listing into the same reviewed draft an OCR intake will use', async () => {
		const model = createInventoryModel();
		await model.load();

		await model.listLoot({
			itemName: 'Animal Oil Residue',
			quantity: 5,
			startingBid: 1.1,
			buyout: 1.25,
			listingFee: 0.5,
			listedAt: '2026-08-10',
		});

		expect(mocked.commitInventorySaleDraft).toHaveBeenCalledWith({
			draft: expect.objectContaining({
				source: 'manual',
				channel: 'auction',
				observedName: 'Animal Oil Residue',
				quantity: 5,
				startingBid: 1.1,
				buyout: 1.25,
				listingFee: 0.5,
			}),
			holding: {
				kind: 'loot',
				holdingId: 'Animal Oil Residue',
				name: 'Animal Oil Residue',
				score: 100,
			},
			occurredAt: '2026-08-10',
		});
	});

	it('applies the shared confidence threshold without hiding the observed markup', async () => {
		mockedMarket.getMarketItemMarkups.mockResolvedValue({
			nanocubeMarkupPct: 100.6,
			items: [
				{
					itemName: 'Animal Oil Residue',
					markupPct: 110,
					unitPricePed: null,
					marketObservedAt: 1_700_000_000,
					manual: null,
					preferred: 'market',
					nanocubeMarked: false,
					horizon: 'month',
					salesPed: 100,
					recommendedPacketTt: 98,
					readings: [
						{ horizon: 'week', markupPct: null, salesPed: 0 },
						{ horizon: 'month', markupPct: 110, salesPed: 100 },
					],
				},
			],
		});
		const model = createInventoryModel();
		await model.load();

		expect(model.loot[0]).toMatchObject({
			markupPct: 110,
			effectiveMarkupPct: 100.6,
			tier: 'illiquid',
			floored: true,
		});

		model.confidenceMode = 'all';
		expect(model.loot[0]).toMatchObject({
			markupPct: 110,
			effectiveMarkupPct: 110,
			floored: false,
		});
	});

	it('uses fixed 101% Shrapnel conversion value while retaining the market observation', async () => {
		mocked.getLootInventory.mockResolvedValue([
			{ itemName: 'Shrapnel', quantity: 10_000, ttValue: 10, listedQuantity: 0 },
		]);
		mockedMarket.getMarketItemMarkups.mockResolvedValue({
			nanocubeMarkupPct: 100.6,
			items: [
				{
					itemName: 'Shrapnel',
					markupPct: 100.9,
					unitPricePed: null,
					marketObservedAt: 1_700_000_000,
					manual: null,
					preferred: 'market',
					nanocubeMarked: false,
					horizon: 'week',
					salesPed: 20_000,
					recommendedPacketTt: null,
					readings: [{ horizon: 'week', markupPct: 100.9, salesPed: 20_000 }],
				},
			],
		});

		const model = createInventoryModel();
		await model.load();

		expect(model.loot[0]).toMatchObject({
			markupPct: 100.9,
			effectiveMarkupPct: 101,
			markupBasis: 'shrapnel_conversion',
			floored: false,
		});
		model.confidenceMode = 'all';
		expect(model.loot[0].effectiveMarkupPct).toBe(101);
	});
});
