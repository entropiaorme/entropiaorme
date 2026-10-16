//! The skilling forecast: how much of each named session (session
//! definition) it takes to reach a goal on a profession or HP, from that
//! definition's recorded play, with its own realised markup applied; and,
//! beside them, planned routes (a weapon on a mob maturity), priced by the
//! skill-per-kill law and the loadout comparison engine before any of it
//! is played. Read-only; the projections live in
//! `eo_services::skilling_forecast` and `eo_services::skilling_routes`.

use eo_services::loadout_comparison::setup_economics;
use eo_services::mob_lookup_service::MobLookupService;
use eo_services::skilling_forecast::{
    forecast_planned, latest_skill_boost, load_forecast_sessions, load_weapon_forecast_sessions,
    session_weapons, skilling_forecast, ForecastSkill, ForecastStatus, ForecastTarget,
    PlannedForecast, SampleWarning, SourceForecast,
};
use eo_services::skilling_routes::{
    between_kill_seconds, mob_pace, skill_shares, weapon_professions, RouteKill, SkillBoost,
    RETARGET_SECONDS_PER_KILL,
};

use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::analytics::analytics_error;
use crate::loadout_comparison::{SetupPick, ASSUMPTIONS};
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

/// The forecast query. `professions` names a `profession` target (one
/// name, or several for a family forecast as their summed level) and is
/// ignored for `hp`; `goal` is the level (summed, for several) or HP to
/// reach.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingForecastQuery {
    pub target: SkillingTargetKind,
    #[serde(default)]
    pub professions: Vec<String>,
    pub goal: f64,
    /// Planned routes to price beside the recorded sessions.
    #[serde(default)]
    pub routes: Vec<SkillingRoutePick>,
    /// The skill boost percent planned routes assume (capped at 100);
    /// absent reads as the most recent session's.
    #[serde(default)]
    pub skill_boost_percent: Option<u8>,
    /// The looter level planned routes are priced at; absent reads as the
    /// player's three-looter mean.
    #[serde(default)]
    pub looter_level: Option<f64>,
    /// Narrow the named sessions to this weapon's part of them; absent
    /// forecasts each session whole.
    #[serde(default)]
    pub session_weapon: Option<String>,
}

/// A planned route: a setup hunting one mob maturity. `species` is the
/// catalogue's species name, or the mob's own name when it has none.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingRoutePick {
    pub setup: SetupPick,
    pub species: String,
    pub maturity: String,
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
    pub current_level: f64,
    /// Share of the definition's skill PES; null when it earned none.
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
    /// Kills at the recorded kills per PED cycled; null when the
    /// definition recorded none.
    pub kills: Nullable<f64>,
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

/// Whether a planned route answers, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillingRouteStatus {
    Ready,
    Reached,
    /// The weapon's professions do not move the target.
    DoesNotTrain,
    OutOfRange,
    /// The weapon is not in the catalogue, or names no professions.
    UnknownWeapon,
    /// The catalogue records no HP or level for the maturity.
    UnknownMob,
    /// The setup cannot be priced (no damage, cost, or Efficiency).
    Unmodelled,
}

impl From<ForecastStatus> for SkillingRouteStatus {
    fn from(status: ForecastStatus) -> Self {
        match status {
            ForecastStatus::Ready => Self::Ready,
            ForecastStatus::Reached => Self::Reached,
            ForecastStatus::DoesNotTrain => Self::DoesNotTrain,
            ForecastStatus::NoEvidence | ForecastStatus::OutOfRange => Self::OutOfRange,
        }
    }
}

/// One planned route's forecast, in request order. The projected figures
/// are zero unless the status is `ready`; the per-kill figures are zero
/// when the route could not be priced.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingRouteForecast {
    pub index: u32,
    pub weapon: String,
    /// The maturity and species as the mob picker shows them.
    pub mob: String,
    pub mob_hp: Nullable<f64>,
    pub mob_level: Nullable<f64>,
    pub status: SkillingRouteStatus,
    /// PES one kill yields, PEC.
    pub skill_per_kill_pec: f64,
    pub cycled_per_kill_pec: f64,
    /// Expected loss per kill, PEC.
    pub loss_per_kill_pec: f64,
    /// Firing time plus the time between kills; null when the attack rate
    /// is unknown.
    pub seconds_per_kill: Nullable<f64>,
    pub kills: f64,
    pub cycled_ped: f64,
    /// Expected TT lost on the way: cycled less expected loot TT.
    pub ped_lost: f64,
    pub hours: Nullable<f64>,
    /// The player's recorded pace between kills on the mob; null when
    /// they have too few recorded kills of it and a fixed allowance stands in.
    pub pace: Nullable<SkillingRoutePace>,
    pub skills: Vec<SkillingForecastSkill>,
}

/// The player's own time between kills on a mob, from their recorded kills
/// of it with any weapon, firing time taken out.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingRoutePace {
    pub between_kill_seconds: f64,
    pub kills: u64,
}

/// A weapon the named sessions used, with the kills it made alone.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillingSessionWeapon {
    pub name: String,
    pub kills: u64,
}

/// Fewest kills a weapon made alone in named sessions to be offered as
/// the weapon to narrow them to.
const SESSION_WEAPON_MIN_KILLS: u64 = 100;

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
    /// The planned routes, in request order.
    pub routes: Vec<SkillingRouteForecast>,
    /// The weapons the named sessions used, most used first, to narrow the
    /// sessions to one of.
    pub session_weapons: Vec<SkillingSessionWeapon>,
    /// Whether hunting prices this target: HP, or professions a weapon
    /// trains. Any other target grows from hunting only as a byproduct, so
    /// no route is priced toward it.
    pub hunting_target: bool,
    /// The skill boost percent the planned routes assumed.
    pub skill_boost_percent: u8,
    /// The looter level the planned routes were priced at.
    pub looter_level: f64,
}

/// The most planned routes one forecast prices.
pub const MAX_ROUTES: usize = 12;

fn skill_dto(skill: ForecastSkill) -> SkillingForecastSkill {
    SkillingForecastSkill {
        name: skill.name,
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
        kills: source.kills.into(),
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
        if query
            .looter_level
            .is_some_and(|level| !(level.is_finite() && (0.0..=200.0).contains(&level)))
        {
            return Err(ApiError::bad_request("a looter level is between 0 and 200"));
        }
        if query.routes.len() > MAX_ROUTES {
            return Err(ApiError::bad_request(format!(
                "a forecast prices at most {MAX_ROUTES} planned routes"
            )));
        }
        let skill_levels = self
            .skill_calibrations(None)
            .await
            .map_err(ApiError::internal("skilling forecast skill calibrations"))?;
        let target = match query.target {
            SkillingTargetKind::Hp => {
                ForecastTarget::hp(self.game_data.get_entities("skills"), &skill_levels)
            }
            SkillingTargetKind::Profession => match self.target_profession(&query.professions)? {
                Ok(entity) => ForecastTarget::profession(&entity, &skill_levels),
                Err(error) => {
                    // The family's soft-error shape, rendered inline.
                    return Ok(SkillingForecastResult {
                        error: Some(error),
                        current: 0.0,
                        goal: query.goal,
                        sources: Vec::new(),
                        routes: Vec::new(),
                        hunting_target: false,
                        session_weapons: Vec::new(),
                        skill_boost_percent: query
                            .skill_boost_percent
                            .map_or(SkillBoost::FIFTY, SkillBoost::new)
                            .percent(),
                        looter_level: 0.0,
                    });
                }
            },
        };
        let (sessions, markups) = tokio::try_join!(
            async {
                match &query.session_weapon {
                    Some(weapon) => load_weapon_forecast_sessions(&self.db, weapon)
                        .await
                        .map_err(ApiError::internal("skilling forecast weapon sessions")),
                    None => load_forecast_sessions(&self.db)
                        .await
                        .map_err(ApiError::internal("skilling forecast sessions")),
                }
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
        let hunting_target = match query.target {
            SkillingTargetKind::Hp => true,
            SkillingTargetKind::Profession => {
                let trained = self.weapon_trained_professions();
                query.professions.iter().all(|name| trained.contains(name))
            }
        };

        let boost = match query.skill_boost_percent {
            Some(percent) => SkillBoost::new(percent),
            None => SkillBoost::from_percent(
                latest_skill_boost(&self.db)
                    .await
                    .map_err(ApiError::internal("skilling forecast latest pill"))?,
            ),
        };
        let (_, looters) = self.planning_looters(query.looter_level).await?;
        let mut routes = Vec::with_capacity(query.routes.len());
        let picks = if hunting_target {
            query.routes.as_slice()
        } else {
            &[]
        };
        for (index, pick) in picks.iter().enumerate() {
            let context = RouteContext {
                skill_levels: &skill_levels,
                target: &target,
                goal: query.goal,
                boost,
                looters,
            };
            routes.push(self.forecast_route(index, pick, &context).await?);
        }

        Ok(SkillingForecastResult {
            error: None,
            current: round_half_even(target.current(), 2),
            goal: query.goal,
            sources: sources.into_iter().map(source_dto).collect(),
            routes,
            hunting_target,
            session_weapons: self.session_weapons().await?,
            skill_boost_percent: boost.percent(),
            looter_level: round_half_even(looters.three_looter_mean(), 2),
        })
    }

    /// One planned route priced toward the goal, from the player's own kills
    /// when there are enough, else from the model. A route the catalogue
    /// can no longer resolve (a stored pick outliving a data refresh)
    /// answers with a status rather than failing the forecast.
    async fn forecast_route(
        &self,
        index: usize,
        pick: &SkillingRoutePick,
        context: &RouteContext<'_>,
    ) -> Result<SkillingRouteForecast, ApiError> {
        let RouteContext {
            skill_levels,
            target,
            goal,
            boost,
            looters,
        } = *context;
        let mob =
            MobLookupService::new(&self.game_data).maturity_stats(&pick.species, &pick.maturity);
        let mut route = SkillingRouteForecast {
            index: index as u32,
            weapon: String::new(),
            mob: if pick.maturity.trim().is_empty() {
                pick.species.trim().to_string()
            } else {
                format!("{} {}", pick.maturity.trim(), pick.species.trim())
            },
            mob_hp: mob.map(|mob| mob.hp).into(),
            mob_level: mob.map(|mob| mob.level).into(),
            status: SkillingRouteStatus::UnknownWeapon,
            skill_per_kill_pec: 0.0,
            cycled_per_kill_pec: 0.0,
            loss_per_kill_pec: 0.0,
            seconds_per_kill: None.into(),
            kills: 0.0,
            cycled_ped: 0.0,
            ped_lost: 0.0,
            hours: None.into(),
            pace: None.into(),
            skills: Vec::new(),
        };
        let Ok(setup) = self.resolve_setup(index, &pick.setup) else {
            return Ok(route);
        };
        route.weapon = setup.name.clone();
        let professions = setup
            .props
            .get("weapon_entity")
            .map(weapon_professions)
            .unwrap_or_default();
        let shares = skill_shares(
            self.game_data.get_entities("professions"),
            &professions,
            skill_levels,
        );
        if shares.is_empty() {
            return Ok(route);
        }
        let Some(mob) = mob else {
            route.status = SkillingRouteStatus::UnknownMob;
            return Ok(route);
        };
        let rate = self
            .weapon_pricing
            .attack_rate(&setup.props)
            .map(|rate| rate.effective_per_minute);
        let pace = self.mob_pace(&pick.species, &pick.maturity).await?;
        route.pace = pace.into();
        let between_kills =
            pace.map_or(RETARGET_SECONDS_PER_KILL, |pace| pace.between_kill_seconds);
        let kill = setup_economics(&setup.props, looters, rate, &ASSUMPTIONS)
            .ok()
            .and_then(|economics| {
                RouteKill::plan(&economics, mob, boost, between_kills, &ASSUMPTIONS)
            });
        let Some(kill) = kill else {
            route.status = SkillingRouteStatus::Unmodelled;
            return Ok(route);
        };
        route.skill_per_kill_pec = round_half_even(kill.skill_pec, 4);
        route.cycled_per_kill_pec = round_half_even(kill.cycled_pec, 4);
        route.loss_per_kill_pec = round_half_even(kill.loss_pec, 4);
        route.seconds_per_kill = kill
            .seconds
            .map(|seconds| round_half_even(seconds, 2))
            .into();
        let PlannedForecast {
            status,
            cycled_ped,
            ped_lost,
            hours,
            kills,
            skills,
        } = forecast_planned(skill_levels, target, goal, &kill.rates(&shares));
        route.status = status.into();
        route.kills = kills;
        route.cycled_ped = cycled_ped;
        route.ped_lost = ped_lost;
        route.hours = hours.into();
        route.skills = skills.into_iter().map(skill_dto).collect();
        Ok(route)
    }

    /// The player's recorded time between kills on a mob maturity.
    async fn mob_pace(
        &self,
        species: &str,
        maturity: &str,
    ) -> Result<Option<SkillingRoutePace>, ApiError> {
        let paces = mob_pace(&self.db, species.trim(), maturity.trim())
            .await
            .map_err(ApiError::internal("skilling route mob pace"))?;
        let rated: Vec<_> = paces
            .into_iter()
            .map(|pace| {
                let rate = self.weapon_rate_by_name(&pace.tool);
                (pace, rate)
            })
            .collect();
        Ok(
            between_kill_seconds(&rated).map(|(seconds, kills)| SkillingRoutePace {
                between_kill_seconds: round_half_even(seconds, 2),
                kills,
            }),
        )
    }

    /// The weapons the named sessions used, most used first.
    async fn session_weapons(&self) -> Result<Vec<SkillingSessionWeapon>, ApiError> {
        let weapons: Vec<SkillingSessionWeapon> =
            session_weapons(&self.db, SESSION_WEAPON_MIN_KILLS)
                .await
                .map_err(ApiError::internal("skilling forecast session weapons"))?
                .into_iter()
                .map(|weapon| SkillingSessionWeapon {
                    name: weapon.name,
                    kills: weapon.kills,
                })
                .collect();
        Ok(weapons)
    }

    /// Every profession some catalogue weapon trains as its hit or damage
    /// profession.
    fn weapon_trained_professions(&self) -> std::collections::HashSet<String> {
        self.game_data
            .get_entities("weapons")
            .iter()
            .flat_map(weapon_professions)
            .collect()
    }

    /// The catalogue weapon a recorded tool name names.
    fn weapon_by_name(&self, name: &str) -> Option<&serde_json::Value> {
        self.game_data
            .get_entities("weapons")
            .iter()
            .find(|weapon| weapon.get("name").and_then(serde_json::Value::as_str) == Some(name))
    }

    /// The attack rate in force for a weapon named as a recorded tool.
    fn weapon_rate_by_name(&self, name: &str) -> Option<f64> {
        let id = self.weapon_by_name(name)?.get("id")?.as_str()?.to_string();
        let setup = self
            .resolve_setup(
                0,
                &SetupPick {
                    weapon: crate::loadout_comparison::ComponentPick {
                        catalog_id: id,
                        markup_percent: None,
                    },
                    amplifier: None,
                    absorber: None,
                    implant: None,
                },
            )
            .ok()?;
        self.weapon_pricing
            .attack_rate(&setup.props)
            .map(|rate| rate.effective_per_minute)
    }
}

/// What every planned route in one forecast is priced against.
#[derive(Clone, Copy)]
struct RouteContext<'a> {
    skill_levels: &'a serde_json::Map<String, serde_json::Value>,
    target: &'a ForecastTarget,
    goal: f64,
    boost: SkillBoost,
    looters: eo_services::expected_hunting::HuntingLooterLevels,
}
