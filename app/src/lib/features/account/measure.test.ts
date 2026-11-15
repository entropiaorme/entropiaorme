import { describe, expect, it } from 'vitest';
import type { MeasuredRepairCost } from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { carried, exact, measure, SPAN, totals, wholePec } from './__fixtures__/measure';
import {
	armourAside,
	comparisonRows,
	describeCoverage,
	difference,
	formatMeasured,
	formatShare,
	formatSigned,
	lootAsides,
	measuredByCategory,
	measureTotals,
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

describe('a measure added up by category', () => {
	it('groups wear by role as the list of measured sessions does', () => {
		const added = measureTotals(
			measure({
				decay: [
					{ item: carried('Rifle'), role: 'weapon', decay: exact(1) },
					{ item: carried('Amp'), role: 'amplifier', decay: exact(0.5) },
					{ item: carried('FAP'), role: 'healing', decay: exact(0.25) },
					{ item: carried('Harness'), role: 'armour', decay: wholePec(0.1) },
					{ item: carried('Plate'), role: 'plating', decay: exact(0.2) },
					{ item: carried('Scope'), role: 'otherEquipment', decay: exact(0.01) },
					{ item: carried('Oil'), role: 'material', decay: exact(0.02) },
				],
			}),
		);
		expect(added.weaponDecay).toEqual(exact(1.5));
		expect(added.healingDecay).toEqual(exact(0.25));
		expect(added.armourDecay).toEqual(wholePec(0.3));
		expect(added.otherEquipmentDecay).toEqual(exact(0.03));
	});

	it('knows the ammo burned only when every type settled', () => {
		const settled = {
			name: 'Universal Ammo',
			netUnits: -1000,
			net: exact(-0.1),
			split: {
				kind: 'settled' as const,
				burnedUnits: 1000,
				burned: exact(0.1),
				strongboxUnits: 0,
				strongbox: exact(0),
				convertedUnits: 0,
				converted: exact(0),
			},
		};
		expect(measureTotals(measure({ ammo: [settled] })).ammo).toEqual({
			kind: 'settled',
			burned: exact(0.1),
		});
		const unsettled = {
			...settled,
			name: 'Explosive Projectiles',
			split: {
				kind: 'unsettled' as const,
				burnedUnitsByRows: 900,
				strongboxUnitsByRows: 100,
				convertedUnits: 0,
			},
		};
		expect(measureTotals(measure({ ammo: [settled, unsettled] })).ammo).toEqual({
			kind: 'unsettled',
		});
	});

	it('adds repairs by how the PED Card shows their cost', () => {
		const repair = (cost: MeasuredRepairCost) => ({
			between: SPAN,
			items: [],
			restored: exact(1),
			cost,
		});
		const added = measureTotals(
			measure({
				repairs: [
					repair({ kind: 'paid', amount: wholePec(1) }),
					repair({ kind: 'derived', amount: wholePec(0.5) }),
					repair({ kind: 'unknown' }),
				],
			}),
		);
		expect(added.repairCost).toEqual(wholePec(1));
		expect(added.repairCostDerived).toEqual(wholePec(0.5));
		expect(added.repairsUncosted).toBe(1);
	});
});

describe('the pairing', () => {
	const tracked = { weapon: 10, healing: 2, armour: 1, loot: 9 };

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
			tracked,
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
		const rows = comparisonRows(totals({ gained: exact(1) }), NOTHING_TRACKED);
		expect(rows.every((row) => row.difference === null)).toBe(true);
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
			trackedFromRow({ weaponCost: 1, healingCost: 2, armourCost: 3, lootTt: 4, totalCycled: 6 }),
		).toEqual({ weapon: 1, healing: 2, armour: 3, loot: 4 });
		expect(trackedFromRow(null)).toEqual(NOTHING_TRACKED);
	});

	it('reads a session summary, with armour unknown while its hits await a recording', () => {
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
				armourCost: 0,
				harvestCost: 0,
			},
		};
		expect(trackedFromSummary(summary, false)).toEqual({
			weapon: 8,
			healing: 1,
			armour: 0,
			loot: 9,
		});
		expect(trackedFromSummary(summary, true).armour).toBeNull();
		const repaired = { ...summary, costBreakdown: { ...summary.costBreakdown, armourCost: 0.5 } };
		expect(trackedFromSummary(repaired, true).armour).toBe(0.5);
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
				gainedPossiblyStrongbox: exact(2),
				gained: [
					{
						item: carried('Shrapnel'),
						role: 'material',
						stacking: 'stack',
						mark: 'cardMovedUnexplained',
						units: 100,
						amount: exact(0.01),
					},
				],
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
		expect(describeCoverage(coverage, true).state).toBe('complete');
	});

	it('gives the plain reason a measure is partial', () => {
		expect(describeCoverage({ ...coverage, level: 'partial', opening: 'unread' }, true).text).toBe(
			'Partial: the readings missed the start of this session.',
		);
		expect(describeCoverage({ ...coverage, level: 'partial', closing: 'unread' }, true).text).toBe(
			'Partial: the readings missed the end of this session.',
		);
		expect(
			describeCoverage(
				{ ...coverage, level: 'partial', opening: 'unread', closing: 'unread' },
				true,
			).text,
		).toBe('Partial: the readings missed the start and the end of this session.');
		expect(describeCoverage({ ...coverage, level: 'partial', gapCount: 2 }, true).text).toBe(
			'Partial: the readings missed part of this session.',
		);
	});

	it('does not call a session still being read partial for lacking its closing reading', () => {
		const running = describeCoverage({ ...coverage, level: 'partial', closing: 'unread' }, false);
		expect(running.state).toBe('reading');
		const gapped = describeCoverage(
			{ ...coverage, level: 'partial', closing: 'unread', gapCount: 1 },
			false,
		);
		expect(gapped.state).toBe('partial');
		expect(gapped.text).toContain('Still being read.');
	});
});
