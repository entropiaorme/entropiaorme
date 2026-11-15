import { describe, expect, it } from 'vitest';
import type {
	InventoryItem,
	MarketHarvestData,
	MarketHarvestItem,
	StockPosition,
	StrongboxExportRead,
} from '$lib/api/commands.gen';
import {
	balanceRefusal,
	boxCost,
	boxHoldings,
	contentLine,
	defaultPlan,
	estimateItem,
	exportRefusal,
	isHit,
	isStrongbox,
	type ManualEntry,
	type MarketContext,
	manualAmmo,
	manualRefusal,
	openingInput,
	orderLines,
	planRefusal,
	resultFromExport,
	resultFromManual,
	summarise,
	untrackedBoxes,
} from './openingDraft';

function stock(itemName: string, quantity: number): StockPosition {
	return {
		itemName,
		quantity,
		ttValue: 0,
		listedQuantity: 0,
		unitTt: 0,
		paidMarkup: 0,
		origins: [],
		derivedFrom: [],
	};
}

function asset(id: string, name: string, quantity: number, markupPaid: number): InventoryItem {
	return {
		id,
		name,
		ttValue: 0,
		markupPaid,
		notes: null,
		acquiredAt: '2026-11-01',
		quantity,
		divisible: true,
		purchases: [],
		origins: [],
		derivedFrom: [],
	};
}

function reading(itemName: string, markupPct: number): MarketHarvestItem {
	return {
		itemName,
		markupPct,
		marketObservedAt: 1,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'month',
		salesPed: 50_000,
		recommendedPacketTt: null,
		readings: [
			{ horizon: 'week', markupPct, salesPed: 20_000 },
			{ horizon: 'month', markupPct, salesPed: 50_000 },
		],
	};
}

const market: MarketHarvestData = {
	nanocubeMarkupPct: 102,
	items: [reading('Skeleton Shirt White (F)', 250), reading('Red Heart Fireworks', 101)],
};
const context: MarketContext = { market, mode: 'all' };

/** The eight Halloween Strongboxes opened in game, as the export reads. */
const halloween: StrongboxExportRead = {
	boxesOpened: 8,
	keys: { count: 8, ttValue: 80 },
	ammoPed: 58,
	contents: [
		{ line: 2, itemName: 'Red Heart Fireworks', quantity: 1, valuePed: 2 },
		{ line: 3, itemName: 'Skeleton Shirt White (F)', quantity: 1, valuePed: 10 },
		{ line: 4, itemName: 'Skeleton Shirt White (M)', quantity: 1, valuePed: 10 },
	],
	unexpected: [],
	skipped: [],
	balanceGap: 0,
};

describe('the boxes held', () => {
	it('gathers each strongbox type across loot and assets, most first', () => {
		const held = boxHoldings(
			[
				stock('Halloween Strongbox', 402),
				stock('Animal Oil Residue', 900),
				stock('Easter Strongbox', 111),
			],
			[asset('a1', 'Unreal Strongbox', 160, 320), asset('a2', 'Halloween Strongbox', 10, 50)],
		);
		expect(held.map((row) => [row.name, row.total])).toEqual([
			['Halloween Strongbox', 412],
			['Unreal Strongbox', 160],
			['Easter Strongbox', 111],
		]);
		expect(held[0].lootQty).toBe(402);
		expect(held[0].assets[0].id).toBe('a2');
	});

	it('knows a strongbox by its name', () => {
		expect(isStrongbox('Summer Strongbox')).toBe(true);
		expect(isStrongbox('Strongbox Key')).toBe(false);
	});
});

describe('valuing what came out', () => {
	it('values an item at its own reading and against what cubes would fetch', () => {
		const shirt = estimateItem('Skeleton Shirt White (F)', 1, 10, context);
		expect(shirt.markupPed).toBeCloseTo(15);
		expect(shirt.cubeMarkupPed).toBeCloseTo(0.2);
		expect(shirt.sellsAsCubes).toBe(false);
	});

	it('says an item with no reading is unvalued rather than floor it to cubes', () => {
		const shirt = estimateItem('Skeleton Shirt White (M)', 1, 10, context);
		expect(shirt.markupPed).toBeNull();
		expect(shirt.sellsAsCubes).toBe(false);
	});

	it('cubes by default only what its own reading puts at or below cubes', () => {
		expect(contentLine('Red Heart Fireworks', 1, 2, context).recycle).toBe(true);
		expect(contentLine('Skeleton Shirt White (F)', 1, 10, context).recycle).toBe(false);
		expect(contentLine('Skeleton Shirt White (M)', 1, 10, context).recycle).toBe(false);
	});

	it('puts the hits first', () => {
		const lines = orderLines([
			contentLine('Red Heart Fireworks', 1, 2, context),
			contentLine('Skeleton Shirt White (M)', 1, 10, context),
			contentLine('Skeleton Shirt White (F)', 1, 10, context),
		]);
		expect(lines.map((line) => line.itemName)).toEqual([
			'Skeleton Shirt White (F)',
			'Red Heart Fireworks',
			'Skeleton Shirt White (M)',
		]);
		expect(isHit(lines[0])).toBe(true);
		expect(isHit(lines[1])).toBe(false);
	});
});

describe('a result from the export', () => {
	it('reads the Halloween export as a clean run of eight', () => {
		const result = resultFromExport(halloween, 'Halloween Strongbox', context);
		expect(result.boxes).toBe(8);
		expect(result.keyUnitTt).toBe(10);
		expect(result.notes).toEqual([]);
		const summary = summarise(result);
		expect(summary.keysTt).toBe(80);
		expect(summary.cubedTt).toBe(2);
		expect(summary.nanocubes).toBe(200);
		expect(summary.keptTt).toBe(20);
		expect(summary.gap).toBeCloseTo(0);
		expect(summary.hits).toBe(1);
		expect(summary.unvalued).toBe(1);
		expect(summary.estimatedMarkup).toBeCloseTo(15 + 0.04);
		expect(balanceRefusal(result)).toBeNull();
	});

	it('refuses an export with none of the chosen box opened, saying why', () => {
		const easter: StrongboxExportRead = {
			...halloween,
			boxesOpened: 0,
			unexpected: [
				{
					row: { line: 5, itemName: 'Easter Strongbox', quantity: -8, valuePed: 0 },
					reason: 'other_box',
				},
			],
		};
		expect(exportRefusal(easter, 'Halloween Strongbox')).toContain('Easter Strongbox');
		expect(exportRefusal(halloween, 'Halloween Strongbox')).toBeNull();
		const nothing: StrongboxExportRead = {
			...halloween,
			boxesOpened: 0,
			contents: [],
			ammoPed: 0,
			keys: null,
			skipped: [{ line: 1, text: 'hello', reason: 'not a row' }],
		};
		expect(exportRefusal(nothing, 'Halloween Strongbox')).toContain('does not look like');
	});

	it('notes keys that do not match the boxes, and a missing key line', () => {
		const odd = resultFromExport(
			{ ...halloween, keys: { count: 9, ttValue: 90 } },
			'Halloween Strongbox',
			context,
		);
		expect(odd.notes[0]).toContain('9 keys');
		const none = resultFromExport({ ...halloween, keys: null }, 'Halloween Strongbox', context);
		expect(none.keyUnitTt).toBe(10);
		expect(none.notes).toHaveLength(1);
	});

	it('refuses to record a result the keys do not balance', () => {
		const short = resultFromExport({ ...halloween, ammoPed: 40 }, 'Halloween Strongbox', context);
		expect(balanceRefusal(short)).toContain('80.00');
	});
});

describe('a result entered by hand', () => {
	const entry: ManualEntry = {
		boxes: 8,
		keyUnitTt: 10,
		lines: [
			{ id: 1, itemName: 'Nanocube', quantity: 200, ttValue: 2 },
			{ id: 2, itemName: 'Skeleton Shirt White (F)', quantity: 1, ttValue: 10 },
			{ id: 3, itemName: '', quantity: null, ttValue: null },
		],
	};

	it('takes the ammo as whatever the keys did not pay back in items', () => {
		expect(manualAmmo(entry)).toBeCloseTo(68);
		expect(manualRefusal(entry)).toBeNull();
		const result = resultFromManual(entry, 'Halloween Strongbox', context);
		expect(result.lines).toHaveLength(2);
		expect(result.ammoPed).toBeCloseTo(68);
		expect(balanceRefusal(result)).toBeNull();
	});

	it('refuses items worth more than the keys, half-filled lines, and typed ammo', () => {
		expect(
			manualRefusal({ ...entry, lines: [{ id: 1, itemName: 'Shirt', quantity: 1, ttValue: 90 }] }),
		).toContain('more than');
		expect(
			manualRefusal({
				...entry,
				lines: [{ id: 1, itemName: 'Shirt', quantity: null, ttValue: 1 }],
			}),
		).toContain('name and a quantity');
		expect(
			manualRefusal({
				...entry,
				lines: [{ id: 1, itemName: 'Universal Ammo', quantity: 5, ttValue: 5 }],
			}),
		).toContain('Universal Ammo');
		expect(manualRefusal({ ...entry, boxes: 2.5 })).toContain('how many');
	});
});

describe('where the boxes came from', () => {
	const holding = boxHoldings(
		[stock('Halloween Strongbox', 5)],
		[asset('a2', 'Halloween Strongbox', 4, 20)],
	)[0];

	it('draws loot first, then assets, and the rest as boxes from before tracking', () => {
		const plan = defaultPlan(12, holding);
		expect(plan.loot).toBe(8);
		expect(plan.assets[0].quantity).toBe(4);
		expect(untrackedBoxes(plan, holding)).toBe(3);
		expect(boxCost(plan)).toBeCloseTo(20);
		expect(planRefusal(plan, 12)).toBeNull();
	});

	it('asks what bought boxes cost, per box or in total', () => {
		const plan = { ...defaultPlan(8, null), loot: 0, bought: 8 };
		expect(planRefusal(plan, 8)).toContain('cost above TT');
		const perBox = { ...plan, boughtMarkup: 1.5, boughtMarkupPer: 'box' as const };
		expect(boxCost(perBox)).toBeCloseTo(12);
		expect(planRefusal(perBox, 8)).toBeNull();
		expect(planRefusal({ ...perBox, bought: 7 }, 8)).toContain('7 boxes');
	});

	it('builds the request the backend records', () => {
		const result = resultFromExport(halloween, 'Halloween Strongbox', context);
		const plan = { ...defaultPlan(8, holding), bought: 0 };
		const input = openingInput(result, plan, '2026-11-15');
		expect(input.sources).toEqual([
			{ kind: 'held', quantity: 5 },
			{ kind: 'asset', itemId: 'a2', quantity: 3 },
		]);
		expect(input.keyUnitTt).toBe(10);
		expect(input.ammoPed).toBe(58);
		expect(input.intake).toBe('export');
		expect(input.contents.find((row) => row.itemName === 'Red Heart Fireworks')?.recycle).toBe(
			true,
		);
	});
});
