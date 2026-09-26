//! The equipment family: catalogue search, the library CRUD, the
//! expanded detail, and the cost shaping behind both.
//!
//! The stored `properties_json` bytes are an owned on-disk contract:
//! the writes serialise with the canonical spacing the DB-state goldens
//! pin, so stored equipment state is byte-stable. The response
//! shapes match the frontend's hand-written contract (`$lib/types/
//! equipment.ts`) field for field; where the HTTP layer passed stored
//! JSON values through untyped, the DTOs pin the number/string types
//! the writes have always produced.

use eo_services::attack_rate::{AttackRate, WeaponPricing};
use eo_services::cost_engine::{
    cost_per_shot_from_props, heal_cost_per_use, heal_cost_per_use_with_implant,
    heal_reload_seconds, is_limited, weapon_damage_profile_from_props,
};
use eo_services::equipment_pricing::{
    healing_profile_from_props, lifesteal_percent_from_props,
    lifesteal_percent_from_props_with_catalog,
};
use eo_services::expected_hunting::{
    self, HuntingLooterLevels, LooterSource, OffensiveLoadoutEvidence,
};
use eo_services::game_data_store::GameDataStore;
use eo_services::weapon_effect::{
    effect_profile_from_props, WeaponEffectProfile, EFFECT_PROFILE_KEY,
};
use eo_wire::normalizer::{round_half_even, to_python_json_dumps};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::Nullable;
use crate::{Api, ApiError};

/// A catalogue vocabulary the search accepts; each maps to one snapshot
/// endpoint. An out-of-vocabulary value cannot be constructed (the
/// bindings expose the closed union), so the old unknown-type reply
/// class is unrepresentable rather than handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchKind {
    Weapon,
    Amp,
    Healer,
    Scope,
    Absorber,
    Consumable,
    Tool,
    Implant,
}

impl SearchKind {
    fn endpoint(self) -> &'static str {
        match self {
            SearchKind::Weapon => "weapons",
            SearchKind::Amp => "weapon_amplifiers",
            SearchKind::Healer => "medical_tools",
            SearchKind::Scope => "weapon_vision_attachments",
            SearchKind::Absorber => "absorbers",
            SearchKind::Consumable => "stimulants",
            SearchKind::Tool => "harvesting_tools",
            SearchKind::Implant => "mindforce_implants",
        }
    }
}

/// The stored equipment classes. `Tool` is a harvesting tool (tree
/// cutting): a single-entity item like a healing tool, costed as pure
/// decay per swing with no ammo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EquipmentKind {
    Weapon,
    Healing,
    Consumable,
    Tool,
}

impl EquipmentKind {
    fn as_str(self) -> &'static str {
        match self {
            EquipmentKind::Weapon => "weapon",
            EquipmentKind::Healing => "healing",
            EquipmentKind::Consumable => "consumable",
            EquipmentKind::Tool => "tool",
        }
    }
}

/// A catalogue search hit.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentSearchHit {
    pub catalog_id: Nullable<String>,
    pub name: String,
    /// Decay per use, PEC.
    pub decay: f64,
    /// Ammo burn per use, PEC (catalogue units / 100).
    pub ammo_burn: f64,
    /// Decay-absorption share, percent, for absorbers/extenders and
    /// Mindforce implants; null for catalogue rows without one.
    pub absorption_percent: Nullable<f64>,
    pub is_limited: bool,
    pub heal_min: Nullable<f64>,
    pub heal_max: Nullable<f64>,
    pub reload_seconds: Nullable<f64>,
    pub lifesteal_percent: Nullable<f64>,
    /// A weapon's catalogue attack rate, attacks a minute; null for other
    /// items and for weapons the catalogue gives no rate.
    pub uses_per_minute: Nullable<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealingMode {
    #[default]
    Direct,
    OverTime,
    Compound,
}

impl HealingMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::OverTime => "over_time",
            Self::Compound => "compound",
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingProfileDto {
    pub mode: HealingMode,
    pub direct_min: Nullable<f64>,
    pub direct_max: Nullable<f64>,
    pub effect_duration_seconds: Nullable<f64>,
    pub tick_min: Nullable<f64>,
    pub tick_max: Nullable<f64>,
    pub tick_seconds: Nullable<f64>,
}

/// How a paid activation of a weapon with a damage-over-time effect shows
/// in the chat log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WeaponEffectMode {
    /// Only ticks: the first tick is the activation's outcome.
    OverTime,
    /// An initial hit, then ticks.
    Compound,
}

impl From<WeaponEffectMode> for eo_services::weapon_effect::WeaponEffectMode {
    fn from(value: WeaponEffectMode) -> Self {
        match value {
            WeaponEffectMode::OverTime => Self::OverTime,
            WeaponEffectMode::Compound => Self::Compound,
        }
    }
}

impl From<eo_services::weapon_effect::WeaponEffectMode> for WeaponEffectMode {
    fn from(value: eo_services::weapon_effect::WeaponEffectMode) -> Self {
        match value {
            eo_services::weapon_effect::WeaponEffectMode::OverTime => Self::OverTime,
            eo_services::weapon_effect::WeaponEffectMode::Compound => Self::Compound,
        }
    }
}

/// A weapon's declared damage-over-time effect: what one paid activation
/// prints, and for how long its ticks follow.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponEffectProfileDto {
    pub mode: WeaponEffectMode,
    /// The initial hit's range; null for an effect with only ticks.
    pub hit_min: Nullable<f64>,
    pub hit_max: Nullable<f64>,
    pub duration_seconds: f64,
    pub tick_min: f64,
    pub tick_max: f64,
    pub tick_seconds: Nullable<f64>,
}

fn weapon_effect_dto(props: &Value) -> Option<WeaponEffectProfileDto> {
    effect_profile_from_props(props).map(|profile| WeaponEffectProfileDto {
        mode: profile.mode.into(),
        hit_min: profile.hit_min.into(),
        hit_max: profile.hit_max.into(),
        duration_seconds: profile.duration_seconds,
        tick_min: profile.tick_min,
        tick_max: profile.tick_max,
        tick_seconds: profile.tick_seconds.into(),
    })
}

/// A weapon's damage-over-time effect as an add or update request declares
/// it, in the request's casing. Absent: the weapon's every hit is a shot.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WeaponEffectRequest {
    pub mode: WeaponEffectMode,
    #[serde(default)]
    pub hit_min: Option<f64>,
    #[serde(default)]
    pub hit_max: Option<f64>,
    pub duration_seconds: f64,
    pub tick_min: f64,
    pub tick_max: f64,
    #[serde(default)]
    pub tick_seconds: Option<f64>,
}

impl WeaponEffectRequest {
    /// The profile to store, or why it cannot be.
    fn profile(&self) -> Result<WeaponEffectProfile, ApiError> {
        WeaponEffectProfile {
            mode: self.mode.into(),
            hit_min: self.hit_min,
            hit_max: self.hit_max,
            duration_seconds: self.duration_seconds,
            tick_min: self.tick_min,
            tick_max: self.tick_max,
            tick_seconds: self.tick_seconds,
        }
        .validated()
        .map_err(|error| ApiError::bad_request(error.to_string()))
    }
}

/// A library entry in the list shape.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentSummary {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: EquipmentKind,
    pub amplifier_name: Nullable<String>,
    pub cost_per_use: f64,
    pub damage_min: Nullable<f64>,
    pub damage_max: Nullable<f64>,
    pub reload_seconds: Nullable<f64>,
    pub is_limited: bool,
    /// 1 = base item, 2 = amplified, 3 = fully accessorised.
    pub enrichment_level: i64,
    pub healing_profile: Nullable<HealingProfileDto>,
    pub lifesteal_percent: Nullable<f64>,
    /// A weapon's declared damage-over-time effect, when it has one.
    pub effect_profile: Nullable<WeaponEffectProfileDto>,
}

/// One configured component of a stored weapon setup.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentComponent {
    pub catalog_id: Nullable<String>,
    pub name: String,
    pub decay: f64,
    pub ammo_burn: f64,
    pub markup_percent: f64,
    pub is_limited: bool,
    pub damage_enhancers: i64,
    /// The component's in-game Efficiency. Null means the bundled catalogue
    /// does not carry enough evidence to model this stream.
    pub efficiency_pct: Nullable<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedLooterSource {
    Animal,
    Mutant,
    Robot,
    ThreeLooterMean,
}

impl From<LooterSource> for ExpectedLooterSource {
    fn from(value: LooterSource) -> Self {
        match value {
            LooterSource::Animal => Self::Animal,
            LooterSource::Mutant => Self::Mutant,
            LooterSource::Robot => Self::Robot,
            LooterSource::ThreeLooterMean => Self::ThreeLooterMean,
        }
    }
}

/// Unlimited-item Efficiency equivalent of the setup's premium-adjusted
/// expected return under the selected community model and looter basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentEffectiveEfficiencyStatus {
    WithinModelRange,
    BelowModelRange,
    AboveModelRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentEffectiveEfficiency {
    pub status: EquipmentEffectiveEfficiencyStatus,
    pub efficiency_pct: Nullable<f64>,
}

impl From<expected_hunting::EffectiveEfficiency> for EquipmentEffectiveEfficiency {
    fn from(value: expected_hunting::EffectiveEfficiency) -> Self {
        match value {
            expected_hunting::EffectiveEfficiency::WithinModelRange { efficiency_pct } => Self {
                status: EquipmentEffectiveEfficiencyStatus::WithinModelRange,
                efficiency_pct: Some(efficiency_pct).into(),
            },
            expected_hunting::EffectiveEfficiency::BelowModelRange => Self {
                status: EquipmentEffectiveEfficiencyStatus::BelowModelRange,
                efficiency_pct: None.into(),
            },
            expected_hunting::EffectiveEfficiency::AboveModelRange => Self {
                status: EquipmentEffectiveEfficiencyStatus::AboveModelRange,
                efficiency_pct: None.into(),
            },
        }
    }
}

/// Community-model economics for the supported offensive slice of one use.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentExpectedReturn {
    pub model_version: String,
    pub looter_source: ExpectedLooterSource,
    pub looter_level: f64,
    pub weighted_efficiency_pct: Nullable<f64>,
    pub offensive_tt_recovery: Nullable<f64>,
    pub expected_tt_rate: Nullable<f64>,
    pub effective_efficiency: Nullable<EquipmentEffectiveEfficiency>,
    pub break_even_loot_markup: Nullable<f64>,
    pub modelled_raw_tt_per_use: f64,
    pub eligible_offensive_cost_per_use: f64,
    pub consumed_premium_per_use: f64,
    pub coverage: f64,
    pub incomplete: bool,
}

/// The absorber component of a stored weapon setup.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbsorberComponent {
    pub catalog_id: Nullable<String>,
    pub name: String,
    pub decay: f64,
    pub ammo_burn: f64,
    pub absorption_percent: f64,
    pub markup_percent: f64,
    pub is_limited: bool,
}

/// One line of a cost breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostBreakdownLine {
    pub component: String,
    pub cost_pec: f64,
    pub markup_multiplier: f64,
    pub effective_cost_pec: f64,
}

/// A weapon's attack rate under the reload speed in effect. Past the
/// server's limit of 100 attacks a minute, the rate the reload speed asks
/// for turns into per-attack damage and cost instead: `factor` is that
/// multiplier (1 within the limit), already applied to the detail's cost
/// breakdown and to the weapon's damage range.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponAttackRate {
    /// The catalogue rate, attacks a minute, before any effect.
    pub base_per_minute: f64,
    /// The reload speed in effect, after the game's stacking limit.
    pub reload_speed_percent: f64,
    /// The rate the reload speed asks for.
    pub buffed_per_minute: f64,
    /// The rate the server runs: the buffed rate, held at the limit.
    pub effective_per_minute: f64,
    /// Per-attack damage and cost multiplier; 1 within the limit.
    pub factor: f64,
}

impl From<AttackRate> for WeaponAttackRate {
    fn from(rate: AttackRate) -> Self {
        Self {
            base_per_minute: rate.base_per_minute,
            reload_speed_percent: rate.reload_speed_percent,
            buffed_per_minute: rate.buffed_per_minute,
            effective_per_minute: rate.effective_per_minute,
            factor: rate.factor,
        }
    }
}

/// The expanded library-entry detail.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentDetail {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: EquipmentKind,
    pub weapon: EquipmentComponent,
    pub amplifier: Nullable<EquipmentComponent>,
    pub scope: Nullable<EquipmentComponent>,
    pub absorber: Nullable<AbsorberComponent>,
    /// The Mindforce implant powering the item, when configured; shares
    /// the absorber component shape (its absorption is the decay share it
    /// takes per use).
    pub implant: Nullable<AbsorberComponent>,
    pub cost_breakdown: Vec<CostBreakdownLine>,
    pub total_cost_per_use: f64,
    pub expected_return: Nullable<EquipmentExpectedReturn>,
    pub healing_profile: Nullable<HealingProfileDto>,
    pub lifesteal_percent: Nullable<f64>,
    /// A weapon's declared damage-over-time effect, when it has one.
    pub effect_profile: Nullable<WeaponEffectProfileDto>,
    /// A weapon's attack rate, when its catalogue publishes a base rate.
    pub attack_rate: Nullable<WeaponAttackRate>,
}

/// An add or update request. Field names stay in the request casing the
/// frontend has always sent.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EquipmentRequest {
    #[serde(rename = "type")]
    pub kind: EquipmentKind,
    #[serde(default)]
    pub catalog_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub amp_catalog_id: Option<String>,
    #[serde(default)]
    pub scope_catalog_id: Option<String>,
    #[serde(default)]
    pub absorber_catalog_id: Option<String>,
    #[serde(default = "default_markup")]
    pub weapon_markup: i64,
    #[serde(default = "default_markup")]
    pub amp_markup: i64,
    #[serde(default = "default_markup")]
    pub scope_markup: i64,
    #[serde(default = "default_markup")]
    pub absorber_markup: i64,
    #[serde(default)]
    pub damage_enhancers: i64,
    #[serde(default)]
    pub implant_catalog_id: Option<String>,
    #[serde(default = "default_markup")]
    pub implant_markup: i64,
    #[serde(default)]
    pub healing_mode: HealingMode,
    #[serde(default)]
    pub heal_min: Option<f64>,
    #[serde(default)]
    pub heal_max: Option<f64>,
    #[serde(default)]
    pub effect_duration_seconds: Option<f64>,
    #[serde(default)]
    pub tick_min: Option<f64>,
    #[serde(default)]
    pub tick_max: Option<f64>,
    #[serde(default)]
    pub tick_seconds: Option<f64>,
    /// A weapon's damage-over-time effect; absent for every other kind.
    #[serde(default)]
    pub weapon_effect: Option<WeaponEffectRequest>,
}

fn default_markup() -> i64 {
    100
}

/// Python `str.strip()`: `char::is_whitespace` plus the four
/// information-separator controls (FS/GS/RS/US) Python's `isspace`
/// includes and Rust's does not.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `entity.get(key) or 0.0` over a catalogue economy number.
fn eco_or_zero(entity: &Value, key: &str) -> f64 {
    entity
        .get("economy")
        .and_then(|eco| eco.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// Python truthiness over a JSON value.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python truthiness over an optional stored entity (`if props.get(..)`).
fn entity_truthy(props: &Value, key: &str) -> bool {
    props.get(key).map(json_truthy).unwrap_or(false)
}

/// An absorber-shaped stored entity (absorber/extender or Mindforce
/// implant) as its typed component, when present.
fn absorption_component(
    props: &Value,
    entity_key: &str,
    id_key: &str,
    markup_key: &str,
) -> Result<Option<AbsorberComponent>, ApiError> {
    match props.get(entity_key).filter(|v| !v.is_null()) {
        Some(entity) => {
            let absorption_pct = entity
                .get("economy")
                .and_then(|eco| eco.get("absorption"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                * 100.0;
            Ok(Some(AbsorberComponent {
                catalog_id: props.get(id_key).and_then(id_string).into(),
                name: entity_name(entity)?,
                decay: eco_or_zero(entity, "decay"),
                ammo_burn: eco_or_zero(entity, "ammo_burn") / 100.0,
                absorption_percent: round_half_even(absorption_pct, 1),
                markup_percent: stored_markup(props, markup_key),
                is_limited: is_limited(entity),
            }))
        }
        None => Ok(None),
    }
}

/// Enrichment level from the configured components.
fn compute_enrichment(props: &Value) -> i64 {
    if entity_truthy(props, "amp_entity") {
        if entity_truthy(props, "scope_entity") || entity_truthy(props, "absorber_entity") {
            return 3;
        }
        return 2;
    }
    1
}

/// `max(0, int(props.get("damage_enhancers", 0) or 0))`.
fn stored_enhancers(props: &Value) -> i64 {
    props
        .get("damage_enhancers")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64
}

/// A stored identifier value as its string form (catalogue ids are
/// strings in the snapshot; a numeric id keeps its decimal rendering).
fn id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A stored entity's display name; a stored setup missing it is
/// unreadable state, reported as the internal error the HTTP layer
/// answered for the same condition.
fn entity_name(entity: &Value) -> Result<String, ApiError> {
    entity
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::invalid_state("stored entity missing a string name"))
}

/// A stored markup value (`props[key] or 100`), as the number the
/// stored JSON carries.
fn stored_markup(props: &Value, key: &str) -> f64 {
    props.get(key).and_then(Value::as_f64).unwrap_or(100.0)
}

/// A catalogue search row as its typed hit.
fn search_hit(row: &Value) -> EquipmentSearchHit {
    let entity = &row["data"];
    EquipmentSearchHit {
        catalog_id: id_string(&row["item_id"]).into(),
        name: row["item_name"].as_str().unwrap_or_default().to_string(),
        decay: eco_or_zero(entity, "decay"),
        ammo_burn: eco_or_zero(entity, "ammo_burn") / 100.0,
        absorption_percent: entity
            .get("economy")
            .and_then(|eco| eco.get("absorption"))
            .and_then(Value::as_f64)
            .map(|share| round_half_even(share * 100.0, 1))
            .into(),
        is_limited: is_limited(entity),
        heal_min: entity.get("min_heal").and_then(Value::as_f64).into(),
        heal_max: entity.get("max_heal").and_then(Value::as_f64).into(),
        reload_seconds: (row["endpoint"].as_str() == Some("medical_tools"))
            .then(|| round_half_even(heal_reload_seconds(entity), 2))
            .into(),
        lifesteal_percent: entity
            .get("lifesteal_percent")
            .and_then(Value::as_f64)
            .into(),
        uses_per_minute: (row["endpoint"].as_str() == Some("weapons"))
            .then(|| entity.get("uses_per_minute").and_then(Value::as_f64))
            .flatten()
            .into(),
    }
}

fn healing_profile_dto(props: &Value) -> HealingProfileDto {
    let profile = healing_profile_from_props(&props.to_string());
    let mode = match profile.mode {
        eo_services::healing_profile::HealingMode::Direct => HealingMode::Direct,
        eo_services::healing_profile::HealingMode::OverTime => HealingMode::OverTime,
        eo_services::healing_profile::HealingMode::Compound => HealingMode::Compound,
    };
    HealingProfileDto {
        mode,
        direct_min: profile.direct_min.into(),
        direct_max: profile.direct_max.into(),
        effect_duration_seconds: profile.effect_duration_seconds.into(),
        tick_min: profile.tick_min.into(),
        tick_max: profile.tick_max.into(),
        tick_seconds: profile.tick_seconds.into(),
    }
}

fn lifesteal_for_props(props: &Value, game_data: &GameDataStore) -> Option<f64> {
    let raw = props.to_string();
    lifesteal_percent_from_props_with_catalog(&raw, game_data)
        .or_else(|| lifesteal_percent_from_props(&raw))
}

/// The typed cost lines from the cost engine's breakdown value.
fn breakdown_lines(cost_result: &Value) -> Result<Vec<CostBreakdownLine>, ApiError> {
    serde_json::from_value(cost_result["costBreakdown"].clone())
        .map_err(ApiError::internal("cost-breakdown shaping"))
}

/// Convert a library row to the list shape. The internal error mirrors
/// the unreadable-row condition the HTTP layer answered a 500 for (a
/// weapon or healing row missing its stored entity).
fn row_to_summary(
    id: i64,
    name: &str,
    item_type: &str,
    props: &Value,
    game_data: &GameDataStore,
    pricing: &WeaponPricing,
) -> Result<EquipmentSummary, ApiError> {
    if item_type == "weapon" {
        let priced_props = pricing.prepare(props);
        let props = &priced_props;
        let weapon_e = props
            .get("weapon_entity")
            .filter(|v| !v.is_null())
            .ok_or_else(|| {
                ApiError::invalid_state(format!("stored weapon row {id} missing weapon_entity"))
            })?;
        let amp_e = props.get("amp_entity").filter(|v| !v.is_null());
        let cost_result = cost_per_shot_from_props(props, None);
        let damage_profile = weapon_damage_profile_from_props(props);
        let rounded = |key: &str| -> Option<f64> {
            damage_profile
                .as_ref()
                .and_then(|profile| profile.get(key))
                .and_then(Value::as_f64)
                .map(|v| round_half_even(v, 2))
        };
        return Ok(EquipmentSummary {
            id: id.to_string(),
            name: name.to_string(),
            kind: EquipmentKind::Weapon,
            amplifier_name: amp_e
                .and_then(|amp| amp.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .into(),
            cost_per_use: cost_result["totalCostPerUse"].as_f64().unwrap_or(0.0),
            damage_min: rounded("damageMin").into(),
            damage_max: rounded("damageMax").into(),
            reload_seconds: None.into(),
            is_limited: is_limited(weapon_e),
            enrichment_level: compute_enrichment(props),
            healing_profile: None.into(),
            lifesteal_percent: lifesteal_for_props(props, game_data).into(),
            effect_profile: weapon_effect_dto(props).into(),
        });
    }

    if item_type == "consumable" {
        return Ok(EquipmentSummary {
            id: id.to_string(),
            name: name.to_string(),
            kind: EquipmentKind::Consumable,
            amplifier_name: None.into(),
            cost_per_use: 0.0,
            damage_min: None.into(),
            damage_max: None.into(),
            reload_seconds: None.into(),
            is_limited: false,
            enrichment_level: 1,
            healing_profile: None.into(),
            lifesteal_percent: None.into(),
            effect_profile: None.into(),
        });
    }

    if item_type == "tool" {
        let tool_e = props
            .get("tool_entity")
            .filter(|v| !v.is_null())
            .ok_or_else(|| {
                ApiError::invalid_state(format!("stored tool row {id} missing tool_entity"))
            })?;
        let markup = props.get("markup").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
        // A harvesting tool has no ammo, so the heal per-use recipe
        // (decay + ammo, markup-weighted) reduces to decay x markup.
        return Ok(EquipmentSummary {
            id: id.to_string(),
            name: name.to_string(),
            kind: EquipmentKind::Tool,
            amplifier_name: None.into(),
            cost_per_use: heal_cost_per_use(tool_e, markup),
            damage_min: None.into(),
            damage_max: None.into(),
            reload_seconds: None.into(),
            is_limited: is_limited(tool_e),
            enrichment_level: 1,
            healing_profile: None.into(),
            lifesteal_percent: None.into(),
            effect_profile: None.into(),
        });
    }

    let tool_e = props
        .get("tool_entity")
        .filter(|v| !v.is_null())
        .ok_or_else(|| {
            ApiError::invalid_state(format!("stored healing row {id} missing tool_entity"))
        })?;
    let markup = props.get("markup").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
    Ok(EquipmentSummary {
        id: id.to_string(),
        name: name.to_string(),
        kind: EquipmentKind::Healing,
        amplifier_name: None.into(),
        cost_per_use: heal_cost_with_stored_implant(props, tool_e, markup),
        damage_min: None.into(),
        damage_max: None.into(),
        reload_seconds: Some(round_half_even(heal_reload_seconds(tool_e), 2)).into(),
        is_limited: is_limited(tool_e),
        enrichment_level: 1,
        healing_profile: Some(healing_profile_dto(props)).into(),
        lifesteal_percent: None.into(),
        effect_profile: None.into(),
    })
}

/// Convert a library row to the expanded detail shape. `catalog_id` is
/// the row's own column.
#[allow(clippy::too_many_arguments)]
fn row_to_detail(
    id: i64,
    name: &str,
    item_type: &str,
    catalog_id: Option<&str>,
    props: &Value,
    game_data: &GameDataStore,
    pricing: &WeaponPricing,
    looters: HuntingLooterLevels,
) -> Result<EquipmentDetail, ApiError> {
    let item_id = id.to_string();

    if item_type == "weapon" {
        let enriched_props = pricing.prepare(
            &expected_hunting::with_current_offensive_efficiencies(props, game_data),
        );
        let props = &enriched_props;
        let weapon_e = props
            .get("weapon_entity")
            .filter(|v| !v.is_null())
            .ok_or_else(|| {
                ApiError::invalid_state(format!("stored weapon row {id} missing weapon_entity"))
            })?;
        let enhancers = stored_enhancers(props).max(0);
        let cost_result = cost_per_shot_from_props(props, None);

        let component = |entity_key: &str,
                         id_key: &str,
                         markup_key: &str|
         -> Result<Option<EquipmentComponent>, ApiError> {
            match props.get(entity_key).filter(|v| !v.is_null()) {
                Some(entity) => Ok(Some(EquipmentComponent {
                    catalog_id: props.get(id_key).and_then(id_string).into(),
                    name: entity_name(entity)?,
                    decay: eco_or_zero(entity, "decay"),
                    ammo_burn: eco_or_zero(entity, "ammo_burn") / 100.0,
                    markup_percent: stored_markup(props, markup_key),
                    is_limited: is_limited(entity),
                    damage_enhancers: 0,
                    efficiency_pct: entity
                        .get("economy")
                        .and_then(|economy| economy.get("efficiency"))
                        .and_then(Value::as_f64)
                        .into(),
                })),
                None => Ok(None),
            }
        };

        let absorber = absorption_component(
            props,
            "absorber_entity",
            "absorber_catalog_id",
            "absorber_markup",
        )?;

        // `props.get("weapon_catalog_id") or <row catalog_id>`.
        let weapon_catalog_id = match props.get("weapon_catalog_id").filter(|v| json_truthy(v)) {
            Some(value) => id_string(value),
            None => catalog_id.map(str::to_string),
        };
        return Ok(EquipmentDetail {
            id: item_id,
            kind: EquipmentKind::Weapon,
            weapon: EquipmentComponent {
                catalog_id: weapon_catalog_id.into(),
                name: entity_name(weapon_e)?,
                decay: eco_or_zero(weapon_e, "decay"),
                ammo_burn: eco_or_zero(weapon_e, "ammo_burn") / 100.0,
                markup_percent: stored_markup(props, "weapon_markup"),
                is_limited: is_limited(weapon_e),
                damage_enhancers: enhancers,
                efficiency_pct: weapon_e
                    .get("economy")
                    .and_then(|economy| economy.get("efficiency"))
                    .and_then(Value::as_f64)
                    .into(),
            },
            amplifier: component("amp_entity", "amp_catalog_id", "amp_markup")?.into(),
            scope: component("scope_entity", "scope_catalog_id", "scope_markup")?.into(),
            absorber: absorber.into(),
            implant: absorption_component(
                props,
                "implant_entity",
                "implant_catalog_id",
                "implant_markup",
            )?
            .into(),
            cost_breakdown: breakdown_lines(&cost_result)?,
            total_cost_per_use: cost_result["totalCostPerUse"].as_f64().unwrap_or(0.0),
            expected_return: equipment_expected_return(props, enhancers, looters)?.into(),
            healing_profile: None.into(),
            lifesteal_percent: lifesteal_for_props(props, game_data).into(),
            effect_profile: weapon_effect_dto(props).into(),
            attack_rate: pricing
                .attack_rate(props)
                .map(WeaponAttackRate::from)
                .into(),
        });
    }

    if item_type == "consumable" {
        return Ok(EquipmentDetail {
            id: item_id,
            kind: EquipmentKind::Consumable,
            weapon: EquipmentComponent {
                catalog_id: catalog_id.map(str::to_string).into(),
                name: name.to_string(),
                decay: 0.0,
                ammo_burn: 0.0,
                markup_percent: 100.0,
                is_limited: false,
                damage_enhancers: 0,
                efficiency_pct: None.into(),
            },
            amplifier: None.into(),
            scope: None.into(),
            absorber: None.into(),
            implant: None.into(),
            cost_breakdown: Vec::new(),
            total_cost_per_use: 0.0,
            expected_return: None.into(),
            healing_profile: None.into(),
            lifesteal_percent: None.into(),
            effect_profile: None.into(),
            attack_rate: None.into(),
        });
    }

    // Healing tool detail (simplified).
    let tool_e = props
        .get("tool_entity")
        .filter(|v| !v.is_null())
        .ok_or_else(|| {
            ApiError::invalid_state(format!("stored healing row {id} missing tool_entity"))
        })?;
    let markup_pct = stored_markup(props, "markup");
    let implant = props.get("implant_entity").filter(|v| !v.is_null());
    let implant_markup = stored_markup(props, "implant_markup") / 100.0;
    let cost = heal_cost_per_use_with_implant(tool_e, markup_pct / 100.0, implant, implant_markup);
    let decay = eco_or_zero(tool_e, "decay");
    // The decay the tool itself keeps after the implant takes its share
    // (identical to `decay` when no implant is configured).
    let implant_share = implant
        .and_then(|entity| entity.get("economy"))
        .and_then(|eco| eco.get("absorption"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let implant_decay = decay * implant_share;
    let tool_decay = decay - implant_decay;
    let mut breakdown = Vec::new();
    if implant_decay > 0.0 {
        breakdown.push(CostBreakdownLine {
            component: "Implant decay".to_string(),
            cost_pec: round_half_even(implant_decay, 4),
            markup_multiplier: implant_markup,
            effective_cost_pec: round_half_even(implant_decay * implant_markup, 4),
        });
    }
    breakdown.push(CostBreakdownLine {
        component: "Decay".to_string(),
        cost_pec: tool_decay,
        markup_multiplier: markup_pct / 100.0,
        effective_cost_pec: round_half_even(tool_decay * markup_pct / 100.0, 4),
    });
    let ammo_pec = eco_or_zero(tool_e, "ammo_burn") / 100.0;
    if ammo_pec > 0.0 {
        breakdown.push(CostBreakdownLine {
            component: "Ammo".to_string(),
            cost_pec: ammo_pec,
            markup_multiplier: 1.0,
            effective_cost_pec: ammo_pec,
        });
    }
    // `props.get("tool_catalog_id") or <row catalog_id>`.
    let tool_catalog_id = match props.get("tool_catalog_id").filter(|v| json_truthy(v)) {
        Some(value) => id_string(value),
        None => catalog_id.map(str::to_string),
    };
    Ok(EquipmentDetail {
        id: item_id,
        kind: if item_type == "tool" {
            EquipmentKind::Tool
        } else {
            EquipmentKind::Healing
        },
        weapon: EquipmentComponent {
            catalog_id: tool_catalog_id.into(),
            name: entity_name(tool_e)?,
            decay,
            ammo_burn: ammo_pec,
            markup_percent: markup_pct,
            is_limited: is_limited(tool_e),
            damage_enhancers: 0,
            efficiency_pct: None.into(),
        },
        amplifier: None.into(),
        scope: None.into(),
        absorber: None.into(),
        implant: absorption_component(
            props,
            "implant_entity",
            "implant_catalog_id",
            "implant_markup",
        )?
        .into(),
        cost_breakdown: breakdown,
        total_cost_per_use: cost,
        expected_return: None.into(),
        healing_profile: (item_type == "healing")
            .then(|| healing_profile_dto(props))
            .into(),
        lifesteal_percent: None.into(),
        effect_profile: None.into(),
        attack_rate: None.into(),
    })
}

fn equipment_expected_return(
    props: &Value,
    enhancers: i64,
    looters: HuntingLooterLevels,
) -> Result<Option<EquipmentExpectedReturn>, ApiError> {
    let evidence: OffensiveLoadoutEvidence =
        expected_hunting::evidence_from_equipment_props(props, Some(enhancers), looters);
    if evidence.components.is_empty() {
        return Ok(None);
    }
    let premium: f64 = evidence
        .components
        .iter()
        .filter(|component| component.efficiency_pct.is_some())
        .map(|component| component.consumed_premium_per_use)
        .sum();
    let result = expected_hunting::evaluate(&evidence)
        .map_err(ApiError::internal("equipment expected return"))?;
    Ok(Some(EquipmentExpectedReturn {
        model_version: result.model_version,
        looter_source: result.looter_source.into(),
        looter_level: result.looter_level,
        weighted_efficiency_pct: result.weighted_efficiency_pct.into(),
        offensive_tt_recovery: result.offensive_tt_recovery.into(),
        expected_tt_rate: result.expected_tt_rate.into(),
        effective_efficiency: result.effective_efficiency.map(Into::into).into(),
        break_even_loot_markup: result.break_even_loot_markup.into(),
        modelled_raw_tt_per_use: result.modelled_raw_tt * 100.0,
        eligible_offensive_cost_per_use: result.eligible_offensive_cost * 100.0,
        consumed_premium_per_use: premium * 100.0,
        coverage: result.coverage,
        incomplete: result.incomplete,
    }))
}

/// The stored props built for an add/update request.
struct BuiltProps {
    name: String,
    stored_catalog_id: Option<String>,
    props: Value,
}

impl Api {
    /// Catalogue search: substring match by display name over one
    /// vocabulary. Queries under two characters answer empty before any
    /// lookup.
    pub async fn equipment_search(
        &self,
        q: &str,
        kind: SearchKind,
    ) -> Result<Vec<EquipmentSearchHit>, ApiError> {
        if q.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let rows = self.game_data.search_entities(q, Some(kind.endpoint()), 50);
        Ok(rows.iter().map(search_hit).collect())
    }

    /// The stored library, oldest first.
    pub async fn equipment_library(&self) -> Result<Vec<EquipmentSummary>, ApiError> {
        let rows = self
            .db
            .equipment_library_rows()
            .await
            .map_err(ApiError::internal("equipment library read"))?;
        let mut results = Vec::with_capacity(rows.len());
        for (id, name, item_type, raw_props) in rows {
            results.push(summary_from_parts(
                id,
                &name,
                &item_type,
                &raw_props,
                &self.game_data,
                &self.weapon_pricing,
            )?);
        }
        Ok(results)
    }

    /// Store a new library entry.
    pub async fn equipment_add(
        &self,
        req: &EquipmentRequest,
    ) -> Result<EquipmentSummary, ApiError> {
        let built = self.build_props(req)?;
        let props_json = to_python_json_dumps(&built.props);
        let inserted = self
            .db
            .insert_equipment(
                built.name,
                req.kind.as_str().to_string(),
                built.stored_catalog_id,
                props_json,
            )
            .await
            .map_err(ApiError::internal("equipment insert"))?;
        let Some((id, name, item_type, raw_props)) = self
            .db
            .equipment_row(inserted)
            .await
            .map_err(ApiError::internal("inserted equipment read-back"))?
        else {
            // The row was just inserted; its absence is a driver-level
            // invariant break.
            return Err(ApiError::invalid_state(
                "inserted equipment read-back failed",
            ));
        };
        summary_from_parts(
            id,
            &name,
            &item_type,
            &raw_props,
            &self.game_data,
            &self.weapon_pricing,
        )
    }

    /// Replace a stored entry's configuration; its class is fixed.
    pub async fn equipment_update(
        &self,
        item_id: i64,
        req: &EquipmentRequest,
    ) -> Result<EquipmentSummary, ApiError> {
        let existing_type: Option<String> = self
            .db
            .equipment_item_type(item_id)
            .await
            .map_err(ApiError::internal("equipment row lookup"))?;
        let Some(existing_type) = existing_type else {
            return Err(ApiError::not_found(format!(
                "Equipment item {item_id} not found"
            )));
        };
        if existing_type != req.kind.as_str() {
            return Err(ApiError::bad_request("Cannot change equipment type"));
        }
        let built = self.build_props(req)?;
        let props_json = to_python_json_dumps(&built.props);
        self.db
            .update_equipment(item_id, built.name, built.stored_catalog_id, props_json)
            .await
            .map_err(ApiError::internal("equipment update"))?;
        let Some((id, name, item_type, raw_props)) = self
            .db
            .equipment_row(item_id)
            .await
            .map_err(ApiError::internal("updated equipment read-back"))?
        else {
            // The row's existence was checked before the update; its
            // absence is a driver-level invariant break.
            return Err(ApiError::invalid_state(
                "updated equipment read-back failed",
            ));
        };
        summary_from_parts(
            id,
            &name,
            &item_type,
            &raw_props,
            &self.game_data,
            &self.weapon_pricing,
        )
    }

    /// Delete a stored entry. Idempotent over a missing row. A hotbar slot
    /// or carried-weapon entry naming it simply stops resolving: the
    /// tracker skips ids the library no longer holds.
    pub async fn equipment_delete(&self, item_id: i64) -> Result<(), ApiError> {
        self.db
            .delete_equipment(item_id)
            .await
            .map_err(ApiError::internal("equipment delete"))?;
        Ok(())
    }

    /// The expanded detail for a stored entry.
    pub async fn equipment_detail(&self, item_id: i64) -> Result<EquipmentDetail, ApiError> {
        self.equipment_detail_with_looters(item_id, None).await
    }

    /// Internal batch seam for callers that already derived the exact three
    /// hunting looters. Avoids repeating the character calculation per row.
    pub(crate) async fn equipment_detail_with_looters(
        &self,
        item_id: i64,
        supplied_looters: Option<HuntingLooterLevels>,
    ) -> Result<EquipmentDetail, ApiError> {
        let row = self
            .db
            .equipment_detail_row(item_id)
            .await
            .map_err(ApiError::internal("equipment detail lookup"))?;
        let Some((id, name, item_type, catalog_id, raw_props)) = row else {
            return Err(ApiError::not_found(format!(
                "Equipment item {item_id} not found"
            )));
        };
        let props = serde_json::from_str::<Value>(&raw_props)
            .map_err(ApiError::internal("stored equipment props parse"))?;
        let looters = if item_type == "weapon" {
            if let Some(looters) = supplied_looters {
                looters
            } else {
                let professions = self.character_professions().await?;
                let level = |name: &str| {
                    professions
                        .iter()
                        .find(|profession| profession.name == name)
                        .map(|profession| profession.level)
                        .unwrap_or(0.0)
                };
                HuntingLooterLevels {
                    animal: level("Animal Looter"),
                    mutant: level("Mutant Looter"),
                    robot: level("Robot Looter"),
                }
            }
        } else {
            HuntingLooterLevels {
                animal: 0.0,
                mutant: 0.0,
                robot: 0.0,
            }
        };
        row_to_detail(
            id,
            &name,
            &item_type,
            catalog_id.as_deref(),
            &props,
            &self.game_data,
            &self.weapon_pricing,
            looters,
        )
    }

    /// `_fetch_entity`: catalogue lookup with the not-found contract.
    fn fetch_entity(&self, endpoint: &str, item_id: &str) -> Result<Value, ApiError> {
        let id_value = Value::String(item_id.to_string());
        match self.game_data.find_entity(endpoint, &id_value) {
            Some(entity) => Ok(entity.clone()),
            None => Err(ApiError::not_found(format!(
                "Entity '{item_id}' not found in catalogue endpoint '{endpoint}'."
            ))),
        }
    }

    /// Build the stored props for an add/update request, reproducing
    /// the route-order validation (missing catalogue id refusals,
    /// catalogue misses, the consumable identity rule).
    fn build_props(&self, req: &EquipmentRequest) -> Result<BuiltProps, ApiError> {
        match req.kind {
            EquipmentKind::Weapon => {
                let catalog_id = req
                    .catalog_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| ApiError::bad_request("catalog_id required for weapon"))?;
                let weapon_e = self.fetch_entity("weapons", catalog_id)?;
                let optional = |endpoint: &str, id: Option<&str>| -> Result<Value, ApiError> {
                    match id.filter(|v| !v.is_empty()) {
                        Some(id) => self.fetch_entity(endpoint, id),
                        None => Ok(Value::Null),
                    }
                };
                let amp_e = optional("weapon_amplifiers", req.amp_catalog_id.as_deref())?;
                let scope_e =
                    optional("weapon_vision_attachments", req.scope_catalog_id.as_deref())?;
                let absorber_e = optional("absorbers", req.absorber_catalog_id.as_deref())?;
                let name = weapon_e["name"].as_str().unwrap_or_default().to_string();
                let mut props = Map::new();
                props.insert("weapon_entity".into(), weapon_e);
                props.insert("weapon_catalog_id".into(), json!(catalog_id));
                props.insert("amp_entity".into(), amp_e);
                props.insert("amp_catalog_id".into(), json!(req.amp_catalog_id));
                props.insert("scope_entity".into(), scope_e);
                props.insert("scope_catalog_id".into(), json!(req.scope_catalog_id));
                props.insert("absorber_entity".into(), absorber_e);
                props.insert("absorber_catalog_id".into(), json!(req.absorber_catalog_id));
                props.insert("weapon_markup".into(), json!(req.weapon_markup));
                props.insert("amp_markup".into(), json!(req.amp_markup));
                props.insert("scope_markup".into(), json!(req.scope_markup));
                props.insert("absorber_markup".into(), json!(req.absorber_markup));
                props.insert(
                    "damage_enhancers".into(),
                    json!(req.damage_enhancers.max(0)),
                );
                // Implant keys are stored only when one is configured, so an
                // implant-free write keeps the byte shape the DB-state
                // goldens pin.
                let implant_e = optional("mindforce_implants", req.implant_catalog_id.as_deref())?;
                if !implant_e.is_null() {
                    props.insert("implant_entity".into(), implant_e);
                    props.insert("implant_catalog_id".into(), json!(req.implant_catalog_id));
                    props.insert("implant_markup".into(), json!(req.implant_markup));
                }
                // Likewise the damage-over-time effect: stored only when
                // declared, validated as attribution will read it.
                if let Some(effect) = &req.weapon_effect {
                    let profile = effect.profile()?;
                    props.insert(
                        EFFECT_PROFILE_KEY.into(),
                        serde_json::to_value(profile)
                            .map_err(ApiError::internal("weapon effect encode"))?,
                    );
                }
                Ok(BuiltProps {
                    name,
                    stored_catalog_id: Some(catalog_id.to_string()),
                    props: Value::Object(props),
                })
            }
            EquipmentKind::Healing => {
                let catalog_id = req
                    .catalog_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| ApiError::bad_request("catalog_id required for healing"))?;
                let tool_e = self.fetch_entity("medical_tools", catalog_id)?;
                let name = tool_e["name"].as_str().unwrap_or_default().to_string();
                let direct_min = req
                    .heal_min
                    .or_else(|| tool_e.get("min_heal").and_then(Value::as_f64));
                let direct_max = req
                    .heal_max
                    .or_else(|| tool_e.get("max_heal").and_then(Value::as_f64));
                validate_healing_profile(
                    req.healing_mode,
                    direct_min,
                    direct_max,
                    req.effect_duration_seconds,
                    req.tick_min,
                    req.tick_max,
                    req.tick_seconds,
                )?;
                let mut props = Map::new();
                props.insert("tool_entity".into(), tool_e);
                props.insert("tool_catalog_id".into(), json!(catalog_id));
                props.insert("markup".into(), json!(req.weapon_markup));
                props.insert(
                    "healing_profile".into(),
                    json!({
                        "mode": req.healing_mode.as_str(),
                        "direct_min": direct_min,
                        "direct_max": direct_max,
                        "effect_duration_seconds": req.effect_duration_seconds,
                        "tick_min": req.tick_min,
                        "tick_max": req.tick_max,
                        "tick_seconds": req.tick_seconds,
                    }),
                );
                let implant_e = match req
                    .implant_catalog_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                {
                    Some(id) => self.fetch_entity("mindforce_implants", id)?,
                    None => Value::Null,
                };
                if !implant_e.is_null() {
                    props.insert("implant_entity".into(), implant_e);
                    props.insert("implant_catalog_id".into(), json!(req.implant_catalog_id));
                    props.insert("implant_markup".into(), json!(req.implant_markup));
                }
                Ok(BuiltProps {
                    name,
                    stored_catalog_id: Some(catalog_id.to_string()),
                    props: Value::Object(props),
                })
            }
            EquipmentKind::Tool => {
                let catalog_id = req
                    .catalog_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| ApiError::bad_request("catalog_id required for tool"))?;
                let tool_e = self.fetch_entity("harvesting_tools", catalog_id)?;
                let name = tool_e["name"].as_str().unwrap_or_default().to_string();
                let mut props = Map::new();
                props.insert("tool_entity".into(), tool_e);
                props.insert("tool_catalog_id".into(), json!(catalog_id));
                props.insert("markup".into(), json!(req.weapon_markup));
                Ok(BuiltProps {
                    name,
                    stored_catalog_id: Some(catalog_id.to_string()),
                    props: Value::Object(props),
                })
            }
            EquipmentKind::Consumable => {
                // Catalogue pick or free-text name.
                if let Some(catalog_id) = req.catalog_id.as_deref().filter(|id| !id.is_empty()) {
                    let entity = self.fetch_entity("stimulants", catalog_id)?;
                    let name = entity["name"].as_str().unwrap_or_default().to_string();
                    let mut props = Map::new();
                    props.insert("catalog_id".into(), json!(catalog_id));
                    props.insert("entity".into(), entity);
                    return Ok(BuiltProps {
                        name,
                        stored_catalog_id: Some(catalog_id.to_string()),
                        props: Value::Object(props),
                    });
                }
                if let Some(name) = req
                    .name
                    .as_deref()
                    .map(py_strip)
                    .filter(|name| !name.is_empty())
                {
                    let mut props = Map::new();
                    props.insert("catalog_id".into(), Value::Null);
                    props.insert("entity".into(), Value::Null);
                    return Ok(BuiltProps {
                        name: name.to_string(),
                        stored_catalog_id: None,
                        props: Value::Object(props),
                    });
                }
                Err(ApiError::bad_request(
                    "Consumable requires either catalog_id (catalogue pick) or name (custom)",
                ))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_healing_profile(
    mode: HealingMode,
    direct_min: Option<f64>,
    direct_max: Option<f64>,
    effect_duration_seconds: Option<f64>,
    tick_min: Option<f64>,
    tick_max: Option<f64>,
    tick_seconds: Option<f64>,
) -> Result<(), ApiError> {
    let values = [
        direct_min,
        direct_max,
        effect_duration_seconds,
        tick_min,
        tick_max,
        tick_seconds,
    ];
    if values
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(ApiError::bad_request(
            "Healing profile values must be finite and non-negative",
        ));
    }
    if direct_min
        .zip(direct_max)
        .is_some_and(|(min, max)| min > max)
        || tick_min.zip(tick_max).is_some_and(|(min, max)| min > max)
    {
        return Err(ApiError::bad_request(
            "Healing profile minimum cannot exceed its maximum",
        ));
    }
    if matches!(mode, HealingMode::Direct | HealingMode::Compound)
        && (direct_min.is_none() || direct_max.is_none())
    {
        return Err(ApiError::bad_request(
            "Direct and compound healing require a direct output interval",
        ));
    }
    if matches!(mode, HealingMode::OverTime | HealingMode::Compound)
        && (effect_duration_seconds.is_none_or(|value| value <= 0.0)
            || tick_min.is_none()
            || tick_max.is_none())
    {
        return Err(ApiError::bad_request(
            "Over-time and compound healing require an effect duration and tick interval",
        ));
    }
    if tick_seconds.is_some_and(|value| value <= 0.0) {
        return Err(ApiError::bad_request(
            "Healing tick cadence must be greater than zero",
        ));
    }
    Ok(())
}

/// The healing per-use cost over a row's stored props: the tool at its
/// markup plus any configured implant at its own.
fn heal_cost_with_stored_implant(props: &Value, tool_e: &Value, markup: f64) -> f64 {
    heal_cost_per_use_with_implant(
        tool_e,
        markup,
        props.get("implant_entity").filter(|v| !v.is_null()),
        stored_markup(props, "implant_markup") / 100.0,
    )
}

/// Shape one library row (id, name, item_type, properties_json) to the
/// list form.
fn summary_from_parts(
    id: i64,
    name: &str,
    item_type: &str,
    raw_props: &str,
    game_data: &GameDataStore,
    pricing: &WeaponPricing,
) -> Result<EquipmentSummary, ApiError> {
    let props = serde_json::from_str::<Value>(raw_props)
        .map_err(ApiError::internal("stored equipment props parse"))?;
    row_to_summary(id, name, item_type, &props, game_data, pricing)
}
