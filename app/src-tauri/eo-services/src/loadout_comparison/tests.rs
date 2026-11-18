//! The comparison engine against hand-worked purchase questions.
//!
//! Each reference figure below was worked by hand (per-kill figures by
//! Monte Carlo averaged over an HP band, lambda without the critical
//! term), so the tolerances are the reference's own precision: Effective
//! Efficiency to 0.15 points, lambda to 3% (after the critical term, which
//! lowers it by 2%), band averages to 4%, and crossovers to 12 HP. Props
//! are built from the stated catalogue stats; the looter level is explicit.

use serde_json::{json, Value};

use super::*;
use crate::expected_hunting::{EffectiveEfficiency, OffensiveComponentKind};

const STANDARD: ComparisonAssumptions = ComparisonAssumptions::STANDARD;
/// The critical term's share of mean damage: 1 + 2% x (2 - 1).
const CRITICAL_TERM: f64 = 1.02;

fn looters(level: f64) -> HuntingLooterLevels {
    HuntingLooterLevels {
        animal: level,
        mutant: level,
        robot: level,
    }
}

fn weapon(name: &str, damage: f64, upm: f64, ammo: f64, decay: f64, efficiency: f64) -> Value {
    json!({
        "name": name,
        "economy": {"decay": decay, "ammo_burn": ammo, "efficiency": efficiency},
        "damage": {"impact": damage},
        "uses_per_minute": upm,
    })
}

fn amp(name: &str, damage: f64, ammo: f64, decay: f64, efficiency: f64) -> Value {
    json!({
        "name": name,
        "economy": {"decay": decay, "ammo_burn": ammo, "efficiency": efficiency},
        "damage": {"impact": damage},
    })
}

fn device(name: &str, absorption: f64) -> Value {
    json!({"name": name, "economy": {"decay": 0.0, "absorption": absorption}})
}

fn p20() -> (Value, f64) {
    (device("ArMatrix Extender P20 (L)", 0.2), 106.0)
}

/// Weapon props in the stored library shape, markups as percentages.
fn props(
    weapon: Value,
    weapon_markup: f64,
    amp: Option<(Value, f64)>,
    absorber: Option<(Value, f64)>,
    implant: Option<(Value, f64)>,
) -> Value {
    let (amp_entity, amp_markup) = amp.unwrap_or((Value::Null, 100.0));
    let (absorber_entity, absorber_markup) = absorber.unwrap_or((Value::Null, 100.0));
    let mut props = json!({
        "weapon_entity": weapon,
        "amp_entity": amp_entity,
        "absorber_entity": absorber_entity,
        "scope_entity": null,
        "weapon_markup": weapon_markup,
        "amp_markup": amp_markup,
        "absorber_markup": absorber_markup,
        "scope_markup": 100,
        "damage_enhancers": 0,
    });
    if let Some((implant, markup)) = implant {
        props["implant_entity"] = implant;
        props["implant_markup"] = json!(markup);
    }
    props
}

fn economics(props: &Value, level: f64) -> SetupEconomics {
    let upm = props["weapon_entity"]["uses_per_minute"].as_f64();
    setup_economics(props, looters(level), upm, &STANDARD).expect("modelled setup")
}

fn effective(economics: &SetupEconomics) -> f64 {
    match economics.expected_return.effective_efficiency {
        Some(EffectiveEfficiency::WithinModelRange { efficiency_pct }) => efficiency_pct,
        other => panic!("effective efficiency out of range: {other:?}"),
    }
}

fn assert_close(label: &str, actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: {actual} is not within {tolerance} of {expected}"
    );
}

fn assert_relative(label: &str, actual: f64, expected: f64, share: f64) {
    assert!(
        ((actual - expected) / expected).abs() <= share,
        "{label}: {actual} is not within {:.1}% of {expected}",
        share * 100.0
    );
}

/// The average kill figures over an HP band at a fixed step, every point
/// weighted equally: how the hand-worked references summarised a band.
fn band_average(setup: &SetupEconomics, from: f64, to: f64, step: f64) -> KillFigures {
    let curve = KillCurve::solve(setup.max_damage, to, &STANDARD).unwrap();
    let points: Vec<KillFigures> = (0..)
        .map(|index| from + index as f64 * step)
        .take_while(|hp| *hp <= to + 1e-9)
        .map(|hp| kill_figures(setup, &curve, hp))
        .collect();
    let count = points.len() as f64;
    let mean = |pick: fn(&KillFigures) -> f64| points.iter().map(pick).sum::<f64>() / count;
    KillFigures {
        hp: (from + to) / 2.0,
        uses: mean(|point| point.uses),
        loss_pec: mean(|point| point.loss_pec),
        cycled_pec: mean(|point| point.cycled_pec),
        seconds: setup
            .uses_per_minute
            .map(|_| mean(|point| point.seconds.unwrap_or(0.0))),
        overkill_share: mean(|point| point.overkill_share),
    }
}

fn bc30(markup: f64) -> Value {
    props(
        weapon(
            "Exceptional A&P Series Mayhem BC-30, Modified (L)",
            38.0,
            67.0,
            804.0,
            0.002,
            80.5,
        ),
        markup,
        None,
        Some(p20()),
        None,
    )
}

fn gauss_weapon() -> Value {
    weapon(
        "Exceptional Kriegerum Mayhem G.2 Gauss Rifle, Perfected (L)",
        80.8,
        40.0,
        1636.0,
        0.01,
        91.6,
    )
}

fn flickerblade(markup: f64) -> Value {
    props(
        weapon(
            "Flickerblade Replica, Exceptional (L)",
            104.0,
            45.0,
            1850.0,
            0.02,
            88.0,
        ),
        markup,
        None,
        Some(p20()),
        None,
    )
}

fn scimitar() -> Value {
    props(
        weapon(
            "Scimitar of the Mages, TWEN Edition",
            156.0,
            42.0,
            2950.0,
            2.336,
            88.0,
        ),
        100.0,
        None,
        None,
        None,
    )
}

#[test]
fn case_1_the_bc30_and_gauss_tie_across_50_to_150_hp_and_cross_near_100() {
    let setups = vec![
        economics(&bc30(1800.0), 50.0),
        economics(
            &props(gauss_weapon(), 1500.0, None, Some(p20()), None),
            50.0,
        ),
    ];
    let bc30 = band_average(&setups[0], 50.0, 150.0, 5.0);
    let gauss = band_average(&setups[1], 50.0, 150.0, 5.0);
    assert_relative("BC-30 loss per kill", bc30.loss_pec, 1.84, 0.04);
    assert_relative("BC-30 PEC cycled per kill", bc30.cycled_pec, 35.5, 0.04);
    assert_relative("BC-30 seconds per kill", bc30.seconds.unwrap(), 3.94, 0.04);
    assert_relative("Gauss loss per kill", gauss.loss_pec, 1.87, 0.04);
    assert_relative("Gauss PEC cycled per kill", gauss.cycled_pec, 39.4, 0.04);
    assert_relative("Gauss seconds per kill", gauss.seconds.unwrap(), 3.58, 0.04);

    // Inside the band they cross once: BC-30 cheaper below, Gauss above.
    let outcome = compare(&setups, &STANDARD).unwrap();
    let in_band: Vec<&Crossover> = outcome
        .crossovers
        .iter()
        .filter(|crossover| (50.0..=150.0).contains(&crossover.hp))
        .collect();
    assert_eq!(in_band.len(), 1, "{:?}", outcome.crossovers);
    let crossover = *in_band[0];
    assert_close("crossover HP", crossover.hp, 99.0, 12.0);
    assert_eq!((crossover.cheaper_below, crossover.cheaper_above), (0, 1));
    // Below the band the discrete shot count shows: from about 36 to 44 HP
    // the Gauss's smallest hit already kills while the BC-30 needs two.
    assert_eq!(outcome.crossovers.len(), 3, "{:?}", outcome.crossovers);
    assert_close("one-shot window opens", outcome.crossovers[0].hp, 36.0, 2.0);
    assert_close(
        "one-shot window closes",
        outcome.crossovers[1].hp,
        44.0,
        2.0,
    );
    let winners: Vec<usize> = outcome
        .winners
        .iter()
        .map(|segment| segment.setup)
        .collect();
    assert_eq!(winners, vec![0, 1, 0, 1]);
    assert_eq!(outcome.winners[0].from_hp, 1.0);
    assert_close("handover", outcome.winners[2].to_hp, crossover.hp, 1e-9);
    assert_eq!(outcome.winners[3].to_hp, CURVE_HP_MAX);
    // The sampled curve agrees with the band arithmetic at a sampled HP.
    let at_100 = outcome.curves[0]
        .iter()
        .find(|point| point.hp == 100.0)
        .unwrap();
    let curve = KillCurve::solve(setups[0].max_damage, 100.0, &STANDARD).unwrap();
    assert_close(
        "curve at 100 HP",
        at_100.loss_pec,
        kill_figures(&setups[0], &curve, 100.0).loss_pec,
        1e-9,
    );
}

#[test]
fn case_2_negotiating_markup_matters_by_decay() {
    assert_close(
        "BC-30 at 1800%",
        effective(&economics(&bc30(1800.0), 50.0)),
        75.9,
        0.15,
    );
    assert_close(
        "BC-30 at 1600%",
        effective(&economics(&bc30(1600.0), 50.0)),
        76.5,
        0.15,
    );
    let drop = effective(&economics(&flickerblade(1100.0), 50.0))
        - effective(&economics(&flickerblade(1200.0), 50.0));
    assert_close(
        "Flickerblade drop per 100 points of markup",
        drop,
        1.16,
        0.15,
    );
}

#[test]
fn case_3_cheap_to_buy_is_not_cheap_to_run() {
    let pp40 = props(
        weapon("Hyperion Aurora PP-40 (L)", 57.0, 67.0, 1264.0, 0.527, 74.0),
        125.0,
        None,
        None,
        None,
    );
    let gauss = props(gauss_weapon(), 1500.0, None, Some(p20()), None);
    assert_close(
        "PP-40 at 125%",
        effective(&economics(&pp40, 50.0)),
        60.6,
        0.15,
    );
    assert_close(
        "Gauss at 1500%",
        effective(&economics(&gauss, 50.0)),
        82.3,
        0.15,
    );
}

#[test]
fn case_4_amplifiers_as_streams() {
    let omegaton = amp("Omegaton A106", 18.0, 100.0, 2.902, 83.6);
    let mut bc30_amped = bc30(1800.0);
    bc30_amped["amp_entity"] = omegaton;
    // An unlimited amp runs at TT whatever it was bought for.
    bc30_amped["amp_markup"] = json!(100);
    assert_close(
        "BC-30 + A106",
        effective(&economics(&bc30_amped, 50.0)),
        78.4,
        0.15,
    );

    let gauss_37b = props(
        gauss_weapon(),
        1500.0,
        Some((
            amp("ArMatrix L-Amplifier 37B (L)", 37.0, 777.0, 0.592, 85.0),
            125.0,
        )),
        Some(p20()),
        None,
    );
    assert_close(
        "Gauss + 37B",
        effective(&economics(&gauss_37b, 50.0)),
        75.1,
        0.15,
    );
    let gauss_alpha = props(
        gauss_weapon(),
        1500.0,
        Some((
            amp("Mayhem L-Amplifier Alpha (L)", 39.0, 898.0, 0.019, 75.0),
            800.0,
        )),
        Some(p20()),
        None,
    );
    assert_close(
        "Gauss + Alpha",
        effective(&economics(&gauss_alpha, 50.0)),
        72.7,
        0.15,
    );
}

fn mayhem_chip(amplifier: (Value, f64)) -> Value {
    let chip = json!({
        "name": "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)",
        "economy": {"decay": 0.011, "ammo_burn": 2750.0, "efficiency": 83.3},
        "damage": {"impact": 28.3, "electric": 106.1},
        "uses_per_minute": 51.0,
    });
    props(
        chip,
        1500.0,
        Some(amplifier),
        Some(p20()),
        Some((device("NeoPsion 85-B Mindforce Implant (L)", 0.2), 110.0)),
    )
}

fn amp_only_effective(props: &Value) -> f64 {
    let mut evidence =
        crate::expected_hunting::evidence_from_equipment_props(props, None, looters(50.0));
    evidence
        .components
        .retain(|component| component.kind == OffensiveComponentKind::Amplifier);
    match crate::expected_hunting::evaluate(&evidence)
        .unwrap()
        .effective_efficiency
    {
        Some(EffectiveEfficiency::WithinModelRange { efficiency_pct }) => efficiency_pct,
        other => panic!("{other:?}"),
    }
}

#[test]
fn case_5_lambda_outranks_effective_efficiency() {
    let delta = mayhem_chip((
        amp("Mayhem MF-Amplifier Delta (L)", 57.0, 898.0, 0.019, 75.0),
        370.0,
    ));
    let kinetic = mayhem_chip((
        amp("Mayhem Kinetic Amplifier 1", 55.0, 1080.0, 1.2, 77.0),
        100.0,
    ));
    let delta_economics = economics(&delta, 50.0);
    let kinetic_economics = economics(&kinetic, 50.0);
    assert_relative(
        "Delta setup lambda",
        delta_economics.loss_per_hp_pec,
        0.0148 / CRITICAL_TERM,
        0.03,
    );
    assert_relative(
        "Kinetic setup lambda",
        kinetic_economics.loss_per_hp_pec,
        0.0156 / CRITICAL_TERM,
        0.03,
    );

    // The Kinetic amp alone is the more efficient stream...
    assert_close(
        "Kinetic amp alone",
        amp_only_effective(&kinetic),
        77.0,
        0.15,
    );
    assert_close("Delta amp alone", amp_only_effective(&delta), 67.3, 0.15);
    // ...yet the Delta setup removes every HP more cheaply, so it ranks
    // cheaper per kill at every HP of the curve.
    assert!(delta_economics.loss_per_hp_pec < kinetic_economics.loss_per_hp_pec);
    let outcome = compare(&[kinetic_economics, delta_economics], &STANDARD).unwrap();
    assert!(outcome.crossovers.is_empty());
    assert_eq!(outcome.winners.len(), 1, "the Kinetic setup never wins");
    assert_eq!(outcome.winners[0].setup, 1);
}

#[test]
fn case_6_lambda_and_damage_per_second_for_the_sword_ladder() {
    let level = 41.5;
    let vortex = economics(
        &props(
            weapon(
                "Vortex Cutter Replica, Exceptional (L)",
                69.0,
                34.0,
                1219.0,
                0.008,
                87.0,
            ),
            1250.0,
            None,
            Some(p20()),
            None,
        ),
        level,
    );
    let flicker = economics(&flickerblade(1200.0), level);
    let scimitar = economics(&scimitar(), level);
    let saif = economics(
        &props(
            weapon("CUHOF Saif", 104.0, 30.0, 1944.0, 1.577, 75.0),
            100.0,
            None,
            Some(p20()),
            None,
        ),
        level,
    );
    // The reference left out the critical term: lambda here is 2% lower and
    // damage per second 2% higher than its figures.
    for (label, setup, lambda, dps) in [
        ("Vortex Cutter", &vortex, 0.0147, 26.4),
        ("Flickerblade", &flicker, 0.0156, 52.6),
        ("Scimitar", &scimitar, 0.0149, 73.7),
        ("CUHOF Saif", &saif, 0.0220, 35.1),
    ] {
        assert_relative(label, setup.loss_per_hp_pec, lambda / CRITICAL_TERM, 0.03);
        assert_relative(
            label,
            setup.damage_per_second.unwrap(),
            dps * CRITICAL_TERM,
            0.01,
        );
    }
}

#[test]
fn the_streams_add_up_to_the_cost_of_one_use() {
    let chip = mayhem_chip((
        amp("Mayhem MF-Amplifier Delta (L)", 57.0, 898.0, 0.019, 75.0),
        370.0,
    ));
    let setup = economics(&chip, 50.0);
    assert_close(
        "streams",
        setup.loot_bearing_pec + setup.consumed_premium_pec + setup.outside_model_pec,
        setup.cost_per_use_pec,
        1e-6,
    );
    // The implant takes 20% of the chip's decay (0.0022 PEC) at 110%, the
    // absorber 20% of the rest (0.0018 PEC rounded) at 106%.
    assert_close(
        "outside the model",
        setup.outside_model_pec,
        0.0022 * 1.1 + 0.0018 * 1.06,
        1e-3,
    );
    assert_close(
        "loss",
        setup.loss_per_use_pec,
        setup.cost_per_use_pec - setup.expected_loot_pec,
        1e-12,
    );
    assert_close(
        "damage per PEC",
        setup.damage_per_pec,
        setup.expected_damage_per_use / setup.cost_per_use_pec,
        1e-12,
    );
}

#[test]
fn the_loss_splits_into_premium_shortfall_outside_and_house_edge() {
    let mut bc30_a106 = bc30(1800.0);
    bc30_a106["amp_entity"] = amp("Omegaton A106", 18.0, 100.0, 2.902, 83.6);
    let setups = [
        ("BC-30 at 1800% with a P20", economics(&bc30(1800.0), 50.0)),
        ("Scimitar, unlimited", economics(&scimitar(), 50.0)),
        ("BC-30 with an unlimited amp", economics(&bc30_a106, 50.0)),
        (
            "Gauss + 37B with a P20",
            economics(
                &props(
                    gauss_weapon(),
                    1500.0,
                    Some((
                        amp("ArMatrix L-Amplifier 37B (L)", 37.0, 777.0, 0.592, 85.0),
                        125.0,
                    )),
                    Some(p20()),
                    None,
                ),
                41.5,
            ),
        ),
        (
            "chip with an implant",
            economics(
                &mayhem_chip((
                    amp("Mayhem MF-Amplifier Delta (L)", 57.0, 898.0, 0.019, 75.0),
                    370.0,
                )),
                50.0,
            ),
        ),
    ];
    for (label, setup) in &setups {
        assert_close(
            label,
            setup.consumed_premium_pec
                + setup.efficiency_shortfall_pec
                + setup.outside_model_pec
                + setup.house_edge_pec,
            setup.loss_per_use_pec,
            1e-9,
        );
        assert!(setup.efficiency_shortfall_pec > 0.0, "{label}");
        assert!(setup.house_edge_pec > 0.0, "{label}");
    }
    // An unlimited setup with nothing outside the model loses only to its
    // Efficiency and the house edge.
    let scimitar = &setups[1].1;
    assert_eq!(scimitar.consumed_premium_pec, 0.0);
    assert_eq!(scimitar.outside_model_pec, 0.0);
    // A limited weapon at markup pays its premium; its absorber sits
    // outside the model.
    let limited = &setups[0].1;
    assert!(limited.consumed_premium_pec > 0.0);
    assert!(limited.outside_model_pec > 0.0);
}

#[test]
fn the_efficiency_shortfall_ignores_the_looter_level_and_the_house_edge_ends_at_100() {
    let at = |level: f64| economics(&bc30(1800.0), level);
    let base = at(0.0);
    let mut previous_edge = f64::INFINITY;
    for level in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let setup = at(level);
        // Within the model's rounding of expected loot to 1e-8 PED.
        assert_close(
            &format!("shortfall at {level}"),
            setup.efficiency_shortfall_pec,
            base.efficiency_shortfall_pec,
            1e-5,
        );
        // Community Model v1 returns 86% + 7% x Efficiency + 7% x looter:
        // a 100% Efficiency item still keeps 7% x (1 - looter / 100) back.
        assert_close(
            &format!("house edge at {level}"),
            setup.house_edge_pec,
            setup.loot_bearing_pec * 0.07 * (1.0 - level / 100.0),
            1e-9,
        );
        assert!(setup.house_edge_pec < previous_edge, "falls with the level");
        previous_edge = setup.house_edge_pec;
    }
    assert_close("house edge at 100", at(100.0).house_edge_pec, 0.0, 1e-12);
    // The model stops counting looter skill at 100.
    assert_close("house edge past 100", at(150.0).house_edge_pec, 0.0, 1e-12);
}

#[test]
fn part_decay_splits_decay_by_part_before_markup() {
    // The implant takes 20% of the chip's 0.011 PEC decay, the absorber 20%
    // of the rest, and the weapon what is left; the amp decays on its own.
    let chip = mayhem_chip((
        amp("Mayhem MF-Amplifier Delta (L)", 57.0, 898.0, 0.019, 75.0),
        370.0,
    ));
    let [weapon, amp_decay, absorber, implant] = part_decay_pec(&chip);
    assert_close("implant", implant, 0.0022, 1e-4);
    assert_close("absorber", absorber, 0.0088 * 0.2, 1e-4);
    assert_close("weapon", weapon, 0.0088 * 0.8, 1e-4);
    assert_close("amp", amp_decay, 0.019, 1e-4);
    assert_close("all of it", weapon + absorber + implant, 0.011, 1e-4);

    // Without an amp or implant those parts take nothing; the 1800% markup
    // does not enter the TT decay.
    assert_eq!(part_decay_pec(&bc30(1800.0)), [0.0016, 0.0, 0.0004, 0.0]);
    assert_eq!(part_decay_pec(&scimitar()), [2.336, 0.0, 0.0, 0.0]);
}

#[test]
fn compare_to_reaches_its_hp_max() {
    let setups = vec![
        economics(&bc30(1800.0), 50.0),
        economics(
            &props(gauss_weapon(), 1500.0, None, Some(p20()), None),
            50.0,
        ),
    ];
    let outcome = compare_to(&setups, &STANDARD, 5000.0).unwrap();
    for curve in &outcome.curves {
        assert_eq!(curve.first().unwrap().hp, 1.0);
        assert_eq!(curve.last().unwrap().hp, 5000.0);
    }
    assert_eq!(outcome.winners.first().unwrap().from_hp, 1.0);
    assert_eq!(outcome.winners.last().unwrap().to_hp, 5000.0);
    // The same setups over the default reach agree where the curves overlap.
    let short = compare(&setups, &STANDARD).unwrap();
    let at_1000 = |curve: &[KillFigures]| *curve.iter().find(|point| point.hp == 1000.0).unwrap();
    assert_close(
        "loss at 1,000 HP",
        at_1000(&outcome.curves[0]).loss_pec,
        at_1000(&short.curves[0]).loss_pec,
        1e-9,
    );
    // A reach of 1 HP or less, or none at all, has no curve.
    assert!(compare_to(&setups, &STANDARD, 1.0).is_none());
    assert!(compare_to(&setups, &STANDARD, f64::NAN).is_none());
    assert!(compare_to(&setups, &STANDARD, f64::INFINITY).is_none());
}

#[test]
fn a_longer_curve_widens_its_step_past_3000_points() {
    let up_to_3000 = curve_points_to(3000.0);
    assert_eq!(up_to_3000.len(), 3000);
    assert_eq!(up_to_3000[1] - up_to_3000[0], 1.0);
    assert_eq!(up_to_3000.last(), Some(&3000.0));

    let points = curve_points_to(5000.0);
    assert_eq!(&points[..3], &[1.0, 2.0, 4.0]);
    assert_eq!(points.len(), 2501);
    assert_eq!(points.last(), Some(&5000.0));

    let points = curve_points_to(30_000.0);
    assert_eq!(&points[..3], &[1.0, 10.0, 20.0]);
    assert_eq!(points.last(), Some(&30_000.0));

    for hp_max in [3001.0, 4500.0, 12_345.0, 100_000.0] {
        let points = curve_points_to(hp_max);
        assert_eq!(points[0], 1.0, "{hp_max}");
        assert!(points.len() <= 3001, "{hp_max}: {} points", points.len());
        assert!(
            points.windows(2).all(|pair| pair[1] > pair[0]),
            "{hp_max}: rising"
        );
        // The curve reaches the slider's end even off a whole step, so a
        // pin there still reads figures.
        assert_eq!(points.last(), Some(&hp_max), "{hp_max}: ends on the reach");
    }
    assert_eq!(curve_points_to(50_000.0).last(), Some(&50_000.0));
}

#[test]
fn amplifier_damage_above_half_the_weapon_is_flagged_as_wasted() {
    let mut capped = bc30(1800.0);
    capped["amp_entity"] = amp("ArMatrix L-Amplifier 37B (L)", 37.0, 777.0, 0.592, 85.0);
    let setup = economics(&capped, 50.0);
    assert_close("max damage", setup.max_damage, 38.0 + 19.0, 1e-9);
    assert_close("wasted", setup.amp_damage_wasted, 18.0, 1e-9);

    let gauss_37b = props(
        gauss_weapon(),
        1500.0,
        Some((
            amp("ArMatrix L-Amplifier 37B (L)", 37.0, 777.0, 0.592, 85.0),
            125.0,
        )),
        None,
        None,
    );
    assert_eq!(economics(&gauss_37b, 50.0).amp_damage_wasted, 0.0);
}

#[test]
fn the_attack_rate_factor_scales_damage_and_cost_alike() {
    let base = economics(&bc30(1800.0), 50.0);
    let mut buffed = bc30(1800.0);
    buffed[crate::attack_rate::ATTACK_RATE_FACTOR_KEY] = json!(1.2);
    let buffed = setup_economics(&buffed, looters(50.0), Some(100.0), &STANDARD).unwrap();
    assert_close("max damage", buffed.max_damage, base.max_damage * 1.2, 1e-9);
    assert_relative(
        "cost",
        buffed.cost_per_use_pec,
        base.cost_per_use_pec * 1.2,
        1e-3,
    );
    assert_relative("lambda", buffed.loss_per_hp_pec, base.loss_per_hp_pec, 1e-3);
    assert_close(
        "damage per second",
        buffed.damage_per_second.unwrap(),
        buffed.expected_damage_per_use * 100.0 / 60.0,
        1e-9,
    );
}

#[test]
fn setups_the_model_cannot_price_say_why() {
    assert_eq!(
        setup_economics(&json!({}), looters(50.0), None, &STANDARD),
        Err(Unmodelled::NoWeapon)
    );
    let mut no_damage = bc30(1800.0);
    no_damage["weapon_entity"]["damage"] = json!({});
    assert_eq!(
        setup_economics(&no_damage, looters(50.0), None, &STANDARD),
        Err(Unmodelled::NoDamage)
    );
    let mut no_efficiency = bc30(1800.0);
    no_efficiency["weapon_entity"]["economy"]
        .as_object_mut()
        .unwrap()
        .remove("efficiency");
    assert_eq!(
        setup_economics(&no_efficiency, looters(50.0), None, &STANDARD),
        Err(Unmodelled::MissingEfficiency)
    );
    let free = props(
        weapon("Free", 10.0, 60.0, 0.0, 0.0, 80.0),
        100.0,
        None,
        None,
        None,
    );
    assert_eq!(
        setup_economics(&free, looters(50.0), None, &STANDARD),
        Err(Unmodelled::NoCost)
    );
}

#[test]
fn an_unknown_attack_rate_leaves_time_unknown_but_the_comparison_whole() {
    let gauss = props(gauss_weapon(), 1500.0, None, Some(p20()), None);
    let unknown_rate = setup_economics(&gauss, looters(50.0), None, &STANDARD).unwrap();
    assert_eq!(unknown_rate.damage_per_second, None);
    let outcome = compare(&[economics(&bc30(1800.0), 50.0), unknown_rate], &STANDARD).unwrap();
    assert!(outcome.curves[1]
        .iter()
        .all(|point| point.seconds.is_none()));
    assert!(outcome.curves[0]
        .iter()
        .all(|point| point.seconds.is_some()));
    assert!(!outcome.crossovers.is_empty());
}

#[test]
fn identical_setups_never_cross_and_the_first_holds_the_curve() {
    let setups = vec![
        economics(&bc30(1800.0), 50.0),
        economics(&bc30(1800.0), 50.0),
    ];
    let outcome = compare(&setups, &STANDARD).unwrap();
    assert!(outcome.crossovers.is_empty());
    assert_eq!(outcome.winners.len(), 1);
    assert_eq!(outcome.winners[0].setup, 0);
    assert!(compare(&[], &STANDARD).is_none());
}

#[test]
fn the_curve_runs_from_one_hp_to_a_thousand_in_even_steps() {
    let points = curve_points();
    assert_eq!(points.first(), Some(&1.0));
    assert_eq!(points[1], 2.0);
    assert_eq!(points.last(), Some(&CURVE_HP_MAX));
    assert_eq!(points.len(), 1000);
    let outcome = compare(&[economics(&bc30(1800.0), 50.0)], &STANDARD).unwrap();
    assert_eq!(outcome.curves[0].len(), points.len());
    // A mob of any HP up to one floor hit dies to the first hit that lands.
    assert_close("uses at 1 HP", outcome.curves[0][0].uses, 1.0 / 0.9, 1e-9);
    assert!(outcome.curves[0]
        .windows(2)
        .all(|pair| pair[1].loss_pec >= pair[0].loss_pec));
}

#[test]
fn the_standard_assumptions_are_the_disclosed_ones() {
    assert_eq!(STANDARD.hit_rate, 0.9);
    assert_eq!(STANDARD.damage_floor, 0.5);
    assert_eq!(STANDARD.critical_chance, 0.02);
    assert_eq!(STANDARD.critical_multiplier, 2.0);
    assert!(STANDARD.is_well_formed());
    assert_close(
        "damage factor",
        STANDARD.damage_per_use_factor(),
        0.9 * 0.75 * 1.02,
        1e-12,
    );
    assert_eq!(RETURN_MODEL, "community_v1");
}

/// Overkill is the damage fired past the kill as a share of all of it: on
/// a mob below the smallest hit, every landed hit kills and all but the
/// mob's HP of it is overkill; on a mob of many hits only the last one
/// overshoots, so the share falls away as the mob grows.
#[test]
fn overkill_is_the_share_of_damage_dealt_past_the_kill() {
    let setup = economics(
        &props(
            weapon("Blade", 8.0, 60.0, 0.0, 1.0, 50.0),
            100.0,
            None,
            None,
            None,
        ),
        50.0,
    );
    let curve = KillCurve::solve(setup.max_damage, 1_000.0, &STANDARD).unwrap();
    // A 2 HP mob dies to the first landed hit, which averages 8 x 0.75
    // with criticals: 6.12 damage dealt for 2 HP removed.
    let one_hit = kill_figures(&setup, &curve, 2.0);
    assert_close(
        "one-hit overkill",
        one_hit.overkill_share,
        1.0 - 2.0 / 6.12,
        1e-9,
    );
    let shares: Vec<f64> = [2.0, 10.0, 50.0, 200.0, 1_000.0]
        .iter()
        .map(|hp| kill_figures(&setup, &curve, *hp).overkill_share)
        .collect();
    assert!(
        shares.windows(2).all(|pair| pair[1] < pair[0]),
        "{shares:?}"
    );
    assert!(shares[4] > 0.0 && shares[4] < 0.005, "{shares:?}");
}
