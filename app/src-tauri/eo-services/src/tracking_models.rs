//! In-memory data models for tracking sessions, kills, and combat.
//! The owner stamps every
//! instant explicitly through its injected clock, so constructing a
//! session can never read ambient time; the readout views are owned
//! detached values, never references into live tracker state.

use chrono::{DateTime, Utc};

use crate::expected_hunting::OffensiveLoadoutEvidence;
use crate::harvest_yield::{HarvestYieldSource, HarvestYieldTier};
use crate::ped::Ped;
use crate::tracker::ActiveActivity;

/// A single item received from a loot drop. Serialises to its wire
/// field names directly: the loot-group bus payload carries these
/// items verbatim (see `bus_events`), so the serde shape is part of
/// the event-stream fingerprint contract.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LootItem {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
    pub is_enhancer_shrapnel: bool,
}

/// Per-tool damage statistics within a kill.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolStats {
    pub tool_name: String,
    pub shots_fired: i64,
    pub damage_dealt: f64,
    pub critical_hits: i64,
    /// From the equipment library.
    pub cost_per_shot: Ped,
    /// Immutable, model-neutral offensive evidence captured for this phase.
    pub expected_economics: Option<OffensiveLoadoutEvidence>,
}

impl ToolStats {
    pub fn new(
        tool_name: &str,
        cost_per_shot: Ped,
        expected_economics: Option<OffensiveLoadoutEvidence>,
    ) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            shots_fired: 0,
            damage_dealt: 0.0,
            critical_hits: 0,
            cost_per_shot,
            expected_economics,
        }
    }
}

/// A single kill: one loot group with its accumulated combat stats,
/// created when a loot group arrives. The accumulated shots and cost
/// since the previous kill (or session start) snapshot into this
/// record; the mob identity stamps from the session's declared mob.
#[derive(Debug, Clone, PartialEq)]
pub struct Kill {
    pub id: String,
    /// Stable identity of the raw chat-log clump that created this row.
    /// None for records written outside the live watcher path.
    pub loot_source_id: Option<String>,
    pub session_id: String,
    /// "Unknown" when no declared mob is in force.
    pub mob_name: String,
    pub mob_species: String,
    pub mob_maturity: String,
    /// Where the mob stamp came from; None when the kill recorded with
    /// no declaration in force (the stamp is "Unknown").
    pub mob_stamp_source: Option<crate::tracker::MobStampSource>,
    /// Epoch seconds (UTC, fractional seconds preserved): when the
    /// loot arrived. The representation deliberately differs from the
    /// session's calendar fields below because the original carries
    /// exactly this split: kill timestamps flow straight into the
    /// database's numeric column, while session instants convert at
    /// the persistence boundary.
    pub timestamp: f64,
    pub shots_fired: i64,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    pub critical_hits: i64,
    /// Total weapon cost (cost per shot times shots, summed per tool).
    pub cost_ped: Ped,
    /// Enhancer cost accumulated during this kill's shots.
    pub enhancer_cost: Ped,
    pub loot_total_ped: Ped,
    pub loot_items: Vec<LootItem>,
    /// Per-tool tracking in first-seen order, keyed by the phase key
    /// (the bare tool name, then `name#2`... when a cost change opens
    /// a new phase of the same tool).
    pub tool_stats: Vec<(String, ToolStats)>,
    pub is_global: bool,
    pub is_hof: bool,
    /// The session context in force when this was recorded: the set of
    /// intervals (a declared quest stretch, a modifier declaration) it
    /// belongs to. None when the row predates the interval model,
    /// never "nothing was in force".
    pub context_id: Option<i64>,
}

/// One harvesting swing (tree cutting): a successful swing arrives as
/// a wood loot group, a failed swing as the explicit harvest-fail
/// line, so every swing is directly countable. The tool identity and
/// per-swing cost are captured at swing time (immune to later
/// equipment edits); `tool_name` is None when no harvesting tool was
/// known (the swing recorded at zero cost with a session warning).
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestEvent {
    pub id: String,
    /// Stable identity of the raw chat-log clump that created this row.
    /// Failed swings and legacy/test records carry none.
    pub loot_source_id: Option<String>,
    pub session_id: String,
    /// Epoch seconds (UTC), same representation as `Kill::timestamp`.
    pub timestamp: f64,
    pub success: bool,
    pub tool_name: Option<String>,
    /// Effective board-yield activity, independent of the tool.
    pub yield_tier: HarvestYieldTier,
    /// None only when the tier remains genuinely unknown.
    pub yield_tier_source: Option<HarvestYieldSource>,
    pub cost_ped: Ped,
    pub loot_total_ped: Ped,
    pub loot_items: Vec<LootItem>,
    /// The session context in force when this was recorded: the set of
    /// intervals (a declared quest stretch, a modifier declaration) it
    /// belongs to. None when the row predates the interval model,
    /// never "nothing was in force".
    pub context_id: Option<i64>,
}

/// A tracking session, started and stopped by the user. The instants
/// are UTC; wall-clock renderings happen at the read surfaces.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingSession {
    pub id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub kills: Vec<Kill>,
    pub harvests: Vec<HarvestEvent>,
    /// Unresolved shots at session end.
    pub dangling_cost: Ped,
}

/// Immutable view of the active-session readout: computed under the
/// tracker's ownership and returned detached, so a caller on the web
/// thread never sees the live kill list mid-mutation.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSessionView {
    pub session_id: String,
    pub started_at: String,
    pub kill_count: i64,
    pub elapsed: i64,
    pub cost: f64,
    pub returns: f64,
    pub pes: f64,
    pub net: f64,
    pub return_rate: f64,
    pub damage_dealt_total: f64,
    pub weapon_damage_dealt: f64,
    pub weapon_cost: f64,
    /// Community-model offensive-only expected TT return over captured
    /// weapon and amplifier evidence. Null until a modelled shot exists.
    pub expected_tt_rate: Option<f64>,
    pub expected_return_coverage: Option<f64>,
    pub expected_return_model: Option<String>,
    pub shots_fired_total: i64,
    pub critical_hits_total: i64,
    pub max_damage: f64,
    pub globals_count: i64,
    pub hofs_count: i64,
    pub latest_kill_loot: Option<f64>,
    pub multiplier_last: Option<f64>,
    pub multiplier_avg: Option<f64>,
    pub multiplier_max: Option<f64>,
    pub multiplier_history: Vec<f64>,
    pub cumulative_net_history: Vec<f64>,
    pub current_mob: Option<String>,
    /// The session's designated name facet, when one was set.
    pub session_name: Option<String>,
    /// The session definition this session is an instance of, as
    /// stamped at start; None for a session outside any definition.
    pub definition_id: Option<i64>,
    pub track_protection_costs: bool,
    /// The skill-boost facet the session runs under (percent), when set.
    pub skill_boost_percent: Option<i64>,
    /// The activities standing on the session, in the order they were
    /// declared: the quest stretches and player-named segments the
    /// Activities control renders as chips. Empty whenever nothing is
    /// declared; an activity exists only while its session runs.
    pub active_activities: Vec<ActiveActivity>,
    /// Harvesting swings this session (successes + explicit fails).
    pub harvest_swings: i64,
    pub harvest_successes: i64,
    pub harvest_loot: f64,
    pub harvest_cost: f64,
    /// The standing harvest-guardrail disagreement, when the loot
    /// evidence last contradicted the hotbar-equipped tool.
    pub harvest_guardrail_mismatch: Option<HarvestGuardrailMismatchView>,
    pub healing: HealingRuntimeView,
    /// Raw rows (event_type, mob_or_item, value_ped, timestamp): the
    /// presentation mapping lives in the HTTP layer.
    pub notable_event_rows: Vec<(String, String, f64, Option<f64>)>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HealingRuntimeView {
    pub tool_name: Option<String>,
    pub state: String,
    pub cooldown_until: Option<f64>,
    pub effect_until: Option<f64>,
    pub activation_count: i64,
    pub direct_output_count: i64,
    pub effect_output_count: i64,
    pub passive_output_count: i64,
    pub unattributed_output_count: i64,
}

/// A harvest-guardrail disagreement as the read surfaces consume it:
/// the tool the loot evidence expects, the tool the hotbar believed
/// (None when none was equipped), the closed board-yield vocabulary
/// ("short" | "long" | "huge"), and when the evidence arrived. The field
/// name is the guardrail alias described on `TreeSize`.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestGuardrailMismatchView {
    pub expected_tool: String,
    pub observed_tool: Option<String>,
    pub tree_size: String,
    pub at_epoch: f64,
}

/// Immutable view of the whole tracking readout: `active` is the
/// session discriminator (None when no session runs); the detected
/// tool is meaningful in both states. The HTTP layer merges the
/// configuration-derived fields around this owned value.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReadout {
    pub current_tool: Option<String>,
    pub current_tool_kind: Option<crate::bus_events::HotbarItemKind>,
    pub active: Option<ActiveSessionView>,
}
