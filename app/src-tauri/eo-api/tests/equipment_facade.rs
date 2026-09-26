//! Behavioural pins for the equipment family over the typed facade,
//! ported from the family's HTTP-era integration tests: the search
//! gates, the catalogue-less validation ladder, the custom-consumable
//! write cycle, the type-change and missing-row refusals, the unguarded
//! delete, and the transport-invariance pins
//! (the typed response serialises to the exact bytes the HTTP route
//! answered, and the stored `properties_json` bytes are unchanged).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::equipment::{EquipmentKind, EquipmentRequest, SearchKind};
use eo_api::{Api, ApiError};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// A minimal catalogue snapshot: two weapons (a limited one, so the
/// `(L)` flag pins, and a fast one with a catalogue attack rate), one
/// stimulant, one Mindforce implant, one
/// absorber/extender.
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
}

/// The composed facade over a fresh migrated database and the test
/// snapshot, plus a database handle of its own for storage assertions.
async fn api_over(dir: &Path) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    write_snapshot(&snapshot);
    let data_dir: PathBuf = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
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
            handles.sale_window_ocr,
            handles.quests.clone(),
            None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_gates_and_hits_match_the_route_behaviour() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    // Short queries answer empty before any lookup.
    let hits = api.equipment_search("o", SearchKind::Weapon).await.unwrap();
    assert!(hits.is_empty());

    // A hit carries the catalogue economy in the response shape.
    let hits = api
        .equipment_search("opalo", SearchKind::Weapon)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit.catalog_id.as_deref(), Some("w1"));
    assert_eq!(hit.name, "Opalo Rifle (L)");
    assert_eq!(hit.decay, 0.5);
    assert_eq!(hit.ammo_burn, 3.0);
    assert!(hit.is_limited);

    // A miss in another vocabulary answers empty, not an error.
    let hits = api
        .equipment_search("opalo", SearchKind::Healer)
        .await
        .unwrap();
    assert!(hits.is_empty());

    let hits = api
        .equipment_search("restoration", SearchKind::Healer)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reload_seconds, Some(3.75));
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
    assert!(api.equipment_library().await.unwrap().is_empty());

    // The stored name is stripped the reference way.
    let added = api
        .equipment_add(&consumable("  Nutrio Bar  "))
        .await
        .unwrap();
    assert_eq!(added.id, "1");
    assert_eq!(added.name, "Nutrio Bar");

    // Transport invariance: the typed summary serialises to the exact
    // body bytes the HTTP route answered for this row.
    assert_eq!(
        serde_json::to_string(&added).unwrap(),
        "{\"id\":\"1\",\"name\":\"Nutrio Bar\",\"type\":\"consumable\",\"amplifierName\":null,\
         \"costPerUse\":0.0,\"damageMin\":null,\"damageMax\":null,\"reloadSeconds\":null,\
         \"isLimited\":false,\"enrichmentLevel\":1,\"healingProfile\":null,\
         \"lifestealPercent\":null,\"effectProfile\":null,\"consumable\":{\"durationSeconds\":0.0,\
         \"effects\":[],\"ttValuePed\":0.0,\"markupPercent\":100.0,\"doseCostPed\":0.0,\
         \"trackCost\":false,\"catalogueEffects\":false,\"catalogueDuration\":false,\
         \"catalogueValue\":false,\"declaredReloadSpeedPercent\":null,\
         \"declaredDurationSeconds\":null,\"declaredTtValuePed\":null}}"
    );

    // Storage invariance: the stored props bytes are the reference
    // `json.dumps` form, unchanged by the transport migration.
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
    assert_eq!(stored, "{\"catalog_id\": null, \"entity\": null}");

    let listed = api.equipment_library().await.unwrap();
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
    assert!(api.equipment_library().await.unwrap().is_empty());
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
    // The search hit carries the rate for the form's preview.
    let hits = api
        .equipment_search("Rapid", SearchKind::Weapon)
        .await
        .unwrap();
    assert_eq!(hits[0].uses_per_minute.as_ref().copied(), Some(90.0));

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
    let listed = api.equipment_library().await.unwrap();
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

    let listed = api.equipment_library().await.unwrap();
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
    let expected = detail.expected_return.as_ref().unwrap();
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
async fn a_catalogue_implant_and_extender_reprice_the_weapon() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;

    // The implant is searchable through its own vocabulary, carrying its
    // absorption share for the form preview.
    let hits = api
        .equipment_search("neopsion", SearchKind::Implant)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].absorption_percent.as_ref(), Some(&20.0));
    assert!(hits[0].is_limited);

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
    assert!(api.equipment_library().await.unwrap().is_empty());
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
    let (api, _db) = api_over(dir.path()).await;
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

    // Outside a session: the dose runs and counts, and books nothing.
    let doses = api.consumable_dose_start(pill_id).await.unwrap();
    assert_eq!(doses.doses.len(), 1);
    let dose = &doses.doses[0];
    assert_eq!(dose.cost_ped, 0.0);
    assert!(dose.session_id.as_ref().is_none());
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
        api.consumable_dose_start(weapon.id.parse().unwrap()).await,
        Err(ApiError::NotFound { .. })
    ));
}
