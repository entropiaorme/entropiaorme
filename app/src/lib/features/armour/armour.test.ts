import { describe, expect, it } from 'vitest';
import type { ArmourPiecesView } from '$lib/api/commands.gen';
import {
	candidatesFor,
	heldIn,
	markupText,
	noLongerHeld,
	type Piece,
	parseMarkup,
	pricingNote,
} from './armour';

function piece(overrides: Partial<Piece> = {}): Piece {
	return {
		item: 'Pegasus Helmet (M,L)',
		list: 'armour',
		held: true,
		ttPed: 10,
		markupPercent: null,
		markupSetAt: null,
		declared: false,
		replacedAt: null,
		...overrides,
	};
}

const view: ArmourPiecesView = {
	pieces: [
		piece(),
		piece({ item: 'Armor Plating Mk. 5B (L)', list: 'plates' }),
		piece({ item: 'Pegasus Gloves (M,L)', held: false, ttPed: null, markupPercent: 140 }),
	],
	candidates: [
		{ item: 'Odd Plate', hint: 'attached' },
		{ item: 'Frontier Harness (M)', hint: 'tagged' },
		{ item: 'Animal Hide', hint: 'other' },
		{ item: 'Ghost Pod', hint: 'awaitingPlacement' },
	],
	readAt: '2026-11-17T10:00:00Z',
};

describe('the two lists', () => {
	it('holds what the latest reading holds, in each list', () => {
		expect(heldIn(view, 'armour').map((p) => p.item)).toEqual(['Pegasus Helmet (M,L)']);
		expect(heldIn(view, 'plates').map((p) => p.item)).toEqual(['Armor Plating Mk. 5B (L)']);
		expect(noLongerHeld(view).map((p) => p.item)).toEqual(['Pegasus Gloves (M,L)']);
	});

	it('is empty before anything was read', () => {
		expect(heldIn(null, 'armour')).toEqual([]);
		expect(noLongerHeld(null)).toEqual([]);
		expect(candidatesFor(null, 'plates', '')).toEqual([]);
	});
});

describe('a markup', () => {
	it('reads without trailing zeros', () => {
		expect(markupText(120)).toBe('120%');
		expect(markupText(120.5)).toBe('120.5%');
		expect(markupText(120.65)).toBe('120.65%');
	});

	it('is typed as a number from 100, or left empty for TT', () => {
		expect(parseMarkup('')).toBeNull();
		expect(parseMarkup(' 140 ')).toBe(140);
		expect(parseMarkup('140%')).toBe(140);
		expect(parseMarkup('120,654')).toBe(120.65);
		expect(parseMarkup('99.9')).toBeUndefined();
		expect(parseMarkup('lots')).toBeUndefined();
	});

	it('says how the piece is billed', () => {
		expect(pricingNote(piece())).toBe('No markup set: billed at TT');
		expect(pricingNote(piece({ markupPercent: 140 }))).toBe('Billed at this markup');
		expect(pricingNote(piece({ markupPercent: 140, markupSetAt: '2026-11-15T12:00:00Z' }))).toMatch(
			/^Set Nov 1[45]; applies from then on$/,
		);
	});
});

describe('the items offered to declare a piece', () => {
	it('puts what wore unplaced first, then attached items for plates and tagged for armour', () => {
		expect(candidatesFor(view, 'plates', '').map((c) => c.item)).toEqual([
			'Ghost Pod',
			'Odd Plate',
			'Frontier Harness (M)',
			'Animal Hide',
		]);
		expect(candidatesFor(view, 'armour', '').map((c) => c.item)).toEqual([
			'Ghost Pod',
			'Frontier Harness (M)',
			'Odd Plate',
			'Animal Hide',
		]);
	});

	it('narrows by what is typed, ignoring case', () => {
		expect(candidatesFor(view, 'armour', ' HARNESS ').map((c) => c.item)).toEqual([
			'Frontier Harness (M)',
		]);
	});
});
