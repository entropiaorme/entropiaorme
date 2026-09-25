/** Protection cost recording: limited sets, recording candidates, the two recordings, and undo. */

import type { ProtectionSetUpdateInput } from './commands.gen';
import * as commands from './commands.gen';

export type {
	ProtectionBacklog,
	ProtectionCandidateSession,
	ProtectionContextShare,
	ProtectionCostAllocation,
	ProtectionCostKind,
	ProtectionCostStatus,
	ProtectionCostWindow,
	ProtectionObservation,
	ProtectionObservationInput,
	ProtectionObservationOutcome,
	ProtectionObservationSource,
	ProtectionOverview,
	ProtectionRecordingCandidates,
	ProtectionRepairInput,
	ProtectionRepairOutcome,
	ProtectionScanResult,
	ProtectionSessionStatus,
	ProtectionSet,
	ProtectionSetInput,
	ProtectionSetKind,
	ProtectionSetUpdateInput,
	ProtectionStream,
	ProtectionUndoTarget,
	UnrecordedProtection,
} from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits after any protection
 * write commits (the colon form of the `protection.updated` wire topic): a
 * pure trigger for surfaces showing armour costs to re-read them.
 */
export const PROTECTION_TOPIC = 'protection:updated';

export const getProtectionOverview = commands.protectionOverview;
export const createProtectionSet = commands.protectionSetCreate;
export const updateProtectionSet = (id: string, input: ProtectionSetUpdateInput) =>
	commands.protectionSetUpdate(Number(id), input);
export const archiveProtectionSet = (id: string) => commands.protectionSetArchive(Number(id));
export const restoreProtectionSet = (id: string) => commands.protectionSetRestore(Number(id));
/** Undo the latest recording of one stream, handing its cost back to its sessions. */
export const undoProtectionRecording = commands.protectionUndo;
/** Which of the given sessions still have hits no armour recording covers. */
export const getUnrecordedArmourSessions = commands.protectionUnrecordedSessions;
/** One session's protection standing: its hits no recording covers yet. */
export const getProtectionSessionStatus = commands.protectionSessionStatus;
/** The sessions a recording of one stream would be spread over. */
export const getProtectionRecordingCandidates = commands.protectionRecordingCandidates;
export const confirmProtectionObservation = commands.protectionObservationConfirm;
export const confirmProtectionRepair = commands.protectionRepairConfirm;
export const scanTradeTerminalValue = commands.protectionTradeTerminalScan;
