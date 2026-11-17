import { describe, expect, it } from 'vitest';
import type { ProtectionSetPieces } from '$lib/api';
import {
	initialSelection,
	pieceChoices,
	piecesSummary,
	piecesView,
	selectionChanged,
} from './setPieces';

function pieces(overrides: Partial<ProtectionSetPieces> = {}): ProtectionSetPieces {
	return {
		setId: '1',
		confirmed: [],
		suggested: [],
		others: [],
		readingTakenAt: 1,
		...overrides,
	};
}

describe("naming a set's pieces", () => {
	it('shows the confirmed pieces, else a choice, else nothing to choose from', () => {
		expect(piecesView(pieces({ confirmed: [{ name: 'a', confirmedAt: 1 }] }))).toBe('confirmed');
		expect(piecesView(pieces({ others: ['b'] }))).toBe('choosing');
		expect(piecesView(pieces())).toBe('empty');
	});

	it('offers each name once, confirmed and suggested ahead of the rest', () => {
		const choices = pieceChoices(
			pieces({
				confirmed: [{ name: 'a', confirmedAt: 1 }],
				suggested: ['b', 'a'],
				others: ['c', 'b'],
			}),
		);
		expect(choices).toEqual([
			{ name: 'a', suggested: true },
			{ name: 'b', suggested: true },
			{ name: 'c', suggested: false },
		]);
	});

	it('starts from the confirmed pieces, or else the suggestions', () => {
		expect([...initialSelection(pieces({ suggested: ['b'], others: ['c'] }))]).toEqual(['b']);
		expect([
			...initialSelection(pieces({ confirmed: [{ name: 'a', confirmedAt: 1 }], suggested: ['b'] })),
		]).toEqual(['a']);
	});

	it('changes the pieces only when the ticked names differ from the confirmed ones', () => {
		const confirmed = pieces({ confirmed: [{ name: 'a', confirmedAt: 1 }] });
		expect(selectionChanged(confirmed, new Set(['a']))).toBe(false);
		expect(selectionChanged(confirmed, new Set(['b']))).toBe(true);
		expect(selectionChanged(confirmed, new Set(['a', 'b']))).toBe(true);
		expect(selectionChanged(confirmed, new Set())).toBe(true);
	});

	it('sums a set up in a few words', () => {
		expect(piecesSummary(0)).toBe('Pieces not confirmed');
		expect(piecesSummary(1)).toBe('1 piece');
		expect(piecesSummary(6)).toBe('6 pieces');
	});
});
