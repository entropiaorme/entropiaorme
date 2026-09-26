//! Behavioural pins for the developer-tools family over the typed facade:
//! the developer-mode gate (off => not-found for every command), the
//! metrics snapshot, the crash-reporting round-trip, and the two
//! maintenance actions (compaction, projection rebuild-and-verify).
//!
//! The family is native-only (no Python arm, no corpus golden, no OpenAPI
//! path), so there is no frozen contract to re-pin; the DTO byte-shapes
//! are pinned as unit tests in `eo_api::dev`. These tests cover the
//! facade's own contract: the fresh-read gate and each command's success
//! leg over a real composed database.

use std::path::Path;
use std::sync::Arc;

use eo_api::settings::SettingsPatch;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

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
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
    );
    (api, data_dir)
}

/// Write `settings.json` so the fresh-read developer-mode gate reads the
/// given state (the file the gate consults on each call).
fn set_developer_mode(data_dir: &Path, enabled: bool) {
    std::fs::write(
        data_dir.join("settings.json"),
        format!(r#"{{"developer_mode_enabled":{enabled}}}"#),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_dev_command_is_not_found_when_developer_mode_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, false);

    assert!(matches!(api.dev_metrics(), Err(ApiError::NotFound { .. })));
    assert!(matches!(
        api.dev_crash_reporting(),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_set_crash_reporting(true),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_compact_database().await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_rebuild_projections().await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_auction_fee_research_start(),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_auction_fee_research_status(),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_auction_fee_research_overlay_status(),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_auction_fee_research_capture(),
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.dev_auction_fee_research_stop(),
        Err(ApiError::NotFound { .. })
    ));
    // The gate-off contract holds even with no settings file at all.
    std::fs::remove_file(data_dir.join("settings.json")).unwrap();
    assert!(matches!(api.dev_metrics(), Err(ApiError::NotFound { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auction_fee_research_is_a_dev_gated_session_with_a_least_privilege_overlay_view() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);

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
async fn disabling_developer_mode_authoritatively_stops_an_active_research_session() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);
    api.dev_auction_fee_research_start()
        .expect("research starts");
    assert!(api.auction_fee_research_active_for_shell());

    api.settings_update(SettingsPatch {
        developer_mode_enabled: Some(false),
        ..SettingsPatch::default()
    })
    .await
    .expect("developer mode disables");

    assert!(!api.auction_fee_research_active_for_shell());
    assert!(matches!(
        api.dev_auction_fee_research_status(),
        Err(ApiError::NotFound { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_metrics_snapshot_reads_under_developer_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);

    // Record onto the process registry so the snapshot is non-trivial.
    eo_wire::metrics::metrics().record_event_published();
    let snapshot = api.dev_metrics().expect("metrics under dev mode");
    assert!(snapshot.events_published >= 1);
    // Every instrumented histogram is present with its bucket vector.
    assert!(!snapshot.ocr_latency.buckets.is_empty());
    assert!(!snapshot.db_query_latency.buckets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_crash_reporting_toggle_round_trips_under_developer_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);

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
async fn compaction_writes_a_copy_under_developer_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);

    let result = api.dev_compact_database().await.expect("compaction");
    assert!(
        result.bytes > 0,
        "the compacted copy has a real size: {result:?}"
    );
    assert!(data_dir.join("entropia_orme-compacted.db").exists());
    assert!(result.path.ends_with("entropia_orme-compacted.db"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_projections_reports_every_model_matching_under_developer_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (api, data_dir) = dev_api(dir.path()).await;
    set_developer_mode(&data_dir, true);

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
