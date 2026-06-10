//! The persistence gating spike: the replay corpus through the native
//! database layer, byte-compared against the committed goldens.
//!
//! The backend's replay suite materialises each scenario's final
//! database (the `EO_DB_DUMP_DIR` hook in the golden harness); this
//! test then proves, per scenario:
//!
//! - READ LEG: the native catalogue snapshot over the materialised
//!   database serialises byte-identically to the committed
//!   `expected/db_state.json` (query order-bys, COALESCE/NULL
//!   handling, float rendering, shared-symbol normalisation).
//! - WRITE LEG: re-inserting every row through the native pool into a
//!   freshly migrated database (insertion order preserved) and
//!   snapshotting again still matches, proving bind round-trips and
//!   pragma parity on the write path.
//!
//! One scenario carries a golden but no dump on hosts without the
//! captured OCR panels (its producing test skips there); the spike
//! reports it as skipped rather than silently shrinking coverage.
//!
//! Gated behind the `cross-language` feature: it spawns the backend's
//! pytest replay suite when the dumps are missing. Run with:
//!   cargo test -p eo-services --features cross-language --test db_replay_spike
#![cfg(feature = "cross-language")]

use std::path::{Path, PathBuf};
use std::process::Command;

use eo_wire::db_snapshot::{capture, serialize, CATALOGUE};
use eo_wire::normalizer::Normalizer;
use serde_json::{Map, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Column, Row, SqlitePool};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn dumps_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/db-dumps")
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

/// Every corpus scenario carrying a committed db_state golden.
fn golden_scenarios() -> Vec<(String, PathBuf)> {
    let corpus = repo_root().join("backend/tests/e2e/corpus");
    let mut out = Vec::new();
    for family in ["scripted", "recorded"] {
        let Ok(entries) = std::fs::read_dir(corpus.join(family)) else {
            continue;
        };
        for entry in entries.flatten() {
            let golden = entry.path().join("expected/db_state.json");
            if golden.exists() {
                out.push((entry.file_name().to_string_lossy().into_owned(), golden));
            }
        }
    }
    out.sort();
    assert!(
        out.len() >= 12,
        "the corpus carries at least twelve db_state goldens"
    );
    out
}

/// Materialise the scenario databases by running the backend's replay
/// suite with the dump hook armed (skipped when every dump already
/// exists from a previous run).
fn ensure_dumps(scenarios: &[(String, PathBuf)]) {
    let dir = dumps_dir();
    let missing: Vec<&str> = scenarios
        .iter()
        .filter(|(name, _)| !dir.join(format!("{name}.db")).exists())
        .map(|(name, _)| name.as_str())
        .collect();
    // The OCR-equivalence scenario cannot materialise on hosts without
    // the captured panels; a full re-run cannot change that, so only
    // re-run when a non-OCR dump is missing.
    if missing.iter().all(|name| *name == "hunt_with_skill_scan") {
        return;
    }
    std::fs::create_dir_all(&dir).expect("dump dir");
    let status = Command::new(oracle_python())
        .args(["-m", "pytest", "backend/tests/e2e", "-q", "--no-header"])
        .env("EO_DB_DUMP_DIR", &dir)
        .current_dir(repo_root())
        .status()
        .expect("replay suite spawn");
    assert!(status.success(), "the backend replay suite must pass");
}

async fn pool_over(path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .pragma("busy_timeout", "5000");
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("pool over the materialised database")
}

/// The catalogue snapshot with the scenario's dumped symbol tables
/// pre-seeded, reproducing the shared-normaliser numbering that the
/// event stream consumed before the snapshot.
async fn native_snapshot_seeded(pool: &SqlitePool, symbols_path: &Path) -> String {
    let raw = std::fs::read_to_string(symbols_path).expect("symbols dump");
    let symbols: Value = serde_json::from_str(&raw).expect("symbols parse");
    let empty = Map::new();
    let uuids = symbols
        .get("uuids")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let timestamps = symbols
        .get("timestamps")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let mut tables = Map::new();
    for spec in CATALOGUE {
        let exists: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(spec.name)
                .fetch_optional(pool)
                .await
                .expect("table existence probe");
        if exists.is_none() {
            tables.insert(spec.name.to_string(), Value::Array(Vec::new()));
            continue;
        }
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
    let mut normalizer = Normalizer::new();
    normalizer.seed_symbols(uuids, timestamps);
    serialize(&capture(&tables, &mut normalizer))
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

/// Re-insert every user table's rows (schema order, rowid order) into
/// a freshly migrated native database.
async fn replay_writes(source: &SqlitePool, fresh: &eo_services::db::Db) -> SqlitePool {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' ORDER BY rowid",
    )
    .fetch_all(source)
    .await
    .expect("table listing");

    for table in &tables {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM \"{table}\" ORDER BY rowid"
        )))
        .fetch_all(source)
        .await
        .expect("source rows");
        for row in &rows {
            let columns: Vec<String> = row
                .columns()
                .iter()
                .map(|c| format!("\"{}\"", c.name()))
                .collect();
            let placeholders: Vec<&str> = row.columns().iter().map(|_| "?").collect();
            let sql = format!(
                "INSERT INTO \"{table}\" ({}) VALUES ({})",
                columns.join(", "),
                placeholders.join(", ")
            );
            let mut insert = sqlx::query(sqlx::AssertSqlSafe(sql));
            for column in row.columns() {
                let index = column.ordinal();
                if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
                    insert = insert.bind(value);
                } else if let Ok(value) = row.try_get::<Option<f64>, _>(index) {
                    insert = insert.bind(value);
                } else if let Ok(value) = row.try_get::<Option<String>, _>(index) {
                    insert = insert.bind(value);
                } else {
                    panic!("unsupported column type in {table}.{}", column.name());
                }
            }
            insert.execute(fresh.pool()).await.expect("replayed insert");
        }
    }
    fresh.pool().clone()
}

#[tokio::test]
async fn replay_corpus_matches_the_committed_db_state_goldens() {
    let scenarios = golden_scenarios();
    ensure_dumps(&scenarios);

    let mut compared = 0usize;
    let mut skipped = Vec::new();
    for (name, golden_path) in &scenarios {
        let dump = dumps_dir().join(format!("{name}.db"));
        if !dump.exists() {
            skipped.push(name.clone());
            continue;
        }
        let expected = std::fs::read_to_string(golden_path).expect("golden read");
        let source = pool_over(&dump).await;

        let symbols_path = dumps_dir().join(format!("{name}.symbols.json"));

        // READ LEG.
        let read_snapshot = native_snapshot_seeded(&source, &symbols_path).await;
        assert_eq!(
            read_snapshot, expected,
            "{name}: read-leg snapshot diverged from the committed golden"
        );

        // WRITE LEG.
        let fresh_dir = tempfile::tempdir().expect("fresh dir");
        let fresh = eo_services::db::Db::open(&fresh_dir.path().join("entropia_orme.db"))
            .await
            .expect("fresh native database");
        let fresh_pool = replay_writes(&source, &fresh).await;
        let write_snapshot = native_snapshot_seeded(&fresh_pool, &symbols_path).await;
        assert_eq!(
            write_snapshot, expected,
            "{name}: write-leg snapshot diverged from the committed golden"
        );
        compared += 1;
    }

    assert!(
        compared >= 12,
        "the spike must compare at least the twelve host-runnable scenarios"
    );
    assert!(
        skipped.iter().all(|name| name == "hunt_with_skill_scan"),
        "only the OCR-gated scenario may lack a dump; skipped: {skipped:?}"
    );
    println!("spike compared {compared} scenarios; skipped {skipped:?}");
}
