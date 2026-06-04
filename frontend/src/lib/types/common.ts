/** Shared types used across multiple pages */

/** PED amounts are always numbers (e.g., 142.31 = 142.31 PED) */
export type Ped = number;

/** PEC amounts (1/100 PED) */
export type Pec = number;

/** PES (Project Entropia Skill) amounts — non-liquid skill-progress
 * denomination, distinct from PED. Stays out of liquid P&L by design. */
export type Pes = number;

/** ISO 8601 date string */
export type ISODate = string;

/** Duration in seconds */
export type Seconds = number;

/** Percentage as a decimal (0.95 = 95%) */
export type Ratio = number;

/** Trend direction for stat displays */
export type Trend = 'improving' | 'declining' | 'stable';

/** Cooldown state for quests */
export type CooldownStatus = 'ready' | 'cooling' | 'no_cooldown';

/** Broad notable event families used for styling */
export type NotableEventCategory = 'global' | 'hof' | 'quest' | 'warning';

/** Canonical notable event subtypes stored by the backend */
export type NotableEventType =
	| 'global_kill'
	| 'global_item'
	| 'hof_kill'
	| 'hof_item'
	| 'quest_started'
	| 'quest_completed'
	| 'quest_completed_pes';
