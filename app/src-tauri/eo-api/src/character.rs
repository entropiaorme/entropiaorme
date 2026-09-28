//! The character family: calibration status, stats, skills, professions,
//! the path and HP optimizers, and the activity recommender, all computed
//! from the calibrated skill levels plus the bundled game-data catalogue
//! through the calculation services (`eo-services`). The skilling forecast
//! over recorded sessions lives beside it in [`crate::skilling`].
//!
//! The family is read-only: no stored bytes change. The response shapes
//! match the frontend's hand-written contract (`$lib/types/analytics.ts`)
//! field for field, expressed directly by the DTOs' declared field order
//! and `f64` typing; no separate projection pass exists.
//!
//! Contract lineage (ADR-0017/0019): the path-optimizer *not-found* soft
//! error converges on its family's full error shape (it was a minimal
//! three-key body). The legacy `GET /api/character/codex` skill-progress
//! list retires unconverted: it has no frontend caller, exactly as the
//! equipment cost endpoint retired with its family.

use eo_services::activity_recommender::{
    activity_recommender, ActivityProjection, RecommenderTarget, RECOMMENDER_PES_CAP,
    RECOMMENDER_SAMPLE_STEP,
};
use eo_services::character_calc::{
    all_profession_levels, combined_profession, hp_skill_optimizer, is_attribute,
    profession_path_optimizer, skill_rank,
};
use eo_services::db::DbError;
use eo_services::time::{naive_to_epoch, to_iso_utc};
use eo_services::tt_value_curve::tt_value_at;
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::Nullable;
use crate::{Api, ApiError};

/// Skills are considered stale after 30 days without recalibration.
const STALE_DAYS: f64 = 30.0;

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

/// One attribute row of the profession / path optimizer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerAttribute {
    pub name: String,
    pub weight: f64,
    pub current_level: f64,
    pub contribution_factor: f64,
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

    /// One target entity for a list of profession names: the profession
    /// itself for one name, the members' [`combined_profession`] for several
    /// (a family, levelled as the sum of its members). An empty list is a
    /// bad request; an unknown name is `Ok(Err(soft error))`, which each
    /// command renders in its family's inline error shape.
    pub(crate) fn target_profession(
        &self,
        professions: &[String],
    ) -> Result<Result<Value, String>, ApiError> {
        if professions.is_empty() {
            return Err(ApiError::bad_request("at least one profession is required"));
        }
        let catalogue = self.game_data.get_entities("professions");
        let mut members: Vec<&Value> = Vec::with_capacity(professions.len());
        for name in professions {
            match catalogue
                .iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(name.as_str()))
            {
                Some(entity) => members.push(entity),
                None => return Ok(Err(format!("Profession '{name}' not found"))),
            }
        }
        Ok(Ok(match members.as_slice() {
            [single] => (*single).clone(),
            _ => combined_profession(&professions.join(", "), &members),
        }))
    }

    /// The path optimizer: greedy allocation for a target level or a PED
    /// budget (exactly one supplied). Several professions are optimised as
    /// one combined target whose level is the sum of theirs.
    pub async fn character_path_optimizer(
        &self,
        professions: &[String],
        target_level: Option<f64>,
        ped_budget: Option<f64>,
    ) -> Result<PathOptimizerResult, ApiError> {
        // The mode contract, validated before dispatch.
        if target_level.is_none() == ped_budget.is_none() {
            return Err(ApiError::bad_request(
                "Exactly one of target_level or ped_budget must be provided",
            ));
        }
        let profession = professions.join(", ");
        let prof_entity = match self.target_profession(professions)? {
            Ok(entity) => entity,
            Err(error) => {
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
                    profession,
                    mode: mode.to_string(),
                    input_target_level: target_level.into(),
                    input_ped_budget: ped_budget.into(),
                    current_level: 0.0,
                    end_level: 0.0,
                    profession_levels_gained: 0.0,
                    total_ped: 0.0,
                    excluded: Vec::new(),
                    error: Some(error),
                });
            }
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
            profession,
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
