/**
 * The pure half of recording an armour cost: which sessions a recording is
 * spread over, grouped the way the player thinks about them.
 *
 * A recording looks back to the previous one of its stream. Every session in
 * that look-back starts ticked; the player unticks a whole session type (the
 * usual case, since gear follows content) or a single session inside one. The
 * look-back can start later than the previous recording, which drops the
 * sessions before the new start without forgetting what was unticked among
 * them. Sessions an earlier recording already covered can be re-included one
 * by one, for the piece that was left unrepaired last time.
 */

import type { ProtectionCandidateSession } from '$lib/api';

/** The sessions of one session type, oldest first. */
export interface SessionGroup {
	key: string;
	label: string;
	sessions: ProtectionCandidateSession[];
}

export type GroupState = 'all' | 'some' | 'none';

/** What the player has chosen over one stream's candidates. */
export interface RecordingSelection {
	/** Index into the look-back sessions of the first one covered. */
	readonly start: number;
	/** Look-back sessions the player unticked. */
	readonly excluded: ReadonlySet<string>;
	/** Earlier, already-covered sessions the player ticked back in. */
	readonly reincluded: ReadonlySet<string>;
}

export const EMPTY_SELECTION: RecordingSelection = {
	start: 0,
	excluded: new Set(),
	reincluded: new Set(),
};

const UNTYPED_LABEL = 'Other sessions';

function groupIdentity(session: ProtectionCandidateSession): { key: string; label: string } {
	if (session.definitionId !== null) {
		return {
			key: `definition:${session.definitionId}`,
			label: session.definitionName ?? session.sessionName ?? UNTYPED_LABEL,
		};
	}
	if (session.sessionName) {
		return { key: `name:${session.sessionName}`, label: session.sessionName };
	}
	return { key: 'untyped', label: UNTYPED_LABEL };
}

/**
 * Group sessions by the session type they were played under, keeping the
 * order in which each type first appears so the list still reads oldest
 * first. A session without a type groups by its own name, then falls into
 * one "Other sessions" group.
 */
export function groupBySessionType(
	sessions: readonly ProtectionCandidateSession[],
): SessionGroup[] {
	const groups = new Map<string, SessionGroup>();
	for (const session of sessions) {
		const { key, label } = groupIdentity(session);
		const group = groups.get(key);
		if (group) group.sessions.push(session);
		else groups.set(key, { key, label, sessions: [session] });
	}
	return [...groups.values()];
}

/** The look-back sessions from the chosen start onward. */
export function lookBack(
	sessions: readonly ProtectionCandidateSession[],
	selection: RecordingSelection,
): ProtectionCandidateSession[] {
	return sessions.slice(Math.min(selection.start, Math.max(sessions.length - 1, 0)));
}

/** The sessions the recording will be spread over, oldest first. */
export function chosenSessions(
	sessions: readonly ProtectionCandidateSession[],
	earlier: readonly ProtectionCandidateSession[],
	selection: RecordingSelection,
): ProtectionCandidateSession[] {
	return [
		...earlier.filter((session) => selection.reincluded.has(session.sessionId)),
		...lookBack(sessions, selection).filter(
			(session) => !selection.excluded.has(session.sessionId),
		),
	];
}

export function groupState(group: SessionGroup, chosen: ReadonlySet<string>): GroupState {
	const ticked = group.sessions.filter((session) => chosen.has(session.sessionId)).length;
	if (ticked === 0) return 'none';
	return ticked === group.sessions.length ? 'all' : 'some';
}

/** Tick or untick one look-back session. */
export function toggleSession(
	selection: RecordingSelection,
	sessionId: string,
): RecordingSelection {
	const excluded = new Set(selection.excluded);
	if (excluded.has(sessionId)) excluded.delete(sessionId);
	else excluded.add(sessionId);
	return { ...selection, excluded };
}

/** Tick a whole look-back group, or untick it when it is fully ticked. */
export function toggleGroup(
	selection: RecordingSelection,
	group: SessionGroup,
	chosen: ReadonlySet<string>,
): RecordingSelection {
	const excluded = new Set(selection.excluded);
	const untick = groupState(group, chosen) === 'all';
	for (const session of group.sessions) {
		if (untick) excluded.add(session.sessionId);
		else excluded.delete(session.sessionId);
	}
	return { ...selection, excluded };
}

/** Tick or untick one earlier, already-covered session. */
export function toggleEarlier(
	selection: RecordingSelection,
	sessionId: string,
): RecordingSelection {
	const reincluded = new Set(selection.reincluded);
	if (reincluded.has(sessionId)) reincluded.delete(sessionId);
	else reincluded.add(sessionId);
	return { ...selection, reincluded };
}

/** Start the look-back at session `start` (an index into the look-back). */
export function startAt(selection: RecordingSelection, start: number): RecordingSelection {
	return { ...selection, start: Math.max(0, start) };
}

/** Hits the recording will be weighed by. */
export function totalHits(sessions: readonly ProtectionCandidateSession[]): number {
	return sessions.reduce((sum, session) => sum + session.hitCount, 0);
}

/** The share of `cost` hit-weighting gives `hits` out of `total`. */
export function projectedShare(cost: number, hits: number, total: number): number {
	return total > 0 ? (cost * hits) / total : 0;
}

/** A calendar day in the player's own locale, e.g. "12 Sep". */
export function formatDay(epochSeconds: number): string {
	return new Date(epochSeconds * 1000).toLocaleDateString(undefined, {
		day: 'numeric',
		month: 'short',
	});
}

/** A day and time for one session row, e.g. "12 Sep, 18:20". */
export function formatSessionStart(epochSeconds: number): string {
	return new Date(epochSeconds * 1000).toLocaleString(undefined, {
		day: 'numeric',
		month: 'short',
		hour: '2-digit',
		minute: '2-digit',
	});
}

/** Parse a typed PED amount, accepting a decimal comma. */
export function parseAmount(text: string): number | null {
	const trimmed = text.trim();
	if (!trimmed) return null;
	const parsed = Number(trimmed.replace(',', '.'));
	return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
}

/** Idempotency token for one recording attempt. */
export function recordingToken(): string {
	return (
		globalThis.crypto?.randomUUID?.() ??
		`armour-${Date.now()}-${Math.random().toString(36).slice(2)}`
	);
}
