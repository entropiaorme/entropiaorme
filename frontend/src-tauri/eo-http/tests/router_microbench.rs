//! In-process router micro-benchmark: the post-collapse dispatch latency over
//! the eleven hydration endpoints the performance baseline pins, against a
//! freshly replayed `basic_hunt_10_events` state.
//!
//! This is the transport-stripped measurement of the path the `api_request`
//! command runs in production: [`dispatch_in_process`] builds the request,
//! oneshots `build_router` through the full guard / CORS / observe stack, and
//! reads the body back, with no socket and no loopback hop in front of it. It
//! mirrors the baseline capture script's HTTP leg one-for-one: the same
//! endpoint set and order, three warm-ups, then a fixed sample of timed calls
//! per endpoint, reported as median (p50) / p95 / min / max in milliseconds.
//!
//! It is `#[ignore]`d on purpose: it is a measurement harness, not a
//! correctness gate, so it compiles under the normal suite (it cannot rot) but
//! only runs when asked. Run it with:
//!
//! ```text
//! cargo test -p eo-http --release --test router_microbench -- --ignored --nocapture
//! ```
//!
//! The committed `backend/architecture/port_baseline.json` HTTP figures are a
//! Linux, over-the-socket, Python capture: a different host, transport, and
//! language, so they are a drift-only cross-reference, never a same-axis
//! before/after. The headline pair is the same-host one the maintainer
//! completes in the quiesced session (the installed v0.1.0 over its socket vs
//! this binary), and is assembled there.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use eo_http::hydration::HydrationState;
use eo_http::{dispatch_in_process, AppState};
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::{MockClock, RealClock};
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
use eo_services::tracker::{HuntTracker, Providers};
use serde_json::Value;

const WARMUPS: usize = 3;
const SAMPLES: usize = 30;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scenario_dir() -> PathBuf {
    repo_root().join("backend/tests/e2e/corpus/scripted/basic_hunt_10_events")
}

/// The committed clock plan: a frozen start instant the driver advances by one
/// step before the session stops (copied from the corpus-replay oracle so the
/// seed reaches the identical state the goldens pin).
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

fn tick_key(line: &str) -> Option<&str> {
    let candidate = line.trim_start().get(0..19)?;
    NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(candidate)
}

/// Group consecutive lines sharing one tick key so each flush is one atomic
/// tick (the tail loop never sees end-of-file mid-tick), exactly as the oracle
/// streams a scenario.
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

/// Replay `basic_hunt_10_events` into a migrated temp database, returning the
/// opened handle (its writes committed) alongside the live tracker and bus.
/// The tracker and bus are kept alive by the caller because the snapshot read
/// is served through the composed tracker (and an inert hotbar listener),
/// exactly as the running app composes them.
fn replay_basic_hunt(
    runtime: &tokio::runtime::Runtime,
    dir: &Path,
) -> (Db, Arc<HuntTracker>, Arc<EventBus>) {
    let scenario = scenario_dir();
    let plan = load_clock_plan(&scenario);

    let db = runtime
        .block_on(Db::open(&dir.join("entropia_orme.db")))
        .expect("migrated database");
    let pool = db.pool().clone();

    let chatlog = dir.join("chat_testing.log");
    std::fs::File::create(&chatlog).expect("empty chatlog");

    let bus = Arc::new(EventBus::new());
    let clock = Arc::new(MockClock::new(Some(plan.start), 0.0));
    let watcher = ChatlogWatcher::new(bus.clone(), &chatlog, None);
    watcher.start();

    let tracker = HuntTracker::new(
        bus.clone(),
        pool.clone(),
        runtime.handle().clone(),
        clock.clone(),
        Providers::default(),
    )
    .expect("tracker");
    tracker.start_session().expect("session start");

    let content = std::fs::read_to_string(scenario.join("chat_replay.log")).expect("chat replay");
    let appended = content.split_inclusive('\n').count() as u64;
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
    watcher
        .wait_until_drained(appended, Duration::from_secs(10))
        .expect("watcher drains the scenario");
    clock.advance(plan.step_seconds).expect("plan step");
    tracker.stop_session().expect("session stop");
    watcher.stop();

    (db, tracker, bus)
}

/// `statistics.median` over a sorted slice (average the two middle elements on
/// an even count), matching the baseline script's `_stats`.
fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n.is_multiple_of(2) {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

/// The baseline's p95 index: `round(0.95 * (n - 1))`, clamped into range.
fn p95(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    let index = (0.95 * (n - 1) as f64).round() as usize;
    sorted[index.min(n - 1)]
}

#[test]
#[ignore = "measurement harness, not a correctness gate; run with --release --ignored --nocapture"]
fn in_process_router_microbench() {
    // A dedicated runtime drives the replay (its watcher/tracker schedule onto
    // a handle) the way the oracle does; the timed dispatch loop then runs on
    // it too, so the measurement never crosses the test's own runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let dir = tempfile::tempdir().expect("temp dir");

    let (db, tracker, bus) = replay_basic_hunt(&runtime, dir.path());

    let game_data =
        Arc::new(GameDataStore::new(&dir.path().join("empty")).expect("empty game-data store"));
    let hydration = Arc::new(HydrationState::new(
        db,
        game_data,
        Arc::new(RealClock::new()),
        dir.path().to_path_buf(),
    ));
    // A resting scan service (default providers report no engine / no window),
    // so `scan/skills/status` answers its idle 200 the way it does in the app
    // before any scan, rather than the unavailable floor.
    let scan = SkillScanManual::new(
        ScanProviders::default(),
        Arc::new(RealClock::new()),
        None,
        None,
        0,
    );
    // An inert hotbar listener (no keystroke source) so `tracking/snapshot`,
    // which reads the composed tracker + hotbar, answers its real 200 the way
    // the app does with the hook library absent.
    let hotbar = HotbarListener::new(bus.clone(), None, None);
    let state = Arc::new(
        AppState::new(0)
            .with_hydration(hydration)
            .with_tracker(tracker)
            .with_hotbar_listener(hotbar)
            .with_skill_scan(scan),
    );

    // The replayed session's id, for the two session-scoped endpoints.
    let session_id = runtime.block_on(async {
        let response = dispatch_in_process(state.clone(), "GET", "/api/tracking/sessions", &[], vec![])
            .await
            .expect("sessions dispatch");
        assert_eq!(response.status, 200, "sessions list");
        let sessions: Value = serde_json::from_slice(&response.body).expect("sessions json");
        sessions
            .as_array()
            .and_then(|list| list.first())
            .and_then(|session| session["id"].as_str())
            .expect("a replayed session id")
            .to_string()
    });

    // The baseline's endpoint set and order: health first, then the curated
    // hydration GET surface, with the session-scoped templates filled.
    let detail = format!("/api/tracking/session/{session_id}");
    let suggestion = format!("/api/tracking/session/{session_id}/quest-link-suggestion");
    let endpoints: [(&str, &str); 11] = [
        ("GET_health", "/api/health"),
        ("GET_tracking_snapshot", "/api/tracking/snapshot"),
        ("GET_tracking_sessions", "/api/tracking/sessions"),
        ("GET_tracking_session_detail", detail.as_str()),
        ("GET_tracking_session_quest_link_suggestion", suggestion.as_str()),
        ("GET_quests", "/api/quests"),
        ("GET_quests_mobs", "/api/quests/mobs"),
        ("GET_quests_analytics", "/api/quests/analytics"),
        ("GET_quests_playlists", "/api/quests/playlists"),
        ("GET_scan_skills_status", "/api/scan/skills/status"),
        ("GET_codex_meta_attributes", "/api/codex/meta/attributes"),
    ];

    let rows: Vec<(String, f64, f64, f64, f64)> = runtime.block_on(async {
        let mut rows = Vec::with_capacity(endpoints.len());
        for (id, path) in endpoints {
            for _ in 0..WARMUPS {
                let response = dispatch_in_process(state.clone(), "GET", path, &[], vec![])
                    .await
                    .unwrap_or_else(|err| panic!("{id} warm-up: {err}"));
                assert_eq!(response.status, 200, "{id} warm-up status");
            }
            let mut timings = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let started = Instant::now();
                let response = dispatch_in_process(state.clone(), "GET", path, &[], vec![])
                    .await
                    .unwrap_or_else(|err| panic!("{id}: {err}"));
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(response.status, 200, "{id} status");
                timings.push(elapsed_ms);
            }
            timings.sort_by(|a, b| a.partial_cmp(b).expect("finite timings"));
            rows.push((
                id.to_string(),
                median(&timings),
                p95(&timings),
                timings[0],
                timings[timings.len() - 1],
            ));
        }
        rows
    });

    println!("\nin-process router micro-bench (AFTER: single pure-Rust binary)");
    println!(
        "{SAMPLES} samples per endpoint after {WARMUPS} warm-ups, over a freshly replayed \
         basic_hunt_10_events state; transport-stripped (dispatch_in_process, no socket).\n"
    );
    println!("| Endpoint | p50 ms | p95 ms | min ms | max ms |");
    println!("| --- | --- | --- | --- | --- |");
    for (id, p50, p95v, min, max) in &rows {
        println!("| `{id}` | {p50:.4} | {p95v:.4} | {min:.4} | {max:.4} |");
    }
    println!();
}
