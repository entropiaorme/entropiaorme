//! In-memory data models for tracking sessions, kills, and combat,
//! ported from the original Python implementation. The owner stamps every
//! instant explicitly through its injected clock, so constructing a
//! session can never read ambient time; the readout views are owned
//! detached values, never references into live tracker state.

use chrono::NaiveDateTime;

/// A single item received from a loot drop.
#[derive(Debug, Clone, PartialEq)]
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
    pub cost_per_shot: f64,
}

impl ToolStats {
    pub fn new(tool_name: &str, cost_per_shot: f64) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            shots_fired: 0,
            damage_dealt: 0.0,
            critical_hits: 0,
            cost_per_shot,
        }
    }
}

/// A single kill: one loot group with its accumulated combat stats,
/// created when a loot group arrives. The accumulated shots and cost
/// since the previous kill (or session start) snapshot into this
/// record; the mob name stamps from the manual or tag state.
#[derive(Debug, Clone, PartialEq)]
pub struct Kill {
    pub id: String,
    pub session_id: String,
    /// "Unknown" when no manual or tag state is set.
    pub mob_name: String,
    pub mob_species: String,
    pub mob_maturity: String,
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
    pub cost_ped: f64,
    /// Enhancer cost accumulated during this kill's shots.
    pub enhancer_cost: f64,
    pub loot_total_ped: f64,
    pub loot_items: Vec<LootItem>,
    /// Per-tool tracking in first-seen order, keyed by the phase key
    /// (the bare tool name, then `name#2`... when a cost change opens
    /// a new phase of the same tool).
    pub tool_stats: Vec<(String, ToolStats)>,
    pub is_global: bool,
    pub is_hof: bool,
}

/// A tracking session, started and stopped by the user.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingSession {
    pub id: String,
    pub start_time: NaiveDateTime,
    pub end_time: Option<NaiveDateTime>,
    pub kills: Vec<Kill>,
    /// Unresolved shots at session end.
    pub dangling_cost: f64,
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
    pub mob_source: Option<String>,
    pub mob_entry_mode: String,
    /// Raw rows (event_type, mob_or_item, value_ped, timestamp): the
    /// presentation mapping lives in the HTTP layer.
    pub notable_event_rows: Vec<(String, String, f64, Option<f64>)>,
    pub warnings: Vec<String>,
}

/// Immutable view of the whole tracking readout: `active` is the
/// session discriminator (None when no session runs); the detected
/// tool is meaningful in both states. The HTTP layer merges the
/// configuration-derived fields around this owned value.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReadout {
    pub current_tool: Option<String>,
    pub active: Option<ActiveSessionView>,
}
