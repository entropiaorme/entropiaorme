/**
 * The wording of a session's stats exclusion: which app-wide statistics a
 * recorded session is left out of. Two independent scopes: the PED side
 * (loot, costs, profit and loss) and the skill gains (PES). The session
 * itself keeps every figure; only the aggregates stop counting it.
 */

import type { SessionExclusion } from '$lib/types/tracking';

/** The state the exclusion write takes, as the two "counts" switches show it. */
export interface ExclusionState {
	economy: boolean;
	progression: boolean;
	note: string | null;
}

/** Whether the session's PED side still counts in the statistics. */
export function countsEconomy(exclusion: SessionExclusion | null): boolean {
	return exclusion?.economyExcludedAt == null;
}

/** Whether the session's skill gains still count in the statistics. */
export function countsProgression(exclusion: SessionExclusion | null): boolean {
	return exclusion?.progressionExcludedAt == null;
}

/** The write that flips one scope and keeps the other scope and the note. */
export function toggled(
	exclusion: SessionExclusion | null,
	scope: 'economy' | 'progression',
): ExclusionState {
	const economy = !countsEconomy(exclusion);
	const progression = !countsProgression(exclusion);
	return {
		economy: scope === 'economy' ? !economy : economy,
		progression: scope === 'progression' ? !progression : progression,
		note: exclusion?.note ?? null,
	};
}

/** The write that keeps both scopes and replaces the note (blank clears it). */
export function withNote(exclusion: SessionExclusion | null, note: string): ExclusionState {
	const trimmed = note.trim();
	return {
		economy: !countsEconomy(exclusion),
		progression: !countsProgression(exclusion),
		note: trimmed === '' ? null : trimmed,
	};
}

/** The write that counts the session everywhere again. */
export const INCLUDED: ExclusionState = { economy: false, progression: false, note: null };

/** What the session is left out of, as a short phrase ("PED stats"), or
 * null while it counts everywhere. */
export function excludedFrom(exclusion: SessionExclusion | null): string | null {
	const economy = !countsEconomy(exclusion);
	const progression = !countsProgression(exclusion);
	if (economy && progression) return 'PED and skill stats';
	if (economy) return 'PED stats';
	if (progression) return 'skill stats';
	return null;
}

/** The compact row mark, or null while the session counts everywhere. */
export function exclusionMark(exclusion: SessionExclusion | null): string | null {
	const economy = !countsEconomy(exclusion);
	const progression = !countsProgression(exclusion);
	if (economy && progression) return 'Not in stats';
	if (economy) return 'Not in PED stats';
	if (progression) return 'Not in skill stats';
	return null;
}

/** The earliest exclusion stamp still in force: when the session first
 * stopped counting somewhere. */
export function excludedSince(exclusion: SessionExclusion | null): string | null {
	const stamps = [exclusion?.economyExcludedAt, exclusion?.progressionExcludedAt].filter(
		(stamp): stamp is string => stamp != null,
	);
	if (stamps.length === 0) return null;
	return stamps.reduce((earliest, stamp) =>
		Date.parse(stamp) < Date.parse(earliest) ? stamp : earliest,
	);
}
