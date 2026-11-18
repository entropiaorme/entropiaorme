import { describe, expect, it } from 'vitest';
import type { StockReconciliationLine } from '$lib/api/commands.gen';
import {
	applySummary,
	asSentence,
	bulkOutcome,
	choicesFor,
	conversionSentence,
	conversionTt,
	differenceTt,
	EXCLUSION_LABEL,
	groupFiguresOf,
	isLargeShortfall,
	remainderSentence,
	signedQuantity,
	splitSettled,
	standaloneChoice,
} from './reconciliation';

function line(
	itemName: string,
	appQuantity: number,
	observedQuantity: number,
	overrides: Partial<StockReconciliationLine> = {},
): StockReconciliationLine {
	const difference = observedQuantity - appQuantity;
	return {
		itemName,
		appQuantity,
		assetQuantity: 0,
		observedQuantity,
		difference,
		unitTt: { kind: 'exact', ped: 0.05 },
		disposition: difference < 0 ? { kind: 'removed' } : { kind: 'unknownOrigin' },
		takenBack: false,
		acceptedAt: null,
		...overrides,
	};
}

function draft(lines: StockReconciliationLine[]) {
	return {
		id: 'd1',
		source: { kind: 'paste' as const },
		observedAt: '2026-11-17T10:00:00Z',
		lines,
		excluded: [],
		groups: [],
		caution: null,
		previouslyAccepted: [],
	};
}

describe('what a line can be set to', () => {
	it('offers removal for a shortfall, conversion for Shrapnel, origin for a surplus', () => {
		expect(choicesFor(line('Lysterium Ingot', 10, 4)).map((c) => c.id)).toEqual([
			'removed',
			'leftAsIs',
		]);
		const shrapnel = line('Shrapnel', 1000, 400, {
			disposition: { kind: 'shrapnelConverted', soldAtTt: null },
		});
		expect(choicesFor(shrapnel).map((c) => c.id)).toEqual([
			'shrapnelConverted',
			'removed',
			'leftAsIs',
		]);
		expect(choicesFor(line('Iron Ingot', 1, 5)).map((c) => c.id)).toEqual([
			'unknownOrigin',
			'leftAsIs',
		]);
		// Extra Shrapnel is a surplus like any other.
		expect(choicesFor(line('Shrapnel', 100, 500)).map((c) => c.id)).toEqual([
			'unknownOrigin',
			'leftAsIs',
		]);
	});

	it("offers a surplus of inexact TT as of unknown origin at the account's TT", () => {
		const truncated = line('Iron Ingot', 1, 5, {
			unitTt: { kind: 'truncated', ped: 0.13 },
			disposition: { kind: 'leftAsIs' },
		});
		const choices = choicesFor(truncated);
		expect(choices.map((c) => c.id)).toEqual(['unknownOrigin', 'leftAsIs']);
		expect(choices[0].title).toMatch(/your account's TT/);
		expect(standaloneChoice(truncated)).toBe('unknownOrigin');
	});

	it('offers a surplus of no known TT only to be left as it is', () => {
		const unknown = line('Iron Ingot', 1, 5, { unitTt: { kind: 'unknown' } });
		expect(choicesFor(unknown).map((c) => c.id)).toEqual(['leftAsIs']);
		expect(standaloneChoice(unknown)).toBe('leftAsIs');
	});

	it('says what a bulk choice was set on and why the rest did not fit', () => {
		expect(bulkOutcome(3, [])).toBeNull();
		const iron = {
			itemName: 'Iron Ingot',
			reason: 'Iron Ingot is not missing, so nothing is removed',
		};
		expect(bulkOutcome(3, [iron])).toBe(
			'Set on 2; 1 did not fit: Iron Ingot is not missing, so nothing is removed.',
		);
		const many = ['A', 'B', 'C', 'D', 'E'].map((name) => ({
			itemName: name,
			reason: `${name} is not missing.`,
		}));
		expect(bulkOutcome(6, many)).toBe(
			'Set on 1; 5 did not fit: A is not missing; B is not missing; C is not missing; and 2 more.',
		);
	});

	it('sends a line out of a conversion back to what it starts as', () => {
		expect(standaloneChoice(line('Lysterium Ingot', 10, 4))).toBe('removed');
		expect(standaloneChoice(line('Shrapnel', 10, 4))).toBe('shrapnelConverted');
		expect(standaloneChoice(line('Nanocube', 0, 4))).toBe('unknownOrigin');
		expect(standaloneChoice(line('Oil', 9, 3, { takenBack: true }))).toBe('leftAsIs');
	});
});

describe('what a difference is worth', () => {
	it('is exact from a known TT per unit, a floor from the account values, absent otherwise', () => {
		expect(differenceTt(line('Oil', 300, 100))).toEqual({ ped: 10, floor: false });
		const truncated = line('Oil', 300, 100, { unitTt: { kind: 'truncated', ped: 0.05 } });
		expect(differenceTt(truncated)).toEqual({ ped: 10, floor: true });
		expect(differenceTt(line('Oil', 300, 100, { unitTt: { kind: 'unknown' } }))).toBeNull();
	});

	it('marks a large shortfall, never a surplus or one of unknown worth', () => {
		expect(isLargeShortfall(line('Oil', 300, 100))).toBe(true);
		expect(isLargeShortfall(line('Oil', 300, 120))).toBe(false);
		expect(isLargeShortfall(line('Oil', 100, 300))).toBe(false);
		expect(isLargeShortfall(line('Oil', 300, 100, { unitTt: { kind: 'unknown' } }))).toBe(false);
	});

	it('signs the difference', () => {
		expect(signedQuantity(12)).toBe('+12');
		expect(signedQuantity(-300)).toBe('−300');
	});
});

describe('what applying will do', () => {
	it('counts each kind of change', () => {
		const lines = [
			line('A', 10, 4),
			line('B', 10, 4),
			line('Shrapnel', 1000, 400, { disposition: { kind: 'shrapnelConverted', soldAtTt: null } }),
			line('C', 10, 4, { disposition: { kind: 'conversionGroup', group: 1 } }),
			line('Nanocube', 0, 4, { disposition: { kind: 'conversionGroup', group: 1 } }),
			line('D', 1, 5),
			line('E', 1, 5),
			line('F', 1, 5, { disposition: { kind: 'leftAsIs' } }),
		];
		expect(applySummary(draft(lines))).toBe(
			[
				'2 items removed from stock',
				'Shrapnel converted to Universal Ammo',
				'1 conversion',
				'2 surpluses added',
				'1 left as it is',
			].join(', '),
		);
	});

	it('says how much Shrapnel was sold at TT', () => {
		const lines = [
			line('Shrapnel', 1000, 400, { disposition: { kind: 'shrapnelConverted', soldAtTt: 200 } }),
		];
		expect(applySummary(draft(lines))).toBe('Shrapnel converted, 200 of it sold at TT');
	});

	it('says when all the missing Shrapnel was sold at TT', () => {
		const lines = [
			line('Shrapnel', 1000, 400, { disposition: { kind: 'shrapnelConverted', soldAtTt: 600 } }),
		];
		expect(applySummary(draft(lines))).toBe('Shrapnel sold at TT');
	});

	it('splits what was settled into what was recorded and what was left as it is', () => {
		const lines = [
			line('A', 10, 4),
			line('B', 10, 4, { disposition: { kind: 'leftAsIs' } }),
			line('C', 1, 5),
		];
		expect(splitSettled(['A', 'B', 'C'], draft(lines))).toEqual({
			changed: ['A', 'C'],
			leftAsIs: ['B'],
		});
		expect(splitSettled(['A'], null)).toEqual({ changed: ['A'], leftAsIs: [] });
	});

	it('says when nothing changes', () => {
		expect(applySummary(draft([line('A', 10, 4, { disposition: { kind: 'leftAsIs' } })]))).toBe(
			'Nothing changes: the item is left as it is',
		);
	});

	it('reads a conversion from either side', () => {
		const lines = [
			line('Oil', 10, 4, { disposition: { kind: 'conversionGroup', group: 1 } }),
			line('Ore', 10, 4, { disposition: { kind: 'conversionGroup', group: 1 } }),
			line('Nanocube', 0, 4, { disposition: { kind: 'conversionGroup', group: 1 } }),
		];
		const d = draft(lines);
		expect(conversionSentence(d, lines[0])).toBe('Went into Nanocube');
		expect(conversionSentence(d, lines[2])).toBe('Made from 2 items');
	});

	it('gives a conversion its TT in and out, and the extra that arrived otherwise', () => {
		const lines = [
			line('Oil', 10, 4, { disposition: { kind: 'conversionGroup', group: 2 } }),
			line('Nanocube', 0, 40, { disposition: { kind: 'conversionGroup', group: 2 } }),
			line('Ore', 10, 4),
		];
		const figures = {
			group: 2,
			ttIn: 0.3,
			ttOut: 0.3,
			outputItem: 'Nanocube',
			outputQuantity: 30,
			remainderQuantity: 10,
		};
		const d = { ...draft(lines), groups: [figures] };
		expect(groupFiguresOf(d, lines[1])).toBe(figures);
		expect(groupFiguresOf(d, lines[2])).toBeNull();
		expect(conversionTt(figures)).toBe('0.30 PED of TT in, 0.30 PED out');
		expect(remainderSentence(figures)).toBe(
			'10 more Nanocube arrived some other way and is kept as stock of unknown origin.',
		);
		expect(remainderSentence({ ...figures, remainderQuantity: 0 })).toBeNull();
	});
});

describe('why an item is left out', () => {
	it('labels the reasons plainly', () => {
		expect(EXCLUSION_LABEL.ammo).toBe('Ammunition, not stock');
		expect(EXCLUSION_LABEL.auctionWithoutListing).toBe(
			'More listed at auction than the app has listings for',
		);
		expect(EXCLUSION_LABEL.heldAsAsset).toMatch(/asset/);
	});
});

describe('a reason read as a sentence', () => {
	it('capitalises and ends it', () => {
		expect(asSentence('line 3 has no name')).toBe('Line 3 has no name.');
		expect(asSentence('Already ended.')).toBe('Already ended.');
	});
});
