//! Differential check: the native quest lifecycle vs the Python
//! oracle, over the mission-matching battery and one scripted
//! completion sequence.
//!
//! Three surfaces compare exactly:
//! 1. the similarity-ratio sweep (every deterministic string pair
//!    scores identically to the reference library);
//! 2. the mission-name matcher over a seeded quest library and a
//!    battery of mission names (exact, case, suffix, containment,
//!    accents, typos, misses);
//! 3. a completion/cancel/filter sequence with an injected identifier
//!    stream on both sides, compared on every filter verdict and the
//!    final ledger, claim, completion, link, and overlay tables.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test quest_lifecycle_differential
#![cfg(feature = "cross-language")]

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use chrono::NaiveDateTime;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::difflib::sequence_ratio;
use eo_services::quests::QuestService;
use serde_json::{json, Map, Value};
use sqlx::Row;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn oracle_python() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_ORACLE_PYTHON") {
        return PathBuf::from(explicit);
    }
    let root = repo_root();
    let windows = root.join(".venv/Scripts/python.exe");
    if windows.exists() {
        windows
    } else {
        root.join(".venv/bin/python")
    }
}

/// The seeded quest library: names with case, punctuation, accents,
/// and near-collisions.
const QUEST_NAMES: [&str; 10] = [
    "Iron Challenge",
    "Daily Hunt: Atrox",
    "Géologist Survey",
    "A Small Obstacle",
    "The Long Road Home",
    "Feffox Cave Run",
    "Feffox Cave Runner",
    "Sweat Circle Social",
    "Zero Bounty",
    "Beacon: Rescue Op",
];

/// The mission-name battery: exact, case/space drift, the repeatable
/// suffix, containment, accent folding, typos at and below the fuzzy
/// floor, and outright misses.
const MISSIONS: [&str; 16] = [
    "Iron Challenge",
    "  IRON CHALLENGE ",
    "Iron Challenge (Repeatable)",
    "Iron Challenge (repeatable)  ",
    "Mission: Iron Challenge Part II",
    "Geologist Survey",
    "GÉOLOGIST SURVEY",
    "Iron Chalenge",
    "Iron Chal",
    "Daily Hunt Atrox",
    "Feffox Cave Run",
    "Feffox Cave Runner",
    "A Smal Obstacle",
    "The Long Road",
    "Totally Different Mission",
    "Sweat",
];

/// Deterministic ratio-sweep pairs derived from the names above.
fn sweep_pairs() -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for a in QUEST_NAMES {
        for b in MISSIONS {
            pairs.push((a.to_lowercase(), b.to_lowercase()));
        }
    }
    pairs
}

const ORACLE_SCRIPT: &str = r#"
import json, sys
from datetime import datetime
from difflib import SequenceMatcher
from pathlib import Path

target = Path(sys.argv[1])
quest_names = json.loads(sys.argv[2])
missions = json.loads(sys.argv[3])
pairs = json.loads(sys.argv[4])

import backend.services.quest_service as qs_module
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService
from backend.testing.clock import MockClock
from backend.tracking.schema import init_tracking_tables

class Counter:
    def __init__(self): self.n = 0
    def __call__(self):
        self.n += 1
        return f"fixed-{self.n:04d}"
counter = Counter()
class FakeUuid:
    def __init__(self, text): self.text = text
    def __str__(self): return self.text
qs_module.uuid.uuid4 = lambda: FakeUuid(counter())

db = AppDatabase(target / "oracle.db")
init_tracking_tables(db.conn)
clock = MockClock(start=datetime(2026, 3, 1, 12, 0, 0))
svc = QuestService(db, clock=clock)

out = {}
out["ratios"] = [SequenceMatcher(None, a, b).ratio() for a, b in pairs]

ids = {}
for i, name in enumerate(quest_names):
    payload = {"name": name}
    if name == "Iron Challenge":
        payload.update({"reward_ped": 2.5, "cooldown_hours": 24})
    if name == "Daily Hunt: Atrox":
        payload.update({"reward_ped": 5.0, "reward_is_skill": True, "cooldown_hours": 1})
    if name == "Zero Bounty":
        payload.update({"reward_ped": 0})
    qid = svc.create_quest(payload)["id"]
    db.conn.execute("UPDATE quests SET created_at=?, updated_at=? WHERE id=?", (1000.0 + i, 1000.0 + i, qid))
    db.conn.commit()
    ids[name] = qid

def match_id(name):
    q = svc.match_quest_by_mission_name(name)
    return q["id"] if q else None
out["matches"] = [match_id(m) for m in missions]

svc._on_session_start({"session_id": "sess-abc"})
clock.advance(60)
out["filter_skill"] = svc.quest_reward_filter("Daily Hunt: Atrox", [], [{"skill_name": "Rifle", "amount": 1.0}])
clock.advance(60)
out["filter_liquid"] = svc.quest_reward_filter("Iron Challenge", [
    {"item_name": "Shrapnel", "quantity": 100, "value": 0.1},
    {"item_name": "Universal Ammo", "quantity": 1, "value": 2.51},
], [])
clock.advance(60)
out["filter_zero"] = svc.quest_reward_filter("Zero Bounty", [
    {"item_name": "A", "value": 0.5}, {"item_name": "B", "value": 0.2},
], [])
clock.advance(60)
svc.start_quest_from_mission("Feffox Cave Run")
clock.advance(60)
out["cancelled"] = svc.cancel_quest(ids["Iron Challenge"], undo_reward=True) is not None
clock.advance(60)
svc.decline_session_link("sess-decl")

out["ledger"] = [list(r) for r in db.conn.execute(
    "SELECT id, date, type, description, amount, tag FROM ledger_entries ORDER BY id")]
out["claims"] = [list(r) for r in db.conn.execute(
    "SELECT quest_id, quest_name, ped_value, claimed_at FROM quest_claims ORDER BY id")]
out["completions"] = [list(r) for r in db.conn.execute(
    "SELECT session_id, quest_id, completed_at FROM session_quest_completions ORDER BY id")]
out["links"] = [list(r) for r in db.conn.execute(
    "SELECT session_id, link_type, quest_id, playlist_id, linked_at FROM session_quest_analytics_links ORDER BY session_id")]
out["events"] = [list(r) for r in db.conn.execute(
    "SELECT session_id, event_type, mob_or_item, value_ped, timestamp FROM notable_events ORDER BY id")]
out["started"] = [list(r) for r in db.conn.execute(
    "SELECT id, started_at FROM quests WHERE started_at IS NOT NULL ORDER BY id")]

# The analytics readers over a seeded economy.
for sid, st, en, active, heal, armour in [
    ("an-1", 1000.0, 4600.0, 0, 1.5, 0.25),
    ("an-2", 5000.0, 5030.5, 0, None, 0.0),
]:
    db.conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) VALUES (?, ?, ?, ?, ?, ?)",
        (sid, st, en, active, heal, armour))
db.conn.execute(
    "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) VALUES ('an-k1', 'an-1', 'Atrox', 1100.0, 10, 100.0, 5.0, 1, 0.3, 0.5, 12.75)")
db.conn.execute(
    "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot) VALUES ('an-k1', 'LR-32', 40, 50.0, 0, 0.05)")
db.conn.execute(
    "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) VALUES ('an-1', 1100.0, 'Rifle', 1.0, 0.8)")
iron = ids["Iron Challenge"]
db.conn.execute(
    "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id, completed_at) VALUES ('an-1', ?, 1500.0)", (iron,))
db.conn.execute(
    "INSERT INTO session_quest_analytics_links (session_id, link_type, quest_id, playlist_id, linked_at) VALUES ('an-1', 'quest', ?, NULL, 9000.0)", (iron,))
db.conn.commit()
pl = svc.create_playlist({"name": "Analytics Run", "items": [
    {"quest_id": iron, "group_type": "immediate"},
    {"quest_id": ids["Zero Bounty"], "group_type": "long_horizon"},
]})["id"]
db.conn.execute("UPDATE quest_playlists SET created_at=3000.0, updated_at=3000.0 WHERE id=?", (pl,))
db.conn.execute(
    "INSERT INTO session_quest_analytics_links (session_id, link_type, quest_id, playlist_id, linked_at) VALUES ('an-2', 'playlist', NULL, ?, 9001.0)", (pl,))
db.conn.execute(
    "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id, completed_at) VALUES ('an-2', ?, 5020.0)", (iron,))
db.conn.commit()
out["quest_analytics"] = svc.get_quest_analytics()
out["playlist_analytics"] = svc.get_all_playlist_analytics()
print(json.dumps(out, sort_keys=True))
"#;

/// Project every integer to a float, recursively, so numeric equality
/// is what the comparison tests.
fn floats(value: Value) -> Value {
    match value {
        Value::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(_), Some(as_float)) => json!(as_float),
            _ => Value::Number(number),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(floats).collect()),
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, entry)| (key, floats(entry)))
                .collect(),
        ),
        other => other,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_native_lifecycle_matches_the_python_oracle() {
    let dir = tempfile::tempdir().unwrap();
    let pairs = sweep_pairs();

    // ── The oracle run ──────────────────────────────────────────────
    let mut command = Command::new(oracle_python());
    command
        .arg("-c")
        .arg(ORACLE_SCRIPT)
        .arg(dir.path().join("python"))
        .arg(serde_json::to_string(&QUEST_NAMES).unwrap())
        .arg(serde_json::to_string(&MISSIONS).unwrap())
        .arg(serde_json::to_string(&pairs).unwrap())
        .current_dir(repo_root())
        .env("PYTHONPATH", repo_root());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    std::fs::create_dir_all(dir.path().join("python")).unwrap();
    let output = command.output().expect("oracle runs");
    assert!(
        output.status.success(),
        "oracle failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let oracle: Value =
        serde_json::from_slice(&output.stdout).expect("oracle output parses as JSON");

    // ── The native run ──────────────────────────────────────────────
    std::fs::create_dir_all(dir.path().join("native")).unwrap();
    let db = Db::open(&dir.path().join("native/entropia_orme.db"))
        .await
        .unwrap();
    let pool = db.pool().clone();
    let clock = Arc::new(MockClock::new(
        Some(NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap()),
        0.0,
    ));
    let svc = Arc::new(QuestService::new(pool.clone(), clock.clone()));
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    svc.set_id_source(Arc::new(move || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        format!("fixed-{n:04}")
    }));
    let bus = Arc::new(eo_services::event_bus::EventBus::new());
    svc.subscribe(&bus, tokio::runtime::Handle::current());

    let mut native = Map::new();
    native.insert(
        "ratios".into(),
        json!(pairs
            .iter()
            .map(|(a, b)| {
                let a: Vec<char> = a.chars().collect();
                let b: Vec<char> = b.chars().collect();
                sequence_ratio(&a, &b)
            })
            .collect::<Vec<_>>()),
    );

    let mut ids: Map<String, Value> = Map::new();
    for (index, name) in QUEST_NAMES.iter().enumerate() {
        let mut payload = json!({"name": name});
        if *name == "Iron Challenge" {
            payload["reward_ped"] = json!(2.5);
            payload["cooldown_hours"] = json!(24);
        }
        if *name == "Daily Hunt: Atrox" {
            payload["reward_ped"] = json!(5.0);
            payload["reward_is_skill"] = json!(true);
            payload["cooldown_hours"] = json!(1);
        }
        if *name == "Zero Bounty" {
            payload["reward_ped"] = json!(0);
        }
        let quest = svc.create_quest(&payload).await.unwrap();
        let quest_id = quest["id"].as_i64().unwrap();
        sqlx::query("UPDATE quests SET created_at=?, updated_at=? WHERE id=?")
            .bind(1000.0 + index as f64)
            .bind(1000.0 + index as f64)
            .bind(quest_id)
            .execute(&pool)
            .await
            .unwrap();
        ids.insert((*name).to_string(), json!(quest_id));
    }

    let mut matches = Vec::new();
    for mission in MISSIONS {
        matches.push(json!(svc
            .match_quest_by_mission_name(mission)
            .await
            .unwrap()
            .map(|quest| quest["id"].as_i64().unwrap())));
    }
    native.insert("matches".into(), json!(matches));

    bus.publish(
        eo_services::event_bus::Topic::SessionStarted,
        &json!({"session_id": "sess-abc"}),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "filter_skill".into(),
        json!(svc
            .quest_reward_filter(
                "Daily Hunt: Atrox",
                &[],
                &[json!({"skill_name": "Rifle", "amount": 1.0})]
            )
            .await
            .unwrap()),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "filter_liquid".into(),
        json!(svc
            .quest_reward_filter(
                "Iron Challenge",
                &[
                    json!({"item_name": "Shrapnel", "quantity": 100, "value": 0.1}),
                    json!({"item_name": "Universal Ammo", "quantity": 1, "value": 2.51}),
                ],
                &[]
            )
            .await
            .unwrap()),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "filter_zero".into(),
        json!(svc
            .quest_reward_filter(
                "Zero Bounty",
                &[
                    json!({"item_name": "A", "value": 0.5}),
                    json!({"item_name": "B", "value": 0.2}),
                ],
                &[]
            )
            .await
            .unwrap()),
    );
    clock.advance(60.0).unwrap();
    svc.start_quest_from_mission("Feffox Cave Run")
        .await
        .unwrap();
    clock.advance(60.0).unwrap();
    native.insert(
        "cancelled".into(),
        json!(svc
            .cancel_quest(ids["Iron Challenge"].as_i64().unwrap(), true)
            .await
            .unwrap()
            .is_some()),
    );
    clock.advance(60.0).unwrap();
    svc.decline_session_link("sess-decl").await.unwrap();

    native.insert(
        "ledger".into(),
        json!(sqlx::query(
            "SELECT id, date, type, description, amount, tag FROM ledger_entries ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([
            row.get::<String, _>(0),
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<String, _>(3),
            row.get::<f64, _>(4),
            row.get::<String, _>(5)
        ]))
        .collect::<Vec<_>>()),
    );
    native.insert(
        "claims".into(),
        json!(sqlx::query(
            "SELECT quest_id, quest_name, ped_value, claimed_at FROM quest_claims ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([
            row.get::<i64, _>(0),
            row.get::<String, _>(1),
            row.get::<f64, _>(2),
            row.get::<f64, _>(3)
        ]))
        .collect::<Vec<_>>()),
    );
    native.insert(
        "completions".into(),
        json!(sqlx::query(
            "SELECT session_id, quest_id, completed_at FROM session_quest_completions ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([
            row.get::<String, _>(0),
            row.get::<i64, _>(1),
            row.get::<f64, _>(2)
        ]))
        .collect::<Vec<_>>()),
    );
    native.insert(
        "links".into(),
        json!(sqlx::query(
            "SELECT session_id, link_type, quest_id, playlist_id, linked_at \
             FROM session_quest_analytics_links ORDER BY session_id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([
            row.get::<String, _>(0),
            row.get::<String, _>(1),
            row.get::<Option<i64>, _>(2),
            row.get::<Option<i64>, _>(3),
            row.get::<f64, _>(4)
        ]))
        .collect::<Vec<_>>()),
    );
    native.insert(
        "events".into(),
        json!(sqlx::query(
            "SELECT session_id, event_type, mob_or_item, value_ped, timestamp \
             FROM notable_events ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([
            row.get::<String, _>(0),
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<f64, _>(3),
            row.get::<f64, _>(4)
        ]))
        .collect::<Vec<_>>()),
    );
    native.insert(
        "started".into(),
        json!(sqlx::query(
            "SELECT id, started_at FROM quests WHERE started_at IS NOT NULL ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| json!([row.get::<i64, _>(0), row.get::<f64, _>(1)]))
        .collect::<Vec<_>>()),
    );

    // The analytics readers over the same seeded economy.
    for (sid, st, en, active, heal, armour) in [
        (
            "an-1",
            1000.0,
            Some(4600.0),
            0i64,
            Some(1.5),
            Some(0.0_f64 + 0.25),
        ),
        ("an-2", 5000.0, Some(5030.5), 0, None, Some(0.0)),
    ] {
        sqlx::query(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(sid)
        .bind(st)
        .bind(en)
        .bind(active)
        .bind(heal)
        .bind(armour)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
         damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
         VALUES ('an-k1', 'an-1', 'Atrox', 1100.0, 10, 100.0, 5.0, 1, 0.3, 0.5, 12.75)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
         critical_hits, cost_per_shot) VALUES ('an-k1', 'LR-32', 40, 50.0, 0, 0.05)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
         VALUES ('an-1', 1100.0, 'Rifle', 1.0, 0.8)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let iron = ids["Iron Challenge"].as_i64().unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id, completed_at) \
         VALUES ('an-1', ?, 1500.0)",
    )
    .bind(iron)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO session_quest_analytics_links \
         (session_id, link_type, quest_id, playlist_id, linked_at) \
         VALUES ('an-1', 'quest', ?, NULL, 9000.0)",
    )
    .bind(iron)
    .execute(&pool)
    .await
    .unwrap();
    let zero = svc
        .match_quest_by_mission_name("Zero Bounty")
        .await
        .unwrap()
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let playlist = svc
        .create_playlist(&json!({"name": "Analytics Run", "items": [
            {"quest_id": iron, "group_type": "immediate"},
            {"quest_id": zero, "group_type": "long_horizon"},
        ]}))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    sqlx::query("UPDATE quest_playlists SET created_at=3000.0, updated_at=3000.0 WHERE id=?")
        .bind(playlist)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO session_quest_analytics_links \
         (session_id, link_type, quest_id, playlist_id, linked_at) \
         VALUES ('an-2', 'playlist', NULL, ?, 9001.0)",
    )
    .bind(playlist)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id, completed_at) \
         VALUES ('an-2', ?, 5020.0)",
    )
    .bind(iron)
    .execute(&pool)
    .await
    .unwrap();
    native.insert(
        "quest_analytics".into(),
        json!(svc.get_quest_analytics().await.unwrap()),
    );
    native.insert(
        "playlist_analytics".into(),
        json!(svc.get_all_playlist_analytics().await.unwrap()),
    );

    // ── The comparison, key by key for a readable failure ───────────
    let oracle = floats(oracle);
    let native = floats(Value::Object(native));
    let oracle_map = oracle.as_object().unwrap();
    let native_map = native.as_object().unwrap();
    let mut oracle_keys: Vec<_> = oracle_map.keys().collect();
    let mut native_keys: Vec<_> = native_map.keys().collect();
    oracle_keys.sort();
    native_keys.sort();
    assert_eq!(oracle_keys, native_keys, "surface key sets diverge");
    for key in oracle_keys {
        assert_eq!(
            native_map[key], oracle_map[key],
            "the native '{key}' surface diverges from the oracle"
        );
    }
}
