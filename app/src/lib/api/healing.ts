/** Healing review: a session's healing outputs, the items one could be corrected to, and the two corrections with their undo. */

import * as commands from './commands.gen';

export type {
	HealingActivationProvenance,
	HealingActivationRow,
	HealingCorrectionKind,
	HealingCorrectionRef,
	HealingCorrectionTarget,
	HealingCorrectionTool,
	HealingOutput,
	HealingOutputClassification,
	HealingOutputPage,
	HealingSessionSummary,
} from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits after any healing
 * correction commits (the colon form of the `healing.updated` wire topic): a
 * pure trigger for surfaces showing heal costs to re-read them.
 */
export const HEALING_TOPIC = 'healing:updated';

/** One page of a session's healing outputs of one classification, oldest first. */
export const getHealingOutputs = commands.healingOutputs;
/** The healing items an output could be marked as a paid use of, fitting ones first. */
export const getHealingCorrectionTools = commands.healingCorrectionTools;
/** Correct an ended session's healing evidence; answers with its refreshed detail. */
export const correctHealing = commands.healingCorrect;
/** Undo a live healing correction; answers with the session's refreshed detail. */
export const undoHealingCorrection = commands.healingCorrectionUndo;
