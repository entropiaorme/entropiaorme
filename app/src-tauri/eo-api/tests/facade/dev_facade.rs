//! Behavioural pins for the developer-tools family over the typed facade:
//! that nothing gates it, the metrics snapshot, the crash-reporting
//! round-trip, and the two maintenance actions (compaction, projection
//! rebuild-and-verify).
//!
//! The family is native-only (no Python arm, no corpus golden, no OpenAPI
//! path), so there is no frozen contract to re-pin; the DTO byte-shapes
//! are pinned as unit tests in `eo_api::dev`. These tests cover the
//! facade's own contract: each command's success leg over a real composed
//! database, with no setting needed to reach it.

use std::path::Path;
use std::sync::Arc;

use eo_api::Api;
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

use crate::common;

/// The composed facade over a fresh migrated database and an empty
/// catalogue (the dev family is catalogue-independent).
async fn dev_api(dir: &Path) -> (Api, std::path::PathBuf) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let api = Api::new(
        db,
        game_data,
        clock,
        data_dir.clone(),
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
    );
    (api, data_dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_family_answers_with_no_setting_turned_on() {
    // No settings file at all, and one still carrying the retired
    // developer-mode key turned off: the tools answer either way.
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    api.dev_metrics().expect("metrics with no settings file");
    std::fs::write(
        data_dir.join("settings.json"),
        r#"{"developer_mode_enabled":false}"#,
    )
    .unwrap();
    api.dev_metrics().expect("metrics");
    api.dev_crash_reporting().expect("crash reporting");
    let research = api
        .dev_auction_fee_research_status()
        .expect("research status");
    assert!(!research.active);
    let overlay = api
        .dev_auction_fee_research_overlay_status()
        .expect("overlay status");
    assert!(!overlay.active, "no session: the overlay captures listings");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auction_fee_research_is_a_session_with_a_least_privilege_overlay_view() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = dev_api(dir.path()).await;

    let started = api
        .dev_auction_fee_research_start()
        .expect("research starts");
    assert!(started.active);
    assert_eq!(started.sample_count, 0);
    let output_dir = started.output_dir.0.expect("main window gets output path");
    assert!(Path::new(&output_dir).join("manifest.json").is_file());

    let overlay = api
        .dev_auction_fee_research_overlay_status()
        .expect("overlay status");
    assert!(overlay.active);
    assert_eq!(overlay.sample_count, 0);
    assert_eq!(overlay.message, None);

    let stopped = api.dev_auction_fee_research_stop().expect("research stops");
    assert!(!stopped.active);
    assert_eq!(stopped.output_dir.0.as_deref(), Some(output_dir.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_metrics_snapshot_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = dev_api(dir.path()).await;

    // Record onto the process registry so the snapshot is non-trivial.
    eo_wire::metrics::metrics().record_event_published();
    let snapshot = api.dev_metrics().expect("metrics under dev mode");
    assert!(snapshot.events_published >= 1);
    // Every instrumented histogram is present with its bucket vector.
    assert!(!snapshot.ocr_latency.buckets.is_empty());
    assert!(!snapshot.db_query_latency.buckets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_crash_reporting_toggle_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;

    // Off by default.
    assert!(!api.dev_crash_reporting().unwrap().crash_reporting_enabled);
    // Enabling it round-trips through the shell-owned observability file.
    assert!(
        api.dev_set_crash_reporting(true)
            .unwrap()
            .crash_reporting_enabled
    );
    assert!(api.dev_crash_reporting().unwrap().crash_reporting_enabled);
    // The opt-in lives in observability.json, NOT settings.json (the gate
    // surface): the toggle never touches the equivalence-covered file.
    assert!(data_dir.join("observability.json").exists());
    // Disabling it round-trips back.
    assert!(
        !api.dev_set_crash_reporting(false)
            .unwrap()
            .crash_reporting_enabled
    );
    assert!(!api.dev_crash_reporting().unwrap().crash_reporting_enabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compaction_writes_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;

    let result = api.dev_compact_database().await.expect("compaction");
    assert!(
        result.bytes > 0,
        "the compacted copy has a real size: {result:?}"
    );
    assert!(data_dir.join("entropia_orme-compacted.db").exists());
    assert!(result.path.ends_with("entropia_orme-compacted.db"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_projections_reports_every_model_matching() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = dev_api(dir.path()).await;

    let report = api.dev_rebuild_projections().await.expect("rebuild");
    assert!(report.all_matched, "{report:?}");
    let tables: Vec<&str> = report.tables.iter().map(|t| t.table.as_str()).collect();
    for expected in [
        "session_summaries",
        "daily_rollups",
        "session_kill_rollups",
        "session_loot_rollups",
        "session_pes_rollups",
    ] {
        assert!(
            tables.contains(&expected),
            "expected {expected} among rebuilt projections: {tables:?}"
        );
    }
}
