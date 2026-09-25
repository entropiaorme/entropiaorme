//! The tracking family: the session-read surface (list, detail, tag /
//! manual-mob suggestions), the live producer
//! surface (start / stop / release-mob / manual-mob-lock / tag-lock / the
//! consolidated dashboard snapshot), the post-hoc session edits (rename /
//! restore mob, loot-item activate / deactivate, re-file under another
//! definition, armour cost, quest-link decision), and the one-shot
//! repair-cost OCR read.
//!
//! Typed DTOs over the composed services. The family's SQL and its wire
//! shaping live in `eo_services::tracking_reads`; this facade owns the
//! DTO definitions and orchestration, bridging each computed value into
//! its declared DTO, whose serde field order is the golden-pinned wire
//! order.
//!
//! Contract lineage (ADR-0019):
//!
//! * The **snapshot** and the **quest-link decision** were served
//!   `response_model_exclude_unset` (a field explicitly set to null stayed
//!   on the wire; an unset field was omitted). A single typed struct with
//!   `#[serde(skip_serializing_if = "Option::is_none")]` cannot tell a
//!   present-null from an absent key, so both collapse to "omitted": the
//!   projection narrows from **exclude-unset to exclude-none** (a
//!   present-null field is dropped rather than serialised `null`). The
//!   generated TypeScript type is all-optional and every consumer reads
//!   these fields defensively, so null and absent are equivalent to them.
//! * Structurally-impossible transport legs retire with no consumer: the
//!   `tainted` surrogate-500 (a Rust `String` argument is always valid
//!   UTF-8), the decoded-slash framework 404 (typed args carry the value
//!   directly), the 503 substrate-unavailable floor (every handle is
//!   present by construction), the ETag conditional-GET contract (a typed
//!   command answers a body, not a status + headers), and the body-taint /
//!   int-parse 422s (typed args are pre-validated).

use std::collections::HashMap;

use eo_services::config_service::{load_config_readonly, AppConfig};
use eo_services::db::Db;
use eo_services::mob_lookup_service::{python_whitespace, MobLookupService};
use eo_services::session_definitions::SessionDefinitionService;
use eo_services::time::{local_isoformat, naive_to_epoch};
use eo_services::tracker::{HuntTracker, TrackerCommandError};
use eo_services::tracking_models::ActiveSessionView;
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use eo_services::tracking_reads::*;

use crate::activities::ActivitySummary;
use crate::Nullable;
use crate::{Api, ApiError};

// ── Constants ───────────────────────────────────────────────────────

/// The `TrackingSnapshot` response-model field order (the polymorphic
/// dashboard hydration shape). The snake-case status trio sits among the
/// camelCase headline numbers exactly as the model declares them.
const SNAPSHOT_FIELDS: [&str; 50] = [
    "status",
    "hotbarListenerActive",
    "hotbarKeysEnabled",
    "repairOcrEnabled",
    "sessionName",
    "sessionDefinitionId",
    "trackProtectionCosts",
    "skillBoostPercent",
    "currentMob",
    "currentTool",
    "currentToolKind",
    "currentActivity",
    "activities",
    "lifetime",
    "recentEvents",
    "session_id",
    "started_at",
    "kill_count",
    "elapsed",
    "cost",
    "returns",
    "pes",
    "net",
    "returnRate",
    "damageDealtTotal",
    "weaponDamageDealt",
    "weaponCost",
    "expectedTtRate",
    "expectedReturnCoverage",
    "expectedReturnModel",
    "shotsFiredTotal",
    "criticalHitsTotal",
    "maxDamage",
    "globalsCount",
    "hofsCount",
    "latestKillLoot",
    "multiplierLast",
    "multiplierAvg",
    "multiplierMax",
    "multiplierHistory",
    "cumulativeNetHistory",
    "harvestSwings",
    "harvestSuccesses",
    "harvestLoot",
    "harvestCost",
    "harvestGuardrail",
    "weaponGuardrail",
    "unpricedShots",
    "healing",
    "warnings",
];

/// The repair-scan response-model field order (`exclude_unset`).
const REPAIR_FIELDS: [&str; 4] = ["cost_ped", "raw_text", "confidence", "error"];

/// A tracker-command precondition surfaced at the facade: a state
/// conflict ("No active session"), mapped to 409 with the error's own
/// message.
pub(crate) fn tracker_conflict(error: TrackerCommandError) -> ApiError {
    ApiError::conflict(error.to_string())
}

fn edit_error(context: &'static str) -> impl Fn(EditError) -> ApiError {
    move |error| match error {
        EditError::NotFound(message) => ApiError::not_found(message),
        EditError::Conflict(message) => ApiError::conflict(message),
        EditError::BadRequest(message) => ApiError::bad_request(message),
        EditError::Internal => ApiError::invalid_state(format!("{context} failed")),
    }
}

// ── Closed vocabularies ─────────────────────────────────────────────
//
// The string fields whose value sets are closed in code, stated as serde
// enums so the generated TypeScript carries the literal unions and the
// compiler owns the vocabulary on both sides of the boundary. Each
// variant's serialised form is byte-identical to the string it replaces.

/// The activity family the held tool implies the next action belongs
/// to: the overlay's derived-activity feedback ("this is Tree Cutting"
/// versus "this is Hunting"). Absent when no tool is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ToolActivity {
    Hunting,
    Treecutting,
}

/// The session state a tracking readout reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrackingState {
    Idle,
    Active,
}

/// The legacy exclusive-capture input mode recorded on pre-facet
/// session rows; read-only vocabulary for labelling those sessions
/// (facet-era rows all read as `mob`, the column default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MobEntryMode {
    Mob,
    Tag,
}

/// The broad notable-event family (drives styling on the frontend).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotableEventCategory {
    Global,
    Hof,
    Quest,
    Warning,
}

/// The canonical notable-event subtype the services store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotableEventType {
    GlobalKill,
    GlobalItem,
    HofKill,
    HofItem,
    QuestStarted,
    QuestCompleted,
    QuestCompletedPes,
}

// ── Response DTOs ───────────────────────────────────────────────────

/// One row of the session list.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackingSession {
    pub id: String,
    pub start_time: Nullable<String>,
    pub end_time: Nullable<String>,
    pub duration: i64,
    pub primary_mobs: Vec<String>,
    pub primary_weapons: Vec<String>,
    pub cost: f64,
    pub returns: f64,
    pub net: f64,
    pub return_rate: f64,
    pub globals: i64,
    pub hofs: i64,
}

/// A keyset page of session-list rows plus the opaque cursor for the
/// next page (`null` on the last page) and the whole-table session
/// count, mirroring the ledger's [`crate::analytics::LedgerPage`] shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    pub sessions: Vec<TrackingSession>,
    pub next_cursor: Nullable<String>,
    pub total: i64,
}

/// The session-detail cost split.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostBreakdown {
    pub weapon_cost: f64,
    pub heal_cost: f64,
    pub enhancer_cost: f64,
    pub armour_cost: f64,
    /// Harvesting (tree cutting) swing decay.
    pub harvest_cost: f64,
}

/// The session-detail headline summary.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub cost: f64,
    pub returns: f64,
    pub pes: f64,
    pub net: f64,
    pub return_rate: f64,
    pub kills: i64,
    pub duration: i64,
    pub cost_breakdown: CostBreakdown,
}

/// One notable event in a session detail.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotableEvent {
    #[serde(rename = "type")]
    pub kind: NotableEventCategory,
    pub event_type: NotableEventType,
    pub target: Nullable<String>,
    pub item: Nullable<String>,
    pub value: f64,
}

/// One aggregated loot line.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LootEntry {
    pub name: String,
    pub quantity: i64,
    pub tt_value: f64,
}

/// One per-mob breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobBreakdownRow {
    pub current_name: String,
    pub original_name: Nullable<String>,
    pub kill_count: i64,
}

/// One per-tool aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolStat {
    pub weapon_name: String,
    pub shots_fired: i64,
    pub damage_dealt: f64,
    pub crits: i64,
    pub cost_attributed: f64,
}

/// One per-skill gain (attributes excluded).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillGain {
    pub skill_name: String,
    pub level: f64,
    pub tt_value_gained: f64,
}

/// A session's harvesting (tree cutting) totals: every swing is a
/// counted event (successes arrive as wood loot groups, fails as the
/// explicit harvest-fail line).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestSummary {
    pub swings: i64,
    pub successes: i64,
    pub loot_tt: f64,
    pub cost: f64,
}

/// How a paid healing activation was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HealingActivationProvenance {
    /// A fresh healer intent met a compatible heal.
    Direct,
    /// A below-interval heal a health cap explains.
    HealthCapped,
    /// A heal reconciled with a hotbar press that arrived after it.
    Retrospective,
    /// The player marked the heal as a paid use.
    Corrected,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingActivationRow {
    pub id: String,
    pub tool_name: String,
    pub observed_at: f64,
    pub cost: f64,
    pub provenance: HealingActivationProvenance,
    pub effect_until: Nullable<f64>,
    pub output_count: i64,
    /// The heal amount that confirmed it.
    pub amount: Nullable<f64>,
    /// It was marked as not a paid use: it costs nothing and stays listed
    /// only until that correction is undone.
    pub superseded: bool,
    /// The live correction that minted or superseded it, which can be undone.
    pub correction: Nullable<crate::healing::HealingCorrectionRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingSessionSummary {
    /// The session has ended, so its healing evidence can be corrected.
    pub correctable: bool,
    pub activations: Vec<HealingActivationRow>,
    pub activation_count: i64,
    pub output_count: i64,
    pub direct_outputs: i64,
    pub effect_outputs: i64,
    pub passive_outputs: i64,
    pub unattributed_outputs: i64,
}

/// A decision the player made on a weapon mismatch while the session ran.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponReviewRow {
    pub id: String,
    pub decision: crate::weapons::WeaponReviewDecision,
    /// The weapon the hotbar declared.
    pub hotbar_tool: String,
    /// The weapon the damage evidence named.
    pub evidence_tool: String,
    /// When the evidence first disagreed.
    pub since: f64,
    pub decided_at: f64,
    pub repriced_shots: i64,
    /// What the repricing moved the session's weapon cost by.
    pub cost_delta: f64,
}

/// How a session's shots were attributed. The two tallies were kept from
/// this app version on and are null for an older session; the counts of
/// stored shots and the decisions exist for every session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponAttributionSummary {
    /// The session has ended, so its unpriced shots can be assigned.
    pub correctable: bool,
    /// Shots priced to the weapon the hotbar declared.
    pub agreed: Nullable<i64>,
    /// Shots priced to the weapon their damage named.
    pub evidenced: Nullable<i64>,
    /// Stored shots whose damage overrode the hotbar's weapon.
    pub evidence_shots: i64,
    /// Shots no single carried weapon explained.
    pub unresolved: i64,
    /// Of those, the ones still without a price.
    pub unpriced: i64,
    /// Of those, the ones assigned a weapon after play.
    pub assigned: i64,
    /// Ticks of an effect an earlier paid activation owns.
    pub effect_ticks: i64,
    pub reviews: Vec<WeaponReviewRow>,
}

/// The full session detail.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session_id: String,
    pub summary: SessionSummary,
    pub harvest: HarvestSummary,
    /// The session-name facet as recorded. Present here because the
    /// record is where the name is corrected: the overlay fixes it once
    /// a session starts.
    pub session_name: Nullable<String>,
    pub mob_entry_mode: MobEntryMode,
    pub notable_events: Vec<NotableEvent>,
    pub loot_breakdown: Vec<LootEntry>,
    pub deactivated_loot_breakdown: Vec<LootEntry>,
    pub mob_breakdown: Vec<MobBreakdownRow>,
    pub effective_loot: f64,
    pub tool_stats: Vec<ToolStat>,
    pub skill_gains: Vec<SkillGain>,
    pub weapon_attribution: WeaponAttributionSummary,
    pub healing: HealingSessionSummary,
}

/// One manual-mob autocomplete suggestion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManualMobSuggestion {
    pub display: String,
    pub species: String,
    pub maturity: String,
}

/// The interval kinds as the engine writes them today. The schema keeps
/// the vocabulary open on purpose (a new kind must not need a
/// migration); the wire types the set that exists, and the reader skips
/// a row it cannot parse rather than failing the whole read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionIntervalKind {
    Modifier,
    Segment,
    Quest,
    Consumable,
}

/// One recorded interval of a session: its identity, bounds, and the
/// events attributed to it through the contexts stamped at insert.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionIntervalRow {
    pub id: i64,
    pub kind: SessionIntervalKind,
    /// The display name (a segment's name, a quest's title); null for
    /// kinds that need none.
    pub label: Nullable<String>,
    /// What the interval points at (a quest id); interpretation is
    /// implied by `kind`.
    pub ref_id: Nullable<i64>,
    /// The modifier magnitude, where 0 is the declared unboosted
    /// baseline and null means the kind carries none.
    pub magnitude: Nullable<f64>,
    /// Wall-clock bounds in epoch seconds, exactly as declared. Never
    /// compare these against event timestamps: events carry the chat
    /// log's centralised server time, an hour apart on real data, which
    /// is why attribution goes through contexts instead.
    pub started_at: f64,
    /// Null while the interval is still open.
    pub ended_at: Nullable<f64>,
    /// Attributed event counts, by context membership.
    pub kills: i64,
    pub harvests: i64,
    pub skill_gains: i64,
}

/// A session's recorded intervals, oldest first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionIntervals {
    pub session_id: String,
    pub intervals: Vec<SessionIntervalRow>,
}

/// One recent event in the active-session snapshot feed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecentEvent {
    #[serde(rename = "type")]
    pub kind: NotableEventCategory,
    pub description: String,
    pub value: f64,
    #[serde(rename = "eventType")]
    pub event_type: NotableEventType,
    pub timestamp: Nullable<String>,
    pub id: String,
}

/// One active-session warning.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Warning {
    #[serde(rename = "type")]
    pub kind: NotableEventCategory,
    pub description: String,
    pub value: f64,
}

/// One definition's lifetime figures: the aggregate over every ended
/// instance recorded under it, plus the in-flight instance when one is
/// running under that same definition. The counterpart to the headline
/// figures of the session in play.
///
/// `net` and `return_rate` are derived here, from the summed parts, so
/// the rate is the ratio of the totals and never the mean of the
/// per-instance rates.
///
/// `cycled` rides the summary's basis, which includes the armour and
/// dangling costs a session only acquires once it ends. An in-flight
/// instance has not picked those up yet, so the family figure is
/// legitimately the more complete of the two rather than inconsistent
/// with it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LifetimeStats {
    /// How many instances these figures span, so a surface can disclose
    /// the span rather than let a thin aggregate read as a deep one.
    /// Counts exactly the sessions summed: one started and abandoned
    /// without cycling anything is in neither.
    pub instance_count: i64,
    pub cycled: f64,
    pub loot_tt: f64,
    pub net: f64,
    pub return_rate: f64,
    pub pes: f64,
    pub duration_seconds: f64,
    /// Shots across these instances recorded without a price: the cycled
    /// figure (and everything derived from it) leaves them out.
    pub unpriced_shots: i64,
}

/// The consolidated dashboard hydration snapshot: the polymorphic idle /
/// active shape, in the model's declaration order. Every field is optional
/// and skipped when absent; under the ratified exclude-unset -> exclude-none
/// movement a present-null field is dropped rather than serialised null.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackingSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TrackingState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotbar_listener_active: Option<bool>,
    /// Whether the player enabled the hotbar key listener: hotbar presses
    /// then declare the weapon in hand. Without it, costs follow the
    /// carried weapons' damage ranges alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotbar_keys_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair_ocr_enabled: Option<bool>,
    /// The session-name facet: the active session's when tracking, the
    /// configured next-session value when idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The selected session definition (stringified id): the active
    /// session's stamped reference when tracking, the configured
    /// selection (re-validated against an active definition) when
    /// idle. Absent when no definition is in force.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_definition_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_protection_costs: Option<bool>,
    /// The skill-boost facet (labelled percent), same idle/active
    /// sourcing as the session name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_boost_percent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_mob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tool_kind: Option<HeldItemKind>,
    /// What the held tool implies the next action is recorded as.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_activity: Option<ToolActivity>,
    /// The Activities control's strip-level readout: whether it appears
    /// at all, the ready cue, and the standing set. Carried on every
    /// frame, idle included, over the definition a start would stamp, so
    /// picking a session shows what it will offer rather than making the
    /// surface appear only once tracking begins. The standing set is
    /// necessarily empty while idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activities: Option<ActivitySummary>,
    /// The lifetime figures of the definition this session runs (or
    /// would run) under, for the instance-versus-family flip. Carried
    /// on every frame, idle included, over the definition a start would
    /// stamp, so the flip is available while picking a session rather
    /// than only once tracking begins. Absent when no definition is in
    /// force: a legacy or unattached session has no family to flip to,
    /// and the surfaces read that absence as "offer no control".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<LifetimeStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_events: Option<Vec<RecentEvent>>,
    #[serde(rename = "session_id", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(rename = "started_at", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(rename = "kill_count", skip_serializing_if = "Option::is_none")]
    pub kill_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_dealt_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon_damage_dealt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_tt_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_return_coverage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_return_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shots_fired_total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_hits_total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_damage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub globals_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hofs_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_kill_loot: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplier_last: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplier_avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplier_max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplier_history: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_net_history: Option<Vec<f64>>,
    /// Harvesting swings this session (successes plus explicit fails).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harvest_swings: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harvest_successes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harvest_loot: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harvest_cost: Option<f64>,
    /// The standing harvest-guardrail disagreement; present only while
    /// the loot evidence contradicts the hotbar-equipped tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harvest_guardrail: Option<HarvestGuardrailAlert>,
    /// The standing weapon mismatch; present only while the damage
    /// evidence contradicts the weapon the hotbar declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon_guardrail: Option<WeaponGuardrailAlert>,
    /// Shots this session recorded without a price because no single
    /// carried weapon explains them: the cost leaves them out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpriced_shots: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healing: Option<HealingStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Warning>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeldItemKind {
    Weapon,
    Healing,
    Consumable,
    Harvesting,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingStatus {
    pub tool_name: Nullable<String>,
    pub state: HealingState,
    pub cooldown_until: Nullable<f64>,
    pub effect_until: Nullable<f64>,
    pub activations: i64,
    pub direct_outputs: i64,
    pub effect_outputs: i64,
    pub passive_outputs: i64,
    pub unattributed_outputs: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealingState {
    Passive,
    Ready,
    Cooldown,
    Effect,
}

/// A harvest-guardrail disagreement on the snapshot: the tool the loot
/// evidence expects for the board-output class, the tool the hotbar believed
/// (null when none was equipped), and when the evidence arrived.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestGuardrailAlert {
    pub expected_tool: String,
    pub observed_tool: Nullable<String>,
    pub tree_size: TreeSizeName,
    pub at_epoch: f64,
}

/// A weapon-guardrail disagreement on the snapshot: the weapon the hotbar
/// declared, the weapon the damage evidence says is being fired (and what is
/// being recorded), when the evidence first disagreed, and the shots it has
/// recorded since.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponGuardrailAlert {
    pub hotbar_tool: String,
    pub recording_tool: String,
    pub since: f64,
    pub shots: i64,
}

/// The guardrail's closed board-yield vocabulary: the yield tier evidenced by
/// a swing's board output. The type name is the guardrail alias described on
/// `TreeSize` in the services layer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TreeSizeName {
    Short,
    Long,
    Huge,
}

/// The start lifecycle acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StartResult {
    pub session_id: String,
    pub started_at: String,
    pub status: TrackingState,
}

/// The stop lifecycle acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StopResult {
    pub session_id: String,
    pub started_at: String,
    pub ended_at: Nullable<String>,
    pub kill_count: i64,
}

/// The release-mob acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReleaseResult {
    pub released: Nullable<String>,
}

/// The manual-mob-lock acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManualMobLockResult {
    pub mob_name: String,
    pub species: String,
    pub maturity: String,
}

/// The session-config acknowledgement: the facet values now in force
/// (null: not declared).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfigResult {
    pub session_name: Nullable<String>,
    pub skill_boost_percent: Nullable<i64>,
}

/// The definition-selection acknowledgement: the selection now in
/// force (stringified id) and the session name it wrote (null: none).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionSelectResult {
    pub session_definition_id: Nullable<String>,
    pub session_name: Nullable<String>,
}

/// The post-hoc re-file result: the definition the session now belongs
/// to, and the name it now carries, which is always the new
/// definition's (the stamp follows the reference unconditionally).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionReassignResult {
    pub session_id: String,
    pub definition_id: String,
    pub session_name: Nullable<String>,
}

/// The mob rename / restore result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobEditResult {
    pub session_id: String,
    pub mob_name: String,
    pub kill_count: i64,
}

/// The loot-item activate / deactivate result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LootItemEditResult {
    pub session_id: String,
    pub item_name: String,
    pub affected_rows: i64,
    pub total_value_delta: f64,
    pub session_total_returns: f64,
}

/// The armour-cost result (echoes the submitted value, not the new total).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmourCostResult {
    pub session_id: String,
    pub armour_cost: f64,
}

/// The one-shot repair-cost read (`exclude_unset`): the cost / raw text /
/// confidence on success, plus `error` on a logical refusal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepairScanResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_ped: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// One keyset page of sessions (newest first), each shaped from its
    /// summary or raw tables. Heals ended-session summaries first (a write
    /// on the read path, preserved from the reference). A malformed cursor
    /// is a bad-request.
    ///
    /// `definition_id` narrows the page to one definition's instances,
    /// which is how the review surface reads one; omitted, the read
    /// is the whole table exactly as before.
    pub async fn tracking_sessions(
        &self,
        cursor: Option<String>,
        limit: Option<i64>,
        definition_id: Option<i64>,
    ) -> Result<SessionPage, ApiError> {
        let seek = match cursor.as_deref() {
            None => None,
            Some(token) => match decode_session_cursor(token) {
                Some(key) => Some(key),
                None => return Err(ApiError::bad_request("Invalid cursor")),
            },
        };
        let now = naive_to_epoch(self.clock.now());
        let page = list_sessions_impl(&self.db, now, seek, limit, definition_id)
            .await
            .map_err(ApiError::internal("tracking sessions"))?;
        let sessions: Vec<TrackingSession> = serde_json::from_value(page.sessions)
            .map_err(ApiError::internal("tracking sessions shaping"))?;
        Ok(SessionPage {
            sessions,
            next_cursor: page.next_cursor.into(),
            total: page.total,
        })
    }

    /// One session's full detail; an absent session is a not-found.
    pub async fn tracking_session_detail(
        &self,
        session_id: String,
    ) -> Result<SessionDetail, ApiError> {
        let now = naive_to_epoch(self.clock.now());
        match get_session_impl(&self.db, &session_id, now)
            .await
            .map_err(ApiError::internal("tracking session detail"))?
        {
            Some(value) => serde_json::from_value(value)
                .map_err(ApiError::internal("tracking session detail shaping")),
            None => Err(ApiError::not_found("Session not found")),
        }
    }

    /// Catalogue mob-name autocomplete for the declared-mob typeahead.
    pub async fn tracking_manual_mob_suggestions(
        &self,
        q: String,
        limit: Option<i64>,
    ) -> Result<Vec<ManualMobSuggestion>, ApiError> {
        let query = q.trim_matches(python_whitespace);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let bounded = limit.unwrap_or(10).clamp(1, 20) as usize;
        let lookup = MobLookupService::new(&self.game_data);
        lookup
            .search_mob_names(query, bounded)
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(ApiError::internal("manual mob suggestions shaping"))
            })
            .collect()
    }

    /// The consolidated dashboard hydration snapshot.
    pub async fn tracking_snapshot(&self) -> Result<TrackingSnapshot, ApiError> {
        let config = load_config_readonly(&self.data_dir)
            .map_err(ApiError::internal("tracking snapshot config"))?;
        let value = build_snapshot_value(
            &self.db,
            &self.tracker,
            &config,
            self.hotbar.is_running(),
            Some(&self.session_definitions),
            naive_to_epoch(self.clock.now()),
        )
        .await?;
        serde_json::from_value(value).map_err(ApiError::internal("tracking snapshot shaping"))
    }

    /// A session's recorded intervals with bounds and attributed event
    /// counts: the thin read over the segment contract. Membership
    /// comes from the contexts stamped on each event row at insert,
    /// never from comparing timestamps (interval bounds are wall-clock
    /// declarations; event timestamps carry the chat log's server
    /// time). The economics on top of this (cycled PED, return rate,
    /// per-segment rewards) belong to the analytics surfaces, not here.
    /// 404 for an absent session.
    pub async fn tracking_session_intervals(
        &self,
        session_id: String,
    ) -> Result<SessionIntervals, ApiError> {
        if !self.tracking_session_exists(&session_id).await? {
            return Err(ApiError::not_found("Session not found"));
        }
        let session = session_id.clone();
        let (rows, counts) = self
            .db
            .with_reader(move |conn| {
                let session_scope = session.clone();
                let mut stmt = conn.prepare(
                    "SELECT id, kind, label, ref_id, magnitude, started_at, ended_at \
                     FROM session_intervals WHERE session_id = ? ORDER BY id",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![session], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<f64>>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, Option<f64>>(6)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                // One count map per event table: every event whose
                // stamped context names the interval belongs to it.
                let mut counts: HashMap<i64, (i64, i64, i64)> = HashMap::new();
                for (table, slot) in [("kills", 0usize), ("harvest_events", 1), ("skill_gains", 2)]
                {
                    let sql = format!(
                        "SELECT sci.interval_id, COUNT(*) \
                         FROM session_context_intervals sci \
                         JOIN session_contexts sc ON sc.id = sci.context_id \
                         JOIN {table} e ON e.context_id = sci.context_id \
                         WHERE sc.session_id = ? \
                         GROUP BY sci.interval_id",
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let mapped = stmt.query_map(rusqlite::params![session_scope], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                    })?;
                    for entry in mapped {
                        let (interval_id, count) = entry?;
                        let bucket = counts.entry(interval_id).or_default();
                        match slot {
                            0 => bucket.0 = count,
                            1 => bucket.1 = count,
                            _ => bucket.2 = count,
                        }
                    }
                }
                Ok((rows, counts))
            })
            .await
            .map_err(ApiError::internal("session intervals"))?;

        let intervals = rows
            .into_iter()
            .filter_map(
                |(id, kind, label, ref_id, magnitude, started_at, ended_at)| {
                    // An unparseable kind can only come from a future build's
                    // rows; skip it rather than failing the whole read.
                    let kind = match kind.as_str() {
                        "modifier" => SessionIntervalKind::Modifier,
                        "segment" => SessionIntervalKind::Segment,
                        "quest" => SessionIntervalKind::Quest,
                        "consumable" => SessionIntervalKind::Consumable,
                        _ => return None,
                    };
                    let (kills, harvests, skill_gains) =
                        counts.get(&id).copied().unwrap_or((0, 0, 0));
                    Some(SessionIntervalRow {
                        id,
                        kind,
                        label: label.into(),
                        ref_id: ref_id.into(),
                        magnitude: magnitude.into(),
                        started_at,
                        ended_at: ended_at.into(),
                        kills,
                        harvests,
                        skill_gains,
                    })
                },
            )
            .collect();
        Ok(SessionIntervals {
            session_id,
            intervals,
        })
    }

    /// Begin a tracking session. 409 if one is already active (before the
    /// tool gate), 400 while no tool is configured.
    pub async fn tracking_start(&self) -> Result<StartResult, ApiError> {
        if self.tracker.is_tracking() {
            return Err(ApiError::conflict("Session already active"));
        }
        let config = load_config_readonly(&self.data_dir)
            .map_err(ApiError::internal("tracking start config"))?;
        let (ready, message) = validate_tools(&config);
        if !ready {
            return Err(ApiError::bad_request(message.unwrap_or_else(|| {
                "Bind a hotbar slot or add a carried weapon in Equipment before tracking."
                    .to_string()
            })));
        }
        let session = self
            .tracker
            .start_session()
            .await
            .map_err(ApiError::internal("tracking start"))?;
        Ok(StartResult {
            session_id: session.id,
            started_at: local_isoformat(session.start_time),
            status: TrackingState::Active,
        })
    }

    /// End the active tracking session. 409 if none is active.
    pub async fn tracking_stop(&self) -> Result<StopResult, ApiError> {
        if !self.tracker.is_tracking() {
            return Err(ApiError::conflict("No active session"));
        }
        match self
            .tracker
            .stop_session()
            .await
            .map_err(ApiError::internal("tracking stop"))?
        {
            Some(session) => Ok(StopResult {
                session_id: session.id.clone(),
                started_at: local_isoformat(session.start_time),
                ended_at: session.end_time.map(local_isoformat).into(),
                kill_count: session.kills.len() as i64,
            }),
            // Defensive: `is_tracking` was true above, so a None is a broken
            // invariant. The transport's 500 message does not survive (the
            // boundary reply is fixed); logged server-side.
            None => Err(ApiError::invalid_state("tracking stop returned no session")),
        }
    }

    /// Clear the declared mob, echoing what was released.
    pub async fn tracking_release_mob(&self) -> Result<ReleaseResult, ApiError> {
        // The (non-`Send`) config guard is scoped around each read/write
        // so it is never held across the tracker's await points; the
        // release-then-write order within each branch is unchanged.
        let lock_config = || {
            self.config_service
                .lock()
                .map_err(|_| ApiError::invalid_state("release mob: poisoned config lock"))
        };
        let released = if self.tracker.is_tracking() {
            let released = self.tracker.release_declared_mob().await;
            lock_config()?
                .update(&clear_manual_mob())
                .map_err(ApiError::internal("release mob"))?;
            released.map(Value::from).unwrap_or(Value::Null)
        } else {
            let mut guard = lock_config()?;
            let species = guard.get().manual_mob_species.trim().to_string();
            let maturity = guard.get().manual_mob_maturity.trim().to_string();
            let released = mob_display(&species, &maturity);
            guard
                .update(&clear_manual_mob())
                .map_err(ApiError::internal("release mob"))?;
            released
        };
        Ok(ReleaseResult {
            released: opt_str(&released).into(),
        })
    }

    /// Declare a catalogue mob for kill stamping. 400 when the mob is
    /// absent from the catalogue; mid-session declaration changes are
    /// allowed by design.
    pub async fn tracking_manual_mob_lock(
        &self,
        species: String,
        maturity: Option<String>,
    ) -> Result<ManualMobLockResult, ApiError> {
        let maturity = maturity.unwrap_or_default();
        let species = species.trim();
        let maturity = maturity.trim();
        let display = if maturity.is_empty() {
            species.to_string()
        } else {
            format!("{maturity} {species}")
        };
        // Validate and write inside a block so the (non-`Send`) config
        // guard is gone before the tracker await below.
        {
            let Ok(mut guard) = self.config_service.lock() else {
                return Err(ApiError::invalid_state(
                    "manual mob lock: poisoned config lock",
                ));
            };
            if !MobLookupService::new(&self.game_data).has_mob_name(species, maturity) {
                return Err(ApiError::bad_request("Mob is not present in the catalogue"));
            }
            let mut updates = Map::new();
            updates.insert("manual_mob_species".into(), json!(species));
            updates.insert("manual_mob_maturity".into(), json!(maturity));
            guard
                .update(&updates)
                .map_err(ApiError::internal("manual mob lock"))?;
        }
        if self.tracker.is_tracking() {
            // The only reachable error is the session stopping between
            // the check and the call; the config write above already
            // carries the declaration into the next session either way.
            let _ = self
                .tracker
                .set_declared_mob(&display, species, maturity)
                .await;
        }
        Ok(ManualMobLockResult {
            mob_name: display,
            species: species.to_string(),
            maturity: maturity.to_string(),
        })
    }

    /// Set the session facets: the designated name and the skill boost.
    /// Full-state apply (a null clears its facet).
    ///
    /// The two facets differ in how they may move, not by taste. The
    /// boost may be re-declared mid-session (a pill expiring is a real
    /// change worth recording), and it is three-state: null withdraws
    /// the declaration, 0 records deliberately-unboosted play (the
    /// baseline a boost's effect is measured against), and a positive
    /// value records a magnitude. The session row mirrors only the
    /// positive case; the declared zero lives on the interval record.
    /// The name names the whole session, so a live edit could only
    /// rewrite its history: it is fixed once a session runs (409). It is
    /// a stamp of the definition's name rather than a label of its own,
    /// so a mis-recorded session is corrected by moving it to another
    /// definition (`tracking_reassign_session`), never by retyping.
    pub async fn tracking_session_config(
        &self,
        session_name: Option<String>,
        skill_boost_percent: Option<i64>,
    ) -> Result<SessionConfigResult, ApiError> {
        let name = session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        // Three-state and deliberately NOT collapsed: `None` withdraws the
        // declaration, `Some(0)` declares deliberately-unboosted play. A
        // negative is nonsense rather than a third meaning, so it reads as
        // a withdrawal instead of reaching the interval layer.
        let boost = skill_boost_percent.filter(|percent| *percent >= 0);
        // Validate and write inside a block so the (non-`Send`) config
        // guard is gone before the tracker await below.
        {
            let Ok(mut guard) = self.config_service.lock() else {
                return Err(ApiError::invalid_state(
                    "session config: poisoned config lock",
                ));
            };
            if self.tracker.is_tracking()
                && name.as_deref().unwrap_or("") != guard.get().session_name.trim()
            {
                return Err(ApiError::conflict(
                    "Session name is fixed for the active session; move the session to another definition once it ends",
                ));
            }
            let mut updates = Map::new();
            updates.insert("session_name".into(), json!(name.as_deref().unwrap_or("")));
            // A name write that CHANGES the name disavows the definition
            // selection (selection is what writes the name, so a
            // diverging name is a free-text declaration); an unchanged
            // name (a boost-only write) leaves the selection standing.
            if name.as_deref().unwrap_or("") != guard.get().session_name.trim() {
                updates.insert("session_definition_id".into(), Value::Null);
            }
            updates.insert("declared_skill_boost_percent".into(), json!(boost));
            guard
                .update(&updates)
                .map_err(ApiError::internal("session config"))?;
        }
        if self.tracker.is_tracking() {
            let _ = self.tracker.set_skill_boost(boost).await;
        }
        Ok(SessionConfigResult {
            session_name: name.into(),
            skill_boost_percent: boost.into(),
        })
    }

    /// Select the session definition the next session starts as an
    /// instance of, writing the session-name facet with the
    /// definition's name in the same motion; null withdraws the
    /// selection and clears the name, which reads back as the protected
    /// default rather than as no session. 404 for an id naming no active
    /// definition; 409 when a session is running and the selection
    /// would change (the facet snapshots at start and never moves for
    /// the session's life).
    pub async fn tracking_definition_select(
        &self,
        definition_id: Option<i64>,
    ) -> Result<DefinitionSelectResult, ApiError> {
        let _transition = self.definition_transition.lock().await;
        let selected = match definition_id {
            Some(id) => Some(
                self.session_definitions
                    .get_active(id)
                    .await
                    .map_err(crate::session_definitions::definition_error(
                        "definition select",
                    ))?
                    .ok_or_else(|| ApiError::not_found("Session definition not found"))?,
            ),
            None => None,
        };
        let selected_id = selected.as_ref().map(|definition| definition.id);
        let selected_name = selected.as_ref().map(|definition| definition.name.clone());
        {
            let Ok(mut guard) = self.config_service.lock() else {
                return Err(ApiError::invalid_state(
                    "definition select: poisoned config lock",
                ));
            };
            if self.tracker.is_tracking() && guard.get().session_definition_id != selected_id {
                return Err(ApiError::conflict(
                    "Session definition is fixed for the active session; a new selection applies from the next session",
                ));
            }
            let mut updates = Map::new();
            updates.insert("session_definition_id".into(), json!(selected_id));
            updates.insert(
                "session_name".into(),
                json!(selected_name.as_deref().unwrap_or("")),
            );
            guard
                .update(&updates)
                .map_err(ApiError::internal("definition select"))?;
        }
        Ok(DefinitionSelectResult {
            session_definition_id: selected_id.map(|id| id.to_string()).into(),
            session_name: selected_name.into(),
        })
    }

    /// Re-file an ended session under a different (active) definition:
    /// the correction for a session recorded against whichever
    /// definition the picker happened to be holding. The stamped name
    /// always follows the reference, because it is a copy of the
    /// definition's name rather than a label of its own.
    pub async fn tracking_reassign_session(
        &self,
        session_id: String,
        definition_id: i64,
    ) -> Result<SessionReassignResult, ApiError> {
        let value = reassign_session_definition_impl(&self.db, &session_id, definition_id)
            .await
            .map_err(edit_error("tracking reassign session"))?;
        serde_json::from_value(value)
            .map_err(ApiError::internal("tracking reassign session shaping"))
    }

    /// Rename a mob across an ended session.
    pub async fn tracking_rename_mob(
        &self,
        session_id: String,
        from_mob_name: String,
        to_mob_name: String,
    ) -> Result<MobEditResult, ApiError> {
        let value = rename_session_mob_impl(&self.db, &session_id, &from_mob_name, &to_mob_name)
            .await
            .map_err(edit_error("tracking rename mob"))?;
        serde_json::from_value(value).map_err(ApiError::internal("tracking rename mob shaping"))
    }

    /// Restore a renamed mob to its preserved original.
    pub async fn tracking_restore_mob(
        &self,
        session_id: String,
        current_mob_name: String,
    ) -> Result<MobEditResult, ApiError> {
        let value = restore_session_mob_impl(&self.db, &session_id, &current_mob_name)
            .await
            .map_err(edit_error("tracking restore mob"))?;
        serde_json::from_value(value).map_err(ApiError::internal("tracking restore mob shaping"))
    }

    /// Re-activate a deactivated loot line.
    pub async fn tracking_loot_item_activate(
        &self,
        session_id: String,
        item_name: String,
    ) -> Result<LootItemEditResult, ApiError> {
        let value = bulk_flip_loot_item(&self.db, &session_id, &item_name, "active")
            .await
            .map_err(edit_error("tracking loot item activate"))?;
        serde_json::from_value(value)
            .map_err(ApiError::internal("tracking loot item activate shaping"))
    }

    /// Deactivate a loot line.
    pub async fn tracking_loot_item_deactivate(
        &self,
        session_id: String,
        item_name: String,
    ) -> Result<LootItemEditResult, ApiError> {
        let value = bulk_flip_loot_item(&self.db, &session_id, &item_name, "deactivated")
            .await
            .map_err(edit_error("tracking loot item deactivate"))?;
        serde_json::from_value(value)
            .map_err(ApiError::internal("tracking loot item deactivate shaping"))
    }

    /// Add an armour cost to a session (no active-session guard; 404 only
    /// when absent). Echoes the submitted value.
    pub async fn tracking_armour_cost(
        &self,
        session_id: String,
        cost: f64,
    ) -> Result<ArmourCostResult, ApiError> {
        let value = set_armour_cost_impl(&self.db, &session_id, cost)
            .await
            .map_err(edit_error("tracking armour cost"))?;
        serde_json::from_value(value).map_err(ApiError::internal("tracking armour cost shaping"))
    }

    /// The one-shot repair-cost OCR read, gated on `repair_ocr_enabled`
    /// (400 when disabled). The `session_id` is unused (the reference
    /// ignores it too); it stays in the signature for the route mapping.
    pub fn tracking_repair_scan(&self, session_id: String) -> Result<RepairScanResult, ApiError> {
        let _ = session_id;
        let config = load_config_readonly(&self.data_dir)
            .map_err(ApiError::internal("repair scan config"))?;
        if !config.repair_ocr_enabled {
            return Err(ApiError::bad_request("Repair OCR is disabled"));
        }
        let value = project(&self.repair_ocr.scan_repair_cost(), &REPAIR_FIELDS);
        serde_json::from_value(value).map_err(ApiError::internal("repair scan shaping"))
    }

    /// Delete a session and all of its data (an active session cannot be
    /// deleted; a missing one is a not-found). The rollups are repaired for
    /// the days it touched. Its healing evidence can explain another
    /// session's heals and a running session's effect windows, so the
    /// deletion is announced as a healing change.
    pub async fn tracking_session_delete(&self, session_id: String) -> Result<(), ApiError> {
        delete_session_impl(&self.db, &session_id)
            .await
            .map_err(edit_error("tracking session delete"))?;
        self.healing_review.announce_changed();
        Ok(())
    }

    // ── Private snapshot assembly ────────────────────────────────────

    /// The session-existence precondition the quest-link operations apply.
    async fn tracking_session_exists(&self, session_id: &str) -> Result<bool, ApiError> {
        session_exists(&self.db, session_id)
            .await
            .map_err(ApiError::internal("session existence"))
    }
}

// ── Snapshot assembly (shared by the live and demo snapshots) ────────

/// The definition's lifetime figures for the instance-versus-family
/// flip, or `None` when no definition is in force (a legacy or
/// deliberately unattached session has no family, so the surfaces offer
/// no control at all rather than an inert one).
///
/// Resolves the definition exactly as the Activities control does: the
/// id stamped at start while a session runs, otherwise the one a start
/// would stamp. The in-flight instance is folded in on top of the
/// stored aggregate, because a "lifetime" that excluded the session the
/// user is watching would be a trap; the summaries cover ended sessions
/// only, so there is no double count.
async fn lifetime_stats(
    db: &Db,
    definitions: Option<&SessionDefinitionService>,
    active: Option<&ActiveSessionView>,
    idle_definition_id: Option<i64>,
) -> Result<Option<LifetimeStats>, ApiError> {
    let definition_id = match active {
        Some(active) => active.definition_id,
        None => idle_definition_id,
    };
    let (Some(service), Some(definition_id)) = (definitions, definition_id) else {
        return Ok(None);
    };
    let stored = service
        .lifetime_stats(definition_id)
        .await
        .map_err(ApiError::internal("snapshot lifetime stats"))?;

    let mut instance_count = stored.instance_count;
    let mut cycled = stored.cycled;
    let mut loot_tt = stored.loot_tt;
    let mut pes = stored.pes;
    let mut duration_seconds = stored.duration_seconds;
    // The ended instances' unpriced shots are stored; the running one's are
    // counted live (most are still waiting for their kill to be written).
    let mut unpriced_shots = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM weapon_shot_evidence e \
                 JOIN tracking_sessions s ON s.id = e.session_id \
                 WHERE s.definition_id = ?1 AND s.is_active = 0 \
                   AND e.attribution = 'unresolved' AND e.tool_name IS NULL",
                [definition_id],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .map_err(ApiError::internal("snapshot lifetime unpriced shots"))?;
    // Only when the running session belongs to THIS definition: a
    // selection changed while idle reads the newly-picked family's
    // history, not the running instance's figures.
    if let Some(active) = active.filter(|active| active.definition_id == Some(definition_id)) {
        instance_count += 1;
        cycled += active.cost;
        loot_tt += active.returns;
        pes += active.pes;
        duration_seconds += active.elapsed as f64;
        unpriced_shots += active.unpriced_shots;
    }

    Ok(Some(LifetimeStats {
        instance_count,
        cycled: round_half_even(cycled, 2),
        loot_tt: round_half_even(loot_tt, 2),
        net: round_half_even(loot_tt - cycled, 2),
        // The ratio of the summed parts, never the mean of the
        // per-instance rates. Zero spend reads as zero, matching the
        // instance figure's own rule.
        return_rate: if cycled > 0.0 {
            round_half_even(loot_tt / cycled, 4)
        } else {
            0.0
        },
        pes: round_half_even(pes, 2),
        duration_seconds: round_half_even(duration_seconds, 0),
        unpriced_shots,
    }))
}

/// Assemble the projected snapshot value from the tracker readout, the
/// resolved config, and the hotbar listener's running state. A free
/// function (rather than an `Api` method) so both the live snapshot and
/// the guide-mode demo snapshot, which runs over its own parallel tracker
/// and database, share one assembly.
pub(crate) async fn build_snapshot_value(
    db: &Db,
    tracker: &HuntTracker,
    config: &AppConfig,
    hotbar_active: bool,
    definitions: Option<&SessionDefinitionService>,
    now: f64,
) -> Result<Value, ApiError> {
    let readout = tracker
        .snapshot()
        .await
        .map_err(ApiError::internal("snapshot readout"))?;
    let current_tool = match &readout.current_tool {
        Some(tool) => Value::String(tool.clone()),
        None => Value::Null,
    };
    // The derived-activity feedback: what the held tool implies the
    // next action is recorded as (absent when no tool is known).
    let current_activity = match readout.current_tool_kind {
        Some(eo_services::bus_events::HotbarItemKind::Harvesting) => {
            Value::String("treecutting".into())
        }
        Some(eo_services::bus_events::HotbarItemKind::Weapon) => Value::String("hunting".into()),
        _ => Value::Null,
    };
    let current_tool_kind = match readout.current_tool_kind {
        Some(eo_services::bus_events::HotbarItemKind::Weapon) => Value::String("weapon".into()),
        Some(eo_services::bus_events::HotbarItemKind::Healing) => Value::String("healing".into()),
        Some(eo_services::bus_events::HotbarItemKind::Consumable) => {
            Value::String("consumable".into())
        }
        Some(eo_services::bus_events::HotbarItemKind::Harvesting) => {
            Value::String("harvesting".into())
        }
        None => Value::Null,
    };
    // The facet pair serialises null for "not declared" (the projection
    // drops the key): idle reads the configured next-session values,
    // active reads the session's snapshot.
    let name_value = |name: Option<&str>| match name.filter(|value| !value.is_empty()) {
        Some(name) => Value::String(name.to_string()),
        None => Value::Null,
    };
    // A declared zero serialises as 0, not null: null is reserved for
    // "nothing declared", and the overlay reads the two apart.
    let boost_value = |percent: Option<i64>| match percent.filter(|value| *value >= 0) {
        Some(percent) => json!(percent),
        None => Value::Null,
    };

    // The definition facet, stringified for the wire (null drops the
    // key under exclude-none). The idle branch re-validates the
    // configured selection against an ACTIVE definition, so one archived
    // while idle stops reading as selected; the active branch trusts
    // the id stamped (and validated) at session start.
    let definition_value = |id: Option<i64>| match id {
        Some(id) => json!(id.to_string()),
        None => Value::Null,
    };
    // Idle resolves through the same rule session start stamps with, so
    // the readout shows exactly what starting now would record: the
    // selection while it is active, otherwise the protected default.
    let idle_selection =
        eo_services::session_definitions::resolve_selection(db, config.session_definition_id)
            .await
            .map_err(ApiError::internal("snapshot definition selection"))?;
    let idle_definition_id = idle_selection.as_ref().map(|(id, _, _)| *id);
    let idle_track_protection_costs = idle_selection
        .as_ref()
        .is_none_or(|(_, _, track_costs)| *track_costs);

    // The Activities control's strip-level readout: whether the control
    // appears at all, how many rows a tap could start, and what is
    // standing. Computed by the same function the control's own menu
    // reads, so the chip's cue and the menu's rows cannot disagree.
    let picture = crate::activities::activity_picture(
        db,
        definitions,
        readout.active.as_ref(),
        idle_definition_id,
        now,
    )
    .await?;
    let activities = json!(ActivitySummary::from(&picture));

    // The family side of the instance-versus-family flip, resolved over
    // the same definition the Activities control reads: the stamped one
    // while tracking, otherwise the one a start would stamp.
    let lifetime =
        lifetime_stats(db, definitions, readout.active.as_ref(), idle_definition_id).await?;
    let lifetime = match lifetime {
        Some(stats) => json!(stats),
        None => Value::Null,
    };

    let value = match &readout.active {
        None => {
            json!({
                "status": "idle",
                "hotbarListenerActive": hotbar_active,
                "hotbarKeysEnabled": config.hotbar_hooks_enabled,
                "repairOcrEnabled": config.repair_ocr_enabled,
                "currentTool": current_tool,
                "currentToolKind": current_tool_kind,
                "currentActivity": current_activity,
                "sessionName": name_value(Some(if config.session_name.trim().is_empty() {
                    idle_selection.as_ref().map_or("", |(_, name, _)| name.as_str())
                } else {
                    config.session_name.trim()
                })),
                "sessionDefinitionId": definition_value(idle_definition_id),
                "trackProtectionCosts": idle_track_protection_costs,
                "skillBoostPercent": boost_value(config.declared_skill_boost_percent),
                "currentMob": declared_mob_label(config),
                "activities": activities,
                "lifetime": lifetime,
                "recentEvents": [],
            })
        }
        Some(active) => {
            let recent_events: Vec<Value> = active
                    .notable_event_rows
                    .iter()
                    .enumerate()
                    .map(|(index, (event_type, mob_or_item, value_ped, ts))| {
                        json!({
                            "type": notable_event_category(event_type),
                            "description": notable_event_description(event_type, mob_or_item, *value_ped),
                            "value": *value_ped,
                            "eventType": event_type.clone(),
                            "timestamp": event_ts_to_iso(*ts),
                            "id": format!("ne-{index}"),
                        })
                    })
                    .collect();
            let warnings: Vec<Value> = active
                .warnings
                .iter()
                .map(|message| json!({"type": "warning", "description": message, "value": 0.0}))
                .collect();
            // The guardrail key rides only while a disagreement stands
            // (`exclude_unset`): absent is the quiet steady state.
            let harvest_guardrail = active.harvest_guardrail_mismatch.as_ref().map(|mismatch| {
                json!({
                    "expectedTool": mismatch.expected_tool.clone(),
                    "observedTool": mismatch.observed_tool.clone(),
                    "treeSize": mismatch.tree_size.clone(),
                    "atEpoch": mismatch.at_epoch,
                })
            });
            let mut value = json!({
                "status": "active",
                "session_id": active.session_id.clone(),
                "started_at": active.started_at.clone(),
                "kill_count": active.kill_count,
                "elapsed": active.elapsed,
                "cost": active.cost,
                "returns": active.returns,
                "pes": active.pes,
                "net": active.net,
                "returnRate": active.return_rate,
                "damageDealtTotal": active.damage_dealt_total,
                "weaponDamageDealt": active.weapon_damage_dealt,
                "weaponCost": active.weapon_cost,
                "expectedTtRate": active.expected_tt_rate,
                "expectedReturnCoverage": active.expected_return_coverage,
                "expectedReturnModel": active.expected_return_model.clone(),
                "shotsFiredTotal": active.shots_fired_total,
                "criticalHitsTotal": active.critical_hits_total,
                "maxDamage": active.max_damage,
                "globalsCount": active.globals_count,
                "hofsCount": active.hofs_count,
                "latestKillLoot": active.latest_kill_loot,
                "multiplierLast": active.multiplier_last,
                "multiplierAvg": active.multiplier_avg,
                "multiplierMax": active.multiplier_max,
                "multiplierHistory": active.multiplier_history.clone(),
                "cumulativeNetHistory": active.cumulative_net_history.clone(),
                "harvestSwings": active.harvest_swings,
                "harvestSuccesses": active.harvest_successes,
                "harvestLoot": active.harvest_loot,
                "harvestCost": active.harvest_cost,
                "unpricedShots": active.unpriced_shots,
                "healing": {
                    "toolName": active.healing.tool_name.clone(),
                    "state": active.healing.state.clone(),
                    "cooldownUntil": active.healing.cooldown_until,
                    "effectUntil": active.healing.effect_until,
                    "activations": active.healing.activation_count,
                    "directOutputs": active.healing.direct_output_count,
                    "effectOutputs": active.healing.effect_output_count,
                    "passiveOutputs": active.healing.passive_output_count,
                    "unattributedOutputs": active.healing.unattributed_output_count,
                },
                "hotbarListenerActive": hotbar_active,
                "hotbarKeysEnabled": config.hotbar_hooks_enabled,
                "repairOcrEnabled": config.repair_ocr_enabled,
                "currentTool": current_tool,
                "currentToolKind": current_tool_kind,
                "currentActivity": current_activity,
                "activities": activities,
                "lifetime": lifetime,
                "sessionName": name_value(active.session_name.as_deref()),
                "sessionDefinitionId": definition_value(active.definition_id),
                "trackProtectionCosts": active.track_protection_costs,
                "skillBoostPercent": boost_value(active.skill_boost_percent),
                "currentMob": active.current_mob.clone(),
                "recentEvents": recent_events,
                "warnings": warnings,
            });
            if let (Some(mismatch), Some(object)) = (harvest_guardrail, value.as_object_mut()) {
                object.insert("harvestGuardrail".to_string(), mismatch);
            }
            // Likewise the weapon guardrail: absent while the evidence
            // agrees with the hotbar.
            let weapon_guardrail = active.weapon_guardrail_mismatch.as_ref().map(|mismatch| {
                json!({
                    "hotbarTool": mismatch.hotbar_tool.clone(),
                    "recordingTool": mismatch.recording_tool.clone(),
                    "since": mismatch.since,
                    "shots": mismatch.shots,
                })
            });
            if let (Some(mismatch), Some(object)) = (weapon_guardrail, value.as_object_mut()) {
                object.insert("weaponGuardrail".to_string(), mismatch);
            }
            value
        }
    };
    Ok(project(&value, &SNAPSHOT_FIELDS))
}
