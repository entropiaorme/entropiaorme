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
//! The one thing the model cannot know is the player's own pace between
//! kills on a mob: spawn density, walking, sharing a public spawn. That
//! comes from their recorded kills of the mob ([`mob_pace`]), whatever
//! weapon they used, so it carries over to a weapon never used there.

use serde_json::{Map, Value};

use crate::db::{Db, DbError};

use crate::character_calc::{is_attribute, iter_profession_skills};
use crate::loadout_comparison::{kill_figures, ComparisonAssumptions, KillCurve, SetupEconomics};
use crate::skilling_forecast::PlannedRates;

/// PES per kill per mob level, PEC, at a 50% skill pill.
pub const KILL_BONUS_PEC_PER_LEVEL: f64 = 0.121;
/// PES per HP removed, PEC, at a 50% skill pill.
pub const PES_PEC_PER_HP: f64 = 0.0033;
/// The part of a kill's skill that lands in the weapon's own hit and damage
/// professions. Recorded single-weapon hunting puts about 98% there; the
/// rest goes to looter skills (Skinning, Butchering) and, when the mob
/// lands hits, defensive ones, which a route does not model.
pub const WEAPON_PROFESSION_SHARE: f64 = 0.98;
/// Seconds between kills spent on anything but firing (finding and
/// reaching the next mob), allowed for every kill on a mob the player has
/// no recorded pace for.
pub const RETARGET_SECONDS_PER_KILL: f64 = 1.0;
/// Fewest recorded kills of a mob before its pace stands in for the
/// allowance.
pub const MIN_PACE_KILLS: u64 = 100;

/// The skill boost in force, a whole percent from 0 to 100. The game caps
/// stacked boosts at 100%, whatever its display shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillBoost(u8);

impl SkillBoost {
    pub const NONE: Self = Self(0);
    pub const FIFTY: Self = Self(50);
    pub const HUNDRED: Self = Self(100);
    pub const MAX_PERCENT: u8 = 100;

    /// A boost of `percent`, capped at 100%.
    pub fn new(percent: u8) -> Self {
        Self(percent.min(Self::MAX_PERCENT))
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    /// Skill gained relative to the 50% boost the law was fitted at,
    /// interpolated between the measured points: none about 1 / 1.5 of it,
    /// the 100% boost x1.21 to x1.33 over it (1.27 taken).
    pub fn factor(self) -> f64 {
        const NONE: f64 = 1.0 / 1.5;
        const HUNDRED: f64 = 1.27;
        let percent = f64::from(self.0);
        if percent <= 50.0 {
            NONE + (1.0 - NONE) * percent / 50.0
        } else {
            1.0 + (HUNDRED - 1.0) * (percent - 50.0) / 50.0
        }
    }

    /// The boost a session recorded; none recorded reads as no boost.
    pub fn from_percent(percent: Option<i64>) -> Self {
        let percent = percent.unwrap_or(0).clamp(0, i64::from(Self::MAX_PERCENT));
        Self::new(u8::try_from(percent).unwrap_or(0))
    }
}

/// The markup a planned route expects its loot to sell at, as a percent of
/// TT (104 for 104%): the player's own planning assumption, never a
/// measurement and never realised. TT is always on offer, so it starts at
/// 100%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LootMarkup(f64);

impl LootMarkup {
    pub const MIN_PERCENT: f64 = 100.0;
    pub const MAX_PERCENT: f64 = 1000.0;

    /// A markup of `percent`; None outside 100% to 1000%.
    pub fn from_percent(percent: f64) -> Option<Self> {
        (percent.is_finite() && (Self::MIN_PERCENT..=Self::MAX_PERCENT).contains(&percent))
            .then_some(Self(percent))
    }

    pub fn percent(self) -> f64 {
        self.0
    }

    /// The markup over TT as a fraction of the loot TT (0.04 at 104%).
    pub fn lift(self) -> f64 {
        self.0 / 100.0 - 1.0
    }
}

/// The share of the clock a route spends hunting, as a percent: a timed
/// instance hunted 10 minutes in every 13 runs at about 77%. Recorded pace
/// leaves breaks out, so downtime a route always carries is stated here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HuntingShare(f64);

impl HuntingShare {
    pub const FULL: Self = Self(100.0);
    pub const MIN_PERCENT: f64 = 1.0;

    /// A share of `percent`; None outside 1% to 100%.
    pub fn from_percent(percent: f64) -> Option<Self> {
        (percent.is_finite() && (Self::MIN_PERCENT..=100.0).contains(&percent))
            .then_some(Self(percent))
    }

    pub fn percent(self) -> f64 {
        self.0
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
    /// Seconds per kill: firing plus the time between kills; None when the
    /// weapon's attack rate is unknown.
    pub seconds: Option<f64>,
    /// The share of the damage fired that lands past the kill, and so of
    /// the PED cycled, the loss, and the firing time.
    pub overkill_share: f64,
}

impl RouteKill {
    /// A setup's kill on a mob maturity, with `between_kills` seconds spent
    /// reaching each next mob. None when the setup's damage cannot define a
    /// kill curve or the maturity has no HP.
    pub fn plan(
        setup: &SetupEconomics,
        mob: MobMaturity,
        boost: SkillBoost,
        between_kills: f64,
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
            seconds: figures.seconds.map(|seconds| seconds + between_kills),
            overkill_share: figures.overkill_share,
        })
    }

    /// The route as per-PED rates the forecast solver projects: each
    /// skill's PES per PED cycled (the weapon professions' part of the
    /// kill's skill), the loss per PED, and the time per PED stretched to
    /// the clock by the `hunting` share. An expected `markup` carries
    /// through as its lift over the loot TT.
    pub fn rates(
        &self,
        shares: &[(String, f64)],
        hunting: HuntingShare,
        markup: Option<LootMarkup>,
    ) -> PlannedRates {
        let pes_per_ped = self.skill_pec * WEAPON_PROFESSION_SHARE / self.cycled_pec;
        PlannedRates {
            skills: shares
                .iter()
                .map(|(name, share)| (name.clone(), share * pes_per_ped))
                .collect(),
            loss_per_ped: self.loss_pec / self.cycled_pec,
            hours_per_ped: self.seconds.map(|seconds| {
                seconds * 100.0 / self.cycled_pec / 3600.0 * 100.0 / hunting.percent()
            }),
            kills_per_ped: 100.0 / self.cycled_pec,
            markup_lift: markup.map(LootMarkup::lift),
        }
    }
}

/// The player's recorded pace with one weapon on a mob maturity.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPace {
    pub tool: String,
    pub kills: u64,
    /// Mean seconds from the previous kill in the session, breaks left out.
    pub mean_gap: f64,
    pub mean_shots: f64,
}

/// The player's pace on a mob maturity with each weapon, from the
/// weapon's own kills there that came within a break of the kill before.
pub async fn mob_pace(db: &Db, species: &str, maturity: &str) -> Result<Vec<ToolPace>, DbError> {
    db.with_writer(crate::session_rollup::heal).await?;
    let (species, maturity) = (species.to_string(), maturity.to_string());
    db.with_reader(move |conn| {
        let mut paces: Vec<ToolPace> = Vec::new();
        let mut totals: Vec<(String, u64, f64, f64)> = Vec::new();
        for cell in crate::session_weapon_rollup::effective_pace_cells(conn, &species, &maturity)? {
            match totals.iter_mut().find(|(tool, ..)| *tool == cell.tool) {
                Some((_, kills, gaps, shots)) => {
                    *kills += cell.kills;
                    *gaps += cell.gap_seconds;
                    *shots += cell.shots;
                }
                None => totals.push((cell.tool, cell.kills, cell.gap_seconds, cell.shots)),
            }
        }
        for (tool, kills, gaps, shots) in totals {
            if kills > 0 {
                paces.push(ToolPace {
                    tool,
                    kills,
                    mean_gap: gaps / kills as f64,
                    mean_shots: shots / kills as f64,
                });
            }
        }
        Ok(paces)
    })
    .await
}

/// The time between kills on a mob, from the recorded pace with each
/// weapon less that weapon's own firing time (`uses_per_minute` of the
/// weapon named, None when it cannot be resolved), weighted by kills.
/// None below [`MIN_PACE_KILLS`] resolvable kills.
pub fn between_kill_seconds(paces: &[(ToolPace, Option<f64>)]) -> Option<(f64, u64)> {
    let (mut weighted, mut kills) = (0.0, 0u64);
    for (pace, rate) in paces {
        let Some(rate) = rate.filter(|rate| rate.is_finite() && *rate > 0.0) else {
            continue;
        };
        let firing = pace.mean_shots * 60.0 / rate;
        weighted += (pace.mean_gap - firing).max(0.0) * pace.kills as f64;
        kills += pace.kills;
    }
    (kills >= MIN_PACE_KILLS).then(|| (weighted / kills as f64, kills))
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
        assert!((skill_per_kill_pec(robber, SkillBoost::FIFTY) - 0.4906).abs() < 1e-12);
        assert!((skill_per_kill_pec(mortirex, SkillBoost::FIFTY) - 8.305).abs() < 1e-9);
        let none = skill_per_kill_pec(robber, SkillBoost::NONE);
        assert!((none * 1.5 - 0.4906).abs() < 1e-12);
        assert!(skill_per_kill_pec(robber, SkillBoost::HUNDRED) > 0.4906);
    }

    #[test]
    fn a_boost_between_the_measured_points_interpolates() {
        assert!((SkillBoost::new(25).factor() - (1.0 / 1.5 + 1.0) / 2.0).abs() < 1e-12);
        assert!((SkillBoost::new(75).factor() - 1.135).abs() < 1e-12);
        assert!((SkillBoost::HUNDRED.factor() - 1.27).abs() < 1e-12);
    }

    #[test]
    fn a_recorded_boost_percent_is_capped_at_the_game_limit() {
        assert_eq!(SkillBoost::from_percent(None), SkillBoost::NONE);
        assert_eq!(SkillBoost::from_percent(Some(-5)), SkillBoost::NONE);
        assert_eq!(SkillBoost::from_percent(Some(30)), SkillBoost::new(30));
        assert_eq!(SkillBoost::from_percent(Some(150)), SkillBoost::HUNDRED);
        assert_eq!(SkillBoost::new(200), SkillBoost::HUNDRED);
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
            overkill_share: 0.5,
        };
        let shares = [
            ("Longblades".to_string(), 0.75),
            ("Anatomy".to_string(), 0.25),
        ];
        let rates = kill.rates(&shares, HuntingShare::FULL, None);
        // 0.5 PEC of skill per 2 PEC cycled: 0.25 PES per PED, of which the
        // weapon professions' share is split.
        let expected = [("Longblades", 0.75), ("Anatomy", 0.25)];
        assert_eq!(rates.skills.len(), expected.len());
        for ((name, rate), (want, share)) in rates.skills.iter().zip(expected) {
            assert_eq!(name, want);
            assert!((rate - 0.25 * WEAPON_PROFESSION_SHARE * share).abs() < 1e-12);
        }
        assert!((rates.loss_per_ped - 0.08).abs() < 1e-12);
        assert!((rates.kills_per_ped - 50.0).abs() < 1e-12);
        // 50 kills of 2 seconds per PED.
        assert!((rates.hours_per_ped.unwrap() - 100.0 / 3600.0).abs() < 1e-12);
        assert_eq!(rates.markup_lift, None);

        // Hunting half the clock doubles the time per PED and nothing else;
        // a markup carries through as its lift.
        let planned = kill.rates(
            &shares,
            HuntingShare::from_percent(50.0).unwrap(),
            LootMarkup::from_percent(104.0),
        );
        assert!((planned.hours_per_ped.unwrap() - 200.0 / 3600.0).abs() < 1e-12);
        assert_eq!(planned.loss_per_ped, rates.loss_per_ped);
        assert_eq!(planned.skills, rates.skills);
        assert!((planned.markup_lift.unwrap() - 0.04).abs() < 1e-12);
    }

    #[test]
    fn a_markup_starts_at_tt_and_a_hunting_share_is_a_part_of_the_clock() {
        assert_eq!(
            LootMarkup::from_percent(100.0).map(LootMarkup::lift),
            Some(0.0)
        );
        assert_eq!(
            LootMarkup::from_percent(1000.0).map(LootMarkup::percent),
            Some(1000.0)
        );
        for wild in [99.9, 1000.1, f64::NAN, f64::INFINITY] {
            assert_eq!(LootMarkup::from_percent(wild), None, "{wild}");
        }
        assert_eq!(HuntingShare::from_percent(100.0), Some(HuntingShare::FULL));
        assert_eq!(
            HuntingShare::from_percent(1.0).map(HuntingShare::percent),
            Some(1.0)
        );
        for wild in [0.0, 0.5, 100.5, f64::NAN] {
            assert_eq!(HuntingShare::from_percent(wild), None, "{wild}");
        }
    }
}
