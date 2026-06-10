//! The persistence base: one SQLite database behind a narrow handle.
//!
//! Design decisions, mirroring `backend/db/base.py` and the porting
//! references:
//!
//! - **Single-owner connection**: a `SqlitePool` capped at one
//!   connection, so every statement serialises exactly as the backend's
//!   single shared connection does today. Relaxing to N readers is a
//!   later, benchmark-justified change re-validated against the DB-state
//!   goldens.
//! - **Identical session configuration**: WAL journal, NORMAL
//!   synchronous, a 5-second busy timeout, and an 8 MB page cache.
//! - **Schema baseline**: the migration chain starts at the schema the
//!   backend creates on a fresh install (version 33), statement text
//!   verbatim, so a freshly-migrated native database is
//!   `sqlite_master`-identical to a freshly-created backend one.
//! - **Adoption over re-creation**: opening an existing database that
//!   the backend has already migrated to version 33 marks the baseline
//!   as applied without running any DDL. Databases on older schema
//!   versions are refused: the backend process owns their upgrade for as
//!   long as it ships, and the pre-baseline upgrade chain moves natively
//!   only when that ownership ends.
//!
//! No driver type escapes this module's API: callers see [`Db`],
//! [`DbError`], and plain data.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::Row;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The schema version the baseline migration reproduces.
const BASELINE_SCHEMA_VERSION: i64 = 33;

#[derive(Debug)]
pub enum DbError {
    /// The on-disk schema predates the supported baseline; the backend
    /// process upgrades it on its own launch.
    UnsupportedSchemaVersion { found: i64, supported: i64 },
    /// Any driver or migration failure.
    Driver(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "database schema version {found} predates the supported baseline {supported}"
            ),
            DbError::Driver(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        DbError::Driver(err.to_string())
    }
}

impl From<sqlx::migrate::MigrateError> for DbError {
    fn from(err: sqlx::migrate::MigrateError) -> Self {
        DbError::Driver(err.to_string())
    }
}

/// The application database handle.
#[derive(Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if missing), adopt or refuse an existing schema,
    /// and bring the migration chain up to date.
    pub async fn open(path: &Path) -> Result<Db, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            // 8 MB page cache, matching the backend's configuration.
            .pragma("cache_size", "-8000");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        adopt_or_refuse(&pool).await?;
        MIGRATOR.run(&pool).await?;
        Ok(Db { pool })
    }

    /// The catalogue rows for the DB-state snapshot, each table in its
    /// deterministic order, shaped for the snapshot emitter.
    pub async fn snapshot_rows(&self) -> Result<Map<String, Value>, DbError> {
        snapshot_rows(&self.pool).await
    }

    /// One equipment-library row by id and item type: (id, name,
    /// properties JSON), or None when absent. The typed accessor the
    /// trifecta resolution reads through.
    pub async fn equipment_item(
        &self,
        id: i64,
        item_type: &str,
    ) -> Result<Option<(i64, String, String)>, DbError> {
        let row = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, name, properties_json FROM equipment_library \
             WHERE id = ? AND item_type = ?",
        )
        .bind(id)
        .bind(item_type)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Test seeding for equipment-reading services (compiled into the
    /// crate's own test builds only).
    #[cfg(test)]
    pub(crate) async fn insert_equipment_for_tests(
        &self,
        id: i64,
        name: &str,
        item_type: &str,
        properties_json: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO equipment_library (id, name, item_type, properties_json) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(item_type)
        .bind(properties_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The schema objects as (type, name, statement) in (type, name)
    /// order, excluding SQLite's own bookkeeping tables: the surface the
    /// schema-conformance acceptance compares across implementations.
    pub async fn schema_master(&self) -> Result<Vec<(String, String, String)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL \
             AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

/// Mark the baseline as applied on a database the backend has already
/// created at the baseline version; refuse older schemas.
async fn adopt_or_refuse(pool: &SqlitePool) -> Result<(), DbError> {
    let has_metadata = table_exists(pool, "db_metadata").await?;
    if !has_metadata {
        // A fresh (or empty) database: the migration chain owns it.
        return Ok(());
    }
    if table_exists(pool, "_sqlx_migrations").await? {
        // Already adopted (or natively created); the chain validates.
        return Ok(());
    }
    let version: Option<String> =
        sqlx::query_scalar("SELECT value FROM db_metadata WHERE key = 'version'")
            .fetch_optional(pool)
            .await?;
    let version: i64 = version.and_then(|raw| raw.parse().ok()).unwrap_or_default();
    if version < BASELINE_SCHEMA_VERSION {
        return Err(DbError::UnsupportedSchemaVersion {
            found: version,
            supported: BASELINE_SCHEMA_VERSION,
        });
    }

    // The ledger row sqlx's own runner would have written had it created
    // the schema; the post-adoption `MIGRATOR.run` validates it (version
    // and checksum), so drift in this DDL or the row fails loudly.
    let baseline = MIGRATOR
        .migrations
        .first()
        .expect("the migration chain carries the baseline");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (\
         version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
         installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, \
         execution_time BIGINT NOT NULL)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (?, ?, TRUE, ?, 0)",
    )
    .bind(baseline.version)
    .bind(baseline.description.as_ref())
    .bind(baseline.checksum.as_ref())
    .execute(pool)
    .await?;
    Ok(())
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool, DbError> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// One row as the snapshot emitter expects it: column-ordered keys, JSON
/// values typed by the stored value (integer, real, text, null).
fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Result<Value, DbError> {
    use sqlx::{Column, TypeInfo, ValueRef};
    let mut object = Map::new();
    for column in row.columns() {
        let raw = row
            .try_get_raw(column.ordinal())
            .map_err(|e| DbError::Driver(e.to_string()))?;
        let value = if raw.is_null() {
            Value::Null
        } else {
            match raw.type_info().name() {
                "INTEGER" | "BOOLEAN" => Value::from(
                    row.try_get::<i64, _>(column.ordinal())
                        .map_err(|e| DbError::Driver(e.to_string()))?,
                ),
                "REAL" => Value::from(
                    row.try_get::<f64, _>(column.ordinal())
                        .map_err(|e| DbError::Driver(e.to_string()))?,
                ),
                "TEXT" => Value::from(
                    row.try_get::<String, _>(column.ordinal())
                        .map_err(|e| DbError::Driver(e.to_string()))?,
                ),
                other => {
                    return Err(DbError::Driver(format!(
                        "unsupported value type {other} in column {}",
                        column.name()
                    )))
                }
            }
        };
        object.insert(column.name().to_string(), value);
    }
    Ok(Value::Object(object))
}

/// Execute the snapshot catalogue: each table's query with its
/// deterministic ORDER BY, exactly as the catalogue documents.
async fn snapshot_rows(pool: &SqlitePool) -> Result<Map<String, Value>, DbError> {
    let mut tables = Map::new();
    for spec in eo_wire::db_snapshot::CATALOGUE {
        // Composed exclusively from the catalogue's compile-time constants
        // (no caller input reaches this string), so the safety assertion
        // is genuine rather than a lint bypass.
        let sql = format!("{} ORDER BY {}", spec.query, spec.order_by.join(", "));
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(pool)
            .await?;
        let mut json_rows = Vec::with_capacity(rows.len());
        for row in &rows {
            json_rows.push(row_to_json(row)?);
        }
        tables.insert(spec.name.to_string(), Value::Array(json_rows));
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_db(dir: &std::path::Path) -> Db {
        Db::open(&dir.join("entropia_orme.db")).await.unwrap()
    }

    #[tokio::test]
    async fn fresh_database_migrates_with_session_pragmas_in_effect() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(journal, "wal");
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(synchronous, 1, "NORMAL");
        let cache: i64 = sqlx::query_scalar("PRAGMA cache_size")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(cache, -8000);
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(busy, 5000);
    }

    #[tokio::test]
    async fn baseline_creates_the_full_schema_surface_and_version_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        let count = |kind: &'static str| {
            let pool = db.pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = ? AND sql IS NOT NULL \
                     AND name != '_sqlx_migrations' AND name NOT LIKE 'sqlite_%'",
                )
                .bind(kind)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        // The fresh backend schema at version 33: 23 declared tables
        // (sqlite_sequence arrives automatically), 18 indexes, 8 triggers.
        assert_eq!(count("table").await, 23);
        assert_eq!(count("index").await, 18);
        assert_eq!(count("trigger").await, 8);

        let version: String =
            sqlx::query_scalar("SELECT value FROM db_metadata WHERE key = 'version'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(version, "33");
    }

    #[tokio::test]
    async fn empty_database_snapshot_yields_the_six_empty_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let rows = db.snapshot_rows().await.unwrap();
        assert_eq!(rows.len(), 6);
        for (table, value) in &rows {
            assert_eq!(
                value.as_array().map(Vec::len),
                Some(0),
                "{table} should be empty"
            );
        }
    }

    #[tokio::test]
    async fn snapshot_rows_carry_typed_values_in_deterministic_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        sqlx::query(
            "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES \
             ('s-2', 200.0, 0), ('s-1', 100.0, 1)",
        )
        .execute(&db.pool)
        .await
        .unwrap();

        let rows = db.snapshot_rows().await.unwrap();
        let sessions = rows["tracking_sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        // rowid order: insertion order, not id order.
        assert_eq!(sessions[0]["id"], "s-2");
        assert_eq!(sessions[0]["started_at"], 200.0);
        assert_eq!(sessions[0]["is_active"], 0);
        assert_eq!(sessions[0]["heal_cost"], 0.0, "COALESCE default");
        assert_eq!(sessions[1]["id"], "s-1");
    }

    #[tokio::test]
    async fn backend_created_baseline_database_is_adopted_with_data_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        // First open: the chain creates everything. Simulate the backend's
        // ownership by seeding data and DELETING the sqlx ledger, leaving
        // exactly what a backend-created version-33 database looks like.
        {
            let db = Db::open(&path).await.unwrap();
            sqlx::query(
                "INSERT INTO tracking_sessions (id, started_at, is_active) \
                 VALUES ('kept', 1.0, 0)",
            )
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query("DROP TABLE _sqlx_migrations")
                .execute(&db.pool)
                .await
                .unwrap();
        }

        // Re-open: adoption must mark the baseline applied without DDL,
        // and the post-adoption run must validate the ledger row.
        let db = Db::open(&path).await.unwrap();
        let kept: String = sqlx::query_scalar("SELECT id FROM tracking_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(kept, "kept");
        let ledger: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(ledger, 1);
    }

    #[tokio::test]
    async fn pre_baseline_schema_versions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        {
            let db = Db::open(&path).await.unwrap();
            sqlx::query("UPDATE db_metadata SET value = '28' WHERE key = 'version'")
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("DROP TABLE _sqlx_migrations")
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let err = Db::open(&path).await.unwrap_err();
        match err {
            DbError::UnsupportedSchemaVersion { found, supported } => {
                assert_eq!((found, supported), (28, 33));
            }
            other => panic!("expected a schema-version refusal, got {other}"),
        }
    }

    #[tokio::test]
    async fn schema_master_lists_the_real_objects_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let master = db.schema_master().await.unwrap();
        // 23 declared tables + the migration ledger + 18 indexes +
        // 8 triggers (only SQLite's own bookkeeping is excluded; the
        // conformance comparison filters the ledger externally as its
        // one deliberate difference).
        assert_eq!(master.len(), 24 + 18 + 8);
        let mut sorted = master.clone();
        sorted.sort();
        assert_eq!(master, sorted, "ordered by (type, name)");
        assert!(master.iter().any(|(kind, name, sql)| {
            kind == "table"
                && name == "tracking_sessions"
                && sql.contains("CREATE TABLE tracking_sessions")
        }));
        assert!(master.iter().any(|(_, name, _)| name == "_sqlx_migrations"));
    }

    #[test]
    fn refusal_error_formats_the_exact_message() {
        let err = DbError::UnsupportedSchemaVersion {
            found: 28,
            supported: 33,
        };
        assert_eq!(
            err.to_string(),
            "database schema version 28 predates the supported baseline 33"
        );
    }
}
