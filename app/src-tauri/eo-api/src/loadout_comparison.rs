//! The loadout comparison: offensive setups built from the catalogue,
//! compared per kill at every mob HP from 1 to 1,000.
//!
//! Stateless: the frontend sends the setups and receives the whole
//! comparison. Nothing is stored and nothing touches the equipment
//! library. Every figure is a planning estimate under Community Model v1
//! and the disclosed assumptions; none of it reaches realised accounting.

use std::collections::HashMap;

use eo_services::cost_engine::is_limited;
use eo_services::expected_hunting::HuntingLooterLevels;
use eo_services::loadout_comparison::{
    self as engine, ComparisonAssumptions, SetupEconomics, Unmodelled,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::equipment::{EquipmentEffectiveEfficiency, ExpectedLooterSource};
use crate::Nullable;
use crate::{Api, ApiError};

// ── Vocabulary ──────────────────────────────────────────────────────

/// The parts a setup is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SetupComponentRole {
    Weapon,
    Amplifier,
    Absorber,
    Implant,
}

// ── Request DTOs ────────────────────────────────────────────────────

/// One catalogue item in a setup.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComponentPick {
    pub catalog_id: String,
    /// The markup a limited item is bought at, percent (100 is TT; absent
    /// reads as 100). Ignored for an unlimited item, which runs at TT.
    #[serde(default)]
    pub markup_percent: Option<f64>,
}

/// A setup: a catalogue weapon with an optional amplifier, absorber, and
/// implant.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupPick {
    pub weapon: ComponentPick,
    #[serde(default)]
    pub amplifier: Option<ComponentPick>,
    #[serde(default)]
    pub absorber: Option<ComponentPick>,
    #[serde(default)]
    pub implant: Option<ComponentPick>,
}

// ── Response DTOs ───────────────────────────────────────────────────

/// The modelling assumptions every figure is estimated under, for the
/// disclosure beside them.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonAssumptionsView {
    pub model_version: String,
    pub looter_source: ExpectedLooterSource,
    pub looter_level: f64,
    /// The share of uses that land.
    pub hit_rate: f64,
    /// The lowest roll as a share of max damage.
    pub damage_floor: f64,
    pub critical_chance: f64,
    /// A critical multiplies the rolled damage by this.
    pub critical_multiplier: f64,
}

/// One part of a setup as priced.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupComponentView {
    pub role: SetupComponentRole,
    pub catalog_id: String,
    pub name: String,
    pub is_limited: bool,
    /// The markup it runs at: the given one for a limited item, 100 for an
    /// unlimited one.
    pub markup_percent: f64,
}

/// Per-use economics, PEC per use unless stated.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupEconomicsView {
    pub cost_per_use_pec: f64,
    pub loss_per_use_pec: f64,
    /// Limited-item markup the weapon and amplifier consume per use.
    pub consumed_premium_pec: f64,
    /// Absorber and implant decay: cost with no modelled return.
    pub outside_model_pec: f64,
    pub effective_efficiency: Nullable<EquipmentEffectiveEfficiency>,
    pub weighted_efficiency_pct: Nullable<f64>,
    pub max_damage: f64,
    /// Amplifier damage above half the weapon's: paid for, never dealt.
    pub amp_damage_wasted: f64,
    pub damage_per_pec: f64,
    /// Expected loss per HP removed: the per-HP ranking metric.
    pub loss_per_hp_pec: f64,
    pub uses_per_minute: Nullable<f64>,
    pub damage_per_second: Nullable<f64>,
}

/// What a setup costs to kill a mob at one HP.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KillFiguresView {
    pub hp: f64,
    pub uses: f64,
    pub loss_pec: f64,
    pub cycled_pec: f64,
    pub seconds: Nullable<f64>,
}

/// Why a setup has no figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnmodelledReason {
    NoDamage,
    NoCost,
    MissingEfficiency,
}

/// One compared setup.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComparedSetup {
    /// Its position in the request.
    pub index: u32,
    /// The weapon's name.
    pub name: String,
    pub components: Vec<SetupComponentView>,
    pub economics: Nullable<SetupEconomicsView>,
    pub unmodelled: Nullable<UnmodelledReason>,
    /// Kill figures at each sampled HP; empty when the setup cannot be
    /// priced.
    pub curve: Vec<KillFiguresView>,
}

/// A stretch of HP where one setup is the cheapest per kill; consecutive
/// segments meet where the cheapest setup changes.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WinnerSegmentView {
    /// The setup's position in the request.
    pub index: u32,
    pub from_hp: f64,
    pub to_hp: f64,
}

/// The comparison.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadoutComparison {
    pub assumptions: ComparisonAssumptionsView,
    /// The highest mob HP the curves reach.
    pub hp_max: f64,
    pub setups: Vec<ComparedSetup>,
    /// The cheapest priced setup across the curve, lowest HP first; empty
    /// unless at least two setups can be priced.
    pub winners: Vec<WinnerSegmentView>,
}

// ── Assembly ────────────────────────────────────────────────────────

const ASSUMPTIONS: ComparisonAssumptions = ComparisonAssumptions::STANDARD;

/// The most setups one comparison takes: a readable chart's worth.
pub const MAX_COMPARED_SETUPS: usize = 8;

fn setup_label(index: usize) -> String {
    format!("Setup {}", index + 1)
}

fn role_label(role: SetupComponentRole) -> &'static str {
    match role {
        SetupComponentRole::Weapon => "weapon",
        SetupComponentRole::Amplifier => "amplifier",
        SetupComponentRole::Absorber => "absorber",
        SetupComponentRole::Implant => "implant",
    }
}

fn unmodelled_reason(reason: Unmodelled) -> UnmodelledReason {
    match reason {
        // The facade always builds a weapon; a missing one cannot reach here.
        Unmodelled::NoWeapon | Unmodelled::NoDamage => UnmodelledReason::NoDamage,
        Unmodelled::NoCost => UnmodelledReason::NoCost,
        Unmodelled::MissingEfficiency => UnmodelledReason::MissingEfficiency,
    }
}

fn economics_view(economics: &SetupEconomics) -> SetupEconomicsView {
    SetupEconomicsView {
        cost_per_use_pec: economics.cost_per_use_pec,
        loss_per_use_pec: economics.loss_per_use_pec,
        consumed_premium_pec: economics.consumed_premium_pec,
        outside_model_pec: economics.outside_model_pec,
        effective_efficiency: economics
            .expected_return
            .effective_efficiency
            .map(Into::into)
            .into(),
        weighted_efficiency_pct: economics.expected_return.weighted_efficiency_pct.into(),
        max_damage: economics.max_damage,
        amp_damage_wasted: economics.amp_damage_wasted,
        damage_per_pec: economics.damage_per_pec,
        loss_per_hp_pec: economics.loss_per_hp_pec,
        uses_per_minute: economics.uses_per_minute.into(),
        damage_per_second: economics.damage_per_second.into(),
    }
}

/// One setup resolved against the catalogue: weapon props in the stored
/// library shape, and the parts as priced.
struct ResolvedSetup {
    name: String,
    components: Vec<SetupComponentView>,
    props: Value,
}

impl Api {
    /// Compare setups per kill from 1 to 1,000 HP.
    pub async fn loadout_compare(
        &self,
        setups: Vec<SetupPick>,
    ) -> Result<LoadoutComparison, ApiError> {
        if setups.is_empty() || setups.len() > MAX_COMPARED_SETUPS {
            return Err(ApiError::bad_request(format!(
                "a comparison takes between 1 and {MAX_COMPARED_SETUPS} setups"
            )));
        }
        let resolved = setups
            .iter()
            .enumerate()
            .map(|(index, pick)| self.resolve_setup(index, pick))
            .collect::<Result<Vec<_>, _>>()?;

        // The player's looters drive the return model.
        let professions = self.character_professions().await?;
        let levels: HashMap<String, f64> = professions
            .iter()
            .map(|profession| (profession.name.clone(), profession.level))
            .collect();
        let level = |name: &str| levels.get(name).copied().unwrap_or(0.0);
        let looters = HuntingLooterLevels {
            animal: level("Animal Looter"),
            mutant: level("Mutant Looter"),
            robot: level("Robot Looter"),
        };

        let economics: Vec<Result<SetupEconomics, UnmodelledReason>> = resolved
            .iter()
            .map(|setup| {
                let rate = self
                    .weapon_pricing
                    .attack_rate(&setup.props)
                    .map(|rate| rate.effective_per_minute);
                engine::setup_economics(&setup.props, looters, rate, &ASSUMPTIONS)
                    .map_err(unmodelled_reason)
            })
            .collect();
        let priced: Vec<SetupEconomics> = economics
            .iter()
            .filter_map(|result| result.as_ref().ok().cloned())
            .collect();
        let outcome = engine::compare(&priced, &ASSUMPTIONS);
        // The outcome indexes the priced setups; map back to request order.
        let priced_indices: Vec<usize> = economics
            .iter()
            .enumerate()
            .filter(|(_, result)| result.is_ok())
            .map(|(index, _)| index)
            .collect();
        let curve_of = |index: usize| -> Vec<KillFiguresView> {
            let Some(position) = priced_indices.iter().position(|priced| *priced == index) else {
                return Vec::new();
            };
            outcome
                .as_ref()
                .map(|outcome| {
                    outcome.curves[position]
                        .iter()
                        .map(|point| KillFiguresView {
                            hp: point.hp,
                            uses: point.uses,
                            loss_pec: point.loss_pec,
                            cycled_pec: point.cycled_pec,
                            seconds: point.seconds.into(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        let setups = resolved
            .iter()
            .zip(&economics)
            .enumerate()
            .map(|(index, (setup, economics))| ComparedSetup {
                index: index as u32,
                name: setup.name.clone(),
                components: setup.components.clone(),
                economics: economics.as_ref().ok().map(economics_view).into(),
                unmodelled: economics.as_ref().err().copied().into(),
                curve: curve_of(index),
            })
            .collect();

        // The cheapest setup only means something with two or more priced.
        let winners = match outcome.as_ref().filter(|_| priced_indices.len() >= 2) {
            Some(outcome) => outcome
                .winners
                .iter()
                .map(|segment| WinnerSegmentView {
                    index: priced_indices[segment.setup] as u32,
                    from_hp: segment.from_hp,
                    to_hp: segment.to_hp,
                })
                .collect(),
            None => Vec::new(),
        };

        Ok(LoadoutComparison {
            assumptions: ComparisonAssumptionsView {
                model_version: engine::RETURN_MODEL.to_string(),
                looter_source: ExpectedLooterSource::ThreeLooterMean,
                looter_level: eo_wire::normalizer::round_half_even(looters.three_looter_mean(), 2),
                hit_rate: ASSUMPTIONS.hit_rate,
                damage_floor: ASSUMPTIONS.damage_floor,
                critical_chance: ASSUMPTIONS.critical_chance,
                critical_multiplier: ASSUMPTIONS.critical_multiplier,
            },
            hp_max: engine::CURVE_HP_MAX,
            setups,
            winners,
        })
    }

    /// Build a setup's props from the catalogue, in the stored library
    /// shape, enriched with the attack rate in force. A limited item runs
    /// at its given markup; an unlimited one at TT, since it is repaired at
    /// TT whatever it cost.
    fn resolve_setup(&self, index: usize, pick: &SetupPick) -> Result<ResolvedSetup, ApiError> {
        let parts = [
            (
                SetupComponentRole::Weapon,
                "weapons",
                "weapon",
                Some(&pick.weapon),
            ),
            (
                SetupComponentRole::Amplifier,
                "weapon_amplifiers",
                "amp",
                pick.amplifier.as_ref(),
            ),
            (
                SetupComponentRole::Absorber,
                "absorbers",
                "absorber",
                pick.absorber.as_ref(),
            ),
            (
                SetupComponentRole::Implant,
                "mindforce_implants",
                "implant",
                pick.implant.as_ref(),
            ),
        ];
        let mut props = Map::new();
        let mut components = Vec::new();
        for (role, endpoint, prefix, component) in parts {
            let Some(component) = component else {
                // An absent implant leaves its keys out entirely, as the
                // library stores an implant-free weapon.
                if role != SetupComponentRole::Implant {
                    props.insert(format!("{prefix}_entity"), Value::Null);
                    props.insert(format!("{prefix}_catalog_id"), Value::Null);
                    props.insert(format!("{prefix}_markup"), json!(100));
                }
                continue;
            };
            let catalog_id = component.catalog_id.trim();
            let id = Value::String(catalog_id.to_string());
            let entity = self
                .game_data
                .find_entity(endpoint, &id)
                .filter(|_| !catalog_id.is_empty())
                .ok_or_else(|| {
                    ApiError::not_found(format!(
                        "{}'s {} '{catalog_id}' is not in the catalogue",
                        setup_label(index),
                        role_label(role)
                    ))
                })?;
            let limited = is_limited(entity);
            let markup = if limited {
                let markup = component.markup_percent.unwrap_or(100.0);
                if !(markup.is_finite() && (100.0..=1_000_000.0).contains(&markup)) {
                    return Err(ApiError::bad_request(format!(
                        "{}'s {} markup must be a percentage of at least 100",
                        setup_label(index),
                        role_label(role)
                    )));
                }
                markup
            } else {
                100.0
            };
            props.insert(format!("{prefix}_entity"), entity.clone());
            props.insert(format!("{prefix}_catalog_id"), id);
            props.insert(format!("{prefix}_markup"), json!(markup));
            components.push(SetupComponentView {
                role,
                catalog_id: catalog_id.to_string(),
                name: entity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_string(),
                is_limited: limited,
                markup_percent: markup,
            });
        }
        props.insert("scope_entity".into(), Value::Null);
        props.insert("scope_catalog_id".into(), Value::Null);
        props.insert("scope_markup".into(), json!(100));
        props.insert("damage_enhancers".into(), json!(0));
        Ok(ResolvedSetup {
            name: components[0].name.clone(),
            components,
            props: self.weapon_pricing.prepare(&Value::Object(props)),
        })
    }
}
