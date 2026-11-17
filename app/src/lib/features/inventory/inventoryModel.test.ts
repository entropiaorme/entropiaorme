import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createInventoryModel } from './inventoryModel.svelte';

vi.mock('$lib/api/inventory', () => ({
	addAssetPurchase: vi.fn(),
	deleteAssetPurchase: vi.fn(),
	updateAssetPurchase: vi.fn(),
	captureSaleWindow: vi.fn(),
	takeSaleWindowCapture: vi.fn(),
	confirmInventoryListing: vi.fn(),
	commitInventorySaleDraft: vi.fn(),
	convertItems: vi.fn(),
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

const frames = vi.hoisted(() => ({ handlers: new Map<string, () => void>() }));
vi.mock('@tauri-apps/api/event', async (importOriginal) => ({
	...(await importOriginal<typeof import('@tauri-apps/api/event')>()),
	listen: vi.fn(async (topic: string, handler: () => void) => {
		frames.handlers.set(topic, handler);
		return () => frames.handlers.delete(topic);
	}),
}));

vi.mock('$lib/api/account', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api/account')>()),
	undoArrivalUnknownOrigin: vi.fn(),
	undoStockReconciliation: vi.fn(),
}));

vi.mock('$lib/api/market', () => ({
	getMarketAuctionPacketThreshold: vi.fn(),
	getMarketItemMarkups: vi.fn(),
	MARKET_TOPIC: 'market:updated',
}));

import * as accountApi from '$lib/api/account';
import * as inventoryApi from '$lib/api/inventory';
import * as marketApi from '$lib/api/market';

const mocked = vi.mocked(inventoryApi);
const mockedMarket = vi.mocked(marketApi);

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getLootInventory.mockResolvedValue([
		{
			itemName: 'Animal Oil Residue',
			quantity: 10,
			ttValue: 1,
			listedQuantity: 0,
			unitTt: 0,
			paidMarkup: 0,
			origins: [],
			derivedFrom: [],
		},
	]);
	mocked.getEquipmentInventory.mockResolvedValue([
		{
			id: 'equipment-1',
			name: 'Ares Ring, Improved',
			ttValue: 25,
			markupPaid: 75,
			notes: null,
			acquiredAt: '2026-08-01',
			quantity: 1,
			divisible: true,
			purchases: [],
			origins: [],
			derivedFrom: [],
		},
		{
			id: 'strongboxes',
			name: 'Strongbox',
			ttValue: 20,
			markupPaid: 1800,
			notes: null,
			acquiredAt: '2026-08-02',
			quantity: 200,
			divisible: true,
			purchases: [],
			origins: [],
			derivedFrom: [],
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

	it('re-reads the stock and the history shown when the account records something', async () => {
		const model = createInventoryModel();
		await model.load();
		await model.loadHistory();
		const stop = await model.subscribe();
		mocked.getLootInventory.mockClear();
		mocked.getInventoryHistory.mockClear();
		const recorded = {
			id: 'conversion-7',
			recordedFromAccount: { from: '2026-11-17T14:40:00Z', to: '2026-11-17T14:41:00Z' },
		} as unknown as Awaited<ReturnType<typeof inventoryApi.getInventoryHistory>>[number];
		mocked.getInventoryHistory.mockResolvedValue([recorded]);

		frames.handlers.get('account:updated')?.();

		await vi.waitFor(() => expect(model.allHistory.map((row) => row.id)).toEqual(['conversion-7']));
		expect(mocked.getLootInventory).toHaveBeenCalledTimes(1);
		stop();
		expect(frames.handlers.has('account:updated')).toBe(false);
	});

	it('leaves the history unread on an account frame until it is shown', async () => {
		const model = createInventoryModel();
		await model.load();
		await model.subscribe();
		mocked.getLootInventory.mockClear();

		frames.handlers.get('account:updated')?.();

		await vi.waitFor(() => expect(mocked.getLootInventory).toHaveBeenCalledTimes(1));
		expect(mocked.getInventoryHistory).not.toHaveBeenCalled();
	});

	it('never lets an earlier read that answers late overwrite a later one', async () => {
		const model = createInventoryModel();
		await model.load();
		const held = (quantity: number) => [
			{
				itemName: 'Animal Oil Residue',
				quantity,
				ttValue: quantity / 10,
				listedQuantity: 0,
				unitTt: 0.1,
				paidMarkup: 0,
				origins: [],
				derivedFrom: [],
			},
		];
		let answerEarlier!: (rows: ReturnType<typeof held>) => void;
		mocked.getLootInventory.mockReturnValueOnce(
			new Promise((resolve) => {
				answerEarlier = resolve;
			}),
		);
		mocked.getLootInventory.mockResolvedValueOnce(held(4));

		const earlier = model.refresh();
		const later = model.refresh();
		await later;
		answerEarlier(held(10));
		await earlier;

		expect(model.loot.map((row) => row.heldQty)).toEqual([4]);
	});

	it('does not keep a listener when another cannot attach', async () => {
		const { listen } = await import('@tauri-apps/api/event');
		const stopMarket = vi.fn();
		vi.mocked(listen).mockResolvedValueOnce(stopMarket).mockRejectedValueOnce(new Error('no bus'));
		const model = createInventoryModel();

		await expect(model.subscribe()).rejects.toThrow('no bus');
		expect(stopMarket).toHaveBeenCalledTimes(1);
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
		// The ring, and two hundred strongboxes at their totals.
		expect(model.equipmentTt).toBe(45);
		expect(model.equipmentBasis).toBe(1920);
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

	it('sells part of an asset by naming how many, and all of it otherwise', async () => {
		const model = createInventoryModel();
		await model.load();

		await model.sellEquipmentByTrade({ itemId: 'strongboxes', quantity: 50, soldFor: 480 });
		expect(mocked.commitInventorySaleDraft).toHaveBeenLastCalledWith(
			expect.objectContaining({
				draft: expect.objectContaining({ channel: 'trade', quantity: 50, finalPrice: 480 }),
				holding: expect.objectContaining({ kind: 'equipment', holdingId: 'strongboxes' }),
			}),
		);

		await model.listEquipment({
			itemId: 'strongboxes',
			startingBid: 2000,
			buyout: null,
			listingFee: 1,
			listedAt: null,
		});
		expect(mocked.commitInventorySaleDraft).toHaveBeenLastCalledWith(
			expect.objectContaining({
				draft: expect.objectContaining({ channel: 'auction', quantity: 200 }),
			}),
		);
	});

	it('offers an asset by its units at its average cost', async () => {
		const model = createInventoryModel();
		await model.load();

		expect(model.convertibleItems.find((item) => item.assetId === 'strongboxes')).toMatchObject({
			heldQty: 200,
			unitTt: 0.1,
			paidPerUnit: 9,
			divisible: true,
		});
		expect(model.holdingOptions.find((option) => option.holdingId === 'strongboxes')).toMatchObject(
			{ heldQty: 200, paidPerUnit: 9 },
		);
	});

	it('records buying more of an asset and reads the inventory again', async () => {
		const model = createInventoryModel();
		await model.load();
		mocked.addAssetPurchase.mockResolvedValue(model.equipment[1]);
		const reads = mocked.getEquipmentInventory.mock.calls.length;

		const purchase = { quantity: 100, ttValue: 10, markupPaid: 800, acquiredAt: '2026-08-10' };
		await model.addPurchase(model.equipment[1], purchase);

		expect(mocked.addAssetPurchase).toHaveBeenCalledWith('strongboxes', purchase);
		expect(mocked.getEquipmentInventory.mock.calls.length).toBeGreaterThan(reads);
	});

	it('undoes a stock check as a whole and reads the inventory again', async () => {
		const model = createInventoryModel();
		await model.load();
		const undoCheck = vi.mocked(accountApi.undoStockReconciliation);
		undoCheck.mockResolvedValue(undefined);
		const reads = mocked.getLootInventory.mock.calls.length;

		await model.undoCheck('check-1');

		expect(undoCheck).toHaveBeenCalledWith({ id: 'check-1' });
		expect(mocked.getLootInventory.mock.calls.length).toBeGreaterThan(reads);
	});

	it('takes back stock of unknown origin through the arrival it came from', async () => {
		const model = createInventoryModel();
		await model.load();
		const undoArrival = vi.mocked(accountApi.undoArrivalUnknownOrigin);
		undoArrival.mockResolvedValue(undefined);
		const reads = mocked.getLootInventory.mock.calls.length;

		await model.undo({
			id: 'unknown-7',
			subjectKind: 'loot',
			channel: 'unknownOrigin',
			kind: 'unknownOrigin',
			status: 'held',
			itemName: 'Iron Stone',
			targetItem: null,
			occurredAt: '2026-11-17',
			quantity: 12,
			ttValue: 1.2,
			netMarkup: null,
			activityNetMarkup: null,
			canRevertSale: false,
			canDelete: true,
			undoBlockedReason: null,
			undone: false,
			conversionLines: [],
			pedAmount: null,
			itemised: false,
			derivedFrom: [],
			recordedFromAccount: null,
			reconciliationId: null,
		});

		expect(undoArrival).toHaveBeenCalledWith({ id: 'unknown-7' });
		expect(mocked.undoInventoryRemoval).not.toHaveBeenCalled();
		expect(mocked.getLootInventory.mock.calls.length).toBeGreaterThan(reads);
	});

	it('says why a stock check could not be undone', async () => {
		const model = createInventoryModel();
		await model.load();
		vi.mocked(accountApi.undoStockReconciliation).mockRejectedValue(
			new Error('Nanocube made by it has since been sold'),
		);

		await expect(model.undoCheck('check-1')).rejects.toThrow();
		expect(model.error).toBe('Nanocube made by it has since been sold');
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

	it('commits a captured draft with where each figure came from, dated to its read', async () => {
		const model = createInventoryModel();
		await model.load();
		const fieldSources = [
			{ field: 'startingBid' as const, source: { kind: 'readChecked' as const, text: '42' } },
			{ field: 'quantity' as const, source: { kind: 'typed' as const } },
		];
		const holding = {
			kind: 'loot',
			holdingId: 'Animal Oil Residue',
			name: 'Animal Oil Residue',
			score: 100,
		};
		const fields = {
			itemName: 'Animal Oil Residue',
			quantity: 5,
			ttValue: 0.5,
			auctionFee: 0.5,
			auctionDays: 7,
			startingBid: 42,
			buyout: null,
		};

		await model.createFromDraft({
			fields,
			channel: 'auction',
			holding,
			occurredAt: '2026-11-03T19:42:07',
			fieldSources,
		});
		expect(mocked.commitInventorySaleDraft).toHaveBeenLastCalledWith({
			draft: expect.objectContaining({ source: 'manual', startingBid: 42, auctionDays: 7 }),
			holding,
			occurredAt: '2026-11-03T19:42:07',
			fieldSources,
		});

		// A trade records no listing, so it keeps no figure's source.
		await model.createFromDraft({
			fields: { ...fields, buyout: 50 },
			channel: 'trade',
			holding,
			occurredAt: null,
			fieldSources,
		});
		expect(mocked.commitInventorySaleDraft).toHaveBeenLastCalledWith(
			expect.objectContaining({ fieldSources: null }),
		);

		// A listing typed by hand carries no sources.
		await model.createFromDraft({ fields, channel: 'auction', holding, occurredAt: null });
		expect(mocked.commitInventorySaleDraft).toHaveBeenLastCalledWith({
			draft: expect.objectContaining({ source: 'manual' }),
			holding,
			occurredAt: null,
			fieldSources: null,
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
			{
				itemName: 'Shrapnel',
				quantity: 10_000,
				ttValue: 10,
				listedQuantity: 0,
				unitTt: 0,
				paidMarkup: 0,
				origins: [],
				derivedFrom: [],
			},
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
