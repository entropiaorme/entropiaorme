/**
 * What the after-session review of detected creatures says, as pure
 * functions over the wire shapes: how sure each group is, in the player's
 * words, and what a settlement did.
 */

import type {
	MobCertainty,
	MobDetectionGrade,
	MobDetectionGroupRef,
	MobDetectionSettled,
} from '$lib/api';

/** How sure the screen was, in the player's words. */
export function describeGrade(grade: MobDetectionGrade): string {
	switch (grade) {
		case 'named':
			return 'Read clearly';
		case 'guess':
			return 'Probably';
		case 'provisional':
			return 'Not in the creature list';
		case 'provisional_guess':
			return 'Not in the creature list, probably';
		case 'latest_read':
		case 'provisional_latest_read':
			return 'Suggested: the last creature read before the kill';
		case 'continuity':
		case 'provisional_continuity':
			return 'From the kills before';
		case 'provisional_pinned':
			return 'Pinned, not in the creature list';
	}
}

/** Kills given the creature being hunted, where the screen could not settle
 * them: listed quietly, after the groups that ask for checking. */
export function isContinuity(grade: MobDetectionGrade): boolean {
	return grade === 'continuity' || grade === 'provisional_continuity';
}

/** The one quiet line a continuity group shows, with what its kills are
 * recorded as once that is another creature (a correction). */
export function continuityLine(count: number, creature?: string, current?: string | null): string {
	const line = `${killCount(count)} assigned on continuity`;
	return current && current !== creature ? `${line}; recorded as ${current}` : line;
}

/** A suggestion the kills did not take: they kept the declared mob. */
export function isSuggestion(grade: MobDetectionGrade): boolean {
	return grade === 'latest_read' || grade === 'provisional_latest_read';
}

/** A creature on no list, by the spelling the screen read. */
export function isUnlisted(grade: MobDetectionGrade): boolean {
	return (
		grade === 'provisional' ||
		grade === 'provisional_guess' ||
		grade === 'provisional_latest_read' ||
		grade === 'provisional_continuity' ||
		grade === 'provisional_pinned'
	);
}

/** A stable key for a group: the same creature at another grade, or an
 * unlisted name read at another level, is another group. */
export function groupKey(group: MobDetectionGroupRef): string {
	const { creature } = group;
	return [
		group.grade,
		creature.name,
		creature.species,
		creature.maturity,
		group.provisionalKey ?? '',
	].join('\u0000');
}

export function killCount(count: number): string {
	return `${count} ${count === 1 ? 'kill' : 'kills'}`;
}

/** What a settlement did, as one sentence. */
export function describeSettled(settled: MobDetectionSettled): string {
	switch (settled.kind) {
		case 'listed':
			return `Recorded as ${settled.creature.name}.`;
		case 'confirmed':
			return `Recorded as ${settled.creature.name}.`;
		case 'newName':
			return `Recorded as ${settled.creature.name}, and known by that name from now on.`;
		case 'declared':
			return 'Put back to the declared mob.';
	}
}

/** The longest name the backend keeps for a creature. */
export const MAX_NAME_LENGTH = 64;

/** What naming a group does, in its button: keeping an unlisted spelling
 * as a new creature is said as such, apart from confirming a listed one. */
export function nameAction(grade: MobDetectionGrade, draft: string, detected: string): string {
	if (draft.trim() !== detected) return 'Save';
	return isUnlisted(grade) ? 'Keep as a new creature' : 'Confirm';
}

/** How sure a mob attribution row is, where the session shows it; nothing
 * for a creature as recorded. */
export function describeCertainty(certainty: MobCertainty): string | null {
	switch (certainty) {
		case 'recorded':
			return null;
		case 'probable':
			return 'probably';
		case 'unlisted':
			return 'not in the creature list, unconfirmed';
		case 'unlisted_probable':
			return 'probably; not in the creature list, unconfirmed';
	}
}
