//! Expected loot over a session's whole Cycled, not only its modelled slice.
//!
//! Community Model v1 ([`crate::expected_hunting`]) answers for the offensive
//! streams whose Efficiency is known: the weapon, amplifier, and scope phases
//! actually fired, each weighted by the TT it cycled. A session also cycles
//! spend the model has no Efficiency for, and spend that returns nothing:
//!
//! - **Unknown-efficiency TT** (armour and plates, healing, harvesting, the
//!   decay absorbers and implants take over from the weapon, and any weapon
//!   phase without an Efficiency) still feeds the loot pool. Its looter
//!   effect is the one the model knows; its efficiency effect is not known.
//!   So it returns somewhere across the model's whole Efficiency span at the
//!   chosen looter level: from `R(0, L)` to `R(100, L)`. The point estimate
//!   is the session's own modelled offensive recovery (the spend joins the
//!   loot pool of the kills it supported), which always lies in that span.
//! - **Lost spend** returns nothing: the markup paid over TT on limited
//!   items, consumable doses, and enhancer breakage.
//!
//! Expected loot is the TT-weighted sum over every stream, never a mean of
//! percentages. It is a planning estimate: never observed loot, never
//! realised accounting.

use crate::expected_hunting::{
    community_model_v1, evaluate, ExpectedHuntingError, LooterSource, OffensiveLoadoutEvidence,
};

/// What the session cycled, as the loaded expectation reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct CycledStreams {
    /// Every offensive phase fired, flattened: the modelled components and
    /// any phase without an Efficiency (which counts as unknown-efficiency
    /// TT at its raw TT).
    pub offensive: OffensiveLoadoutEvidence,
    /// Loot-bearing TT outside the offensive components: armour and plate
    /// TT, healing TT, harvesting decay, and the decay absorbers and
    /// implants take over from the weapon. Markup paid over it is not here.
    pub unknown_efficiency_tt: f64,
    /// The session's whole Cycled.
    pub cycled: f64,
}

/// Expected loot over the whole Cycled at one looter basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadedExpectation {
    pub looter_source: LooterSource,
    pub looter_level: f64,
    /// Expected loot TT: the modelled streams plus the unknown-efficiency
    /// TT at the session's own offensive recovery.
    pub expected_loot_tt: f64,
    /// The same with the unknown-efficiency TT at the model's 0% Efficiency
    /// return, and at its 100% return.
    pub expected_loot_tt_low: f64,
    pub expected_loot_tt_high: f64,
    /// Expected loot from the modelled offensive streams alone.
    pub modelled_expected_loot_tt: f64,
}

/// The split of a session's Cycled the expectation rests on, the same at
/// every looter basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycledSplit {
    pub cycled: f64,
    /// Modelled offensive spend: raw TT plus the limited premium it consumes.
    pub modelled_cost: f64,
    /// Loot-bearing TT of unknown efficiency.
    pub unknown_efficiency_tt: f64,
    /// Spend that returns nothing: what remains of Cycled.
    pub lost: f64,
}

/// The split of Cycled into modelled, unknown-efficiency, and lost spend.
/// None while nothing modelled has been fired (an expected figure would
/// rest on the band alone) or nothing has been cycled.
pub fn cycled_split(streams: &CycledStreams) -> Result<Option<CycledSplit>, ExpectedHuntingError> {
    let result = evaluate(&streams.offensive)?;
    if result.modelled_raw_tt <= 0.0 || !streams.cycled.is_finite() || streams.cycled <= 0.0 {
        return Ok(None);
    }
    let unmodelled_offensive_tt: f64 = streams
        .offensive
        .components
        .iter()
        .filter(|component| component.efficiency_pct.is_none())
        .map(|component| component.raw_tt_per_use.max(0.0))
        .sum();
    let modelled_cost = result.eligible_offensive_cost.min(streams.cycled);
    // Nothing can be loot-bearing beyond what was cycled: the bound keeps a
    // TT basis estimated from catalogue figures from overrunning the cost
    // actually booked.
    let unknown_efficiency_tt = (streams.unknown_efficiency_tt.max(0.0) + unmodelled_offensive_tt)
        .min((streams.cycled - modelled_cost).max(0.0));
    Ok(Some(CycledSplit {
        cycled: streams.cycled,
        modelled_cost,
        unknown_efficiency_tt,
        lost: (streams.cycled - modelled_cost - unknown_efficiency_tt).max(0.0),
    }))
}

/// Expected loot over the whole Cycled at `looter_source`. None exactly when
/// [`cycled_split`] is.
pub fn loaded_expectation(
    streams: &CycledStreams,
    looter_source: LooterSource,
) -> Result<Option<LoadedExpectation>, ExpectedHuntingError> {
    let Some(split) = cycled_split(streams)? else {
        return Ok(None);
    };
    let evidence = OffensiveLoadoutEvidence {
        looter_source,
        ..streams.offensive.clone()
    };
    let result = evaluate(&evidence)?;
    let looter = evidence.selected_looter_level();
    let low = community_model_v1(0.0, looter)?;
    let high = community_model_v1(100.0, looter)?;
    let point = (result.expected_loot_tt / result.modelled_raw_tt).clamp(low, high);
    let modelled = result.expected_loot_tt;
    let unknown = split.unknown_efficiency_tt;
    Ok(Some(LoadedExpectation {
        looter_source,
        looter_level: looter,
        expected_loot_tt: modelled + unknown * point,
        expected_loot_tt_low: modelled + unknown * low,
        expected_loot_tt_high: modelled + unknown * high,
        modelled_expected_loot_tt: modelled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expected_hunting::{
        HuntingLooterLevels, OffensiveComponentEvidence, OffensiveComponentKind,
    };
    use proptest::prelude::*;

    fn weapon(efficiency: Option<f64>, raw: f64, premium: f64) -> OffensiveComponentEvidence {
        OffensiveComponentEvidence {
            kind: OffensiveComponentKind::Weapon,
            catalog_id: None,
            name: "Weapon".into(),
            efficiency_pct: efficiency,
            raw_tt_per_use: raw,
            consumed_premium_per_use: premium,
        }
    }

    fn streams(
        components: Vec<OffensiveComponentEvidence>,
        unknown: f64,
        cycled: f64,
    ) -> CycledStreams {
        CycledStreams {
            offensive: OffensiveLoadoutEvidence {
                components,
                looters: HuntingLooterLevels {
                    animal: 37.0,
                    mutant: 37.0,
                    robot: 37.0,
                },
                looter_source: LooterSource::ThreeLooterMean,
                transferred_tt_per_use: 0.0,
            },
            unknown_efficiency_tt: unknown,
            cycled,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The worked case: 100 PED of weapon (98 TT at 55% Efficiency, 2 of
    /// limited premium), 8 of armour (6 TT, 2 premium), 4 of heals, at a
    /// looter level of 37. The premium returns nothing; the 10 PED of
    /// armour and heal TT returns across the Efficiency span.
    #[test]
    fn the_worked_case_returns_the_band_it_was_designed_with() {
        let input = streams(vec![weapon(Some(55.0), 98.0, 2.0)], 10.0, 112.0);
        let split = cycled_split(&input).unwrap().unwrap();
        assert!(close(split.modelled_cost, 100.0));
        assert!(close(split.unknown_efficiency_tt, 10.0));
        assert!(close(split.lost, 2.0));

        let at = loaded_expectation(&input, LooterSource::ThreeLooterMean)
            .unwrap()
            .unwrap();
        let r = |e: f64| 0.86 + 0.07 * e / 100.0 + 0.07 * 37.0 / 100.0;
        assert!(close(at.modelled_expected_loot_tt, 98.0 * r(55.0)));
        assert!(close(at.expected_loot_tt, 98.0 * r(55.0) + 10.0 * r(55.0)));
        assert!(close(
            at.expected_loot_tt_low,
            98.0 * r(55.0) + 10.0 * r(0.0)
        ));
        assert!(close(
            at.expected_loot_tt_high,
            98.0 * r(55.0) + 10.0 * r(100.0)
        ));
    }

    /// Two weapons at different Efficiencies count by the TT each cycled,
    /// not by an average of their rates.
    #[test]
    fn several_weapons_weigh_by_the_tt_each_cycled() {
        let input = streams(
            vec![weapon(Some(80.0), 30.0, 0.0), weapon(Some(40.0), 10.0, 0.0)],
            0.0,
            40.0,
        );
        let at = loaded_expectation(&input, LooterSource::ThreeLooterMean)
            .unwrap()
            .unwrap();
        let r = |e: f64| 0.86 + 0.07 * e / 100.0 + 0.07 * 37.0 / 100.0;
        assert!(close(at.expected_loot_tt, 30.0 * r(80.0) + 10.0 * r(40.0)));
        assert!(close(at.expected_loot_tt, at.expected_loot_tt_low));
    }

    /// A weapon phase without an Efficiency is unknown-efficiency TT, not a
    /// stream returning nothing.
    #[test]
    fn a_weapon_without_efficiency_joins_the_band() {
        let input = streams(
            vec![weapon(Some(60.0), 20.0, 0.0), weapon(None, 5.0, 0.0)],
            0.0,
            25.0,
        );
        let split = cycled_split(&input).unwrap().unwrap();
        assert!(close(split.unknown_efficiency_tt, 5.0));
        assert!(close(split.lost, 0.0));
    }

    #[test]
    fn nothing_modelled_gives_no_figure() {
        let input = streams(vec![weapon(None, 5.0, 0.0)], 3.0, 8.0);
        assert_eq!(cycled_split(&input).unwrap(), None);
        assert_eq!(
            loaded_expectation(&input, LooterSource::Animal).unwrap(),
            None
        );
        let idle = streams(vec![weapon(Some(60.0), 0.0, 0.0)], 0.0, 0.0);
        assert_eq!(cycled_split(&idle).unwrap(), None);
    }

    /// The looter basis moves both the modelled return and the band.
    #[test]
    fn the_looter_basis_reads_its_own_level() {
        let mut input = streams(vec![weapon(Some(60.0), 20.0, 0.0)], 5.0, 25.0);
        input.offensive.looters = HuntingLooterLevels {
            animal: 80.0,
            mutant: 10.0,
            robot: 30.0,
        };
        let animal = loaded_expectation(&input, LooterSource::Animal)
            .unwrap()
            .unwrap();
        let mutant = loaded_expectation(&input, LooterSource::Mutant)
            .unwrap()
            .unwrap();
        assert!(close(animal.looter_level, 80.0));
        assert!(animal.expected_loot_tt > mutant.expected_loot_tt);
        assert!(animal.expected_loot_tt_low > mutant.expected_loot_tt_low);
    }

    proptest! {
        /// However the spend divides, the point lies in the band, the band
        /// is exactly as wide as the unknown TT times the model's span, the
        /// split sums to Cycled, and lost spend never returns anything.
        #[test]
        fn the_point_lies_in_the_band_and_the_split_sums_to_cycled(
            efficiency in 0.0f64..100.0,
            raw in 0.01f64..500.0,
            premium in 0.0f64..50.0,
            unknown in 0.0f64..200.0,
            lost in 0.0f64..100.0,
            looter in 0.0f64..100.0,
        ) {
            let cycled = raw + premium + unknown + lost;
            let mut input = streams(vec![weapon(Some(efficiency), raw, premium)], unknown, cycled);
            input.offensive.looters = HuntingLooterLevels { animal: looter, mutant: looter, robot: looter };
            let split = cycled_split(&input).unwrap().unwrap();
            prop_assert!((split.modelled_cost + split.unknown_efficiency_tt + split.lost - cycled).abs() < 1e-6);
            let at = loaded_expectation(&input, LooterSource::Animal).unwrap().unwrap();
            prop_assert!(at.expected_loot_tt_low <= at.expected_loot_tt + 1e-9);
            prop_assert!(at.expected_loot_tt <= at.expected_loot_tt_high + 1e-9);
            let span = 0.07 * split.unknown_efficiency_tt;
            prop_assert!((at.expected_loot_tt_high - at.expected_loot_tt_low - span).abs() < 1e-6);
            // Lost spend and premium add no loot: more of either leaves the
            // expected loot exactly where it was.
            let mut more_lost = input.clone();
            more_lost.cycled += 10.0;
            let more = loaded_expectation(&more_lost, LooterSource::Animal).unwrap().unwrap();
            prop_assert!((more.expected_loot_tt - at.expected_loot_tt).abs() < 1e-9);
        }
    }
}
