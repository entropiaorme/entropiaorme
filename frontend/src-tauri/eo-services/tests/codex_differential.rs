//! Differential check: the native codex service vs the Python oracle,
//! over the real game-data catalogue and one scripted claim sequence.
//!
//! Both sides start from a fresh database and identical calibration
//! seeds, walk the same sequence (claims, calibration, a meta claim,
//! every validation leg), and dump every read surface plus the final
//! claim/progress/calibration tables. The comparison projects integers
//! to floats first: the service layer emits whichever numeric type the
//! catalogue carries (some base costs are integers), and the HTTP
//! response models coerce them to floats, so value equality is the
//! contract.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test codex_differential
#![cfg(feature = "cross-language")]

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use chrono::NaiveDateTime;
use eo_services::clock::MockClock;
use eo_services::codex::{CodexError, CodexService};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
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

/// The same calibration seed both sides insert before the sequence.
const SEEDS: [(&str, f64, f64); 7] = [
    ("Rifle", 42.5, 100.0),
    ("Rifle", 48.25, 200.0),
    ("Aim", 12.0, 150.0),
    ("Dodge", 30.0, 150.0),
    ("Agility", 32.04, 150.0),
    ("Zoology", 7.5, 150.0),
    ("Athletics", 5.0, 150.0),
];

const ORACLE_SCRIPT: &str = r#"
import json, sys
from datetime import datetime
from pathlib import Path

target = Path(sys.argv[1])
from backend.db.app_database import AppDatabase
from backend.services.codex_service import CodexService
from backend.services.game_data_store import GameDataStore
from backend.testing.clock import MockClock

db = AppDatabase(target / "oracle.db")
store = GameDataStore(Path("backend/data/snapshot"))
clock = MockClock(start=datetime(2026, 3, 1, 12, 0, 0))
svc = CodexService(db, store, clock)

for name, level, at in json.loads(sys.argv[2]):
    db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        (name, level, at))
db.conn.commit()

def err(fn, *a):
    try:
        fn(*a)
        return None
    except ValueError as e:
        return str(e)

out = {}
out["all_species_initial"] = svc.get_all_species()
out["claim_boar_1"] = svc.claim_rank("Boar", 1, "Rifle"); clock.advance(60)
out["claim_boar_2"] = svc.claim_rank("Boar", 2, "Anatomy"); clock.advance(60)
out["calibrate_zeladoth"] = svc.calibrate("Zeladoth", 4)
out["claim_zeladoth_5"] = svc.claim_rank("Zeladoth", 5, "Zoology"); clock.advance(60)
out["meta_claim"] = svc.meta_claim("Agility"); clock.advance(60)
out["errors"] = {
    "unknown_species": err(svc.claim_rank, "Nessie", 1, "Rifle"),
    "wrong_next": err(svc.claim_rank, "Boar", 7, "Rifle"),
    "bad_skill": err(svc.claim_rank, "Boar", 3, "Rifle"),
    "cat4_on_plain_rank": err(svc.claim_rank, "Zeladoth", 6, "Zoology"),
    "calibrate_low": err(svc.calibrate, "Boar", -1),
    "calibrate_high": err(svc.calibrate, "Boar", 26),
    "meta_invalid": err(svc.meta_claim, "Luck"),
}
out["boar_ranks"] = svc.get_species_ranks("Boar")
out["zeladoth_ranks"] = svc.get_species_ranks("Zeladoth")
out["ranks_unknown"] = svc.get_species_ranks("Nessie")
out["options_profession"] = svc.get_skill_options("Boar", 1, "BLP Sniper (Hit)", "profession")
out["options_hp"] = svc.get_skill_options("Zeladoth", 5, None, "hp")
out["options_unweighted"] = svc.get_skill_options("Boar", 3, None, "profession")
out["meta_attributes"] = svc.get_meta_attributes()
out["all_species_final"] = svc.get_all_species()
out["table_codex_claims"] = [list(r) for r in db.conn.execute(
    "SELECT species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name FROM codex_claims ORDER BY id")]
out["table_codex_progress"] = [list(r) for r in db.conn.execute(
    "SELECT species_name, current_rank, updated_at FROM codex_progress ORDER BY species_name")]
out["table_skill_calibrations"] = [list(r) for r in db.conn.execute(
    "SELECT skill_name, level, source, scanned_at FROM skill_calibrations ORDER BY id")]
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

fn invalid_message(error: CodexError) -> Value {
    match error {
        CodexError::Invalid(message) => json!(message),
        CodexError::Db(error) => panic!("expected a validation error, got: {error}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_native_codex_service_matches_the_python_oracle() {
    let dir = tempfile::tempdir().unwrap();

    // ── The oracle run ──────────────────────────────────────────────
    let mut command = Command::new(oracle_python());
    command
        .arg("-c")
        .arg(ORACLE_SCRIPT)
        .arg(dir.path().join("python"))
        .arg(
            serde_json::to_string(&json!(SEEDS
                .iter()
                .map(|(name, level, at)| json!([name, level, at]))
                .collect::<Vec<_>>()))
            .unwrap(),
        )
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
    let store = Arc::new(GameDataStore::new(&repo_root().join("backend/data/snapshot")).unwrap());
    let clock = Arc::new(MockClock::new(
        Some(NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap()),
        0.0,
    ));
    let svc = CodexService::new(pool.clone(), store, clock.clone());

    for (name, level, at) in SEEDS {
        sqlx::query(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, 'scan', ?)",
        )
        .bind(name)
        .bind(level)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut native = Map::new();
    native.insert(
        "all_species_initial".into(),
        json!(svc.get_all_species().await.unwrap()),
    );
    native.insert(
        "claim_boar_1".into(),
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap(),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "claim_boar_2".into(),
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap(),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "calibrate_zeladoth".into(),
        svc.calibrate("Zeladoth", 4).await.unwrap(),
    );
    native.insert(
        "claim_zeladoth_5".into(),
        svc.claim_rank("Zeladoth", 5, "Zoology").await.unwrap(),
    );
    clock.advance(60.0).unwrap();
    native.insert(
        "meta_claim".into(),
        svc.meta_claim("Agility").await.unwrap(),
    );
    clock.advance(60.0).unwrap();

    native.insert(
        "errors".into(),
        json!({
            "unknown_species":
                invalid_message(svc.claim_rank("Nessie", 1, "Rifle").await.unwrap_err()),
            "wrong_next": invalid_message(svc.claim_rank("Boar", 7, "Rifle").await.unwrap_err()),
            "bad_skill": invalid_message(svc.claim_rank("Boar", 3, "Rifle").await.unwrap_err()),
            "cat4_on_plain_rank":
                invalid_message(svc.claim_rank("Zeladoth", 6, "Zoology").await.unwrap_err()),
            "calibrate_low": invalid_message(svc.calibrate("Boar", -1).await.unwrap_err()),
            "calibrate_high": invalid_message(svc.calibrate("Boar", 26).await.unwrap_err()),
            "meta_invalid": invalid_message(svc.meta_claim("Luck").await.unwrap_err()),
        }),
    );

    native.insert(
        "boar_ranks".into(),
        svc.get_species_ranks("Boar").await.unwrap().unwrap(),
    );
    native.insert(
        "zeladoth_ranks".into(),
        svc.get_species_ranks("Zeladoth").await.unwrap().unwrap(),
    );
    native.insert(
        "ranks_unknown".into(),
        json!(svc.get_species_ranks("Nessie").await.unwrap()),
    );
    native.insert(
        "options_profession".into(),
        json!(svc
            .get_skill_options("Boar", 1, Some("BLP Sniper (Hit)"), "profession")
            .await
            .unwrap()),
    );
    native.insert(
        "options_hp".into(),
        json!(svc
            .get_skill_options("Zeladoth", 5, None, "hp")
            .await
            .unwrap()),
    );
    native.insert(
        "options_unweighted".into(),
        json!(svc
            .get_skill_options("Boar", 3, None, "profession")
            .await
            .unwrap()),
    );
    native.insert(
        "meta_attributes".into(),
        json!(svc.get_meta_attributes().await.unwrap()),
    );
    native.insert(
        "all_species_final".into(),
        json!(svc.get_all_species().await.unwrap()),
    );

    let claims = sqlx::query(
        "SELECT species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name \
         FROM codex_claims ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    native.insert(
        "table_codex_claims".into(),
        json!(claims
            .iter()
            .map(|row| {
                json!([
                    row.get::<String, _>(0),
                    row.get::<i64, _>(1),
                    row.get::<String, _>(2),
                    row.get::<f64, _>(3),
                    row.get::<f64, _>(4),
                    row.get::<String, _>(5),
                    row.get::<Option<String>, _>(6),
                ])
            })
            .collect::<Vec<_>>()),
    );
    let progress = sqlx::query(
        "SELECT species_name, current_rank, updated_at FROM codex_progress \
         ORDER BY species_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    native.insert(
        "table_codex_progress".into(),
        json!(progress
            .iter()
            .map(|row| {
                json!([
                    row.get::<String, _>(0),
                    row.get::<i64, _>(1),
                    row.get::<f64, _>(2),
                ])
            })
            .collect::<Vec<_>>()),
    );
    let calibrations = sqlx::query(
        "SELECT skill_name, level, source, scanned_at FROM skill_calibrations ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    native.insert(
        "table_skill_calibrations".into(),
        json!(calibrations
            .iter()
            .map(|row| {
                json!([
                    row.get::<String, _>(0),
                    row.get::<f64, _>(1),
                    row.get::<String, _>(2),
                    row.get::<f64, _>(3),
                ])
            })
            .collect::<Vec<_>>()),
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
