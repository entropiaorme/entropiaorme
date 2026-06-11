//! The corpus replay oracle: every scenario replayed through the
//! complete native pipeline (chat-log tail -> bus -> tracker ->
//! database) must match the committed goldens byte-for-byte, on both
//! surfaces at once:
//!
//! - the normalised event fingerprint (`expected/fingerprint.jsonl`),
//!   now including the tracker's own session lifecycle and domain
//!   events alongside the watcher's stream;
//! - the catalogue database snapshot (`expected/db_state.json`),
//!   produced by the tracker's real persistence writes.
//!
//! The goldens are the cross-implementation contract: the backend's
//! replay suite proves the Python pipeline against them in CI, so a
//! byte-identical native replay proves end-to-end equivalence without
//! spawning the other implementation. The two serialisations share
//! one normaliser, in fingerprint-then-snapshot order, exactly as the
//! golden harness assigns its encounter-order symbols.
//!
//! The replay protocol mirrors the harness: a frozen, driver-advanced
//! clock from the scenario's committed plan; lines streamed one flush
//! per timestamp tick so the tail never observes end-of-file inside a
//! tick; a drain barrier on the appended line count; one plan step
//! before the session stops.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::fingerprint_recorder::FingerprintRecorder;
use eo_services::tracker::{HuntTracker, Providers};
use eo_wire::db_snapshot::{capture, serialize, CATALOGUE};
use eo_wire::normalizer::Normalizer;
use serde_json::{Map, Value};
use sqlx::{Column, Row, SqlitePool};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scenario_dir(family: &str, name: &str) -> PathBuf {
    repo_root()
        .join("backend/tests/e2e/corpus")
        .join(family)
        .join(name)
}

/// The committed clock plan: a frozen start instant the driver
/// advances by one step before the session stops.
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
/// riding with the line before them, exactly as the harness streams:
/// a tick is the atomic flush unit, so the tail loop can never see
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

fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    let mut object = Map::new();
    for column in row.columns() {
        let index = column.ordinal();
        let value = if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
            value.map(Value::from).unwrap_or(Value::Null)
        } else if let Ok(value) = row.try_get::<Option<f64>, _>(index) {
            value.map(Value::from).unwrap_or(Value::Null)
        } else if let Ok(value) = row.try_get::<Option<String>, _>(index) {
            value.map(Value::from).unwrap_or(Value::Null)
        } else {
            panic!("unsupported column type in {}", column.name());
        };
        object.insert(column.name().to_string(), value);
    }
    Value::Object(object)
}

/// The catalogue snapshot over the live database, normalised with the
/// fingerprint's own symbol tables (the shared-normaliser contract).
async fn catalogue_snapshot(pool: &SqlitePool, normalizer: &mut Normalizer) -> String {
    let mut tables = Map::new();
    for spec in CATALOGUE {
        let sql = format!("{} ORDER BY {}", spec.query, spec.order_by.join(", "));
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(pool)
            .await
            .expect("catalogue query");
        let mut json_rows = Vec::with_capacity(rows.len());
        for row in &rows {
            json_rows.push(row_to_json(row));
        }
        tables.insert(spec.name.to_string(), Value::Array(json_rows));
    }
    serialize(&capture(&tables, normalizer))
}

/// Replay one scenario through the full native pipeline and assert
/// both committed goldens byte-for-byte.
fn replay_against_goldens(family: &str, name: &str, player_name: &str) {
    let scenario = scenario_dir(family, name);
    let plan = load_clock_plan(&scenario);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let dir = tempfile::tempdir().expect("scenario tempdir");
    let db = runtime
        .block_on(Db::open(&dir.path().join("entropia_orme.db")))
        .expect("migrated database");
    let pool = db.pool().clone();

    let chatlog = dir.path().join("chat_testing.log");
    std::fs::File::create(&chatlog).expect("empty chatlog");

    let bus = Arc::new(EventBus::new());
    let clock = Arc::new(MockClock::new(Some(plan.start), 0.0));
    let watcher = ChatlogWatcher::new(bus.clone(), &chatlog, None);
    watcher.start();

    // The recorder installs before the session starts, so the start
    // events are the fingerprint's opening lines.
    let recorder = FingerprintRecorder::new();
    recorder.install(&bus);

    let tracker = HuntTracker::new(
        bus.clone(),
        pool.clone(),
        runtime.handle().clone(),
        clock.clone(),
        Providers {
            player_name: player_name.to_string(),
            ..Providers::default()
        },
    )
    .expect("tracker");
    tracker.start_session().expect("session start");

    // Stream the replay one tick per flush, then drain on the line
    // count (the watcher counts every line it has read whole).
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

    // Fingerprint first, snapshot second, one normaliser: the symbol
    // tables assign in exactly the golden harness's encounter order.
    let mut normalizer = Normalizer::new();
    let actual_fingerprint = recorder.serialize(&mut normalizer);
    let actual_snapshot = runtime.block_on(catalogue_snapshot(&pool, &mut normalizer));

    let expected_fingerprint = std::fs::read_to_string(scenario.join("expected/fingerprint.jsonl"))
        .expect("fingerprint golden");
    assert_eq!(
        actual_fingerprint,
        expected_fingerprint,
        "{name}: the native fingerprint diverged from its golden\n{}",
        first_divergence(&expected_fingerprint, &actual_fingerprint)
    );

    let expected_snapshot =
        std::fs::read_to_string(scenario.join("expected/db_state.json")).expect("db_state golden");
    assert_eq!(
        actual_snapshot,
        expected_snapshot,
        "{name}: the native database snapshot diverged from its golden\n{}",
        first_divergence(&expected_snapshot, &actual_snapshot)
    );
}

#[test]
fn single_mob_hunt_matches_the_goldens() {
    replay_against_goldens("scripted", "single_mob_hunt", "");
}

#[test]
fn basic_hunt_10_events_matches_the_goldens() {
    replay_against_goldens("scripted", "basic_hunt_10_events", "");
}

#[test]
fn empty_session_matches_the_goldens() {
    replay_against_goldens("scripted", "empty_session", "");
}

#[test]
fn crit_dodge_evade_jam_matches_the_goldens() {
    replay_against_goldens("scripted", "crit_dodge_evade_jam", "");
}

#[test]
fn defensive_combat_round_matches_the_goldens() {
    replay_against_goldens("scripted", "defensive_combat_round", "");
}

#[test]
fn enhancer_break_during_hunt_matches_the_goldens() {
    replay_against_goldens("scripted", "enhancer_break_during_hunt", "");
}

#[test]
fn multi_mob_hunt_loot_grouping_matches_the_goldens() {
    replay_against_goldens("scripted", "multi_mob_hunt_loot_grouping", "");
}

#[test]
fn skill_gain_across_tick_matches_the_goldens() {
    replay_against_goldens("scripted", "skill_gain_across_tick", "");
}

#[test]
fn mission_completion_with_reward_suppression_matches_the_goldens() {
    // The harness pipeline runs without the quest-reward filter, so
    // the would-be suppressed gain flows; the goldens pin that shape.
    replay_against_goldens("scripted", "mission_completion_with_reward_suppression", "");
}

#[test]
fn global_kill_correlated_matches_the_goldens() {
    replay_against_goldens("scripted", "global_kill_correlated", "TestPlayer");
}

#[test]
fn hof_item_drop_matches_the_goldens() {
    replay_against_goldens("scripted", "hof_item_drop", "TestPlayer");
}

#[test]
fn placeholder_recorded_hunt_matches_the_goldens() {
    replay_against_goldens("recorded", "placeholder_recorded_hunt", "");
}

#[test]
fn deferred_scenarios_are_named_not_silently_dropped() {
    // The remaining golden-carrying scenario needs the skill-scan
    // capture pipeline, which joins the oracle when that service
    // lands; naming it here keeps the coverage gap loud.
    let deferred = ["hunt_with_skill_scan"];
    for name in deferred {
        assert!(
            scenario_dir("recorded", name).is_dir(),
            "{name} left the corpus; update the deferred manifest"
        );
    }
}
