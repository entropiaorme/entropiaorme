//! Behavioural pins for the equipment family over the typed facade: the
//! search gates, the catalogue-less validation ladder, the
//! custom-consumable write cycle, the type-change and missing-row
//! refusals, the unguarded delete, and the rebuild of the tools the
//! held-tool watcher matches against after every edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::equipment::{
    EquipmentKind, EquipmentRequest, EquipmentSearchHit, ExpectedLooterSource, SearchKind,
};
use eo_api::{Api, ApiError};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::Value;

use crate::common;

/// A minimal catalogue snapshot: two weapons (a limited one, so the
/// `(L)` flag pins, and a fast one with a catalogue attack rate), one
/// stimulant, one Mindforce implant, one
/// absorber/extender, and the three hunting looters, each a hundredth of
/// its own skill.
fn write_snapshot(dir: &Path) {
    std::fs::write(
        dir.join("weapons.json"),
        r#"[{"id": "w1", "name": "Opalo Rifle (L)", "economy": {"decay": 0.5, "ammo_burn": 300, "efficiency": 56.7}},
            {"id": "w2", "name": "Rapid Carbine", "uses_per_minute": 90, "economy": {"decay": 1.0, "ammo_burn": 100, "efficiency": 60.0}, "damage": {"impact": 40}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("stimulants.json"),
        r#"[{"id": "s1", "name": "Vita Bar", "economy": {}},
            {"id": "s2", "name": "Nanobots - Adrenaline Boost", "economy": {"max_tt": 3},
             "effects": [{"name": "Reload Speed Increased", "strength": 20, "unit": "%", "duration_seconds": 3600}]}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("medical_tools.json"),
        r#"[{"id": "h1", "name": "Restoration Chip", "uses_per_minute": 60, "mindforce": {"cooldown": 3.75}, "min_heal": 25, "max_heal": 30, "economy": {"decay": 1}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("weapon_amplifiers.json"),
        r#"[{"id": "a1", "name": "Mayhem MF-Amplifier Delta (L)", "lifesteal_percent": 2.0, "economy": {"decay": 0.1, "ammo_burn": 0, "efficiency": 75.0}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("mindforce_implants.json"),
        r#"[{"id": "i1", "name": "NeoPsion 85-B Mindforce Implant (L)", "economy": {"decay": null, "absorption": 0.2, "max_tt": 188}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("absorbers.json"),
        r#"[{"id": "x1", "name": "ArMatrix Extender P20 (L)", "economy": {"decay": null, "absorption": 0.2}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("professions.json"),
        r#"[{"name": "Animal Looter", "skills": [{"weight": 100, "skill": {"name": "Animal Lore"}}]},
            {"name": "Mutant Looter", "skills": [{"weight": 100, "skill": {"name": "Mutant Lore"}}]},
            {"name": "Robot Looter", "skills": [{"weight": 100, "skill": {"name": "Robot Lore"}}]}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("skills.json"),
        r#"[{"name": "Animal Lore"}, {"name": "Mutant Lore"}, {"name": "Robot Lore"}]"#,
    )
    .unwrap();
}

/// Scanned skills putting the Animal, Mutant, and Robot Looter professions
/// at the given levels.
async fn seed_looters(db: &Db, animal: f64, mutant: f64, robot: f64) {
    db.with_writer(move |conn| {
        for (skill, level) in [
            ("Animal Lore", animal),
            ("Mutant Lore", mutant),
            ("Robot Lore", robot),
        ] {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?1, ?2, 'scan', 1700000000.0)",
                rusqlite::params![skill, level * 100.0],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// The composed facade over a fresh migrated database and the test
/// snapshot, plus a database handle of its own for storage assertions.
async fn api_over(dir: &Path) -> (Api, Db) {
    api_over_settings(dir, None).await
}

/// [`api_over`] with a settings file in place before the services start.
async fn api_over_settings(dir: &Path, settings: Option<&str>) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    write_snapshot(&snapshot);
    let data_dir: PathBuf = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    if let Some(settings) = settings {
        std::fs::write(data_dir.join("settings.json"), settings).unwrap();
    }
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("snapshot store"));
    let clock = Arc::new(eo_services::clock::RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    (
        Api::new(
            db.clone(),
            game_data,
            clock,
            data_dir,
            handles.config_service,
            handles.tracker,
            handles.hotbar,
            handles.watcher,
            handles.skill_tracker,
            handles.skill_scan,
            handles.spacebar,
            handles.repair_ocr,
            handles.sale_capture,
            handles.quests.clone(),
            None,
            None,
            None,
        ),
        db,
    )
}

fn consumable(name: &str) -> EquipmentRequest {
    EquipmentRequest {
        kind: EquipmentKind::Consumable,
        catalog_id: None,
        name: Some(name.to_string()),
        amp_catalog_id: None,
        scope_catalog_id: None,
        absorber_catalog_id: None,
        weapon_markup: 100,
        amp_markup: 100,
        scope_markup: 100,
        absorber_markup: 100,
        damage_enhancers: 0,
        implant_catalog_id: None,
        implant_markup: 100,
        healing_mode: Default::default(),
        heal_min: None,
        heal_max: None,
        effect_duration_seconds: None,
        tick_min: None,
        tick_max: None,
        tick_seconds: None,
        weapon_effect: None,
        dose: None,
    }
}

/// The named entry of one vocabulary's catalogue.
async fn catalogue_entry(api: &Api, kind: SearchKind, name: &str) -> EquipmentSearchHit {
    api.equipment_catalogue(kind)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is in the catalogue"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_catalogue_lists_one_vocabulary_with_its_economy() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    // Every entry of the vocabulary, in catalogue order.
    let weapons = api.equipment_catalogue(SearchKind::Weapon).await.unwrap();
    let names: Vec<&str> = weapons.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["Opalo Rifle (L)", "Rapid Carbine"]);

    // An entry carries the catalogue economy in the response shape.
    let entry = &weapons[0];
    assert_eq!(entry.catalog_id.as_deref(), Some("w1"));
    assert_eq!(entry.decay, 0.5);
    assert_eq!(entry.ammo_burn, 3.0);
    assert!(entry.is_limited);

    // Each vocabulary reads its own endpoint.
    let healers = api.equipment_catalogue(SearchKind::Healer).await.unwrap();
    assert_eq!(healers.len(), 1);
    assert_eq!(healers[0].name, "Restoration Chip");
    assert_eq!(healers[0].reload_seconds, Some(3.75));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_validation_ladder_matches_the_route_replies() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    // Weapon without a catalogue id.
    let mut req = consumable("x");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    assert_eq!(
        api.equipment_add(&req).await.unwrap_err(),
        ApiError::bad_request("catalog_id required for weapon"),
    );

    // A catalogue miss names the endpoint, exactly as before.
    req.catalog_id = Some("nope".to_string());
    assert_eq!(
        api.equipment_add(&req).await.unwrap_err(),
        ApiError::not_found("Entity 'nope' not found in catalogue endpoint 'weapons'."),
    );

    // A consumable needs an identity.
    let mut req = consumable("x");
    req.name = None;
    assert_eq!(
        api.equipment_add(&req).await.unwrap_err(),
        ApiError::bad_request(
            "Consumable requires either catalog_id (catalogue pick) or name (custom)"
        ),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_custom_consumable_cycle_matches_the_http_era_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;

    // The library starts empty.
    assert!(api.equipment_library().await.unwrap().entries.is_empty());

    // The stored name is stripped the reference way.
    let added = api
        .equipment_add(&consumable("  Nutrio Bar  "))
        .await
        .unwrap();
    assert_eq!(added.id, "1");
    assert_eq!(added.name, "Nutrio Bar");

    // The added consumable carries its defaults: no amplifier, damage,
    // or reload figures; not limited; base enrichment; no healing or
    // effect profile.
    assert_eq!(added.kind, EquipmentKind::Consumable);
    assert!(added.amplifier_name.is_none());
    assert_eq!(added.cost_per_use, 0.0);
    assert!(added.damage_min.is_none());
    assert!(added.damage_max.is_none());
    assert!(added.reload_seconds.is_none());
    assert!(!added.is_limited);
    assert_eq!(added.enrichment_level, 1);
    assert!(added.healing_profile.is_none());
    assert!(added.lifesteal_percent.is_none());
    assert!(added.effect_profile.is_none());
    let dose = added.consumable.as_ref().expect("consumable dose");
    assert_eq!(dose.duration_seconds, 0.0);
    assert!(dose.effects.is_empty());
    assert_eq!(dose.tt_value_ped, 0.0);
    assert_eq!(dose.markup_percent, 100.0);
    assert_eq!(dose.dose_cost_ped, 0.0);
    assert!(!dose.track_cost);
    assert!(!dose.catalogue_effects);
    assert!(!dose.catalogue_duration);
    assert!(!dose.catalogue_value);

    // The stored properties carry no catalogue linkage for a hand-added
    // consumable.
    let stored: String = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT properties_json FROM equipment_library WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert!(stored["catalog_id"].is_null());
    assert!(stored["entity"].is_null());

    let listed = api.equipment_library().await.unwrap().entries;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "1");

    // The stored class is fixed.
    let mut as_weapon = consumable("x");
    as_weapon.kind = EquipmentKind::Weapon;
    as_weapon.catalog_id = Some("w1".to_string());
    assert_eq!(
        api.equipment_update(1, &as_weapon).await.unwrap_err(),
        ApiError::bad_request("Cannot change equipment type"),
    );

    // A missing row names itself.
    assert_eq!(
        api.equipment_update(9, &consumable("X")).await.unwrap_err(),
        ApiError::not_found("Equipment item 9 not found"),
    );

    // The detail mirrors the row into the weapon slot.
    let detail = api.equipment_detail(1).await.unwrap();
    assert_eq!(detail.weapon.name, "Nutrio Bar");
    assert_eq!(detail.total_cost_per_use, 0.0);
    assert!(detail.cost_breakdown.is_empty());
    assert_eq!(
        api.equipment_detail(9).await.unwrap_err(),
        ApiError::not_found("Equipment item 9 not found"),
    );

    // Deletes are idempotent acknowledgements, present row or not.
    api.equipment_delete(1).await.unwrap();
    api.equipment_delete(9).await.unwrap();
    assert!(api.equipment_library().await.unwrap().entries.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weapon_setup_stores_and_lists_with_its_catalogue_economy() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w1".to_string());
    req.weapon_markup = 110;
    let added = api.equipment_add(&req).await.unwrap();
    assert_eq!(added.name, "Opalo Rifle (L)");
    assert!(added.is_limited);
    assert_eq!(added.enrichment_level, 1);
    assert_eq!(added.kind, EquipmentKind::Weapon);

    let detail = api.equipment_detail(1).await.unwrap();
    assert_eq!(detail.weapon.catalog_id.as_deref(), Some("w1"));
    assert_eq!(detail.weapon.markup_percent, 110.0);
    assert_eq!(detail.weapon.decay, 0.5);
    assert_eq!(detail.weapon.ammo_burn, 3.0);
    assert!(detail.amplifier.is_none());
    assert!(detail.scope.is_none());
    assert!(detail.absorber.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_speed_past_the_attack_rate_limit_reprices_every_weapon_surface() {
    use eo_api::settings::{
        PassiveEffectInput, PassiveEffectKind, PassiveEffectSourceInput, SettingsPatch,
    };

    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w2".to_string());
    let added = api.equipment_add(&req).await.unwrap();
    let id: i64 = added.id.parse().unwrap();

    // At its own 90 a minute: 1 PEC decay and 1 PEC ammo, 20-40 damage.
    let detail = api.equipment_detail(id).await.unwrap();
    let rate = detail
        .attack_rate
        .as_ref()
        .expect("the catalogue publishes a rate");
    assert_eq!(rate.base_per_minute, 90.0);
    assert_eq!(rate.effective_per_minute, 90.0);
    assert_eq!(rate.factor, 1.0);
    assert_eq!(detail.total_cost_per_use, 2.0);
    assert_eq!(added.damage_max.as_ref().copied(), Some(40.0));
    // The catalogue entry carries the rate for the form's preview.
    let entry = catalogue_entry(&api, SearchKind::Weapon, "Rapid Carbine").await;
    assert_eq!(entry.uses_per_minute.as_ref().copied(), Some(90.0));

    // Two rings declaring 30% between them; equipped items add at most 15%.
    let ring = |id: &str, percent: f64| PassiveEffectSourceInput {
        id: id.into(),
        name: format!("Ring {id}"),
        enabled: true,
        effects: vec![PassiveEffectInput {
            kind: PassiveEffectKind::ReloadSpeed,
            magnitude_percent: percent,
        }],
    };
    let settings = api
        .settings_update(SettingsPatch {
            passive_effect_sources: Some(vec![ring("a", 15.0), ring("b", 15.0)]),
            ..SettingsPatch::default()
        })
        .await
        .unwrap();
    assert_eq!(settings.reload_speed.declared_percent, 30.0);
    assert_eq!(settings.reload_speed.effective_percent, 15.0);
    assert_eq!(settings.reload_speed.item_limit_percent, 15.0);

    // 90 x 1.15 = 103.5 a minute asked for; the server runs 100 and each
    // attack deals and costs 1.035 times as much.
    let detail = api.equipment_detail(id).await.unwrap();
    let rate = detail.attack_rate.as_ref().unwrap();
    assert_eq!(rate.reload_speed_percent, 15.0);
    assert!((rate.buffed_per_minute - 103.5).abs() < 1e-9);
    assert_eq!(rate.effective_per_minute, 100.0);
    assert!((rate.factor - 1.035).abs() < 1e-9);
    assert!((detail.total_cost_per_use - 2.07).abs() < 1e-9);
    let listed = api.equipment_library().await.unwrap().entries;
    let row = listed.iter().find(|item| item.id == added.id).unwrap();
    assert!((row.cost_per_use - 2.07).abs() < 1e-9);
    assert_eq!(row.damage_max.as_ref().copied(), Some(41.4));
    assert_eq!(row.damage_min.as_ref().copied(), Some(20.7));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weapon_declares_its_damage_over_time_effect_and_can_drop_it() {
    use eo_api::equipment::{WeaponEffectMode, WeaponEffectRequest};

    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w1".to_string());
    req.weapon_effect = Some(WeaponEffectRequest {
        mode: WeaponEffectMode::Compound,
        hit_min: Some(100.0),
        hit_max: Some(160.0),
        duration_seconds: 25.0,
        tick_min: 35.0,
        tick_max: 75.0,
        tick_seconds: Some(1.2),
    });
    let added = api.equipment_add(&req).await.unwrap();
    let effect = added.effect_profile.as_ref().unwrap();
    assert_eq!(effect.mode, WeaponEffectMode::Compound);
    assert_eq!(effect.hit_min.as_ref(), Some(&100.0));
    assert_eq!(effect.duration_seconds, 25.0);
    assert_eq!(effect.tick_seconds.as_ref(), Some(&1.2));
    let detail = api.equipment_detail(1).await.unwrap();
    assert_eq!(detail.effect_profile.as_ref().unwrap().tick_max, 75.0);

    // The stored props carry it where attribution reads it.
    let stored = db
        .with_reader(|conn| {
            Ok::<String, eo_services::db::DbError>(conn.query_row(
                "SELECT properties_json FROM equipment_library WHERE id = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    let props: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        eo_services::tracker::damage_band_from_props(&props),
        Some(eo_services::tracker::DamageBand {
            min: 100.0,
            max: 160.0
        })
    );

    // A profile attribution could not use is refused, and changes nothing.
    let mut bad = req.clone();
    bad.weapon_effect = Some(WeaponEffectRequest {
        mode: WeaponEffectMode::OverTime,
        hit_min: None,
        hit_max: None,
        duration_seconds: 0.0,
        tick_min: 35.0,
        tick_max: 75.0,
        tick_seconds: None,
    });
    assert_eq!(
        api.equipment_update(1, &bad).await.unwrap_err(),
        ApiError::bad_request("An effect needs a duration above zero and at most ten minutes"),
    );
    assert!(api
        .equipment_detail(1)
        .await
        .unwrap()
        .effect_profile
        .is_some());

    // Saving without one drops it: every hit is a shot again.
    let mut plain = req.clone();
    plain.weapon_effect = None;
    let updated = api.equipment_update(1, &plain).await.unwrap();
    assert!(updated.effect_profile.is_none());
    assert!(api
        .equipment_detail(1)
        .await
        .unwrap()
        .effect_profile
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_existing_weapon_setup_gains_descriptive_lifesteal_without_a_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
             VALUES ('Chip + Delta', 'weapon', 'w1', ?)",
            [r#"{"weapon_entity":{"id":"w1","name":"Opalo Rifle (L)","economy":{"decay":0.5,"ammo_burn":300}},"weapon_catalog_id":"w1","amp_entity":{"id":"a1","name":"Mayhem MF-Amplifier Delta (L)","economy":{"decay":0.1,"ammo_burn":0}},"amp_catalog_id":"a1"}"#],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let listed = api.equipment_library().await.unwrap().entries;
    assert_eq!(listed[0].lifesteal_percent.as_ref(), Some(&2.0));
    let detail = api.equipment_detail(1).await.unwrap();
    assert_eq!(detail.lifesteal_percent.as_ref(), Some(&2.0));
    assert_eq!(detail.weapon.efficiency_pct.as_ref(), Some(&56.7));
    assert_eq!(
        detail
            .amplifier
            .as_ref()
            .and_then(|amplifier| amplifier.efficiency_pct.0),
        Some(75.0)
    );
    let expected = &detail.expected_returns.as_ref().unwrap().three_looter_mean;
    assert_eq!(expected.looter_level, 0.0);
    assert_eq!(expected.coverage, 1.0);
    assert!(!expected.incomplete);
    assert_eq!(
        expected.effective_efficiency.as_ref(),
        Some(&eo_api::equipment::EquipmentEffectiveEfficiency {
            status: eo_api::equipment::EquipmentEffectiveEfficiencyStatus::WithinModelRange,
            efficiency_pct: Some(57.21).into(),
        })
    );

    let stored = db
        .with_reader(|conn| {
            Ok::<String, eo_services::db::DbError>(conn.query_row(
                "SELECT properties_json FROM equipment_library WHERE id = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!stored.contains("lifesteal_percent"));
    assert!(!stored.contains("efficiency"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weapon_detail_combines_efficiency_and_damage_into_loss_per_100_hp() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
             VALUES ('Test Rifle', 'weapon', 'w9', ?)",
            [r#"{"weapon_entity":{"id":"w9","name":"Test Rifle","economy":{"decay":1.0,"ammo_burn":100,"efficiency":80},"damage":{"impact":40}},"weapon_catalog_id":"w9"}"#],
        )?;
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
             VALUES ('No Damage', 'weapon', 'w1', ?)",
            [r#"{"weapon_entity":{"id":"w1","name":"Opalo Rifle (L)","economy":{"decay":0.5,"ammo_burn":300}},"weapon_catalog_id":"w1"}"#],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // At looter 0 a use costs 2 PEC and returns 86% + 7% x 80% of it, so it
    // loses 0.168 PEC; an expected use deals 40 x 0.9 x 0.75 x 1.02 damage.
    let detail = api.equipment_detail(1).await.unwrap();
    let expected = &detail.expected_returns.as_ref().unwrap().three_looter_mean;
    let loss = expected
        .loss_per_100_hp_pec
        .0
        .expect("a damaging weapon has a figure");
    let want = 100.0 * 0.168 / (40.0 * 0.9 * 0.75 * 1.02);
    assert!((loss - want).abs() < 1e-9, "{loss} against {want}");

    // A weapon the catalogue gives no damage cannot be placed per HP.
    let undamaged = api.equipment_detail(2).await.unwrap();
    assert!(undamaged
        .expected_returns
        .as_ref()
        .unwrap()
        .three_looter_mean
        .loss_per_100_hp_pec
        .0
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weapon_reads_its_expected_return_at_every_looter_basis() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
             VALUES ('Test Rifle', 'weapon', 'w9', ?)",
            [r#"{"weapon_entity":{"id":"w9","name":"Test Rifle","economy":{"decay":1.0,"ammo_burn":100,"efficiency":80},"damage":{"impact":40}},"weapon_catalog_id":"w9"}"#],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    seed_looters(&db, 20.0, 50.0, 80.0).await;

    // Community Model v1 returns 86% + 7% x 80% Efficiency + 7% x looter / 100
    // of a 2 PEC use; the rest is lost over 40 x 0.9 x 0.75 x 1.02 damage.
    let returns = api
        .equipment_detail(1)
        .await
        .unwrap()
        .expected_returns
        .0
        .expect("a modelled weapon");
    let damage = 40.0 * 0.9 * 0.75 * 1.02;
    for (expected, source, level) in [
        (
            &returns.three_looter_mean,
            ExpectedLooterSource::ThreeLooterMean,
            50.0,
        ),
        (&returns.animal, ExpectedLooterSource::Animal, 20.0),
        (&returns.mutant, ExpectedLooterSource::Mutant, 50.0),
        (&returns.robot, ExpectedLooterSource::Robot, 80.0),
    ] {
        let rate = 0.86 + 0.07 * 0.8 + 0.07 * level / 100.0;
        assert_eq!(expected.looter_source, source);
        assert_eq!(expected.looter_level, level);
        let tt_rate = expected.expected_tt_rate.0.unwrap();
        assert!(
            (tt_rate - rate).abs() < 1e-9,
            "{source:?}: {tt_rate} against {rate}"
        );
        let loss = expected.loss_per_100_hp_pec.0.unwrap();
        let want = 100.0 * 2.0 * (1.0 - rate) / damage;
        assert!(
            (loss - want).abs() < 1e-9,
            "{source:?}: {loss} against {want}"
        );
    }
    // A higher looter level lowers the break-even markup.
    let markup = |expected: &eo_api::equipment::EquipmentExpectedReturn| {
        expected.break_even_loot_markup.0.unwrap()
    };
    assert!(markup(&returns.robot) < markup(&returns.mutant));
    assert!(markup(&returns.mutant) < markup(&returns.animal));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_catalogue_implant_and_extender_reprice_the_weapon() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    // The implant is listed in its own vocabulary, carrying its
    // absorption share for the form preview.
    let entry = catalogue_entry(
        &api,
        SearchKind::Implant,
        "NeoPsion 85-B Mindforce Implant (L)",
    )
    .await;
    assert_eq!(entry.absorption_percent.as_ref(), Some(&20.0));
    assert!(entry.is_limited);

    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w1".to_string());
    req.weapon_markup = 1500;
    req.implant_catalog_id = Some("i1".to_string());
    req.implant_markup = 110;
    req.absorber_catalog_id = Some("x1".to_string());
    req.absorber_markup = 108;
    let added = api.equipment_add(&req).await.unwrap();
    // Implant 20% of 0.5 decay @ 1.10 = 0.11; extender/absorber 20% of the
    // 0.4 remainder @ 1.08 = 0.0864; weapon keeps 0.32 @ 15.0 = 4.8; ammo 3.0.
    assert_eq!(added.cost_per_use, 7.9964);

    let detail = api.equipment_detail(1).await.unwrap();
    let implant = detail.implant.as_ref().unwrap();
    assert_eq!(implant.name, "NeoPsion 85-B Mindforce Implant (L)");
    assert_eq!(implant.catalog_id.as_deref(), Some("i1"));
    assert_eq!(implant.absorption_percent, 20.0);
    assert_eq!(implant.markup_percent, 110.0);
    assert!(implant.is_limited);
    let components: Vec<&str> = detail
        .cost_breakdown
        .iter()
        .map(|line| line.component.as_str())
        .collect();
    assert_eq!(
        components,
        ["Implant decay", "Absorber decay", "Weapon decay", "Ammo"]
    );
    assert_eq!(detail.total_cost_per_use, 7.9964);

    // Clearing the implant on update removes it entirely.
    let mut cleared = req.clone();
    cleared.implant_catalog_id = None;
    cleared.absorber_catalog_id = None;
    let updated = api.equipment_update(1, &cleared).await.unwrap();
    assert_eq!(updated.cost_per_use, 10.5);
    let detail = api.equipment_detail(1).await.unwrap();
    assert!(detail.implant.is_none());
    assert!(detail.absorber.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slotted_or_carried_row_still_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    api.equipment_add(&consumable("Gone")).await.unwrap();

    // A hotbar slot and the carried list naming row 1 do not hold it: the
    // tracker skips ids the library no longer holds. A retired preset
    // naming it holds it no longer either.
    std::fs::write(
        dir.path().join("data").join("settings.json"),
        r#"{"hotbar": {"1": 1}, "carried_weapon_ids": [1],
            "trifecta_presets": [{"id": "p1", "name": "P", "small_weapon_id": 1}]}"#,
    )
    .unwrap();
    api.equipment_delete(1).await.unwrap();
    assert!(api.equipment_library().await.unwrap().entries.is_empty());
    // Idempotent over the missing row.
    api.equipment_delete(1).await.unwrap();
}

fn dosed(catalog_id: Option<&str>, name: Option<&str>) -> EquipmentRequest {
    let mut request = consumable(name.unwrap_or(""));
    request.catalog_id = catalog_id.map(str::to_string);
    request.name = name.map(str::to_string);
    request.dose = Some(eo_api::equipment::ConsumableDoseRequest {
        markup_percent: 150.0,
        track_cost: true,
        duration_seconds: Some(600.0),
        reload_speed_percent: Some(12.0),
        tt_value_ped: Some(2.0),
    });
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_catalogue_consumable_takes_its_dose_from_the_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let added = api.equipment_add(&dosed(Some("s2"), None)).await.unwrap();
    let settings = added
        .consumable
        .as_ref()
        .expect("a consumable carries its dose");
    // The catalogue's effects, duration, and TT value win over the
    // declared ones; the markup and tracking are the player's.
    assert!(settings.catalogue_effects && settings.catalogue_duration && settings.catalogue_value);
    assert_eq!(settings.duration_seconds, 3600.0);
    assert_eq!(settings.tt_value_ped, 3.0);
    assert_eq!(settings.effects.len(), 1);
    assert_eq!(
        settings.effects[0].reload_speed_percent.as_ref(),
        Some(&20.0)
    );
    assert!((settings.dose_cost_ped - 4.5).abs() < 1e-12);
    assert!((added.cost_per_use - 450.0).abs() < 1e-9);

    // The catalogue entry carries the catalogue dose for the form to show before
    // anything is saved; tracking waits for the player's choice.
    let entry = catalogue_entry(&api, SearchKind::Consumable, "Nanobots - Adrenaline Boost").await;
    let dose = entry
        .consumable
        .as_ref()
        .expect("a stimulant carries its dose");
    assert_eq!(dose.duration_seconds, 3600.0);
    assert_eq!(dose.tt_value_ped, 3.0);
    assert!(!dose.track_cost);
    assert!(entry.uses_per_minute.as_ref().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_custom_consumable_declares_its_dose_and_rejects_nonsense() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let added = api
        .equipment_add(&dosed(None, Some("Home Brew")))
        .await
        .unwrap();
    let settings = added.consumable.as_ref().unwrap();
    assert!(!settings.catalogue_effects);
    assert_eq!(settings.duration_seconds, 600.0);
    assert_eq!(
        settings.effects[0].reload_speed_percent.as_ref(),
        Some(&12.0)
    );
    assert!((settings.dose_cost_ped - 3.0).abs() < 1e-12);

    let mut bad = dosed(None, Some("Bad Brew"));
    bad.dose.as_mut().unwrap().markup_percent = 0.0;
    assert!(matches!(
        api.equipment_add(&bad).await,
        Err(ApiError::BadRequest { .. })
    ));
    let mut bad = dosed(None, Some("Bad Brew"));
    bad.dose.as_mut().unwrap().reload_speed_percent = Some(-100.0);
    assert!(matches!(
        api.equipment_add(&bad).await,
        Err(ApiError::BadRequest { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dose_moves_the_reload_speed_every_surface_prices_under() {
    let dir = tempfile::tempdir().unwrap();
    // One slot bound, so a session can start.
    let (api, _db) = api_over_settings(
        dir.path(),
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    let weapon = api
        .equipment_add(&{
            let mut request = consumable("");
            request.kind = EquipmentKind::Weapon;
            request.catalog_id = Some("w2".into());
            request.name = None;
            request
        })
        .await
        .unwrap();
    let pill = api.equipment_add(&dosed(Some("s2"), None)).await.unwrap();
    let pill_id: i64 = pill.id.parse().unwrap();
    let before = api
        .equipment_detail(weapon.id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(before.attack_rate.as_ref().unwrap().factor, 1.0);

    // Outside a session no dose is taken.
    assert!(matches!(
        api.consumable_dose_start(pill_id, false).await,
        Err(ApiError::Conflict { .. })
    ));

    // In a session: the dose runs, counts, and books its cost.
    api.tracking_start().await.unwrap();
    let doses = api.consumable_dose_start(pill_id, false).await.unwrap();
    assert_eq!(doses.doses.len(), 1);
    let dose = &doses.doses[0];
    assert!((dose.cost_ped - 4.5).abs() < 1e-12);
    assert!(!dose.untimed);
    assert!(dose.ends_at.as_ref().is_some());
    assert!(dose.session_id.as_ref().is_some());
    assert_eq!(doses.reload_speed.consumed_percent, 20.0);
    assert_eq!(doses.reload_speed.in_effect_percent, 20.0);
    assert_eq!(doses.options.len(), 1);

    // 90 attacks a minute at +20% asks for 108; the server holds 100.
    let during = api
        .equipment_detail(weapon.id.parse().unwrap())
        .await
        .unwrap();
    assert!((during.attack_rate.as_ref().unwrap().factor - 1.08).abs() < 1e-12);

    // A misclick comes off exactly, and back.
    let removed = api.consumable_dose_remove(dose.id.clone()).await.unwrap();
    assert!(removed.doses.is_empty());
    assert_eq!(removed.reload_speed.in_effect_percent, 0.0);
    let restored = api.consumable_dose_restore(dose.id.clone()).await.unwrap();
    assert_eq!(restored.doses.len(), 1);
    assert_eq!(restored.reload_speed.in_effect_percent, 20.0);

    assert!(matches!(
        api.consumable_dose_remove("nope".into()).await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.consumable_dose_start(weapon.id.parse().unwrap(), false)
            .await,
        Err(ApiError::NotFound { .. })
    ));
    // A timed dose ends on its own.
    assert!(matches!(
        api.consumable_dose_end(dose.id.clone()).await,
        Err(ApiError::Conflict { .. })
    ));

    // An untimed dose, declared from before: in force with no end until the
    // player ends it.
    let carried = api.consumable_dose_start(pill_id, true).await.unwrap();
    let running: Vec<_> = carried
        .doses
        .iter()
        .filter(|dose| dose.ends_at.as_ref().is_none())
        .collect();
    assert_eq!(running.len(), 1);
    assert!(running[0].untimed);
    assert_eq!(running[0].cost_ped, 0.0);
    assert_eq!(carried.reload_speed.consumed_percent, 20.0);
    let ended = api
        .consumable_dose_end(running[0].id.clone())
        .await
        .unwrap();
    assert_eq!(ended.reload_speed.consumed_percent, 0.0);

    // Stopping the session ends every dose, and the readout empties.
    api.consumable_dose_start(pill_id, false).await.unwrap();
    api.tracking_stop().await.unwrap();
    let stopped = api.consumable_doses().await.unwrap();
    assert!(stopped.doses.is_empty());
    assert_eq!(stopped.reload_speed.in_effect_percent, 0.0);
}

/// The facade of [`api_over`] grouping the library by the bundled
/// interface profile's rules for reading tool names.
async fn api_with_tool_names(dir: &Path) -> (Api, Db) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources/ui_profile.json");
    let profile = eo_services::game_ui::profile::UiProfile::load(&path)
        .expect("the bundled interface profile loads");
    let (api, db) = api_over(dir).await;
    (api.with_tool_names(Some(profile.tool_names)), db)
}

/// Store a weapon, healer or harvesting tool from the test catalogue,
/// answering its id.
async fn add_catalogued(api: &Api, kind: EquipmentKind, catalog_id: &str) -> i64 {
    let mut req = consumable("");
    req.kind = kind;
    req.name = None;
    req.catalog_id = Some(catalog_id.to_string());
    let added = api.equipment_add(&req).await.unwrap();
    added.id.parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_starts_enabled_and_turns_off_and_on() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;

    // Every new entry is enabled, whatever its kind.
    let weapon = api
        .equipment_add(&{
            let mut req = consumable("");
            req.kind = EquipmentKind::Weapon;
            req.name = None;
            req.catalog_id = Some("w1".to_string());
            req
        })
        .await
        .unwrap();
    assert!(weapon.enabled);
    let pill = api.equipment_add(&consumable("Pill")).await.unwrap();
    assert!(pill.enabled);
    let id: i64 = weapon.id.parse().unwrap();

    let stored = |db: Db, id: i64| async move {
        db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT enabled FROM equipment_library WHERE id = ?",
                rusqlite::params![id],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap()
    };

    // Off: the answer, the stored row and the library all say so; the
    // other entry is untouched.
    let off = api.equipment_set_enabled(id, false).await.unwrap();
    assert_eq!(off.id, weapon.id);
    assert!(!off.enabled);
    assert_eq!(stored(db.clone(), id).await, 0);
    let listed = api.equipment_library().await.unwrap().entries;
    let enabled: Vec<(&str, bool)> = listed
        .iter()
        .map(|entry| (entry.id.as_str(), entry.enabled))
        .collect();
    assert_eq!(
        enabled,
        [(weapon.id.as_str(), false), (pill.id.as_str(), true)]
    );

    // Turning it off again changes nothing; on again restores it.
    assert!(!api.equipment_set_enabled(id, false).await.unwrap().enabled);
    let on = api.equipment_set_enabled(id, true).await.unwrap();
    assert!(on.enabled);
    assert_eq!(stored(db.clone(), id).await, 1);
    assert!(api.equipment_library().await.unwrap().entries[0].enabled);

    // Editing an entry keeps its flag: only the flag's own command moves it.
    api.equipment_set_enabled(id, false).await.unwrap();
    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w1".to_string());
    req.weapon_markup = 120;
    let edited = api.equipment_update(id, &req).await.unwrap();
    assert!(!edited.enabled);
    assert_eq!(stored(db.clone(), id).await, 0);

    // A missing entry is refused as the other edits refuse it.
    assert_eq!(
        api.equipment_set_enabled(99, true).await.unwrap_err(),
        ApiError::not_found("Equipment item 99 not found"),
    );
    assert_eq!(
        api.equipment_set_enabled(99, false).await.unwrap_err(),
        api.equipment_update(99, &req).await.unwrap_err(),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_library_groups_the_entries_the_bar_cannot_tell_apart() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_with_tool_names(dir.path()).await;

    // Nothing stored, nothing grouped.
    assert!(api
        .equipment_library()
        .await
        .unwrap()
        .indistinguishable
        .is_empty());

    // One weapon alone stands apart.
    let first = add_catalogued(&api, EquipmentKind::Weapon, "w1").await;
    assert!(api
        .equipment_library()
        .await
        .unwrap()
        .indistinguishable
        .is_empty());

    // The same weapon kept in a second set-up: the two are one group.
    let second = add_catalogued(&api, EquipmentKind::Weapon, "w1").await;
    let library = api.equipment_library().await.unwrap();
    assert_eq!(
        library.indistinguishable,
        [vec![first.to_string(), second.to_string()]]
    );

    // A third entry of another name, and a healer, leave the group of two.
    add_catalogued(&api, EquipmentKind::Weapon, "w2").await;
    add_catalogued(&api, EquipmentKind::Healing, "h1").await;
    let library = api.equipment_library().await.unwrap();
    assert_eq!(library.entries.len(), 4);
    assert_eq!(
        library.indistinguishable,
        [vec![first.to_string(), second.to_string()]]
    );

    // A group holds its members on or off alike.
    api.equipment_set_enabled(second, false).await.unwrap();
    assert_eq!(
        api.equipment_library().await.unwrap().indistinguishable,
        [vec![first.to_string(), second.to_string()]]
    );

    // A consumable is never held, so never grouped, even two of one name
    // or one named as a weapon is.
    api.equipment_add(&consumable("Vita Bar")).await.unwrap();
    api.equipment_add(&consumable("Vita Bar")).await.unwrap();
    api.equipment_add(&consumable("Opalo Rifle (L)"))
        .await
        .unwrap();
    let library = api.equipment_library().await.unwrap();
    assert_eq!(library.entries.len(), 7);
    assert_eq!(
        library.indistinguishable,
        [vec![first.to_string(), second.to_string()]]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_interface_profile_the_library_reads_with_no_groups() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    add_catalogued(&api, EquipmentKind::Weapon, "w1").await;
    add_catalogued(&api, EquipmentKind::Weapon, "w1").await;
    let library = api.equipment_library().await.unwrap();
    assert_eq!(library.entries.len(), 2);
    assert!(library.indistinguishable.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_turned_off_reads_as_before_everywhere_else() {
    let dir = tempfile::tempdir().unwrap();
    // The weapon, the first entry, bound to a slot so a session can start.
    let (api, db) = api_over_settings(
        dir.path(),
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    let weapon = add_catalogued(&api, EquipmentKind::Weapon, "w2").await;
    assert_eq!(weapon, 1);
    let healer = add_catalogued(&api, EquipmentKind::Healing, "h1").await;
    // A pill lasting ten minutes, booked at its TT.
    let mut pill = consumable("Pill");
    pill.dose = Some(eo_api::equipment::ConsumableDoseRequest {
        markup_percent: 100.0,
        track_cost: true,
        duration_seconds: Some(600.0),
        reload_speed_percent: None,
        tt_value_ped: Some(1.5),
    });
    let pill = api
        .equipment_add(&pill)
        .await
        .unwrap()
        .id
        .parse::<i64>()
        .unwrap();
    let ids = Entries {
        weapon,
        healer,
        pill,
    };
    // An ended session with an uncosted heal and an unpriced shot for the
    // reviews to offer the entries for, and a running one for doses.
    db.with_writer(move |conn| {
        conn.execute_batch(&format!(
            r#"INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,heal_cost,
                 dangling_cost,mob_tracking_mode,updated_at,weapon_shots_agreed,
                 weapon_shots_evidenced)
               VALUES('ended',1000.0,4600.0,0,0.0,0.0,'mob',4600.0,0,0);
               INSERT INTO healing_outputs(id,session_id,activation_id,observed_at,
                 chat_timestamp,amount,classification,reason)
               VALUES('heal','ended',NULL,1200,'t',28,'unattributed','seeded');
               INSERT INTO weapon_shot_evidence(id,session_id,kill_id,observed_at,amount,
                 critical,attribution,hotbar_tool,tool_name,cost_per_shot,candidates_json,
                 reason)
               VALUES('shot','ended',NULL,1300.0,30.0,0,'unresolved',NULL,NULL,0,
                 '[{{"equipmentId":{weapon},"name":"Rapid Carbine","fits":true}}]',
                 'no weapon declared');"#
        ))?;
        Ok(())
    })
    .await
    .unwrap();
    api.tracking_start().await.unwrap();

    let before = everything_but_the_flag(&api, &db, ids).await;
    assert_ne!(
        before["weaponCostByName"], 0.0,
        "the weapon is priced by name"
    );
    assert_eq!(
        before["healingCorrectionTools"][0]["equipmentId"], healer,
        "the healer is offered for the heal"
    );
    assert_eq!(
        before["weaponCorrectionWeapons"][0]["equipmentId"], weapon,
        "the weapon is offered for the shot"
    );
    for id in [weapon, healer, pill] {
        api.equipment_set_enabled(id, false).await.unwrap();
    }
    let after = everything_but_the_flag(&api, &db, ids).await;
    for (reader, value) in before.as_object().unwrap() {
        assert_eq!(&after[reader], value, "{reader} reads as before");
    }
}

/// The entries [`everything_but_the_flag`] reads.
#[derive(Clone, Copy)]
struct Entries {
    weapon: i64,
    healer: i64,
    pill: i64,
}

/// The error kind and message of a refusal, or the value of an answer.
fn answer<T: serde::Serialize>(result: Result<T, ApiError>) -> Value {
    match result {
        Ok(value) => serde_json::to_value(value).unwrap(),
        Err(error) => serde_json::json!({ "refused": error }),
    }
}

/// What the entries are to every reader of the library but the flag's
/// own, by reader. The reads that write (marking a heal or a shot, taking a
/// dose) are undone after, so the reads can be taken again.
async fn everything_but_the_flag(api: &Api, db: &Db, ids: Entries) -> Value {
    use eo_api::healing::HealingCorrectionTarget;
    use eo_api::weapons::WeaponShotGroup;
    use eo_services::equipment_pricing::{
        hotbar_equipment_row_sync, weapon_cost_by_name, weapon_properties_by_name_fragment_sync,
    };

    let Entries {
        weapon,
        healer,
        pill,
    } = ids;
    let mut reads = serde_json::Map::new();

    // The library, less the flag, and each entry's detail.
    let mut summaries = Vec::new();
    for entry in api.equipment_library().await.unwrap().entries {
        let mut value = serde_json::to_value(&entry).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        summaries.push(value);
    }
    reads.insert("library".into(), summaries.into());
    let mut details = Vec::new();
    for id in [weapon, healer, pill] {
        details.push(answer(api.equipment_detail(id).await));
    }
    reads.insert("details".into(), details.into());

    // The hotbar and tool-change paths' rows, the trifecta's typed rows,
    // and the weapon cost looked up by name.
    let (hotbar, by_name, by_name_async, cost) = (
        db.with_reader(move |conn| {
            [weapon, healer, pill]
                .into_iter()
                .map(|id| hotbar_equipment_row_sync(conn, id))
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .unwrap(),
        db.with_reader(|conn| weapon_properties_by_name_fragment_sync(conn, "Carbine"))
            .await
            .unwrap(),
        db.weapon_properties_by_name_fragment("Carbine")
            .await
            .unwrap(),
        db.with_reader(|conn| {
            Ok(weapon_cost_by_name(
                conn,
                "Rapid Carbine",
                &|props: &Value| props.clone(),
            ))
        })
        .await
        .unwrap(),
    );
    reads.insert("hotbarRows".into(), serde_json::json!(hotbar));
    reads.insert("weaponByName".into(), serde_json::json!(by_name));
    reads.insert("weaponByNameAsync".into(), serde_json::json!(by_name_async));
    reads.insert("weaponCostByName".into(), serde_json::json!(cost));
    let mut typed = Vec::new();
    for (id, kind) in [
        (weapon, "weapon"),
        (healer, "healing"),
        (pill, "consumable"),
    ] {
        typed.push(serde_json::json!(db
            .equipment_item(id, kind)
            .await
            .unwrap()));
    }
    reads.insert("typedRows".into(), typed.into());

    // The consumables: the items the doses readout offers, and a dose taken
    // of the entry (then removed).
    reads.insert(
        "consumableOptions".into(),
        serde_json::to_value(api.consumable_doses().await.unwrap().options).unwrap(),
    );
    let taken = api.consumable_dose_start(pill, false).await.unwrap();
    let dose = taken
        .doses
        .iter()
        .find(|dose| {
            dose.equipment_id.as_ref().copied() == Some(pill)
                && dose.removed_at.as_ref().is_none()
                && !dose.replaced
        })
        .expect("the dose is taken");
    reads.insert(
        "consumableDoseStart".into(),
        serde_json::json!({
            "itemName": dose.item_name,
            "costPed": dose.cost_ped,
            "costTracked": dose.cost_tracked,
            "effects": dose.effects,
        }),
    );
    api.consumable_dose_remove(dose.id.clone()).await.unwrap();

    // The healing review: the items offered for the heal, and marking the
    // heal a paid use of the healer (then undoing it).
    reads.insert(
        "healingCorrectionTools".into(),
        answer(api.healing_correction_tools("heal".into()).await),
    );
    let corrected = api
        .healing_correct(HealingCorrectionTarget::PaidUse {
            output_id: "heal".into(),
            equipment_id: healer,
        })
        .await
        .unwrap();
    let minted = corrected
        .healing
        .activations
        .iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .find(|row| row["provenance"] == "corrected")
        .expect("the heal is a paid use");
    reads.insert(
        "healingCorrect".into(),
        serde_json::json!({
            "healCost": corrected.summary.cost_breakdown.heal_cost,
            "activations": corrected.healing.activation_count,
            "toolName": minted["toolName"],
            "costPed": minted["costPed"],
        }),
    );
    api.healing_correction_undo(minted["correction"]["id"].as_str().unwrap().to_string())
        .await
        .unwrap();

    // The weapon review: the weapons offered for the shot, and assigning
    // the shot to the weapon (then undoing it).
    reads.insert(
        "weaponCorrectionWeapons".into(),
        answer(api.weapon_correction_weapons("shot".into()).await),
    );
    let assigned = api.weapon_assign("shot".into(), weapon).await.unwrap();
    reads.insert(
        "weaponAssign".into(),
        serde_json::json!({
            "cost": assigned.summary.cost,
            "assigned": serde_json::to_value(&assigned.weapon_attribution).unwrap()["assigned"],
        }),
    );
    let correction = api
        .weapon_shots("ended".into(), WeaponShotGroup::Unresolved, 0, 10)
        .await
        .unwrap()
        .shots[0]
        .correction_id
        .0
        .clone()
        .expect("a live assignment");
    api.weapon_assignment_undo(correction).await.unwrap();

    Value::Object(reads)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_held_tool_catalogue_is_built_only_while_the_reading_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    let store = Arc::new(GameDataStore::new(&dir.path().join("snapshot")).expect("snapshot store"));
    let data_dir = dir.path().join("data");
    let catalogues = eo_services::tool_catalogue::ToolCatalogues::new(
        db.clone(),
        store,
        Arc::new(move || {
            eo_services::config_service::load_config_readonly(&data_dir)
                .is_ok_and(|config| config.tool_reading_on())
        }),
    );
    let cell = catalogues.cell();
    let api = api.with_tool_reading(catalogues, None);
    let library = || -> Vec<(String, bool)> {
        cell.current()
            .library()
            .map(|tool| (tool.name.clone(), tool.enabled))
            .collect()
    };
    let turn = |on: bool| eo_api::settings::SettingsPatch {
        tool_reading_enabled: Some(on),
        ..Default::default()
    };

    // Off: an edit builds nothing.
    let id = add_catalogued(&api, EquipmentKind::Weapon, "w1").await;
    assert!(library().is_empty(), "an edit while off builds nothing");
    assert_eq!(cell.current().bundled().count(), 0);

    // Turning it on builds once, from the library as it stands.
    api.settings_update(turn(true)).await.unwrap();
    assert_eq!(library(), [("Opalo Rifle (L)".to_string(), true)]);
    assert!(
        cell.current().bundled().count() > 0,
        "with the bundled tools"
    );

    // Already built: a patch of anything else reads nothing again, so a
    // row written behind the facade's back is not picked up.
    db.insert_equipment("Behind".into(), "weapon".into(), None, "{}".into())
        .await
        .unwrap();
    api.settings_update(eo_api::settings::SettingsPatch {
        session_name: Some("Atrox run".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(library().len(), 1, "built once");

    // On: every edit rebuilds.
    let mut req = consumable("");
    req.kind = EquipmentKind::Weapon;
    req.name = None;
    req.catalog_id = Some("w2".to_string());
    api.equipment_update(id, &req).await.unwrap();
    assert_eq!(
        library(),
        [
            ("Rapid Carbine".to_string(), true),
            ("Behind".to_string(), true)
        ],
        "an edit"
    );

    api.equipment_set_enabled(id, false).await.unwrap();
    assert_eq!(
        library()[0],
        ("Rapid Carbine".to_string(), false),
        "turned off"
    );

    // A consumable is never held, so it is not on the bar.
    api.equipment_add(&consumable("Pill")).await.unwrap();
    assert_eq!(library().len(), 2);

    api.equipment_delete(id).await.unwrap();
    assert_eq!(library(), [("Behind".to_string(), true)], "a deletion");
}
