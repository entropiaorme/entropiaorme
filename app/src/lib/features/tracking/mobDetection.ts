/**
 * What the after-session review of detected mobs says, as pure
 * functions over the wire shapes: each mob's line in the player's words,
 * and what a settlement did.
 */

import type { MobCertainty, MobDetectionSettled, MobReviewRef, ReviewedMob } from '$lib/api';

/** A stable key for a line: a mob as its kills were recorded (an unlisted
 * name read at another level is another line), or the kills not named. */
export function mobKey(mob: MobReviewRef): string {
	if (mob.kind === 'unnamed') return 'unnamed';
	return ['mob', mob.species, mob.maturity, mob.provisionalKey ?? ''].join('\u0000');
}

/** The kills the screen could not name, as one line. */
export function isUnnamed(mob: ReviewedMob): boolean {
	return mob.mob.kind === 'unnamed';
}

/** A line that asks for the player: a mob on no list whose spelling is
 * still the screen's. Every other line is an account, a click from a
 * correction. */
export function needsAnswer(mob: ReviewedMob): boolean {
	return mob.unlisted && !mob.reviewed;
}

/** What a line is headed by. */
export function lineName(mob: ReviewedMob): string {
	return mob.name ?? 'Not named';
}

/** The quiet note beside a line's name: how many of its kills the screen
 * was less than sure of, that the mob is on no list, and what the kills are
 * recorded as once that is something else. */
export function lineNote(mob: ReviewedMob): string | null {
	const parts: string[] = [];
	if (isUnnamed(mob)) {
		if (mob.currentName) parts.push(`recorded as ${mob.currentName}`);
		return parts.join('; ') || null;
	}
	if (needsAnswer(mob)) parts.push('not in the mob list');
	if (mob.unsureCount > 0 && mob.unsureCount < mob.killCount) {
		parts.push(`${mob.unsureCount} less certain`);
	} else if (mob.unsureCount > 0) {
		parts.push('less certain');
	}
	if (mob.currentName && mob.currentName !== mob.name) {
		parts.push(`recorded as ${mob.currentName}`);
	}
	return parts.join('; ') || null;
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

/** The longest name the backend keeps for a mob. */
export const MAX_NAME_LENGTH = 64;

/** What naming a line does, in its button: keeping an unlisted spelling
 * as a new mob is said as such, apart from confirming a listed one. */
export function nameAction(mob: ReviewedMob, draft: string): string {
	if (isUnnamed(mob)) return 'Record';
	if (draft.trim() !== (mob.name ?? '')) return 'Save';
	return mob.unlisted ? 'Keep as a new mob' : 'Confirm';
}

/** How sure a mob attribution row is, where the session shows it; nothing
 * for a mob as recorded. */
export function describeCertainty(certainty: MobCertainty): string | null {
	switch (certainty) {
		case 'recorded':
			return null;
		case 'probable':
			return 'probably';
		case 'unlisted':
			return 'not in the mob list, unconfirmed';
		case 'unlisted_probable':
			return 'probably; not in the mob list, unconfirmed';
	}
}
