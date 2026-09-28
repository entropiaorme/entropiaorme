//! The skilling forecast: how much of each named session (session
//! definition) it takes to reach a goal on a profession or HP, from that
//! definition's recorded play, with its own realised markup applied.
//! Read-only; the projection lives in `eo_services::skilling_forecast`.

use eo_services::skilling_forecast::{
    load_forecast_sessions, skilling_forecast, ForecastSkill, ForecastStatus, ForecastTarget,
    SampleWarning, SourceForecast,
};
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analytics::analytics_error;
use crate::Nullable;
use crate::{Api, ApiError};

/// What the forecast measures progress on. A closed vocabulary: the
/// bindings expose only these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SkillingTargetKind {
    Profession,
    Hp,
}

/// The forecast query. `profession` names the target for a `profession`
/// target and is ignored for `hp`; `goal` is the profession level or HP
/// to reach.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastQuery {
    pub target: SkillingTargetKind,
    #[serde(default)]
    pub profession: Option<String>,
    pub goal: f64,
}

/// Whether a definition answers, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillingForecastStatus {
    Ready,
    Reached,
    DoesNotTrain,
    NoEvidence,
    OutOfRange,
}

impl From<ForecastStatus> for SkillingForecastStatus {
    fn from(status: ForecastStatus) -> Self {
        match status {
            ForecastStatus::Ready => Self::Ready,
            ForecastStatus::Reached => Self::Reached,
            ForecastStatus::DoesNotTrain => Self::DoesNotTrain,
            ForecastStatus::NoEvidence => Self::NoEvidence,
            ForecastStatus::OutOfRange => Self::OutOfRange,
        }
    }
}

/// A sample-quality caveat on a forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillingSampleWarning {
    ThinSessions,
    ThinHours,
    ThinCycling,
    LongExtrapolation,
}

impl From<SampleWarning> for SkillingSampleWarning {
    fn from(warning: SampleWarning) -> Self {
        match warning {
            SampleWarning::ThinSessions => Self::ThinSessions,
            SampleWarning::ThinHours => Self::ThinHours,
            SampleWarning::ThinCycling => Self::ThinCycling,
            SampleWarning::LongExtrapolation => Self::LongExtrapolation,
        }
    }
}

/// The recorded play a definition's forecast projects from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastSample {
    pub sessions: i64,
    pub hours: f64,
    pub cycled_ped: f64,
    pub loot_tt: f64,
    pub pes: f64,
    /// Net markup realised from confirmed sales of the definition's stock;
    /// null when none has sold.
    pub realised_markup: Nullable<f64>,
    /// That markup as a fraction of the definition's loot TT.
    pub markup_lift: Nullable<f64>,
}

/// One skill the definition trains, projected to the goal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastSkill {
    pub name: String,
    pub is_attribute: bool,
    pub current_level: f64,
    /// Share of the definition's skill PES; null for attributes.
    pub pes_share: Nullable<f64>,
    pub level_gain: f64,
    pub end_level: f64,
    /// Profession levels or HP this skill's gain contributes.
    pub target_gain: f64,
    pub moves_target: bool,
}

/// One definition's forecast. The projected figures are zero unless the
/// status is `ready`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastSource {
    pub definition_id: i64,
    pub name: String,
    pub archived: bool,
    pub sample: SkillingForecastSample,
    pub status: SkillingForecastStatus,
    pub cycled_ped: f64,
    pub hours: f64,
    pub loot_tt: f64,
    /// Realised markup lift applied to the forecast loot; null when the
    /// definition has no confirmed sales.
    pub markup: Nullable<f64>,
    /// Cycled minus loot TT.
    pub tt_cost: f64,
    /// TT cost less realised markup; null with the markup.
    pub net_cost: Nullable<f64>,
    pub skills: Vec<SkillingForecastSkill>,
    pub warnings: Vec<SkillingSampleWarning>,
}

/// The forecast from every named session, ready ones first (least TT
/// cost to the goal leading). `error` is present only on the soft
/// unknown-profession path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The target's current value (profession level or HP).
    pub current: f64,
    pub goal: f64,
    pub sources: Vec<SkillingForecastSource>,
}

fn skill_dto(skill: ForecastSkill) -> SkillingForecastSkill {
    SkillingForecastSkill {
        name: skill.name,
        is_attribute: skill.is_attribute,
        current_level: skill.current_level,
        pes_share: skill.pes_share.into(),
        level_gain: skill.level_gain,
        end_level: skill.end_level,
        target_gain: skill.target_gain,
        moves_target: skill.moves_target,
    }
}

fn source_dto(source: SourceForecast) -> SkillingForecastSource {
    let evidence = source.evidence;
    let markup_lift = evidence.markup_lift().map(|lift| round_half_even(lift, 4));
    SkillingForecastSource {
        definition_id: evidence.definition_id,
        name: evidence.name,
        archived: evidence.archived,
        sample: SkillingForecastSample {
            sessions: evidence.sessions as i64,
            hours: evidence.hours,
            cycled_ped: evidence.cycled_ped,
            loot_tt: evidence.loot_tt,
            pes: evidence.pes,
            realised_markup: evidence.realised_markup.into(),
            markup_lift: markup_lift.into(),
        },
        status: source.status.into(),
        cycled_ped: source.cycled_ped,
        hours: source.hours,
        loot_tt: source.loot_tt,
        markup: source.markup.into(),
        tt_cost: source.tt_cost,
        net_cost: source.net_cost.into(),
        skills: source.skills.into_iter().map(skill_dto).collect(),
        warnings: source.warnings.into_iter().map(Into::into).collect(),
    }
}

impl Api {
    /// The skilling forecast toward `goal` from every named session.
    pub async fn character_skilling_forecast(
        &self,
        query: &SkillingForecastQuery,
    ) -> Result<SkillingForecastResult, ApiError> {
        if !query.goal.is_finite() || query.goal <= 0.0 {
            return Err(ApiError::bad_request("goal must be positive"));
        }
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("skilling forecast skill calibrations"))?;
        let target = match query.target {
            SkillingTargetKind::Hp => {
                ForecastTarget::hp(self.game_data.get_entities("skills"), &skill_levels)
            }
            SkillingTargetKind::Profession => {
                let Some(name) = query.profession.as_deref().filter(|name| !name.is_empty()) else {
                    return Err(ApiError::bad_request(
                        "profession is required for a profession target",
                    ));
                };
                let Some(entity) = self
                    .game_data
                    .get_entities("professions")
                    .iter()
                    .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
                else {
                    // The family's soft-error shape, rendered inline.
                    return Ok(SkillingForecastResult {
                        error: Some(format!("Profession '{name}' not found")),
                        current: 0.0,
                        goal: query.goal,
                        sources: Vec::new(),
                    });
                };
                ForecastTarget::profession(entity, &skill_levels)
            }
        };
        let (sessions, markups) = tokio::try_join!(
            async {
                load_forecast_sessions(&self.db)
                    .await
                    .map_err(ApiError::internal("skilling forecast sessions"))
            },
            async {
                self.analytics
                    .realised_markup_by_definition()
                    .await
                    .map_err(analytics_error("skilling forecast realised markup"))
            },
        )?;
        let markups: Vec<(i64, f64)> = markups
            .into_iter()
            .map(|row| (row.definition_id, row.net_markup))
            .collect();
        let sources = skilling_forecast(&skill_levels, &target, query.goal, &sessions, &markups);
        Ok(SkillingForecastResult {
            error: None,
            current: round_half_even(target.current(), 2),
            goal: query.goal,
            sources: sources.into_iter().map(source_dto).collect(),
        })
    }
}
