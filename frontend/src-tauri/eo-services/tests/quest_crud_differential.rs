//! Differential check: the native quest CRUD slice vs the Python
//! oracle, over one scripted create/update/delete sequence.
//!
//! Both sides start from a fresh database, walk the same sequence
//! (quest and playlist creation with payload-default and mob-rule
//! coverage, a seeded completion for the cooldown derivation, partial
//! updates with markup re-normalisation, soft deletes), and dump every
//! read surface plus the final mob and item tables. The schema stamps
//! `created_at`/`updated_at` from the wall clock, so both sides pin
//! them by direct UPDATE after each insert; everything else compares
//! exactly (integers project to floats first, the response models'
//! coercion).
//!
//! The two invalid-group error legs run LAST and compare messages
//! only: on a validation failure mid-rewrite the original leaves its
//! partial writes pending on the shared connection (a later commit
//! ratifies them), while the pooled port rolls the transaction back;
//! that repair is the migration's settled architecture, so post-error
//! state is deliberately out of the comparison.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test quest_crud_differential
#![cfg(feature = "cross-language")]

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use std::sync::Arc;

use chrono::NaiveDateTime;
use eo_services::clock::MockClock;
use eo_services::db::Db;
use eo_services::quests::{QuestError, QuestService};
use serde_json::{json, Map, Value};
use sqlx::{Row, SqlitePool};

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

const ORACLE_SCRIPT: &str = r#"
import json, sys
from pathlib import Path

target = Path(sys.argv[1])
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService

db = AppDatabase(target / "oracle.db")
svc = QuestService(db)

def pin(table, row_id, ts):
    db.conn.execute(f"UPDATE {table} SET created_at=?, updated_at=? WHERE id=?", (ts, ts, row_id))
    db.conn.commit()

out = {}
q1 = svc.create_quest({"name": "Iron Challenge"})["id"]; pin("quests", q1, 1000.0)
q2 = svc.create_quest({
    "name": "Atrox Cull", "planet": "Foma", "waypoint": "/wp 1,2",
    "cooldown_hours": 24, "reward_ped": 12.5, "reward_is_skill": False,
    "expected_reward_markup_percent": 150.0, "notes": "bring fap",
    "chain_name": "Cull", "chain_position": 1, "chain_total": 3,
    "category": "hunt", "reward_description": "ammo",
    "mobs": [" Atrox ", "", "Atrax", "Atrox"],
})["id"]; pin("quests", q2, 1001.0)
q3 = svc.create_quest({"name": "Skill Run", "reward_ped": 5.0,
                       "reward_is_skill": True,
                       "expected_reward_markup_percent": 120.0})["id"]; pin("quests", q3, 1002.0)
out["ids"] = [q1, q2, q3]
out["q1_fresh"] = svc.get_quest(q1)
out["q2_fresh"] = svc.get_quest(q2)
out["q3_fresh"] = svc.get_quest(q3)
out["q_missing"] = svc.get_quest(9999)

db.conn.execute(
    "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) VALUES ('sess-1', ?, 1772366400.0)",
    (q2,))
db.conn.commit()
out["q2_cooling"] = svc.get_quest(q2)

out["u1"] = svc.update_quest(q1, {"reward_ped": 10.0, "expected_reward_markup_percent": 130.0})
out["u2"] = svc.update_quest(q1, {"reward_is_skill": True})
out["u3"] = svc.update_quest(q2, {"mobs": ["Snablesnot"]})
out["u_missing"] = svc.update_quest(9999, {"name": "x"})

p1 = svc.create_playlist({"name": "Morning Run", "quest_ids": [q1, q2]})["id"]; pin("quest_playlists", p1, 2000.0)
p2 = svc.create_playlist({"name": "Big Loop", "planet": "Foma", "estimated_minutes": 90,
    "items": [{"quest_id": q2, "description": "warmup", "group_type": "immediate"},
              {"quest_id": q1, "group_type": "long_horizon"}]})["id"]; pin("quest_playlists", p2, 2001.0)
out["pids"] = [p1, p2]
out["p1"] = svc.get_playlist(p1)
out["p2"] = svc.get_playlist(p2)
out["p_missing"] = svc.get_playlist(9999)

out["u_pl"] = svc.update_playlist(p1, {"name": "Dawn Run", "quest_ids": [q2]})
out["u_pl_missing"] = svc.update_playlist(9999, {"name": "x"})
out["del_q3"] = svc.delete_quest(q3)
out["del_q3_again"] = svc.delete_quest(q3)
out["del_p2"] = svc.delete_playlist(p2)
out["del_p2_again"] = svc.delete_playlist(p2)
out["quests_active"] = svc.get_quests()
out["quests_all"] = svc.get_quests(active_only=False)
out["playlists_active"] = svc.get_playlists()
out["playlists_all"] = svc.get_playlists(active_only=False)
out["mob_names"] = svc.get_all_mob_names()
out["q1_final"] = svc.get_quest(q1)
out["items_table"] = [list(r) for r in db.conn.execute(
    "SELECT playlist_id, quest_id, sort_order, description, group_type FROM quest_playlist_items ORDER BY playlist_id, sort_order")]
out["mobs_table"] = [list(r) for r in db.conn.execute(
    "SELECT quest_id, mob_name FROM quest_mobs ORDER BY quest_id, mob_name")]

def err(fn, *a):
    try:
        fn(*a)
        return None
    except ValueError as e:
        return str(e)
out["err_bad_group"] = err(svc.create_playlist, {"name": "Bad", "items": [{"quest_id": q1, "group_type": "weekly"}]})
out["err_null_group"] = err(svc.update_playlist, p1, {"items": [{"quest_id": q1, "group_type": None}]})
print(json.dumps(out, sort_keys=True))
"#;

/// Project every integer to a float, recursively, so numeric equality
/// (the response models' coercion) is what the comparison tests.
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

fn invalid_message(error: QuestError) -> Value {
    match error {
        QuestError::Invalid(message) => json!(message),
        QuestError::Db(error) => panic!("expected a validation error, got: {error}"),
    }
}

async fn pin(pool: &SqlitePool, table: &str, row_id: i64, ts: f64) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET created_at=?, updated_at=? WHERE id=?"
    )))
    .bind(ts)
    .bind(ts)
    .bind(row_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_native_quest_crud_matches_the_python_oracle() {
    let dir = tempfile::tempdir().unwrap();

    // ── The oracle run ──────────────────────────────────────────────
    let mut command = Command::new(oracle_python());
    command
        .arg("-c")
        .arg(ORACLE_SCRIPT)
        .arg(dir.path().join("python"))
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
    let svc = QuestService::new(pool.clone(), clock);

    let mut native = Map::new();
    let q1 = svc
        .create_quest(&json!({"name": "Iron Challenge"}))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    pin(&pool, "quests", q1, 1000.0).await;
    let q2 = svc
        .create_quest(&json!({
            "name": "Atrox Cull", "planet": "Foma", "waypoint": "/wp 1,2",
            "cooldown_hours": 24, "reward_ped": 12.5, "reward_is_skill": false,
            "expected_reward_markup_percent": 150.0, "notes": "bring fap",
            "chain_name": "Cull", "chain_position": 1, "chain_total": 3,
            "category": "hunt", "reward_description": "ammo",
            "mobs": [" Atrox ", "", "Atrax", "Atrox"],
        }))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    pin(&pool, "quests", q2, 1001.0).await;
    let q3 = svc
        .create_quest(&json!({
            "name": "Skill Run", "reward_ped": 5.0, "reward_is_skill": true,
            "expected_reward_markup_percent": 120.0,
        }))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    pin(&pool, "quests", q3, 1002.0).await;
    native.insert("ids".into(), json!([q1, q2, q3]));
    native.insert("q1_fresh".into(), json!(svc.get_quest(q1).await.unwrap()));
    native.insert("q2_fresh".into(), json!(svc.get_quest(q2).await.unwrap()));
    native.insert("q3_fresh".into(), json!(svc.get_quest(q3).await.unwrap()));
    native.insert(
        "q_missing".into(),
        json!(svc.get_quest(9999).await.unwrap()),
    );

    sqlx::query(
        "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
         VALUES ('sess-1', ?, 1772366400.0)",
    )
    .bind(q2)
    .execute(&pool)
    .await
    .unwrap();
    native.insert("q2_cooling".into(), json!(svc.get_quest(q2).await.unwrap()));

    native.insert(
        "u1".into(),
        json!(svc
            .update_quest(
                q1,
                &json!({"reward_ped": 10.0, "expected_reward_markup_percent": 130.0})
            )
            .await
            .unwrap()),
    );
    native.insert(
        "u2".into(),
        json!(svc
            .update_quest(q1, &json!({"reward_is_skill": true}))
            .await
            .unwrap()),
    );
    native.insert(
        "u3".into(),
        json!(svc
            .update_quest(q2, &json!({"mobs": ["Snablesnot"]}))
            .await
            .unwrap()),
    );
    native.insert(
        "u_missing".into(),
        json!(svc.update_quest(9999, &json!({"name": "x"})).await.unwrap()),
    );

    let p1 = svc
        .create_playlist(&json!({"name": "Morning Run", "quest_ids": [q1, q2]}))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    pin(&pool, "quest_playlists", p1, 2000.0).await;
    let p2 = svc
        .create_playlist(&json!({
            "name": "Big Loop", "planet": "Foma", "estimated_minutes": 90,
            "items": [
                {"quest_id": q2, "description": "warmup", "group_type": "immediate"},
                {"quest_id": q1, "group_type": "long_horizon"},
            ],
        }))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    pin(&pool, "quest_playlists", p2, 2001.0).await;
    native.insert("pids".into(), json!([p1, p2]));
    native.insert("p1".into(), json!(svc.get_playlist(p1).await.unwrap()));
    native.insert("p2".into(), json!(svc.get_playlist(p2).await.unwrap()));
    native.insert(
        "p_missing".into(),
        json!(svc.get_playlist(9999).await.unwrap()),
    );

    native.insert(
        "u_pl".into(),
        json!(svc
            .update_playlist(p1, &json!({"name": "Dawn Run", "quest_ids": [q2]}))
            .await
            .unwrap()),
    );
    native.insert(
        "u_pl_missing".into(),
        json!(svc
            .update_playlist(9999, &json!({"name": "x"}))
            .await
            .unwrap()),
    );
    native.insert("del_q3".into(), json!(svc.delete_quest(q3).await.unwrap()));
    native.insert(
        "del_q3_again".into(),
        json!(svc.delete_quest(q3).await.unwrap()),
    );
    native.insert(
        "del_p2".into(),
        json!(svc.delete_playlist(p2).await.unwrap()),
    );
    native.insert(
        "del_p2_again".into(),
        json!(svc.delete_playlist(p2).await.unwrap()),
    );
    native.insert(
        "quests_active".into(),
        json!(svc.get_quests(true).await.unwrap()),
    );
    native.insert(
        "quests_all".into(),
        json!(svc.get_quests(false).await.unwrap()),
    );
    native.insert(
        "playlists_active".into(),
        json!(svc.get_playlists(true).await.unwrap()),
    );
    native.insert(
        "playlists_all".into(),
        json!(svc.get_playlists(false).await.unwrap()),
    );
    native.insert(
        "mob_names".into(),
        json!(svc.get_all_mob_names().await.unwrap()),
    );
    native.insert("q1_final".into(), json!(svc.get_quest(q1).await.unwrap()));

    let items = sqlx::query(
        "SELECT playlist_id, quest_id, sort_order, description, group_type \
         FROM quest_playlist_items ORDER BY playlist_id, sort_order",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    native.insert(
        "items_table".into(),
        json!(items
            .iter()
            .map(|row| {
                json!([
                    row.get::<i64, _>(0),
                    row.get::<i64, _>(1),
                    row.get::<i64, _>(2),
                    row.get::<Option<String>, _>(3),
                    row.get::<String, _>(4),
                ])
            })
            .collect::<Vec<_>>()),
    );
    let mobs = sqlx::query("SELECT quest_id, mob_name FROM quest_mobs ORDER BY quest_id, mob_name")
        .fetch_all(&pool)
        .await
        .unwrap();
    native.insert(
        "mobs_table".into(),
        json!(mobs
            .iter()
            .map(|row| json!([row.get::<i64, _>(0), row.get::<String, _>(1)]))
            .collect::<Vec<_>>()),
    );

    // The error legs, last (message comparison only; see the module
    // doc for why post-error state stays out of the comparison).
    native.insert(
        "err_bad_group".into(),
        invalid_message(
            svc.create_playlist(&json!({
                "name": "Bad",
                "items": [{"quest_id": q1, "group_type": "weekly"}],
            }))
            .await
            .unwrap_err(),
        ),
    );
    native.insert(
        "err_null_group".into(),
        invalid_message(
            svc.update_playlist(
                p1,
                &json!({"items": [{"quest_id": q1, "group_type": null}]}),
            )
            .await
            .unwrap_err(),
        ),
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
