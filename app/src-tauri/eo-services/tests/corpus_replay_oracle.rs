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
//! The goldens are the frozen end-to-end equivalence evidence: banked
//! when the native pipeline was proven byte-identical to the reference
//! implementation, they pin that behaviour permanently, so a
//! byte-identical native replay proves equivalence on every CI run with
//! no second implementation present. The two serialisations share one
//! normaliser, in fingerprint-then-snapshot order, exactly as the
//! golden harness assigns its encounter-order symbols.
//!
//! The replay protocol mirrors the harness: a frozen, driver-advanced
//! clock from the scenario's committed plan; lines streamed one flush
//! per timestamp tick so the tail never observes end-of-file inside a
//! tick; a drain barrier on the appended line count; one plan step
//! before the session stops.
//!
//! A scenario may also carry a `steps.jsonl` script, for behaviour the
//! chat log alone cannot drive: it interleaves the log's tick groups with
//! clock steps, resolved hotbar presses (stamped at the clock's current
//! instant, as the listener stamps the OS key occurrence), segment
//! declarations, and process restarts. A restart is a crash: the old
//! process's bus and tail go quiet, and a fresh bus, tail, and tracker
//! open the same database, whose construction recovers the orphaned
//! session before the next one starts. Every producer publish completes
//! its tracker dispatch before returning, so a drained tick group is a
//! processed one and the next step observes its effects.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use eo_services::bus_events::{BusEvent, HotbarIntentPayload, HotbarItemKind};
use eo_services::chatlog_time::ChatLogClock;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::{Clock, MockClock};
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::fingerprint_recorder::FingerprintRecorder;
use eo_services::healing_profile::HealingProfile;
use eo_services::time::naive_to_epoch;
use eo_services::tracker::{ActivityKey, ActivityRef, HuntTracker, Providers};
use eo_wire::db_snapshot::{capture, serialize};
use eo_wire::normalizer::Normalizer;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scenario_dir(family: &str, name: &str) -> PathBuf {
    repo_root()
        .join("app/src-tauri/fixtures/corpus")
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

/// The catalogue snapshot over the live database, normalised with the
/// fingerprint's own symbol tables (the shared-normaliser contract).
///
/// The rows come from [`Db::snapshot_rows`], which runs the catalogue queries
/// (each with its deterministic ORDER BY) on a reader connection and shapes
/// every row through the same stored-value-typed normaliser the production
/// snapshot uses. Proving the frozen DB-state golden against that path is the
/// point: the oracle and the app read the database through one shaper.
async fn catalogue_snapshot(db: &Db, normalizer: &mut Normalizer) -> String {
    let tables = db.snapshot_rows().await.expect("catalogue snapshot");
    serialize(&capture(&tables, normalizer))
}

/// One step of a scenario's optional `steps.jsonl` script.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    /// Stream the next N tick groups of the chat log and drain them.
    Chat(usize),
    /// Advance the injected clock by this many seconds.
    Advance(f64),
    /// A resolved hotbar press at the clock's current instant.
    Hotbar(Box<ScriptedPress>),
    /// Declare a segment, or end the standing one with null.
    Segment(Option<String>),
    /// The process dies; a new one starts over the same database.
    Restart,
}

/// What the hotbar listener publishes for a resolved slot, minus the
/// session and the occurrence instant the replay supplies.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptedPress {
    slot: String,
    equipment_id: i64,
    item_name: String,
    item_kind: String,
    cost_per_use_ped: f64,
    reload_seconds: f64,
    #[serde(default)]
    healing_profile: Option<HealingProfile>,
    #[serde(default)]
    lifesteal_percent: Option<f64>,
}

fn load_steps(scenario: &Path) -> Option<Vec<Step>> {
    let script = std::fs::read_to_string(scenario.join("steps.jsonl")).ok()?;
    Some(
        script
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a well-formed replay step"))
            .collect(),
    )
}

/// One process lifetime: its bus, chat-log tail, and tracker, plus the
/// lines this tail has been handed.
struct Process {
    bus: Arc<EventBus>,
    watcher: ChatlogWatcher,
    tracker: Arc<HuntTracker>,
    appended: u64,
}

/// Start a process and its session, in the order the fingerprint's
/// opening lines assume: the tail, the recorder, the tracker, the start.
fn boot(
    runtime: &tokio::runtime::Runtime,
    db: &Db,
    clock: &Arc<MockClock>,
    chatlog: &Path,
    recorder: &FingerprintRecorder,
    player_name: &str,
) -> Process {
    let bus = Arc::new(EventBus::new());
    let watcher = ChatlogWatcher::new(bus.clone(), chatlog, None, ChatLogClock::host_local());
    watcher.start();
    recorder.install(&bus);
    let tracker = runtime
        .block_on(HuntTracker::new(
            bus.clone(),
            db.clone(),
            clock.clone(),
            ChatLogClock::host_local(),
            Providers {
                player_name: player_name.to_string(),
                ..Providers::default()
            },
        ))
        .expect("tracker");
    runtime
        .block_on(tracker.start_session())
        .expect("session start");
    Process {
        bus,
        watcher,
        tracker,
        appended: 0,
    }
}

/// Append tick groups one flush each, then wait until the tail has read
/// (and so dispatched) every line.
fn stream(chatlog: &Path, process: &mut Process, groups: &[String]) {
    let mut sink = std::fs::OpenOptions::new()
        .append(true)
        .open(chatlog)
        .expect("chatlog append");
    for group in groups {
        sink.write_all(group.as_bytes()).expect("tick write");
        sink.flush().expect("tick flush");
        process.appended += group.split_inclusive('\n').count() as u64;
    }
    process
        .watcher
        .wait_until_drained(process.appended, Duration::from_secs(10))
        .expect("watcher drains the scenario");
}

fn press(process: &Process, clock: &MockClock, press: &ScriptedPress) {
    let item_kind = match press.item_kind.as_str() {
        "healing" => HotbarItemKind::Healing,
        "weapon" => HotbarItemKind::Weapon,
        other => panic!("unsupported scripted hotbar item kind {other}"),
    };
    process
        .bus
        .publish(&BusEvent::HotbarIntent(HotbarIntentPayload {
            session_id: None,
            slot: press.slot.clone(),
            occurred_at: naive_to_epoch(clock.now()),
            equipment_id: press.equipment_id,
            item_name: press.item_name.clone(),
            item_kind,
            cost_per_use_ped: press.cost_per_use_ped,
            reload_seconds: press.reload_seconds,
            healing_profile: press.healing_profile.clone(),
            lifesteal_percent: press.lifesteal_percent,
        }));
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

    let chatlog = dir.path().join("chat_testing.log");
    std::fs::File::create(&chatlog).expect("empty chatlog");

    let clock = Arc::new(MockClock::new(Some(plan.start), 0.0));
    // The recorder installs before the session starts, so the start
    // events are the fingerprint's opening lines.
    let recorder = FingerprintRecorder::new();
    let mut process = boot(&runtime, &db, &clock, &chatlog, &recorder, player_name);

    // Stream the replay one tick per flush, then drain on the line
    // count (the watcher counts every line it has read whole); a
    // scripted scenario interleaves its steps between tick groups.
    let content = std::fs::read_to_string(scenario.join("chat_replay.log")).expect("chat replay");
    let groups = tick_groups(&content);
    match load_steps(&scenario) {
        None => stream(&chatlog, &mut process, &groups),
        Some(steps) => {
            let mut next = 0;
            let mut segment: Option<String> = None;
            for step in steps {
                match step {
                    Step::Chat(count) => {
                        let until = next + count;
                        assert!(
                            until <= groups.len(),
                            "{name}: the script outruns the chat log"
                        );
                        stream(&chatlog, &mut process, &groups[next..until]);
                        next = until;
                    }
                    Step::Advance(seconds) => clock.advance(seconds).expect("script step"),
                    Step::Hotbar(scripted) => press(&process, &clock, &scripted),
                    Step::Segment(declared) => {
                        if let Some(standing) = segment.take() {
                            runtime
                                .block_on(
                                    process
                                        .tracker
                                        .deactivate_activity(ActivityKey::Segment(standing)),
                                )
                                .expect("segment ends");
                        }
                        if let Some(name) = declared.clone() {
                            runtime
                                .block_on(
                                    process
                                        .tracker
                                        .activate_activity(ActivityRef::Segment { name }, false),
                                )
                                .expect("segment starts");
                        }
                        segment = declared;
                    }
                    Step::Restart => {
                        recorder.uninstall(&process.bus);
                        process.watcher.stop();
                        process = boot(&runtime, &db, &clock, &chatlog, &recorder, player_name);
                        segment = None;
                    }
                }
            }
            assert_eq!(
                next,
                groups.len(),
                "{name}: the script leaves chat unreplayed"
            );
        }
    }
    clock.advance(plan.step_seconds).expect("plan step");
    runtime
        .block_on(process.tracker.stop_session())
        .expect("session stop");
    process.watcher.stop();

    // Fingerprint first, snapshot second, one normaliser: the symbol
    // tables assign in exactly the golden harness's encounter order.
    let mut normalizer = Normalizer::new();
    let actual_fingerprint = recorder.serialize(&mut normalizer);
    let actual_snapshot = runtime.block_on(catalogue_snapshot(&db, &mut normalizer));

    // Deliberate re-ratification hook (see TESTING.md "Goldens
    // regeneration", and the demo-goldens UPDATE hook it mirrors): write
    // what the pipeline currently produces instead of asserting. Every
    // write is still gated behind the ratification guard at push time.
    if std::env::var_os("UPDATE_CORPUS_GOLDENS").is_some() {
        std::fs::create_dir_all(scenario.join("expected")).expect("expected dir");
        std::fs::write(
            scenario.join("expected/fingerprint.jsonl"),
            &actual_fingerprint,
        )
        .expect("fingerprint golden writes");
        std::fs::write(scenario.join("expected/db_state.json"), &actual_snapshot)
            .expect("db_state golden writes");
        return;
    }

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
fn global_item_drop_matches_the_goldens() {
    replay_against_goldens("scripted", "global_item_drop", "TestPlayer");
}

#[test]
fn hof_kill_correlated_matches_the_goldens() {
    replay_against_goldens("scripted", "hof_kill_correlated", "TestPlayer");
}

#[test]
fn tree_harvesting_session_matches_the_goldens() {
    replay_against_goldens("scripted", "tree_harvesting_session", "");
}

#[test]
fn placeholder_recorded_hunt_matches_the_goldens() {
    replay_against_goldens("recorded", "placeholder_recorded_hunt", "");
}

#[test]
fn healing_effect_rotation_matches_the_goldens() {
    replay_against_goldens("scripted", "healing_effect_rotation", "");
}

#[test]
fn deferred_scenarios_are_named_not_silently_dropped() {
    // The remaining golden-carrying scenario needs the skill-scan
    // capture pipeline, which joins the oracle when that service
    // lands; naming it here keeps the coverage gap loud. Real
    // recorded bundles are local-by-default and stay out of the
    // public tree, so the scenario is simply absent on most hosts;
    // where it is present, it must still carry the goldens this
    // manifest defers.
    let deferred = ["hunt_with_skill_scan"];
    for name in deferred {
        let dir = scenario_dir("recorded", name);
        if !dir.is_dir() {
            eprintln!("{name}: local-only bundle absent on this host; the deferral stands");
            continue;
        }
        assert!(
            dir.join("expected/db_state.json").exists(),
            "{name} is present without goldens; update the deferred manifest"
        );
    }
}
