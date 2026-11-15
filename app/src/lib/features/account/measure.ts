/**
 * What the account readings measured of a session, set beside what the
 * session tracked.
 *
 * Measured figures stand beside the tracked ones and are never added into
 * them: nothing here forms a measured cost, a measured net, or a measured
 * return. Each category pairs one tracked figure with the measured figures
 * that answer it, and the difference between the two is a discrepancy
 * between two sources, never a result.
 *
 * Every measured amount carries how exactly it is known: an exact amount
 * (from a quantity and an exact unit value) shows four decimals, one read
 * from values the site truncated to the whole PEC shows two. A sum is only
 * as exact as its least exact part. A figure that is not known (an ammo
 * split that could not be settled, a tracked figure the session has not
 * recorded) is absent, never zero.
 */

import type {
	AccountSessionMeasure,
	MeasuredAmount,
	MeasuredCoverage,
	MeasuredPrecision,
	MeasuredTotals,
	SessionSummary,
	TrackedSessionFigures,
} from '$lib/api/commands.gen';
import { formatPed, formatPercent, formatQuantity, NO_DATA } from '$lib/utils/format';

const MICRO_PER_PED = 1_000_000;
/** Within this much (a PEC), two figures read as agreeing. */
const NEGLIGIBLE_MICRO = 10_000;

function micro(ped: number): number {
	return Math.round(ped * MICRO_PER_PED);
}

function amountOf(microPed: number, precision: MeasuredPrecision): MeasuredAmount {
	return { ped: microPed / MICRO_PER_PED, precision };
}

/** Nothing, exactly. */
export const NO_AMOUNT: MeasuredAmount = { ped: 0, precision: 'exact' };

/** The less exact of two precisions. */
export function weaker(a: MeasuredPrecision, b: MeasuredPrecision): MeasuredPrecision {
	return a === 'wholePec' || b === 'wholePec' ? 'wholePec' : 'exact';
}

/** The sum of some amounts, as exact as its least exact part. Summed in
 * micro-PED so a long list cannot drift. */
export function sumAmounts(amounts: readonly MeasuredAmount[]): MeasuredAmount {
	let total = 0;
	let precision: MeasuredPrecision = 'exact';
	for (const amount of amounts) {
		total += micro(amount.ped);
		precision = weaker(precision, amount.precision);
	}
	return amountOf(total, precision);
}

/** Whether an amount is nothing at all. */
export function isNothing(amount: MeasuredAmount): boolean {
	return micro(amount.ped) === 0;
}

/** An amount at the precision it is known to: four decimals when exact,
 * two when it rests on whole-PEC values. */
export function formatMeasured(amount: MeasuredAmount): string {
	const value = micro(amount.ped) / MICRO_PER_PED;
	const text = amount.precision === 'exact' ? value.toFixed(4) : formatPed(value);
	return /^-0\.0+$/.test(text) ? text.slice(1) : text;
}

/** A signed amount: a sign before anything that is not zero at the
 * precision shown. */
export function formatSigned(amount: MeasuredAmount): string {
	const magnitude = formatMeasured({ ped: Math.abs(amount.ped), precision: amount.precision });
	if (/^0\.0+$/.test(magnitude)) return magnitude;
	return `${amount.ped < 0 ? '-' : '+'}${magnitude}`;
}

// ── A measure's figures by category ─────────────────────────────────────

/**
 * A measure's figures by category, as the list of measured sessions carries
 * them for each row. A single session's measure arrives without them, so
 * they are added up here by the same rules: wear by role (weapons with
 * their amplifiers, armour with its plates, everything else as other
 * equipment), the ammo burned only when every type's split settled, and a
 * repair's cost by how the PED Card shows it.
 */
export function measureTotals(measure: AccountSessionMeasure): MeasuredTotals {
	const wear = (roles: readonly string[]) =>
		sumAmounts(measure.decay.filter((item) => roles.includes(item.role)).map((item) => item.decay));
	const burned: MeasuredAmount[] = [];
	let settled = true;
	for (const used of measure.ammo) {
		if (used.split.kind === 'settled') burned.push(used.split.burned);
		else settled = false;
	}
	const paid: MeasuredAmount[] = [];
	const derived: MeasuredAmount[] = [];
	let uncosted = 0;
	for (const repair of measure.repairs) {
		if (repair.cost.kind === 'paid') paid.push(repair.cost.amount);
		else if (repair.cost.kind === 'derived') derived.push(repair.cost.amount);
		else uncosted += 1;
	}
	return {
		ammo: settled ? { kind: 'settled', burned: sumAmounts(burned) } : { kind: 'unsettled' },
		weaponDecay: wear(['weapon', 'amplifier']),
		healingDecay: wear(['healing']),
		armourDecay: wear(['armour', 'plating']),
		otherEquipmentDecay: wear(['ammo', 'otherEquipment', 'material']),
		otherConsumption: sumAmounts(measure.otherConsumption.map((item) => item.amount)),
		unexplainedDepartures: sumAmounts(measure.unexplainedDepartures.map((item) => item.amount)),
		repairCost: sumAmounts(paid),
		repairCostDerived: sumAmounts(derived),
		repairsUncosted: uncosted,
		gained: measure.gainedTotal,
		gainedPossiblyStrongbox: measure.gainedPossiblyStrongbox,
	};
}

// ── The pairing ─────────────────────────────────────────────────────────

export type Category = 'weapon' | 'healing' | 'armour' | 'other' | 'loot';

/** The categories in the order they are compared. */
export const CATEGORIES: readonly Category[] = ['weapon', 'healing', 'armour', 'other', 'loot'];

export const CATEGORY_LABELS: Record<Category, string> = {
	weapon: 'Weapon',
	healing: 'Healing',
	armour: 'Armour',
	other: 'Other',
	loot: 'Loot',
};

/** A category with a tracked figure to stand beside. */
export type PairedCategory = Exclude<Category, 'other'>;

/** The categories with a tracked figure to stand beside, in order. */
export const PAIRED: readonly PairedCategory[] = ['weapon', 'healing', 'armour', 'loot'];

/** A session's tracked figures, by category; `null` where the session has
 * not recorded one. Other has no tracked counterpart. */
export type TrackedFigures = Record<PairedCategory, number | null>;

export const NOTHING_TRACKED: TrackedFigures = {
	weapon: null,
	healing: null,
	armour: null,
	loot: null,
};

/** The tracked figures a measured session row carries. */
export function trackedFromRow(tracked: TrackedSessionFigures | null): TrackedFigures {
	if (!tracked) return NOTHING_TRACKED;
	return {
		weapon: tracked.weaponCost,
		healing: tracked.healingCost,
		armour: tracked.armourCost,
		loot: tracked.lootTt,
	};
}

/** The tracked figures of a session's own summary. Armour cost is recorded
 * at a repair, possibly sessions later, so while the session's hits await
 * one its armour figure is not known rather than nothing. */
export function trackedFromSummary(
	summary: SessionSummary,
	armourUnrecorded: boolean,
): TrackedFigures {
	const costs = summary.costBreakdown;
	return {
		weapon: costs.weaponCost,
		healing: costs.healCost,
		armour: armourUnrecorded && micro(costs.armourCost) === 0 ? null : costs.armourCost,
		loot: summary.returns,
	};
}

/** A measured figure: an amount, or the ammo burned that could not be
 * settled (so neither is the weapon figure it is part of). */
export type MeasuredFigure = { kind: 'amount'; amount: MeasuredAmount } | { kind: 'unsettled' };

/** Which measured figures answer each tracked one. */
export function measuredByCategory(totals: MeasuredTotals): Record<Category, MeasuredFigure> {
	const amount = (value: MeasuredAmount): MeasuredFigure => ({ kind: 'amount', amount: value });
	return {
		weapon:
			totals.ammo.kind === 'settled'
				? amount(sumAmounts([totals.ammo.burned, totals.weaponDecay]))
				: { kind: 'unsettled' },
		healing: amount(totals.healingDecay),
		armour: amount(totals.armourDecay),
		other: amount(sumAmounts([totals.otherEquipmentDecay, totals.otherConsumption])),
		loot: amount(totals.gained),
	};
}

/** Measured minus tracked, at the measured figure's precision, and
 * whether the two agree within a PEC. */
export interface Difference {
	amount: MeasuredAmount;
	negligible: boolean;
}

/** The difference between a measured and a tracked figure; `null` when
 * either is not known. */
export function difference(tracked: number | null, measured: MeasuredFigure): Difference | null {
	if (tracked === null || measured.kind !== 'amount') return null;
	const delta = micro(measured.amount.ped) - micro(tracked);
	return {
		amount: amountOf(delta, measured.amount.precision),
		negligible: Math.abs(delta) <= NEGLIGIBLE_MICRO,
	};
}

/** A difference as a share of the tracked figure; `null` when the tracked
 * figure is nothing (a share of nothing says nothing). */
export function shareOfTracked(delta: MeasuredAmount, tracked: number | null): number | null {
	if (tracked === null || micro(tracked) === 0) return null;
	return delta.ped / tracked;
}

/** A share as signed text ("+1.2%"), or the no-data dash. */
export function formatShare(share: number | null): string {
	if (share === null || !Number.isFinite(share)) return NO_DATA;
	const text = formatPercent(Math.abs(share));
	if (/^0\.0%$/.test(text)) return text;
	return `${share < 0 ? '-' : '+'}${text}`;
}

/** One line of the comparison. */
export interface ComparisonRow {
	category: Category;
	label: string;
	tracked: number | null;
	measured: MeasuredFigure;
	difference: Difference | null;
}

/** The comparison, one row per category in order. Other has no tracked
 * figure, so no difference. */
export function comparisonRows(totals: MeasuredTotals, tracked: TrackedFigures): ComparisonRow[] {
	const measured = measuredByCategory(totals);
	return CATEGORIES.map((category) => {
		const trackedFigure = category === 'other' ? null : tracked[category];
		return {
			category,
			label: CATEGORY_LABELS[category],
			tracked: trackedFigure,
			measured: measured[category],
			difference: difference(trackedFigure, measured[category]),
		};
	});
}

// ── The quiet asides ────────────────────────────────────────────────────

/** The armour row's aside, only while both figures exist: the two measure
 * different things. */
export function armourAside(tracked: number | null, measured: MeasuredFigure): string | null {
	if (tracked === null || micro(tracked) === 0) return null;
	if (measured.kind !== 'amount' || isNothing(measured.amount)) return null;
	return 'tracked is what repairs cost, measured is the wear';
}

/** The weapon row's asides when the ammo burned could not be settled: the
 * wear that is known, and each unsettled type's net change in units. */
export function unsettledAmmoAsides(measure: AccountSessionMeasure): string[] {
	if (measure.ammo.every((used) => used.split.kind === 'settled')) return [];
	const lines: string[] = [];
	const wear = measureTotals(measure).weaponDecay;
	lines.push(`Weapon and amplifier wear ${formatMeasured(wear)} PED`);
	for (const used of measure.ammo) {
		if (used.split.kind !== 'unsettled') continue;
		lines.push(
			`${used.name}: net change ${formatUnits(used.netUnits, true)} units; the split between ammo fired and ammo paid out by strongboxes could not be settled`,
		);
	}
	return lines;
}

/** The loot row's "of which" lines: what arrived that may be a strongbox's
 * contents, and what arrived in a minute the PED Card moved unexplained. */
export function lootAsides(measure: AccountSessionMeasure): string[] {
	const lines: string[] = [];
	if (!isNothing(measure.gainedPossiblyStrongbox)) {
		lines.push(
			`of which ${formatMeasured(measure.gainedPossiblyStrongbox)} PED may be a strongbox's contents`,
		);
	}
	const unexplained = sumAmounts(
		measure.gained
			.filter((gain) => gain.mark === 'cardMovedUnexplained')
			.map((gain) => gain.amount),
	);
	if (!isNothing(unexplained)) {
		lines.push(
			`of which ${formatMeasured(unexplained)} PED arrived in a minute the PED Card moved unexplained`,
		);
	}
	return lines;
}

/** A count of units, with a sign when asked for. */
export function formatUnits(units: number, signed = false): string {
	const text = formatQuantity(Math.abs(units));
	if (!signed || units === 0) return text;
	return `${units < 0 ? '-' : '+'}${text}`;
}

// ── Coverage ────────────────────────────────────────────────────────────

export type CoverageState = 'complete' | 'partial' | 'reading';

/** How well the readings cover the session, in one plain line. A session
 * still being read has no closing reading yet, which is not a gap. */
export function describeCoverage(
	coverage: MeasuredCoverage,
	sealed: boolean,
): { state: CoverageState; text: string } {
	if (sealed && coverage.level === 'complete') {
		return { state: 'complete', text: 'Complete: the readings cover the whole session.' };
	}
	const missedStart = coverage.opening === 'unread';
	const missedEnd = sealed && coverage.closing === 'unread';
	const missedPart = coverage.gapCount > 0;
	const still = sealed ? '' : ' Still being read.';
	if (missedStart && missedEnd) {
		return {
			state: 'partial',
			text: `Partial: the readings missed the start and the end of this session.${still}`,
		};
	}
	if (missedStart) {
		return {
			state: 'partial',
			text: `Partial: the readings missed the start of this session.${still}`,
		};
	}
	if (missedEnd) {
		return {
			state: 'partial',
			text: `Partial: the readings missed the end of this session.${still}`,
		};
	}
	if (missedPart || sealed) {
		return { state: 'partial', text: `Partial: the readings missed part of this session.${still}` };
	}
	return {
		state: 'reading',
		text: 'Being read: these figures grow until the session ends and its closing reading is in.',
	};
}
