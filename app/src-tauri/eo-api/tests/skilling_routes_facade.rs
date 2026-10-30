//! Behavioural pins for planned skilling routes over the typed facade: a
//! weapon on a mob maturity priced toward a profession goal from the
//! skill-per-kill law and the loadout comparison engine, beside (never
//! instead of) the recorded sessions; the skill boost and the looter level;
//! the player's own pace on a mob; a route's expected loot markup and
//! hunting share; which targets hunting prices; routes the
//! catalogue cannot resolve; and named sessions narrowed to one weapon.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::loadout_comparison::{ComponentPick, SetupPick};
use eo_api::skilling::{
    SkillingForecastQuery, SkillingForecastResult, SkillingRoutePick, SkillingRouteStatus,
    SkillingTargetKind, MAX_ROUTES,
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
        loot_markup_percent: None,
        hunting_share_percent: None,
    }
}

fn swordsman_query(goal: f64, routes: Vec<SkillingRoutePick>) -> SkillingForecastQuery {
    SkillingForecastQuery {
        target: SkillingTargetKind::Profession,
        professions: vec!["Swordsman (Hit)".into()],
        goal,
        routes,
        skill_boost_percent: Some(50),
        looter_level: None,
        session_weapon: None,
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
    assert!(result.hunting_target);
    assert_eq!(result.skill_boost_percent, 50);
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
    // Yet the sword overkills the small mob far more: every landed hit
    // (6.12 damage on average) kills its 2 HP.
    assert!(
        (robber.overkill_share - 0.6732).abs() < 1e-9,
        "{}",
        robber.overkill_share
    );
    assert!(carabok.overkill_share > 0.0 && carabok.overkill_share < robber.overkill_share);

    // The recorded sessions are still answered beside the planned routes.
    assert!(result.sources.is_empty());
}

/// The boost scales skill per kill (none is two thirds of the 50% boost,
/// a boost between the measured points is read between them), so the
/// unboosted route needs half again the kills; with no boost given, the
/// most recent session's decides, capped at the game's 100%.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_boost_scales_a_route_and_defaults_to_the_latest_session() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let routes = vec![route("sword", "Bank Robber", "Default")];
    let fifty = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    let none = forecast(
        &api,
        &SkillingForecastQuery {
            skill_boost_percent: Some(0),
            ..swordsman_query(20.0, routes.clone())
        },
    )
    .await;
    let ratio = none.routes[0].kills / fifty.routes[0].kills;
    assert!((ratio - 1.5).abs() < 1e-3, "kill ratio {ratio}");
    // 75% sits halfway between the 50% boost and the 100% one (x1.27).
    let three_quarters = forecast(
        &api,
        &SkillingForecastQuery {
            skill_boost_percent: Some(75),
            ..swordsman_query(20.0, routes.clone())
        },
    )
    .await;
    let ratio = fifty.routes[0].kills / three_quarters.routes[0].kills;
    assert!((ratio - 1.135).abs() < 1e-3, "kill ratio {ratio}");
    // Past the cap prices at the cap.
    let over = forecast(
        &api,
        &SkillingForecastQuery {
            skill_boost_percent: Some(150),
            ..swordsman_query(20.0, routes.clone())
        },
    )
    .await;
    assert_eq!(over.skill_boost_percent, 100);

    // No boost given and no session recorded: unboosted.
    let unstated = SkillingForecastQuery {
        skill_boost_percent: None,
        ..swordsman_query(20.0, routes.clone())
    };
    assert_eq!(forecast(&api, &unstated).await.skill_boost_percent, 0);

    // The latest session recorded 150%, which the game caps at 100%.
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
    assert_eq!(forecast(&api, &unstated).await.skill_boost_percent, 100);

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
    assert_eq!(wire["skillBoostPercent"], 50);

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

/// A route's expected loot markup lifts the loot it expects (cycled less
/// TT lost) and nets off the loss; the cycling, the loss, and the time are
/// the TT route's. A route without one carries no markup and no net cost,
/// never a zero. A hunting share stretches only the time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_route_prices_its_expected_markup_and_its_hunting_share() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let plain = route("sword", "Carabok", "Puny");
    let marked = SkillingRoutePick {
        loot_markup_percent: Some(110.0),
        ..plain.clone()
    };
    let part_time = SkillingRoutePick {
        hunting_share_percent: Some(50.0),
        ..plain.clone()
    };
    let result = forecast(&api, &swordsman_query(20.0, vec![plain, marked, part_time])).await;
    let [tt, marked, part_time] = [&result.routes[0], &result.routes[1], &result.routes[2]];
    for route in [tt, marked, part_time] {
        assert_eq!(route.status, SkillingRouteStatus::Ready);
    }

    assert_eq!(tt.markup, None::<f64>);
    assert_eq!(tt.net_cost, None::<f64>);
    assert_eq!(
        (marked.cycled_ped, marked.ped_lost),
        (tt.cycled_ped, tt.ped_lost)
    );
    assert_eq!(marked.hours, tt.hours);
    let markup = marked.markup.expect("a markup was set");
    assert!((markup - (tt.cycled_ped - tt.ped_lost) * 0.10).abs() < 0.02);
    let net = marked.net_cost.expect("a net cost beside the markup");
    assert!((net - (tt.ped_lost - markup)).abs() < 0.011);
    // The sword returns about 93% on Carabok: 10% over TT on its loot
    // outweighs the 7% loss, so the net cost reads below zero.
    assert!(net < 0.0, "net {net}");

    // Twice the clock, to the hundredth of an hour the hours are given in.
    let (part, whole) = (part_time.hours.unwrap(), tt.hours.unwrap());
    assert!(
        (part - 2.0 * whole).abs() <= 0.011,
        "{part} against {whole}"
    );
    assert_eq!(part_time.seconds_per_kill, tt.seconds_per_kill);
    assert_eq!(
        (part_time.cycled_ped, part_time.ped_lost),
        (tt.cycled_ped, tt.ped_lost)
    );
    assert_eq!(part_time.markup, None::<f64>);

    let wire = serde_json::to_value(&result).unwrap();
    assert!(wire["routes"][0]["markup"].is_null());
    assert!(wire["routes"][1]["netCost"].is_number());
}

/// A markup below TT (which is always on offer) or past 1000%, and a
/// hunting share outside 1% to 100%, refuse the forecast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forecast_refuses_a_wild_markup_or_hunting_share() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;

    let base = route("sword", "Carabok", "Puny");
    let wild = [
        SkillingRoutePick {
            loot_markup_percent: Some(99.0),
            ..base.clone()
        },
        SkillingRoutePick {
            loot_markup_percent: Some(1001.0),
            ..base.clone()
        },
        SkillingRoutePick {
            hunting_share_percent: Some(0.0),
            ..base.clone()
        },
        SkillingRoutePick {
            hunting_share_percent: Some(101.0),
            ..base.clone()
        },
    ];
    for pick in wild {
        let query = swordsman_query(20.0, vec![base.clone(), pick]);
        assert!(api.character_skilling_forecast(&query).await.is_err());
    }
    let edges = vec![
        SkillingRoutePick {
            loot_markup_percent: Some(100.0),
            ..base.clone()
        },
        SkillingRoutePick {
            loot_markup_percent: Some(1000.0),
            hunting_share_percent: Some(1.0),
            ..base
        },
    ];
    let result = forecast(&api, &swordsman_query(20.0, edges)).await;
    // At TT the markup is a priced zero, not an absent one.
    assert_eq!(result.routes[0].markup, Some(0.0));
}

/// Only HP and professions some weapon trains are priced by hunting; any
/// other target grows from hunting only as a byproduct, so its routes are
/// left unpriced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hunting_prices_only_targets_a_weapon_trains() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;
    let routes = vec![route("sword", "Bank Robber", "Default")];

    let looter = forecast(
        &api,
        &SkillingForecastQuery {
            professions: vec!["Animal Looter".into()],
            ..swordsman_query(200.0, routes.clone())
        },
    )
    .await;
    assert!(!looter.hunting_target);
    assert!(looter.routes.is_empty());

    let hp = forecast(
        &api,
        &SkillingForecastQuery {
            target: SkillingTargetKind::Hp,
            professions: Vec::new(),
            ..swordsman_query(200.0, routes)
        },
    )
    .await;
    assert!(hp.hunting_target);
    assert_eq!(hp.routes.len(), 1);
}

/// Recorded kills: `count` kills by `tool` of the 2 HP mob two seconds
/// apart from `start`, each costing 2 PEC and preceded by 0.6 PEC of
/// `skill`, in session `session` (ended unless `live`).
#[allow(clippy::too_many_arguments)]
async fn seed_kills(
    db: &Db,
    session: &str,
    definition: Option<i64>,
    live: bool,
    tool: &str,
    skill: &str,
    start: f64,
    count: usize,
) {
    let (session, tool, skill) = (session.to_string(), tool.to_string(), skill.to_string());
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO tracking_sessions \
               (id, started_at, ended_at, is_active, skill_boost_percent, definition_id) \
             VALUES (?1, ?2, ?3, ?4, 50, ?5)",
            rusqlite::params![
                session,
                start,
                (!live).then_some(start + 10_000.0),
                i64::from(live),
                definition
            ],
        )?;
        for index in 0..count {
            let timestamp = start + 2.0 * index as f64;
            let id = format!("{session}-{tool}-{index}");
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
                   shots_fired, damage_dealt, cost_ped) \
                 VALUES (?1, ?2, 'Bank Robber', 'Bank Robber', 'Default', ?3, 1, 3.0, 0.02)",
                rusqlite::params![id, session, timestamp],
            )?;
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, cost_per_shot) \
                 VALUES (?1, ?2, 1, 3.0, 0.02)",
                rusqlite::params![id, tool],
            )?;
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?1, ?2, ?3, 0.1, 0.006)",
                rusqlite::params![session, timestamp - 0.5, skill],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// Once the player has recorded enough kills of a mob, a route on it takes
/// their own time between kills (with the weapon's firing time taken out)
/// in place of the fixed allowance. Ended sessions count from their
/// settled rollups and the live one from its raw kills.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_pace_on_a_mob_replaces_the_allowance() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;
    let routes = vec![route("sword", "Bank Robber", "Default")];

    let allowance = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    assert!(allowance.routes[0].pace.is_none());
    let firing = allowance.routes[0].seconds_per_kill.unwrap() - 1.0;

    // 60 kills: 59 gaps, too few to stand in.
    seed_kills(
        &db,
        "a",
        None,
        false,
        "Laconian Sword",
        "Longblades",
        1.7e9,
        60,
    )
    .await;
    let thin = forecast(&api, &swordsman_query(20.0, routes.clone())).await;
    assert!(thin.routes[0].pace.is_none());

    // A live session's kills join the ended one's: 59 + 59 gaps.
    seed_kills(
        &db,
        "b",
        None,
        true,
        "Laconian Sword",
        "Longblades",
        1.8e9,
        60,
    )
    .await;
    let paced = forecast(&api, &swordsman_query(20.0, routes)).await;
    let route = &paced.routes[0];
    let pace = route.pace.as_ref().expect("recorded pace");
    assert_eq!(pace.kills, 118);
    // Two seconds from kill to kill, less the one recorded swing at the
    // sword's 67 a minute (0.9 s).
    assert!((pace.between_kill_seconds - 1.1).abs() < 0.01);
    // The route fires at its modelled pace, then waits the recorded time.
    let seconds = route.seconds_per_kill.unwrap();
    assert!((seconds - (firing + pace.between_kill_seconds)).abs() < 0.01);
}

async fn seed_definition(db: &Db, id: i64, name: &str) {
    let name = name.to_string();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_definitions (id, name) VALUES (?1, ?2)",
            rusqlite::params![id, name],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// A named session played with two weapons forecasts whole, or narrowed to
/// one weapon's part: that weapon's own kills, their cost, the skill gains
/// that led up to them, and its share of the session's time. The weapons
/// the named sessions used are offered, most used first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_sessions_narrow_to_one_weapon() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;
    seed_definition(&db, 7, "Mixed Skilling").await;
    let start = 1.7e9;
    seed_kills(
        &db,
        "m",
        Some(7),
        false,
        "Laconian Sword",
        "Longblades",
        start,
        150,
    )
    .await;
    seed_kills(
        &db,
        "m",
        Some(7),
        false,
        "SET-P1 Civilian Sidearm, Adjusted",
        "Handgun",
        start + 300.0,
        120,
    )
    .await;

    let whole = forecast(&api, &swordsman_query(20.0, Vec::new())).await;
    let names: Vec<(&str, u64)> = whole
        .session_weapons
        .iter()
        .map(|weapon| (weapon.name.as_str(), weapon.kills))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Laconian Sword", 150),
            ("SET-P1 Civilian Sidearm, Adjusted", 120)
        ]
    );
    let whole_source = &whole.sources[0];
    assert_eq!(whole_source.name, "Mixed Skilling");
    assert!((whole_source.sample.pes - 270.0 * 0.006).abs() < 1e-6);

    let sword = forecast(
        &api,
        &SkillingForecastQuery {
            session_weapon: Some("Laconian Sword".into()),
            ..swordsman_query(20.0, Vec::new())
        },
    )
    .await;
    let source = &sword.sources[0];
    // Only the sword's kills and the Longblades gains before them.
    assert!((source.sample.pes - 150.0 * 0.006).abs() < 1e-6);
    assert!((source.sample.cycled_ped - 150.0 * 0.02).abs() < 1e-6);
    // The sword's 149 gaps against the pistol's 120 (its first kill came
    // two seconds after the sword's last): its share of the session's
    // 10,000 seconds.
    let share = 149.0 / (149.0 + 120.0);
    assert!((source.sample.hours - 10_000.0 / 3600.0 * share).abs() < 0.01);

    let none = forecast(
        &api,
        &SkillingForecastQuery {
            session_weapon: Some("Unheard Of".into()),
            ..swordsman_query(20.0, Vec::new())
        },
    )
    .await;
    assert!(none.sources.is_empty());
}

/// A named session's expected loss prices its weapons with the engine the
/// routes use, at the planning looter basis, beside the recorded loss.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_sessions_price_an_expected_loss_like_a_route() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, standard_levels()).await;
    seed_definition(&db, 7, "Mixed Skilling").await;
    let start = 1.7e9;
    seed_kills(
        &db,
        "m",
        Some(7),
        false,
        "Laconian Sword",
        "Longblades",
        start,
        150,
    )
    .await;
    seed_kills(
        &db,
        "m",
        Some(7),
        false,
        "SET-P1 Civilian Sidearm, Adjusted",
        "Handgun",
        start + 300.0,
        120,
    )
    .await;

    let sword_query = |looter_level: Option<f64>| SkillingForecastQuery {
        session_weapon: Some("Laconian Sword".into()),
        looter_level,
        ..swordsman_query(20.0, vec![route("sword", "Carabok", "Puny")])
    };
    let sword = forecast(&api, &sword_query(Some(40.0))).await;
    let source = &sword.sources[0];
    let expected = source.expected.expect("the sword is priced");
    let route = &sword.routes[0];
    let route_return = 1.0 - route.loss_per_kill_pec / route.cycled_per_kill_pec;
    assert!((expected.weapon_return - route_return).abs() < 1e-4);
    assert_eq!(expected.coverage, 1.0);
    // All of the narrowed cycling is the sword's.
    assert!((expected.tt_cost - source.cycled_ped * (1.0 - expected.weapon_return)).abs() < 0.02);
    // The recorded loss stands beside it at the recorded return.
    assert!((source.tt_cost - (source.cycled_ped - source.loot_tt)).abs() < 0.011);

    // A higher looter basis expects more back.
    let richer = forecast(&api, &sword_query(Some(80.0))).await;
    let richer = richer.sources[0].expected.expect("priced");
    assert!(richer.weapon_return > expected.weapon_return);

    // Whole, both weapons are priced by their spend.
    let whole = forecast(
        &api,
        &SkillingForecastQuery {
            looter_level: Some(40.0),
            ..swordsman_query(20.0, Vec::new())
        },
    )
    .await;
    let whole = whole.sources[0].expected.expect("priced");
    assert_eq!(whole.coverage, 1.0);
    assert!(whole.weapon_return > expected.weapon_return);
}
