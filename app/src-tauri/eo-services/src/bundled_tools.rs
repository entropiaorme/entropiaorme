//! A held tool of the bundled game data, priced bare: what the tracker
//! declares when the tool read off the screen is not an entry of the
//! player's equipment library.
//!
//! The props are the ones the library's add path stores for the same pick
//! with nothing else chosen (no amplifier, no attachments, no implant, no
//! enhancers) and markup at the add path's default, so the tool prices
//! through the same cost engine and its band derives through the same
//! profile builders as a library entry would. A weapon's props are
//! prepared under the reload speed in effect, as a library weapon's are.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::attack_rate::{with_attack_rate, ReloadSpeedSource};
use crate::bus_events::HotbarItemKind;
use crate::equipment_pricing::{cost_per_shot_ped, heal_cost_from_props};
use crate::game_data_store::GameDataStore;
use crate::tracker::{damage_band_from_props, BundledTool, BundledToolResolver, BundledWeapon};

/// The markup an entry added to the library starts with, in percent.
const DEFAULT_MARKUP: i64 = 100;

/// The kind of held tool a bundled collection holds; None for a collection
/// of anything else.
pub fn kind_of(endpoint: &str) -> Option<HotbarItemKind> {
    match endpoint {
        "weapons" => Some(HotbarItemKind::Weapon),
        "medical_tools" => Some(HotbarItemKind::Healing),
        "harvesting_tools" => Some(HotbarItemKind::Harvesting),
        _ => None,
    }
}

/// The props the add path stores for `entity` of `endpoint` with nothing
/// else chosen; None for a collection holding no held tool.
pub fn bare_props(endpoint: &str, item_id: &str, entity: &Value) -> Option<Value> {
    let mut props = Map::new();
    match kind_of(endpoint)? {
        HotbarItemKind::Weapon => {
            props.insert("weapon_entity".into(), entity.clone());
            props.insert("weapon_catalog_id".into(), json!(item_id));
            props.insert("amp_entity".into(), Value::Null);
            props.insert("amp_catalog_id".into(), Value::Null);
            props.insert("scope_entity".into(), Value::Null);
            props.insert("scope_catalog_id".into(), Value::Null);
            props.insert("absorber_entity".into(), Value::Null);
            props.insert("absorber_catalog_id".into(), Value::Null);
            props.insert("weapon_markup".into(), json!(DEFAULT_MARKUP));
            props.insert("amp_markup".into(), json!(DEFAULT_MARKUP));
            props.insert("scope_markup".into(), json!(DEFAULT_MARKUP));
            props.insert("absorber_markup".into(), json!(DEFAULT_MARKUP));
            props.insert("damage_enhancers".into(), json!(0));
        }
        HotbarItemKind::Healing | HotbarItemKind::Harvesting => {
            props.insert("tool_entity".into(), entity.clone());
            props.insert("tool_catalog_id".into(), json!(item_id));
            props.insert("markup".into(), json!(DEFAULT_MARKUP));
        }
        HotbarItemKind::Consumable => return None,
    }
    Some(Value::Object(props))
}

/// A catalogue entity's name as a library entry and a tool read off the
/// screen both carry it: trimmed, None when blank. The one place either
/// takes a name from the game's data.
pub fn entity_name(entity: &Value) -> Option<String> {
    entity
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// `entity` of `endpoint`, priced bare under `reload_speed_percent`; None
/// for a collection holding no held tool or an entity without a name.
pub fn bare_tool(
    endpoint: &str,
    item_id: &str,
    entity: &Value,
    game_data: Option<&GameDataStore>,
    reload_speed_percent: f64,
) -> Option<BundledTool> {
    let name = entity_name(entity)?;
    let kind = kind_of(endpoint)?;
    let props = bare_props(endpoint, item_id, entity)?;
    match kind {
        HotbarItemKind::Weapon => {
            let props = with_attack_rate(&props, game_data, reload_speed_percent);
            Some(BundledTool {
                name,
                kind,
                cost_per_use_ped: cost_per_shot_ped(&props),
                weapon: Some(BundledWeapon {
                    band: damage_band_from_props(&props),
                    props: props.as_object().cloned().unwrap_or_default(),
                }),
            })
        }
        _ => Some(BundledTool {
            name,
            kind,
            cost_per_use_ped: heal_cost_from_props(&props.to_string()).0,
            weapon: None,
        }),
    }
}

/// The resolver the tracker is given: a tool of `game_data` by its
/// collection and id, priced bare under the reload speed in effect when it
/// is resolved.
pub fn resolver(
    game_data: Arc<GameDataStore>,
    reload_speed: ReloadSpeedSource,
) -> BundledToolResolver {
    Arc::new(move |endpoint: &str, item_id: &str| {
        let entity = game_data.find_entity(endpoint, &Value::String(item_id.to_string()))?;
        bare_tool(
            endpoint,
            item_id,
            entity,
            Some(game_data.as_ref()),
            reload_speed(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rifle() -> Value {
        json!({
            "id": 7,
            "name": "Sollomate Opalo",
            "damage": {"impact": 8.0},
            "economy": {"decay": 2.5, "ammo_burn": 0}
        })
    }

    #[test]
    fn a_bundled_weapon_is_priced_and_banded_bare() {
        let tool = bare_tool("weapons", "7", &rifle(), None, 0.0).expect("a weapon");
        assert_eq!(tool.name, "Sollomate Opalo");
        assert_eq!(tool.kind, HotbarItemKind::Weapon);
        let weapon = tool.weapon.expect("a weapon's band and props");
        // The same props priced through the cost engine directly: the
        // decay alone, in PED.
        let props = Value::Object(weapon.props.clone());
        assert!((tool.cost_per_use_ped - cost_per_shot_ped(&props)).abs() < 1e-12);
        assert!(tool.cost_per_use_ped > 0.0);
        assert_eq!(props["amp_entity"], Value::Null);
        assert_eq!(props["damage_enhancers"], json!(0));
        assert_eq!(props["weapon_markup"], json!(DEFAULT_MARKUP));
        let band = weapon.band.expect("a band from the catalogue damage");
        assert_eq!(band, damage_band_from_props(&props).unwrap());
        assert!(band.max >= 8.0 - 1e-9);
    }

    #[test]
    fn healing_and_harvesting_tools_carry_no_band() {
        let tool = json!({"id": 3, "name": "Vivo T1", "economy": {"decay": 1.0}});
        for (endpoint, kind) in [
            ("medical_tools", HotbarItemKind::Healing),
            ("harvesting_tools", HotbarItemKind::Harvesting),
        ] {
            let bare = bare_tool(endpoint, "3", &tool, None, 0.0).expect("a held tool");
            assert_eq!(bare.kind, kind);
            assert!(bare.weapon.is_none());
            let props = bare_props(endpoint, "3", &tool).unwrap();
            assert_eq!(
                bare.cost_per_use_ped,
                heal_cost_from_props(&props.to_string()).0
            );
            assert_eq!(props["markup"], json!(DEFAULT_MARKUP));
        }
    }

    #[test]
    fn other_collections_and_nameless_entities_are_no_tool() {
        assert!(bare_tool("stimulants", "1", &rifle(), None, 0.0).is_none());
        assert!(bare_tool("weapon_amplifiers", "1", &rifle(), None, 0.0).is_none());
        assert!(bare_tool("weapons", "7", &json!({"id": 7, "name": " "}), None, 0.0).is_none());
        assert_eq!(kind_of("absorbers"), None);
    }
}
