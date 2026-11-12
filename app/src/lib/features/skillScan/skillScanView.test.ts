import { describe, expect, it } from 'vitest';
import type { SkillScanHint, SkillScanReview, SkillScanStatus } from '$lib/api';
import {
	HINT_TEXT,
	needsConfirmation,
	pageList,
	REFUSAL_TEXT,
	reviewNotes,
	STOP_TEXT,
	scanView,
	totalNote,
} from './skillScanView';

function collecting(
	pagesCollected: number,
	pageCount: number | null,
	missingPages: number[],
	hint: SkillScanHint | null = null,
): SkillScanStatus {
	return { phase: 'collecting', pagesCollected, pageCount, missingPages, hint };
}

function review(overrides: Partial<SkillScanReview> = {}): SkillScanReview {
	return {
		scan: 1,
		skills: { Agility: 73.71 },
		total: { check: 'matches' },
		nearMisses: [],
		guesses: [],
		unknown: [],
		unplaced: [],
		repeated: [],
		...overrides,
	};
}

describe('scanView', () => {
	it('waits for the first status', () => {
		expect(scanView(null)).toEqual({ kind: 'loading' });
	});

	it('lets an idle scan start only when the reader is available', () => {
		expect(scanView({ phase: 'idle', available: true, stopped: null })).toEqual({
			kind: 'idle',
			canStart: true,
			note: null,
		});
		const unavailable = scanView({ phase: 'idle', available: false, stopped: null });
		expect(unavailable).toEqual({
			kind: 'idle',
			canStart: false,
			note: 'Reading skills is unavailable.',
		});
	});

	it('says why the last scan stopped, in words the player can act on', () => {
		for (const stopped of [
			'readFailed',
			'captureNotAllowed',
			'noNewPage',
			'interfaceSizeUnreadable',
		] as const) {
			expect(scanView({ phase: 'idle', available: true, stopped })).toEqual({
				kind: 'idle',
				canStart: true,
				note: STOP_TEXT[stopped],
			});
		}
		expect(STOP_TEXT.captureNotAllowed).toMatch(/asked once more/);
		expect(STOP_TEXT.noNewPage).toMatch(/no new page/);
	});

	it('counts pages and lists the missing ones only when few remain', () => {
		expect(scanView(collecting(7, 13, [2, 3, 5, 8, 9, 12]))).toEqual({
			kind: 'collecting',
			progress: '7 of 13 pages',
			fraction: 7 / 13,
			missing: null,
			hint: null,
		});
		expect(scanView(collecting(11, 13, [4, 9]))).toMatchObject({
			progress: '11 of 13 pages',
			missing: 'Still to show: pages 4 and 9',
		});
		expect(scanView(collecting(12, 13, [13]))).toMatchObject({
			missing: 'Still to show: page 13',
		});
	});

	it('says no page has been seen before the count is known', () => {
		expect(scanView(collecting(0, null, []))).toEqual({
			kind: 'collecting',
			progress: 'No pages yet',
			fraction: 0,
			missing: null,
			hint: null,
		});
	});

	it('shows the one hint the status carries, and none when it carries none', () => {
		expect(scanView(collecting(3, 13, [1], 'windowTooShort'))).toMatchObject({
			hint: HINT_TEXT.windowTooShort,
		});
		expect(scanView(collecting(3, 13, [1]))).toMatchObject({ hint: null });
	});

	it('says plainly when the window is drawn at a size no page can be read at', () => {
		expect(scanView(collecting(0, null, [], 'interfaceSizeUnreadable'))).toMatchObject({
			hint: 'The Skills window is drawn at an interface size this app cannot read yet.',
		});
		// Neither the hint nor the stop that follows it sends the player to
		// show the page again or to start again: neither would help.
		for (const text of [HINT_TEXT.interfaceSizeUnreadable, STOP_TEXT.interfaceSizeUnreadable]) {
			expect(text).toMatch(/interface size this app cannot read yet/);
			expect(text).not.toMatch(/again|retry|show it/i);
		}
		expect(
			scanView({ phase: 'idle', available: true, stopped: 'interfaceSizeUnreadable' }),
		).toEqual({
			kind: 'idle',
			canStart: true,
			note: 'The scan stopped: the Skills window is drawn at an interface size this app cannot read yet.',
		});
	});

	it('hands over to the review once every page is in, and says when it saves', () => {
		expect(scanView({ phase: 'awaitingReview' })).toEqual({ kind: 'review' });
		expect(scanView({ phase: 'saving' })).toEqual({ kind: 'saving' });
	});

	it('words every hint as something the player can do or know', () => {
		for (const text of [
			...Object.values(HINT_TEXT),
			...Object.values(STOP_TEXT),
			...Object.values(REFUSAL_TEXT),
		]) {
			expect(text).toMatch(/\.$/);
			expect(text).not.toMatch(/dock|corner|bottom-right|confidence/i);
		}
	});
});

describe('pageList', () => {
	it('lists one, two or more pages in plain words', () => {
		expect(pageList([4])).toBe('page 4');
		expect(pageList([4, 9])).toBe('pages 4 and 9');
		expect(pageList([4, 9, 12])).toBe('pages 4, 9 and 12');
	});
});

describe('totalNote', () => {
	it('says nothing when the figures add up', () => {
		expect(totalNote({ check: 'matches' })).toBeNull();
	});

	it('gives both figures when they do not', () => {
		const note = totalNote({ check: 'mismatch', sum: 4427, total: 4500 });
		expect(note).toContain('4,427');
		expect(note).toContain('4,500');
		expect(note).toMatch(/misread or missed/);
	});

	it('says when there was nothing to check against', () => {
		expect(totalNote({ check: 'unchecked' })).toMatch(/could not be checked/);
	});

	it('asks for confirmation unless the figures add up', () => {
		expect(needsConfirmation({ check: 'matches' })).toBe(false);
		expect(needsConfirmation({ check: 'mismatch', sum: 1, total: 2 })).toBe(true);
		expect(needsConfirmation({ check: 'unchecked' })).toBe(true);
	});
});

describe('reviewNotes', () => {
	it('is empty for a scan that adds up with every name read exactly', () => {
		expect(reviewNotes(review())).toEqual([]);
	});

	it('names near misses, unknown, unplaced and repeated skills, each only when present', () => {
		const notes = reviewNotes(
			review({
				total: { check: 'unchecked' },
				nearMisses: [{ skill: 'Aim', readAs: 'Aim.' }],
				guesses: [{ readAs: 'Anatomv', probably: 'Anatomy', pointsText: '40.00' }],
				unknown: [{ name: 'Food Technology', pointsText: '5.25' }],
				unplaced: [{ name: 'Arn', pointsText: '9.00' }],
				repeated: ['Analysis'],
			}),
		);
		expect(notes.map((note) => note.items)).toEqual([
			[],
			['Aim (read as "Aim.")'],
			['read as "Anatomv", probably Anatomy 40.00'],
			['Food Technology 5.25'],
			['Arn 9.00'],
			['Analysis'],
		]);
		expect(notes[2].text).toMatch(/Not saved.*scan again/);
		expect(notes[3].text).toMatch(/not saved/);
		expect(notes[4].text).toMatch(/Not saved/);
		expect(notes[5].text).toMatch(/Not saved/);

		const unknownOnly = reviewNotes(
			review({ unknown: [{ name: 'Food Technology', pointsText: '5.25' }] }),
		);
		expect(unknownOnly).toHaveLength(1);
		expect(unknownOnly[0].items).toEqual(['Food Technology 5.25']);
	});
});
