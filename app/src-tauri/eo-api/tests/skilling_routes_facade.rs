//! Behavioural pins for planned skilling routes over the typed facade: a
//! weapon on a mob maturity priced toward a profession goal from the
//! skill-per-kill law and the loadout comparison engine, beside (never
//! instead of) the recorded sessions; the pill and the looter level; and
//! routes the catalogue cannot resolve.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::loadout_comparison::{ComponentPick, SetupPick};
use eo_api::skilling::{
    SkillBoostLevel, SkillingForecastQuery, SkillingForecastResult, SkillingRoutePick,
    SkillingRouteStatus, SkillingTargetKind, MAX_ROUTES,
};
use eo_api::Api;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::json;

mod common;

/// A catalogue with a sword and a pistol, the professions they train, and
/// two level-4 mobs of very different HP.
fn write_snapshot(dir: &Path) {
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(dir.join(name), value.to_string()).unwrap();
    };
    write(
        "weapons.json",
        json!([
            {"id": "sword", "name": "Laconian Sword",
             "economy": {"decay": 1.95, "ammo_burn": 0, "efficiency": 54.9, "max_tt": 8},
             "damage": {"impact": 1, "cut": 3, "stab": 3, "penetration": 1}, "uses_per_minute": 67,
             "professions": {"hit": "Swordsman (Hit)", "dmg": "Swordsman (Dmg)"}},
            {"id": "pistol", "name": "SET-P1 Civilian Sidearm, Adjusted",
             "economy": {"decay": 0.001, "ammo_burn": 7, "efficiency": 80.1, "max_tt": 0.1},
             "damage": {"burn": 1}, "uses_per_minute": 86,
             "professions": {"hit": "Laser Pistoleer (Hit)", "dmg": "Ranged Laser (Dmg)"}},
            {"id": "blank", "name": "Untrained Blade",
             "economy": {"decay": 1.0, "ammo_burn": 0, "efficiency": 50, "max_tt": 8},
             "damage": {"cut": 8}, "uses_per_minute": 60}
        ]),
    );
    write("weapon_amplifiers.json", json!([]));
    write("absorbers.json", json!([]));
    write("mindforce_implants.json", json!([]));
    write(
        "mobs.json",
        json!([
            {"name": "Carabok", "species": {"name": "Carabok", "codex_base_cost": 1.25, "codex_type": "Mob"},
             "planet": "Arkadia", "maturities": [{"name": "Puny", "hp": 10, "level": 4}]},
            {"name": "Bank Robber", "species": {"name": null}, "planet": "ROCKtropia",
             "maturities": [{"name": "Default", "hp": 2, "level": 4}]},
            {"name": "Ghost", "species": {"name": "Ghost"}, "planet": "Calypso",
             "maturities": [{"name": "Young", "hp": null, "level": null}]}
        ]),
    );
    let profession = |name: &str, skills: &[(&str, f64)]| {
        json!({"name": name, "skills": skills.iter()
            .map(|(skill, weight)| json!({"skill": {"name": skill}, "weight": weight}))
            .collect::<Vec<_>>()})
    };
    write(
        "professions.json",
        json!([
            profession(
                "Swordsman (Hit)",
                &[
                    ("Longblades", 60.0),
                    ("Melee Combat", 30.0),
                    ("Agility", 10.0)
                ]
            ),
            profession(
                "Swordsman (Dmg)",
                &[
                    ("Longblades", 50.0),
                    ("Anatomy", 30.0),
                    ("Kill Strike", 20.0)
                ]
            ),
            profession(
                "Laser Pistoleer (Hit)",
                &[("Handgun", 80.0), ("Agility", 20.0)]
            ),
            profession(
                "Ranged Laser (Dmg)",
                &[("Laser Weaponry Technology", 70.0), ("Anatomy", 30.0)]
            ),
            profession("Animal Looter", &[("Looting", 100.0)]),
            profession("Mutant Looter", &[("Looting", 100.0)]),
            profession("Robot Looter", &[("Looting", 100.0)]),
        ]),
    );
    write(
        "skills.json",
        json!([
            {"name": "Longblades"}, {"name": "Melee Combat"}, {"name": "Anatomy"},
            {"name": "Kill Strike"}, {"name": "Handgun"}, {"name": "Laser Weaponry Technology"},
            {"name": "Looting"}, {"name": "Agility", "category": {"name": "Attributes"}}
        ]),
    );
    write("medical_tools.json", json!([]));
}

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
        ),
        db,
    )
}

/// Scanned skill levels. Kill Strike is absent: a locked skill.
async fn seed_skills(db: &Db, levels: Vec<(String, f64)>) {
    db.with_writer(move |conn| {
        for (name, level) in &levels {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?1, ?2, 'scan', 1700000000.0)",
                rusqlite::params![name, level],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

fn standard_levels() -> Vec<(String, f64)> {
    [
        ("Longblades", 2000.0),
        ("Melee Combat", 1500.0),
        ("Anatomy", 3000.0),
        ("Handgun", 500.0),
        ("Laser Weaponry Technology", 900.0),
        ("Agility", 50.0),
        ("Looting", 4000.0),
    ]
    .iter()
    .map(|(name, level)| (name.to_string(), *level))
    .collect()
}

fn route(weapon: &str, species: &str, maturity: &str) -> SkillingRoutePick {
    SkillingRoutePick {
        setup: SetupPick {
            weapon: ComponentPick {
                catalog_id: weapon.to_string(),
                markup_percent: None,
            },
            amplifier: None,
            absorber: None,
            implant: None,
        },
        species: species.to_string(),
        maturity: maturity.to_string(),
    }
}

fn swordsman_query(goal: f64, routes: Vec<SkillingRoutePick>) -> SkillingForecastQuery {
    SkillingForecastQuery {
        target: SkillingTargetKind::Profession,
        professions: vec!["Swordsman (Hit)".into()],
        goal,
        routes,
        skill_boost: Some(SkillBoostLevel::Fifty),
        looter_level: None,
    }
}

async fn forecast(api: &Api, query: &SkillingForecastQuery) -> SkillingForecastResult {
    api.character_skilling_forecast(query).await.unwrap()
}

/// The same sword on two level-4 mobs: the same skill per kill, so the
/// same kills to the goal, but the 2 HP mob costs a fraction per kill, so
/// the route there loses far less. Loss is the cycling at the setup's
/// expected loss rate, and the figures are the comparison engine's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_planned_route_prices_the_goal_per_weapon_and_mob() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let result = forecast(
        &api,
        &swordsman_query(
            20.0,
            vec![
                route("sword", "Carabok", "Puny"),
                route("sword", "Bank Robber", "Default"),
            ],
        ),
    )
    .await;
    assert!(result.error.is_none());
    assert_eq!(result.skill_boost, SkillBoostLevel::Fifty);
    assert_eq!(result.looter_level, 40.0);
    assert_eq!(result.routes.len(), 2);

    let [carabok, robber] = [&result.routes[0], &result.routes[1]];
    for route in [carabok, robber] {
        assert_eq!(route.status, SkillingRouteStatus::Ready, "{}", route.mob);
        assert_eq!(route.weapon, "Laconian Sword");
        assert!(route.cycled_ped > 0.0 && route.ped_lost > 0.0 && route.kills > 0.0);
        let loss_rate = route.loss_per_kill_pec / route.cycled_per_kill_pec;
        assert!((route.ped_lost / route.cycled_ped - loss_rate).abs() < 1e-3);
        // Skill lands in the sword's professions only: Agility is an
        // attribute and Kill Strike is locked.
        let names: Vec<&str> = route
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert!(names.contains(&"Longblades") && names.contains(&"Anatomy"));
        assert!(!names.contains(&"Agility") && !names.contains(&"Kill Strike"));
        assert!(route.hours.is_some());
    }
    assert_eq!(carabok.mob, "Puny Carabok");
    assert_eq!(carabok.mob_hp, Some(10.0));
    assert_eq!(robber.mob, "Default Bank Robber");

    // 0.121 x level + 0.0033 x HP at the 50% pill.
    assert!((carabok.skill_per_kill_pec - 0.517).abs() < 1e-9);
    assert!((robber.skill_per_kill_pec - 0.4906).abs() < 1e-9);
    // Kills scale inversely with skill per kill.
    let ratio = robber.kills / carabok.kills;
    assert!((ratio - 0.517 / 0.4906).abs() < 1e-3, "kill ratio {ratio}");
    // The small mob is the cheaper place to earn the same skill.
    assert!(robber.cycled_per_kill_pec < carabok.cycled_per_kill_pec);
    assert!(robber.ped_lost < carabok.ped_lost);

    // The recorded sessions are still answered beside the planned routes.
    assert!(result.sources.is_empty());
}

/// The pill scales skill per kill (none is two thirds of the 50% pill),
/// so the unpilled route needs half again the kills; with no pill given,
/// the most recent session's decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pill_scales_a_route_and_defaults_to_the_latest_session() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let routes = vec![route("sword", "Bank Robber", "Default")];
    let fifty = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    let none = forecast(
        &api,
        &SkillingForecastQuery {
            skill_boost: Some(SkillBoostLevel::None),
            ..swordsman_query(20.0, routes.clone())
        },
    )
    .await;
    let ratio = none.routes[0].kills / fifty.routes[0].kills;
    assert!((ratio - 1.5).abs() < 1e-3, "kill ratio {ratio}");

    // No pill given and no session recorded: unpilled.
    let unstated = SkillingForecastQuery {
        skill_boost: None,
        ..swordsman_query(20.0, routes.clone())
    };
    assert_eq!(
        forecast(&api, &unstated).await.skill_boost,
        SkillBoostLevel::None
    );

    // The latest session ran the 100% pill (stacks above it cap there).
    db.with_writer(|conn| {
        conn.execute_batch(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, skill_boost_percent) \
             VALUES ('old', 1700000000, 1700003600, 0, 50), \
                    ('new', 1700100000, 1700103600, 0, 150);",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        forecast(&api, &unstated).await.skill_boost,
        SkillBoostLevel::Hundred
    );

    // A lower looter level loses more on the same route.
    let low_looter = forecast(
        &api,
        &SkillingForecastQuery {
            looter_level: Some(0.0),
            ..swordsman_query(20.0, routes)
        },
    )
    .await;
    assert_eq!(low_looter.looter_level, 0.0);
    assert!(low_looter.routes[0].ped_lost > fifty.routes[0].ped_lost);
}

/// Routes the catalogue cannot resolve answer with a status, in request
/// order, without failing the forecast; a weapon training other
/// professions does not train the goal; a met goal asks for nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unresolvable_and_untrained_routes_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let result = forecast(
        &api,
        &swordsman_query(
            20.0,
            vec![
                route("pistol", "Bank Robber", "Default"),
                route("gone", "Bank Robber", "Default"),
                route("blank", "Bank Robber", "Default"),
                route("sword", "Ghost", "Young"),
                route("sword", "Nowhere", "Old"),
            ],
        ),
    )
    .await;
    let statuses: Vec<SkillingRouteStatus> = result.routes.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        vec![
            SkillingRouteStatus::DoesNotTrain,
            SkillingRouteStatus::UnknownWeapon,
            SkillingRouteStatus::UnknownWeapon,
            SkillingRouteStatus::UnknownMob,
            SkillingRouteStatus::UnknownMob,
        ]
    );
    assert_eq!(result.routes[0].weapon, "SET-P1 Civilian Sidearm, Adjusted");
    assert_eq!(result.routes[3].mob_hp, None::<f64>);
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["routes"][3]["status"], "unknown_mob");
    assert_eq!(wire["skillBoost"], "fifty");

    let reached = forecast(
        &api,
        &swordsman_query(1.0, vec![route("sword", "Bank Robber", "Default")]),
    )
    .await;
    assert_eq!(reached.routes[0].status, SkillingRouteStatus::Reached);
    assert_eq!(reached.routes[0].kills, 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forecast_refuses_too_many_routes_or_a_wild_looter_level() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let many = vec![route("sword", "Bank Robber", "Default"); MAX_ROUTES + 1];
    assert!(api
        .character_skilling_forecast(&swordsman_query(20.0, many))
        .await
        .is_err());
    let wild = SkillingForecastQuery {
        looter_level: Some(250.0),
        ..swordsman_query(20.0, Vec::new())
    };
    assert!(api.character_skilling_forecast(&wild).await.is_err());
}

/// Recorded kills: `count` single-sword kills of the 2 HP mob two seconds
/// apart from `start`, each costing 2 PEC and preceded by 0.6 PEC of skill,
/// in one session at `boost`.
async fn seed_kills(db: &Db, session: &str, boost: Option<i64>, start: f64, count: usize) {
    let session = session.to_string();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, skill_boost_percent) \
             VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![session, start, start + 10_000.0, boost],
        )?;
        for index in 0..count {
            let timestamp = start + 2.0 * index as f64;
            let id = format!("{session}-{index}");
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
                   shots_fired, damage_dealt, cost_ped) \
                 VALUES (?1, ?2, 'Bank Robber', 'Bank Robber', 'Default', ?3, 1, 3.0, 0.02)",
                rusqlite::params![id, session, timestamp],
            )?;
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, cost_per_shot) \
                 VALUES (?1, 'Laconian Sword', 1, 3.0, 0.02)",
                rusqlite::params![id],
            )?;
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?1, ?2, 'Longblades', 0.1, 0.006)",
                rusqlite::params![session, timestamp - 0.5],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// With enough recorded kills of the weapon on the maturity, the route is
/// priced from them: their skill per kill (restated for the pill asked),
/// their cycling, and their pace. Kills whose damage belongs to another
/// maturity are left out, and a new kill refreshes the evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enough_recorded_kills_price_a_route_from_play() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;
    let routes = vec![route("sword", "Bank Robber", "Default")];

    // Too few kills: the model prices it.
    seed_kills(&db, "a", Some(50), 1_700_000_000.0, 200).await;
    let modelled = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    assert!(modelled.routes[0].measured.is_none());
    assert!((modelled.routes[0].skill_per_kill_pec - 0.4906).abs() < 1e-9);

    seed_kills(&db, "b", Some(50), 1_700_100_000.0, 150).await;
    // A kill of a bigger maturity in the same declared session.
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
               shots_fired, damage_dealt, cost_ped) \
             VALUES ('big', 'b', 'Bank Robber', 'Bank Robber', 'Default', 1700100999, 9, 400.0, 2.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, cost_per_shot) \
             VALUES ('big', 'Laconian Sword', 9, 400.0, 0.22)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let measured = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    let route = &measured.routes[0];
    let evidence = route.measured.as_ref().expect("measured from play");
    assert_eq!((evidence.kills, evidence.sessions), (350, 2));
    assert!((route.skill_per_kill_pec - 0.6).abs() < 1e-9);
    assert!((route.cycled_per_kill_pec - 2.0).abs() < 1e-9);
    assert_eq!(route.seconds_per_kill, Some(2.0));
    // The loss rate is still the setup's modelled one.
    let modelled_rate =
        modelled.routes[0].loss_per_kill_pec / modelled.routes[0].cycled_per_kill_pec;
    assert!((route.loss_per_kill_pec / 2.0 - modelled_rate).abs() < 1e-3);

    // Unpilled play earns two thirds of the 50% pill's skill.
    let unpilled = forecast(
        &api,
        &SkillingForecastQuery {
            skill_boost: Some(SkillBoostLevel::None),
            ..swordsman_query(20.0, routes.clone())
        },
    )
    .await;
    assert!((unpilled.routes[0].skill_per_kill_pec - 0.4).abs() < 1e-9);

    // A session recorded without a pill counts at two thirds as well, so
    // its kills restate up to the same 50% figure.
    seed_kills(&db, "c", None, 1_700_200_000.0, 100).await;
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE skill_gains SET ped_value = 0.004 WHERE session_id = 'c'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let refreshed = forecast(&api, &swordsman_query(20.0, routes)).await;
    let evidence = refreshed.routes[0].measured.as_ref().unwrap();
    assert_eq!((evidence.kills, evidence.sessions), (450, 3));
    assert!((refreshed.routes[0].skill_per_kill_pec - 0.6).abs() < 1e-9);
}
