//! Versioned expected-hunting economics over immutable offensive evidence.
//!
//! Community Model v1 is a planning estimate, not observed loot and not
//! realised accounting. Weapon and amplifier streams retain their own raw TT,
//! consumed limited-item premium, and Efficiency. Healing, protection,
//! harvesting, enhancers, scopes, absorbers, implants, and other components
//! stay outside this model until their return treatment is grounded.

use eo_wire::normalizer::round_half_even;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game_data_store::GameDataStore;

/// Stable identity of the currently implemented community model.
pub const COMMUNITY_MODEL_V1: &str = "community_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffensiveComponentKind {
    Weapon,
    Amplifier,
}

/// Model-neutral evidence for one efficiency-bearing candidate stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffensiveComponentEvidence {
    pub kind: OffensiveComponentKind,
    pub catalog_id: Option<String>,
    pub name: String,
    pub efficiency_pct: Option<f64>,
    /// Loot-bearing TT per activation, in PED. Acquisition premium is absent.
    pub raw_tt_per_use: f64,
    /// Deterministically consumed limited-item premium per activation, in PED.
    pub consumed_premium_per_use: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingLooterLevels {
    pub animal: f64,
    pub mutant: f64,
    pub robot: f64,
}

impl HuntingLooterLevels {
    pub fn three_looter_mean(self) -> f64 {
        (self.animal + self.mutant + self.robot) / 3.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LooterSource {
    Animal,
    Mutant,
    Robot,
    ThreeLooterMean,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffensiveLoadoutEvidence {
    pub components: Vec<OffensiveComponentEvidence>,
    pub looters: HuntingLooterLevels,
    pub looter_source: LooterSource,
}

impl OffensiveLoadoutEvidence {
    pub fn selected_looter_level(&self) -> f64 {
        match self.looter_source {
            LooterSource::Animal => self.looters.animal,
            LooterSource::Mutant => self.looters.mutant,
            LooterSource::Robot => self.looters.robot,
            LooterSource::ThreeLooterMean => self.looters.three_looter_mean(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedHuntingResult {
    pub model_version: String,
    pub looter_source: LooterSource,
    pub looter_level: f64,
    pub weighted_efficiency_pct: Option<f64>,
    pub expected_loot_tt: f64,
    pub modelled_raw_tt: f64,
    pub eligible_offensive_cost: f64,
    pub offensive_tt_recovery: Option<f64>,
    pub expected_tt_rate: Option<f64>,
    pub effective_efficiency: Option<EffectiveEfficiency>,
    pub break_even_loot_markup: Option<f64>,
    pub coverage: f64,
    pub component_count: usize,
    pub modelled_component_count: usize,
    pub incomplete: bool,
}

/// Economic Efficiency equivalent under Community Model v1.
///
/// This never changes a component's in-game Efficiency. It inverts the
/// premium-adjusted expected return into the Efficiency an otherwise
/// identical unlimited offensive setup would require at the same looter
/// level. Rates outside v1's declared Efficiency domain remain explicit
/// rather than being clamped into a plausible value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EffectiveEfficiency {
    WithinModelRange {
        #[serde(rename = "efficiencyPct")]
        efficiency_pct: f64,
    },
    BelowModelRange,
    AboveModelRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExpectedHuntingError {
    #[error("expected-hunting input must be finite")]
    NonFinite,
}

/// Community Model v1 as a multiplier. Inputs are clamped to the model's
/// declared 0..=100 domain; this is not a claim that the game itself caps.
pub fn community_model_v1(
    efficiency_pct: f64,
    looter_level: f64,
) -> Result<f64, ExpectedHuntingError> {
    if !efficiency_pct.is_finite() || !looter_level.is_finite() {
        return Err(ExpectedHuntingError::NonFinite);
    }
    let efficiency = efficiency_pct.clamp(0.0, 100.0);
    let looter = looter_level.clamp(0.0, 100.0);
    Ok(0.86 + 0.07 * efficiency / 100.0 + 0.07 * looter / 100.0)
}

/// Invert a premium-adjusted return into its unlimited Efficiency equivalent.
pub fn effective_efficiency_v1(
    expected_return: f64,
    looter_level: f64,
) -> Result<EffectiveEfficiency, ExpectedHuntingError> {
    if !expected_return.is_finite() || !looter_level.is_finite() {
        return Err(ExpectedHuntingError::NonFinite);
    }
    let lower = community_model_v1(0.0, looter_level)?;
    let upper = community_model_v1(100.0, looter_level)?;
    if expected_return < lower {
        return Ok(EffectiveEfficiency::BelowModelRange);
    }
    if expected_return > upper {
        return Ok(EffectiveEfficiency::AboveModelRange);
    }
    let efficiency = (expected_return - lower) * 100.0 / 0.07;
    Ok(EffectiveEfficiency::WithinModelRange {
        efficiency_pct: round_half_even(efficiency, 2),
    })
}

/// Aggregate component streams by raw TT, preserving premium as denominator
/// drag and excluding components whose Efficiency is unavailable.
pub fn evaluate(
    evidence: &OffensiveLoadoutEvidence,
) -> Result<ExpectedHuntingResult, ExpectedHuntingError> {
    let looter = evidence.selected_looter_level();
    if !looter.is_finite()
        || evidence.components.iter().any(|component| {
            !component.raw_tt_per_use.is_finite()
                || !component.consumed_premium_per_use.is_finite()
                || component
                    .efficiency_pct
                    .is_some_and(|efficiency| !efficiency.is_finite())
        })
    {
        return Err(ExpectedHuntingError::NonFinite);
    }

    let candidate_raw_tt: f64 = evidence
        .components
        .iter()
        .map(|component| component.raw_tt_per_use.max(0.0))
        .sum();
    let mut modelled_raw_tt = 0.0;
    let mut eligible_cost = 0.0;
    let mut expected_loot = 0.0;
    let mut efficiency_weight = 0.0;
    let mut modelled_count = 0;

    for component in &evidence.components {
        let raw = component.raw_tt_per_use.max(0.0);
        if raw <= 0.0 {
            continue;
        }
        let Some(efficiency) = component.efficiency_pct else {
            continue;
        };
        let premium = component.consumed_premium_per_use.max(0.0);
        modelled_raw_tt += raw;
        eligible_cost += raw + premium;
        expected_loot += raw * community_model_v1(efficiency, looter)?;
        efficiency_weight += raw * efficiency;
        modelled_count += 1;
    }

    let ratio = |numerator: f64, denominator: f64| {
        (denominator > 0.0).then(|| round_half_even(numerator / denominator, 6))
    };
    let coverage = if candidate_raw_tt > 0.0 {
        (modelled_raw_tt / candidate_raw_tt).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let incomplete = coverage < 1.0;
    let economic_rate = (eligible_cost > 0.0).then(|| expected_loot / eligible_cost);
    let expected_tt_rate = economic_rate.map(|rate| round_half_even(rate, 6));
    let effective_efficiency = if incomplete {
        None
    } else {
        economic_rate
            .map(|rate| effective_efficiency_v1(rate, looter))
            .transpose()?
    };

    Ok(ExpectedHuntingResult {
        model_version: COMMUNITY_MODEL_V1.to_string(),
        looter_source: evidence.looter_source,
        looter_level: round_half_even(looter, 2),
        weighted_efficiency_pct: ratio(efficiency_weight, modelled_raw_tt)
            .map(|value| round_half_even(value, 2)),
        expected_loot_tt: round_half_even(expected_loot, 8),
        modelled_raw_tt: round_half_even(modelled_raw_tt, 8),
        eligible_offensive_cost: round_half_even(eligible_cost, 8),
        offensive_tt_recovery: ratio(expected_loot, modelled_raw_tt),
        expected_tt_rate,
        effective_efficiency,
        break_even_loot_markup: expected_tt_rate
            .filter(|rate| *rate > 0.0)
            .map(|rate| round_half_even(1.0 / rate, 6)),
        coverage: round_half_even(coverage, 4),
        component_count: evidence.components.len(),
        modelled_component_count: modelled_count,
        incomplete,
    })
}

fn id_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

fn economy_number(entity: &Value, key: &str) -> f64 {
    entity
        .get("economy")
        .and_then(|economy| economy.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// Overlay current catalogue Efficiency onto a saved equipment profile.
///
/// Equipment rows retain the complete catalogue entities that existed when
/// they were saved, so rows created before Efficiency entered the bundled
/// snapshot legitimately lack the field. Efficiency is game metadata rather
/// than a player-entered costing input: current Equipment projections and new
/// tracking phases should use the bundled catalogue value resolved by stable
/// identity. The returned clone leaves the saved row untouched, and tracking
/// still persists the resolved evidence so historical phases never change
/// after a later snapshot refresh.
pub fn with_current_offensive_efficiencies(props: &Value, game_data: &GameDataStore) -> Value {
    let mut enriched = props.clone();
    for (entity_key, id_key, endpoint) in [
        ("weapon_entity", "weapon_catalog_id", "weapons"),
        ("amp_entity", "amp_catalog_id", "weapon_amplifiers"),
    ] {
        let Some(entity) = props.get(entity_key).filter(|entity| !entity.is_null()) else {
            continue;
        };
        let Some(catalog_id) = props
            .get(id_key)
            .filter(|id| !id.is_null())
            .or_else(|| entity.get("id").filter(|id| !id.is_null()))
        else {
            continue;
        };
        let Some(efficiency) = game_data
            .find_entity(endpoint, catalog_id)
            .and_then(|current| current.get("economy"))
            .and_then(|economy| economy.get("efficiency"))
            .and_then(Value::as_f64)
        else {
            continue;
        };
        let Some(saved_entity) = enriched.get_mut(entity_key).and_then(Value::as_object_mut) else {
            continue;
        };
        let economy = saved_entity
            .entry("economy")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(economy) = economy.as_object_mut() {
            economy.insert("efficiency".into(), Value::from(efficiency));
        }
    }
    enriched
}

fn component_from_props(
    props: &Value,
    kind: OffensiveComponentKind,
    entity_key: &str,
    id_key: &str,
    markup_key: &str,
    damage_enhancers: i64,
) -> Option<OffensiveComponentEvidence> {
    let entity = props.get(entity_key).filter(|value| !value.is_null())?;
    // Past the server's attack-rate limit every component of an attack
    // consumes the carried factor more (1 on props never enriched), so the
    // per-use figures agree with the cost engine's; ratios are unchanged.
    let enhancement = if kind == OffensiveComponentKind::Weapon {
        1.0 + 0.1 * damage_enhancers.max(0) as f64
    } else {
        1.0
    };
    let multiplier = enhancement * crate::attack_rate::factor_from_props(props);
    let decay_pec = economy_number(entity, "decay") * multiplier;
    let ammo_pec = economy_number(entity, "ammo_burn") / 100.0 * multiplier;

    // Implant and absorber transfer part of base decay away from the weapon.
    // Those streams are deliberately outside this model, so only the decay
    // retained by the weapon remains eligible.
    let retained_decay_pec = if kind == OffensiveComponentKind::Weapon {
        let share = |key: &str| {
            props
                .get(key)
                .filter(|value| !value.is_null())
                .map(|device| economy_number(device, "absorption").clamp(0.0, 1.0))
                .unwrap_or(0.0)
        };
        decay_pec * (1.0 - share("implant_entity")) * (1.0 - share("absorber_entity"))
    } else {
        decay_pec
    };
    let raw_tt_per_use = (retained_decay_pec + ammo_pec) / 100.0;
    let is_limited = entity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .contains("(L)");
    let markup = props
        .get(markup_key)
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        .max(0.0)
        / 100.0;
    let premium = if is_limited {
        retained_decay_pec / 100.0 * (markup - 1.0).max(0.0)
    } else {
        0.0
    };

    Some(OffensiveComponentEvidence {
        kind,
        catalog_id: props.get(id_key).and_then(id_string),
        name: entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        efficiency_pct: entity
            .get("economy")
            .and_then(|economy| economy.get("efficiency"))
            .and_then(Value::as_f64),
        raw_tt_per_use: round_half_even(raw_tt_per_use, 8),
        consumed_premium_per_use: round_half_even(premium, 8),
    })
}

/// Extract the supported weapon and amplifier streams from one immutable
/// stored equipment payload. Other component families are absent by design.
pub fn evidence_from_equipment_props(
    props: &Value,
    damage_enhancers: Option<i64>,
    looters: HuntingLooterLevels,
) -> OffensiveLoadoutEvidence {
    let configured = damage_enhancers.unwrap_or_else(|| {
        props
            .get("damage_enhancers")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    });
    let mut components = Vec::new();
    if let Some(weapon) = component_from_props(
        props,
        OffensiveComponentKind::Weapon,
        "weapon_entity",
        "weapon_catalog_id",
        "weapon_markup",
        configured,
    ) {
        components.push(weapon);
    }
    if let Some(amplifier) = component_from_props(
        props,
        OffensiveComponentKind::Amplifier,
        "amp_entity",
        "amp_catalog_id",
        "amp_markup",
        configured,
    ) {
        components.push(amplifier);
    }
    OffensiveLoadoutEvidence {
        components,
        looters,
        looter_source: LooterSource::ThreeLooterMean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn looters(level: f64) -> HuntingLooterLevels {
        HuntingLooterLevels {
            animal: level,
            mutant: level,
            robot: level,
        }
    }

    #[test]
    fn the_attack_rate_scales_per_use_evidence_and_leaves_every_rate_alone() {
        let props = json!({
            "weapon_entity": {
                "name": "Rapid (L)",
                "uses_per_minute": 90,
                "economy": {"decay": 2.0, "ammo_burn": 200, "efficiency": 60.0},
            },
            "weapon_markup": 120,
            "amp_entity": {
                "name": "Amp",
                "economy": {"decay": 0.5, "ammo_burn": 50, "efficiency": 70.0},
            },
        });
        let own = evaluate(&evidence_from_equipment_props(&props, None, looters(20.0))).unwrap();
        let rated_props = crate::attack_rate::with_attack_rate(&props, None, 30.0);
        let rated = evaluate(&evidence_from_equipment_props(
            &rated_props,
            None,
            looters(20.0),
        ))
        .unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(rated.modelled_raw_tt, own.modelled_raw_tt * 1.17));
        assert!(close(
            rated.eligible_offensive_cost,
            own.eligible_offensive_cost * 1.17
        ));
        assert!(close(rated.expected_loot_tt, own.expected_loot_tt * 1.17));
        assert_eq!(rated.expected_tt_rate, own.expected_tt_rate);
        assert_eq!(rated.break_even_loot_markup, own.break_even_loot_markup);
        assert_eq!(rated.weighted_efficiency_pct, own.weighted_efficiency_pct);
    }

    #[test]
    fn community_v1_declares_its_domain_and_version_examples() {
        assert_eq!(COMMUNITY_MODEL_V1, "community_v1");
        assert_eq!(community_model_v1(0.0, 0.0).unwrap(), 0.86);
        assert_eq!(community_model_v1(100.0, 100.0).unwrap(), 1.0);
        assert_eq!(community_model_v1(120.0, 130.0).unwrap(), 1.0);
        assert_eq!(community_model_v1(-5.0, -5.0).unwrap(), 0.86);
        assert_eq!(
            community_model_v1(f64::NAN, 50.0),
            Err(ExpectedHuntingError::NonFinite)
        );
    }

    #[test]
    fn weighted_components_keep_efficiencies_and_premiums_separate() {
        let evidence = OffensiveLoadoutEvidence {
            components: vec![
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Weapon,
                    catalog_id: Some("weapon".into()),
                    name: "Weapon".into(),
                    efficiency_pct: Some(90.0),
                    raw_tt_per_use: 0.03,
                    consumed_premium_per_use: 0.0,
                },
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Amplifier,
                    catalog_id: Some("amp".into()),
                    name: "Amp".into(),
                    efficiency_pct: Some(60.0),
                    raw_tt_per_use: 0.01,
                    consumed_premium_per_use: 0.002,
                },
            ],
            looters: looters(50.0),
            looter_source: LooterSource::ThreeLooterMean,
        };
        let result = evaluate(&evidence).unwrap();
        assert_eq!(result.weighted_efficiency_pct, Some(82.5));
        assert!((result.expected_loot_tt - 0.03811).abs() < 1e-9);
        assert_eq!(result.modelled_raw_tt, 0.04);
        assert_eq!(result.eligible_offensive_cost, 0.042);
        assert!(result.expected_tt_rate.unwrap() < result.offensive_tt_recovery.unwrap());
        assert_eq!(
            result.effective_efficiency,
            Some(EffectiveEfficiency::WithinModelRange {
                efficiency_pct: 17.69
            })
        );
        assert!(!result.incomplete);
    }

    #[test]
    fn effective_efficiency_inverts_the_markup_adjusted_return() {
        let raw_return = community_model_v1(90.0, 50.0).unwrap();
        let economic_return = raw_return - 0.0035;
        assert_eq!(
            effective_efficiency_v1(economic_return, 50.0).unwrap(),
            EffectiveEfficiency::WithinModelRange {
                efficiency_pct: 85.0
            }
        );
        assert_eq!(
            effective_efficiency_v1(raw_return - 0.01, 50.0).unwrap(),
            EffectiveEfficiency::WithinModelRange {
                efficiency_pct: 75.71
            }
        );
    }

    #[test]
    fn effective_efficiency_keeps_out_of_domain_results_explicit() {
        assert_eq!(
            effective_efficiency_v1(0.80, 50.0).unwrap(),
            EffectiveEfficiency::BelowModelRange
        );
        assert_eq!(
            effective_efficiency_v1(1.01, 50.0).unwrap(),
            EffectiveEfficiency::AboveModelRange
        );
        assert_eq!(
            effective_efficiency_v1(f64::NAN, 50.0),
            Err(ExpectedHuntingError::NonFinite)
        );
    }

    #[test]
    fn missing_efficiency_narrows_coverage_instead_of_becoming_zero_return() {
        let evidence = OffensiveLoadoutEvidence {
            components: vec![
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Weapon,
                    catalog_id: None,
                    name: "Known".into(),
                    efficiency_pct: Some(80.0),
                    raw_tt_per_use: 0.03,
                    consumed_premium_per_use: 0.0,
                },
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Amplifier,
                    catalog_id: None,
                    name: "Unknown".into(),
                    efficiency_pct: None,
                    raw_tt_per_use: 0.01,
                    consumed_premium_per_use: 4.0,
                },
            ],
            looters: looters(50.0),
            looter_source: LooterSource::ThreeLooterMean,
        };
        let result = evaluate(&evidence).unwrap();
        assert_eq!(result.coverage, 0.75);
        assert_eq!(result.eligible_offensive_cost, 0.03);
        assert_eq!(result.effective_efficiency, None);
        assert!(result.incomplete);
    }

    #[test]
    fn zero_tt_components_do_not_create_false_incompleteness() {
        let evidence = OffensiveLoadoutEvidence {
            components: vec![
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Weapon,
                    catalog_id: None,
                    name: "Known".into(),
                    efficiency_pct: Some(80.0),
                    raw_tt_per_use: 0.03,
                    consumed_premium_per_use: 0.0,
                },
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Amplifier,
                    catalog_id: None,
                    name: "No TT stream".into(),
                    efficiency_pct: None,
                    raw_tt_per_use: 0.0,
                    consumed_premium_per_use: 0.0,
                },
            ],
            looters: looters(50.0),
            looter_source: LooterSource::ThreeLooterMean,
        };
        let result = evaluate(&evidence).unwrap();
        assert_eq!(result.coverage, 1.0);
        assert!(!result.incomplete);
        assert!(result.effective_efficiency.is_some());
    }

    #[test]
    fn extraction_amortises_high_limited_markup_only_over_decay() {
        let props = json!({
            "weapon_catalog_id": "nano",
            "weapon_markup": 1500.0,
            "weapon_entity": {
                "name": "Nanochip (L)",
                "economy": {"decay": 0.01, "ammo_burn": 1000.0, "efficiency": 85.0}
            },
            "amp_entity": null
        });
        let evidence = evidence_from_equipment_props(&props, None, looters(50.0));
        let weapon = &evidence.components[0];
        assert_eq!(weapon.raw_tt_per_use, 0.1001);
        assert_eq!(weapon.consumed_premium_per_use, 0.0014);
        assert!(weapon.consumed_premium_per_use < weapon.raw_tt_per_use / 50.0);
    }

    #[test]
    fn exact_three_looter_mean_excludes_every_other_profession() {
        let levels = HuntingLooterLevels {
            animal: 30.0,
            mutant: 60.0,
            robot: 90.0,
        };
        assert_eq!(levels.three_looter_mean(), 60.0);
    }

    #[test]
    fn component_order_does_not_change_the_weighted_result() {
        let component = |name: &str, efficiency, raw| OffensiveComponentEvidence {
            kind: OffensiveComponentKind::Weapon,
            catalog_id: None,
            name: name.into(),
            efficiency_pct: Some(efficiency),
            raw_tt_per_use: raw,
            consumed_premium_per_use: 0.0,
        };
        let evaluate_components = |components| {
            evaluate(&OffensiveLoadoutEvidence {
                components,
                looters: looters(50.0),
                looter_source: LooterSource::ThreeLooterMean,
            })
            .unwrap()
        };
        let forward = evaluate_components(vec![
            component("Low", 40.0, 0.01),
            component("High", 90.0, 0.03),
        ]);
        let reverse = evaluate_components(vec![
            component("High", 90.0, 0.03),
            component("Low", 40.0, 0.01),
        ]);
        assert_eq!(forward, reverse);
        assert_eq!(forward.weighted_efficiency_pct, Some(77.5));
    }

    #[test]
    fn looter_source_selects_one_profession_or_the_labelled_mean() {
        let levels = HuntingLooterLevels {
            animal: 25.0,
            mutant: 50.0,
            robot: 75.0,
        };
        for (source, expected) in [
            (LooterSource::Animal, 25.0),
            (LooterSource::Mutant, 50.0),
            (LooterSource::Robot, 75.0),
            (LooterSource::ThreeLooterMean, 50.0),
        ] {
            let evidence = OffensiveLoadoutEvidence {
                components: vec![],
                looters: levels,
                looter_source: source,
            };
            assert_eq!(evidence.selected_looter_level(), expected);
        }
    }

    #[test]
    fn folklore_example_stays_below_break_even_at_exact_thresholds() {
        let tt_rate = community_model_v1(90.0, 50.0).unwrap();
        assert_eq!(round_half_even(tt_rate, 6), 0.958);
        assert_eq!(round_half_even(tt_rate * 1.04, 6), 0.99632);
        assert_eq!(round_half_even(1.0 / tt_rate, 6), 1.043841);
    }

    #[test]
    fn unrelated_cost_payloads_cannot_enter_the_offensive_model() {
        let base = json!({
            "weapon_catalog_id": "weapon",
            "weapon_entity": {
                "name": "Weapon",
                "economy": {"decay": 1.0, "ammo_burn": 100.0, "efficiency": 80.0}
            },
            "amp_entity": null
        });
        let mut with_ancillary = base.clone();
        let object = with_ancillary.as_object_mut().unwrap();
        object.insert("healing_cost".into(), json!(999.0));
        object.insert("armour_cost".into(), json!(999.0));
        object.insert("harvest_cost".into(), json!(999.0));

        let plain = evidence_from_equipment_props(&base, None, looters(50.0));
        let ancillary = evidence_from_equipment_props(&with_ancillary, None, looters(50.0));
        assert_eq!(plain, ancillary);
        assert_eq!(evaluate(&plain).unwrap(), evaluate(&ancillary).unwrap());
    }

    #[test]
    fn current_catalogue_efficiency_enriches_legacy_props_without_rewriting_costs() {
        let snapshot = tempfile::tempdir().unwrap();
        std::fs::write(
            snapshot.path().join("weapons.json"),
            r#"[{"id":"weapon","name":"Legacy weapon","economy":{"decay":9.0,"efficiency":56.7}}]"#,
        )
        .unwrap();
        std::fs::write(
            snapshot.path().join("weapon_amplifiers.json"),
            r#"[{"id":"amp","name":"Legacy amp","economy":{"decay":8.0,"efficiency":75.0}}]"#,
        )
        .unwrap();
        let game_data = GameDataStore::new(snapshot.path()).unwrap();
        let props = json!({
            "weapon_catalog_id": "weapon",
            "weapon_entity": {
                "id": "weapon",
                "name": "Legacy weapon",
                "economy": {"decay": 1.0, "ammo_burn": 100.0}
            },
            "amp_catalog_id": "amp",
            "amp_entity": {
                "id": "amp",
                "name": "Legacy amp",
                "economy": {"decay": 0.5, "efficiency": 60.0}
            }
        });

        let enriched = with_current_offensive_efficiencies(&props, &game_data);

        assert_eq!(enriched["weapon_entity"]["economy"]["efficiency"], 56.7);
        assert_eq!(enriched["amp_entity"]["economy"]["efficiency"], 75.0);
        assert_eq!(enriched["weapon_entity"]["economy"]["decay"], 1.0);
        assert_eq!(enriched["amp_entity"]["economy"]["decay"], 0.5);
        assert!(props["weapon_entity"]["economy"]["efficiency"].is_null());
        assert_eq!(props["amp_entity"]["economy"]["efficiency"], 60.0);
    }
}
