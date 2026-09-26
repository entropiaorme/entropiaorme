//! Behavioural pins for the character family over the typed facade,
//! ported from the family's HTTP-era hermetic handler tests: the
//! calibration staleness read, the stats truncation and ranking, the
//! skill/profession shaping with scan-anchored gains, and the optimizer
//! surfaces, plus a transport-invariance pin (the typed calibration
//! response serialises to the exact bytes the HTTP route answered) and
//! the two ratified not-found convergences.

use std::path::Path;
use std::sync::Arc;

use eo_api::character::ProspectQuery;
use eo_api::Api;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::{json, Value};

mod common;

fn write_fixture(dir: &Path, name: &str, value: &Value) {
    std::fs::write(dir.join(name), serde_json::to_string(value).unwrap()).unwrap();
}

/// The composed facade over a fresh migrated database seeded with the
/// same calibrations and catalogue the HTTP-era handler test used, and a
/// clock frozen days past the latest scan (inside the 30-day window).
async fn seeded_api(dir: &Path) -> Api {
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
    Api::new(
        db,
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
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_facade_shapes_the_seeded_state() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // Calibration, the transport-invariance pin: the typed response
    // serialises to the exact bytes the HTTP route answered (the
    // believed-latest timestamp in UTC ISO, the frozen clock inside the
    // 30-day staleness window).
    let calibration = api.character_calibration().await.unwrap();
    assert_eq!(
        serde_json::to_string(&calibration).unwrap(),
        "{\"calibrated\":true,\"lastCalibration\":\"2023-11-14T23:13:20+00:00\",\"stale\":false}"
    );

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

    // Prospect options: no recorded sessions in the seed, so every axis
    // is empty.
    let options = api.character_prospect_options().await.unwrap();
    assert!(options.tags.is_empty());
    assert!(options.mobs.is_empty());
    assert!(options.weapons.is_empty());

    // The profession optimizer composes the calc service with the
    // projections; the declared-then-extra order is the wire order, and
    // nextLevel renders as a float.
    let optimizer = api
        .character_profession_optimizer("Marksman")
        .await
        .unwrap();
    assert_eq!(optimizer.profession.as_deref(), Some("Marksman"));
    assert!(optimizer.next_level.is_some());
    assert!(optimizer.error.is_none());
    let serialised = serde_json::to_value(&optimizer).unwrap();
    let keys: Vec<&str> = serialised
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "skills",
            "attributes",
            "profession",
            "currentLevel",
            "nextLevel",
            "gap"
        ]
    );

    // Both path-optimizer modes carry their mode inputs (the other input
    // echoes null, present).
    let target = api
        .character_path_optimizer("Marksman", Some(7.0), None)
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
        .character_path_optimizer("Marksman", None, Some(25.0))
        .await
        .unwrap();
    assert_eq!(budget.mode, "budget");
    assert_eq!(budget.input_ped_budget, Some(25.0));

    // The HP optimizer reconciles current HP to the truncated Health
    // skill (the Stats-panel reading).
    let hp = api.character_hp_optimizer().await.unwrap();
    assert_eq!(hp.current_hp, 142.0);
}

/// Transport invariance on the family's richest responses: the typed
/// DTOs serialise to the exact bytes the HTTP handlers answered on the
/// same seed (captured from the HTTP-era handlers before their deletion).
/// This pins the `float | None` coercions (`nextLevel` 6 -> `6.0`,
/// `codexDivisor` 200 -> `200.0`) and the declared-field wire ordering
/// the DTOs now carry in place of the pydantic projection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_computed_reads_serialise_byte_for_byte_like_the_http_route() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    let optimizer = api
        .character_profession_optimizer("Marksman")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&optimizer).unwrap(),
        "{\"skills\":[{\"name\":\"Rifle\",\"weight\":40.0,\"currentLevel\":1250.0,\"levelsNeeded\":50.0,\
         \"pedToNextLevel\":0.24,\"codexCategory\":\"cat1\",\"codexDivisor\":200.0},\
         {\"name\":\"Anatomy\",\"weight\":10.0,\"currentLevel\":800.0,\"levelsNeeded\":200.0,\
         \"pedToNextLevel\":1.07,\"codexCategory\":\"cat1\",\"codexDivisor\":200.0}],\
         \"attributes\":[],\"profession\":\"Marksman\",\"currentLevel\":5.8,\"nextLevel\":6.0,\"gap\":0.2}"
    );

    let skills = api.character_skills().await.unwrap();
    assert_eq!(
        serde_json::to_string(&skills).unwrap(),
        "[{\"name\":\"Rifle\",\"category\":\"Combat\",\"level\":1250.0,\"anchorLevel\":1200.0,\
         \"gainSinceAnchor\":50.0,\"rankName\":\"Adept\",\"ttValue\":4.9,\"isAttribute\":false},\
         {\"name\":\"Anatomy\",\"category\":\"Medical\",\"level\":800.0,\"anchorLevel\":800.0,\
         \"gainSinceAnchor\":0.0,\"rankName\":\"Novice\",\"ttValue\":2.35,\"isAttribute\":false},\
         {\"name\":\"Health\",\"category\":\"General\",\"level\":142.7,\"anchorLevel\":142.7,\
         \"gainSinceAnchor\":0.0,\"rankName\":\"Novice\",\"ttValue\":0.18,\"isAttribute\":true}]"
    );

    let hp = api.character_hp_optimizer().await.unwrap();
    assert_eq!(
        serde_json::to_string(&hp).unwrap(),
        "{\"currentHp\":142.0,\"skills\":[],\"attributes\":[]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_optimizers_report_a_missing_profession() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // The profession optimizer keeps its minimal not-found shape: empty
    // lists and the error, nothing else.
    let missing = api.character_profession_optimizer("Nope").await.unwrap();
    assert_eq!(
        missing.error.as_deref(),
        Some("Profession 'Nope' not found")
    );
    assert!(missing.skills.is_empty());
    let bytes = serde_json::to_value(&missing).unwrap();
    let keys: Vec<&str> = bytes
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["skills", "attributes", "error"]);

    // The path optimizer's not-found converges on the full error shape
    // (ratified): the mode inputs echo, the aggregates zero, the error
    // marks the miss.
    let missing = api
        .character_path_optimizer("Nope", Some(7.0), None)
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
async fn prospect_validates_and_converges_on_a_missing_profession() {
    let dir = tempfile::tempdir().unwrap();
    let api = seeded_api(dir.path()).await;

    // Value validations survive as bad-request rejections.
    let mut query = ProspectQuery {
        profession: "Marksman".into(),
        target_level: 0.0,
        slice_type: eo_api::character::ProspectSliceType::Global,
        slice_value: None,
        markup_uplift: 0.0,
    };
    assert!(api.character_prospect(&query).await.is_err());
    query.target_level = 10.0;
    query.slice_type = eo_api::character::ProspectSliceType::Mob;
    // A non-global slice with no value is refused.
    assert!(api.character_prospect(&query).await.is_err());

    // A missing profession converges on the full error shape (ratified):
    // the error is present, the echoes and empty sample accompany it.
    query.slice_type = eo_api::character::ProspectSliceType::Global;
    query.profession = "Nope".into();
    let result = api.character_prospect(&query).await.unwrap();
    assert_eq!(result.error.as_deref(), Some("Profession 'Nope' not found"));
    assert_eq!(result.profession, "Nope");
    assert_eq!(result.slice_type, "global");
    assert_eq!(result.speculative_loot_tt, None);
    assert!(result.rows.is_empty());
    assert_eq!(result.sample.sessions, 0);
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
