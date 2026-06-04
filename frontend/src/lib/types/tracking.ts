import type {
	Ped,
	Pes,
	Seconds,
	ISODate,
	Ratio,
	NotableEventCategory,
	NotableEventType,
} from './common';

/** A tracking session summary (shown in session history list) */
export interface TrackingSession {
	id: string;
	startTime: ISODate;
	endTime: ISODate | null;
	duration: Seconds;
	primaryMobs: string[];
	primaryWeapons: string[];
	cost: Ped;
	returns: Ped;
	net: Ped;
	returnRate: Ratio;
	globals: number;
	hofs: number;
}

export interface CostBreakdown {
	weaponCost: Ped;
	healCost: Ped;
	enhancerCost: Ped;
	armourCost: Ped;
}

/** Mob-attribution input mode the session was captured under.
 * Persisted at session start; never mutates afterwards. Drives label
 * vocabulary in post-hoc edit surfaces ('Mob Attribution' vs 'Tag
 * Attribution'); data semantics are identical between the two modes. */
export type MobEntryMode = 'mob' | 'tag';

/** Expanded session detail (inline expand from history row) */
export interface SessionDetail {
	sessionId: string;
	summary: {
		cost: Ped;
		returns: Ped;
		pes: Pes;
		net: Ped;
		returnRate: Ratio;
		kills: number;
		duration: Seconds;
		costBreakdown?: CostBreakdown;
	};
	mobEntryMode: MobEntryMode;
	notableEvents: NotableEvent[];
	/** Item-name aggregate of currently-active loot rows. */
	lootBreakdown: LootItem[];
	/** Item-name aggregate of currently-deactivated loot rows. Parallel
	 * to lootBreakdown; an item appearing in both means a partial-state
	 * cohort (some captures active, some deactivated). */
	deactivatedLootBreakdown: LootItem[];
	mobBreakdown: MobBreakdownRow[];
	effectiveLoot: Ped;
	toolStats: ToolStat[];
	skillGains: SkillGain[];
}

/** Per-mob row for the sessions-tab metadata-edit affordance.
 * `originalName` is populated when the mob has been renamed at least
 * once; the frontend renders an "originally X" indicator and offers a
 * Restore action that calls `restore-mob`.
 */
export interface MobBreakdownRow {
	currentName: string;
	originalName: string | null;
	killCount: number;
}

export interface NotableEvent {
	type: NotableEventCategory;
	eventType: NotableEventType;
	target: string;
	item: string;
	value: Ped;
}

export interface LootItem {
	name: string;
	quantity: number;
	ttValue: Ped;
}

export interface ToolStat {
	weaponName: string;
	shotsFired: number;
	damageDealt: number;
	crits: number;
	costAttributed: Ped;
}

export interface SkillGain {
	skillName: string;
	level: number;
	ttValueGained: Ped;
}
