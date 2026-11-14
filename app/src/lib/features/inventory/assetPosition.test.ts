import { describe, expect, it } from 'vitest';
import type { InventoryItem } from '$lib/api/commands.gen';
import {
	averageAfter,
	currentAverage,
	figuresEditable,
	figuresLockedReason,
	formatQuantity,
	isStack,
	markupPct,
	shareOf,
} from './assetPosition';

const purchase = (id: number, quantity: number, markupPaid: number, editable = true) => ({
	id,
	quantity,
	ttValue: quantity * 0.1,
	markupPaid,
	acquiredAt: '2026-08-01',
	fromConversion: false,
	editable,
});

const strongboxes: InventoryItem = {
	id: 'strongboxes',
	name: 'Strongbox',
	ttValue: 10,
	markupPaid: 1000,
	notes: null,
	acquiredAt: '2026-08-01',
	quantity: 100,
	divisible: true,
	purchases: [purchase(1, 100, 1000)],
	origins: [],
	derivedFrom: [],
};

describe('an asset position', () => {
	it('averages a second purchase into the first', () => {
		expect(currentAverage(strongboxes).markupPerUnit).toBe(10);
		const after = averageAfter(strongboxes, { quantity: 100, ttValue: 10, markupPaid: 800 });
		expect(after.quantity).toBe(200);
		expect(after.markupPerUnit).toBe(9);
		expect(after.costPerUnit).toBeCloseTo(9.1, 9);
	});

	it('corrects a purchase by its difference rather than adding it again', () => {
		const bought: InventoryItem = {
			...strongboxes,
			ttValue: 20,
			markupPaid: 1900,
			quantity: 200,
			purchases: [purchase(1, 100, 1000), purchase(2, 100, 900)],
		};
		const corrected = averageAfter(
			bought,
			{ quantity: 100, ttValue: 10, markupPaid: 800 },
			bought.purchases[1],
		);
		expect(corrected.quantity).toBe(200);
		expect(corrected.markupPerUnit).toBe(9);
	});

	it('lets part of it leave at the average cost', () => {
		expect(shareOf(strongboxes, 25)).toEqual({ ttValue: 2.5, markupPaid: 250 });
		expect(shareOf(strongboxes, 500)).toEqual({ ttValue: 10, markupPaid: 1000 });
	});

	it('changes its own figures only while it is one open purchase', () => {
		expect(figuresEditable(strongboxes)).toBe(true);
		expect(figuresLockedReason(strongboxes)).toBeNull();
		const twice = { ...strongboxes, purchases: [purchase(1, 50, 500), purchase(2, 50, 500)] };
		expect(figuresLockedReason(twice)).toMatch(/purchases/);
		const drawnOn = { ...strongboxes, purchases: [purchase(1, 100, 1000, false)] };
		expect(figuresLockedReason(drawnOn)).toMatch(/fixed/);
	});

	it('reads quantities and markups the way the game writes them', () => {
		expect(formatQuantity(200)).toBe('200');
		expect(formatQuantity(2.5)).toBe('2.50');
		expect(isStack(strongboxes)).toBe(true);
		expect(isStack({ quantity: 1 })).toBe(false);
		expect(markupPct(10, 1000)).toBe(10100);
		expect(markupPct(0, 5)).toBeNull();
	});
});
