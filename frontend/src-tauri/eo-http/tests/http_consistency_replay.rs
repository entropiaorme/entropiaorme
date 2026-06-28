//! HTTP consistency replay: the four `consistency_*_midpoint` scenarios
//! replayed through the full native pipeline, then their committed
//! HTTP-response goldens (`expected/http_responses/<endpoint_id>.json`)
//! re-asserted byte-for-byte over a live substrate, with no Python.
//!
//! These goldens were previously consumed only by a now-retired Python
//! consistency suite. This test preserves that equivalence evidence: it
//! drives each scenario's chat-log segments through the real watcher ->
//! bus -> tracker -> database pipeline, stops the session (the "midpoint"
//! name notwithstanding, the goldens capture the end-of-scenario, idle
//! hydration shape), then drives the read + producer surface in-memory
//! through `build_router(state).oneshot` and fingerprints ten endpoints
//! through the same `eo_wire::http_fingerprint` emitter the goldens were
//! banked with.
//!
//! The goldens are frozen evidence: this test only READS and ASSERTS
//! them. It does not regenerate or modify any golden file.
//!
//! The replay protocol mirrors `eo-services/tests/corpus_replay_oracle.rs`:
//! a frozen, driver-advanced clock from the committed plan; lines streamed
//! one flush per timestamp tick so the tail never observes end-of-file
//! inside a tick; a drain barrier on the cumulative line count after each
//! segment; one plan step before the session stops. The transport mirrors
//! `eo-http/tests/native_router.rs` (the router driven in-memory via
//! `build_router(state).oneshot`, no socket), and the capture half mirrors
//! `eo-wire/tests/emitters_proof.rs` (one shared `Normalizer` walked over
//! the fixed endpoint order, `serialize_capture(capture(..))` per response).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use chrono::NaiveDateTime;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
use eo_services::tracker::{HuntTracker, Providers};
use eo_wire::http_fingerprint::{self, RawResponse};
use eo_wire::normalizer::Normalizer;
use http_body_util::BodyExt;
use serde_json::{Map, Value};
use sqlx::Row;
use tower::ServiceExt;

/// The scripted-corpus base, a sibling of this crate (`eo-http`) under
/// `frontend/src-tauri`, exactly as the sibling `eo-wire` test resolves it.
fn corpus_scripted() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/corpus/scripted")
}

// ── The committed clock plan (copied from the corpus replay oracle) ──

/// The committed clock plan: a frozen start instant the driver advances
/// by one step before the session stops.
struct ClockPlan {
    start: NaiveDateTime,
    step_seconds: f64,
}

fn load_clock_plan(scenario: &Path) -> ClockPlan {
    let metadata =
        std::fs::read_to_string(scenario.join("metadata.yaml")).expect("scenario metadata");
    let mut in_clock = false;
    let mut start = None;
    let mut step_seconds = None;
    for line in metadata.lines() {
        if line.trim_end() == "clock:" {
            in_clock = true;
            continue;
        }
        if in_clock {
            let trimmed = line.trim();
            if let Some(raw) = trimmed.strip_prefix("start:") {
                start = NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%dT%H:%M:%S").ok();
            } else if let Some(raw) = trimmed.strip_prefix("step_seconds:") {
                step_seconds = raw.trim().parse::<f64>().ok();
            } else if !line.starts_with(' ') && !trimmed.is_empty() {
                in_clock = false;
            }
        }
    }
    ClockPlan {
        start: start.expect("clock plan start"),
        step_seconds: step_seconds.expect("clock plan step"),
    }
}

/// The tick-grouping key: the line's leading chat-log timestamp.
fn tick_key(line: &str) -> Option<&str> {
    let candidate = line.trim_start().get(0..19)?;
    NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(candidate)
}

/// Group consecutive lines sharing one tick key, untimestamped lines
/// riding with the line before them, exactly as the harness streams: a
/// tick is the atomic flush unit, so the tail loop can never see
/// end-of-file in the middle of one.
fn tick_groups(content: &str) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    let mut group = String::new();
    let mut current: Option<String> = None;
    for line in content.split_inclusive('\n') {
        let key = tick_key(line).map(str::to_string);
        if !group.is_empty() {
            if let Some(key) = &key {
                if current.as_ref() != Some(key) {
                    groups.push(std::mem::take(&mut group));
                }
            }
        }
        group.push_str(line);
        if key.is_some() {
            current = key;
        }
    }
    if !group.is_empty() {
        groups.push(group);
    }
    groups
}

fn first_divergence(expected: &str, actual: &str) -> String {
    for (index, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        if want != got {
            return format!(
                "first divergence at line {}:\n  expected: {want}\n  actual:   {got}",
                index + 1
            );
        }
    }
    format!(
        "line counts differ: expected {}, actual {}",
        expected.lines().count(),
        actual.lines().count()
    )
}

// ── The in-memory transport (mirrors the native-router test) ─────────

/// Dispatch a GET through a freshly built router (`oneshot` consumes the
/// router, so each request gets its own), capturing the same
/// (status, headers, body) tuple a socket round-trip produced.
async fn get(state: &Arc<AppState>, path: &str) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .expect("request builds");
    let response = eo_http::build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

/// Project the response headers into the `Map<String, Value>` the
/// fingerprint emitter's `RawResponse` expects (the emitter lower-cases
/// and projects to its pinned three; an unrepresentable value is dropped
/// exactly as a non-string header would be).
fn headers_as_map(headers: &http::HeaderMap) -> Map<String, Value> {
    let mut map = Map::new();
    for (name, value) in headers {
        if let Ok(text) = value.to_str() {
            map.insert(name.as_str().to_string(), Value::String(text.to_string()));
        }
    }
    map
}

/// The curated ten endpoints, in the fixed capture order the shared
/// symbol table depends on, with the live session id substituted into the
/// two session-scoped paths. The `endpoint_id` is the golden file stem.
fn endpoint_table(session_id: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "GET_tracking_snapshot",
            "/api/tracking/snapshot".to_string(),
        ),
        (
            "GET_tracking_sessions",
            "/api/tracking/sessions".to_string(),
        ),
        (
            "GET_tracking_session_detail",
            format!("/api/tracking/session/{session_id}"),
        ),
        (
            "GET_tracking_session_quest_link_suggestion",
            format!("/api/tracking/session/{session_id}/quest-link-suggestion"),
        ),
        ("GET_quests", "/api/quests".to_string()),
        ("GET_quests_mobs", "/api/quests/mobs".to_string()),
        ("GET_quests_analytics", "/api/quests/analytics".to_string()),
        ("GET_quests_playlists", "/api/quests/playlists".to_string()),
        (
            "GET_scan_skills_status",
            "/api/scan/skills/status".to_string(),
        ),
        (
            "GET_codex_meta_attributes",
            "/api/codex/meta/attributes".to_string(),
        ),
    ]
}

/// Replay one consistency scenario through the full native pipeline, then
/// serve the read + producer surface and assert every committed
/// HTTP-response golden byte-for-byte.
async fn assert_consistency_goldens(scenario_name: &str) {
    let scenario = corpus_scripted().join(scenario_name);
    let plan = load_clock_plan(&scenario);

    // The replay pipeline over a fresh temp database and an empty chatlog
    // the watcher tails. The producer pieces use DEFAULT providers, so the
    // goldens' inert default-provider state (mob "Unknown", weapon
    // "Unknown", zeroed cost breakdown) is reproduced.
    let db_dir = tempfile::tempdir().expect("scenario tempdir");
    let db = Db::open(&db_dir.path().join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let pool = db.pool().clone();

    let chatlog = db_dir.path().join("chat_testing.log");
    std::fs::File::create(&chatlog).expect("empty chatlog");

    let bus = Arc::new(EventBus::new());
    let clock = Arc::new(MockClock::new(Some(plan.start), 0.0));
    let watcher = ChatlogWatcher::new(bus.clone(), &chatlog, None);
    watcher.start();

    let tracker = HuntTracker::new(
        bus.clone(),
        pool.clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
        Providers {
            player_name: String::new(),
            ..Providers::default()
        },
    )
    .expect("tracker");
    tracker.start_session().expect("session start");

    // Stream each segment one tick per flush, draining on the cumulative
    // line count after each (the watcher counts every whole line read).
    let mut cumulative = 0u64;
    for segment in ["chat_replay.log", "chat_replay_after.log"] {
        let path = scenario.join(segment);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("chat segment");
        cumulative += content.split_inclusive('\n').count() as u64;
        {
            let mut sink = std::fs::OpenOptions::new()
                .append(true)
                .open(&chatlog)
                .expect("chatlog append");
            for group in tick_groups(&content) {
                sink.write_all(group.as_bytes()).expect("tick write");
                sink.flush().expect("tick flush");
            }
        }
        // The drain wait is a blocking condvar park; yield the worker via
        // block_in_place so the runtime keeps polling the tracker's
        // database futures while the watcher thread drains.
        let total = cumulative;
        tokio::task::block_in_place(|| watcher.wait_until_drained(total, Duration::from_secs(10)))
            .expect("watcher drains the segment");
    }

    clock.advance(plan.step_seconds).expect("plan step");
    tracker.stop_session().expect("session stop");
    watcher.stop();

    // The single persisted session id, substituted into the session-scoped
    // golden paths.
    let session_id: String = sqlx::query("SELECT id FROM tracking_sessions LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("the replay persisted exactly one session")
        .get("id");

    // The read + producer substrate over the SAME pool and the SAME live
    // tracker: an empty game-data store, a manual skill-scan whose engine
    // reports available (status configured:true), and a hotbar listener
    // composed but never started (is_running()==false).
    let hydration_dir = tempfile::tempdir().expect("hydration data dir");
    let game_data_dir = tempfile::tempdir().expect("game data dir");
    let dev_data_dir = tempfile::tempdir().expect("dev data dir");
    let game_data = Arc::new(GameDataStore::new(game_data_dir.path()).expect("empty store"));
    let hydration = Arc::new(HydrationState::new(
        Db::from_pool(pool.clone()),
        game_data,
        clock.clone(),
        hydration_dir.path().to_path_buf(),
    ));
    let scan = SkillScanManual::new(
        ScanProviders {
            engine_available: Arc::new(|| true),
            ..ScanProviders::default()
        },
        clock.clone(),
        None,
        None,
        0,
    );
    let hotbar = HotbarListener::new(bus.clone(), None, None);
    assert!(
        !hotbar.is_running(),
        "the hotbar listener is composed but never started (snapshot hotbarListenerActive:false)"
    );

    let state = Arc::new(
        AppState::new(0)
            .with_hydration(hydration)
            .with_tracker(tracker.clone())
            .with_skill_scan(scan)
            .with_hotbar_listener(hotbar)
            .with_cors(CorsConfig::new(5173, None))
            .with_data_dir(dev_data_dir.path().to_path_buf()),
    );

    // Capture the ten endpoints in the fixed order under one shared
    // Normalizer, fingerprinting each through the same emitter the goldens
    // were banked with, and asserting byte-equality.
    let endpoints = endpoint_table(&session_id);
    let mut normalizer = Normalizer::new();
    let empty_query: Map<String, Value> = Map::new();
    for (endpoint_id, path) in &endpoints {
        let (status, headers, body) = get(&state, path).await;
        assert_eq!(
            status,
            http::StatusCode::OK,
            "{scenario_name}: {endpoint_id} ({path}) did not answer 200"
        );
        let header_map = headers_as_map(&headers);
        let raw = RawResponse {
            method: "GET",
            path: path.as_str(),
            query: &empty_query,
            status_code: 200,
            headers: &header_map,
            body: &body,
        };
        let actual =
            http_fingerprint::serialize_capture(&http_fingerprint::capture(&raw, &mut normalizer));
        let golden = scenario
            .join("expected/http_responses")
            .join(format!("{endpoint_id}.json"));
        let expected = std::fs::read_to_string(&golden)
            .unwrap_or_else(|e| panic!("read golden {}: {e}", golden.display()));
        assert_eq!(
            actual,
            expected,
            "{scenario_name}: {endpoint_id} diverged from its golden\n{}",
            first_divergence(&expected, &actual)
        );
    }
}

/// Guard the capture cardinality: the shared symbol table grows in
/// encounter order, so the curated set must stay at exactly ten endpoints
/// in the fixed order, mirroring the retired Python contract's pin.
#[test]
fn the_endpoint_set_is_the_fixed_ten() {
    assert_eq!(
        endpoint_table("session-id").len(),
        10,
        "the shared symbol table depends on capturing exactly the curated ten endpoints"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consistency_tracking_hunt_midpoint_http_replay_matches_goldens() {
    assert_consistency_goldens("consistency_tracking_hunt_midpoint").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consistency_quests_mission_lifecycle_midpoint_http_replay_matches_goldens() {
    assert_consistency_goldens("consistency_quests_mission_lifecycle_midpoint").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consistency_scan_isolation_midpoint_http_replay_matches_goldens() {
    assert_consistency_goldens("consistency_scan_isolation_midpoint").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consistency_codex_isolation_midpoint_http_replay_matches_goldens() {
    assert_consistency_goldens("consistency_codex_isolation_midpoint").await;
}
