/** Weapon attribution: the live decision on a standing mismatch, and the post-play review of a session's stored shots with their corrections (assigning a shot left without a price to a weapon, marking an unresolved hit as an effect's tick) and the undo of either. */

import * as commands from './commands.gen';

export type {
	WeaponAttributionSummary,
	WeaponBulkAssignment,
	WeaponBulkOption,
	WeaponCorrectionBatch,
	WeaponCorrectionKind,
	WeaponCorrectionWeapon,
	WeaponEffectCandidate,
	WeaponEffectRow,
	WeaponGuardrailAlert,
	WeaponMismatchDecision,
	WeaponReviewDecision,
	WeaponReviewRow,
	WeaponShot,
	WeaponShotCandidate,
	WeaponShotGroup,
	WeaponShotPage,
	WeaponUnpricedExposure,
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
/** The sessions among these still holding an unpriced shot, in the given order, each with the most those shots could have cost. */
export const getUnpricedExposure = commands.weaponUnpricedExposure;
/** The weapons an ended session's unpriced shots could all be assigned to at once, the one fitting the most first. */
export const getWeaponBulkOptions = commands.weaponBulkOptions;
/** Assign every unpriced shot of an ended session the weapon fitted to it, as one batch; answers with what it did and the refreshed detail. */
export const assignFittingWeaponShots = commands.weaponAssignFitting;
/** Undo a bulk assignment's corrections still live; answers with the session's refreshed detail. */
export const undoWeaponBatch = commands.weaponBatchUndo;
/** The weapons an unpriced shot could be assigned to, fitting ones first. */
export const getWeaponCorrectionWeapons = commands.weaponCorrectionWeapons;
/** Assign an ended session's shot left without a price (an unresolved shot, or an effect tick) to a weapon; answers with its refreshed detail. */
export const assignWeaponShot = commands.weaponAssign;
/** Mark an ended session's unresolved hit as a tick of an effect open when it landed; answers with its refreshed detail. */
export const markWeaponShotEffectTick = commands.weaponMarkEffectTick;
/** Undo a live correction; answers with the session's refreshed detail. */
export const undoWeaponAssignment = commands.weaponAssignmentUndo;
