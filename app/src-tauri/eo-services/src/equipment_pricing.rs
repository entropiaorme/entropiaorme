//! Equipment pricing lookups over the equipment library: the per-shot
//! weapon cost, the healing-tool per-use cost and reload, and the
//! synchronous row lookups the input listeners resolve tools through.
//!
//! The composition root wires these into the hotbar resolver and the
//! tracker's equipment seam; the arithmetic composes [`crate::cost_engine`]
//! so the figures stay bit-identical to the frozen goldens that pin them
//! downstream.

use serde_json::Value;

use crate::game_data_store::GameDataStore;

use crate::cost_engine::{
    cost_per_shot_from_props, heal_cost_per_use_with_implant, heal_reload_seconds,
};
use crate::db::DbError;
use crate::healing_profile::{HealingMode, HealingProfile};

/// The per-shot cost in PED derived from a weapon's properties:
/// `totalCostPerUse / 100`, or 0 when the profile carries no figure.
pub fn cost_per_shot_ped(props: &Value) -> f64 {
    cost_per_shot_from_props(props, None)["totalCostPerUse"]
        .as_f64()
        .unwrap_or(0.0)
        / 100.0
}

/// The per-shot weapon cost in PED for a tool by name fragment
/// (`totalCostPerUse / 100`), its stored props prepared by `pricing` (the
/// attack rate in force, see [`crate::attack_rate`]), or 0 when the tool is
/// unknown. A read failure degrades to 0 (the tool-unknown value), so a
/// weapon slot still resolves rather than dropping the tool change.
pub fn weapon_cost_by_name(
    conn: &rusqlite::Connection,
    name: &str,
    pricing: &dyn Fn(&Value) -> Value,
) -> f64 {
    match weapon_properties_by_name_fragment_sync(conn, name)
        .ok()
        .flatten()
    {
        Some(properties_json) => {
            let props: Value = serde_json::from_str(&properties_json).unwrap_or(Value::Null);
            cost_per_shot_ped(&pricing(&props))
        }
        None => 0.0,
    }
}

/// The healing-tool per-use cost (PED) and reload (seconds) from a row's
/// properties: a missing or empty tool entity falls back to `(0, 2.5)`;
/// otherwise the cost engine's per-use cost over the entity and its
/// markup, in PED, and the entity's reload.
pub fn heal_cost_from_props(properties_json: &str) -> (f64, f64) {
    let props: Value = serde_json::from_str(properties_json).unwrap_or(Value::Null);
    let tool = props
        .get("tool_entity")
        .filter(|value| !value.is_null() && value.as_object().is_none_or(|map| !map.is_empty()));
    let Some(tool) = tool else {
        return (0.0, 2.5);
    };
    let markup = props.get("markup").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
    let implant = props.get("implant_entity").filter(|value| !value.is_null());
    let implant_markup = props
        .get("implant_markup")
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        / 100.0;
    (
        heal_cost_per_use_with_implant(tool, markup, implant, implant_markup) / 100.0,
        heal_reload_seconds(tool),
    )
}

/// The activation/output profile stored with a healing setup. Existing rows
/// predate explicit effect configuration, so their catalogue direct interval
/// is the lossless default.
pub fn healing_profile_from_props(properties_json: &str) -> HealingProfile {
    let props: Value = serde_json::from_str(properties_json).unwrap_or(Value::Null);
    let tool = props.get("tool_entity").unwrap_or(&Value::Null);
    let configured = props.get("healing_profile");
    let number = |key: &str| {
        configured
            .and_then(|value| value.get(key))
            .and_then(Value::as_f64)
    };
    let mode = configured
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .and_then(|value| match value {
            "direct" => Some(HealingMode::Direct),
            "over_time" => Some(HealingMode::OverTime),
            "compound" => Some(HealingMode::Compound),
            _ => None,
        })
        .unwrap_or_default();
    HealingProfile {
        mode,
        direct_min: number("direct_min").or_else(|| tool.get("min_heal").and_then(Value::as_f64)),
        direct_max: number("direct_max").or_else(|| tool.get("max_heal").and_then(Value::as_f64)),
        effect_duration_seconds: number("effect_duration_seconds"),
        tick_min: number("tick_min"),
        tick_max: number("tick_max"),
        tick_seconds: number("tick_seconds"),
        ..HealingProfile::default()
    }
}

/// Sum the known passive lifesteal contribution carried by a weapon setup's
/// base item and amplifier. The bundled catalogue normalises the upstream
/// effect vocabulary to one percentage field; absent data is deliberately
/// `None`, because safe billing never depends on knowing the passive rate.
pub fn lifesteal_percent_from_props(properties_json: &str) -> Option<f64> {
    let props: Value = serde_json::from_str(properties_json).ok()?;
    let total: f64 = ["weapon_entity", "amp_entity"]
        .into_iter()
        .filter_map(|key| props.get(key))
        .filter_map(|entity| entity.get("lifesteal_percent").and_then(Value::as_f64))
        .filter(|value| value.is_finite() && *value > 0.0)
        .sum();
    (total > 0.0).then_some(total)
}

/// Resolve lifesteal from a stored weapon setup, falling back to the current
/// bundled catalogue for components saved before lifesteal was normalised into
/// the snapshot. The fallback is read-only: stored costing inputs remain the
/// immutable setup the user saved, while descriptive effect metadata can gain
/// fidelity as the bundled catalogue does.
pub fn lifesteal_percent_from_props_with_catalog(
    properties_json: &str,
    game_data: &GameDataStore,
) -> Option<f64> {
    let props: Value = serde_json::from_str(properties_json).ok()?;
    let components = [
        ("weapon_entity", "weapon_catalog_id", "weapons"),
        ("amp_entity", "amp_catalog_id", "weapon_amplifiers"),
    ];
    let total: f64 = components
        .into_iter()
        .filter_map(|(entity_key, id_key, endpoint)| {
            props
                .get(entity_key)
                .and_then(|entity| entity.get("lifesteal_percent"))
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)
                .or_else(|| {
                    props
                        .get(id_key)
                        .filter(|id| !id.is_null())
                        .and_then(|id| game_data.find_entity(endpoint, id))
                        .and_then(|entity| entity.get("lifesteal_percent"))
                        .and_then(Value::as_f64)
                        .filter(|value| value.is_finite() && *value > 0.0)
                })
        })
        .sum();
    (total > 0.0).then_some(total)
}

/// One equipment-library row by id: `(name, item_type, properties JSON)`,
/// or None when absent. The synchronous reader-core counterpart of
/// [`crate::db::Db::hotbar_equipment_row`], for callers on a plain OS
/// thread (the hotbar listener's key thread).
pub fn hotbar_equipment_row_sync(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<(String, String, String)>, DbError> {
    use rusqlite::OptionalExtension as _;
    let row = conn
        .query_row(
            "SELECT name, item_type, properties_json FROM equipment_library \
             WHERE id = ?",
            rusqlite::params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// The first weapon-row `properties_json` whose name contains the
/// fragment, the synchronous reader-core counterpart of
/// [`crate::db::Db::weapon_properties_by_name_fragment`]: a
/// `LIKE '%fragment%'` over weapon rows with the fragment's own
/// `%` / `_` / `\` escaped under an explicit `ESCAPE '\'`, and the
/// fragment trimmed exactly as the async path trims it.
pub fn weapon_properties_by_name_fragment_sync(
    conn: &rusqlite::Connection,
    fragment: &str,
) -> Result<Option<String>, DbError> {
    use rusqlite::OptionalExtension as _;
    let safe = fragment
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let row = conn
        .query_row(
            "SELECT properties_json FROM equipment_library \
             WHERE item_type = 'weapon' AND name LIKE ? ESCAPE '\\'",
            rusqlite::params![format!("%{safe}%")],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use serde_json::json;

    async fn open() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    async fn seed(db: &Db, sql: &str) {
        let sql = sql.to_string();
        db.with_writer(move |conn| {
            conn.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn cost_per_shot_ped_divides_total_cost_per_use_by_a_hundred() {
        // decay 0.05 @ 1.0 + ammo (200 pec / 100) 2.0 @ 1.0 = 2.05 per use;
        // the per-shot PED figure is that over 100.
        let props = json!({"weapon_entity": {"economy": {"decay": 0.05, "ammo_burn": 200}}});
        assert_eq!(cost_per_shot_ped(&props), 2.05 / 100.0);
    }

    #[test]
    #[should_panic(expected = "weapon_entity")]
    fn cost_per_shot_ped_panics_on_a_missing_weapon_entity() {
        // Current behaviour: a properties payload with no weapon_entity reaches
        // the cost engine's fail-fast. Pinned as-is (see the inbox note).
        let _ = cost_per_shot_ped(&json!({}));
    }

    #[test]
    fn heal_cost_from_props_uses_the_tool_economy_and_markup() {
        // (decay 0.0512 + ammo 0.3) * markup 1.1 = 0.38632 -> 0.3863 pec,
        // over 100 for PED; reload 60 / 30 uses = 2.0s.
        let props = json!({
            "tool_entity": {"economy": {"decay": 0.0512, "ammo_burn": 30}, "uses_per_minute": 30},
            "markup": 110,
        });
        assert_eq!(
            heal_cost_from_props(&props.to_string()),
            (0.3863 / 100.0, 2.0)
        );
    }

    #[test]
    fn heal_cost_from_props_falls_back_for_a_degenerate_tool() {
        // Missing, null and empty tool entities all fall back to (0, 2.5); a
        // kept degenerate tool would compute to the same pair, so the filter's
        // guards are behaviour-equivalent here.
        assert_eq!(heal_cost_from_props("{}"), (0.0, 2.5));
        assert_eq!(
            heal_cost_from_props(&json!({"tool_entity": null}).to_string()),
            (0.0, 2.5)
        );
        assert_eq!(
            heal_cost_from_props(&json!({"tool_entity": {}}).to_string()),
            (0.0, 2.5)
        );
        // Unparseable JSON degrades to the same fallback.
        assert_eq!(heal_cost_from_props("not json"), (0.0, 2.5));
    }

    #[test]
    fn lifesteal_catalogue_fallback_enriches_an_existing_saved_setup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("weapons.json"),
            r#"[{"id":"chip-15","name":"Chip","lifesteal_percent":3.0}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("weapon_amplifiers.json"),
            r#"[{"id":"delta","name":"Delta","lifesteal_percent":2.0}]"#,
        )
        .unwrap();
        let catalogue = GameDataStore::new(dir.path()).unwrap();
        let legacy_props = json!({
            "weapon_catalog_id": "chip-15",
            "weapon_entity": {"id": "chip-15", "name": "Chip"},
            "amp_catalog_id": "delta",
            "amp_entity": {"id": "delta", "name": "Delta"}
        });

        assert_eq!(
            lifesteal_percent_from_props_with_catalog(&legacy_props.to_string(), &catalogue),
            Some(5.0)
        );

        let stored_override = json!({
            "weapon_catalog_id": "chip-15",
            "weapon_entity": {"id": "chip-15", "lifesteal_percent": 1.0},
            "amp_catalog_id": "delta",
            "amp_entity": {"id": "delta"}
        });
        assert_eq!(
            lifesteal_percent_from_props_with_catalog(&stored_override.to_string(), &catalogue),
            Some(3.0)
        );

        let stale_values = json!({
            "weapon_catalog_id": "chip-15",
            "weapon_entity": {"id": "chip-15", "lifesteal_percent": 0.0},
            "amp_catalog_id": "delta",
            "amp_entity": {"id": "delta", "lifesteal_percent": -2.0}
        });
        assert_eq!(
            lifesteal_percent_from_props_with_catalog(&stale_values.to_string(), &catalogue),
            Some(5.0)
        );
        assert_eq!(
            lifesteal_percent_from_props(&stale_values.to_string()),
            None
        );
    }

    #[tokio::test]
    async fn weapon_cost_by_name_prices_a_seeded_weapon_and_zeroes_the_unknown() {
        let (_dir, db) = open().await;
        seed(
            &db,
            "INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES \
             (1, 'Opalo Rifle', 'weapon', \
              '{\"weapon_entity\":{\"economy\":{\"decay\":0.05,\"ammo_burn\":200}}}')",
        )
        .await;

        let cost = db
            .with_reader(|conn| Ok(weapon_cost_by_name(conn, "Opalo", &Value::clone)))
            .await
            .unwrap();
        assert_eq!(cost, 2.05 / 100.0);

        // Pricing sees the props after preparation: a weapon held at the
        // attack-rate limit costs its factor more per attack.
        let limited = db
            .with_reader(|conn| {
                Ok(weapon_cost_by_name(conn, "Opalo", &|props| {
                    let mut props = props.clone();
                    props["weapon_entity"]["uses_per_minute"] = serde_json::json!(90);
                    crate::attack_rate::with_attack_rate(&props, None, 30.0)
                }))
            })
            .await
            .unwrap();
        assert!((limited - 2.05 * 1.17 / 100.0).abs() < 1e-9);

        // An unknown tool prices to zero rather than dropping the tool change.
        let unknown = db
            .with_reader(|conn| Ok(weapon_cost_by_name(conn, "Nonexistent", &Value::clone)))
            .await
            .unwrap();
        assert_eq!(unknown, 0.0);
    }

    #[tokio::test]
    async fn hotbar_equipment_row_sync_reads_one_row_by_id() {
        let (_dir, db) = open().await;
        seed(
            &db,
            "INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES \
             (7, 'Opalo', 'weapon', '{\"x\":1}')",
        )
        .await;

        let row = db
            .with_reader(|conn| hotbar_equipment_row_sync(conn, 7))
            .await
            .unwrap();
        assert_eq!(
            row,
            Some(("Opalo".into(), "weapon".into(), "{\"x\":1}".into()))
        );

        let missing = db
            .with_reader(|conn| hotbar_equipment_row_sync(conn, 999))
            .await
            .unwrap();
        assert_eq!(missing, None);
    }

    #[tokio::test]
    async fn weapon_properties_by_name_fragment_sync_matches_the_first_weapon() {
        let (_dir, db) = open().await;
        seed(
            &db,
            "INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES \
             (1, 'Opalo Mk2', 'weapon', '{\"tag\":\"o\"}'), \
             (2, 'Longsword', 'melee', '{\"tag\":\"s\"}')",
        )
        .await;

        let hit = db
            .with_reader(|conn| weapon_properties_by_name_fragment_sync(conn, "Opalo"))
            .await
            .unwrap();
        assert_eq!(hit, Some("{\"tag\":\"o\"}".into()));

        // The item_type gate keeps a same-named non-weapon out.
        let melee = db
            .with_reader(|conn| weapon_properties_by_name_fragment_sync(conn, "Longsword"))
            .await
            .unwrap();
        assert_eq!(melee, None);

        // An absent fragment matches nothing.
        let miss = db
            .with_reader(|conn| weapon_properties_by_name_fragment_sync(conn, "Zzz"))
            .await
            .unwrap();
        assert_eq!(miss, None);
    }
}
