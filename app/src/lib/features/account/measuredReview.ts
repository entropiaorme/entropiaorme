/**
 * The review of measured sessions: each session's tracked figures beside
 * what its readings measured, and the same pair summed over the sessions
 * the readings cover completely.
 *
 * The sum counts a session only when its readings are complete and its
 * PED side still counts in the statistics (a session left out of the PED
 * stats is left out here as everywhere else). A category counts a session
 * only when both of its figures are known there: a session without a
 * tracked summary, or whose ammo split could not be settled, would set one
 * side's sum against a different set of sessions than the other's.
 */

import type { MeasuredAmount, MeasuredSessionRow } from '$lib/api/commands.gen';
import { countsEconomy } from '$lib/features/sessions/exclusion';
import {
	CATEGORIES,
	type Category,
	type Difference,
	difference,
	type MeasuredFigure,
	measuredByCategory,
	NO_AMOUNT,
	shareOfTracked,
	sumAmounts,
	type TrackedFigures,
	trackedFromRow,
} from './measure';

/** A session's figures, by category. */
export interface SessionFigures {
	tracked: TrackedFigures;
	measured: Record<Category, MeasuredFigure>;
}

export function sessionFigures(row: MeasuredSessionRow): SessionFigures {
	return { tracked: trackedFromRow(row.tracked), measured: measuredByCategory(row.measured) };
}

/** Whether a row's readings cover the whole session. */
export function isComplete(row: MeasuredSessionRow): boolean {
	return row.coverage === 'complete';
}

/** The rows the list shows: complete ones only, or all of them. */
export function listedRows(
	rows: readonly MeasuredSessionRow[],
	completeOnly: boolean,
): MeasuredSessionRow[] {
	return completeOnly ? rows.filter(isComplete) : [...rows];
}

/** A row's coverage mark, or null for a complete one. */
export function coverageMark(row: MeasuredSessionRow): string | null {
	if (!row.sealed) return 'Being read';
	return isComplete(row) ? null : 'Partial';
}

/** How long the session ran, in seconds; null while it runs. */
export function sessionSeconds(row: MeasuredSessionRow): number | null {
	if (row.endTime === null) return null;
	const seconds = (Date.parse(row.endTime) - Date.parse(row.startTime)) / 1000;
	return Number.isFinite(seconds) && seconds >= 0 ? seconds : null;
}

/** One category's pair summed over the sessions it counts. */
export interface CategorySum {
	category: Category;
	/** How many sessions the sum covers. */
	sessions: number;
	/** `null` for Other, which has no tracked counterpart. */
	tracked: number | null;
	measured: MeasuredAmount;
	difference: Difference | null;
	/** The difference as a share of the tracked sum; null when that is
	 * nothing or there is none. */
	share: number | null;
}

export interface MeasuredSummary {
	/** The sessions the sum may count: complete, and in the PED stats. */
	sessions: number;
	categories: CategorySum[];
}

/** The pair summed per category over the listed complete sessions that
 * count in the PED stats. */
export function summarise(rows: readonly MeasuredSessionRow[]): MeasuredSummary {
	const counted = rows.filter((row) => isComplete(row) && countsEconomy(row.exclusion));
	const figures = counted.map(sessionFigures);
	const categories = CATEGORIES.map((category): CategorySum => {
		if (category === 'other') {
			const measured = sumAmounts(
				figures.flatMap((figure) =>
					figure.measured.other.kind === 'amount' ? [figure.measured.other.amount] : [],
				),
			);
			return {
				category,
				sessions: figures.length,
				tracked: null,
				measured,
				difference: null,
				share: null,
			};
		}
		let tracked = 0;
		const measured: MeasuredAmount[] = [];
		for (const figure of figures) {
			const trackedFigure = figure.tracked[category];
			const measuredFigure = figure.measured[category];
			if (trackedFigure === null || measuredFigure.kind !== 'amount') continue;
			tracked += trackedFigure;
			measured.push(measuredFigure.amount);
		}
		if (measured.length === 0) {
			return {
				category,
				sessions: 0,
				tracked: null,
				measured: NO_AMOUNT,
				difference: null,
				share: null,
			};
		}
		const total = sumAmounts(measured);
		const delta = difference(tracked, { kind: 'amount', amount: total });
		return {
			category,
			sessions: measured.length,
			tracked,
			measured: total,
			difference: delta,
			share: delta ? shareOfTracked(delta.amount, tracked) : null,
		};
	});
	return { sessions: counted.length, categories };
}
