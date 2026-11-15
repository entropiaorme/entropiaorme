import { describe, expect, it } from 'vitest';
import { exact, row, totals, wholePec } from './__fixtures__/measure';
import { coverageMark, listedRows, sessionSeconds, summarise } from './measuredReview';

const tracked = (weapon: number, loot: number) => ({
	weaponCost: weapon,
	healingCost: 0,
	armourCost: 0,
	lootTt: loot,
	totalCycled: weapon,
});

const EXCLUDED = {
	economyExcludedAt: '2026-11-15T15:00:00Z',
	progressionExcludedAt: null,
	note: 'test run',
};

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
});

describe('the sum over sessions', () => {
	it('sums each pair over the complete sessions in the PED stats', () => {
		const summary = summarise([
			row({
				id: 'a',
				tracked: tracked(10, 9),
				measured: totals({
					ammo: { kind: 'settled', burned: exact(9) },
					weaponDecay: exact(1.5),
					gained: exact(9),
				}),
			}),
			row({
				id: 'b',
				tracked: tracked(20, 18),
				measured: totals({
					ammo: { kind: 'settled', burned: exact(19) },
					weaponDecay: exact(1),
					gained: wholePec(17),
				}),
			}),
			// Partial: never summed.
			row({
				id: 'c',
				coverage: 'partial',
				tracked: tracked(100, 100),
				measured: totals({ gained: exact(1) }),
			}),
			// Left out of the PED stats: left out here too.
			row({
				id: 'd',
				exclusion: EXCLUDED,
				tracked: tracked(100, 100),
				measured: totals({ gained: exact(1) }),
			}),
		]);
		expect(summary.sessions).toBe(2);
		const weapon = summary.categories.find((sum) => sum.category === 'weapon');
		expect(weapon?.tracked).toBe(30);
		expect(weapon?.measured).toEqual(exact(30.5));
		expect(weapon?.difference?.amount).toEqual(exact(0.5));
		expect(weapon?.share).toBeCloseTo(0.5 / 30);
		const loot = summary.categories.find((sum) => sum.category === 'loot');
		expect(loot?.measured).toEqual(wholePec(26));
		expect(loot?.difference?.amount).toEqual(wholePec(-1));
	});

	it('counts a session for skill stats exclusion only, since the PED side still counts', () => {
		const summary = summarise([
			row({
				exclusion: {
					...EXCLUDED,
					economyExcludedAt: null,
					progressionExcludedAt: EXCLUDED.economyExcludedAt,
				},
			}),
		]);
		expect(summary.sessions).toBe(1);
	});

	it('leaves a session out of a category where one side is not known', () => {
		const summary = summarise([
			row({ id: 'a', tracked: tracked(10, 9), measured: totals({ ammo: { kind: 'unsettled' } }) }),
			row({ id: 'b', tracked: null, measured: totals({ gained: exact(5) }) }),
			row({
				id: 'c',
				tracked: tracked(4, 3),
				measured: totals({ ammo: { kind: 'settled', burned: exact(4) }, gained: exact(3) }),
			}),
		]);
		const weapon = summary.categories.find((sum) => sum.category === 'weapon');
		expect(weapon?.sessions).toBe(1);
		expect(weapon?.tracked).toBe(4);
		const loot = summary.categories.find((sum) => sum.category === 'loot');
		expect(loot?.sessions).toBe(2);
		expect(loot?.tracked).toBe(12);
		expect(loot?.measured).toEqual(exact(3));
		const other = summary.categories.find((sum) => sum.category === 'other');
		expect(other?.sessions).toBe(3);
		expect(other?.tracked).toBeNull();
	});

	it('gives no share, never infinity or zero, when the tracked sum is nothing', () => {
		const summary = summarise([
			row({ tracked: tracked(0, 0), measured: totals({ healingDecay: exact(0.2) }) }),
		]);
		const healing = summary.categories.find((sum) => sum.category === 'healing');
		expect(healing?.difference?.amount).toEqual(exact(0.2));
		expect(healing?.share).toBeNull();
	});

	it('has nothing to sum without a complete session', () => {
		const summary = summarise([row({ coverage: 'partial' })]);
		expect(summary.sessions).toBe(0);
		expect(summary.categories.find((sum) => sum.category === 'weapon')?.difference).toBeNull();
	});
});
