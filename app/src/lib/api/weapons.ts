/** Weapon attribution: the live decision on a standing mismatch, and the post-play review of a session's stored shots with the assignment of an unpriced shot and its undo. */

import * as commands from './commands.gen';

export type {
	WeaponAttributionSummary,
	WeaponCorrectionWeapon,
	WeaponGuardrailAlert,
	WeaponMismatchDecision,
	WeaponReviewDecision,
	WeaponReviewRow,
	WeaponShot,
	WeaponShotCandidate,
	WeaponShotGroup,
	WeaponShotPage,
} from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits after any weapon
 * assignment commits (the colon form of the `weapons.updated` wire topic): a
 * pure trigger for surfaces showing weapon costs to re-read them.
 */
export const WEAPONS_TOPIC = 'weapons:updated';

/** Decide the running session's standing weapon mismatch: confirm the weapon the damage names, or keep the hotbar's. False when none stood. */
export const decideWeaponMismatch = commands.trackingWeaponDecide;
/** One page of a session's stored shots of one group, oldest first. */
export const getWeaponShots = commands.weaponShots;
/** Which of these sessions still hold an unpriced shot, in the given order. */
export const getUnpricedShotSessions = commands.weaponUnpricedSessions;
/** The weapons an unpriced shot could be assigned to, fitting ones first. */
export const getWeaponCorrectionWeapons = commands.weaponCorrectionWeapons;
/** Assign an ended session's unpriced shot to a weapon; answers with its refreshed detail. */
export const assignWeaponShot = commands.weaponAssign;
/** Undo a live assignment; answers with the session's refreshed detail. */
export const undoWeaponAssignment = commands.weaponAssignmentUndo;
