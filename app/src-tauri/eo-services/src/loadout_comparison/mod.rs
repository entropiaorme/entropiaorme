//! Loadout comparison: what an offensive setup costs per kill at every
//! mob HP, and where one setup becomes cheaper than another.
//!
//! The comparison is a planning estimate under Community Model v1 and the
//! assumption set in [`ComparisonAssumptions`]; it never touches realised
//! accounting. Per setup it reuses the cost engine for the cost of one use
//! and its streams, and [`crate::expected_hunting`] for the expected loot
//! of the loot-bearing streams and Effective Efficiency, then adds what
//! neither carries: damage per PEC, loss per HP removed, damage per
//! second, and, through the deterministic renewal curve in
//! [`KillCurve`], the expected uses, loss, PEC cycled, and firing time per
//! kill at each HP of the curve. From those follow the crossovers between
//! setups and the cheapest setup at each HP.
//!
//! Pure arithmetic over catalogue-shaped props: no clock, no database, no
//! configuration. The facade builds the props from the catalogue, supplies
//! the player's looter levels, and shapes the outcome for the wire.

mod kill_curve;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::cost_engine::{cost_per_shot_from_props, weapon_total_damage};
use crate::expected_hunting::{
    self, ExpectedHuntingResult, HuntingLooterLevels, COMMUNITY_MODEL_V1,
};

pub use kill_curve::KillCurve;

/// The return model every comparison figure is estimated under.
pub const RETURN_MODEL: &str = COMMUNITY_MODEL_V1;

/// The modelling assumptions behind every comparison figure, in one place
/// so the interface can disclose them beside the numbers they produce.
///
/// Beyond the fields below, two assumptions are fixed by the return model
/// and stated here so they are never implicit: expected loot follows
/// Community Model v1 ([`RETURN_MODEL`]) over the weapon and amplifier
/// streams at the player's three-looter mean; and the decay an absorber
/// or implant takes from the weapon is a cost with no modelled return,
/// because those components have no grounded return treatment.
/// Every figure also assumes the player has maxed the weapon: below its
/// maximum a weapon hits less often and rolls lower, which this model does
/// not represent: the comparison answers which setup is better once maxed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComparisonAssumptions {
    /// The share of uses that land on the target: about 90% for a maxed
    /// weapon by community measurement.
    pub hit_rate: f64,
    /// The lowest damage roll as a share of max damage at full damage
    /// skill; rolls are uniform from there up to max damage.
    pub damage_floor: f64,
    /// The chance that a landed hit is critical. Community sources put the
    /// base chance at about 1 to 2%; effects that raise it are not read.
    pub critical_chance: f64,
    /// A critical hit multiplies the rolled damage by this.
    pub critical_multiplier: f64,
}

impl ComparisonAssumptions {
    /// The assumption set the comparison runs under.
    pub const STANDARD: Self = Self {
        hit_rate: 0.9,
        damage_floor: 0.5,
        critical_chance: 0.02,
        critical_multiplier: 2.0,
    };

    /// Whether the set defines a usable damage model: a hit can land, every
    /// roll removes some HP, and a critical never reduces damage.
    pub fn is_well_formed(&self) -> bool {
        let finite = [
            self.hit_rate,
            self.damage_floor,
            self.critical_chance,
            self.critical_multiplier,
        ]
        .iter()
        .all(|value| value.is_finite());
        finite
            && self.hit_rate > 0.0
            && self.hit_rate <= 1.0
            && self.damage_floor > 0.0
            && self.damage_floor < 1.0
            && (0.0..=1.0).contains(&self.critical_chance)
            && self.critical_multiplier >= 1.0
    }

    /// The expected damage of one use per point of max damage: hit rate
    /// times the mean roll, criticals included.
    pub fn damage_per_use_factor(&self) -> f64 {
        self.hit_rate * (1.0 + self.damage_floor) / 2.0
            * (1.0 + self.critical_chance * (self.critical_multiplier - 1.0))
    }
}

/// Why a setup cannot be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmodelled {
    /// The props carry no weapon.
    NoWeapon,
    /// The catalogue publishes no damage for the weapon.
    NoDamage,
    /// A use costs nothing, so no per-PEC figure exists.
    NoCost,
    /// A loot-bearing stream has no catalogue Efficiency, so its expected
    /// return cannot be modelled.
    MissingEfficiency,
}

/// The per-use economics of one setup. Every cost and loot figure is in
/// PEC per use; damage is per use (one attack).
#[derive(Debug, Clone, PartialEq)]
pub struct SetupEconomics {
    /// Everything one use costs: decay at each component's markup, and
    /// ammunition.
    pub cost_per_use_pec: f64,
    /// The weapon and amplifier TT the return model applies to.
    pub loot_bearing_pec: f64,
    /// Limited-item premium the weapon and amplifier consume per use.
    pub consumed_premium_pec: f64,
    /// Cost with no modelled return: the absorber's and implant's decay at
    /// their markups, and any scope.
    pub outside_model_pec: f64,
    pub expected_loot_pec: f64,
    /// Cost minus expected loot.
    pub loss_per_use_pec: f64,
    /// The weapon and amplifier streams under Community Model v1:
    /// Effective Efficiency and the looter basis live here.
    pub expected_return: ExpectedHuntingResult,
    /// Max damage per use: the weapon's, plus the amplifier's up to half
    /// the weapon's, at the attack-rate factor in force.
    pub max_damage: f64,
    /// Amplifier damage above that cap: paid for, never dealt.
    pub amp_damage_wasted: f64,
    /// Expected damage per use: max damage times hit rate, mean roll, and
    /// the critical term.
    pub expected_damage_per_use: f64,
    pub damage_per_pec: f64,
    /// Expected loss per HP removed: loss per use over expected damage per
    /// use. The per-HP ranking metric.
    pub loss_per_hp_pec: f64,
    /// Uses a minute the server runs, when the catalogue gives a rate.
    pub uses_per_minute: Option<f64>,
    pub damage_per_second: Option<f64>,
}

/// Per-use economics of one setup from its props: a weapon payload in the
/// stored library shape, already enriched with current catalogue Efficiency and
/// the attack-rate factor in force. `uses_per_minute` is the effective
/// rate the server runs the weapon at, when known.
pub fn setup_economics(
    props: &Value,
    looters: HuntingLooterLevels,
    uses_per_minute: Option<f64>,
    assumptions: &ComparisonAssumptions,
) -> Result<SetupEconomics, Unmodelled> {
    let weapon = props
        .get("weapon_entity")
        .filter(|entity| entity.is_object())
        .ok_or(Unmodelled::NoWeapon)?;
    let enhancers = (props
        .get("damage_enhancers")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64)
        .max(0);
    let amp = props.get("amp_entity").filter(|entity| !entity.is_null());
    let factor = crate::attack_rate::factor_from_props(props);

    let base_damage = weapon_total_damage(weapon, None, enhancers).ok_or(Unmodelled::NoDamage)?;
    let with_amp = weapon_total_damage(weapon, amp, enhancers).ok_or(Unmodelled::NoDamage)?;
    let amp_published = amp
        .and_then(|amp| weapon_total_damage(amp, None, 0))
        .unwrap_or(0.0);
    let amp_damage_wasted = (amp_published - (with_amp - base_damage)).max(0.0) * factor;
    let max_damage = with_amp * factor;

    let cost = cost_per_shot_from_props(props, None);
    let cost_per_use_pec = cost["totalCostPerUse"].as_f64().unwrap_or(0.0);
    if cost_per_use_pec.is_nan() || cost_per_use_pec <= 0.0 {
        return Err(Unmodelled::NoCost);
    }

    let evidence = expected_hunting::evidence_from_equipment_props(props, None, looters);
    let expected_return =
        expected_hunting::evaluate(&evidence).map_err(|_| Unmodelled::MissingEfficiency)?;
    if expected_return.incomplete || expected_return.modelled_component_count == 0 {
        return Err(Unmodelled::MissingEfficiency);
    }
    let loot_bearing_pec = expected_return.modelled_raw_tt * 100.0;
    let consumed_premium_pec =
        (expected_return.eligible_offensive_cost - expected_return.modelled_raw_tt) * 100.0;
    let expected_loot_pec = expected_return.expected_loot_tt * 100.0;
    let outside_model_pec = (cost_per_use_pec - loot_bearing_pec - consumed_premium_pec).max(0.0);
    let loss_per_use_pec = cost_per_use_pec - expected_loot_pec;

    let expected_damage_per_use = max_damage * assumptions.damage_per_use_factor();
    let uses_per_minute = uses_per_minute.filter(|rate| rate.is_finite() && *rate > 0.0);
    Ok(SetupEconomics {
        cost_per_use_pec,
        loot_bearing_pec,
        consumed_premium_pec,
        outside_model_pec,
        expected_loot_pec,
        loss_per_use_pec,
        expected_return,
        max_damage,
        amp_damage_wasted,
        expected_damage_per_use,
        damage_per_pec: expected_damage_per_use / cost_per_use_pec,
        loss_per_hp_pec: loss_per_use_pec / expected_damage_per_use,
        uses_per_minute,
        damage_per_second: uses_per_minute.map(|rate| expected_damage_per_use * rate / 60.0),
    })
}

/// The highest mob HP the comparison curve reaches.
pub const CURVE_HP_MAX: f64 = 1_000.0;

/// HP between the curve's sampled points, from 1 HP up: fine enough that
/// a zoomed chart still shows where a smallest hit starts to one-shot.
pub const CURVE_HP_STEP: f64 = 1.0;

/// Points the crossover and winner scan walks across the curve.
const SCAN_POINTS: usize = 2_000;

/// What one setup costs to kill a mob at one HP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KillFigures {
    pub hp: f64,
    pub uses: f64,
    pub loss_pec: f64,
    pub cycled_pec: f64,
    /// Firing time, when the setup's attack rate is known.
    pub seconds: Option<f64>,
}

/// A stretch of the curve where one setup is the cheapest per kill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinnerSegment {
    pub setup: usize,
    pub from_hp: f64,
    pub to_hp: f64,
}

/// Where two setups' loss-per-kill curves cross.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossover {
    pub hp: f64,
    pub cheaper_below: usize,
    pub cheaper_above: usize,
}

/// Several setups compared across the HP curve. Setup indices refer to
/// the slice [`compare`] was given.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonOutcome {
    /// Per setup, its kill figures at each sampled HP.
    pub curves: Vec<Vec<KillFigures>>,
    /// The cheapest setup per kill across the curve, lowest HP first.
    pub winners: Vec<WinnerSegment>,
    /// Every crossing of two setups' loss curves, lowest HP first.
    pub crossovers: Vec<Crossover>,
}

/// The HP points the curve is sampled at: 1 HP, then every
/// [`CURVE_HP_STEP`] up to [`CURVE_HP_MAX`].
pub fn curve_points() -> Vec<f64> {
    let steps = (CURVE_HP_MAX / CURVE_HP_STEP).round() as usize;
    std::iter::once(1.0)
        .chain(
            (1..=steps)
                .map(|index| index as f64 * CURVE_HP_STEP)
                .filter(|hp| *hp > 1.0),
        )
        .collect()
}

/// What `setup` costs to kill a mob of `hp` HP on its solved curve.
pub fn kill_figures(setup: &SetupEconomics, curve: &KillCurve, hp: f64) -> KillFigures {
    let uses = curve.expected_uses(hp);
    KillFigures {
        hp,
        uses,
        loss_pec: uses * setup.loss_per_use_pec,
        cycled_pec: uses * setup.cost_per_use_pec,
        seconds: setup.uses_per_minute.map(|rate| uses * 60.0 / rate),
    }
}

/// The HP inside `[left, right]` where two loss curves meet, by linear
/// interpolation of their difference (`left_gap` and `right_gap` are the
/// first minus the second at each end).
fn meeting_point(left: f64, right: f64, left_gap: f64, right_gap: f64) -> f64 {
    let span = left_gap - right_gap;
    if span == 0.0 {
        return (left + right) / 2.0;
    }
    left + (right - left) * (left_gap / span).clamp(0.0, 1.0)
}

/// Compare setups per kill from 1 HP to [`CURVE_HP_MAX`]. None when there
/// is nothing to compare or a setup's damage cannot define a kill curve.
pub fn compare(
    setups: &[SetupEconomics],
    assumptions: &ComparisonAssumptions,
) -> Option<ComparisonOutcome> {
    if setups.is_empty() {
        return None;
    }
    let solved: Vec<KillCurve> = setups
        .iter()
        .map(|setup| KillCurve::solve(setup.max_damage, CURVE_HP_MAX, assumptions))
        .collect::<Option<_>>()?;
    let points = curve_points();
    let curves = setups
        .iter()
        .zip(&solved)
        .map(|(setup, curve)| {
            points
                .iter()
                .map(|hp| kill_figures(setup, curve, *hp))
                .collect()
        })
        .collect();

    // The scan: every setup's loss per kill at evenly spaced HP, for the
    // crossovers and the cheapest setup at each HP.
    let scan: Vec<f64> = (0..=SCAN_POINTS)
        .map(|index| 1.0 + (CURVE_HP_MAX - 1.0) * index as f64 / SCAN_POINTS as f64)
        .collect();
    let losses: Vec<Vec<f64>> = setups
        .iter()
        .zip(&solved)
        .map(|(setup, curve)| {
            scan.iter()
                .map(|hp| curve.expected_uses(*hp) * setup.loss_per_use_pec)
                .collect()
        })
        .collect();

    let mut crossovers = Vec::new();
    for first in 0..setups.len() {
        for second in first + 1..setups.len() {
            // Walk sign changes, carrying the last nonzero sign across an
            // exact touch so a tangency is not read as two crossings.
            let mut last: Option<(usize, f64)> = None;
            for index in 0..scan.len() {
                let gap = losses[first][index] - losses[second][index];
                if gap == 0.0 {
                    continue;
                }
                if let Some((from, previous)) = last {
                    if previous.signum() != gap.signum() {
                        let hp = meeting_point(scan[from], scan[index], previous, gap);
                        let (cheaper_below, cheaper_above) = if previous < 0.0 {
                            (first, second)
                        } else {
                            (second, first)
                        };
                        crossovers.push(Crossover {
                            hp,
                            cheaper_below,
                            cheaper_above,
                        });
                    }
                }
                last = Some((index, gap));
            }
        }
    }
    crossovers.sort_by(|a, b| a.hp.total_cmp(&b.hp));

    let cheapest_at = |index: usize| -> usize {
        (0..setups.len())
            .min_by(|a, b| {
                losses[*a][index]
                    .total_cmp(&losses[*b][index])
                    .then(a.cmp(b))
            })
            .unwrap_or(0)
    };
    let mut winners: Vec<WinnerSegment> = Vec::new();
    for index in 0..scan.len() {
        let winner = cheapest_at(index);
        let from_hp = match winners.last_mut() {
            Some(segment) if segment.setup == winner => {
                segment.to_hp = scan[index];
                continue;
            }
            Some(segment) => {
                let hp = meeting_point(
                    scan[index - 1],
                    scan[index],
                    losses[segment.setup][index - 1] - losses[winner][index - 1],
                    losses[segment.setup][index] - losses[winner][index],
                );
                segment.to_hp = hp;
                hp
            }
            None => scan[index],
        };
        winners.push(WinnerSegment {
            setup: winner,
            from_hp,
            to_hp: scan[index],
        });
    }

    Some(ComparisonOutcome {
        curves,
        winners,
        crossovers,
    })
}
