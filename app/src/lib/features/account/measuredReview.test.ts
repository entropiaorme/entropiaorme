import { describe, expect, it } from 'vitest';
import { exact, row, summary, tracked, wholePec } from './__fixtures__/measure';
import {
	appendOlder,
	coverageMark,
	foldNewest,
	includedSentence,
	leftOutSentence,
	listedRows,
	noCompleteSentence,
	sessionFigures,
	sessionSeconds,
	summaryPairs,
} from './measuredReview';

describe('the listed sessions', () => {
	const complete = row({ id: 'a' });
	const partial = row({ id: 'b', coverage: 'partial' });

	it('lists complete readings only by default, or every session', () => {
		expect(listedRows([complete, partial], true).map((r) => r.id)).toEqual(['a']);
		expect(listedRows([complete, partial], false).map((r) => r.id)).toEqual(['a', 'b']);
	});

	it('marks a partial session and one still being read', () => {
		expect(coverageMark(complete)).toBeNull();
		expect(coverageMark(partial)).toBe('Partial');
		expect(coverageMark(row({ sealed: false, coverage: 'partial', endTime: null }))).toBe(
			'Being read',
		);
	});

	it('has no duration while the session runs', () => {
		expect(sessionSeconds(complete)).toBe(5400);
		expect(sessionSeconds(row({ endTime: null }))).toBeNull();
	});

	it('shows no tracked armour for a session whose armour is not recorded yet', () => {
		const figures = sessionFigures(
			row({ tracked: tracked({ armourCost: 0.4, armourUnrecorded: true }) }),
		);
		expect(figures.tracked.armour).toBeNull();
	});
});

describe('the whole-history summary', () => {
	it('sets each pair beside its difference and its share of tracked', () => {
		const pairs = summaryPairs(
			summary({
				weapon: { sessions: 2, tracked: 30, measured: exact(30.5) },
				loot: { sessions: 2, tracked: 27, measured: wholePec(26) },
				included: 2,
			}),
		);
		expect(pairs.map((pair) => pair.label)).toEqual(['Weapon', 'Healing', 'Armour', 'Loot']);
		expect(pairs[0].difference?.amount).toEqual(exact(0.5));
		expect(pairs[0].share).toBeCloseTo(0.5 / 30);
		expect(pairs[3].difference?.amount).toEqual(wholePec(-1));
	});

	it('shows nothing, never zero, for a pair no session went into', () => {
		const [, healing] = summaryPairs(summary({ included: 1 }));
		expect(healing).toMatchObject({ sessions: 0, tracked: null, measured: null, difference: null });
	});

	it('gives no share, never infinity or zero, when the tracked sum is nothing', () => {
		const [, healing] = summaryPairs(
			summary({ healing: { sessions: 1, tracked: 0, measured: exact(0.2) }, included: 1 }),
		);
		expect(healing.difference?.amount).toEqual(exact(0.2));
		expect(healing.share).toBeNull();
	});

	it('says how many sessions it covers, and never sums over none', () => {
		expect(includedSentence(summary({ included: 1 }))).toBe(
			'Summed over 1 session with complete readings in the PED stats.',
		);
		const none = includedSentence(summary());
		expect(none).toMatch(/nothing to sum/);
		expect(none).not.toMatch(/over 0/);
	});

	it('says which sessions with readings it leaves out, and why', () => {
		expect(leftOutSentence(summary())).toBeNull();
		expect(
			leftOutSentence(
				summary({
					leftOut: [
						{ reason: 'partialCoverage', sessions: 2 },
						{ reason: 'excluded', sessions: 1 },
						{ reason: 'notSealed', sessions: 1 },
					],
				}),
			),
		).toBe(
			'Not in these figures: 4 sessions with readings (2 with partial readings, 1 left out of the PED stats and 1 whose closing readings are not in yet).',
		);
		expect(
			leftOutSentence(
				summary({
					leftOut: [
						{ reason: 'partialCoverage', sessions: 0 },
						{ reason: 'excluded', sessions: 1 },
						{ reason: 'notSealed', sessions: 0 },
					],
				}),
			),
		).toBe('Not in these figures: 1 session with readings (1 left out of the PED stats).');
	});

	it('claims no more than the loaded sessions when none listed is complete', () => {
		expect(noCompleteSentence(3, false)).toBe(
			'None of the 3 sessions with readings is complete yet.',
		);
		expect(noCompleteSentence(50, true)).toBe(
			'None of the newest 50 sessions with readings is complete yet.',
		);
		expect(noCompleteSentence(1, false)).toBe('The one session with readings is not complete yet.');
	});
});

describe('the loaded window', () => {
	const page = (ids: string[], nextCursor: string | null) => ({
		sessions: ids.map((id, index) =>
			row({ id, startTime: `2026-11-${String(20 - index).padStart(2, '0')}T10:00:00Z` }),
		),
		nextCursor,
		total: ids.length,
	});

	it('takes a newest page that holds every session as the whole list, so a deleted one goes', () => {
		const loaded = page(['a', 'b', 'c'], null).sessions;
		const folded = foldNewest(loaded, null, page(['a', 'c'], null));
		expect(folded.rows.map((r) => r.id)).toEqual(['a', 'c']);
		expect(folded.cursor).toBeNull();
	});

	it('keeps older loaded rows past the newest page, with the cursor after them', () => {
		const loaded = [
			row({ id: 'n1', startTime: '2026-11-20T10:00:00Z' }),
			row({ id: 'n2', startTime: '2026-11-19T10:00:00Z' }),
			row({ id: 'o1', startTime: '2026-11-18T10:00:00Z' }),
		];
		// A new session pushed n2 off the newest page.
		const newest = {
			sessions: [
				row({ id: 'new', startTime: '2026-11-21T10:00:00Z' }),
				row({ id: 'n1', startTime: '2026-11-20T10:00:00Z' }),
			],
			nextCursor: 'after-n1',
			total: 4,
		};
		const folded = foldNewest(loaded, 'after-o1', newest);
		expect(folded.rows.map((r) => r.id)).toEqual(['new', 'n1', 'n2', 'o1']);
		expect(folded.cursor).toBe('after-o1');
	});

	it('appends an older page once', () => {
		const loaded = page(['a', 'b'], 'x').sessions;
		const rows = appendOlder(loaded, {
			sessions: [loaded[1], row({ id: 'c' })],
			nextCursor: null,
			total: 3,
		});
		expect(rows.map((r) => r.id)).toEqual(['a', 'b', 'c']);
	});
});
