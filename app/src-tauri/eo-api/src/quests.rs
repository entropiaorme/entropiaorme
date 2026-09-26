//! The quests family: quest CRUD and lifecycle, mob-name autocomplete,
//! reward review, families, and per-quest analytics.
//!
//! The computation stays in `eo_services::quests::QuestService`, which
//! speaks `serde_json::Value` in **snake_case** and leaves the camelCase
//! wire projection to its caller; the facade owns that projection as
//! declared response DTOs whose field order is the golden-pinned wire
//! order. The request DTOs carry the frontend's snake_case field casing
//! and build the exact snake_case `Value` the service consumes, so no
//! stored bytes change.
//!
//! The facade serves over the composed [`QuestService`] (the same
//! instance whose owning task carries the bus-fed session tracking,
//! mission auto-start, and reward suppression), so a facade-driven
//! completion records against the live tracking session exactly as a
//! chat-log-driven one does.
//!
//! Contract lineage (ADR-0017/0019): transport-era behaviours retired at
//! the typed-command crossing. The conditional-GET (ETag) contract retires with the
//! transport (the reads answer their body directly). The write surface's
//! framework validation envelope (the create/update field-type 422s, the
//! surrogate-taint / beyond-`i64` deferred 500 ceremony) becomes
//! unrepresentable over the typed DTOs (a typed field cannot carry a
//! wrong type, an `i64` argument cannot overflow the parse). The quests
//! router caught no service error, so every `QuestError` was the
//! backend's unhandled-exception 500; the facade maps them all to
//! [`ApiError::Internal`] verbatim (there is no `bad_request` arm on this
//! family, unlike codex). The `{"ok": true}` delete body retires with no
//! consumer (the frontend delete wrappers ignore it).

use eo_services::quests::QuestError;
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::Nullable;
use crate::{Api, ApiError};

// ── Request arguments ───────────────────────────────────────────────

/// A quest create or update payload. One DTO serves both operations, in
/// the frontend's snake_case field casing: the sole client sends the
/// full field set for both create and update (nulls explicit), so every
/// field is serialised to the service `Value` (present-null clears a
/// column on update, exactly as the exclude-unset contract did for the
/// only payload the client ever produced). `update_quest` ignores the
/// `mobs` key by design, so a full payload is behaviour-identical to the
/// HTTP path either way.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct QuestInput {
    pub name: String,
    #[serde(default = "default_planet")]
    pub planet: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub waypoint: Option<String>,
    #[serde(default)]
    pub cooldown_hours: Option<f64>,
    #[serde(default)]
    pub reward_ped: Option<f64>,
    #[serde(default)]
    pub reward_is_skill: bool,
    #[serde(default)]
    pub reward_description: Option<String>,
    #[serde(default)]
    pub completion_trigger: Option<QuestCompletionTrigger>,
    #[serde(default)]
    pub reward_policy: Option<QuestRewardPolicy>,
    #[serde(default)]
    pub reward_item_names: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub chain_name: Option<String>,
    #[serde(default)]
    pub chain_position: Option<i64>,
    #[serde(default)]
    pub chain_total: Option<i64>,
    #[serde(default)]
    pub mobs: Vec<String>,
    /// The signal loot item, when set: the quest completes the moment
    /// this item arrives in a loot pickup carrying no mission
    /// completion (the instance-boss pattern), and declaring it starts
    /// it directly. Completion and reward policies are independent.
    #[serde(default)]
    pub signal_loot_item: Option<String>,
    /// The family this quest is a variant of; null (or absent) leaves
    /// it standalone. Sent explicitly by the form so a cleared select
    /// detaches; the service refuses an id that names no active family.
    #[serde(default)]
    pub family_id: Option<i64>,
    /// When this quest's OWN cooldown timer starts; absent keeps the
    /// service default ('completion', the pre-family behaviour).
    #[serde(default)]
    pub cooldown_anchor: Option<QuestCooldownAnchor>,
}

impl QuestInput {
    /// The snake_case service payload the create/update dumps produced:
    /// every field present, optionals as explicit `null`, defaults
    /// applied. `create_quest` reads the full set; `update_quest` applies
    /// the keys it allows (all but `mobs`), present-null included.
    fn to_service_value(&self) -> Value {
        let mut payload = json!({
            "name": self.name,
            "planet": self.planet,
            "category": self.category,
            "waypoint": self.waypoint,
            "cooldown_hours": self.cooldown_hours,
            "reward_ped": self.reward_ped,
            "reward_is_skill": self.reward_is_skill,
            "reward_description": self.reward_description,
            "reward_item_names": self.reward_item_names,
            "notes": self.notes,
            "chain_name": self.chain_name,
            "chain_position": self.chain_position,
            "chain_total": self.chain_total,
            "mobs": self.mobs,
            "signal_loot_item": self.signal_loot_item,
            "family_id": self.family_id,
        });
        if let Some(trigger) = self.completion_trigger {
            payload["completion_trigger"] = json!(trigger.as_service_str());
        }
        if let Some(policy) = self.reward_policy {
            payload["reward_policy"] = json!(policy.as_service_str());
        }
        // The anchor column is non-nullable, so the key is sent only
        // when a value was chosen; absent keeps the stored/default
        // anchor (present-null would be a refusal, not a clear).
        if let Some(anchor) = self.cooldown_anchor {
            payload["cooldown_anchor"] = json!(anchor.as_service_str());
        }
        payload
    }
}

fn default_planet() -> String {
    "Calypso".to_string()
}

// ── Response DTOs ───────────────────────────────────────────────────

/// When a cooldown timer starts: `pickup` runs it from the last
/// recorded start (the giver hands the mission over and the slot's
/// timer begins, whatever happens after); `completion` runs it from the
/// last recorded completion (the pre-family rule, and the natural shape
/// for boss runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestCooldownAnchor {
    Pickup,
    Completion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestCompletionTrigger {
    MissionLog,
    SignalItem,
    ManualHandIn,
}

impl QuestCompletionTrigger {
    fn as_service_str(self) -> &'static str {
        match self {
            Self::MissionLog => "mission_log",
            Self::SignalItem => "signal_item",
            Self::ManualHandIn => "manual_hand_in",
        }
    }

    fn from_service(value: &Value) -> Self {
        match value.as_str() {
            Some("signal_item") => Self::SignalItem,
            Some("manual_hand_in") => Self::ManualHandIn,
            _ => Self::MissionLog,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestRewardPolicy {
    None,
    FixedPes,
    NamedItems,
    CompletionClump,
}

impl QuestRewardPolicy {
    fn as_service_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FixedPes => "fixed_pes",
            Self::NamedItems => "named_items",
            Self::CompletionClump => "completion_clump",
        }
    }

    fn from_service(value: &Value) -> Self {
        match value.as_str() {
            Some("fixed_pes") => Self::FixedPes,
            Some("named_items") => Self::NamedItems,
            Some("completion_clump") => Self::CompletionClump,
            _ => Self::None,
        }
    }
}

impl QuestCooldownAnchor {
    fn as_service_str(self) -> &'static str {
        match self {
            QuestCooldownAnchor::Pickup => "pickup",
            QuestCooldownAnchor::Completion => "completion",
        }
    }

    /// The stored vocabulary back to the typed wire form; the schema
    /// admits nothing else, so anything unexpected reads as the
    /// pre-family default rather than inventing a third state.
    fn from_service(value: &Value) -> Self {
        match value.as_str() {
            Some("pickup") => QuestCooldownAnchor::Pickup,
            _ => QuestCooldownAnchor::Completion,
        }
    }
}

/// A quest in the wire shape (`_format_quest` key for key). Ids are
/// stringified; `rewardDescription` / `notes` collapse null-or-empty to
/// `""`; `rewardIsSkill` is the boolean of the stored int flag.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Quest {
    pub id: String,
    pub name: String,
    pub category: Nullable<String>,
    pub target_mobs: Vec<String>,
    pub planet: String,
    pub waypoint: Nullable<String>,
    pub cooldown_duration_hours: Nullable<f64>,
    pub cooldown_expires_at: Nullable<String>,
    pub reward: Nullable<f64>,
    pub reward_is_skill: bool,
    pub reward_description: String,
    pub notes: String,
    pub chain_name: Nullable<String>,
    pub chain_position: Nullable<i64>,
    pub chain_total: Nullable<i64>,
    /// A fractional epoch-seconds timestamp (the tracker's clock is
    /// sub-second), null while the quest is not in progress.
    pub started_at: Nullable<f64>,
    /// The signal loot item completing this quest, null for quests on
    /// the mission-log lifecycle.
    pub signal_loot_item: Nullable<String>,
    pub completion_trigger: QuestCompletionTrigger,
    pub reward_policy: QuestRewardPolicy,
    pub reward_item_names: Vec<String>,
    /// When this quest's OWN cooldown timer starts.
    pub cooldown_anchor: QuestCooldownAnchor,
    /// The durable last-start instant (fractional epoch seconds); the
    /// pickup anchor's base fact, surviving completion and cancel.
    pub last_started_at: Nullable<f64>,
    /// The family this quest is a variant of (stringified id), null for
    /// a standalone quest.
    pub family_id: Nullable<String>,
    pub family_name: Nullable<String>,
    pub family_cooldown_duration_hours: Nullable<f64>,
    pub family_cooldown_anchor: Nullable<QuestCooldownAnchor>,
    /// The family-wide cooldown expiry: availability is the LATER of
    /// this and `cooldownExpiresAt` (the quest's own window).
    pub family_cooldown_expires_at: Nullable<String>,
    /// Whether the latest completion owns an unreversed economic reward.
    pub reward_undo_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestHandInItem {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestHandInCandidate {
    pub id: i64,
    pub observed_at: String,
    pub items: Vec<QuestHandInItem>,
    pub total_ped: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestHandInState {
    pub quest_id: i64,
    pub quest_name: String,
    pub waiting: bool,
    pub candidate: Nullable<QuestHandInCandidate>,
}

impl QuestHandInState {
    fn from_service(state: eo_services::quests::HandInState) -> Self {
        Self {
            quest_id: state.quest_id,
            quest_name: state.quest_name,
            waiting: state.waiting,
            candidate: state
                .candidate
                .map(|candidate| QuestHandInCandidate {
                    id: candidate.id,
                    observed_at: eo_services::time::to_iso_utc(candidate.observed_at),
                    items: candidate
                        .items
                        .into_iter()
                        .map(|item| QuestHandInItem {
                            item_name: item.item_name,
                            quantity: item.quantity,
                            value_ped: round_half_even(item.value_ped, 4),
                        })
                        .collect(),
                    total_ped: round_half_even(candidate.total_ped, 4),
                })
                .into(),
        }
    }
}

impl Quest {
    /// Port of `_format_quest`: shape one snake_case service quest into
    /// the wire DTO.
    fn from_service(quest: &Value) -> Self {
        Self {
            id: str_of(&quest["id"]),
            name: string_field(&quest["name"]),
            category: opt_string(&quest["category"]).into(),
            target_mobs: string_list(&quest["mobs"]),
            planet: string_field(&quest["planet"]),
            waypoint: opt_string(&quest["waypoint"]).into(),
            cooldown_duration_hours: opt_f64(&quest["cooldown_hours"]).into(),
            cooldown_expires_at: opt_string(&quest["cooldown_expires_at"]).into(),
            reward: if quest["reward_is_skill"].as_i64().unwrap_or(0) != 0 {
                opt_f64(&quest["reward_ped"])
            } else {
                None
            }
            .into(),
            reward_is_skill: quest["reward_is_skill"].as_i64().unwrap_or(0) != 0,
            reward_description: or_empty(&quest["reward_description"]),
            notes: or_empty(&quest["notes"]),
            chain_name: opt_string(&quest["chain_name"]).into(),
            chain_position: opt_i64(&quest["chain_position"]).into(),
            chain_total: opt_i64(&quest["chain_total"]).into(),
            started_at: opt_f64(&quest["started_at"]).into(),
            signal_loot_item: opt_string(&quest["signal_loot_item"]).into(),
            completion_trigger: QuestCompletionTrigger::from_service(&quest["completion_trigger"]),
            reward_policy: QuestRewardPolicy::from_service(&quest["reward_policy"]),
            reward_item_names: string_list(&quest["reward_item_names"]),
            cooldown_anchor: QuestCooldownAnchor::from_service(&quest["cooldown_anchor"]),
            last_started_at: opt_f64(&quest["last_started_at"]).into(),
            family_id: opt_i64(&quest["family_id"]).map(|id| id.to_string()).into(),
            family_name: opt_string(&quest["family_name"]).into(),
            family_cooldown_duration_hours: opt_f64(&quest["family_cooldown_hours"]).into(),
            family_cooldown_anchor: quest
                .get("family_cooldown_anchor")
                .filter(|value| !value.is_null())
                .map(QuestCooldownAnchor::from_service)
                .into(),
            family_cooldown_expires_at: opt_string(&quest["family_cooldown_expires_at"]).into(),
            reward_undo_available: quest["reward_undo_available"].as_bool().unwrap_or(false),
        }
    }
}

/// A quest-family create or update payload, in the frontend's
/// snake_case casing. One DTO serves both operations, exactly the quest
/// pattern: `name` and `planet` always bind, a null `cooldown_hours`
/// clears the gate (the family then groups without gating), and the
/// anchor binds only when chosen (the column is non-nullable).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct QuestFamilyInput {
    pub name: String,
    #[serde(default = "default_planet")]
    pub planet: String,
    #[serde(default)]
    pub cooldown_hours: Option<f64>,
    #[serde(default)]
    pub cooldown_anchor: Option<QuestCooldownAnchor>,
}

impl QuestFamilyInput {
    fn to_service_value(&self) -> Value {
        let mut payload = json!({
            "name": self.name,
            "planet": self.planet,
            "cooldown_hours": self.cooldown_hours,
        });
        if let Some(anchor) = self.cooldown_anchor {
            payload["cooldown_anchor"] = json!(anchor.as_service_str());
        }
        payload
    }
}

/// A quest family in the wire shape: the authored slot (name, planet,
/// cooldown hours + anchor) plus the derived availability picture (the
/// family-wide anchor instants and the expiry they produce) and the
/// active member count.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestFamily {
    pub id: String,
    pub name: String,
    pub planet: String,
    pub cooldown_duration_hours: Nullable<f64>,
    pub cooldown_anchor: QuestCooldownAnchor,
    /// The family's derived cooldown expiry (UTC ISO), null when ready
    /// or ungated.
    pub cooldown_expires_at: Nullable<String>,
    pub member_count: i64,
    /// The latest member start (fractional epoch seconds).
    pub last_started_at: Nullable<f64>,
    /// The latest member completion (fractional epoch seconds).
    pub last_completed_at: Nullable<f64>,
}

impl QuestFamily {
    fn from_service(family: &Value) -> Self {
        Self {
            id: str_of(&family["id"]),
            name: string_field(&family["name"]),
            planet: string_field(&family["planet"]),
            cooldown_duration_hours: opt_f64(&family["cooldown_hours"]).into(),
            cooldown_anchor: QuestCooldownAnchor::from_service(&family["cooldown_anchor"]),
            cooldown_expires_at: opt_string(&family["cooldown_expires_at"]).into(),
            member_count: family["member_count"].as_i64().unwrap_or(0),
            last_started_at: opt_f64(&family["last_started_at"]).into(),
            last_completed_at: opt_f64(&family["last_completed_at"]).into(),
        }
    }
}

/// Per-quest analytics in the wire shape (`_format_quest_analytics`).
/// The reward and cost columns are model-float coerced and rounded; the
/// session count is an integer; the markup passes through raw.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestAnalyticsRow {
    pub quest_id: String,
    pub quest_name: String,
    pub planet: String,
    pub category: Nullable<String>,
    pub recorded_completions: i64,
    pub confirmed_completions: i64,
    pub unresolved_completions: i64,
    pub total_recorded_reward_tt: f64,
    pub total_recorded_reward_pes: f64,
    pub total_recorded_item_tt: f64,
    pub total_realised_reward_markup: f64,
    pub recorded_reward_items: Vec<QuestRewardCandidate>,
    pub linked_sessions: i64,
    pub total_duration_sec: f64,
    pub total_weapon_cost: f64,
    pub total_heal_cost: f64,
    /// Consumed doses of cost-tracked items in the linked sessions.
    pub total_consumable_cost: f64,
    pub total_enhancer_cost: f64,
    pub total_armour_cost: f64,
    pub total_loot_tt: f64,
    pub total_pes: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestRewardCandidate {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedQuestReward {
    pub completion_id: i64,
    pub quest_id: String,
    pub quest_name: String,
    pub completed_at: f64,
    pub policy: Nullable<String>,
    pub reason: Nullable<String>,
    pub loot: Vec<QuestRewardCandidate>,
    pub isolated: bool,
}

impl UnresolvedQuestReward {
    fn from_service(row: &Value) -> Self {
        Self {
            completion_id: row["completion_id"].as_i64().unwrap_or(0),
            quest_id: str_of(&row["quest_id"]),
            quest_name: string_field(&row["quest_name"]),
            completed_at: row["completed_at"].as_f64().unwrap_or(0.0),
            policy: opt_string(&row["policy"]).into(),
            reason: opt_string(&row["reason"]).into(),
            loot: row["loot"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| QuestRewardCandidate {
                    item_name: string_field(&item["item_name"]),
                    quantity: item["quantity"].as_i64().unwrap_or(1).max(1),
                    value_ped: item["value"].as_f64().unwrap_or(0.0).max(0.0),
                })
                .collect(),
            isolated: row["isolated"].as_bool().unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestRewardReviewInput {
    pub completion_id: i64,
    #[serde(default)]
    pub selected_indices: Vec<i64>,
    #[serde(default)]
    pub declare_none: bool,
}

impl QuestAnalyticsRow {
    fn from_service(row: &Value) -> Self {
        Self {
            quest_id: str_of(&row["quest_id"]),
            quest_name: string_field(&row["quest_name"]),
            planet: string_field(&row["planet"]),
            category: opt_string(&row["category"]).into(),
            recorded_completions: row["recorded_completions"].as_i64().unwrap_or(0),
            confirmed_completions: row["confirmed_completions"].as_i64().unwrap_or(0),
            unresolved_completions: row["unresolved_completions"].as_i64().unwrap_or(0),
            total_recorded_reward_tt: model_float(&row["total_recorded_reward_tt"], 2),
            total_recorded_reward_pes: model_float(&row["total_recorded_reward_pes"], 2),
            total_recorded_item_tt: model_float(&row["total_recorded_item_tt"], 2),
            total_realised_reward_markup: model_float(&row["total_realised_reward_markup"], 2),
            recorded_reward_items: row["recorded_reward_items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| QuestRewardCandidate {
                    item_name: string_field(&item["item_name"]),
                    quantity: item["quantity"].as_i64().unwrap_or(1).max(1),
                    value_ped: item["value_ped"].as_f64().unwrap_or(0.0).max(0.0),
                })
                .collect(),
            linked_sessions: row["linked_sessions"].as_i64().unwrap_or(0),
            total_duration_sec: model_float(&row["total_duration"], 1),
            total_weapon_cost: model_float(&row["weapon_cost"], 4),
            total_heal_cost: model_float(&row["heal_cost"], 4),
            total_consumable_cost: model_float(&row["consumable_cost"], 4),
            total_enhancer_cost: model_float(&row["enhancer_cost"], 4),
            total_armour_cost: model_float(&row["armour_cost"], 4),
            total_loot_tt: model_float(&row["loot_tt"], 4),
            total_pes: model_float(&row["skill_tt"], 4),
        }
    }
}

// ── Value shaping helpers (ports of the hydration-layer formatters) ──

/// `str(value)`: a string passes through, a number takes its decimal
/// rendering (ids are integers in the service, strings once stringified).
fn str_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// A required string field; an absent/non-string value renders empty
/// (the columns this fronts are non-null by schema).
fn string_field(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// A nullable text column: null stays `None`, a string is `Some`.
fn opt_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn opt_f64(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn opt_i64(value: &Value) -> Option<i64> {
    value.as_i64()
}

/// `value or ""` over a nullable text column: null or the empty string
/// both render `""`.
fn or_empty(value: &Value) -> String {
    match value.as_str() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => String::new(),
    }
}

/// A list of strings (mob names).
fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(string_field)
        .collect()
}

/// A list of ids, each stringified (`str(id)`).
/// A model-declared float column: `float_field(round(value, places))`.
/// The response models coerce an integer-typed engine value to its float
/// form at serialisation (an engine zero leaves the wire as `0.0`), and
/// round floats banker's-style to the column's precision. A non-null
/// numeric value is assumed (these columns are non-null by contract).
fn model_float(value: &Value, places: usize) -> f64 {
    round_half_even(value.as_f64().unwrap_or(0.0), places)
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// All active quests, enriched with their target mobs.
    pub async fn quests_list(&self) -> Result<Vec<Quest>, ApiError> {
        let quests = self
            .quests
            .get_quests(true)
            .await
            .map_err(quest_error("quests list read"))?;
        Ok(quests.iter().map(Quest::from_service).collect())
    }

    /// One quest by id; a missing quest is the typed not-found.
    pub async fn quest_get(&self, quest_id: i64) -> Result<Quest, ApiError> {
        match self
            .quests
            .get_quest(quest_id)
            .await
            .map_err(quest_error("quest read"))?
        {
            Some(quest) => Ok(Quest::from_service(&quest)),
            None => Err(quest_not_found()),
        }
    }

    /// Create a quest.
    pub async fn quest_create(&self, input: QuestInput) -> Result<Quest, ApiError> {
        let created = self
            .quests
            .create_quest(&input.to_service_value())
            .await
            .map_err(quest_error("quest create"))?;
        Ok(Quest::from_service(&created))
    }

    /// Update a quest's fields (present-null clears a column); a missing
    /// quest is a 404.
    pub async fn quest_update(&self, quest_id: i64, input: QuestInput) -> Result<Quest, ApiError> {
        match self
            .quests
            .update_quest(quest_id, &input.to_service_value())
            .await
            .map_err(quest_error("quest update"))?
        {
            Some(updated) => Ok(Quest::from_service(&updated)),
            None => Err(quest_not_found()),
        }
    }

    /// Delete a quest; a missing quest is a 404. The prior `{"ok": true}`
    /// body retires (no consumer).
    pub async fn quest_delete(&self, quest_id: i64) -> Result<(), ApiError> {
        match self
            .quests
            .delete_quest(quest_id)
            .await
            .map_err(quest_error("quest delete"))?
        {
            true => Ok(()),
            false => Err(quest_not_found()),
        }
    }

    /// Start a quest (stamps `started_at`); a missing quest is a 404.
    pub async fn quest_start(&self, quest_id: i64) -> Result<Quest, ApiError> {
        match self
            .quests
            .start_quest(quest_id)
            .await
            .map_err(quest_error("quest start"))?
        {
            Some(quest) => Ok(Quest::from_service(&quest)),
            None => Err(quest_not_found()),
        }
    }

    /// Complete a quest (records the reward, opens the cooldown); a
    /// missing quest is a 404.
    pub async fn quest_complete(&self, quest_id: i64) -> Result<Quest, ApiError> {
        match self
            .quests
            .complete_quest(quest_id)
            .await
            .map_err(quest_error("quest complete"))?
        {
            Some(quest) => Ok(Quest::from_service(&quest)),
            None => Err(quest_not_found()),
        }
    }

    /// Open the contextual manual hand-in flow. With no retrospective
    /// candidate this arms the quest for the next raw clump.
    pub async fn quest_hand_in_begin(&self, quest_id: i64) -> Result<QuestHandInState, ApiError> {
        self.quests
            .hand_in_begin(quest_id)
            .await
            .map(QuestHandInState::from_service)
            .map_err(quest_error("quest hand-in begin"))
    }

    pub async fn quest_hand_in_state(&self, quest_id: i64) -> Result<QuestHandInState, ApiError> {
        self.quests
            .hand_in_state(quest_id)
            .await
            .map(QuestHandInState::from_service)
            .map_err(quest_error("quest hand-in state"))
    }

    pub async fn quest_hand_in_wait(
        &self,
        quest_id: i64,
        after_clump_id: i64,
    ) -> Result<QuestHandInState, ApiError> {
        self.quests
            .hand_in_wait(quest_id, after_clump_id)
            .await
            .map(QuestHandInState::from_service)
            .map_err(quest_error("quest hand-in wait"))
    }

    pub async fn quest_hand_in_cancel(&self, quest_id: i64) -> Result<(), ApiError> {
        self.quests
            .hand_in_cancel(quest_id)
            .await
            .map_err(quest_error("quest hand-in cancel"))
    }

    pub async fn quest_hand_in_confirm(
        &self,
        quest_id: i64,
        clump_id: i64,
    ) -> Result<(), ApiError> {
        self.quests
            .hand_in_confirm(quest_id, clump_id)
            .await
            .map_err(quest_error("quest hand-in confirm"))
    }

    /// Cancel a quest; `undo_reward` reverses a recorded completion. A
    /// missing quest is a 404.
    pub async fn quest_cancel(&self, quest_id: i64, undo_reward: bool) -> Result<Quest, ApiError> {
        match self
            .quests
            .cancel_quest(quest_id, undo_reward)
            .await
            .map_err(quest_error("quest cancel"))?
        {
            Some(quest) => Ok(Quest::from_service(&quest)),
            None => Err(quest_not_found()),
        }
    }

    /// The distinct mob names across active quests, for autocomplete.
    pub async fn quests_mobs(&self) -> Result<Vec<String>, ApiError> {
        self.quests
            .get_all_mob_names()
            .await
            .map_err(quest_error("quest mob names read"))
    }

    /// Per-quest analytics over curated linked sessions.
    pub async fn quests_analytics(&self) -> Result<Vec<QuestAnalyticsRow>, ApiError> {
        let rows = self
            .quests
            .get_quest_analytics()
            .await
            .map_err(quest_error("quest analytics read"))?;
        Ok(rows.iter().map(QuestAnalyticsRow::from_service).collect())
    }

    pub async fn quest_rewards_unresolved(&self) -> Result<Vec<UnresolvedQuestReward>, ApiError> {
        let rows = self
            .quests
            .unresolved_reward_reviews()
            .await
            .map_err(quest_error("unresolved quest rewards read"))?;
        Ok(rows
            .iter()
            .map(UnresolvedQuestReward::from_service)
            .collect())
    }

    pub async fn quest_reward_review(&self, input: QuestRewardReviewInput) -> Result<(), ApiError> {
        self.quests
            .resolve_reward_review(
                input.completion_id,
                &input.selected_indices,
                input.declare_none,
            )
            .await
            .map_err(quest_error("quest reward review"))
    }

    /// All active quest families, with their derived availability.
    pub async fn quest_families_list(&self) -> Result<Vec<QuestFamily>, ApiError> {
        let families = self
            .quests
            .get_families(true)
            .await
            .map_err(quest_error("quest families list read"))?;
        Ok(families.iter().map(QuestFamily::from_service).collect())
    }

    /// Create a quest family; unattached quests whose colon-split
    /// family part matches the name sweep in as members.
    pub async fn quest_family_create(
        &self,
        input: QuestFamilyInput,
    ) -> Result<QuestFamily, ApiError> {
        let created = self
            .quests
            .create_family(&input.to_service_value())
            .await
            .map_err(quest_error("quest family create"))?;
        Ok(QuestFamily::from_service(&created))
    }

    /// Update a quest family; a rename sweeps newly matching quests in.
    /// A missing family is a 404.
    pub async fn quest_family_update(
        &self,
        family_id: i64,
        input: QuestFamilyInput,
    ) -> Result<QuestFamily, ApiError> {
        match self
            .quests
            .update_family(family_id, &input.to_service_value())
            .await
            .map_err(quest_error("quest family update"))?
        {
            Some(updated) => Ok(QuestFamily::from_service(&updated)),
            None => Err(family_not_found()),
        }
    }

    /// Delete a quest family, detaching its members; a missing family
    /// is a 404.
    pub async fn quest_family_delete(&self, family_id: i64) -> Result<(), ApiError> {
        match self
            .quests
            .delete_family(family_id)
            .await
            .map_err(quest_error("quest family delete"))?
        {
            true => Ok(()),
            false => Err(family_not_found()),
        }
    }
}

/// The quests family's error mapping: every failure (invalid input,
/// driver, rollup) collapses to the one internal-error reply, an
/// inherited contract the goldens pin (there is no user-facing
/// `bad_request` on this family); the source is logged server-side while
/// the boundary reply stays fixed.
fn quest_error(context: &'static str) -> impl FnOnce(QuestError) -> ApiError {
    move |error| match error {
        // A validation refusal is the caller's to fix, not an internal
        // fault: it surfaces with its own message as a request error.
        QuestError::Invalid(message) => ApiError::bad_request(message),
        other => ApiError::internal(context)(other),
    }
}

fn quest_not_found() -> ApiError {
    ApiError::not_found("Quest not found")
}

fn family_not_found() -> ApiError {
    ApiError::not_found("Quest family not found")
}
