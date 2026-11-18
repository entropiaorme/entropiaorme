/**
 * What a session's cost figures rest on, in the player's words.
 *
 * Where the account readings measured a cost bucket, the session's figure
 * is the measured one; elsewhere it is the chat log's tracked estimate. A
 * session measured throughout needs no word per bucket, so it says so once;
 * a session that mixes the two says, beside each bucket, which it is; a
 * session the readings never covered reads exactly as before.
 *
 * Where a bucket's figure is measured and the tracked estimate differed by
 * more than a PEC, the estimate is kept as a note: it is what the bucket
 * read before measurement, and so what explains a step in a trend.
 */

import type {
	CostBasis,
	CostBreakdown,
	CostBreakdownBasis,
	CostReason,
} from '$lib/api/commands.gen';
import { withinPec } from './measure';

/** One of the session detail's cost buckets. */
export type CostBucket = keyof CostBreakdownBasis;

/** How each basis reads beside a bucket, and what it means. */
export const BASIS_MARKS: Record<CostBasis, { label: string; meaning: string }> = {
	measured: {
		label: 'from your account',
		meaning: 'Read from your account: what your items lost and the units used up in the session.',
	},
	tracked: {
		label: 'tracked',
		meaning:
			'Tracked by the app from your play and your recordings: the readings did not measure this cost.',
	},
	mixed: {
		label: 'partly from your account',
		meaning:
			'Part read from your account, part tracked by the app from your play and your recordings.',
	},
};

/** Why a bucket's figure is not what the readings measured, one plain
 * sentence per reason the app gives. */
export const COST_REASONS: Record<CostReason, string> = {
	noMeasuredCosts: "The readings have not measured this session's costs.",
	partialCoverage: 'The readings missed the start or the end of this session.',
	gap: 'Two of the readings in this session are too far apart to measure it.',
	ammoUnsettled:
		'How much of the ammo was fired, rather than paid out by strongboxes, could not be settled.',
	ammoToppedUp:
		'Universal Ammo was topped up during this session, so the ammo fired in the span it arrived in is the tracked figure, and when that span runs past your last kill, the whole weapon is.',
	limitedSetUnconfirmed:
		"A limited piece lost TT, and pricing it needs its set's pieces confirmed; any other armour the readings measured is in the figure.",
	trackedOnly: 'The readings never measure this cost.',
	playInGap:
		'The readings missed an end of this session across a gap in which the chat log shows other play.',
	chatLogMissesGap:
		'The readings missed an end of this session across a gap the chat log does not cover.',
	chatLogUnchecked:
		'The readings missed an end of this session across a gap the chat log could not be checked over.',
	gapUnaccounted:
		"The readings missed an end of this session across a gap that holds a repair, or a cost this session's play never had.",
	overheadInGap:
		'The readings missed an end of this session across a gap, and this was used up in that gap, possibly after the session, so it is not billed to it.',
};

/** A reason that says the session has readings at all: anything but a
 * session the readings never measured. */
function speaksOfReadings(reason: CostReason | null): reason is CostReason {
	return reason !== null && reason !== 'noMeasuredCosts';
}

/** The mark beside a bucket: in a session that mixes bases, whichever
 * basis the bucket rests on; elsewhere only on a bucket the readings did
 * not measure in a session that has readings, so its reason can be read. A
 * session resting on one basis throughout otherwise says so once, or not
 * at all. */
export function bucketMark(
	sessionBasis: CostBasis,
	bucketBasis: CostBasis,
	reason: CostReason | null = null,
): CostBasis | null {
	if (sessionBasis === 'mixed') return bucketBasis;
	return bucketBasis !== 'measured' && speaksOfReadings(reason) ? bucketBasis : null;
}

/** Whether the readings speak to any of the session's buckets: measured
 * them, or say why they could not. Such a session shows its split, so
 * each figure's basis can be read. */
export function readingsSpeak(breakdown: CostBreakdown): boolean {
	return (
		Object.values(breakdown.basis).some((basis) => basis !== 'tracked') ||
		Object.values(breakdown.reasons).some(speaksOfReadings)
	);
}

/** Why a bucket is not wholly measured, in a plain sentence; `null` for a
 * measured bucket, or where the readings never measured the session. */
export function bucketReason(breakdown: CostBreakdown, bucket: CostBucket): string | null {
	if (breakdown.basis[bucket] === 'measured') return null;
	const reason = breakdown.reasons[bucket];
	return speaksOfReadings(reason) ? COST_REASONS[reason] : null;
}

/** What the chat log estimated for a bucket the readings measured, in
 * whole or in part, when it differs from the figure shown by more than a
 * PEC; `null` otherwise. Armour whose hits no recording covers has no
 * tracked figure to set beside it. */
export function trackedEstimate(breakdown: CostBreakdown, bucket: CostBucket): number | null {
	if (breakdown.basis[bucket] === 'tracked') return null;
	if (bucket === 'armourCost' && breakdown.armourUnrecorded) return null;
	const estimate = breakdown.tracked[bucket];
	return withinPec(breakdown[bucket], estimate) ? null : estimate;
}

/** Whether a bucket is worth showing: it holds something, or the
 * readings measured it to a figure its tracked estimate differs from (a
 * measured nothing against an estimate of something says as much as any
 * figure). */
export function showsBucket(breakdown: CostBreakdown, bucket: CostBucket): boolean {
	return breakdown[bucket] > 0 || trackedEstimate(breakdown, bucket) !== null;
}

/** The one line a session measured throughout carries; `null` for any
 * other session. */
export function sessionBasisLine(sessionBasis: CostBasis): string | null {
	return sessionBasis === 'measured' ? 'All from your account' : null;
}

/** Whether the session's costs rest on the readings at all, so its cost
 * split is worth showing even when the weapon is its only cost. */
export function restsOnReadings(sessionBasis: CostBasis): boolean {
	return sessionBasis === 'measured' || sessionBasis === 'mixed';
}
