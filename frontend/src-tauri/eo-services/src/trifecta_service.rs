//! Trifecta resolution and validation, ported from
//! the original Python implementation: resolves a configured
//! small-weapon / big-weapon / healing-tool preset into tracking-ready
//! attribution data, validating the damage-band split.
//!
//! Result keys, numeric conversions, and every error string match the
//! backend character-for-character: the map folds into the tracking
//! snapshot's `trifectaAttribution` field downstream, and the error
//! strings reach the wire through validation responses.

use serde_json::{Map, Value};

use crate::cost_engine::{
    cost_per_shot_from_props, get_weapon_damage_profile, heal_cost_per_use,
    heal_range_at_max_skill, heal_reload_seconds,
};
use crate::db::{Db, DbError};

/// The configured trifecta preset ids, mirroring the backend preset row's
/// three foreign keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrifectaPreset {
    pub small_weapon_id: Option<i64>,
    pub big_weapon_id: Option<i64>,
    pub heal_id: Option<i64>,
}

fn ranges_overlap(first_min: f64, first_max: f64, second_min: f64, second_max: f64) -> bool {
    first_min.max(second_min) <= first_max.min(second_max)
}

/// Python's `f"{value:.1f}"`: fixed one-decimal formatting (round half to
/// even at the formatting boundary, exactly as Rust's `{:.1}` formats).
fn format_range(minimum: f64, maximum: f64) -> String {
    format!("{minimum:.1}-{maximum:.1}")
}

/// Resolve a trifecta preset into tracking-ready data plus validation.
/// `Ok((None, Some(reason)))` mirrors the backend's `(None, error)`
/// shape; database errors surface separately as `Err`.
pub async fn describe_trifecta(
    db: &Db,
    preset: Option<&TrifectaPreset>,
) -> Result<(Option<Map<String, Value>>, Option<String>), DbError> {
    let Some(preset) = preset else {
        return Ok((
            None,
            Some("Trifecta attribution requires an active preset".to_string()),
        ));
    };
    let (Some(small_id), Some(big_id), Some(heal_id)) =
        (preset.small_weapon_id, preset.big_weapon_id, preset.heal_id)
    else {
        return Ok((
            None,
            Some(
                "Trifecta attribution requires a configured small weapon, big weapon, and healing tool"
                    .to_string(),
            ),
        ));
    };

    let mut result = Map::new();

    for (key, label, id) in [
        ("small_weapon", "small weapon", small_id),
        ("big_weapon", "big weapon", big_id),
    ] {
        let Some((row_id, name, properties_json)) = db.equipment_item(id, "weapon").await? else {
            return Ok((
                None,
                Some(format!(
                    "Trifecta attribution {label} is not found in the equipment library"
                )),
            ));
        };

        let props: Value = serde_json::from_str(&properties_json)
            .map_err(|e| DbError::Driver(format!("equipment properties parse: {e}")))?;
        // `max(0, int(configured or 0))`, the same coercion the cost
        // engine applies (JSON ints and floats both truncate).
        let damage_enhancers = (props
            .get("damage_enhancers")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64)
            .max(0);
        // `props.get("amp_entity")` in the backend yields None for an
        // absent OR null key; filter the JSON null to match.
        let damage_profile = get_weapon_damage_profile(
            props
                .get("weapon_entity")
                .expect("weapon rows carry weapon_entity"),
            props.get("amp_entity").filter(|v| !v.is_null()),
            damage_enhancers,
        );
        let Some(damage_profile) = damage_profile else {
            return Ok((
                None,
                Some(format!(
                    "Trifecta attribution {label} does not expose a usable damage range"
                )),
            ));
        };

        let cost_result = cost_per_shot_from_props(&props, None);
        let mut entry = Map::new();
        entry.insert("id".into(), Value::from(row_id));
        entry.insert("name".into(), Value::from(name));
        entry.insert("role".into(), Value::from(key));
        entry.insert(
            "cost_per_shot_ped".into(),
            Value::from(
                cost_result["totalCostPerUse"]
                    .as_f64()
                    .expect("cost result carries totalCostPerUse")
                    / 100.0,
            ),
        );
        entry.insert("damage_min".into(), damage_profile["damageMin"].clone());
        entry.insert("damage_max".into(), damage_profile["damageMax"].clone());
        entry.insert("total_damage".into(), damage_profile["totalDamage"].clone());
        entry.insert("weapon_props".into(), props);
        result.insert(key.to_string(), Value::Object(entry));
    }

    let small = &result["small_weapon"];
    let big = &result["big_weapon"];
    let (small_min, small_max) = (
        small["damage_min"].as_f64().expect("numeric damage_min"),
        small["damage_max"].as_f64().expect("numeric damage_max"),
    );
    let (big_min, big_max) = (
        big["damage_min"].as_f64().expect("numeric damage_min"),
        big["damage_max"].as_f64().expect("numeric damage_max"),
    );
    if ranges_overlap(small_min, small_max, big_min, big_max) {
        let small_name = small["name"].as_str().expect("string name");
        let big_name = big["name"].as_str().expect("string name");
        return Ok((
            None,
            Some(format!(
                "Trifecta attribution requires non-overlapping small/big weapon ranges \
                 ({small_name}: {}, {big_name}: {})",
                format_range(small_min, small_max),
                format_range(big_min, big_max),
            )),
        ));
    }

    let Some((heal_row_id, heal_name, heal_properties_json)) =
        db.equipment_item(heal_id, "healing").await?
    else {
        return Ok((
            None,
            Some(
                "Trifecta attribution healing tool is not found in the equipment library"
                    .to_string(),
            ),
        ));
    };

    let heal_props: Value = serde_json::from_str(&heal_properties_json)
        .map_err(|e| DbError::Driver(format!("equipment properties parse: {e}")))?;
    let markup = heal_props
        .get("markup")
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        / 100.0;
    let tool = heal_props
        .get("tool_entity")
        .filter(|v| !v.is_null())
        .expect("healing rows carry tool_entity");
    let heal_interval = heal_range_at_max_skill(tool);
    let mut heal_entry = Map::new();
    heal_entry.insert("id".into(), Value::from(heal_row_id));
    heal_entry.insert("name".into(), Value::from(heal_name));
    heal_entry.insert(
        "cost_per_use_ped".into(),
        Value::from(heal_cost_per_use(tool, markup) / 100.0),
    );
    heal_entry.insert(
        "reload_seconds".into(),
        Value::from(heal_reload_seconds(tool)),
    );
    let (heal_min, heal_max) = match &heal_interval {
        Some(interval) => (interval["min"].clone(), interval["max"].clone()),
        None => (Value::Null, Value::Null),
    };
    heal_entry.insert("heal_min".into(), heal_min);
    heal_entry.insert("heal_max".into(), heal_max);
    result.insert("heal_tool".to_string(), Value::Object(heal_entry));

    Ok((Some(result), None))
}

/// The boolean validation view over [`describe_trifecta`].
pub async fn validate_trifecta(
    db: &Db,
    preset: Option<&TrifectaPreset>,
) -> Result<(bool, Option<String>), DbError> {
    let (trifecta, error) = describe_trifecta(db, preset).await?;
    Ok((trifecta.is_some(), error))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    async fn seeded_db(dir: &std::path::Path, rows: &[(i64, &str, &str, Value)]) -> Db {
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        for (id, name, item_type, props) in rows {
            db.insert_equipment_for_tests(*id, name, item_type, &props.to_string())
                .await
                .unwrap();
        }
        db
    }

    fn weapon_props(damage: f64, decay: f64) -> Value {
        json!({
            "weapon_entity": {
                "damage": {"impact": damage},
                "economy": {"decay": decay, "ammo_burn": 100},
            },
        })
    }

    fn heal_props() -> Value {
        json!({
            "markup": 110,
            "tool_entity": {
                "min_heal": 12.0,
                "max_heal": 45.0,
                "uses_per_minute": 30,
                "economy": {"decay": 0.08, "ammo_burn": 0},
            },
        })
    }

    fn full_preset() -> TrifectaPreset {
        TrifectaPreset {
            small_weapon_id: Some(1),
            big_weapon_id: Some(2),
            heal_id: Some(3),
        }
    }

    #[tokio::test]
    async fn missing_preset_and_missing_ids_return_the_exact_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path(), &[]).await;

        let (data, error) = describe_trifecta(&db, None).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some("Trifecta attribution requires an active preset")
        );

        let partial = TrifectaPreset {
            small_weapon_id: Some(1),
            big_weapon_id: None,
            heal_id: Some(3),
        };
        let (data, error) = describe_trifecta(&db, Some(&partial)).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some("Trifecta attribution requires a configured small weapon, big weapon, and healing tool")
        );
    }

    #[tokio::test]
    async fn unknown_rows_and_unusable_ranges_return_the_exact_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(10.0, 0.05)),
                // id 2 absent; id 4 carries no damage.
                (
                    4,
                    "Broken",
                    "weapon",
                    json!({"weapon_entity": {"economy": {"decay": 0.1}}}),
                ),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;

        let (data, error) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some("Trifecta attribution big weapon is not found in the equipment library")
        );

        let broken_big = TrifectaPreset {
            small_weapon_id: Some(1),
            big_weapon_id: Some(4),
            heal_id: Some(3),
        };
        let (data, error) = describe_trifecta(&db, Some(&broken_big)).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some("Trifecta attribution big weapon does not expose a usable damage range")
        );
    }

    #[tokio::test]
    async fn overlapping_ranges_format_the_exact_error_string() {
        let dir = tempfile::tempdir().unwrap();
        // damage 10 -> range 5.0-10.0; damage 16 -> range 8.0-16.0: overlap.
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(10.0, 0.05)),
                (2, "Rifle", "weapon", weapon_props(16.0, 0.2)),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;

        let (data, error) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some(
                "Trifecta attribution requires non-overlapping small/big weapon ranges \
                 (Pistol: 5.0-10.0, Rifle: 8.0-16.0)"
            )
        );
    }

    #[tokio::test]
    async fn touching_ranges_overlap_and_ties_format_half_even() {
        let dir = tempfile::tempdir().unwrap();
        // damage 14.5 -> range 7.25-14.5 (the .25 tie formats half-even
        // to "7.2"); damage 29 -> range 14.5-29.0: touching ranges count
        // as overlapping on both implementations.
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(14.5, 0.05)),
                (2, "Rifle", "weapon", weapon_props(29.0, 0.2)),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;

        let (data, error) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert!(data.is_none());
        assert_eq!(
            error.as_deref(),
            Some(
                "Trifecta attribution requires non-overlapping small/big weapon ranges \
                 (Pistol: 7.2-14.5, Rifle: 14.5-29.0)"
            )
        );
    }

    #[tokio::test]
    async fn happy_path_resolves_the_full_attribution_map() {
        let dir = tempfile::tempdir().unwrap();
        // damage 10 -> 5..10; damage 30 -> 15..30: disjoint.
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(10.0, 0.05)),
                (2, "Cannon", "weapon", weapon_props(30.0, 0.2)),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;

        let (data, error) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert_eq!(error, None);
        let data = data.expect("resolved attribution");

        let small = &data["small_weapon"];
        assert_eq!(small["id"], 1);
        assert_eq!(small["name"], "Pistol");
        assert_eq!(small["role"], "small_weapon");
        assert_eq!(small["damage_min"], 5.0);
        assert_eq!(small["damage_max"], 10.0);
        assert_eq!(small["total_damage"], 10.0);
        assert!(small["weapon_props"]["weapon_entity"].is_object());
        let expected_cost = cost_per_shot_from_props(&weapon_props(10.0, 0.05), None)
            ["totalCostPerUse"]
            .as_f64()
            .unwrap()
            / 100.0;
        assert_eq!(small["cost_per_shot_ped"].as_f64().unwrap(), expected_cost);

        let heal = &data["heal_tool"];
        assert_eq!(heal["id"], 3);
        assert_eq!(heal["name"], "Healer");
        assert_eq!(heal["heal_min"], 12.0);
        assert_eq!(heal["heal_max"], 45.0);
        assert_eq!(heal["reload_seconds"], 2.0);
        let expected_heal_cost = heal_cost_per_use(&heal_props()["tool_entity"], 1.1) / 100.0;
        assert_eq!(
            heal["cost_per_use_ped"].as_f64().unwrap(),
            expected_heal_cost
        );

        let (ok, error) = validate_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert!(ok);
        assert_eq!(error, None);
    }

    #[tokio::test]
    async fn heal_without_published_interval_yields_nulls() {
        let dir = tempfile::tempdir().unwrap();
        let mut props = heal_props();
        props["tool_entity"]
            .as_object_mut()
            .unwrap()
            .remove("min_heal");
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(10.0, 0.05)),
                (2, "Cannon", "weapon", weapon_props(30.0, 0.2)),
                (3, "Healer", "healing", props),
            ],
        )
        .await;
        let (data, _) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        let heal = &data.unwrap()["heal_tool"];
        assert_eq!(heal["heal_min"], Value::Null);
        assert_eq!(heal["heal_max"], Value::Null);
    }

    #[tokio::test]
    async fn amp_entities_change_the_damage_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut amped = weapon_props(10.0, 0.05);
        amped["amp_entity"] = json!({"damage": {"burn": 4.0}});
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", amped),
                (2, "Cannon", "weapon", weapon_props(40.0, 0.2)),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;
        let (data, error) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        assert_eq!(error, None);
        let small = &data.unwrap()["small_weapon"];
        // 10 + min(5, 4) = 14: the amp genuinely participates.
        assert_eq!(small["total_damage"], 14.0);
        assert_eq!(small["damage_min"], 7.0);
        assert_eq!(small["damage_max"], 14.0);
    }

    #[tokio::test]
    async fn validation_reports_failures_with_their_reason() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path(), &[]).await;
        let (ok, error) = validate_trifecta(&db, None).await.unwrap();
        assert!(!ok);
        assert_eq!(
            error.as_deref(),
            Some("Trifecta attribution requires an active preset")
        );
    }

    #[tokio::test]
    async fn happy_path_heal_figures_match_hand_computed_values() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(
            dir.path(),
            &[
                (1, "Pistol", "weapon", weapon_props(10.0, 0.05)),
                (2, "Cannon", "weapon", weapon_props(30.0, 0.2)),
                (3, "Healer", "healing", heal_props()),
            ],
        )
        .await;
        let (data, _) = describe_trifecta(&db, Some(&full_preset())).await.unwrap();
        let data = data.unwrap();
        // decay 0.08 PEC at markup 1.1 -> 0.088 PEC, divided (unrounded,
        // exactly as the backend does) into PED.
        assert_eq!(
            data["heal_tool"]["cost_per_use_ped"].as_f64().unwrap(),
            0.088 / 100.0
        );
        // weapon decay 0.05 + ammo 1.0 PEC -> 1.05 PEC -> 0.0105 PED.
        assert_eq!(
            data["small_weapon"]["cost_per_shot_ped"].as_f64().unwrap(),
            0.0105
        );
    }
}
