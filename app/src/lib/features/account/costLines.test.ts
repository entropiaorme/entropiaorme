import { describe, expect, it } from 'vitest';
import type {
	CostBreakdown,
	CostPlacementCategory,
	MeasuredArmourPart,
	MeasuredCostDetail,
	MeasuredCostLine,
} from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { exact } from './__fixtures__/measure';
import {
	armourPartLine,
	awaitingSummary,
	costsListWholePec,
	cycledGroups,
	hasOverhead,
	isCurrentChoice,
	lineFigure,
	lineQuantity,
	overheadCategories,
	overheadWithheld,
	PLACE_CHOICES,
	PLACEMENT_LABELS,
	peggedLedgerNote,
	UNPRICED,
} from './costLines';

const NO_OVERHEAD_REASONS = { travel: null, fieldLogistics: null, unclassified: null };

function line(overrides: Partial<MeasuredCostLine> = {}): MeasuredCostLine {
	return {
		kind: 'decay',
		item: 'Item',
		units: null,
		amount: 0.1,
		precision: 'exact',
		markupPercent: null,
		markupNotIncluded: false,
		unpriced: null,
		peggedLedgerEntryId: null,
		peggedLedgerEntryDescription: null,
		...overrides,
	};
}

function breakdown(overrides: Partial<CostBreakdown> = {}): CostBreakdown {
	const none = {
		weaponCost: 0,
		healCost: 0,
		consumableCost: 0,
		enhancerCost: 0,
		armourCost: 0,
		harvestCost: 0,
	};
	return {
		...none,
		tracked: { ...none },
		basis: {
			weaponCost: 'measured',
			healCost: 'measured',
			consumableCost: 'measured',
			enhancerCost: 'measured',
			armourCost: 'measured',
			harvestCost: 'measured',
		},
		reasons: {
			weaponCost: null,
			healCost: null,
			consumableCost: null,
			enhancerCost: null,
			armourCost: null,
			harvestCost: null,
		},
		overheadCost: 0,
		overhead: { travel: 0, fieldLogistics: 0, unclassified: 0, reasons: NO_OVERHEAD_REASONS },
		measured: null,
		armourUnrecorded: false,
		...overrides,
	};
}

describe("a session's overhead", () => {
	const measured: MeasuredCostDetail = {
		buckets: [
			{ bucket: 'weapon', lines: [line({ item: 'Rifle', amount: 2 })] },
			{
				bucket: 'travel',
				lines: [line({ kind: 'consumed', item: 'Mind Essence', units: 40, amount: 0.4 })],
			},
			{ bucket: 'unclassified', lines: [line({ item: 'Odd Thing', amount: 0.05 })] },
		],
		armourParts: [],
	};

	it('lists only the categories holding something, in a fixed order, with their lines', () => {
		const costs = breakdown({
			overheadCost: 0.45,
			overhead: {
				travel: 0.4,
				fieldLogistics: 0,
				unclassified: 0.05,
				reasons: NO_OVERHEAD_REASONS,
			},
			measured,
		});
		const categories = overheadCategories(costs);
		expect(categories.map((c) => [c.bucket, c.label, c.amount])).toEqual([
			['travel', 'Travel', 0.4],
			['unclassified', 'Unclassified', 0.05],
		]);
		expect(categories[0].lines.map((l) => l.item)).toEqual(['Mind Essence']);
		expect(hasOverhead(costs)).toBe(true);
	});

	it('keeps a category withheld with a reason, and says what stands in place of its figure', () => {
		const costs = breakdown({
			overheadCost: 0,
			overhead: {
				travel: 0,
				fieldLogistics: 0,
				unclassified: 0,
				reasons: { travel: 'overheadInGap', fieldLogistics: null, unclassified: null },
			},
			measured: {
				buckets: [
					{
						bucket: 'unclassified',
						lines: [line({ item: 'Strange Stack', amount: null, unpriced: 'notPlaced' })],
					},
				],
				armourParts: [],
			},
		});
		const categories = overheadCategories(costs);
		expect(categories.map((c) => [c.bucket, c.reason])).toEqual([
			['travel', 'overheadInGap'],
			['unclassified', null],
		]);
		expect(categories.map(overheadWithheld)).toEqual(['reason', 'toPlace']);
		expect(hasOverhead(costs)).toBe(true);
	});

	it('quotes the ledger entry a line may repeat', () => {
		expect(peggedLedgerNote('Fuel for the Valkyrie')).toBe(
			'You also recorded this in the ledger: Fuel for the Valkyrie',
		);
		expect(peggedLedgerNote(null)).toBe(
			'You also recorded this in the ledger, pegged to this session',
		);
	});

	it('is absent from a session without any', () => {
		expect(overheadCategories(breakdown())).toEqual([]);
		expect(hasOverhead(breakdown())).toBe(false);
	});

	it("keeps the overhead's lines out of Cycled's item-by-item groups", () => {
		expect(cycledGroups(measured).map((group) => group.bucket)).toEqual(['weapon']);
		expect(cycledGroups(null)).toEqual([]);
	});
});

describe('a measured line in words', () => {
	it('says the units used or burned, or the TT lost', () => {
		expect(lineQuantity(line({ kind: 'ammoBurned', units: 95000 }))).toBe('95000 units burned');
		expect(lineQuantity(line({ kind: 'consumed', units: 1 }))).toBe('1 unit used');
		expect(lineQuantity(line({ kind: 'decay' }))).toBe('TT lost');
	});

	it('names the markup a line was priced at', () => {
		expect(lineQuantity(line({ kind: 'consumed', units: 3, markupPercent: 120 }))).toBe(
			'3 units used, at 120.0% markup',
		);
		expect(lineQuantity(line({ kind: 'limitedDecay', markupPercent: 150 }))).toBe(
			"TT lost, priced at its set's 150.0% markup",
		);
	});

	it('shows a figure at the precision it is known to, or why it has none', () => {
		expect(lineFigure(line({ amount: 0.1234, precision: 'exact' })).text).toBe('0.1234');
		expect(lineFigure(line({ amount: 0.12, precision: 'wholePec' })).text).toBe('0.12');
		expect(lineFigure(line({ amount: null, unpriced: 'boughtThroughLedger' }))).toEqual({
			text: UNPRICED.boughtThroughLedger,
			title: null,
		});
		expect(lineFigure(line({ amount: null, unpriced: 'notPlaced' })).text).toBe(UNPRICED.notPlaced);
		expect(lineFigure(line({ amount: null })).text).toBe(NO_DATA);
	});

	it('says when any figure rests on values the site rounds down to the PEC', () => {
		const exactOnly: MeasuredCostDetail = {
			buckets: [{ bucket: 'weapon', lines: [line()] }],
			armourParts: [],
		};
		expect(costsListWholePec(exactOnly)).toBe(false);
		expect(costsListWholePec(null)).toBe(false);
		expect(
			costsListWholePec({
				...exactOnly,
				buckets: [{ bucket: 'weapon', lines: [line({ precision: 'wholePec' })] }],
			}),
		).toBe(true);
		// Overhead lines are listed with the overhead, not here.
		expect(
			costsListWholePec({
				...exactOnly,
				buckets: [{ bucket: 'travel', lines: [line({ precision: 'wholePec' })] }],
			}),
		).toBe(false);
	});
});

describe("the session's armour in parts", () => {
	function part(overrides: Partial<MeasuredArmourPart> = {}): MeasuredArmourPart {
		return {
			part: 'unlimited',
			setId: null,
			setName: null,
			markupPercent: null,
			lost: 0.5,
			lostPrecision: 'exact',
			cost: 0.5,
			costPrecision: 'exact',
			...overrides,
		};
	}

	it('names each part, its TT lost and the markup a set is priced at', () => {
		expect(armourPartLine(part())).toEqual({
			label: 'Unlimited armour and plates',
			detail: '0.5000 TT lost',
			cost: { text: '0.5000', title: 'Exact' },
		});
		const set = armourPartLine(
			part({ part: 'limitedSet', setName: 'Ghost (L)', markupPercent: 140, cost: 0.7 }),
		);
		expect(set.label).toBe('Ghost (L)');
		expect(set.detail).toBe('0.5000 TT lost, at 140.0% markup');
		expect(set.cost.text).toBe('0.7000');
	});

	it('gives what no confirmed set holds no price, and says why', () => {
		const unconfirmed = armourPartLine(
			part({ part: 'unconfirmed', cost: null, costPrecision: null }),
		);
		expect(unconfirmed.label).toBe('Limited pieces in no confirmed set');
		expect(unconfirmed.cost).toEqual({ text: UNPRICED.limitedSetUnconfirmed, title: null });
	});

	it('names each limited piece, at its markup or at TT when none is set', () => {
		const priced = armourPartLine(
			part({ part: 'limitedPiece', item: 'Pegasus Helmet (M,L)', markupPercent: 140, cost: 0.7 }),
		);
		expect(priced.label).toBe('Pegasus Helmet (M,L)');
		expect(priced.detail).toBe('0.5000 TT lost, at 140.0% markup');
		expect(priced.cost.text).toBe('0.7000');
		const atTt = armourPartLine(part({ part: 'limitedPiece', item: 'Odd Gloves (L)' }));
		expect(atTt.detail).toBe('0.5000 TT lost, no markup set: billed at TT');
		expect(atTt.cost.text).toBe('0.5000');
	});
});

describe('placing an item', () => {
	it('offers the plain choices in order, each to a category an item may be placed in', () => {
		expect(PLACE_CHOICES.map((choice) => choice.label)).toEqual([
			'Weapon',
			'Harvesting',
			'Healing',
			'Armour',
			'Limited armour',
			'Consumable',
			'Travel',
			'Field logistics',
			'Not a cost',
		]);
		for (const choice of PLACE_CHOICES) {
			expect(choice.covers).toContain(choice.category);
		}
		expect(PLACE_CHOICES.find((choice) => choice.label === 'Harvesting')?.group).toBe('cycled');
		expect(PLACE_CHOICES.find((choice) => choice.label === 'Harvesting')?.category).toBe(
			'harvestingTool',
		);
	});

	it('names every stored placement', () => {
		const categories = Object.keys(PLACEMENT_LABELS) as CostPlacementCategory[];
		expect(categories).toHaveLength(15);
		for (const category of categories) {
			expect(PLACEMENT_LABELS[category].length).toBeGreaterThan(0);
		}
	});

	it('says what an item awaiting a placement lost wearing, and what of it left', () => {
		expect(awaitingSummary({ item: 'A', sessions: 3, lost: exact(0.1234), units: null })).toBe(
			'3 sessions, 0.1234 TT lost',
		);
		expect(awaitingSummary({ item: 'B', sessions: 1, lost: null, units: 1 })).toBe(
			'1 session, 1 unit left',
		);
		expect(awaitingSummary({ item: 'C', sessions: 2, lost: exact(0.5), units: 40 })).toBe(
			'2 sessions, 0.5000 TT lost, 40 units left',
		);
	});

	it('reads a placement as the choice that covers it', () => {
		const healing = PLACE_CHOICES.find((choice) => choice.label === 'Healing');
		if (!healing) throw new Error('no Healing choice');
		expect(isCurrentChoice(healing, 'healingConsumable')).toBe(true);
		expect(isCurrentChoice(healing, 'travel')).toBe(false);
		expect(isCurrentChoice(healing, null)).toBe(false);
	});
});
