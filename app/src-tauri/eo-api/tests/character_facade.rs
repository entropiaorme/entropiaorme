//! Behavioural pins for the character family over the typed facade: the
//! calibration staleness read, the stats truncation and ranking, the
//! skill/profession shaping with scan-anchored gains, and the optimizer
//! surfaces, plus the two ratified not-found convergences.

use std::path::Path;
use std::sync::Arc;

use eo_api::Api;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::{json, Value};

mod common;

fn write_fixture(dir: &Path, name: &str, value: &Value) {
    std::fs::write(dir.join(name), serde_json::to_string(value).unwrap()).unwrap();
}

/// The composed facade over a fresh migrated database seeded with a few
/// calibrations and a small catalogue, and a clock frozen days past the
/// latest scan (inside the 30-day window).
async fn seeded_api(dir: &Path) -> Api {
    seeded_api_with_db(dir).await.0
}

/// [`seeded_api`] plus the facade's database handle, for tests that seed
/// recorded play beyond the calibrations.
async fn seeded_api_with_db(dir: &Path) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    write_fixture(
        &snapshot,
        "professions.json",
        &json!([
            {"name": "Marksman", "category": "Combat", "skills": [
                {"weight": 40, "skill": {"name": "Rifle"}},
                {"weight": 10, "skill": {"name": "Anatomy"}},
            ]},
            {"name": "Healer", "skills": [
                {"weight": 50, "skill": {"name": "Anatomy"}},
            ]},
        ]),
    );
    write_fixture(
        &snapshot,
        "skills.json",
        &json!([
            {"name": "Rifle", "category": {"name": "Combat"}},
            {"name": "Anatomy", "category": {"name": "Medical"}},
            {"name": "Health"},
        ]),
    );
    write_fixture(
        &snapshot,
        "skill_ranks.json",
        &json!({"table": {"rows": [
            {"name": "Adept", "skill": 1000},
            {"name": "Novice", "skill": 0},
            {"name": "Broken", "skill": null},
            {"name": null, "skill": 5},
        ]}}),
    );
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let seed_rows: Vec<(String, f64, String, f64)> = [
        ("Rifle", 1200.0, "scan", 1700000000.5),
        ("Rifle", 1250.0, "chatlog", 1700003600.0),
        ("Anatomy", 800.0, "scan", 1700000000.5),
        ("Health", 142.7, "scan", 1700000000.5),
    ]
    .into_iter()
    .map(|(name, level, source, ts)| (name.to_string(), level, source.to_string(), ts))
    .collect();
    db.with_writer(move |conn| {
        for (name, level, source, ts) in &seed_rows {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![name, level, source, ts],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let clock = Arc::new(MockClock::new(
        Some(
            chrono::NaiveDateTime::parse_from_str("2023-11-20 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        ),
        0.0,
    ));
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let api = Api::new(
        db.clone(),
        Arc::new(GameDataStore::new(&snapshot).unwrap()),
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
    );
    (api, db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_facade_shapes_the_seeded_state() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // Calibration: the believed-latest timestamp in UTC ISO, the frozen
    // clock inside the 30-day staleness window.
    let calibration = api.character_calibration().await.unwrap();
    assert!(calibration.calibrated);
    assert_eq!(
        calibration.last_calibration.as_deref(),
        Some("2023-11-14T23:13:20+00:00")
    );
    assert!(!calibration.stale);

    // Stats: Python int() truncation of Health, professions ranked.
    let stats = api.character_stats().await.unwrap();
    assert_eq!(stats.hp, 142);
    assert_eq!(stats.top_professions.len(), 2);
    assert_eq!(stats.top_professions[0].name, "Marksman");
    assert_eq!(stats.top_professions[0].category, "Combat");
    assert_eq!(stats.top_professions[1].name, "Healer");
    assert_eq!(stats.top_professions[1].category, "General");

    // Skills: believed-current levels, anchors, gains, ranks, TT.
    let skills = api.character_skills().await.unwrap();
    let rifle = &skills[0];
    assert_eq!(rifle.name, "Rifle");
    assert_eq!(rifle.category, "Combat");
    assert_eq!(rifle.level, 1250.0);
    assert_eq!(rifle.anchor_level, Some(1200.0));
    assert_eq!(rifle.gain_since_anchor, Some(50.0));
    assert_eq!(rifle.rank_name, "Adept");
    assert_eq!(
        rifle.tt_value,
        eo_wire::normalizer::round_half_even(eo_services::tt_value_curve::tt_value_at(1250.0), 2)
    );
    assert!(!rifle.is_attribute);
    let health = &skills[2];
    assert_eq!(health.name, "Health");
    assert_eq!(health.category, "General");
    assert!(health.is_attribute);

    // Professions: anchor levels computed over the scan snapshot.
    let professions = api.character_professions().await.unwrap();
    let marksman = &professions[0];
    assert_eq!(marksman.name, "Marksman");
    let level = marksman.level;
    let anchor = marksman.anchor_level.unwrap();
    assert!(level > anchor, "the chatlog gain moves believed-current");
    assert_eq!(
        marksman.gain_since_anchor,
        Some(eo_wire::normalizer::round_half_even(level - anchor, 4))
    );

    // Both path-optimizer modes carry their mode inputs (the other input
    // echoes null, present).
    let target = api
        .character_path_optimizer(&["Marksman".to_string()], Some(7.0), None)
        .await
        .unwrap();
    assert_eq!(target.mode, "target");
    assert_eq!(target.input_target_level, Some(7.0));
    assert_eq!(target.input_ped_budget, None);
    let target_bytes = serde_json::to_value(&target).unwrap();
    assert_eq!(
        target_bytes["inputPedBudget"],
        Value::Null,
        "the unused mode echoes null"
    );
    let budget = api
        .character_path_optimizer(&["Marksman".to_string()], None, Some(25.0))
        .await
        .unwrap();
    assert_eq!(budget.mode, "budget");
    assert_eq!(budget.input_ped_budget, Some(25.0));

    // Several professions optimise as one combined target, levelled as the
    // sum of theirs and named by its members.
    let family = api
        .character_path_optimizer(
            &["Marksman".to_string(), "Healer".to_string()],
            Some(11.0),
            None,
        )
        .await
        .unwrap();
    assert!(family.error.is_none());
    assert_eq!(family.profession, "Marksman, Healer");
    assert_eq!(family.current_level, 9.8);
    assert!(family.end_level >= 11.0);
    assert!(api
        .character_path_optimizer(&[], Some(11.0), None)
        .await
        .is_err());

    // The HP optimizer reconciles current HP to the truncated Health
    // skill (the Stats-panel reading).
    let hp = api.character_hp_optimizer().await.unwrap();
    assert_eq!(hp.current_hp, 142.0);
}

/// The computed reads over the family's richest responses carry the
/// expected figures: the skill list's levels and ranks, and the HP
/// optimizer's current reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_computed_reads_carry_the_expected_figures() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    let skills = api.character_skills().await.unwrap();
    assert_eq!(skills.len(), 3);
    let rifle = &skills[0];
    assert_eq!(rifle.name, "Rifle");
    assert_eq!(rifle.level, 1250.0);
    assert_eq!(rifle.rank_name, "Adept");
    assert_eq!(rifle.tt_value, 4.9);
    assert!(!rifle.is_attribute);
    let anatomy = &skills[1];
    assert_eq!(anatomy.name, "Anatomy");
    assert_eq!(anatomy.level, 800.0);
    assert_eq!(anatomy.rank_name, "Novice");
    assert_eq!(anatomy.tt_value, 2.35);
    assert!(!anatomy.is_attribute);
    let health = &skills[2];
    assert_eq!(health.name, "Health");
    assert_eq!(health.level, 142.7);
    assert_eq!(health.rank_name, "Novice");
    assert_eq!(health.tt_value, 0.18);
    assert!(health.is_attribute);

    let hp = api.character_hp_optimizer().await.unwrap();
    assert_eq!(hp.current_hp, 142.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_optimizers_report_a_missing_profession() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // The path optimizer's not-found converges on the full error shape
    // (ratified): the mode inputs echo, the aggregates zero, the error
    // marks the miss.
    let missing = api
        .character_path_optimizer(&["Nope".to_string()], Some(7.0), None)
        .await
        .unwrap();
    assert_eq!(
        missing.error.as_deref(),
        Some("Profession 'Nope' not found")
    );
    assert_eq!(missing.mode, "target");
    assert_eq!(missing.input_target_level, Some(7.0));
    assert_eq!(missing.current_level, 0.0);
    assert!(missing.allocations.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_activity_recommender_ranks_arbitrage_and_validates() {
    use eo_api::character::{ActivityRecommenderQuery, RecommenderTargetKind};

    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // Healer trains Anatomy at weight 50 while Marksman holds it at 10:
    // the arbitrage the recommender exists to surface. Marksman itself
    // is the direct reference, excluded from the candidates.
    let query = ActivityRecommenderQuery {
        target: RecommenderTargetKind::Profession,
        professions: vec!["Marksman".into()],
    };
    let result = api.character_activity_recommender(&query).await.unwrap();
    assert!(result.error.is_none());
    assert_eq!(result.pes_cap, 1000.0);
    assert_eq!(result.sample_step, 20.0);
    let direct = result.direct.as_ref().expect("direct reference present");
    assert_eq!(direct.activity, "Marksman");
    assert_eq!(result.candidates.len(), 1);
    let healer = &result.candidates[0];
    assert_eq!(healer.activity, "Healer");
    assert_eq!(healer.professions, vec!["Healer".to_string()]);
    assert!(healer.gain_at_cap > 0.0);
    assert_eq!(healer.series.len(), 51);
    // The whole transfer rides Anatomy (Rifle is not on Healer's panel).
    assert_eq!(healer.contributors.len(), 1);
    assert_eq!(healer.contributors[0].name, "Anatomy");

    // A profession target with no professions is a bad request.
    let query = ActivityRecommenderQuery {
        target: RecommenderTargetKind::Profession,
        professions: Vec::new(),
    };
    assert!(api.character_activity_recommender(&query).await.is_err());

    // A missing profession converges on the family's soft-error shape.
    let query = ActivityRecommenderQuery {
        target: RecommenderTargetKind::Profession,
        professions: vec!["Nope".into()],
    };
    let missing = api.character_activity_recommender(&query).await.unwrap();
    assert_eq!(
        missing.error.as_deref(),
        Some("Profession 'Nope' not found")
    );
    assert!(missing.candidates.is_empty());
    assert!(missing.direct.is_none());

    // The seeded skills carry no hp_increase, so an HP target finds no
    // candidates and defines no direct activity.
    let query = ActivityRecommenderQuery {
        target: RecommenderTargetKind::Hp,
        professions: Vec::new(),
    };
    let hp = api.character_activity_recommender(&query).await.unwrap();
    assert!(hp.error.is_none());
    assert!(hp.candidates.is_empty());
    assert!(hp.direct.is_none());
}

/// The skilling forecast reads each named session's recorded play: the
/// seeded definition trains Rifle, so it answers a Marksman goal, while a
/// definition that only trained Mining says it does not train the target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_skilling_forecast_answers_from_named_sessions() {
    use eo_api::skilling::{
        SkillingForecastQuery, SkillingForecastStatus, SkillingSampleWarning, SkillingTargetKind,
    };
    use eo_services::session_summary::SUMMARY_VERSION;

    let dir = tempfile::tempdir().unwrap();
    let (api, db) = seeded_api_with_db(dir.path()).await;
    db.with_writer(|conn| {
            conn.execute_batch(
                "INSERT INTO session_definitions (id, name) VALUES (50, 'Rifle Skilling'); \
                 INSERT INTO session_definitions (id, name, is_active) VALUES (51, 'Old Mining', 0);",
            )?;
            let sessions = [
                ("s1", 50, 3.0, 300.0, 270.0, r#"{"Rifle": 6.0, "Anatomy": 2.0}"#),
                ("s2", 50, 2.0, 200.0, 190.0, r#"{"Rifle": 4.0}"#),
                ("s3", 51, 1.0, 80.0, 60.0, r#"{"Mining": 3.0}"#),
            ];
            for (id, definition, hours, cycled, loot, skills) in sessions {
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, definition_id) \
                     VALUES (?1, 1700000000, 1700003600, 0, ?2)",
                    rusqlite::params![id, definition],
                )?;
                conn.execute(
                    "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
                       duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, \
                       heal_cost, dangling_cost, cycled_ped, regular_skill_ped_json, \
                       attribute_levels_json, regular_skill_tt, attribute_levels_total) \
                     VALUES (?1, ?2, 1700000000, 1700003600, ?3, 0, ?4, ?5, 0, 0, 0, 0, ?5, ?6, '{}', 0, 0)",
                    rusqlite::params![id, SUMMARY_VERSION, hours, loot, cycled, skills],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    let query = SkillingForecastQuery {
        target: SkillingTargetKind::Profession,
        professions: vec!["Marksman".into()],
        goal: 6.0,
    };
    let result = api.character_skilling_forecast(&query).await.unwrap();
    assert!(result.error.is_none());
    assert_eq!(result.goal, 6.0);
    assert_eq!(result.current, 5.8);
    assert_eq!(result.sources.len(), 2);

    let rifle = &result.sources[0];
    assert_eq!(rifle.name, "Rifle Skilling");
    assert_eq!(rifle.status, SkillingForecastStatus::Ready);
    assert_eq!(rifle.sample.sessions, 2);
    assert_eq!(rifle.sample.cycled_ped, 500.0);
    assert_eq!(rifle.sample.pes, 12.0);
    // Nothing sold: markup is absent, never a zero lift.
    assert_eq!(rifle.sample.realised_markup, None);
    assert_eq!(rifle.markup, None);
    assert_eq!(rifle.net_cost, None);
    assert!(rifle.cycled_ped > 0.0);
    assert!(rifle.tt_cost > 0.0);
    assert_eq!(rifle.skills[0].name, "Rifle");
    assert!(rifle.skills[0].moves_target);
    assert_eq!(rifle.warnings, vec![SkillingSampleWarning::ThinSessions]);

    let mining = &result.sources[1];
    assert_eq!(mining.name, "Old Mining");
    assert!(mining.archived);
    assert_eq!(mining.status, SkillingForecastStatus::DoesNotTrain);
    assert_eq!(mining.cycled_ped, 0.0);

    // The wire spells the statuses and warnings in snake case.
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["sources"][1]["status"], "does_not_train");
    assert_eq!(wire["sources"][0]["warnings"][0], "thin_sessions");
    assert_eq!(wire["sources"][0]["markup"], Value::Null);

    // HP reads the Health attribute as its current value; the seeded
    // skills carry no HP contribution, so no definition trains it.
    let hp = api
        .character_skilling_forecast(&SkillingForecastQuery {
            target: SkillingTargetKind::Hp,
            professions: Vec::new(),
            goal: 150.0,
        })
        .await
        .unwrap();
    assert_eq!(hp.current, 142.7);
    assert!(hp
        .sources
        .iter()
        .all(|source| source.status == SkillingForecastStatus::DoesNotTrain));

    // A goal already met asks for no cycling from any definition.
    let reached = api
        .character_skilling_forecast(&SkillingForecastQuery {
            goal: 1.0,
            ..query.clone()
        })
        .await
        .unwrap();
    assert!(reached
        .sources
        .iter()
        .all(|source| source.status == SkillingForecastStatus::Reached));

    // A family forecasts as its summed level: Marksman (5.8) plus Healer
    // (Anatomy 800 at weight 50: 4.0) is 9.8, so +0.2 asks for Rifle or
    // Anatomy gains through the combined weights.
    let family = api
        .character_skilling_forecast(&SkillingForecastQuery {
            professions: vec!["Marksman".into(), "Healer".into()],
            goal: 10.0,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(family.current, 9.8);
    assert_eq!(family.sources[0].name, "Rifle Skilling");
    assert_eq!(family.sources[0].status, SkillingForecastStatus::Ready);
    // Anatomy counts toward both members, so it outranks its single-member weight.
    let anatomy = family.sources[0]
        .skills
        .iter()
        .find(|skill| skill.name == "Anatomy")
        .unwrap();
    assert!(anatomy.moves_target);

    // Validation and the soft unknown-profession shape.
    let bad_goal = SkillingForecastQuery {
        goal: 0.0,
        ..query.clone()
    };
    assert!(api.character_skilling_forecast(&bad_goal).await.is_err());
    let unnamed = SkillingForecastQuery {
        professions: Vec::new(),
        ..query.clone()
    };
    assert!(api.character_skilling_forecast(&unnamed).await.is_err());
    let missing = api
        .character_skilling_forecast(&SkillingForecastQuery {
            professions: vec!["Nope".into()],
            ..query
        })
        .await
        .unwrap();
    assert_eq!(
        missing.error.as_deref(),
        Some("Profession 'Nope' not found")
    );
    assert!(missing.sources.is_empty());
}
