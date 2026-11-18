/**
 * The skill scan: it collects the Skills window's pages by itself while the
 * player pages through them in the game, then holds what it read for review.
 * Thin wrappers over the generated typed commands.
 */

import * as commands from './commands.gen';

export type {
	SkillScanHint,
	SkillScanRefusal,
	SkillScanReply,
	SkillScanReview,
	SkillScanStatus,
	SkillScanStop,
	SkillScanTotal,
} from './commands.gen';

export const getSkillScanStatus = commands.scanStatus;
export const startSkillScan = commands.scanStart;
export const cancelSkillScan = commands.scanCancel;
export const getSkillScanReview = commands.scanPending;

/** Save the review of scan `scan`; one whose figures do not add up is saved
 * only with `saveAnyway`. */
export const acceptSkillScan = (scan: number, saveAnyway = false) =>
	commands.scanAccept(scan, saveAnyway);

/** Discard the review of scan `scan`. */
export const rejectSkillScan = (scan: number) => commands.scanReject(scan);
