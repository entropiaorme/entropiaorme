//! Schema-conformance acceptance: a freshly-migrated native database
//! against a freshly-created backend database.
//!
//! The backend's fresh-install schema is the contract the migration
//! baseline copies. This test creates BOTH databases (the backend one
//! through the real `AppDatabase` + tracking-schema initialisation, the
//! native one through the migration chain), then asserts:
//!
//! 1. identical `sqlite_master` definitions (type, name, statement text),
//!    with the native side's migration ledger as the one deliberate
//!    difference; and
//! 2. identical serialised output of an empty DB-state snapshot, each
//!    side produced by its own capture implementation.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test schema_conformance
//!
//! The interpreter is `$EO_ORACLE_PYTHON` if set, else the local
//! virtualenv (`.venv/Scripts/python.exe` on Windows, `.venv/bin/python`
//! elsewhere).
#![cfg(feature = "cross-language")]

use std::path::PathBuf;
use std::process::Command;

use eo_services::db::Db;
use eo_wire::normalizer::Normalizer;

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
from backend.tracking.schema import init_tracking_tables
from backend.testing.db_snapshot import capture, serialize

db = AppDatabase(target / "entropia_orme.db")
init_tracking_tables(db.conn)
rows = db.conn.execute(
    "SELECT type, name, sql FROM sqlite_master "
    "WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' "
    "ORDER BY type, name"
).fetchall()
print(json.dumps({
    "master": [[r["type"], r["name"], r["sql"]] for r in rows],
    "empty_snapshot": serialize(capture(db.conn)),
}))
"#;

#[tokio::test]
async fn fresh_native_database_matches_the_backend_fresh_install() {
    // Backend side: the oracle creates its database and reports.
    let oracle_dir = tempfile::tempdir().unwrap();
    let output = Command::new(oracle_python())
        .args(["-c", ORACLE_SCRIPT])
        .arg(oracle_dir.path())
        .current_dir(repo_root())
        .output()
        .expect("oracle spawn");
    assert!(
        output.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let oracle: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("oracle output parses");

    // Native side: the migration chain creates its database.
    let native_dir = tempfile::tempdir().unwrap();
    let db = Db::open(&native_dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    let pool_rows = db.snapshot_rows().await.unwrap();
    let mut normalizer = Normalizer::new();
    let native_snapshot = eo_wire::db_snapshot::serialize(&eo_wire::db_snapshot::capture(
        &pool_rows,
        &mut normalizer,
    ));

    // The native master dump, excluding the migration ledger (the one
    // deliberate difference) and SQLite's own bookkeeping.
    let native_master: Vec<(String, String, String)> = {
        let raw = db.schema_master().await.unwrap();
        raw.into_iter()
            .filter(|(_, name, _)| name != "_sqlx_migrations")
            .collect()
    };
    let oracle_master: Vec<(String, String, String)> = oracle["master"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry[0].as_str().unwrap().to_string(),
                entry[1].as_str().unwrap().to_string(),
                entry[2].as_str().unwrap().to_string(),
            )
        })
        .collect();

    assert_eq!(
        native_master.len(),
        oracle_master.len(),
        "schema object counts diverged"
    );
    for (native, oracle) in native_master.iter().zip(oracle_master.iter()) {
        assert_eq!(native, oracle, "sqlite_master entry diverged");
    }

    // Empty-snapshot identity, each side via its own serialiser.
    assert_eq!(
        native_snapshot,
        oracle["empty_snapshot"].as_str().unwrap(),
        "empty snapshot output diverged"
    );
}
