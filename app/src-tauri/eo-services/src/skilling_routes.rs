//! Skilling routes: what a weapon on a mob maturity yields in skill, and
//! what each kill costs, so a skilling goal can be priced before any of it
//! is played.
//!
//! Skill per kill is set by the mob alone: a kill bonus proportional to the
//! mob's level plus skill earned per HP removed, fitted on recorded play
//! from 2 HP mobs to level-43 ones. The weapon does not change it; the
//! weapon decides what the kill costs and which professions receive the
//! skill, split across their skills by weight. Cost, expected loss, and
//! firing time per kill come from the loadout comparison engine at the
//! maturity's HP, so a route is priced exactly as the Equipment comparison
//! prices the same setup.
//!
//! Once the player has recorded enough kills of a weapon on a maturity,
//! [`measured_route`] reads what those kills actually yielded and cost, so
//! a route can be priced from the player's own play instead of the model.

use std::collections::{HashMap, HashSet};

use rusqlite::params;
use serde_json::{Map, Value};

use crate::db::{Db, DbError};

use crate::character_calc::{is_attribute, iter_profession_skills, ATTRIBUTE_SKILLS};
use crate::loadout_comparison::{kill_figures, ComparisonAssumptions, KillCurve, SetupEconomics};
use crate::skilling_forecast::PlannedRates;

/// PES per kill per mob level, PEC, at a 50% skill pill.
pub const KILL_BONUS_PEC_PER_LEVEL: f64 = 0.121;
/// PES per HP removed, PEC, at a 50% skill pill.
pub const PES_PEC_PER_HP: f64 = 0.0033;
/// Seconds between kills spent on anything but firing (finding and
/// reaching the next mob), allowed for every planned kill.
pub const RETARGET_SECONDS_PER_KILL: f64 = 1.0;

/// The skill pill in force: none, the 50% pill, or the 100% pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillBoost {
    None,
    Fifty,
    Hundred,
}

impl SkillBoost {
    /// Skill gained relative to the 50% pill the law was fitted at. The
    /// 50% pill measured about x1.5 over none; the 100% pill measured
    /// x1.21 to x1.33 over the 50% one.
    pub fn factor(self) -> f64 {
        match self {
            Self::None => 1.0 / 1.5,
            Self::Fifty => 1.0,
            Self::Hundred => 1.27,
        }
    }

    /// The pill a session's recorded boost percent stands for. The game
    /// caps the stacked boost at 100%.
    pub fn from_percent(percent: Option<i64>) -> Self {
        match percent {
            Some(percent) if percent >= 100 => Self::Hundred,
            Some(percent) if percent > 0 => Self::Fifty,
            _ => Self::None,
        }
    }
}

/// A mob maturity's HP and level, as the catalogue records them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MobMaturity {
    pub hp: f64,
    pub level: f64,
}

/// PES one kill yields, PEC.
pub fn skill_per_kill_pec(mob: MobMaturity, boost: SkillBoost) -> f64 {
    (KILL_BONUS_PEC_PER_LEVEL * mob.level + PES_PEC_PER_HP * mob.hp) * boost.factor()
}

/// The hit and damage professions a weapon trains, as the catalogue names
/// them.
pub fn weapon_professions(weapon: &Value) -> Vec<String> {
    let Some(professions) = weapon.get("professions") else {
        return Vec::new();
    };
    ["hit", "dmg"]
        .iter()
        .filter_map(|key| professions.get(key).and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// How a weapon's skill gain splits across skills: its professions'
/// weights summed per skill, over regular skills the player has unlocked
/// (a locked skill gains nothing and its share goes to the rest), as
/// fractions summing to one. Empty when the weapon trains nothing known.
pub fn skill_shares(
    professions: &[Value],
    names: &[String],
    skill_levels: &Map<String, Value>,
) -> Vec<(String, f64)> {
    let mut shares: Vec<(String, f64)> = Vec::new();
    for name in names {
        let Some(profession) = professions
            .iter()
            .find(|entity| entity.get("name").and_then(Value::as_str) == Some(name.as_str()))
        else {
            continue;
        };
        for (skill, weight) in iter_profession_skills(profession) {
            let unlocked = skill_levels
                .get(&skill)
                .and_then(Value::as_f64)
                .is_some_and(|level| level > 0.0);
            if weight <= 0.0 || is_attribute(&skill) || !unlocked {
                continue;
            }
            match shares.iter_mut().find(|(entry, _)| *entry == skill) {
                Some((_, total)) => *total += weight,
                None => shares.push((skill, weight)),
            }
        }
    }
    let total: f64 = shares.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    for (_, weight) in &mut shares {
        *weight /= total;
    }
    shares
}

/// One kill on a route, as planned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteKill {
    /// PES the kill yields, PEC.
    pub skill_pec: f64,
    /// PED cycled on the kill, PEC.
    pub cycled_pec: f64,
    /// Expected loss on the kill, PEC.
    pub loss_pec: f64,
    /// Seconds per kill including the retarget allowance; None when the
    /// weapon's attack rate is unknown.
    pub seconds: Option<f64>,
}

impl RouteKill {
    /// A setup's kill on a mob maturity. None when the setup's damage
    /// cannot define a kill curve or the maturity has no HP.
    pub fn plan(
        setup: &SetupEconomics,
        mob: MobMaturity,
        boost: SkillBoost,
        assumptions: &ComparisonAssumptions,
    ) -> Option<Self> {
        if !(mob.hp.is_finite() && mob.hp > 0.0) {
            return None;
        }
        let curve = KillCurve::solve(setup.max_damage, mob.hp, assumptions)?;
        let figures = kill_figures(setup, &curve, mob.hp);
        if !(figures.cycled_pec.is_finite() && figures.cycled_pec > 0.0) {
            return None;
        }
        Some(Self {
            skill_pec: skill_per_kill_pec(mob, boost),
            cycled_pec: figures.cycled_pec,
            loss_pec: figures.loss_pec,
            seconds: figures
                .seconds
                .map(|seconds| seconds + RETARGET_SECONDS_PER_KILL),
        })
    }

    /// The route as per-PED rates the forecast solver projects: each
    /// skill's PES per PED cycled, and the loss and time per PED.
    pub fn rates(&self, shares: &[(String, f64)]) -> PlannedRates {
        let pes_per_ped = self.skill_pec / self.cycled_pec;
        PlannedRates {
            skills: shares
                .iter()
                .map(|(name, share)| (name.clone(), share * pes_per_ped))
                .collect(),
            loss_per_ped: self.loss_pec / self.cycled_pec,
            hours_per_ped: self
                .seconds
                .map(|seconds| seconds * 100.0 / self.cycled_pec / 3600.0),
            kills_per_ped: 100.0 / self.cycled_pec,
        }
    }
}

/// Fewest recorded kills of a weapon on a maturity before a route is
/// priced from them rather than from the model.
pub const MIN_MEASURED_KILLS: u64 = 300;
/// The longest gap between kills counted as time spent on the route;
/// longer ones are breaks.
const MAX_KILL_GAP_SECONDS: f64 = 60.0;

/// What the player's own kills of a weapon on a maturity yielded and cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredRoute {
    pub kills: u64,
    pub sessions: u64,
    /// PES per kill, PEC, restated at the 50% pill from each session's own.
    pub skill_pec_at_fifty: f64,
    pub cycled_pec: f64,
    /// Mean seconds from the previous kill, breaks left out; None when no
    /// kill had a recorded predecessor within reach.
    pub seconds: Option<f64>,
}

impl MeasuredRoute {
    /// The route's kill from this evidence: measured skill (at `boost`),
    /// cycling, and time, with loss at the setup's modelled loss rate on
    /// the measured cycling (recorded loot is too noisy to price a route).
    pub fn kill(&self, planned: &RouteKill, boost: SkillBoost) -> RouteKill {
        let loss_rate = planned.loss_pec / planned.cycled_pec;
        RouteKill {
            skill_pec: self.skill_pec_at_fifty * boost.factor(),
            cycled_pec: self.cycled_pec,
            loss_pec: self.cycled_pec * loss_rate,
            seconds: self.seconds.or(planned.seconds),
        }
    }
}

/// A recorded kill: its timestamp, PEC cycled, and session skill boost.
type RecordedKill = (f64, f64, Option<i64>);

/// The newest recorded kill's timestamp: recorded evidence can only have
/// changed since a read if this has.
pub async fn latest_kill_timestamp(db: &Db) -> Result<Option<f64>, DbError> {
    db.with_reader(|conn| {
        Ok(
            conn.query_row("SELECT MAX(timestamp) FROM kills", [], |row| {
                row.get::<_, Option<f64>>(0)
            })?,
        )
    })
    .await
}

/// The player's single-weapon kills of `tool` on one maturity, when there
/// are at least [`MIN_MEASURED_KILLS`]. A kill's damage must fit the
/// maturity (from its HP to `max_overkill` above it), because a session
/// declared as one mob can hold others. Skill gains are attributed to the
/// kill they precede in their session.
pub async fn measured_route(
    db: &Db,
    tool: &str,
    species: &str,
    maturity: &str,
    hp: f64,
    max_overkill: f64,
) -> Result<Option<MeasuredRoute>, DbError> {
    let (tool, species, maturity) = (tool.to_string(), species.to_string(), maturity.to_string());
    db.with_reader(move |conn| {
        // (session, timestamp, PEC cycled, skill boost percent)
        let mut stmt = conn.prepare(
            "SELECT k.session_id, k.timestamp, \
                    (COALESCE(k.cost_ped, 0) + COALESCE(k.enhancer_cost, 0)) * 100.0, \
                    s.skill_boost_percent \
             FROM kills k \
             JOIN tracking_sessions s ON s.id = k.session_id \
             WHERE k.mob_species = ?1 AND k.mob_maturity = ?2 \
               AND k.damage_dealt >= ?4 * 0.9 AND k.damage_dealt <= ?4 + ?5 \
               AND EXISTS (SELECT 1 FROM kill_tool_stats t \
                           WHERE t.kill_id = k.id AND t.tool_name = ?3) \
               AND NOT EXISTS (SELECT 1 FROM kill_tool_stats t \
                               WHERE t.kill_id = k.id AND t.tool_name <> ?3)",
        )?;
        let candidates = stmt
            .query_map(params![species, maturity, tool, hp, max_overkill], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if (candidates.len() as u64) < MIN_MEASURED_KILLS {
            return Ok(None);
        }
        let mut by_session: HashMap<&str, Vec<RecordedKill>> = HashMap::new();
        for (session, timestamp, cycled, boost) in &candidates {
            by_session
                .entry(session.as_str())
                .or_default()
                .push((*timestamp, *cycled, *boost));
        }

        let attributes = ATTRIBUTE_SKILLS
            .iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut kills_stmt =
            conn.prepare("SELECT timestamp FROM kills WHERE session_id = ?1 ORDER BY timestamp")?;
        let mut gains_stmt = conn.prepare(&format!(
            "SELECT timestamp, ped_value FROM skill_gains \
             WHERE session_id = ?1 AND ped_value IS NOT NULL AND skill_name NOT IN ({attributes})"
        ))?;
        let (mut skill_at_fifty, mut cycled_total) = (0.0, 0.0);
        let (mut gap_total, mut gaps) = (0.0, 0u64);
        for (session, kills) in &by_session {
            let all: Vec<f64> = kills_stmt
                .query_map(params![session], |row| row.get::<_, f64>(0))?
                .collect::<rusqlite::Result<_>>()?;
            // Skill gained up to each kill of the session, since the one before.
            let mut skill_by_kill = vec![0.0; all.len()];
            for gain in gains_stmt.query_map(params![session], |row| {
                Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
            })? {
                let (timestamp, pes) = gain?;
                let index = all.partition_point(|kill| *kill < timestamp);
                if let Some(slot) = skill_by_kill.get_mut(index) {
                    *slot += pes;
                }
            }
            // Kills sharing a second each take their own slot.
            let mut taken = HashSet::new();
            for (timestamp, cycled, boost) in kills {
                let mut index = all.partition_point(|kill| kill < timestamp);
                while taken.contains(&index) && all.get(index + 1) == Some(timestamp) {
                    index += 1;
                }
                taken.insert(index);
                let factor = SkillBoost::from_percent(*boost).factor();
                skill_at_fifty += skill_by_kill.get(index).copied().unwrap_or(0.0) * 100.0 / factor;
                cycled_total += cycled;
                if let Some(previous) = index.checked_sub(1).and_then(|i| all.get(i)) {
                    let gap = timestamp - previous;
                    if gap > 0.0 && gap <= MAX_KILL_GAP_SECONDS {
                        gap_total += gap;
                        gaps += 1;
                    }
                }
            }
        }
        let kills = candidates.len() as u64;
        Ok(Some(MeasuredRoute {
            kills,
            sessions: by_session.len() as u64,
            skill_pec_at_fifty: skill_at_fifty / kills as f64,
            cycled_pec: cycled_total / kills as f64,
            seconds: (gaps > 0).then(|| gap_total / gaps as f64),
        }))
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

    #[test]
    fn skill_per_kill_follows_level_and_hp_scaled_by_the_pill() {
        let robber = MobMaturity {
            hp: 2.0,
            level: 4.0,
        };
        let mortirex = MobMaturity {
            hp: 1380.0,
            level: 31.0,
        };
        assert!((skill_per_kill_pec(robber, SkillBoost::Fifty) - 0.4906).abs() < 1e-12);
        assert!((skill_per_kill_pec(mortirex, SkillBoost::Fifty) - 8.305).abs() < 1e-9);
        let none = skill_per_kill_pec(robber, SkillBoost::None);
        assert!((none * 1.5 - 0.4906).abs() < 1e-12);
        assert!(skill_per_kill_pec(robber, SkillBoost::Hundred) > 0.4906);
    }

    #[test]
    fn a_recorded_boost_percent_names_its_pill() {
        assert_eq!(SkillBoost::from_percent(None), SkillBoost::None);
        assert_eq!(SkillBoost::from_percent(Some(0)), SkillBoost::None);
        assert_eq!(SkillBoost::from_percent(Some(50)), SkillBoost::Fifty);
        assert_eq!(SkillBoost::from_percent(Some(100)), SkillBoost::Hundred);
        assert_eq!(SkillBoost::from_percent(Some(150)), SkillBoost::Hundred);
    }

    #[test]
    fn a_weapon_names_its_hit_and_damage_professions() {
        let sword = json!({"professions": {"hit": "Swordsman (Hit)", "dmg": "Swordsman (Dmg)"}});
        assert_eq!(
            weapon_professions(&sword),
            vec!["Swordsman (Hit)".to_string(), "Swordsman (Dmg)".to_string()]
        );
        assert!(weapon_professions(&json!({"name": "Old Row"})).is_empty());
        let half = json!({"professions": {"hit": "Swordsman (Hit)", "dmg": null}});
        assert_eq!(
            weapon_professions(&half),
            vec!["Swordsman (Hit)".to_string()]
        );
    }

    /// Weights sum per skill across both panels; attributes and locked
    /// skills take no share, and the rest renormalise to one.
    #[test]
    fn skill_shares_split_by_weight_over_unlocked_regular_skills() {
        let professions = vec![
            json!({"name": "Hit", "skills": [
                {"skill": {"name": "Longblades"}, "weight": 60},
                {"skill": {"name": "Agility"}, "weight": 10},
                {"skill": {"name": "Melee Combat"}, "weight": 30}]}),
            json!({"name": "Dmg", "skills": [
                {"skill": {"name": "Longblades"}, "weight": 40},
                {"skill": {"name": "Kill Strike"}, "weight": 20},
                {"skill": {"name": "Anatomy"}, "weight": 30}]}),
        ];
        let skill_levels = levels(&[
            ("Longblades", 100.0),
            ("Melee Combat", 50.0),
            ("Anatomy", 70.0),
            ("Agility", 40.0),
            ("Kill Strike", 0.0),
        ]);
        let shares = skill_shares(
            &professions,
            &["Hit".to_string(), "Dmg".to_string()],
            &skill_levels,
        );
        let share = |name: &str| shares.iter().find(|(n, _)| n == name).map(|(_, s)| *s);
        assert_eq!(shares.len(), 3);
        assert!((share("Longblades").unwrap() - 100.0 / 160.0).abs() < 1e-12);
        assert!((share("Melee Combat").unwrap() - 30.0 / 160.0).abs() < 1e-12);
        assert_eq!(share("Agility"), None);
        assert_eq!(share("Kill Strike"), None);
        assert!(skill_shares(&professions, &["Nope".to_string()], &skill_levels).is_empty());
    }

    #[test]
    fn a_route_kill_becomes_per_ped_rates() {
        let kill = RouteKill {
            skill_pec: 0.5,
            cycled_pec: 2.0,
            loss_pec: 0.16,
            seconds: Some(2.0),
        };
        let rates = kill.rates(&[
            ("Longblades".to_string(), 0.75),
            ("Anatomy".to_string(), 0.25),
        ]);
        // 0.5 PEC of skill per 2 PEC cycled: 0.25 PES per PED, split.
        assert_eq!(
            rates.skills,
            vec![
                ("Longblades".to_string(), 0.1875),
                ("Anatomy".to_string(), 0.0625)
            ]
        );
        assert!((rates.loss_per_ped - 0.08).abs() < 1e-12);
        assert!((rates.kills_per_ped - 50.0).abs() < 1e-12);
        // 50 kills of 2 seconds per PED.
        assert!((rates.hours_per_ped.unwrap() - 100.0 / 3600.0).abs() < 1e-12);
    }
}
