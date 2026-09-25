//! Event-path bench: per-event producer-blocking latency through the
//! composed bus consumers, over a real-scale database fixture.
//!
//! Every bus consumer absorbs events before the producer's `publish`
//! returns (the tracker and quest actors by call-semantics rendezvous,
//! the skill tracker by persisting on the publisher's thread), so the
//! per-publish wall time IS the producer-blocking cost of the current
//! posture: mailbox hop + handler work + any inline persistence. This
//! harness measures that cost with the production consumer set (hunt
//! tracker, quest service, skill tracker) over a copy of a real-scale
//! database, in two regimes:
//!
//! - a flood leg (publish as fast as the pipeline absorbs) for the
//!   worst-case latency distribution and the throughput ceiling;
//! - a paced leg at a configurable realistic event rate for the
//!   steady-state distribution.
//!
//! The event stream is synthesised, not hand-built: the replay corpus's
//! scripted scenarios are looped with rewritten, strictly-advancing
//! timestamps and streamed through a real `ChatlogWatcher` once
//! (unmeasured) to capture the exact `BusEvent` sequence the production
//! parser and tick-flush path produce; the measured legs then republish
//! that captured stream from a plain producer thread, exactly the
//! watcher's own threading posture. Absent from the composition: the
//! frontend push bridge's tap (a per-publish conversion cost this
//! harness does not model).
//!
//! `#[ignore]`d on purpose: a measurement harness, not a correctness
//! gate. Run it with:
//!
//! ```text
//! EO_PERF_FIXTURE=<path to a database copy> \
//!   cargo test -p eo-services --release --test event_path_bench -- --ignored --nocapture
//! ```
//!
//! Knobs: `EO_BENCH_REPS` (scenario repetitions per leg, default 250),
//! `EO_BENCH_PACED_RATE` (paced-leg events/second, default 10),
//! `EO_BENCH_PACED_SECONDS` (paced-leg duration cap, default 20).

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use eo_services::bus_events::BusEvent;
use eo_services::chatlog_time::ChatLogClock;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::event_bus::{EventBus, Topic};
use eo_services::quests::QuestService;
use eo_services::skill_tracker::SkillTracker;
use eo_services::tracker::{HuntTracker, Providers};

/// The scripted scenarios looped into the long synthetic hunt: combat
/// with crits, multi-item loot ticks with shrapnel, and skill gains
/// spanning ticks, so the tracker's combat/loot paths and the skill
/// tracker's persistence path all carry load.
const SCENARIOS: &[&str] = &["multi_mob_hunt_loot_grouping", "skill_gain_across_tick"];

/// Seconds of dead air between scenario blocks, so each block's final
/// loot tick flushes on the next block's first timestamp.
const BLOCK_GAP_SECONDS: i64 = 30;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scenario_log(name: &str) -> String {
    let path = repo_root()
        .join("app/src-tauri/fixtures/corpus/scripted")
        .join(name)
        .join("chat_replay.log");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n.is_multiple_of(2) {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

fn p95(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    let index = (0.95 * (n - 1) as f64).round() as usize;
    sorted[index.min(n - 1)]
}

fn parse_stamp(line: &str) -> Option<NaiveDateTime> {
    let candidate = line.trim_start().get(0..19)?;
    NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S").ok()
}

/// Loop the scenario logs with rewritten, strictly-advancing timestamps:
/// each repetition of each scenario becomes one block, offset so blocks
/// never overlap. Returns the log content, its line count, and the
/// synthetic span in seconds.
fn build_replay_log(reps: usize) -> (String, u64, f64) {
    let sources: Vec<(String, NaiveDateTime, i64)> = SCENARIOS
        .iter()
        .map(|name| {
            let content = scenario_log(name);
            let stamps: Vec<NaiveDateTime> = content.lines().filter_map(parse_stamp).collect();
            let first = *stamps.first().expect("scenario has timestamped lines");
            let span =
                (*stamps.last().expect("scenario has timestamped lines") - first).num_seconds();
            (content, first, span)
        })
        .collect();

    let mut base = NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
        .expect("base stamp");
    let mut log = String::new();
    let mut lines: u64 = 0;
    for _ in 0..reps {
        for (content, first, span) in &sources {
            for line in content.lines() {
                match parse_stamp(line) {
                    Some(stamp) => {
                        let shifted = base + (stamp - *first);
                        log.push_str(&shifted.format("%Y-%m-%d %H:%M:%S").to_string());
                        log.push_str(&line[19..]);
                    }
                    None => log.push_str(line),
                }
                log.push('\n');
                lines += 1;
            }
            base += chrono::Duration::seconds(span + BLOCK_GAP_SECONDS);
        }
    }
    let span_seconds = (base
        - NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("base stamp"))
    .num_seconds() as f64;
    (log, lines, span_seconds)
}

/// The tick-grouping key and grouping, mirroring the corpus replay
/// oracle's streaming protocol: a tick is the atomic flush unit, so the
/// tail never observes end-of-file inside one.
fn tick_groups(content: &str) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    let mut group = String::new();
    let mut current: Option<NaiveDateTime> = None;
    for line in content.split_inclusive('\n') {
        let key = parse_stamp(line);
        if !group.is_empty() {
            if let Some(key) = key {
                if current != Some(key) {
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

/// Stream the synthetic log through a real watcher once and capture the
/// exact `BusEvent` sequence it publishes (a full-stream tap; a dummy
/// combat subscriber defeats the idle fast-path so combat lines parse,
/// exactly as they do while a session runs).
fn capture_events(content: &str, lines: u64) -> Vec<BusEvent> {
    let dir = tempfile::tempdir().expect("capture tempdir");
    let chatlog = dir.path().join("chat_capture.log");
    std::fs::File::create(&chatlog).expect("empty chatlog");

    let bus = Arc::new(EventBus::new());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    bus.add_tap(move |event: &BusEvent| sink.lock().expect("capture sink").push(event.clone()));
    bus.subscribe(Topic::Combat, |_| {});

    let watcher = ChatlogWatcher::new(bus.clone(), &chatlog, None, ChatLogClock::host_local());
    watcher.start();
    {
        let mut sink = std::fs::OpenOptions::new()
            .append(true)
            .open(&chatlog)
            .expect("chatlog append");
        for group in tick_groups(content) {
            sink.write_all(group.as_bytes()).expect("tick write");
            sink.flush().expect("tick flush");
        }
    }
    watcher
        .wait_until_drained(lines, Duration::from_secs(300))
        .expect("watcher drains the synthetic log");
    watcher.stop();

    let events = std::mem::take(&mut *captured.lock().expect("captured events"));
    events
}

fn label(event: &BusEvent) -> &'static str {
    match event {
        BusEvent::Combat(_) => "combat",
        BusEvent::LootGroup(_) => "loot_group",
        BusEvent::HarvestFail(_) => "harvest_fail",
        BusEvent::SkillGain(_) => "skill_gain",
        BusEvent::EnhancerBreak(_) => "enhancer_break",
        BusEvent::Global(_) => "global",
        BusEvent::ActiveToolChanged(_) => "active_tool",
        BusEvent::ActiveHealToolChanged(_) => "active_heal_tool",
        BusEvent::ActiveHarvestToolChanged(_) => "active_harvest_tool",
        BusEvent::HotbarIntent(_) => "hotbar_intent",
        BusEvent::SessionStarted(_) => "session_started",
        BusEvent::SessionStopped(_) => "session_stopped",
        BusEvent::MissionReceived(_) => "mission_received",
        BusEvent::TickFlushed(_) => "tick_flushed",
        BusEvent::TrackingSessionUpdated(_) => "tracking_session_updated",
        BusEvent::ScanStatusChanged(_) => "scan_status_changed",
        BusEvent::HarvestRecorded(_) => "harvest_recorded",
        BusEvent::NavigationUpdated(_) => "navigation_updated",
        BusEvent::ProtectionUpdated(_) => "protection_updated",
    }
}

/// Per-topic and overall latency rows out of one leg's samples.
fn summarise(samples: &[(&'static str, f64)]) -> Vec<(String, usize, f64, f64, f64)> {
    let mut by_label: std::collections::BTreeMap<&str, Vec<f64>> =
        std::collections::BTreeMap::new();
    for (label, ms) in samples {
        by_label.entry(label).or_default().push(*ms);
    }
    let mut rows = Vec::new();
    let mut all: Vec<f64> = samples.iter().map(|(_, ms)| *ms).collect();
    for (label, mut timings) in by_label {
        timings.sort_by(|a, b| a.partial_cmp(b).expect("finite timings"));
        let max = timings[timings.len() - 1];
        rows.push((
            label.to_string(),
            timings.len(),
            median(&timings),
            p95(&timings),
            max,
        ));
    }
    all.sort_by(|a, b| a.partial_cmp(b).expect("finite timings"));
    let max = all[all.len() - 1];
    rows.push(("ALL".to_string(), all.len(), median(&all), p95(&all), max));
    rows
}

fn print_rows(title: &str, rows: &[(String, usize, f64, f64, f64)]) {
    println!("\n{title}");
    println!("| topic | events | p50 ms | p95 ms | max ms |");
    println!("| --- | --- | --- | --- | --- |");
    for (label, count, p50, p95v, max) in rows {
        println!("| `{label}` | {count} | {p50:.4} | {p95v:.4} | {max:.4} |");
    }
}

async fn count_kills(db: &Db) -> i64 {
    db.with_reader(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM kills", [], |row| row.get::<_, i64>(0))?)
    })
    .await
    .expect("kill count")
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "measurement harness, not a correctness gate; run with --release --ignored --nocapture and EO_PERF_FIXTURE"]
fn event_path_bench() {
    let Ok(fixture) = std::env::var("EO_PERF_FIXTURE") else {
        eprintln!("EO_PERF_FIXTURE not set; skipping (point it at a database copy)");
        return;
    };
    let reps: usize = env_or("EO_BENCH_REPS", 250);
    let paced_rate: f64 = env_or("EO_BENCH_PACED_RATE", 10.0);
    let paced_seconds: f64 = env_or("EO_BENCH_PACED_SECONDS", 20.0);

    // Synthesise the hunt and capture its exact bus-event stream.
    let (content, lines, span_seconds) = build_replay_log(reps);
    let events = capture_events(&content, lines);
    assert!(!events.is_empty(), "capture produced no events");

    // Stand the production consumer set up over a fixture copy.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let dir = tempfile::tempdir().expect("bench tempdir");
    let db_path = dir.path().join("entropia_orme.db");
    std::fs::copy(&fixture, &db_path).expect("fixture copy");
    let wal = format!("{fixture}-wal");
    if Path::new(&wal).exists()
        && std::fs::metadata(&wal)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        std::fs::copy(&wal, dir.path().join("entropia_orme.db-wal")).expect("fixture wal copy");
    }
    let db = runtime
        .block_on(Db::open(&db_path))
        .expect("migrated fixture database");
    let kills_before = runtime.block_on(count_kills(&db));

    let bus = Arc::new(EventBus::new());
    let clock = Arc::new(MockClock::new(
        Some(
            NaiveDateTime::parse_from_str("2026-01-01T00:00:00", "%Y-%m-%dT%H:%M:%S")
                .expect("clock start"),
        ),
        0.0,
    ));
    let _quests = QuestService::start(&bus, db.clone(), clock.clone(), runtime.handle().clone());
    let _skill_tracker =
        SkillTracker::new(&bus, db.clone(), clock.clone(), ChatLogClock::host_local());
    let tracker = runtime
        .block_on(HuntTracker::new(
            bus.clone(),
            db.clone(),
            clock.clone(),
            ChatLogClock::host_local(),
            Providers {
                player_name: "TestPlayer".to_string(),
                ..Providers::default()
            },
        ))
        .expect("tracker");
    runtime
        .block_on(tracker.start_session())
        .expect("session start");

    // Flood leg: publish as fast as the pipeline absorbs, from a plain
    // thread (the watcher's own posture). Doubles as the warm-up.
    let mut flood: Vec<(&'static str, f64)> = Vec::with_capacity(events.len());
    let flood_started = Instant::now();
    for event in &events {
        let started = Instant::now();
        bus.publish(event);
        flood.push((label(event), started.elapsed().as_secs_f64() * 1000.0));
    }
    let flood_elapsed = flood_started.elapsed().as_secs_f64();

    // Paced leg: the same stream at a fixed realistic rate, deadline-paced.
    let paced_count = ((paced_rate * paced_seconds) as usize).min(events.len());
    let gap = Duration::from_secs_f64(1.0 / paced_rate);
    let mut paced: Vec<(&'static str, f64)> = Vec::with_capacity(paced_count);
    let mut deadline = Instant::now();
    for event in events.iter().take(paced_count) {
        deadline += gap;
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let started = Instant::now();
        bus.publish(event);
        paced.push((label(event), started.elapsed().as_secs_f64() * 1000.0));
    }

    clock.advance(span_seconds + 1.0).expect("clock advance");
    let stop_started = Instant::now();
    runtime
        .block_on(tracker.stop_session())
        .expect("session stop");
    let stop_ms = stop_started.elapsed().as_secs_f64() * 1000.0;
    let kills_after = runtime.block_on(count_kills(&db));

    println!("\nevent-path bench (producer-blocking latency through the composed consumer set)");
    println!(
        "fixture: {fixture}; scenarios {SCENARIOS:?} x {reps} reps = {lines} lines, {} bus events; \
         kills {kills_before} -> {kills_after} (+{})",
        events.len(),
        kills_after - kills_before
    );
    print_rows(
        &format!(
            "flood leg (as fast as absorbed; {:.0} events/s sustained over {:.2} s):",
            events.len() as f64 / flood_elapsed,
            flood_elapsed
        ),
        &summarise(&flood),
    );
    print_rows(
        &format!("paced leg ({paced_rate} events/s, {paced_count} events):"),
        &summarise(&paced),
    );
    println!("\nstop_session (summary recompute + WAL truncate, one-off): {stop_ms:.1} ms");
}
