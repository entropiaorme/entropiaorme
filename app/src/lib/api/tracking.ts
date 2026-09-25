/**
 * The tracking family: session lifecycle, the consolidated snapshot,
 * session reads and post-hoc edits, and mob locking
 * flow. Thin wrappers over the generated typed commands; the session
 * and snapshot reads swap onto the parallel `demo_*` commands while the
 * guide is active (see `./guide`).
 */

import type {
	ActivitySummary,
	HarvestGuardrailAlert,
	HealingStatus,
	NotableEventCategory,
	NotableEventType,
	RecentEvent,
	ToolActivity,
	TrackingSnapshot,
	TrackingState,
	TrifectaAttribution,
	Warning,
	WeaponAttribution,
} from './commands.gen';
import * as commands from './commands.gen';
import { guideSwapped } from './guide';

/** The snapshot shape the status-flavoured consumers (the stat
 * registry, the overlay pills) render from. */
export type TrackingStatus = TrackingSnapshot;

/** The overlay strip's camelCase render shape, mapped field by field
 * from the consolidated snapshot (`applySnapshot` in the overlay
 * route). Frontend-owned: this is a view model, not a wire shape. */
export interface TrackingLive {
	status: TrackingState;
	sessionId?: string | null;
	elapsed?: number | null;
	killCount?: number | null;
	kills?: number | null;
	cost?: number | null;
	returns?: number | null;
	pes?: number | null;
	net?: number | null;
	returnRate?: number | null;
	weaponAttribution?: WeaponAttribution | null;
	repairOcrEnabled?: boolean | null;
	sessionName?: string | null;
	/** The selected session definition (stringified id): the active
	 * session's stamped reference when tracking, the configured
	 * selection when idle. */
	sessionDefinitionId?: string | null;
	trackProtectionCosts?: boolean | null;
	skillBoostPercent?: number | null;
	currentMob?: string | null;
	currentTool?: string | null;
	currentToolKind?: 'weapon' | 'healing' | 'consumable' | 'harvesting' | null;
	/** What the held tool implies the next action records as. */
	currentActivity?: ToolActivity | null;
	/** The Activities control's strip-level readout: whether the control
	 * appears at all, the ready cue, and the standing set. Carried on
	 * every frame, idle included, over the session a start would run as;
	 * the standing set is necessarily empty until one does. */
	activities?: ActivitySummary | null;
	trifectaAttribution?: TrifectaAttribution | null;
	harvestGuardrail?: HarvestGuardrailAlert | null;
	healing?: HealingStatus | null;
	warnings?: Warning[] | null;
	recentEvents?: {
		type: NotableEventCategory;
		eventType?: NotableEventType;
		description: string;
		value: number;
		timestamp?: string | number;
	}[];
}

export type { HarvestGuardrailAlert, RecentEvent, TrackingSnapshot };

export const startTracking = commands.trackingStart;
export const stopTracking = commands.trackingStop;
export const releaseMob = commands.trackingReleaseMob;
export const deleteSession = commands.trackingSessionDelete;
export const deactivateLootItem = commands.trackingLootItemDeactivate;
export const activateLootItem = commands.trackingLootItemActivate;
/** Re-file an ended session under a different (active) definition: the
 * correction for a session recorded against whichever one the picker
 * happened to be holding. The stamped name always follows the move: it
 * is a copy of the definition's name, not a label of its own. */
export async function reassignSession(sessionId: string, definitionId: string) {
	return commands.trackingReassignSession(sessionId, Number(definitionId));
}
export const renameSessionMob = commands.trackingRenameMob;
export const restoreSessionMob = commands.trackingRestoreMob;
/** Set the session facets (full-state apply: null clears a facet). The
 * name is fixed while a session runs (the backend answers 409 on an
 * attempted change): it is a stamp of the definition's name, so a
 * mis-recorded session is corrected by re-filing it, not by retyping.
 * The boost stays editable throughout. */
export const setSessionConfig = commands.trackingSessionConfig;
/** Select the session definition the next session starts as an instance
 * of; writes the session-name facet with the definition's name in the
 * same motion. Fixed while a session runs (the backend answers 409 on an
 * attempted change). A null id is the wire's "nothing chosen", which the
 * backend reads back as the protected default rather than as no session;
 * the app never sends one. */
export const selectDefinition = commands.trackingDefinitionSelect;
export const scanRepairCost = commands.trackingRepairScan;
export const saveArmourCost = commands.trackingArmourCost;
/** What the Activities control offers right now: the session
 * definition's roster resolved against what is actually in play, plus
 * the facts nobody rostered. Absent (`visible: false`) when the control
 * has nothing to offer. */
export const getActivityOptions = commands.trackingActivityOptions;
/** Declare an activity: the one-tap switch (exclusive across quests AND
 * segments) unless additive, which co-activates instead. A segment needs
 * a name (a blank one is refused); the name joins the session's own
 * activities only where that session opted into naming them as it
 * plays, and is simply recorded otherwise. */
export const activateActivity = commands.trackingActivityActivate;
/** End one standing activity, leaving the others running. Idempotent. */
export const deactivateActivity = commands.trackingActivityDeactivate;

const readSessionsPage = guideSwapped(commands.trackingSessions, commands.demoTrackingSessions);

/** One keyset page of sessions plus the cursor for the next page (null on
 * the last page). `definitionId` narrows the page to one definition's
 * instances, which is how the review surface reads one; omitted, the
 * page is the whole history. */
export async function getTrackingSessions(cursor?: string, limit?: number, definitionId?: string) {
	return readSessionsPage(
		cursor ?? null,
		limit ?? null,
		definitionId === undefined ? null : Number(definitionId),
	);
}
export const getSessionDetail = guideSwapped(
	commands.trackingSessionDetail,
	commands.demoTrackingSessionDetail,
);
export const getTrackingSnapshot = guideSwapped(
	commands.trackingSnapshot,
	commands.demoTrackingSnapshot,
);

export async function getManualMobSuggestions(query: string) {
	if (!query.trim()) return [];
	return commands.trackingManualMobSuggestions(query.trim(), null);
}

export async function lockManualMob(species: string, maturity = '') {
	return commands.trackingManualMobLock(species, maturity);
}
