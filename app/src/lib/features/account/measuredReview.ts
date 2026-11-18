/**
 * The review of measured sessions. Where the readings cover a session, its
 * measured figures are its costs; here each session's tracked estimates
 * are compared with them, and the same pairs summed over the whole
 * history, as the backend sums them: every session whose readings cover
 * it whole and that counts in the PED stats, each pair over the sessions
 * where both of its figures are known.
 */

import type {
	AccountMeasuredSummary,
	MeasuredAmount,
	MeasuredLeftOutReason,
	MeasuredSessionPage,
	MeasuredSessionRow,
} from '$lib/api/commands.gen';
import { listNames } from '$lib/utils/format';
import {
	CATEGORY_LABELS,
	type Category,
	type Difference,
	difference,
	type MeasuredFigure,
	measuredByCategory,
	PAIRED,
	type PairedCategory,
	shareOfTracked,
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

// ── The whole-history summary ───────────────────────────────────────────

/** One category's pair summed over every session it counts. */
export interface SummaryPairView {
	category: PairedCategory;
	label: string;
	/** How many sessions went into the pair. */
	sessions: number;
	/** `null` when no session went in. */
	tracked: number | null;
	measured: MeasuredAmount | null;
	difference: Difference | null;
	/** The difference as a share of the tracked sum; null when that is
	 * nothing or there is no difference. */
	share: number | null;
}

/** The four pairs of the summary, in order. */
export function summaryPairs(summary: AccountMeasuredSummary): SummaryPairView[] {
	return PAIRED.map((category) => {
		const pair = summary[category];
		if (pair.sessions === 0) {
			return {
				category,
				label: CATEGORY_LABELS[category],
				sessions: 0,
				tracked: null,
				measured: null,
				difference: null,
				share: null,
			};
		}
		const delta = difference(pair.tracked, { kind: 'amount', amount: pair.measured });
		return {
			category,
			label: CATEGORY_LABELS[category],
			sessions: pair.sessions,
			tracked: pair.tracked,
			measured: pair.measured,
			difference: delta,
			share: delta ? shareOfTracked(delta.amount, pair.tracked) : null,
		};
	});
}

function sessions(count: number): string {
	return `${count} ${count === 1 ? 'session' : 'sessions'}`;
}

/** Which sessions the summary covers, in one sentence. */
export function includedSentence(summary: AccountMeasuredSummary): string {
	if (summary.included === 0) {
		return 'No session with readings is complete and in the PED stats yet, so there is nothing to sum.';
	}
	return `Summed over ${sessions(summary.included)} with complete readings in the PED stats.`;
}

const LEFT_OUT: Record<MeasuredLeftOutReason, string> = {
	partialCoverage: 'with partial readings',
	excluded: 'left out of the PED stats',
	notSealed: 'whose closing readings are not in yet',
};

/** How many sessions with readings the summary leaves out, and why; null
 * when it leaves none out. */
export function leftOutSentence(summary: AccountMeasuredSummary): string | null {
	const reasons = summary.leftOut.filter((entry) => entry.sessions > 0);
	const total = reasons.reduce((sum, entry) => sum + entry.sessions, 0);
	if (total === 0) return null;
	const parts = reasons.map((entry) => `${entry.sessions} ${LEFT_OUT[entry.reason]}`);
	return `Not in these figures: ${sessions(total)} with readings (${listNames(parts)}).`;
}

/** What the list says when the filter leaves nothing to show. */
export function noCompleteSentence(loaded: number, hasOlder: boolean): string {
	if (loaded === 1 && !hasOlder) return 'The one session with readings is not complete yet.';
	return hasOlder
		? `None of the newest ${sessions(loaded)} with readings is complete yet.`
		: `None of the ${sessions(loaded)} with readings is complete yet.`;
}

// ── The loaded window ───────────────────────────────────────────────────

function olderThan(row: MeasuredSessionRow, last: MeasuredSessionRow): boolean {
	const a = Date.parse(row.startTime);
	const b = Date.parse(last.startTime);
	return a < b || (a === b && row.id < last.id);
}

/**
 * Fold a fresh read of the newest page into the loaded window. The newest
 * page is the truth for every session it spans, so a session deleted since
 * the last read leaves the list; loaded rows older than the page's last
 * row stay, with the cursor that continues after them. When the newest
 * page holds every session, it is the whole list.
 */
export function foldNewest(
	loaded: readonly MeasuredSessionRow[],
	cursor: string | null,
	newest: MeasuredSessionPage,
): { rows: MeasuredSessionRow[]; cursor: string | null } {
	const last = newest.sessions.at(-1);
	if (newest.nextCursor === null || !last) {
		return { rows: [...newest.sessions], cursor: newest.nextCursor };
	}
	const fresh = new Set(newest.sessions.map((row) => row.id));
	const kept = loaded.filter((row) => !fresh.has(row.id) && olderThan(row, last));
	return {
		rows: [...newest.sessions, ...kept],
		cursor: kept.length > 0 ? cursor : newest.nextCursor,
	};
}

/** Append an older page to the loaded window, once each. */
export function appendOlder(
	loaded: readonly MeasuredSessionRow[],
	page: MeasuredSessionPage,
): MeasuredSessionRow[] {
	const known = new Set(loaded.map((row) => row.id));
	return [...loaded, ...page.sessions.filter((row) => !known.has(row.id))];
}
