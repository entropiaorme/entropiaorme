/**
 * What the account readings measured of a session, and how it compares
 * with what the session tracked.
 *
 * Where the readings cover a session's cost, the measured amount is the
 * session's cost; the app resolves that elsewhere and every surface shows
 * the result. What this module serves is the comparison the review of
 * measured sessions draws: each category pairs the chat log's tracked
 * estimate with the measured figures that answer it, and the difference
 * says how far the estimate was off. Nothing here forms a cost, a net or
 * a return of its own.
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
	TrackedSessionFigures,
} from '$lib/api/commands.gen';
import { formatPed, formatPercent, formatQuantity, NO_DATA } from '$lib/utils/format';

const MICRO_PER_PED = 1_000_000;
/** Within this much (a PEC), two figures read as agreeing. */
const NEGLIGIBLE_MICRO = 10_000;

function micro(ped: number): number {
	return Math.round(ped * MICRO_PER_PED);
}

/** Whether two figures agree to within a PEC. */
export function withinPec(a: number, b: number): boolean {
	return Math.abs(micro(a) - micro(b)) <= NEGLIGIBLE_MICRO;
}

function amountOf(microPed: number, precision: MeasuredPrecision): MeasuredAmount {
	return { ped: microPed / MICRO_PER_PED, precision };
}

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

/** A category with a tracked estimate to compare with. */
export type PairedCategory = Exclude<Category, 'other'>;

/** The categories with a tracked estimate to compare with, in order. */
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

/**
 * Armour cost is recorded at a repair, possibly sessions later. While a
 * session holds hits no recording covers yet, its tracked armour figure is
 * not known, whatever the cost booked so far: it is absent in the review,
 * and never summed.
 */

/** The tracked figures a measured session row carries. */
export function trackedFromRow(tracked: TrackedSessionFigures | null): TrackedFigures {
	if (!tracked) return NOTHING_TRACKED;
	return {
		weapon: tracked.weaponCost,
		healing: tracked.healingCost,
		armour: tracked.armourUnrecorded ? null : tracked.armourCost,
		loot: tracked.lootTt,
	};
}

/** A tracked estimate compared with a measured figure: to four decimals
 * when the measured figure is exact, so the row can be checked by eye; to
 * two otherwise, as the app shows tracked figures everywhere. */
export function formatTracked(tracked: number | null, precision: MeasuredPrecision): string {
	if (tracked === null) return NO_DATA;
	return formatMeasured({ ped: tracked, precision });
}

/** A measured figure: an amount, or the ammo burned that could not be
 * settled (so neither is the weapon figure it is part of). An amount that
 * leaves out the span the ammo was topped up in is `partial`: it is not the
 * whole session's, so no tracked figure is set against it. */
export type MeasuredFigure =
	| { kind: 'amount'; amount: MeasuredAmount; partial?: boolean }
	| { kind: 'unsettled' };

/** Which measured figures answer each tracked one. */
export function measuredByCategory(totals: MeasuredTotals): Record<Category, MeasuredFigure> {
	const amount = (value: MeasuredAmount): MeasuredFigure => ({ kind: 'amount', amount: value });
	return {
		weapon:
			totals.ammo.kind === 'unsettled'
				? { kind: 'unsettled' }
				: totals.ammo.kind === 'toppedUp'
					? {
							kind: 'amount',
							amount: sumAmounts([totals.ammo.burned, totals.weaponDecay]),
							partial: true,
						}
					: amount(sumAmounts([totals.ammo.burned, totals.weaponDecay])),
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
 * either is not known, or the measured one is not the whole session's. */
export function difference(tracked: number | null, measured: MeasuredFigure): Difference | null {
	if (tracked === null || measured.kind !== 'amount' || measured.partial) return null;
	const delta = micro(measured.amount.ped) - micro(tracked);
	return {
		amount: amountOf(delta, measured.amount.precision),
		negligible: withinPec(measured.amount.ped, tracked),
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

/** Whether a measure covers its whole session: sealed, and complete. */
export function coversWhole(measure: AccountSessionMeasure): boolean {
	return measure.sealed && measure.coverage.level === 'complete';
}

/** The comparison, one row per category in order. Other has no tracked
 * figure, so no difference; and a measure of part of a session is never
 * differenced against the whole session's tracked figures (`whole` false),
 * since part of a session against all of it says nothing of the estimate. */
export function comparisonRows(
	totals: MeasuredTotals,
	tracked: TrackedFigures,
	whole: boolean,
): ComparisonRow[] {
	const measured = measuredByCategory(totals);
	return CATEGORIES.map((category) => {
		const trackedFigure = category === 'other' ? null : tracked[category];
		return {
			category,
			label: CATEGORY_LABELS[category],
			tracked: trackedFigure,
			measured: measured[category],
			difference: whole ? difference(trackedFigure, measured[category]) : null,
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
	lines.push(`Weapon and amplifier wear ${formatMeasured(measure.totals.weaponDecay)} PED`);
	for (const used of measure.ammo) {
		if (used.split.kind !== 'unsettled') continue;
		lines.push(
			`${used.name}: net change ${formatUnits(used.netUnits, true)} units; the split between ammo fired and ammo paid out by strongboxes could not be settled`,
		);
	}
	return lines;
}

/** The weapon row's aside when the ammo was topped up mid-session: the
 * figure leaves out what was fired in that span. */
export function toppedUpAmmoAsides(measure: AccountSessionMeasure): string[] {
	return measure.totals.ammo.kind === 'toppedUp'
		? ['the ammo fired in the span it was topped up in is not measured']
		: [];
}

/** The loot row's "of which" lines: what arrived that may be a strongbox's
 * contents. */
export function lootAsides(measure: AccountSessionMeasure): string[] {
	const lines: string[] = [];
	if (!isNothing(measure.gainedPossiblyStrongbox)) {
		lines.push(
			`of which ${formatMeasured(measure.gainedPossiblyStrongbox)} PED may be a strongbox's contents`,
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

/**
 * How well the readings cover the session, in one plain line. A session
 * still being read has no closing reading yet, which is not a gap. `ended`
 * says whether the session itself has ended (`null` when the surface
 * cannot tell): an ended session whose closing readings are still being
 * taken is said to be just that.
 */
export function describeCoverage(
	coverage: MeasuredCoverage,
	sealed: boolean,
	ended: boolean | null,
	acrossGapMinutes: number | null = null,
): { state: CoverageState; text: string } {
	if (sealed && coverage.level === 'complete') {
		return { state: 'complete', text: 'Complete: the readings cover the whole session.' };
	}
	// An end the readings missed, read across a gap the chat log shows held
	// the session's own play alone: the costs were measured across it, so
	// the missed end is no shortfall and is never said to be one. The gap
	// itself is described once, with the costs.
	if (sealed && acrossGapMinutes !== null) {
		return {
			state: 'complete',
			text: 'Complete: measured across the gap noted with the costs.',
		};
	}
	const missedStart = coverage.opening === 'unread';
	const missedEnd = sealed && coverage.closing === 'unread';
	const missedPart = coverage.gapCount > 0;
	const still = sealed ? '' : ` ${stillReading(ended)}`;
	const partial = (what: string) => ({
		state: 'partial' as const,
		text: `Partial: the readings missed ${what} of this session.${still}`,
	});
	if (missedStart && missedEnd) return partial('the start and the end');
	if (missedStart) return partial('the start');
	if (missedEnd) return partial('the end');
	if (missedPart || sealed) return partial('part');
	return { state: 'reading', text: stillReading(ended) };
}

/** The line a session measured across a gap in its readings carries: how
 * long the gap was, and that the chat log shows no other play in it. */
export function acrossGapNote(minutes: number): string {
	const span = `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
	return `Measured across a gap of ${span}; the chat log shows no other play in it`;
}

function stillReading(ended: boolean | null): string {
	if (ended === true) return 'The session has ended; its closing readings are still being taken.';
	if (ended === false) return 'Still being read: the session is running.';
	return "Still being read: these figures may change until the session's closing readings are in.";
}
