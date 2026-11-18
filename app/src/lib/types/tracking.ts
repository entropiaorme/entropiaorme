/**
 * Tracking-surface types. The wire shapes re-export from the generated
 * bindings (the authoritative contract); `LootItem` is the historical
 * consumer-facing name of the generated `LootEntry`.
 */

export type {
	CostBreakdown,
	LootEntry as LootItem,
	MobBreakdownRow,
	MobEntryMode,
	NotableEvent,
	SessionDetail,
	SessionExclusion,
	SkillGain,
	ToolStat,
	TrackingSession,
} from '$lib/api/commands.gen';
