//! What one dose of a consumable is: its printed effects, how long they
//! last, and what the dose costs.
//!
//! A catalogue item takes its effects, duration, and TT value from the
//! bundled snapshot (the current catalogue first, then the entity saved with
//! the item); a custom item declares them. The acquisition markup and whether
//! each dose's cost is booked are the player's own, stored with the item.
//! The profile is snapshotted onto every dose, so a later Equipment edit
//! never reinterprets a dose already taken.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game_data_store::GameDataStore;

/// What the app evaluates of an effect. The upstream vocabulary is open (a
/// few dozen names); only the reload-speed pair feeds an evaluator, and every
/// other effect is carried for display under its printed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoseEffectKind {
    ReloadSpeedIncreased,
    ReloadSpeedDecreased,
    Other,
}

impl DoseEffectKind {
    /// Classify a printed effect name.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "reload speed increased" => Self::ReloadSpeedIncreased,
            "reload speed decreased" => Self::ReloadSpeedDecreased,
            _ => Self::Other,
        }
    }
}

/// One effect a dose grants, as the item prints it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoseEffect {
    pub name: String,
    pub kind: DoseEffectKind,
    /// The printed strength, unsigned; the name says which way it goes.
    pub strength: Option<f64>,
    pub unit: Option<String>,
}

impl DoseEffect {
    pub fn new(name: &str, strength: Option<f64>, unit: Option<&str>) -> Self {
        Self {
            name: name.trim().to_string(),
            kind: DoseEffectKind::from_name(name),
            strength: strength.filter(|value| value.is_finite()),
            unit: unit.map(str::to_string),
        }
    }

    /// The signed reload speed this effect adds, percent, when it is a
    /// reload-speed effect printed in percent.
    pub fn reload_speed_percent(&self) -> Option<f64> {
        let strength = self.strength?.abs();
        if self.unit.as_deref() != Some("%") {
            return None;
        }
        match self.kind {
            DoseEffectKind::ReloadSpeedIncreased => Some(strength),
            DoseEffectKind::ReloadSpeedDecreased => Some(-strength),
            DoseEffectKind::Other => None,
        }
    }
}

/// The reload speed a set of effects adds, percent, summed as the item
/// prints it; the game's limits apply in [`crate::passive_effects`].
pub fn effects_reload_speed_percent(effects: &[DoseEffect]) -> f64 {
    effects
        .iter()
        .filter_map(DoseEffect::reload_speed_percent)
        .sum()
}

/// One dose of a configured consumable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumableProfile {
    /// How long the effects last, seconds; 0 for an item whose effect is
    /// immediate (a heal pill) or unknown.
    pub duration_seconds: f64,
    pub effects: Vec<DoseEffect>,
    /// One dose's TT value, PED.
    pub tt_value_ped: f64,
    /// The acquisition markup, percent of TT.
    pub markup_percent: f64,
    /// Whether taking a dose books its cost to the session.
    pub track_cost: bool,
}

impl ConsumableProfile {
    /// What one dose cost to acquire, PED: its TT value at the recorded
    /// markup. Current market estimates never substitute for it.
    pub fn dose_cost_ped(&self) -> f64 {
        let cost = self.tt_value_ped * self.markup_percent / 100.0;
        if cost.is_finite() && cost > 0.0 {
            cost
        } else {
            0.0
        }
    }

    /// What taking a dose books to the session, PED: the dose cost when
    /// tracking is on, nothing otherwise.
    pub fn booked_cost_ped(&self) -> f64 {
        if self.track_cost {
            self.dose_cost_ped()
        } else {
            0.0
        }
    }

    /// Whether the dose lasts: an immediate item books its cost and ends.
    pub fn is_timed(&self) -> bool {
        self.duration_seconds > 0.0
    }
}

/// Where a stored consumable's catalogue data came from, for Equipment to
/// say which figures the player may declare.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConsumable {
    pub profile: ConsumableProfile,
    /// Whether the effects come from the catalogue.
    pub catalogue_effects: bool,
    /// Whether the duration comes from the catalogue.
    pub catalogue_duration: bool,
    /// Whether the TT value comes from the catalogue.
    pub catalogue_value: bool,
}

/// The consumable's profile from its stored properties, over the current
/// catalogue when it has one.
pub fn consumable_profile_from_props(
    props: &Value,
    game_data: Option<&GameDataStore>,
) -> ResolvedConsumable {
    let dose = props.get("dose");
    let declared = |key: &str| dose.and_then(|dose| dose.get(key)).and_then(Value::as_f64);
    let entity = catalogue_entity(props, game_data);
    let catalogue_effects = entity.map(catalogue_effects).unwrap_or_default();
    let catalogue_duration = entity.and_then(catalogue_duration);
    let catalogue_tt = entity
        .and_then(|entity| entity.get("economy"))
        .and_then(|economy| economy.get("max_tt"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);

    let has_catalogue_effects = !catalogue_effects.is_empty();
    let effects = if has_catalogue_effects {
        catalogue_effects
    } else {
        declared_effects(declared("reload_speed_percent"))
    };
    let duration_seconds = catalogue_duration
        .or_else(|| declared("duration_seconds"))
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(0.0);
    let tt_value_ped = catalogue_tt
        .or_else(|| declared("tt_value_ped"))
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let markup_percent = declared("markup_percent")
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(100.0);
    // Items saved before cost tracking existed booked nothing; they keep
    // doing so until the player turns it on.
    let track_cost = dose
        .and_then(|dose| dose.get("track_cost"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ResolvedConsumable {
        profile: ConsumableProfile {
            duration_seconds,
            effects,
            tt_value_ped,
            markup_percent,
            track_cost,
        },
        catalogue_effects: has_catalogue_effects,
        catalogue_duration: catalogue_duration.is_some(),
        catalogue_value: catalogue_tt.is_some(),
    }
}

/// The effect list a custom item declares: only a reload speed can be
/// declared, since it is the only effect the app evaluates.
fn declared_effects(reload_speed_percent: Option<f64>) -> Vec<DoseEffect> {
    match reload_speed_percent.filter(|value| value.is_finite() && *value != 0.0) {
        Some(percent) if percent > 0.0 => {
            vec![DoseEffect::new(
                "Reload Speed Increased",
                Some(percent),
                Some("%"),
            )]
        }
        Some(percent) => vec![DoseEffect::new(
            "Reload Speed Decreased",
            Some(-percent),
            Some("%"),
        )],
        None => Vec::new(),
    }
}

/// The stimulant's catalogue row: the current catalogue's when the item
/// resolves there, else the entity saved with the item.
fn catalogue_entity<'a>(
    props: &'a Value,
    game_data: Option<&'a GameDataStore>,
) -> Option<&'a Value> {
    let saved = props.get("entity").filter(|entity| entity.is_object());
    let catalog_id = props
        .get("catalog_id")
        .filter(|id| !id.is_null())
        .or_else(|| saved.and_then(|entity| entity.get("id")));
    game_data
        .zip(catalog_id)
        .and_then(|(game_data, id)| game_data.find_entity("stimulants", id))
        .or(saved)
}

fn catalogue_effects(entity: &Value) -> Vec<DoseEffect> {
    timed_effects(entity.get("effects"))
}

fn catalogue_duration(entity: &Value) -> Option<f64> {
    longest_duration(entity.get("effects"))
}

fn timed_effects(list: Option<&Value>) -> Vec<DoseEffect> {
    list.and_then(Value::as_array)
        .map(|effects| {
            effects
                .iter()
                .filter_map(|effect| {
                    let name = effect.get("name").and_then(Value::as_str)?;
                    Some(DoseEffect::new(
                        name,
                        effect.get("strength").and_then(Value::as_f64),
                        effect.get("unit").and_then(Value::as_str),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn longest_duration(list: Option<&Value>) -> Option<f64> {
    list.and_then(Value::as_array)?
        .iter()
        .filter_map(|effect| effect.get("duration_seconds").and_then(Value::as_f64))
        .filter(|value| value.is_finite() && *value > 0.0)
        .reduce(f64::max)
}

/// A buff a healing tool grants with each paid use (Eir Mk 1: reload speed
/// for eight seconds). Only effects the app evaluates open a dose; a
/// tool's printed heal-over-time is the healing profile's business.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnUseEffect {
    pub duration_seconds: f64,
    pub effects: Vec<DoseEffect>,
}

/// The on-use buff of a healing tool, from the current catalogue's medical
/// tool row (else the entity saved with the tool). None when the tool
/// grants nothing the app evaluates.
pub fn on_use_effect_from_props(
    props: &Value,
    game_data: Option<&GameDataStore>,
) -> Option<OnUseEffect> {
    let saved = props.get("tool_entity").filter(|entity| entity.is_object());
    let catalog_id = props
        .get("tool_catalog_id")
        .filter(|id| !id.is_null())
        .or_else(|| saved.and_then(|entity| entity.get("id")));
    let entity = game_data
        .zip(catalog_id)
        .and_then(|(game_data, id)| game_data.find_entity("medical_tools", id))
        .or(saved)?;
    let list = entity.get("effects_on_use")?.as_array()?;
    let mut effects = Vec::new();
    let mut duration: Option<f64> = None;
    for raw in list {
        let Some(name) = raw.get("name").and_then(Value::as_str) else {
            continue;
        };
        let effect = DoseEffect::new(
            name,
            raw.get("strength").and_then(Value::as_f64),
            raw.get("unit").and_then(Value::as_str),
        );
        let seconds = raw
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0);
        if effect.reload_speed_percent().is_some() {
            if let Some(seconds) = seconds {
                duration = Some(duration.map_or(seconds, |longest| longest.max(seconds)));
                effects.push(effect);
            }
        }
    }
    Some(OnUseEffect {
        duration_seconds: duration?,
        effects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn adrenaline() -> Value {
        json!({
            "id": "73f99a215803",
            "name": "Nanobots - Adrenaline Boost",
            "economy": {"max_tt": 3},
            "effects": [{
                "name": "Reload Speed Increased",
                "strength": 10,
                "unit": "%",
                "duration_seconds": 3600
            }]
        })
    }

    #[test]
    fn a_catalogue_item_takes_its_effects_duration_and_value_from_the_entity() {
        let props = json!({
            "catalog_id": "73f99a215803",
            "entity": adrenaline(),
            "dose": {"markup_percent": 150.0, "track_cost": true}
        });
        let resolved = consumable_profile_from_props(&props, None);
        assert!(resolved.catalogue_effects);
        assert!(resolved.catalogue_duration);
        assert!(resolved.catalogue_value);
        let profile = resolved.profile;
        assert_eq!(profile.duration_seconds, 3600.0);
        assert_eq!(effects_reload_speed_percent(&profile.effects), 10.0);
        assert!((profile.dose_cost_ped() - 4.5).abs() < 1e-12);
        assert!((profile.booked_cost_ped() - 4.5).abs() < 1e-12);
    }

    #[test]
    fn an_item_saved_before_dose_settings_books_nothing() {
        // The shape every consumable had before doses: a name and a
        // catalogue pick, with no dose settings.
        let props = json!({"catalog_id": "73f99a215803", "entity": {"id": "73f99a215803", "name": "Nanobots - Adrenaline Boost"}});
        let profile = consumable_profile_from_props(&props, None).profile;
        assert!(!profile.track_cost);
        assert_eq!(profile.booked_cost_ped(), 0.0);
        assert_eq!(profile.markup_percent, 100.0);
        assert!(profile.effects.is_empty());
        assert!(!profile.is_timed());
    }

    #[test]
    fn a_custom_item_declares_its_reload_speed_duration_and_value() {
        let props = json!({
            "catalog_id": null,
            "entity": null,
            "dose": {
                "reload_speed_percent": 12.0,
                "duration_seconds": 600.0,
                "tt_value_ped": 2.0,
                "markup_percent": 110.0,
                "track_cost": true
            }
        });
        let resolved = consumable_profile_from_props(&props, None);
        assert!(!resolved.catalogue_effects);
        let profile = resolved.profile;
        assert_eq!(effects_reload_speed_percent(&profile.effects), 12.0);
        assert_eq!(profile.duration_seconds, 600.0);
        assert!((profile.dose_cost_ped() - 2.2).abs() < 1e-12);
    }

    #[test]
    fn tracking_off_records_the_dose_cost_but_books_none() {
        let props =
            json!({"entity": adrenaline(), "dose": {"markup_percent": 200.0, "track_cost": false}});
        let profile = consumable_profile_from_props(&props, None).profile;
        assert_eq!(profile.dose_cost_ped(), 6.0);
        assert_eq!(profile.booked_cost_ped(), 0.0);
    }

    #[test]
    fn only_reload_effects_in_percent_evaluate() {
        let effects = [
            DoseEffect::new("Reload Speed Increased", Some(10.0), Some("%")),
            DoseEffect::new("Reload Speed Decreased", Some(4.0), Some("%")),
            DoseEffect::new("Critical Chance Added", Some(1.0), Some("%")),
            DoseEffect::new("Reload Speed Increased", Some(3.0), Some("HP")),
            DoseEffect::new("Reload Speed Increased", None, Some("%")),
        ];
        assert_eq!(effects_reload_speed_percent(&effects), 6.0);
        assert_eq!(effects[2].kind, DoseEffectKind::Other);
    }

    #[test]
    fn a_healing_tool_grants_only_the_buffs_the_app_evaluates_on_use() {
        let eir = json!({
            "tool_entity": {
                "id": "217ec193a6ec",
                "name": "Eir Mk 1",
                "effects_on_use": [{
                    "name": "Reload Speed Increased",
                    "strength": 10,
                    "unit": "%",
                    "duration_seconds": 8
                }]
            }
        });
        let on_use = on_use_effect_from_props(&eir, None).expect("Eir grants reload speed");
        assert_eq!(on_use.duration_seconds, 8.0);
        assert_eq!(effects_reload_speed_percent(&on_use.effects), 10.0);

        let fap = json!({
            "tool_entity": {
                "name": "Vivo T10",
                "effects_on_use": [{"name": "Heal Over Time", "strength": 50, "unit": "%", "duration_seconds": 5}]
            }
        });
        assert_eq!(on_use_effect_from_props(&fap, None), None);
        assert_eq!(on_use_effect_from_props(&json!({}), None), None);
    }
}
