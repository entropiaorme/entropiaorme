import { describe, expect, it } from 'vitest';
import type { MinuteNoCorrection } from '$lib/api/commands.gen';
import { formatTime, NO_DATA } from '$lib/utils/format';
import { exact } from './__fixtures__/measure';
import { arrival, minutes, missing, step, stepSpan, stretch } from './__fixtures__/minutes';
import {
	actionable,
	archivedUnits,
	arrivalSentence,
	flagCount,
	measuredCell,
	minuteRows,
	minutesClock,
	missingNote,
	missingSentence,
	notProposedNote,
	rowsToShow,
	standingRecordOf,
	stretchSentence,
	toggleLabel,
	trackedText,
	unconfirmedLine,
} from './minutes';

const clock = formatTime;
const at = (index: number) => formatTime(stepSpan(index).to);
const from = (index: number) => formatTime(stepSpan(index).from);

describe('what arrived that no loot line explains', () => {
	it('names the item, its TT and when it arrived', () => {
		expect(arrivalSentence(arrival('Foo Rifle'), step(7), clock)).toBe(
			`Foo Rifle, 5.00 TT, arrived at ${at(7)}: no loot line names it`,
		);
	});

	it('says how many, where it arrived when not carried, and a Card that fell by its TT', () => {
		const seen = arrival('Weapon Cells', {
			units: 40,
			tt: exact(0.4),
			arrivedIn: { kind: 'storage', planet: 'Calypso' },
			boughtAtTt: true,
		});
		expect(arrivalSentence(seen, step(2), clock)).toBe(
			`40 Weapon Cells, 0.40 TT, arrived in storage on Calypso at ${at(2)}: no loot line names it, and your PED Card fell by its TT`,
		);
	});
});

describe('loot that never arrived', () => {
	it('names the loot line and when it was looted', () => {
		// Flagged in the step after the one it was looted in.
		expect(missingSentence(missing('Shrapnel', 4), clock)).toBe(
			`3 Shrapnel, 0.30 TT, looted at ${at(3)}: it never arrived`,
		);
	});

	it('says why the loot correction is not offered, and nothing when it is', () => {
		const notOffered = (reason: MinuteNoCorrection) =>
			missingNote(missing('Wool', 1, { correction: { kind: 'notOffered', reason } }));
		expect(missingNote(missing('Shrapnel', 1))).toBeNull();
		expect(notOffered('harvestLoot')).toBe('Harvested loot has no correction here.');
		expect(notOffered('otherLinesArrived')).toBe(
			'Other loot of this name in the session did arrive, and removing it takes out every line of it.',
		);
	});
});

describe('a stretch where the tracked cost drifted', () => {
	const steps = [0, 1, 2, 3, 4, 5].map((index) => step(index));

	it('reads as a share above what was measured, from its first minute to the end', () => {
		expect(stretchSentence('weapon', stretch(2, 5, 10, 11.8, -1.5), steps, clock)).toBe(
			`From ${from(2)} the tracked weapon cost ran 15% further above what your account measured than over the rest of the session: check the weapon's cost settings`,
		);
	});

	it('names its last minute when it ends before the session does, and a share below', () => {
		expect(stretchSentence('healing', stretch(1, 3, 1.3, 1, 0.3), steps, clock)).toBe(
			`From ${from(1)} to ${at(3)} the tracked healing cost ran 23% further below what your account measured than over the rest of the session: check the healing tools' cost settings`,
		);
	});

	it('reads in PED when nothing was measured to take a share of', () => {
		expect(stretchSentence('weapon', stretch(0, 5, 0, 0.42, -0.42), steps, clock)).toBe(
			`From ${from(0)} the tracked weapon cost ran 0.42 PED further above what your account measured than over the rest of the session: check the weapon's cost settings`,
		);
	});

	it('is sized by the drift that flagged it, never by the raw gap', () => {
		// Tracked runs 30% above over the stretch, but 25% of that is the
		// session's usual gap: only the 5% move flagged it.
		expect(stretchSentence('weapon', stretch(0, 5, 10, 13, -0.5), steps, clock)).toBe(
			`From ${from(0)} the tracked weapon cost ran 5% further above what your account measured than over the rest of the session: check the weapon's cost settings`,
		);
	});
});

describe('the figures a minute shows', () => {
	it('shows a measured figure to its precision, and an unmeasured one as absent with why', () => {
		expect(measuredCell({ kind: 'measured', amount: exact(1.25) })).toEqual({
			text: '1.2500',
			title: 'Exact',
			measured: true,
		});
		expect(measuredCell({ kind: 'unmeasured', reason: 'ammoToppedUp' })).toEqual({
			text: NO_DATA,
			title:
				'Universal Ammo arrived in this minute (topped up, or converted from Shrapnel), so what was fired is not known',
			measured: false,
		});
		expect(measuredCell({ kind: 'unmeasured', reason: 'limitedSetUnconfirmed' }).title).toBe(
			'A limited piece lost TT; pricing it needs its set confirmed',
		);
	});

	it('shows a tracked figure to the precision of the measured one beside it', () => {
		expect(trackedText(1.5, { kind: 'measured', amount: exact(1) })).toBe('1.5000');
		expect(trackedText(1.5, { kind: 'unmeasured', reason: 'ammoUnsettled' })).toBe('1.50');
	});
});

describe('the rows and what needs a look', () => {
	function session() {
		return minutes({
			steps: [
				step(0),
				step(1, {
					arrivedNotLooted: [
						arrival('Foo Rifle'),
						arrival('Old Hat', { disposition: { state: 'ignored', proposalId: 8 } }),
					],
				}),
				step(2, { lootedNeverArrived: [missing('Shrapnel', 2)] }),
				step(3, {
					arrivedNotLooted: [
						arrival('Gloves', {
							disposition: { state: 'recorded', proposalId: 9, recordId: 'r-9', as: 'purchase' },
						}),
					],
				}),
				step(4, { gap: true, between: { from: stepSpan(4).from, to: stepSpan(15).to } }),
				step(5, { settled: false }),
			],
			weaponStretches: [stretch(2, 3, 10, 11.8, -1.5)],
		});
	}

	it('puts each flag on its minute, and a stretch on its first', () => {
		const series = session();
		const rows = minuteRows(series, minutesClock(series));
		expect(rows.map((row) => row.flags.map((flag) => flag.kind))).toEqual([
			[],
			['arrival', 'arrival'],
			['stretch', 'missing'],
			['arrival'],
			[],
			[],
		]);
		expect(rows.map((row) => row.needsLook)).toEqual([false, true, true, false, false, false]);
		const drifting = rows.map((row) => row.drifting.weapon);
		expect(drifting).toEqual([false, false, true, true, false, false]);
		expect(rows.every((row) => !row.drifting.healing)).toBe(true);
	});

	it('says a step across a gap and one still settling', () => {
		const series = session();
		const rows = minuteRows(series, minutesClock(series));
		expect(rows[4].aside).toBe('across a 12 min gap');
		expect(rows[5].aside).toBe('still settling');
		expect(rows[0].aside).toBeNull();
		expect(rows[1].time).toBe(at(1));
		expect(rows[1].span).toBe(`Between ${from(1)} and ${at(1)}`);
	});

	it('counts what the player can act on, leaving out decided arrivals and stretches', () => {
		const series = session();
		// The open arrival and the loot the correction can take out; the
		// stretch is listed, not counted.
		expect(flagCount(minuteRows(series, minutesClock(series)))).toBe(2);
	});

	it('reads the stock an open arrival still stands behind, and none for any other', () => {
		const standing = arrival('Foo Rifle', {
			disposition: {
				state: 'open',
				proposalId: 7,
				possiblyRecorded: true,
				unlinkedRecordId: 'r-8',
			},
		});
		expect(standingRecordOf(standing)).toBe('r-8');
		expect(standingRecordOf(arrival('Foo Rifle'))).toBeNull();
		const recorded = arrival('Foo Rifle', {
			disposition: { state: 'recorded', proposalId: 7, recordId: 'r-8', as: 'unknownOrigin' },
		});
		expect(standingRecordOf(recorded)).toBeNull();
	});

	it('lists loot with no correction without counting it', () => {
		const series = minutes({
			steps: [
				step(0),
				step(1, {
					lootedNeverArrived: [
						missing('Wool', 1, { correction: { kind: 'notOffered', reason: 'harvestLoot' } }),
					],
				}),
			],
		});
		const rows = minuteRows(series, minutesClock(series));
		expect(flagCount(rows)).toBe(0);
		expect(rows[1].needsLook).toBe(true);
		expect(actionable(rows[1].flags[0])).toBe(false);
	});

	it('counts every line the loot correction takes out, across the session', () => {
		const series = minutes({
			steps: [
				step(0),
				step(1, { lootedNeverArrived: [missing('Shrapnel', 1)] }),
				step(2),
				step(3, { lootedNeverArrived: [missing('Shrapnel', 3, { units: 2 })] }),
				step(4, {
					lootedNeverArrived: [
						missing('Shrapnel', 4, {
							units: 9,
							correction: { kind: 'notOffered', reason: 'otherLinesArrived' },
						}),
						missing('Wool', 4),
					],
				}),
			],
		});
		expect(archivedUnits(series, 'Shrapnel')).toBe(5);
		expect(archivedUnits(series, 'Wool')).toBe(3);
		expect(archivedUnits(series, 'Hide')).toBe(0);
	});

	it('counts nothing for an arrival not offered to decide, and says why it is not', () => {
		const series = minutes({
			steps: [
				step(0, {
					arrivedNotLooted: [
						arrival('Foo Rifle', { disposition: { state: 'notProposed', reason: 'settling' } }),
					],
				}),
			],
		});
		const rows = minuteRows(series, minutesClock(series));
		expect(flagCount(rows)).toBe(0);
		expect(rows[0].needsLook).toBe(false);
		expect(notProposedNote('beforeRecognition')).toBe('Before the app recorded from your account.');
		expect(notProposedNote('outsideSession')).toBe(
			"After the session: any purchase is offered with your account's purchases.",
		);
		expect(notProposedNote('settling')).toBe('May still be explained by the next reading.');
		expect(notProposedNote('awaiting')).toBe('Offered on the next read.');
	});

	it('keeps only the minutes that need a look, and those acted on here', () => {
		const series = session();
		const rows = minuteRows(series, minutesClock(series));
		expect(rowsToShow(rows, false, new Set())).toHaveLength(6);
		expect(rowsToShow(rows, true, new Set()).map((row) => row.index)).toEqual([1, 2]);
		const kept = new Set([`arrival:${series.steps[3].readingId}:Gloves`]);
		expect(rowsToShow(rows, true, kept).map((row) => row.index)).toEqual([1, 2, 3]);
	});

	it('labels the toggle with how many need a look', () => {
		expect(toggleLabel(0)).toBe('Minute by minute');
		expect(toggleLabel(1)).toBe('Minute by minute, 1 needs a look');
		expect(toggleLabel(4)).toBe('Minute by minute, 4 need a look');
	});

	it('says loot no reading confirms, differently while the session runs', () => {
		expect(unconfirmedLine(minutes())).toBeNull();
		expect(unconfirmedLine(minutes({ lootUnconfirmed: 1.2 }))).toBe(
			'1.20 PED of loot came after the last reading, so no reading shows whether it arrived.',
		);
		expect(unconfirmedLine(minutes({ lootUnconfirmed: 1.2, running: true }))).toBe(
			'1.20 PED of loot came after the last reading; the next one shows whether it arrived.',
		);
	});
});
