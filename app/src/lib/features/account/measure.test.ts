import { describe, expect, it } from 'vitest';
import { NO_DATA } from '$lib/utils/format';
import { carried, exact, gain, measure, totals, tracked, wholePec } from './__fixtures__/measure';
import {
	armourAside,
	comparisonRows,
	coversWhole,
	describeCoverage,
	difference,
	formatMeasured,
	formatShare,
	formatSigned,
	formatTracked,
	lootAsides,
	measuredByCategory,
	NOTHING_TRACKED,
	shareOfTracked,
	sumAmounts,
	trackedFromRow,
	trackedFromSummary,
	unsettledAmmoAsides,
} from './measure';

describe('precision-aware formatting', () => {
	it('shows an exact amount to four decimals and a whole-PEC one to two', () => {
		expect(formatMeasured(exact(1.23456))).toBe('1.2346');
		expect(formatMeasured(wholePec(1.2))).toBe('1.20');
	});

	it('never shows a negative zero', () => {
		expect(formatMeasured(exact(-0.00001))).toBe('0.0000');
		expect(formatMeasured(wholePec(-0.001))).toBe('0.00');
	});

	it('signs anything that is not zero at the precision shown', () => {
		expect(formatSigned(exact(0.5))).toBe('+0.5000');
		expect(formatSigned(exact(-0.5))).toBe('-0.5000');
		expect(formatSigned(wholePec(-0.001))).toBe('0.00');
		expect(formatSigned(exact(0))).toBe('0.0000');
	});

	it('signs a share, and shows a missing one as the no-data dash', () => {
		expect(formatShare(0.0123)).toBe('+1.2%');
		expect(formatShare(-0.5)).toBe('-50.0%');
		expect(formatShare(0)).toBe('0.0%');
		expect(formatShare(null)).toBe(NO_DATA);
		expect(formatShare(Number.POSITIVE_INFINITY)).toBe(NO_DATA);
	});
});

describe('sums', () => {
	it('is only as exact as its least exact part', () => {
		expect(sumAmounts([exact(1), exact(2)])).toEqual(exact(3));
		expect(sumAmounts([exact(1), wholePec(2)])).toEqual(wholePec(3));
		expect(sumAmounts([])).toEqual(exact(0));
	});

	it('does not drift over many parts', () => {
		const parts = Array.from({ length: 1000 }, () => exact(0.0001));
		expect(sumAmounts(parts).ped).toBe(0.1);
	});
});

describe('the pairing', () => {
	const figures = { weapon: 10, healing: 2, armour: 1, loot: 9 };

	it('stands each tracked figure beside the measured ones that answer it, in order', () => {
		const rows = comparisonRows(
			totals({
				ammo: { kind: 'settled', burned: exact(8) },
				weaponDecay: exact(2.5),
				healingDecay: exact(2),
				armourDecay: exact(0.75),
				otherEquipmentDecay: exact(0.1),
				otherConsumption: wholePec(0.4),
				gained: exact(9.5),
			}),
			figures,
			true,
		);
		expect(rows.map((row) => row.label)).toEqual(['Weapon', 'Healing', 'Armour', 'Other', 'Loot']);
		expect(rows[0].measured).toEqual({ kind: 'amount', amount: exact(10.5) });
		expect(rows[0].difference?.amount).toEqual(exact(0.5));
		expect(rows[2].difference?.amount).toEqual(exact(-0.25));
		expect(rows[3].tracked).toBeNull();
		expect(rows[3].measured).toEqual({ kind: 'amount', amount: wholePec(0.5) });
		expect(rows[3].difference).toBeNull();
		expect(rows[4].difference?.amount).toEqual(exact(0.5));
	});

	it('forms no weapon figure while the ammo split is unsettled', () => {
		const measured = measuredByCategory(
			totals({ ammo: { kind: 'unsettled' }, weaponDecay: exact(3) }),
		);
		expect(measured.weapon).toEqual({ kind: 'unsettled' });
		expect(difference(10, measured.weapon)).toBeNull();
	});

	it('has no difference where nothing was tracked', () => {
		const rows = comparisonRows(totals({ gained: exact(1) }), NOTHING_TRACKED, true);
		expect(rows.every((row) => row.difference === null)).toBe(true);
	});

	it('never differences a measure of part of a session against the whole', () => {
		const rows = comparisonRows(totals({ gained: exact(1) }), figures, false);
		expect(rows.every((row) => row.difference === null)).toBe(true);
		expect(rows[4].measured).toEqual({ kind: 'amount', amount: exact(1) });
	});

	it('takes a measure as whole only once sealed and complete', () => {
		expect(coversWhole(measure())).toBe(true);
		expect(coversWhole(measure({ sealed: false }))).toBe(false);
		expect(
			coversWhole(measure({ coverage: { ...measure().coverage, level: 'partial', gapCount: 1 } })),
		).toBe(false);
	});

	it('shows a tracked figure to four decimals beside an exact measured one, to two otherwise', () => {
		expect(formatTracked(12.3412, 'exact')).toBe('12.3412');
		expect(formatTracked(12.3412, 'wholePec')).toBe('12.34');
		expect(formatTracked(null, 'exact')).toBe(NO_DATA);
	});

	it('reads two figures within a PEC as agreeing', () => {
		expect(difference(1, { kind: 'amount', amount: exact(1.01) })?.negligible).toBe(true);
		expect(difference(1, { kind: 'amount', amount: exact(1.0101) })?.negligible).toBe(false);
	});

	it('keeps the measured figure precision in the difference', () => {
		expect(difference(1, { kind: 'amount', amount: wholePec(1.5) })?.amount).toEqual(wholePec(0.5));
	});

	it('gives no share of a tracked figure that is nothing', () => {
		expect(shareOfTracked(exact(1), 0)).toBeNull();
		expect(shareOfTracked(exact(1), null)).toBeNull();
		expect(shareOfTracked(exact(1), 4)).toBe(0.25);
	});
});

describe('tracked figures', () => {
	it('reads a measured session row, or nothing without a summary', () => {
		expect(
			trackedFromRow(tracked({ weaponCost: 1, healingCost: 2, armourCost: 3, lootTt: 4 })),
		).toEqual({ weapon: 1, healing: 2, armour: 3, loot: 4 });
		expect(trackedFromRow(null)).toEqual(NOTHING_TRACKED);
	});

	const summary = {
		cost: 10,
		returns: 9,
		pes: 0,
		net: -1,
		returnRate: 0.9,
		kills: 10,
		duration: 600,
		costBreakdown: {
			weaponCost: 8,
			healCost: 1,
			consumableCost: 0,
			enhancerCost: 0,
			armourCost: 0.5,
			harvestCost: 0,
		},
	};

	it('reads a session summary', () => {
		expect(trackedFromSummary(summary, false)).toEqual({
			weapon: 8,
			healing: 1,
			armour: 0.5,
			loot: 9,
		});
	});

	it('reads one session alike in its detail and in the review while its armour is not recorded', () => {
		const inDetail = trackedFromSummary(summary, true);
		const inReview = trackedFromRow(
			tracked({
				weaponCost: 8,
				healingCost: 1,
				armourCost: 0.5,
				armourUnrecorded: true,
				lootTt: 9,
			}),
		);
		expect(inDetail.armour).toBeNull();
		expect(inReview).toEqual(inDetail);
	});
});

describe('the asides', () => {
	it('notes on the armour row, only while both figures exist, that they measure different things', () => {
		expect(armourAside(1, { kind: 'amount', amount: exact(0.5) })).toMatch(/repairs cost/);
		expect(armourAside(0, { kind: 'amount', amount: exact(0.5) })).toBeNull();
		expect(armourAside(null, { kind: 'amount', amount: exact(0.5) })).toBeNull();
		expect(armourAside(1, { kind: 'amount', amount: exact(0) })).toBeNull();
	});

	it("says how much of what arrived may be a strongbox's contents or came with an unexplained card move", () => {
		const lines = lootAsides(
			measure({
				gained: [
					gain('Fireworks', exact(2), 'possiblyStrongbox'),
					gain('Shrapnel', exact(0.01), 'cardMovedUnexplained'),
				],
				gainedTotal: exact(2.01),
				gainedPossiblyStrongbox: exact(2),
				totals: totals({ gained: exact(2.01), gainedPossiblyStrongbox: exact(2) }),
			}),
		);
		expect(lines).toEqual([
			"of which 2.0000 PED may be a strongbox's contents",
			'of which 0.0100 PED arrived in a minute the PED Card moved unexplained',
		]);
		expect(lootAsides(measure())).toEqual([]);
	});

	it('shows an unsettled ammo type by its net change and never a burned figure', () => {
		const lines = unsettledAmmoAsides(
			measure({
				decay: [{ item: carried('Rifle'), role: 'weapon', decay: exact(1.5) }],
				totals: totals({ ammo: { kind: 'unsettled' }, weaponDecay: exact(1.5) }),
				ammo: [
					{
						name: 'Universal Ammo',
						netUnits: -58000,
						net: exact(-5.8),
						split: {
							kind: 'unsettled',
							burnedUnitsByRows: 57000,
							strongboxUnitsByRows: 1000,
							convertedUnits: 0,
						},
					},
				],
			}),
		);
		expect(lines[0]).toBe('Weapon and amplifier wear 1.5000 PED');
		expect(lines[1]).toContain('Universal Ammo: net change -58000 units');
		expect(lines[1]).toContain(
			'the split between ammo fired and ammo paid out by strongboxes could not be settled',
		);
		expect(lines.join(' ')).not.toContain('57000');
		expect(unsettledAmmoAsides(measure())).toEqual([]);
	});
});

describe('coverage', () => {
	const coverage = measure().coverage;

	it('is complete when the readings bracket a finished session', () => {
		expect(describeCoverage(coverage, true, true).state).toBe('complete');
	});

	it('gives the plain reason a measure is partial', () => {
		expect(
			describeCoverage({ ...coverage, level: 'partial', opening: 'unread' }, true, true).text,
		).toBe('Partial: the readings missed the start of this session.');
		expect(
			describeCoverage({ ...coverage, level: 'partial', closing: 'unread' }, true, true).text,
		).toBe('Partial: the readings missed the end of this session.');
		expect(
			describeCoverage(
				{ ...coverage, level: 'partial', opening: 'unread', closing: 'unread' },
				true,
				true,
			).text,
		).toBe('Partial: the readings missed the start and the end of this session.');
		expect(describeCoverage({ ...coverage, level: 'partial', gapCount: 2 }, true, true).text).toBe(
			'Partial: the readings missed part of this session.',
		);
	});

	it('does not call a session still being read partial for lacking its closing reading', () => {
		const lacking = { ...coverage, level: 'partial' as const, closing: 'unread' as const };
		const running = describeCoverage(lacking, false, false);
		expect(running.state).toBe('reading');
		expect(running.text).toBe('Still being read: the session is running.');
		const gapped = describeCoverage({ ...lacking, gapCount: 1 }, false, false);
		expect(gapped.state).toBe('partial');
		expect(gapped.text).toContain('Still being read');
	});

	it('says an ended session is waiting on its closing readings, not that it grows', () => {
		const lacking = { ...coverage, level: 'partial' as const, closing: 'unread' as const };
		expect(describeCoverage(lacking, false, true).text).toBe(
			'The session has ended; its closing readings are still being taken.',
		);
		expect(describeCoverage(lacking, false, null).text).toBe(
			"Still being read: these figures may change until the session's closing readings are in.",
		);
		expect(describeCoverage(lacking, false, true).text).not.toMatch(/grow/);
	});
});
