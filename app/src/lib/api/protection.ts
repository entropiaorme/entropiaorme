/** Protection cost recording: limited sets, recording candidates, and the two recordings. */

import type { ProtectionSetUpdateInput } from './commands.gen';
import * as commands from './commands.gen';

export type {
	ProtectionCandidateSession,
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
	UnrecordedProtection,
} from './commands.gen';

export const getProtectionOverview = commands.protectionOverview;
export const createProtectionSet = commands.protectionSetCreate;
export const updateProtectionSet = (id: string, input: ProtectionSetUpdateInput) =>
	commands.protectionSetUpdate(Number(id), input);
export const archiveProtectionSet = (id: string) => commands.protectionSetArchive(Number(id));
/** One session's protection standing: its hits no recording covers yet. */
export const getProtectionSessionStatus = commands.protectionSessionStatus;
/** The sessions a recording of one stream would be spread over. */
export const getProtectionRecordingCandidates = commands.protectionRecordingCandidates;
export const confirmProtectionObservation = commands.protectionObservationConfirm;
export const confirmProtectionRepair = commands.protectionRepairConfirm;
export const scanTradeTerminalValue = commands.protectionTradeTerminalScan;
