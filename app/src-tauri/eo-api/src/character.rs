//! The character family: calibration status, stats, skills, professions,
//! the Prospect forecast family, and the profession / path / HP
//! optimizers, all computed from the calibrated skill levels plus the
//! bundled game-data catalogue through the calculation services
//! (`eo-services`).
//!
//! The family is read-only: no stored bytes change. The response shapes
//! match the frontend's hand-written contract (`$lib/types/analytics.ts`)
//! field for field, expressed directly by the DTOs' declared field order
//! and `f64` typing; no separate projection pass exists.
//!
//! Contract lineage (ADR-0017/0019): two response shapes moved
//! deliberately at the typed-command crossing: the Prospect and path-optimizer *not-found* soft
//! errors converge on their families' full error shape (they were a
//! minimal three-key body), and the Prospect `sample` drops the internal
//! `skillShares` / `attributeRates` maps it leaked (computation
//! intermediates no consumer read). The legacy `GET /api/character/codex`
//! skill-progress list retires unconverted: it has no frontend caller,
//! exactly as the equipment cost endpoint retired with its family.

use eo_services::activity_recommender::{
    activity_recommender, ActivityProjection, RecommenderTarget, RECOMMENDER_PES_CAP,
    RECOMMENDER_SAMPLE_STEP,
};
use eo_services::character_calc::{
    all_profession_levels, effective_points, hp_skill_optimizer, is_attribute, profession_level,
    profession_path_optimizer, profession_skill_optimizer, skill_rank,
};
use eo_services::db::DbError;
use eo_services::time::{naive_to_epoch, to_iso_utc};
use eo_services::tt_value_curve::{levels_for_tt_value, tt_value_at};
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::Nullable;
use crate::{Api, ApiError};

/// Skills are considered stale after 30 days without recalibration.
const STALE_DAYS: f64 = 30.0;
const PROSPECT_SAMPLE_WARN_SESSIONS: i64 = 3;
const PROSPECT_SAMPLE_WARN_HOURS: f64 = 2.0;
const PROSPECT_SAMPLE_WARN_CYCLED_PED: f64 = 50.0;

// ── Query arguments ─────────────────────────────────────────────────

/// The slice a Prospect forecast aggregates over. A closed vocabulary:
/// the bindings expose only these four, so the old out-of-vocabulary 422
/// is unrepresentable rather than validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProspectSliceType {
    Global,
    Tag,
    Mob,
    Weapon,
}

impl ProspectSliceType {
    fn as_str(self) -> &'static str {
        match self {
            ProspectSliceType::Global => "global",
            ProspectSliceType::Tag => "tag",
            ProspectSliceType::Mob => "mob",
            ProspectSliceType::Weapon => "weapon",
        }
    }
}

/// The Prospect forecast query. `sliceValue` is required for every slice
/// but `global`; `markupUplift` defaults to zero.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProspectQuery {
    pub profession: String,
    pub target_level: f64,
    pub slice_type: ProspectSliceType,
    #[serde(default)]
    pub slice_value: Option<String>,
    #[serde(default)]
    pub markup_uplift: f64,
}

// ── Response DTOs ───────────────────────────────────────────────────

/// GET calibration: whether skills are calibrated and how fresh.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationStatus {
    pub calibrated: bool,
    pub last_calibration: Nullable<String>,
    pub stale: bool,
}

/// One of the top professions on the stats card: the trimmed shape the
/// card renders (name, level, category), not the full profession row.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatProfession {
    pub name: String,
    pub level: f64,
    pub category: String,
}

/// GET stats: current HP and the top five professions.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputedCharacterStats {
    pub hp: i64,
    pub top_professions: Vec<StatProfession>,
}

/// One calibrated skill row.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillLevel {
    pub name: String,
    pub category: String,
    pub level: f64,
    pub anchor_level: Nullable<f64>,
    pub gain_since_anchor: Nullable<f64>,
    pub rank_name: String,
    pub tt_value: f64,
    pub is_attribute: bool,
}

/// One profession row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfessionLevel {
    pub name: String,
    pub level: f64,
    pub anchor_level: Nullable<f64>,
    pub gain_since_anchor: Nullable<f64>,
    pub category: String,
}

/// One grouped Prospect slice option.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProspectOption {
    pub value: String,
    pub label: String,
    pub sessions: i64,
    pub kills: i64,
    pub hours: f64,
    pub cycled_ped: f64,
}

/// GET prospect-options: the grouped slice options, one list per axis.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CharacterProspectOptions {
    pub tags: Vec<ProspectOption>,
    pub mobs: Vec<ProspectOption>,
    pub weapons: Vec<ProspectOption>,
}

/// The observed sample a Prospect forecast projects from. The internal
/// `skillShares` / `attributeRates` maps are computed but not surfaced
/// (no consumer reads them; dropped with the migration).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProspectSample {
    pub sessions: i64,
    pub kills: i64,
    pub hours: f64,
    pub cycled_ped: f64,
    pub loot_tt: f64,
    pub pes: f64,
    pub attribute_levels: f64,
    pub cycled_per_hour: f64,
    pub loot_per_hour: f64,
    pub return_rate: f64,
    pub pes_per_ped: f64,
    pub loot_tt_per_ped: f64,
}

/// One skill/attribute row of a Prospect forecast.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProspectRow {
    pub name: String,
    pub is_attribute: bool,
    pub weight: f64,
    pub current_level: f64,
    pub observed_share: f64,
    pub observed_rate: f64,
    pub projected_gain: f64,
    pub projected_end_level: f64,
    pub profession_contribution: f64,
    pub relevant: bool,
}

/// GET prospect: the forecast. `error` is present only on the soft-error
/// paths (the frontend renders it inline rather than throwing); every
/// other field is always present, so the declared order below is the
/// wire order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProspectResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rows: Vec<ProspectRow>,
    pub warnings: Vec<String>,
    pub profession: String,
    pub slice_type: String,
    pub slice_value: Nullable<String>,
    pub markup_uplift: f64,
    pub current_level: f64,
    pub target_level: f64,
    pub projected_cycled_ped: f64,
    pub projected_hours: f64,
    pub expected_loot_tt: f64,
    pub expected_net_tt_burn: f64,
    pub speculative_loot_tt: Nullable<f64>,
    pub speculative_net_tt_burn: Nullable<f64>,
    pub sample: ProspectSample,
}

/// One skill row of the profession optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerSkill {
    pub name: String,
    pub weight: f64,
    pub current_level: f64,
    pub levels_needed: f64,
    pub ped_to_next_level: f64,
    pub codex_category: Nullable<String>,
    pub codex_divisor: Nullable<f64>,
}

/// One attribute row of the profession / path optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerAttribute {
    pub name: String,
    pub weight: f64,
    pub current_level: f64,
    pub contribution_factor: f64,
}

/// GET profession-optimizer: the cheapest-skill breakdown to the next
/// profession level. On a missing profession the declared tail fields
/// stay unset and only `error` accompanies the empty lists.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfessionOptimizerResult {
    pub skills: Vec<OptimizerSkill>,
    pub attributes: Vec<OptimizerAttribute>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profession: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_level: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_level: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One allocation of the path optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathAllocation {
    pub name: String,
    pub weight: f64,
    pub current_level: f64,
    pub levels_to_gain: f64,
    pub ped_cost: f64,
    pub new_level: f64,
    pub codex_category: Nullable<String>,
    pub codex_divisor: Nullable<f64>,
}

/// A skill the path optimizer left out, with the reason.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedSkill {
    pub name: String,
    pub weight: f64,
    pub reason: String,
}

/// GET profession-path-optimizer: the greedy allocation for a target
/// level or a PED budget. `inputTargetLevel` / `inputPedBudget` echo the
/// mode (exactly one is non-null); `error` marks a missing profession.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathOptimizerResult {
    pub allocations: Vec<PathAllocation>,
    pub attributes: Vec<OptimizerAttribute>,
    pub profession: String,
    pub mode: String,
    pub input_target_level: Nullable<f64>,
    pub input_ped_budget: Nullable<f64>,
    pub current_level: f64,
    pub end_level: f64,
    pub profession_levels_gained: f64,
    pub total_ped: f64,
    pub excluded: Vec<ExcludedSkill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One skill row of the HP optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HpOptimizerSkill {
    pub name: String,
    pub hp_increase: f64,
    pub current_level: f64,
    pub levels_per_hp: f64,
    pub ped_per_hp: f64,
    pub hp_per_ped: f64,
    pub codex_category: Nullable<String>,
    pub codex_divisor: Nullable<f64>,
}

/// One attribute row of the HP optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HpOptimizerAttribute {
    pub name: String,
    pub hp_increase: f64,
    pub current_level: f64,
    pub levels_per_hp: f64,
}

/// GET hp-optimizer: the HP-per-PED breakdown across contributing
/// skills and attributes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HpOptimizerResult {
    pub current_hp: f64,
    pub skills: Vec<HpOptimizerSkill>,
    pub attributes: Vec<HpOptimizerAttribute>,
}

/// What the activity recommender optimises toward. A closed vocabulary:
/// the bindings expose only these two, so an out-of-vocabulary target is
/// unrepresentable rather than validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecommenderTargetKind {
    Hp,
    Profession,
}

/// The activity-recommender query. `professions` carries the target
/// profession name(s) for a `profession` target (one name, or several
/// for a family) and is ignored for `hp`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecommenderQuery {
    pub target: RecommenderTargetKind,
    #[serde(default)]
    pub professions: Vec<String>,
}

/// One skill's share of a recommended activity's projected gain.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecommenderContribution {
    pub name: String,
    pub current_level: f64,
    pub level_gain: f64,
    pub target_gain: f64,
}

/// One activity's projection toward the recommender target.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecommenderActivity {
    pub activity: String,
    pub professions: Vec<String>,
    pub pes_to_plus_one: Nullable<f64>,
    pub gain_at_cap: f64,
    pub series: Vec<f64>,
    pub contributors: Vec<RecommenderContribution>,
}

/// GET activity-recommender: candidates ranked by PES-to-+1 on the
/// target, plus the direct-grind reference for single-profession
/// targets. `error` is present only on the soft-error path (an unknown
/// profession name), matching the family's inline-render convention.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecommenderResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub pes_cap: f64,
    pub sample_step: f64,
    pub direct: Nullable<RecommenderActivity>,
    pub candidates: Vec<RecommenderActivity>,
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// Calibration status: believed-latest calibration timestamp and its
    /// staleness against the injected clock.
    pub async fn character_calibration(&self) -> Result<CalibrationStatus, ApiError> {
        let last_ts = self
            .last_calibration_ts()
            .await
            .map_err(ApiError::internal("calibration timestamp read"))?;
        let Some(last_ts) = last_ts else {
            return Ok(CalibrationStatus {
                calibrated: false,
                last_calibration: None.into(),
                stale: true,
            });
        };
        let age_days = (naive_to_epoch(self.clock.now()) - last_ts) / 86400.0;
        Ok(CalibrationStatus {
            calibrated: true,
            last_calibration: Some(to_iso_utc(last_ts)).into(),
            stale: age_days > STALE_DAYS,
        })
    }

    /// Current HP (Python `int()` truncation of the `Health` skill) and
    /// the top five professions by level.
    pub async fn character_stats(&self) -> Result<ComputedCharacterStats, ApiError> {
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("stats skill calibrations"))?;
        let hp = skill_levels
            .get("Health")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;

        let professions_data = self.game_data.get_entities("professions");
        let levels_by_name = all_profession_levels(&skill_levels, professions_data);
        let mut top_professions: Vec<StatProfession> = Vec::new();
        for prof in professions_data {
            let Some(name) = prof.get("name").and_then(Value::as_str) else {
                continue;
            };
            let level = levels_by_name
                .get(name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if level > 0.0 {
                top_professions.push(StatProfession {
                    name: name.to_string(),
                    level,
                    category: prof
                        .get("category")
                        .and_then(Value::as_str)
                        .unwrap_or("General")
                        .to_string(),
                });
            }
        }
        sort_desc_by(&mut top_professions, |p| p.level);
        top_professions.truncate(5);
        Ok(ComputedCharacterStats {
            hp,
            top_professions,
        })
    }

    /// The calibrated skills, believed-current levels with scan-anchored
    /// gains, ranks, and TT valuations, ordered by level descending.
    pub async fn character_skills(&self) -> Result<Vec<SkillLevel>, ApiError> {
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("skills skill calibrations"))?;
        if skill_levels.is_empty() {
            return Ok(Vec::new());
        }
        let anchor_levels = self
            .skill_calibrations(Some("scan"))
            .await
            .map_err(ApiError::internal("skills anchor calibrations"))?;
        let skills_data = self.game_data.get_entities("skills");
        let ranks = get_ranks(&self.game_data);

        let mut result: Vec<SkillLevel> = Vec::new();
        for (name, level_value) in &skill_levels {
            let level = level_value.as_f64().unwrap_or(0.0);
            let entity = skills_data
                .iter()
                .find(|s| s.get("name").and_then(Value::as_str) == Some(name.as_str()));
            let category = entity
                .and_then(|e| e.get("category"))
                .filter(|c| json_truthy(c))
                .and_then(Value::as_object)
                .and_then(|c| c.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("General")
                .to_string();
            let anchor = anchor_levels.get(name).and_then(Value::as_f64);
            let gain = anchor.map(|a| round_half_even(level - a, 4));
            result.push(SkillLevel {
                name: name.clone(),
                category,
                level,
                anchor_level: anchor.into(),
                gain_since_anchor: gain.into(),
                rank_name: skill_rank(level, &ranks),
                tt_value: round_half_even(tt_value_at(level), 2),
                is_attribute: is_attribute(name),
            });
        }
        sort_desc_by(&mut result, |s| s.level);
        Ok(result)
    }

    /// The professions, believed-current levels with scan-anchored
    /// gains, ordered by level descending.
    pub async fn character_professions(&self) -> Result<Vec<ProfessionLevel>, ApiError> {
        let professions_data = self.game_data.get_entities("professions");
        if professions_data.is_empty() {
            return Ok(Vec::new());
        }
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("professions skill calibrations"))?;
        let anchor_skills = self
            .skill_calibrations(Some("scan"))
            .await
            .map_err(ApiError::internal("professions anchor calibrations"))?;
        let current_levels = all_profession_levels(&skill_levels, professions_data);
        let anchor_levels = all_profession_levels(&anchor_skills, professions_data);
        let has_anchor = !anchor_skills.is_empty();

        let mut result: Vec<ProfessionLevel> = Vec::new();
        for prof in professions_data {
            let Some(name) = prof.get("name").and_then(Value::as_str) else {
                continue;
            };
            let level = current_levels
                .get(name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let anchor = if has_anchor {
                Some(
                    anchor_levels
                        .get(name)
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                )
            } else {
                None
            };
            let gain = anchor.map(|a| round_half_even(level - a, 4));
            result.push(ProfessionLevel {
                name: name.to_string(),
                level,
                anchor_level: anchor.into(),
                gain_since_anchor: gain.into(),
                category: prof
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or("General")
                    .to_string(),
            });
        }
        sort_desc_by(&mut result, |p| p.level);
        Ok(result)
    }

    /// The grouped Prospect slice options over the recorded sessions.
    pub async fn character_prospect_options(&self) -> Result<CharacterProspectOptions, ApiError> {
        let sessions = eo_services::session_summary::load_prospect_sessions(&self.db)
            .await
            .map_err(ApiError::internal("prospect-options sessions load"))?;
        Ok(CharacterProspectOptions {
            tags: prospect_option_list(&sessions, "dominantTag"),
            mobs: prospect_option_list(&sessions, "dominantMob"),
            weapons: prospect_option_list(&sessions, "dominantWeapon"),
        })
    }

    /// The Prospect forecast for a profession and target level over an
    /// observed session slice.
    pub async fn character_prospect(
        &self,
        query: &ProspectQuery,
    ) -> Result<ProspectResult, ApiError> {
        // The value validations performed before dispatch; the slice-type
        // vocabulary is a closed enum, so an out-of-vocabulary slice is
        // unrepresentable rather than checked.
        if query.target_level <= 0.0 {
            return Err(ApiError::bad_request("target_level must be positive"));
        }
        if query.markup_uplift < 0.0 {
            return Err(ApiError::bad_request(
                "markup_uplift must be zero or positive",
            ));
        }
        let slice_type = query.slice_type.as_str();
        if query.slice_type != ProspectSliceType::Global
            && query.slice_value.as_deref().is_none_or(str::is_empty)
        {
            return Err(ApiError::bad_request(
                "slice_value is required for non-global slices",
            ));
        }

        let profession_entity = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|prof| {
                prof.get("name").and_then(Value::as_str) == Some(query.profession.as_str())
            })
            .cloned();
        let Some(profession_entity) = profession_entity else {
            // A missing profession converges on the full error shape
            // (was a minimal {error, rows, warnings}); ratified.
            return Ok(prospect_error_result(
                &query.profession,
                slice_type,
                &query.slice_value,
                query.markup_uplift,
                0.0,
                query.target_level,
                prospect_sample(&[]),
                &format!("Profession '{}' not found", query.profession),
            ));
        };
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("prospect skill calibrations"))?;
        let sessions = eo_services::session_summary::load_prospect_sessions(&self.db)
            .await
            .map_err(ApiError::internal("prospect sessions load"))?;
        let matched = match_prospect_sessions(&sessions, slice_type, &query.slice_value);
        let sample = prospect_sample(&matched);
        Ok(build_prospect_result(
            &query.profession,
            &profession_entity,
            &skill_levels,
            query.target_level,
            sample,
            slice_type,
            &query.slice_value,
            query.markup_uplift,
        ))
    }

    /// The profession optimizer: cheapest skill allocation to the next
    /// profession level.
    pub async fn character_profession_optimizer(
        &self,
        profession: &str,
    ) -> Result<ProfessionOptimizerResult, ApiError> {
        let prof_entity = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(profession))
            .cloned();
        let Some(prof_entity) = prof_entity else {
            return Ok(ProfessionOptimizerResult {
                skills: Vec::new(),
                attributes: Vec::new(),
                profession: None,
                current_level: None,
                next_level: None,
                gap: None,
                error: Some(format!("Profession '{profession}' not found")),
            });
        };
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("optimizer skill calibrations"))?;
        let result = profession_skill_optimizer(&skill_levels, &prof_entity);
        Ok(ProfessionOptimizerResult {
            skills: result.skills.into_iter().map(optimizer_skill_dto).collect(),
            attributes: result
                .attributes
                .into_iter()
                .map(optimizer_attribute_dto)
                .collect(),
            profession: Some(profession.to_string()),
            current_level: Some(result.current_level),
            next_level: Some(result.next_level as f64),
            gap: Some(result.gap),
            error: None,
        })
    }

    /// The path optimizer: greedy allocation for a target level or a PED
    /// budget (exactly one supplied).
    pub async fn character_path_optimizer(
        &self,
        profession: &str,
        target_level: Option<f64>,
        ped_budget: Option<f64>,
    ) -> Result<PathOptimizerResult, ApiError> {
        // The mode contract, validated before dispatch.
        if target_level.is_none() == ped_budget.is_none() {
            return Err(ApiError::bad_request(
                "Exactly one of target_level or ped_budget must be provided",
            ));
        }
        let prof_entity = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(profession))
            .cloned();
        let Some(prof_entity) = prof_entity else {
            // A missing profession converges on the full error shape
            // (was a minimal {allocations, attributes, error}); ratified.
            let mode = if target_level.is_some() {
                "target"
            } else {
                "budget"
            };
            return Ok(PathOptimizerResult {
                allocations: Vec::new(),
                attributes: Vec::new(),
                profession: profession.to_string(),
                mode: mode.to_string(),
                input_target_level: target_level.into(),
                input_ped_budget: ped_budget.into(),
                current_level: 0.0,
                end_level: 0.0,
                profession_levels_gained: 0.0,
                total_ped: 0.0,
                excluded: Vec::new(),
                error: Some(format!("Profession '{profession}' not found")),
            });
        };
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("path optimizer skill calibrations"))?;
        // The mode contract is validated above; a service-level rejection
        // here is unreachable.
        let result =
            profession_path_optimizer(&skill_levels, &prof_entity, target_level, ped_budget)
                .map_err(|_| ApiError::Internal)?;
        Ok(PathOptimizerResult {
            allocations: result
                .allocations
                .into_iter()
                .map(path_allocation_dto)
                .collect(),
            attributes: result
                .attributes
                .into_iter()
                .map(optimizer_attribute_dto)
                .collect(),
            profession: profession.to_string(),
            mode: result.mode.to_string(),
            input_target_level: result.input_target_level.into(),
            input_ped_budget: result.input_ped_budget.into(),
            current_level: result.current_level,
            end_level: result.end_level,
            profession_levels_gained: result.profession_levels_gained,
            total_ped: result.total_ped,
            excluded: result
                .excluded
                .into_iter()
                .map(|row| ExcludedSkill {
                    name: row.name,
                    weight: row.weight,
                    reason: row.reason.to_string(),
                })
                .collect(),
            error: None,
        })
    }

    /// The HP optimizer: HP-per-PED across contributing skills and
    /// attributes.
    pub async fn character_hp_optimizer(&self) -> Result<HpOptimizerResult, ApiError> {
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("hp optimizer skill calibrations"))?;
        let skills_data = self.game_data.get_entities("skills");
        let result = hp_skill_optimizer(&skill_levels, skills_data);
        Ok(HpOptimizerResult {
            current_hp: result.current_hp,
            skills: result
                .skills
                .into_iter()
                .map(|row| HpOptimizerSkill {
                    name: row.name,
                    hp_increase: row.hp_increase,
                    current_level: row.current_level,
                    levels_per_hp: row.levels_per_hp,
                    ped_per_hp: row.ped_per_hp,
                    hp_per_ped: row.hp_per_ped,
                    codex_category: row.codex_category.map(str::to_string).into(),
                    codex_divisor: row.codex_divisor.map(|divisor| divisor as f64).into(),
                })
                .collect(),
            attributes: result
                .attributes
                .into_iter()
                .map(|row| HpOptimizerAttribute {
                    name: row.name,
                    hp_increase: row.hp_increase,
                    current_level: row.current_level,
                    levels_per_hp: row.levels_per_hp,
                })
                .collect(),
        })
    }

    /// The activity recommender: every performable activity projected
    /// against the target (a profession, a profession family, or HP)
    /// and ranked by skilling-PES-to-+1, with the direct-grind
    /// reference for single-profession targets.
    pub async fn character_activity_recommender(
        &self,
        query: &ActivityRecommenderQuery,
    ) -> Result<ActivityRecommenderResult, ApiError> {
        let professions = self.game_data.get_entities("professions");
        let target = match query.target {
            RecommenderTargetKind::Hp => RecommenderTarget::Hp,
            RecommenderTargetKind::Profession => {
                if query.professions.is_empty() {
                    return Err(ApiError::bad_request(
                        "professions is required for a profession target",
                    ));
                }
                if let Some(unknown) = query.professions.iter().find(|name| {
                    !professions
                        .iter()
                        .any(|p| p.get("name").and_then(Value::as_str) == Some(name.as_str()))
                }) {
                    // A missing profession is the family's soft-error
                    // shape, rendered inline rather than thrown.
                    return Ok(ActivityRecommenderResult {
                        error: Some(format!("Profession '{unknown}' not found")),
                        pes_cap: RECOMMENDER_PES_CAP,
                        sample_step: RECOMMENDER_SAMPLE_STEP,
                        direct: None.into(),
                        candidates: Vec::new(),
                    });
                }
                RecommenderTarget::Professions(query.professions.clone())
            }
        };
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("recommender skill calibrations"))?;
        let skills_data = self.game_data.get_entities("skills");
        let breakdown = activity_recommender(&skill_levels, professions, skills_data, &target);
        Ok(ActivityRecommenderResult {
            error: None,
            pes_cap: breakdown.pes_cap,
            sample_step: breakdown.sample_step,
            direct: breakdown.direct.map(recommender_activity_dto).into(),
            candidates: breakdown
                .candidates
                .into_iter()
                .map(recommender_activity_dto)
                .collect(),
        })
    }

    /// Latest calibrated level per skill: believed-current when `source`
    /// is None, the scan anchor when `source='scan'` (the
    /// `MAX(scanned_at)` / `MAX(id)` tiebreaker read behind
    /// [`Db::latest_skill_calibrations`]), as the insertion-ordered map
    /// the calculation services consume.
    pub(crate) async fn skill_calibrations(
        &self,
        source: Option<&str>,
    ) -> Result<Map<String, Value>, DbError> {
        let rows = self
            .db
            .latest_skill_calibrations(source.map(str::to_string))
            .await?;
        let mut levels = Map::new();
        for (name, level) in rows {
            levels.insert(name, json!(level));
        }
        Ok(levels)
    }

    /// Epoch timestamp of the most recent calibration, or None.
    async fn last_calibration_ts(&self) -> Result<Option<f64>, DbError> {
        self.db.last_calibration_epoch().await
    }
}

// ── Shaping helpers ─────────────────────────────────────────────────

fn recommender_activity_dto(projection: ActivityProjection) -> RecommenderActivity {
    RecommenderActivity {
        activity: projection.activity,
        professions: projection.professions,
        pes_to_plus_one: projection.pes_to_plus_one.into(),
        gain_at_cap: projection.gain_at_cap,
        series: projection.series,
        contributors: projection
            .contributors
            .into_iter()
            .map(|row| RecommenderContribution {
                name: row.name,
                current_level: row.current_level,
                level_gain: row.level_gain,
                target_gain: row.target_gain,
            })
            .collect(),
    }
}

fn optimizer_skill_dto(row: eo_services::character_calc::OptimizerSkillRow) -> OptimizerSkill {
    OptimizerSkill {
        name: row.name,
        weight: row.weight,
        current_level: row.current_level,
        levels_needed: row.levels_needed,
        ped_to_next_level: row.ped_to_next_level,
        codex_category: row.codex_category.map(str::to_string).into(),
        codex_divisor: row.codex_divisor.map(|divisor| divisor as f64).into(),
    }
}

fn optimizer_attribute_dto(
    row: eo_services::character_calc::OptimizerAttributeRow,
) -> OptimizerAttribute {
    OptimizerAttribute {
        name: row.name,
        weight: row.weight,
        current_level: row.current_level,
        contribution_factor: row.contribution_factor,
    }
}

fn path_allocation_dto(row: eo_services::character_calc::PathAllocationRow) -> PathAllocation {
    PathAllocation {
        name: row.name,
        weight: row.weight,
        current_level: row.current_level,
        levels_to_gain: row.levels_to_gain,
        ped_cost: row.ped_cost,
        new_level: row.new_level,
        codex_category: row.codex_category.map(str::to_string).into(),
        codex_divisor: row.codex_divisor.map(|divisor| divisor as f64).into(),
    }
}

/// Stable descending sort by a float key (Python `sort(reverse=True)`).
fn sort_desc_by<T>(items: &mut [T], key: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| key(b).partial_cmp(&key(a)).expect("levels are finite"));
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

/// Sorted `{name, skill}` rank thresholds from the catalogue.
fn get_ranks(game_data: &eo_services::game_data_store::GameDataStore) -> Vec<Value> {
    let entities = game_data.get_entities("skill_ranks");
    let Some(first) = entities.first() else {
        return Vec::new();
    };
    let rows = first
        .get("table")
        .and_then(|t| t.get("rows"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut valid: Vec<Value> = Vec::new();
    for row in rows {
        let Some(threshold) = row.get("skill").and_then(Value::as_f64) else {
            continue;
        };
        let Some(name) = row.get("name").filter(|n| !n.is_null()) else {
            continue;
        };
        valid.push(json!({"name": name, "skill": threshold}));
    }
    valid.sort_by(|a, b| {
        let left = a["skill"].as_f64().unwrap_or(0.0);
        let right = b["skill"].as_f64().unwrap_or(0.0);
        left.partial_cmp(&right).expect("thresholds are finite")
    });
    valid
}

// ── Prospect helpers ────────────────────────────────────────────────

/// The aggregated Prospect sample, carrying the internal share / rate
/// maps the forecast projects with (accumulation order preserved); the
/// response DTO surfaces only the aggregates.
struct SampleData {
    sessions: i64,
    kills: i64,
    hours: f64,
    cycled_ped: f64,
    loot_tt: f64,
    pes: f64,
    attribute_levels: f64,
    cycled_per_hour: f64,
    loot_per_hour: f64,
    return_rate: f64,
    pes_per_ped: f64,
    loot_tt_per_ped: f64,
    skill_shares: Vec<(String, f64)>,
    attribute_rates: Vec<(String, f64)>,
}

impl SampleData {
    fn dto(&self) -> ProspectSample {
        ProspectSample {
            sessions: self.sessions,
            kills: self.kills,
            hours: self.hours,
            cycled_ped: self.cycled_ped,
            loot_tt: self.loot_tt,
            pes: self.pes,
            attribute_levels: self.attribute_levels,
            cycled_per_hour: self.cycled_per_hour,
            loot_per_hour: self.loot_per_hour,
            return_rate: self.return_rate,
            pes_per_ped: self.pes_per_ped,
            loot_tt_per_ped: self.loot_tt_per_ped,
        }
    }
}

/// A first-match lookup over an insertion-ordered `(name, value)` list.
fn lookup(entries: &[(String, f64)], name: &str) -> Option<f64> {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|&(_, value)| value)
}

/// Aggregate a session group into the Prospect sample.
fn prospect_sample(sessions: &[&Value]) -> SampleData {
    let mut regular_skill_ped: Vec<(String, f64)> = Vec::new();
    let mut attribute_level_sums: Vec<(String, f64)> = Vec::new();

    let sum_of = |key: &str| -> f64 {
        if sessions.is_empty() {
            return 0.0;
        }
        let total: f64 = sessions
            .iter()
            .map(|s| s.get(key).and_then(Value::as_f64).unwrap_or(0.0))
            .sum();
        round_half_even(total, 4)
    };
    let kills: i64 = sessions
        .iter()
        .map(|s| s.get("kills").and_then(Value::as_i64).unwrap_or(0))
        .sum();

    let hours = sum_of("durationHours");
    let cycled = sum_of("cycledPed");
    let loot_tt = sum_of("lootTt");
    let pes = sum_of("regularSkillTt");
    let attribute_levels = sum_of("attributeLevelsTotal");

    let accumulate = |entries: &mut Vec<(String, f64)>, name: &str, amount: f64| match entries
        .iter_mut()
        .find(|(entry, _)| entry == name)
    {
        Some((_, value)) => *value += amount,
        None => entries.push((name.to_string(), amount)),
    };
    for session in sessions {
        if let Some(map) = session.get("regularSkillPed").and_then(Value::as_object) {
            for (name, ped) in map {
                accumulate(&mut regular_skill_ped, name, ped.as_f64().unwrap_or(0.0));
            }
        }
        if let Some(map) = session.get("attributeLevels").and_then(Value::as_object) {
            for (name, amount) in map {
                accumulate(
                    &mut attribute_level_sums,
                    name,
                    amount.as_f64().unwrap_or(0.0),
                );
            }
        }
    }

    let skill_shares: Vec<(String, f64)> = regular_skill_ped
        .into_iter()
        .filter(|&(_, ped)| pes > 0.0 && ped > 0.0)
        .map(|(name, ped)| (name, ped / pes))
        .collect();
    let attribute_rates: Vec<(String, f64)> = attribute_level_sums
        .into_iter()
        .filter(|&(_, amount)| cycled > 0.0 && amount > 0.0)
        .map(|(name, amount)| (name, amount / cycled))
        .collect();

    SampleData {
        sessions: sessions.len() as i64,
        kills,
        hours,
        cycled_ped: cycled,
        loot_tt,
        pes,
        attribute_levels,
        cycled_per_hour: if hours > 0.0 {
            round_half_even(cycled / hours, 4)
        } else {
            0.0
        },
        loot_per_hour: if hours > 0.0 {
            round_half_even(loot_tt / hours, 4)
        } else {
            0.0
        },
        return_rate: if cycled > 0.0 {
            round_half_even(loot_tt / cycled, 4)
        } else {
            0.0
        },
        pes_per_ped: if cycled > 0.0 {
            round_half_even(pes / cycled, 6)
        } else {
            0.0
        },
        loot_tt_per_ped: if cycled > 0.0 {
            round_half_even(loot_tt / cycled, 6)
        } else {
            0.0
        },
        skill_shares,
        attribute_rates,
    }
}

/// The grouped option list for one dominant-value key.
fn prospect_option_list(sessions: &[Value], key: &str) -> Vec<ProspectOption> {
    let mut grouped: Vec<(String, Vec<&Value>)> = Vec::new();
    for session in sessions {
        let Some(value) = session
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        match grouped.iter_mut().find(|(group, _)| group == value) {
            Some((_, members)) => members.push(session),
            None => grouped.push((value.to_string(), vec![session])),
        }
    }

    let mut options: Vec<ProspectOption> = Vec::new();
    for (value, members) in &grouped {
        let sample = prospect_sample(members);
        options.push(ProspectOption {
            value: value.clone(),
            label: value.clone(),
            sessions: sample.sessions,
            kills: sample.kills,
            hours: round_half_even(sample.hours, 2),
            cycled_ped: round_half_even(sample.cycled_ped, 2),
        });
    }

    options.sort_by(|a, b| {
        b.sessions
            .cmp(&a.sessions)
            .then_with(|| {
                b.cycled_ped
                    .partial_cmp(&a.cycled_ped)
                    .expect("cycled values are finite")
            })
            .then_with(|| a.label.cmp(&b.label))
    });
    options
}

/// Filter sessions to a slice (`global` passes everything through).
fn match_prospect_sessions<'s>(
    sessions: &'s [Value],
    slice_type: &str,
    slice_value: &Option<String>,
) -> Vec<&'s Value> {
    if slice_type == "global" {
        return sessions.iter().collect();
    }
    let Some(slice_value) = slice_value.as_deref().filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    let key = match slice_type {
        "tag" => "dominantTag",
        "mob" => "dominantMob",
        "weapon" => "dominantWeapon",
        _ => return Vec::new(),
    };
    sessions
        .iter()
        .filter(|session| session.get(key).and_then(Value::as_str) == Some(slice_value))
        .collect()
}

fn build_prospect_warnings(sample: &SampleData, projected_cycled_ped: f64) -> Vec<String> {
    let mut warnings = Vec::new();
    if sample.sessions < PROSPECT_SAMPLE_WARN_SESSIONS {
        warnings.push("Thin sample: fewer than 3 matching sessions.".to_string());
    }
    if sample.hours < PROSPECT_SAMPLE_WARN_HOURS {
        warnings.push("Thin sample: less than 2 hours of matching play.".to_string());
    }
    if sample.cycled_ped < PROSPECT_SAMPLE_WARN_CYCLED_PED {
        warnings.push("Thin sample: less than 50 PED of matching cycling.".to_string());
    }
    if sample.cycled_ped > 0.0 && projected_cycled_ped > sample.cycled_ped * 20.0 {
        warnings.push(
            "Long extrapolation: forecast extends far beyond the observed sample.".to_string(),
        );
    }
    warnings
}

/// Project skill levels after cycling `total_ped` through the sample's
/// observed composition: (projected levels, projected gains).
fn project_prospect_levels(
    skill_levels: &Map<String, Value>,
    sample: &SampleData,
    total_ped: f64,
) -> (Map<String, Value>, Vec<(String, f64)>) {
    let mut projected_levels: Map<String, Value> = skill_levels
        .iter()
        .map(|(name, level)| (name.clone(), json!(level.as_f64().unwrap_or(0.0))))
        .collect();
    let mut projected_gains: Vec<(String, f64)> = Vec::new();

    let skill_tt_budget = total_ped * sample.pes_per_ped;
    for (skill_name, share) in &sample.skill_shares {
        let current = projected_levels
            .get(skill_name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let allocated_tt = skill_tt_budget * share;
        let gained = levels_for_tt_value(current, allocated_tt);
        projected_levels.insert(
            skill_name.clone(),
            json!(round_half_even(current + gained, 4)),
        );
        projected_gains.push((skill_name.clone(), round_half_even(gained, 4)));
    }
    for (skill_name, rate) in &sample.attribute_rates {
        let current = projected_levels
            .get(skill_name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let gained = total_ped * rate;
        projected_levels.insert(
            skill_name.clone(),
            json!(round_half_even(current + gained, 4)),
        );
        match projected_gains
            .iter_mut()
            .find(|(name, _)| name == skill_name)
        {
            Some((_, value)) => *value = round_half_even(gained, 4),
            None => projected_gains.push((skill_name.clone(), round_half_even(gained, 4))),
        }
    }
    (projected_levels, projected_gains)
}

/// Whether the observed sample contains gains that move the profession.
fn relevant_prospect_progress(sample: &SampleData, profession: &Value) -> bool {
    let skills = profession
        .get("skills")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for entry in skills {
        let name = entry
            .get("skill")
            .and_then(|s| s.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
        if name.is_empty() || weight <= 0.0 {
            continue;
        }
        if lookup(&sample.skill_shares, name).is_some()
            || lookup(&sample.attribute_rates, name).is_some()
        {
            return true;
        }
    }
    false
}

/// An early-return prospect result (error before any forecast values).
#[allow(clippy::too_many_arguments)]
fn prospect_error_result(
    profession_name: &str,
    slice_type: &str,
    slice_value: &Option<String>,
    markup_uplift: f64,
    current_level: f64,
    target_level: f64,
    sample: SampleData,
    error: &str,
) -> ProspectResult {
    ProspectResult {
        error: Some(error.to_string()),
        rows: Vec::new(),
        warnings: Vec::new(),
        profession: profession_name.to_string(),
        slice_type: slice_type.to_string(),
        slice_value: slice_value.clone().into(),
        markup_uplift,
        current_level: round_half_even(current_level, 2),
        target_level: round_half_even(target_level, 2),
        projected_cycled_ped: 0.0,
        projected_hours: 0.0,
        expected_loot_tt: 0.0,
        expected_net_tt_burn: 0.0,
        speculative_loot_tt: None.into(),
        speculative_net_tt_burn: None.into(),
        sample: sample.dto(),
    }
}

/// The full forecast, mirroring `_build_prospect_result` (including the
/// doubling search and 60-step bisection over projected cycling).
#[allow(clippy::too_many_arguments)]
fn build_prospect_result(
    profession_name: &str,
    profession: &Value,
    skill_levels: &Map<String, Value>,
    target_level: f64,
    sample: SampleData,
    slice_type: &str,
    slice_value: &Option<String>,
    markup_uplift: f64,
) -> ProspectResult {
    let current_level = profession_level(skill_levels, profession);

    let projected_levels: Map<String, Value>;
    let mut projected_gains: Vec<(String, f64)> = Vec::new();
    let projected_cycled_ped: f64;

    if target_level <= current_level {
        projected_levels = skill_levels
            .iter()
            .map(|(name, level)| (name.clone(), json!(level.as_f64().unwrap_or(0.0))))
            .collect();
        projected_cycled_ped = 0.0;
    } else {
        let cycled = sample.cycled_ped;
        let hours = sample.hours;
        if cycled <= 0.0 || hours <= 0.0 {
            return prospect_error_result(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "Insufficient matching data for a forecast.",
            );
        }
        if !relevant_prospect_progress(&sample, profession) {
            return prospect_error_result(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "The observed sample does not contain gains that move this profession.",
            );
        }

        let mut lower = 0.0_f64;
        let mut upper = cycled.max(1.0);
        let mut upper_level = profession_level(
            &project_prospect_levels(skill_levels, &sample, upper).0,
            profession,
        );
        while upper_level < target_level && upper < 1_000_000_000.0 {
            lower = upper;
            upper *= 2.0;
            upper_level = profession_level(
                &project_prospect_levels(skill_levels, &sample, upper).0,
                profession,
            );
        }
        if upper_level < target_level {
            return prospect_error_result(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "Target is outside the reachable forecast range for this sample.",
            );
        }
        for _ in 0..60 {
            let mid = (lower + upper) / 2.0;
            let (test_levels, _) = project_prospect_levels(skill_levels, &sample, mid);
            if profession_level(&test_levels, profession) >= target_level {
                upper = mid;
            } else {
                lower = mid;
            }
        }
        projected_cycled_ped = round_half_even(upper, 2);
        let projected = project_prospect_levels(skill_levels, &sample, projected_cycled_ped);
        projected_levels = projected.0;
        projected_gains = projected.1;
    }

    let expected_loot_tt = round_half_even(projected_cycled_ped * sample.loot_tt_per_ped, 2);
    let expected_net_tt_burn = round_half_even(projected_cycled_ped - expected_loot_tt, 2);
    let projected_hours = if sample.cycled_ped > 0.0 {
        round_half_even(projected_cycled_ped * (sample.hours / sample.cycled_ped), 2)
    } else {
        0.0
    };

    let (speculative_loot_tt, speculative_net_tt_burn) = if markup_uplift > 0.0 {
        let loot = round_half_even(expected_loot_tt * (1.0 + markup_uplift), 2);
        (
            Some(loot),
            Some(round_half_even(projected_cycled_ped - loot, 2)),
        )
    } else {
        (None, None)
    };

    let mut weights: Vec<(String, f64)> = Vec::new();
    if let Some(skills) = profession.get("skills").and_then(Value::as_array) {
        for entry in skills {
            let name = entry
                .get("skill")
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
            match weights.iter_mut().find(|(entry, _)| *entry == name) {
                Some((_, value)) => *value = weight,
                None => weights.push((name, weight)),
            }
        }
    }

    // The union of observed shares and rates; union order is free (the
    // rows sort below is total: contribution, attribute flag, then the
    // unique name), so insertion order here never reaches the wire.
    let mut row_names: Vec<String> = sample
        .skill_shares
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    for (name, _) in &sample.attribute_rates {
        if !row_names.contains(name) {
            row_names.push(name.clone());
        }
    }

    let mut rows: Vec<ProspectRow> = Vec::new();
    for name in &row_names {
        let current_skill_level = skill_levels
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let projected_gain = lookup(&projected_gains, name).unwrap_or(0.0);
        let projected_end_level = projected_levels
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(current_skill_level);
        let weight = lookup(&weights, name).unwrap_or(0.0);
        let contribution = if weight > 0.0 {
            (effective_points(name, projected_gain) * weight) / 10000.0
        } else {
            0.0
        };
        let observed_share = lookup(&sample.skill_shares, name).unwrap_or(0.0);
        let observed_rate = lookup(&sample.attribute_rates, name).unwrap_or(0.0);
        rows.push(ProspectRow {
            name: name.clone(),
            is_attribute: is_attribute(name),
            weight,
            current_level: round_half_even(current_skill_level, 2),
            observed_share: round_half_even(observed_share, 4),
            observed_rate: round_half_even(observed_rate, 6),
            projected_gain: round_half_even(projected_gain, 2),
            projected_end_level: round_half_even(projected_end_level, 2),
            profession_contribution: round_half_even(contribution, 4),
            relevant: weight > 0.0,
        });
    }
    rows.sort_by(|a, b| {
        b.profession_contribution
            .partial_cmp(&a.profession_contribution)
            .expect("contributions are finite")
            .then_with(|| a.is_attribute.cmp(&b.is_attribute))
            .then_with(|| a.name.cmp(&b.name))
    });

    let warnings = build_prospect_warnings(&sample, projected_cycled_ped);
    ProspectResult {
        error: None,
        rows,
        warnings,
        profession: profession_name.to_string(),
        slice_type: slice_type.to_string(),
        slice_value: slice_value.clone().into(),
        markup_uplift,
        current_level: round_half_even(current_level, 2),
        target_level: round_half_even(target_level, 2),
        projected_cycled_ped,
        projected_hours,
        expected_loot_tt,
        expected_net_tt_burn,
        speculative_loot_tt: speculative_loot_tt.into(),
        speculative_net_tt_burn: speculative_net_tt_burn.into(),
        sample: sample.dto(),
    }
}
