//! The skilling forecast: how much more of a recorded activity it takes
//! to reach a goal on one target (a profession level or HP).
//!
//! The evidence unit is the session definition, the player's own named
//! activity family ("Carabok Skilling", "ARIS Dailies"). Every ended,
//! summarised session that ran under a definition contributes its cycled
//! spend, loot TT, and skill gains; the forecast then assumes future play
//! of that definition keeps the observed mix: the same PES earned per PED
//! cycled, split across the same skills, and the same attribute gains per
//! PED. Each skill's gain is read off the TT value curve from its current
//! calibrated level, so diminishing returns are priced in, and the cycled
//! spend that reaches the goal is found by bisection over that projection.
//!
//! Money travels in its own accounting classes. Loot TT is the observed
//! loot-only rate applied forward. Realised markup is the definition's net
//! markup from confirmed sales of its stock, expressed as a lift over the
//! same definition's loot TT: it is what this activity's loot has actually
//! returned above TT so far, never a market estimate, and it is absent
//! (not zero) when nothing from the definition has sold.

use serde_json::{Map, Value};

use crate::character_calc::{
    effective_points, is_attribute, iter_hp_skills, iter_profession_skills,
};
use crate::db::{Db, DbError};
use crate::session_summary::heal_summaries;
use crate::tt_value_curve::levels_for_tt_value;
use eo_wire::normalizer::round_half_even;

/// Below any of these the sample is flagged as thin.
const THIN_SESSIONS: usize = 3;
const THIN_HOURS: f64 = 2.0;
const THIN_CYCLED_PED: f64 = 50.0;
/// A forecast needing more than this multiple of the observed cycling
/// extrapolates far past its evidence.
const LONG_EXTRAPOLATION_FACTOR: f64 = 20.0;
/// The bisection's search ceiling: a goal not reached by this much
/// cycling is out of range for the sample.
const SEARCH_CEILING_PED: f64 = 1_000_000_000.0;

/// One ended, summarised session that ran under a definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastSession {
    pub definition_id: i64,
    pub definition_name: String,
    /// The definition has since been archived (deleted from the picker);
    /// its recorded sessions remain evidence.
    pub definition_archived: bool,
    pub hours: f64,
    pub cycled_ped: f64,
    pub loot_tt: f64,
    /// PES earned per regular skill, in first-seen order.
    pub skill_pes: Vec<(String, f64)>,
    /// Levels gained per attribute.
    pub attribute_levels: Vec<(String, f64)>,
}

/// What the forecast measures progress on: the per-level weight each
/// skill carries toward the target metric, plus the metric's current
/// value. Profession levels and HP are both linear in skill levels
/// (attributes counting ×20), so one weighted sum serves both.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastTarget {
    weights: Vec<(String, f64)>,
    current: f64,
}

impl ForecastTarget {
    /// A profession: level = Σ effective points × weight / 10000.
    pub fn profession(entity: &Value, skill_levels: &Map<String, Value>) -> Self {
        let mut weights: Vec<(String, f64)> = Vec::new();
        let mut current = 0.0;
        for (name, weight) in iter_profession_skills(entity) {
            if weight <= 0.0 {
                continue;
            }
            current += effective_points(&name, level_of(skill_levels, &name)) * weight / 10000.0;
            weights.push((
                name.clone(),
                effective_points(&name, 1.0) * weight / 10000.0,
            ));
        }
        Self { weights, current }
    }

    /// HP: each contributing skill adds one HP per `hp_increase` levels
    /// (attributes ×20). The current reading is the `Health` attribute,
    /// whose integer part is the HP the Stats panel shows and whose
    /// fraction is progress toward the next point.
    pub fn hp(skills_data: &[Value], skill_levels: &Map<String, Value>) -> Self {
        let weights = iter_hp_skills(skills_data)
            .into_iter()
            .map(|(name, hp_increase)| {
                let weight = effective_points(&name, 1.0) / hp_increase;
                (name, weight)
            })
            .collect();
        Self {
            weights,
            current: level_of(skill_levels, "Health"),
        }
    }

    /// The metric's current value, unrounded.
    pub fn current(&self) -> f64 {
        self.current
    }

    fn weight_of(&self, name: &str) -> f64 {
        lookup(&self.weights, name).unwrap_or(0.0)
    }
}

/// Why a definition cannot answer, or that it already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForecastStatus {
    /// A forecast to the goal.
    Ready,
    /// The goal is at or below the current value: nothing to cycle.
    Reached,
    /// None of the definition's recorded skill gains move the target.
    DoesNotTrain,
    /// The recorded sessions carry no cycled spend or no duration.
    NoEvidence,
    /// The goal lies beyond what the observed mix can reach (a skill
    /// ceiling on the curve, or an absurd goal).
    OutOfRange,
}

/// A sample-quality caveat on a forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleWarning {
    ThinSessions,
    ThinHours,
    ThinCycling,
    /// The forecast cycles far more than the sample observed.
    LongExtrapolation,
}

/// One skill the definition trains, projected to the goal.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastSkill {
    pub name: String,
    pub is_attribute: bool,
    pub current_level: f64,
    /// Share of the definition's regular-skill PES (none for attributes,
    /// which carry no PES).
    pub pes_share: Option<f64>,
    pub level_gain: f64,
    pub end_level: f64,
    /// The target metric this skill's gain moves (profession levels or HP).
    pub target_gain: f64,
    pub moves_target: bool,
}

/// The aggregated evidence behind one definition's forecast.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastEvidence {
    pub definition_id: i64,
    pub name: String,
    pub archived: bool,
    pub sessions: usize,
    pub hours: f64,
    pub cycled_ped: f64,
    pub loot_tt: f64,
    pub pes: f64,
    /// Net markup realised from confirmed sales of this definition's stock.
    pub realised_markup: Option<f64>,
}

impl ForecastEvidence {
    /// Realised markup as a lift over the definition's loot TT.
    pub fn markup_lift(&self) -> Option<f64> {
        let markup = self.realised_markup?;
        (self.loot_tt > 0.0).then(|| markup / self.loot_tt)
    }
}

/// One definition's forecast to the goal.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceForecast {
    pub evidence: ForecastEvidence,
    pub status: ForecastStatus,
    pub cycled_ped: f64,
    pub hours: f64,
    pub loot_tt: f64,
    /// The realised markup lift applied to the forecast loot; absent when
    /// the definition has no confirmed sales.
    pub markup: Option<f64>,
    /// Cycled minus loot TT: the TT the forecast burns.
    pub tt_cost: f64,
    /// The TT cost less realised markup; absent with the markup.
    pub net_cost: Option<f64>,
    pub skills: Vec<ForecastSkill>,
    pub warnings: Vec<SampleWarning>,
}

/// The per-PED rates a definition's sample projects forward.
struct Rates {
    /// PES per PED cycled, per regular skill.
    skills: Vec<(String, f64)>,
    /// Levels per PED cycled, per attribute.
    attributes: Vec<(String, f64)>,
}

fn level_of(skill_levels: &Map<String, Value>, name: &str) -> f64 {
    skill_levels
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn lookup(entries: &[(String, f64)], name: &str) -> Option<f64> {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|&(_, value)| value)
}

fn accumulate(entries: &mut Vec<(String, f64)>, name: &str, amount: f64) {
    match entries.iter_mut().find(|(entry, _)| entry == name) {
        Some((_, value)) => *value += amount,
        None => entries.push((name.to_string(), amount)),
    }
}

/// Skill level gains after cycling `cycled` PED at the sample's rates.
fn project(skill_levels: &Map<String, Value>, rates: &Rates, cycled: f64) -> Vec<(String, f64)> {
    let mut gains: Vec<(String, f64)> = rates
        .skills
        .iter()
        .map(|(name, rate)| {
            let gain = levels_for_tt_value(level_of(skill_levels, name), rate * cycled);
            (name.clone(), gain)
        })
        .collect();
    for (name, rate) in &rates.attributes {
        gains.push((name.clone(), rate * cycled));
    }
    gains
}

fn target_gain(target: &ForecastTarget, gains: &[(String, f64)]) -> f64 {
    gains
        .iter()
        .map(|(name, gain)| gain * target.weight_of(name))
        .sum()
}

/// The cycled spend at which the projected target gain reaches `needed`,
/// or None when it lies beyond the search ceiling. Doubling brackets the
/// crossing; bisection narrows it.
fn cycled_to_reach(
    skill_levels: &Map<String, Value>,
    target: &ForecastTarget,
    rates: &Rates,
    start: f64,
    needed: f64,
) -> Option<f64> {
    let reaches =
        |cycled: f64| target_gain(target, &project(skill_levels, rates, cycled)) >= needed;
    let mut lower = 0.0_f64;
    let mut upper = start.max(1.0);
    while !reaches(upper) {
        if upper >= SEARCH_CEILING_PED {
            return None;
        }
        lower = upper;
        upper *= 2.0;
    }
    for _ in 0..60 {
        let mid = (lower + upper) / 2.0;
        if reaches(mid) {
            upper = mid;
        } else {
            lower = mid;
        }
    }
    Some(upper)
}

/// Group sessions by definition, in first-seen order.
fn group_by_definition(sessions: &[ForecastSession]) -> Vec<Vec<&ForecastSession>> {
    let mut groups: Vec<Vec<&ForecastSession>> = Vec::new();
    for session in sessions {
        match groups
            .iter_mut()
            .find(|group| group[0].definition_id == session.definition_id)
        {
            Some(group) => group.push(session),
            None => groups.push(vec![session]),
        }
    }
    groups
}

fn forecast_source(
    skill_levels: &Map<String, Value>,
    target: &ForecastTarget,
    goal: f64,
    group: &[&ForecastSession],
    realised_markup: Option<f64>,
) -> SourceForecast {
    let first = group[0];
    let hours: f64 = group.iter().map(|s| s.hours).sum();
    let cycled: f64 = group.iter().map(|s| s.cycled_ped).sum();
    let loot: f64 = group.iter().map(|s| s.loot_tt).sum();
    let mut skill_pes: Vec<(String, f64)> = Vec::new();
    let mut attribute_levels: Vec<(String, f64)> = Vec::new();
    for session in group {
        for (name, pes) in &session.skill_pes {
            accumulate(&mut skill_pes, name, *pes);
        }
        for (name, levels) in &session.attribute_levels {
            accumulate(&mut attribute_levels, name, *levels);
        }
    }
    skill_pes.retain(|(_, pes)| *pes > 0.0);
    attribute_levels.retain(|(_, levels)| *levels > 0.0);
    let pes: f64 = skill_pes.iter().map(|(_, pes)| pes).sum();

    let evidence = ForecastEvidence {
        definition_id: first.definition_id,
        name: first.definition_name.clone(),
        archived: first.definition_archived,
        sessions: group.len(),
        hours: round_half_even(hours, 2),
        cycled_ped: round_half_even(cycled, 2),
        loot_tt: round_half_even(loot, 2),
        pes: round_half_even(pes, 2),
        realised_markup: realised_markup.map(|markup| round_half_even(markup, 2)),
    };
    let unanswered = |status: ForecastStatus| SourceForecast {
        evidence: evidence.clone(),
        status,
        cycled_ped: 0.0,
        hours: 0.0,
        loot_tt: 0.0,
        markup: None,
        tt_cost: 0.0,
        net_cost: None,
        skills: Vec::new(),
        warnings: Vec::new(),
    };

    let needed = goal - target.current();
    if needed <= 0.0 {
        return unanswered(ForecastStatus::Reached);
    }
    if cycled <= 0.0 || hours <= 0.0 {
        return unanswered(ForecastStatus::NoEvidence);
    }
    let rates = Rates {
        skills: skill_pes
            .iter()
            .map(|(name, pes)| (name.clone(), pes / cycled))
            .collect(),
        attributes: attribute_levels
            .iter()
            .map(|(name, levels)| (name.clone(), levels / cycled))
            .collect(),
    };
    let trains_target = rates
        .skills
        .iter()
        .chain(&rates.attributes)
        .any(|(name, _)| target.weight_of(name) > 0.0);
    if !trains_target {
        return unanswered(ForecastStatus::DoesNotTrain);
    }
    let Some(forecast_cycled) = cycled_to_reach(skill_levels, target, &rates, cycled, needed)
    else {
        return unanswered(ForecastStatus::OutOfRange);
    };

    let gains = project(skill_levels, &rates, forecast_cycled);
    let mut skills: Vec<ForecastSkill> = gains
        .iter()
        .map(|(name, gain)| {
            let current_level = level_of(skill_levels, name);
            let weight = target.weight_of(name);
            ForecastSkill {
                name: name.clone(),
                is_attribute: is_attribute(name),
                current_level: round_half_even(current_level, 2),
                pes_share: lookup(&skill_pes, name)
                    .filter(|_| pes > 0.0)
                    .map(|skill| round_half_even(skill / pes, 4)),
                level_gain: round_half_even(*gain, 2),
                end_level: round_half_even(current_level + gain, 2),
                target_gain: round_half_even(gain * weight, 4),
                moves_target: weight > 0.0,
            }
        })
        .collect();
    skills.sort_by(|a, b| {
        b.target_gain
            .total_cmp(&a.target_gain)
            .then_with(|| b.level_gain.total_cmp(&a.level_gain))
            .then_with(|| a.name.cmp(&b.name))
    });

    let forecast_loot = forecast_cycled * loot / cycled;
    let markup = evidence.markup_lift().map(|lift| forecast_loot * lift);
    let tt_cost = forecast_cycled - forecast_loot;

    let mut warnings = Vec::new();
    if group.len() < THIN_SESSIONS {
        warnings.push(SampleWarning::ThinSessions);
    }
    if hours < THIN_HOURS {
        warnings.push(SampleWarning::ThinHours);
    }
    if cycled < THIN_CYCLED_PED {
        warnings.push(SampleWarning::ThinCycling);
    }
    if forecast_cycled > cycled * LONG_EXTRAPOLATION_FACTOR {
        warnings.push(SampleWarning::LongExtrapolation);
    }

    SourceForecast {
        evidence,
        status: ForecastStatus::Ready,
        cycled_ped: round_half_even(forecast_cycled, 2),
        hours: round_half_even(forecast_cycled * hours / cycled, 2),
        loot_tt: round_half_even(forecast_loot, 2),
        markup: markup.map(|value| round_half_even(value, 2)),
        tt_cost: round_half_even(tt_cost, 2),
        net_cost: markup.map(|value| round_half_even(tt_cost - value, 2)),
        skills,
        warnings,
    }
}

fn status_rank(status: ForecastStatus) -> u8 {
    match status {
        ForecastStatus::Ready => 0,
        ForecastStatus::Reached => 1,
        ForecastStatus::OutOfRange => 2,
        ForecastStatus::DoesNotTrain => 3,
        ForecastStatus::NoEvidence => 4,
    }
}

/// Forecast `goal` on `target` from every definition's recorded sessions.
/// Definitions that answer come first, quickest to the goal (least
/// cycling) leading; the rest follow by how much play they recorded.
/// `realised_markup` maps a definition id to its net realised markup.
pub fn skilling_forecast(
    skill_levels: &Map<String, Value>,
    target: &ForecastTarget,
    goal: f64,
    sessions: &[ForecastSession],
    realised_markup: &[(i64, f64)],
) -> Vec<SourceForecast> {
    let mut sources: Vec<SourceForecast> = group_by_definition(sessions)
        .iter()
        .map(|group| {
            let markup = realised_markup
                .iter()
                .find(|(id, _)| *id == group[0].definition_id)
                .map(|&(_, markup)| markup);
            forecast_source(skill_levels, target, goal, group, markup)
        })
        .collect();
    sources.sort_by(|a, b| {
        status_rank(a.status)
            .cmp(&status_rank(b.status))
            .then_with(|| a.cycled_ped.total_cmp(&b.cycled_ped))
            .then_with(|| b.evidence.cycled_ped.total_cmp(&a.evidence.cycled_ped))
            .then_with(|| a.evidence.name.cmp(&b.evidence.name))
    });
    sources
}

fn json_pairs(text: Option<String>) -> Vec<(String, f64)> {
    // The summary is a rebuildable cache: an unreadable map reads as no
    // gains rather than failing the whole forecast.
    text.filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text).ok())
        .map(|map| {
            map.into_iter()
                .map(|(name, value)| (name, value.as_f64().unwrap_or(0.0)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every ended, summarised session that ran under a definition, healing
/// missing or stale summaries first (the prospect loader's contract).
pub async fn load_forecast_sessions(db: &Db) -> Result<Vec<ForecastSession>, DbError> {
    db.with_writer(|conn| heal_summaries(conn)).await?;
    db.with_reader(|conn| {
        let mut stmt = conn.prepare(
            "SELECT s.definition_id, d.name, d.is_active, ss.duration_hours, ss.cycled_ped, \
                    ss.loot_tt, ss.regular_skill_ped_json, ss.attribute_levels_json \
             FROM session_summaries ss \
             JOIN tracking_sessions s ON s.id = ss.session_id \
             JOIN session_definitions d ON d.id = s.definition_id \
             ORDER BY s.started_at, s.id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ForecastSession {
                    definition_id: row.get(0)?,
                    definition_name: row.get(1)?,
                    definition_archived: row.get::<_, i64>(2)? == 0,
                    hours: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    cycled_ped: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                    loot_tt: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    skill_pes: json_pairs(row.get(6)?),
                    attribute_levels: json_pairs(row.get(7)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn levels(pairs: &[(&str, f64)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(name, level)| (name.to_string(), json!(level)))
            .collect()
    }

    fn marksman() -> Value {
        json!({"name": "Marksman", "skills": [
            {"skill": {"name": "Rifle"}, "weight": 40},
            {"skill": {"name": "Anatomy"}, "weight": 10},
            {"skill": {"name": "Agility"}, "weight": 5},
        ]})
    }

    fn session(id: i64, name: &str, cycled: f64, skills: &[(&str, f64)]) -> ForecastSession {
        ForecastSession {
            definition_id: id,
            definition_name: name.to_string(),
            definition_archived: false,
            hours: 2.0,
            cycled_ped: cycled,
            loot_tt: cycled * 0.9,
            skill_pes: skills.iter().map(|(n, p)| (n.to_string(), *p)).collect(),
            attribute_levels: Vec::new(),
        }
    }

    fn start() -> Map<String, Value> {
        levels(&[
            ("Rifle", 1000.0),
            ("Anatomy", 800.0),
            ("Agility", 20.0),
            ("Health", 120.4),
        ])
    }

    #[test]
    fn a_profession_target_weighs_attributes_twenty_fold() {
        let target = ForecastTarget::profession(&marksman(), &start());
        // (1000*40 + 800*10 + 20*20*5) / 10000
        assert!((target.current() - 5.0).abs() < 1e-9);
        assert!((target.weight_of("Agility") - 0.01).abs() < 1e-12);
        assert!((target.weight_of("Rifle") - 0.004).abs() < 1e-12);
        assert_eq!(target.weight_of("Mining"), 0.0);
    }

    #[test]
    fn an_hp_target_reads_health_and_skips_non_contributors() {
        let skills = json!([
            {"name": "Rifle", "hp_increase": 1600},
            {"name": "Stamina", "hp_increase": 9.25},
            {"name": "Health", "hp_increase": 0},
        ]);
        let target = ForecastTarget::hp(skills.as_array().unwrap(), &start());
        assert!((target.current() - 120.4).abs() < 1e-12);
        assert!((target.weight_of("Rifle") - 1.0 / 1600.0).abs() < 1e-15);
        assert!((target.weight_of("Stamina") - 20.0 / 9.25).abs() < 1e-12);
        assert_eq!(target.weight_of("Health"), 0.0);
    }

    #[test]
    fn the_forecast_reaches_the_goal_at_the_bisected_cycling() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let sessions = [session(
            7,
            "Rifle work",
            200.0,
            &[("Rifle", 4.0), ("Anatomy", 1.0)],
        )];
        let sources = skilling_forecast(&skill_levels, &target, 5.5, &sessions, &[]);
        assert_eq!(sources.len(), 1);
        let source = &sources[0];
        assert_eq!(source.status, ForecastStatus::Ready);
        assert!(source.cycled_ped > 0.0);

        // Re-project at the answer: the goal is met by the cent-rounded
        // answer (plus the cent its rounding may have shaved), and a hair
        // less cycling falls short.
        let rates = Rates {
            skills: vec![
                ("Rifle".into(), 4.0 / 200.0),
                ("Anatomy".into(), 1.0 / 200.0),
            ],
            attributes: Vec::new(),
        };
        let reached = target_gain(
            &target,
            &project(&skill_levels, &rates, source.cycled_ped + 0.01),
        );
        assert!(reached >= 0.5 - 1e-9, "reached {reached} at {}", source.cycled_ped);
        let short = target_gain(
            &target,
            &project(&skill_levels, &rates, source.cycled_ped * 0.99),
        );
        assert!(short < 0.5, "short {short} at {}", source.cycled_ped);

        // Time and loot scale with the sample's per-PED rates.
        assert!((source.hours - source.cycled_ped * 2.0 / 200.0).abs() < 0.01);
        assert!((source.loot_tt - source.cycled_ped * 0.9).abs() < 0.01);
        assert!((source.tt_cost - (source.cycled_ped - source.loot_tt)).abs() < 0.011);
        // No confirmed sales: no markup, no net cost; never a zero.
        assert_eq!(source.markup, None);
        assert_eq!(source.net_cost, None);

        // Skills lead with what moves the target most; shares partition PES.
        assert_eq!(source.skills[0].name, "Rifle");
        assert_eq!(source.skills[0].pes_share, Some(0.8));
        assert!(source.skills.iter().all(|skill| skill.moves_target));
        // Thin on sessions (one) but not on hours or cycling.
        assert_eq!(source.warnings, vec![SampleWarning::ThinSessions]);
    }

    #[test]
    fn realised_markup_lifts_loot_and_lowers_the_net_cost() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let sessions = [session(7, "Rifle work", 200.0, &[("Rifle", 5.0)])];
        // 18 PED net markup on 180 PED loot TT: a 10% lift.
        let sources = skilling_forecast(&skill_levels, &target, 5.5, &sessions, &[(7, 18.0)]);
        let source = &sources[0];
        assert_eq!(source.evidence.markup_lift(), Some(0.1));
        let markup = source.markup.unwrap();
        assert!((markup - source.loot_tt * 0.1).abs() < 0.011);
        assert!((source.net_cost.unwrap() - (source.tt_cost - markup)).abs() < 0.011);
    }

    #[test]
    fn definitions_that_cannot_answer_say_why_and_trail_the_ready_ones() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let mut idle = session(3, "Idle", 0.0, &[("Rifle", 1.0)]);
        idle.hours = 0.0;
        let sessions = [
            session(1, "Mining", 500.0, &[("Surveying", 3.0)]),
            session(2, "Slow rifle", 400.0, &[("Rifle", 1.0)]),
            idle,
            session(4, "Fast rifle", 100.0, &[("Rifle", 5.0)]),
        ];
        let sources = skilling_forecast(&skill_levels, &target, 5.2, &sessions, &[]);
        let order: Vec<(&str, ForecastStatus)> = sources
            .iter()
            .map(|s| (s.evidence.name.as_str(), s.status))
            .collect();
        assert_eq!(
            order,
            vec![
                ("Fast rifle", ForecastStatus::Ready),
                ("Slow rifle", ForecastStatus::Ready),
                ("Mining", ForecastStatus::DoesNotTrain),
                ("Idle", ForecastStatus::NoEvidence),
            ]
        );
        assert!(sources[0].cycled_ped < sources[1].cycled_ped);
    }

    #[test]
    fn a_goal_already_met_needs_no_cycling() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let sessions = [session(7, "Rifle work", 200.0, &[("Rifle", 5.0)])];
        let sources = skilling_forecast(&skill_levels, &target, 4.0, &sessions, &[]);
        assert_eq!(sources[0].status, ForecastStatus::Reached);
        assert_eq!(sources[0].cycled_ped, 0.0);
        assert!(sources[0].skills.is_empty());
    }

    #[test]
    fn a_goal_past_every_skill_ceiling_is_out_of_range() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let sessions = [session(7, "Rifle work", 200.0, &[("Rifle", 5.0)])];
        let sources = skilling_forecast(&skill_levels, &target, 10_000.0, &sessions, &[]);
        assert_eq!(sources[0].status, ForecastStatus::OutOfRange);
    }

    #[test]
    fn attribute_gains_count_toward_the_target_at_their_observed_rate() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let mut only_agility = session(9, "Agility", 100.0, &[]);
        only_agility.attribute_levels = vec![("Agility".into(), 0.5)];
        let sources = skilling_forecast(&skill_levels, &target, 5.01, &[only_agility], &[]);
        let source = &sources[0];
        assert_eq!(source.status, ForecastStatus::Ready);
        // 0.01 profession levels at 0.01 per Agility level: one level, at
        // 0.005 levels per PED, is 200 PED.
        assert!((source.cycled_ped - 200.0).abs() < 0.01);
        assert_eq!(source.skills[0].pes_share, None);
        assert!(source.skills[0].is_attribute);
    }

    #[test]
    fn sessions_pool_per_definition() {
        let skill_levels = start();
        let target = ForecastTarget::profession(&marksman(), &skill_levels);
        let sessions = [
            session(7, "Rifle work", 100.0, &[("Rifle", 2.0)]),
            session(8, "Other", 100.0, &[("Anatomy", 2.0)]),
            session(7, "Rifle work", 150.0, &[("Rifle", 3.0), ("Anatomy", 1.0)]),
        ];
        let sources = skilling_forecast(&skill_levels, &target, 5.1, &sessions, &[]);
        let rifle = sources
            .iter()
            .find(|s| s.evidence.definition_id == 7)
            .unwrap();
        assert_eq!(rifle.evidence.sessions, 2);
        assert_eq!(rifle.evidence.cycled_ped, 250.0);
        assert_eq!(rifle.evidence.pes, 6.0);
        assert_eq!(rifle.warnings, vec![SampleWarning::ThinSessions]);
    }
}
