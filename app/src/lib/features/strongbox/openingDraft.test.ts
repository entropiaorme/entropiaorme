import { describe, expect, it } from 'vitest';
import type {
	InventoryItem,
	MarketHarvestData,
	MarketHarvestItem,
	StockPosition,
	StrongboxExportRead,
} from '$lib/api/commands.gen';
import { five, seven, unsettled } from './__fixtures__/proposals';
import {
	ammoNote,
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
	paidAloneSentence,
	planRefusal,
	proposalRefusal,
	resultFromExport,
	resultFromManual,
	resultFromProposals,
	summarise,
	untrackedBoxes,
	withPossible,
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

	it('keeps every content: recycling is done in game, and the readings record it', () => {
		expect(contentLine('Red Heart Fireworks', 1, 2, context).recycle).toBe(false);
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
		// Every content is kept, the fireworks too.
		expect(summary.cubedTt).toBe(0);
		expect(summary.nanocubes).toBe(0);
		expect(summary.keptTt).toBe(22);
		expect(summary.gap).toBeCloseTo(0);
		expect(summary.hits).toBe(1);
		expect(summary.unvalued).toBe(1);
		const fireworks = estimateItem('Red Heart Fireworks', 1, 2, context).markupPed ?? 0;
		expect(summary.estimatedMarkup).toBeCloseTo(15 + fireworks);
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
		expect(input.contents.every((row) => !row.recycle)).toBe(true);
	});
});

describe('a result read from the account', () => {
	it('fills the review as an export would: the boxes, keys, ammo and items', () => {
		const result = resultFromProposals([seven()], 'Halloween Strongbox', context);
		expect(result.boxes).toBe(7);
		expect(result.keyUnitTt).toBe(10);
		expect(result.ammoPed).toBe(60);
		expect(result.lines.map((line) => [line.itemName, line.quantity, line.ttValue])).toEqual([
			['Skeleton Pants Red (F)', 1, 10],
		]);
		expect(result.intake).toBe('export');
		expect(result.ammoFromKeys).toBe(true);
		expect(result.notes).toEqual([]);
		expect(balanceRefusal(result)).toBeNull();
		expect(paidAloneSentence(result)).toBe('6 of 7 paid ammo alone');
		const input = openingInput(result, defaultPlan(7, null), '2026-11-14');
		expect(input.intake).toBe('export');
		expect(input.ammoPed).toBe(60);
		expect(input.contents).toEqual([
			{ itemName: 'Skeleton Pants Red (F)', quantity: 1, ttValue: 10, recycle: false },
		]);
	});

	it("keeps an unsettled opening's doubt in the notes and what may be its own apart", () => {
		const result = resultFromProposals([unsettled()], 'Halloween Strongbox', context);
		expect(result.lines).toEqual([]);
		expect(result.possible.map((line) => [line.itemName, line.included])).toEqual([
			['Skeleton Pants Red (F)', false],
			['Mayhem Token', false],
		]);
		expect(result.notes[0]).toBe('6 keys were used for 7 boxes.');
		expect(result.notes[1]).toMatch(/2 items arrived at the same time/);
		// The ammo is what the keys did not pay back in items, and moves as
		// the player adds one.
		expect(result.ammoFromKeys).toBe(true);
		expect(result.ammoPed).toBeCloseTo(70);
		expect(paidAloneSentence(result)).toBeNull();
		const added = withPossible(result, 'Skeleton Pants Red (F)', true);
		expect(added.ammoPed).toBeCloseTo(60);
		expect(summarise(added).keptTt).toBe(10);
		const input = openingInput(added, defaultPlan(7, null), null);
		expect(input.contents.map((row) => row.itemName)).toEqual(['Skeleton Pants Red (F)']);
		expect(withPossible(added, 'Skeleton Pants Red (F)', false).ammoPed).toBeCloseTo(70);
	});

	it('takes the ammo as the keys less what else came out, noting a read figure that differs', () => {
		const result = resultFromProposals([five()], 'Halloween Strongbox', context);
		expect(result.ammoPed).toBe(50);
		expect(ammoNote(result)).toBe(
			'Your account shows 49.95 PED of ammo; the keys less what else came out are 50.00 PED, and that is the figure recorded.',
		);
		expect(balanceRefusal(result)).toBeNull();
		expect(result.possible.map((line) => [line.itemName, line.ttValue, line.included])).toEqual([
			['Halloween Fireworks', 0, false],
		]);
		expect(paidAloneSentence(result)).toBe('4 of 5 paid ammo alone');
		const input = openingInput(result, defaultPlan(5, null), null);
		expect(input.ammoPed).toBe(50);
		expect(input.contents).toEqual([]);
		// Added, the fireworks change nothing: they have no TT.
		expect(withPossible(result, 'Halloween Fireworks', true).ammoPed).toBe(50);
	});

	it('says what the readings showed of the ammo as the figures stand', () => {
		// The rows read 60.00 PED beside trousers that may be the boxes'.
		const proposal = seven({
			contents: [],
			possibleContents: [
				{
					item: { name: 'Skeleton Pants Red (F)', place: { kind: 'carried' } },
					units: 1,
					amount: { ped: 10, precision: 'wholePec' },
				},
			],
		});
		const left = resultFromProposals([proposal], 'Halloween Strongbox', context);
		expect(left.ammoPed).toBe(70);
		expect(ammoNote(left)).toMatch(/shows 60\.00 PED of ammo; .* are 70\.00 PED/);
		const added = withPossible(left, 'Skeleton Pants Red (F)', true);
		expect(added.ammoPed).toBe(60);
		expect(ammoNote(added)).toBeNull();
		expect(ammoNote(withPossible(added, 'Skeleton Pants Red (F)', false))).toBe(ammoNote(left));
	});

	it('adds up a run read across two minutes into one opening', () => {
		const first = seven({
			boxes: [{ name: 'Halloween Strongbox', count: 3 }],
			keysUsed: 3,
			keys: { ped: 30, precision: 'wholePec' },
			ammo: { units: 300_000, amount: { ped: 30, precision: 'exact' } },
			contents: [],
			paidAmmoAlone: 3,
		});
		const second = seven({
			boxes: [{ name: 'Halloween Strongbox', count: 4 }],
			keysUsed: 4,
			keys: { ped: 40, precision: 'wholePec' },
			ammo: { units: 300_000, amount: { ped: 30, precision: 'exact' } },
			paidAmmoAlone: 3,
		});
		const result = resultFromProposals([first, second], 'Halloween Strongbox', context);
		expect(result.boxes).toBe(7);
		expect(result.ammoPed).toBe(60);
		expect(result.lines).toHaveLength(1);
		expect(paidAloneSentence(result)).toBe('6 of 7 paid ammo alone');
	});

	it('refuses an opening of more than one kind of box, or of another box', () => {
		const two = seven({
			boxes: [
				{ name: 'Halloween Strongbox', count: 3 },
				{ name: 'Easter Strongbox', count: 4 },
			],
		});
		expect(proposalRefusal([two], 'Halloween Strongbox')).toMatch(/more than one kind of box/);
		expect(proposalRefusal([seven()], 'Easter Strongbox')).toBe(
			'Your account shows Halloween Strongbox opened, not Easter Strongbox. Choose that box instead.',
		);
		expect(proposalRefusal([seven()], 'halloween strongbox')).toBeNull();
	});

	it('says nothing of how the boxes paid unless the readings show it', () => {
		const result = resultFromProposals(
			[seven({ paidAmmoAlone: null })],
			'Halloween Strongbox',
			context,
		);
		expect(paidAloneSentence(result)).toBeNull();
		const all = resultFromProposals(
			[
				seven({
					paidAmmoAlone: 7,
					contents: [],
					ammo: { units: 700_000, amount: { ped: 70, precision: 'exact' } },
				}),
			],
			'Halloween Strongbox',
			context,
		);
		expect(paidAloneSentence(all)).toBe('All 7 paid ammo alone');
	});
});
