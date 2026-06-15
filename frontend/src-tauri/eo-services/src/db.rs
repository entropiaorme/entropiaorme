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

//!
//! Queries here are runtime-prepared (`sqlx::query`), not compile-time
//! checked macros: the snapshot catalogue composes its SQL from
//! constants, so an offline statement cache has nothing to hold. If a
//! compile-time-checked query (`sqlx::query!`) ever lands in this
//! workspace, wire `cargo sqlx prepare` and the committed `.sqlx`
//! cache into CI in the same change.

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

/// The composition-root open outcome over an application database (see
/// [`Db::open_adopted`]): a pre-existing database that cannot be
/// adopted quarantines (native arm stands down, file untouched), while
/// a failure with no prior file is an ordinary environment error.
#[derive(Debug)]
pub enum AdoptError {
    Quarantined {
        path: std::path::PathBuf,
        source: DbError,
    },
    Fresh(DbError),
}

impl std::fmt::Display for AdoptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdoptError::Quarantined { path, source } => write!(
                f,
                "existing database at {} cannot be adopted ({source}); native services stand \
                 down and the file is left untouched for diagnosis",
                path.display()
            ),
            AdoptError::Fresh(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for AdoptError {}

impl AdoptError {
    /// True when the decline is "the existing database predates the
    /// adoptable baseline" ([`DbError::UnsupportedSchemaVersion`]): the
    /// transient state on a first launch after an upgrade, while the
    /// co-bundled sidecar migrates the database forward to the baseline this
    /// process adopts at. The composition root retries on this; every other
    /// decline (a corrupt file, a driver fault) is final and the substrate
    /// stays proxy-only for the session.
    pub fn is_below_baseline(&self) -> bool {
        matches!(
            self,
            AdoptError::Quarantined {
                source: DbError::UnsupportedSchemaVersion { .. },
                ..
            }
        )
    }
}

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

/// Decode a numeric aggregate that SQLite may hand back as INTEGER
/// (a `SUM`/`COALESCE` expression result keeps the integer type even
/// over REAL-affinity columns). Only a value that decodes as no
/// number at all (NULL, text) falls back to zero; a structural
/// failure (a missing column) is a programming error and panics
/// rather than silently zeroing an analytic.
pub(crate) fn decoded_f64(row: &sqlx::sqlite::SqliteRow, index: usize) -> f64 {
    use sqlx::Row as _;
    row.try_get::<f64, _>(index)
        .or_else(|_| row.try_get::<i64, _>(index).map(|value| value as f64))
        .unwrap_or_else(|error| match error {
            sqlx::Error::ColumnDecode { .. } => 0.0,
            other => panic!("decoded_f64 column {index}: {other}"),
        })
}

/// The application database handle. Cloning shares the one underlying
/// pool (the composition root still opens the database exactly once);
/// a clone is a handle, never a second owner.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// The underlying pool, for harnesses that drive raw statements
    /// (the catalogue snapshot and the replay spike).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Rebind a handle over an already-opened pool. The composition
    /// root still opens the application database exactly once via
    /// [`Db::open`]; this exists for harnesses attaching to a database
    /// another process created and migrated.
    pub fn from_pool(pool: SqlitePool) -> Db {
        Db { pool }
    }

    /// Open (creating if missing), adopt or refuse an existing schema,
    /// and bring the migration chain up to date.
    pub async fn open(path: &Path) -> Result<Db, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            // The backend never enables foreign-key enforcement (the
            // sqlite3 default), so the schema's REFERENCES clauses are
            // declarative there; the driver here enables it by default,
            // which would refuse writes the backend accepts (an overlay
            // event for a session id with no surviving session row).
            // Match the backend's effective pragma surface.
            .foreign_keys(false)
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

    /// Open the application's own database at the composition root.
    ///
    /// Distinguishes failure on a PRE-EXISTING database from failure on
    /// a fresh path: an existing file that cannot be adopted or
    /// migrated is a quarantine signal, not a bare error. The file is
    /// left exactly as found (it is the user's data, and the sidecar
    /// may still serve it); the caller stands the native arm down and
    /// surfaces the condition loudly.
    pub async fn open_adopted(path: &Path) -> Result<Db, AdoptError> {
        let pre_existing = path.exists();
        match Db::open(path).await {
            Ok(db) => Ok(db),
            Err(source) if pre_existing => Err(AdoptError::Quarantined {
                path: path.to_path_buf(),
                source,
            }),
            Err(source) => Err(AdoptError::Fresh(source)),
        }
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

    /// One equipment-library row by id alone: `(name, item_type, properties
    /// JSON)`, or None when absent. The hotbar resolver reads it to branch on
    /// the item type the slot's bound id resolves to (mirroring the backend's
    /// `SELECT id, name, item_type FROM equipment_library WHERE id = ?`, with
    /// the properties carried so the healing branch reads them without a
    /// second query).
    pub async fn hotbar_equipment_row(
        &self,
        id: i64,
    ) -> Result<Option<(String, String, String)>, DbError> {
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT name, item_type, properties_json FROM equipment_library \
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The first weapon-row `properties_json` whose name contains the
    /// supplied fragment, ported from the backend's
    /// `_equipment_profile_lookup`: a `LIKE '%fragment%'` over weapon
    /// rows, with the fragment's own `%` / `_` / `\` escaped (so an
    /// embedded wildcard cannot widen the match) under an explicit
    /// `ESCAPE '\'`. The fragment is trimmed exactly as the backend
    /// trims it before the query.
    pub async fn weapon_properties_by_name_fragment(
        &self,
        fragment: &str,
    ) -> Result<Option<String>, DbError> {
        let safe = fragment
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT properties_json FROM equipment_library \
             WHERE item_type = 'weapon' AND name LIKE ? ESCAPE '\\'",
        )
        .bind(format!("%{safe}%"))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(properties_json,)| properties_json))
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
    async fn hotbar_equipment_row_reads_name_type_and_properties_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        db.insert_equipment_for_tests(7, "Healer", "healing", r#"{"tool_entity":{"x":1}}"#)
            .await
            .unwrap();
        db.insert_equipment_for_tests(8, "Opalo", "weapon", r#"{"weapon_entity":{}}"#)
            .await
            .unwrap();

        assert_eq!(
            db.hotbar_equipment_row(7).await.unwrap(),
            Some((
                "Healer".to_string(),
                "healing".to_string(),
                r#"{"tool_entity":{"x":1}}"#.to_string(),
            )),
        );
        assert_eq!(
            db.hotbar_equipment_row(8).await.unwrap(),
            Some((
                "Opalo".to_string(),
                "weapon".to_string(),
                r#"{"weapon_entity":{}}"#.to_string(),
            )),
        );
        // An absent id yields None.
        assert_eq!(db.hotbar_equipment_row(999).await.unwrap(), None);
    }

    #[tokio::test]
    async fn weapon_profile_lookup_matches_on_a_fragment_and_escapes_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        db.insert_equipment_for_tests(1, "ArMatrix LR-35", "weapon", r#"{"weapon_entity":{}}"#)
            .await
            .unwrap();
        db.insert_equipment_for_tests(2, "Healer", "healing", r#"{"tool_entity":{}}"#)
            .await
            .unwrap();
        db.insert_equipment_for_tests(
            3,
            "100% Plain Name",
            "weapon",
            r#"{"weapon_entity":{"id":3}}"#,
        )
        .await
        .unwrap();

        // A fragment matches the weapon row.
        let found = db
            .weapon_properties_by_name_fragment("LR-35")
            .await
            .unwrap();
        assert_eq!(found.as_deref(), Some(r#"{"weapon_entity":{}}"#));

        // Healing rows are never returned (weapon-only filter).
        let absent = db
            .weapon_properties_by_name_fragment("Healer")
            .await
            .unwrap();
        assert_eq!(absent, None);

        // A literal `%` in the fragment is escaped: it matches the row
        // whose name actually contains `%`, not every row.
        let percent = db.weapon_properties_by_name_fragment("100%").await.unwrap();
        assert_eq!(percent.as_deref(), Some(r#"{"weapon_entity":{"id":3}}"#));
        // A bare wildcard, were it unescaped, would match everything;
        // escaped, it matches nothing because no name contains a literal
        // percent-followed-by-space-P beyond row 3, and the leading `%`
        // here is a literal.
        let only_literal = db
            .weapon_properties_by_name_fragment("%Plain")
            .await
            .unwrap();
        assert_eq!(
            only_literal, None,
            "the leading % is a literal, not a wildcard"
        );
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
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            foreign_keys, 0,
            "referential enforcement stays off, matching the backend's pragma surface"
        );
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
    async fn empty_database_snapshot_yields_every_catalogue_table_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let rows = db.snapshot_rows().await.unwrap();
        assert_eq!(rows.len(), eo_wire::db_snapshot::CATALOGUE.len());
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
            sqlx::query(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                 VALUES ('keep-me', '2026-01-01', 'markup', 'survives refusal', 1.25, 'manual')",
            )
            .execute(&db.pool)
            .await
            .unwrap();
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

        // The refusal is lossless: the user's rows are untouched (the
        // connect-time pragmas may legitimately convert the journal
        // mode, so the assertion is content-level, not byte-level).
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let (description, amount): (String, f64) =
            sqlx::query_as("SELECT description, amount FROM ledger_entries WHERE id = 'keep-me'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(description, "survives refusal");
        assert_eq!(amount, 1.25);
        let version: String =
            sqlx::query_scalar("SELECT value FROM db_metadata WHERE key = 'version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, "28", "the stamp is left for the upgrade owner");
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

    #[tokio::test]
    async fn open_adopted_succeeds_on_fresh_and_healthy_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        // Fresh path: created and migrated.
        let db = Db::open_adopted(&path).await.unwrap();
        drop(db);
        // Healthy pre-existing database: adopted.
        Db::open_adopted(&path).await.unwrap();
    }

    #[tokio::test]
    async fn open_adopted_quarantines_an_unadoptable_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        std::fs::write(&path, b"this is not a sqlite database").unwrap();
        let before = std::fs::read(&path).unwrap();
        match Db::open_adopted(&path).await {
            Err(AdoptError::Quarantined { path: reported, .. }) => {
                assert_eq!(reported, path);
            }
            other => panic!("expected quarantine, got {other:?}"),
        }
        // The quarantine left the user's file byte-identical.
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn is_below_baseline_distinguishes_the_pre_upgrade_race_from_a_real_fault() {
        let dir = tempfile::tempdir().unwrap();

        // A database the backend created but has not yet migrated up to the
        // baseline (version below it, no sqlx ledger): the first-launch-
        // after-upgrade race. open_adopted quarantines it, and
        // is_below_baseline() flags it as the retry-worthy case.
        let below = dir.path().join("below.db");
        {
            let db = Db::open(&below).await.unwrap();
            sqlx::query("UPDATE db_metadata SET value = '28' WHERE key = 'version'")
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("DROP TABLE _sqlx_migrations")
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let err = Db::open_adopted(&below).await.unwrap_err();
        assert!(
            err.is_below_baseline(),
            "a pre-baseline database is the retry-worthy race, got {err:?}"
        );

        // A genuinely unadoptable file also quarantines, but is NOT the
        // race: retrying would never help, so it must not be flagged.
        let corrupt = dir.path().join("corrupt.db");
        std::fs::write(&corrupt, b"this is not a sqlite database").unwrap();
        let err = Db::open_adopted(&corrupt).await.unwrap_err();
        assert!(
            !err.is_below_baseline(),
            "a corrupt file is a permanent fault, not the race, got {err:?}"
        );

        // A fresh-path failure is likewise never the race.
        assert!(!AdoptError::Fresh(DbError::Driver("boom".into())).is_below_baseline());
    }

    #[test]
    fn adopt_error_display_carries_the_path_and_the_stand_down() {
        let quarantined = AdoptError::Quarantined {
            path: std::path::PathBuf::from("somewhere/entropia_orme.db"),
            source: DbError::Driver("file is not a database".into()),
        };
        let rendered = quarantined.to_string();
        assert!(rendered.contains("somewhere"), "{rendered}");
        assert!(rendered.contains("cannot be adopted"), "{rendered}");
        assert!(rendered.contains("file is not a database"), "{rendered}");
        assert!(rendered.contains("left untouched"), "{rendered}");
        assert_eq!(
            AdoptError::Fresh(DbError::Driver("boom".into())).to_string(),
            "boom"
        );
    }

    #[tokio::test]
    async fn open_adopted_reports_fresh_path_failures_plainly() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in the file's place defeats creation without any
        // pre-existing database file at the path... but exists() is
        // true for directories, so use a missing parent instead.
        let path = dir.path().join("missing-parent").join("entropia_orme.db");
        match Db::open_adopted(&path).await {
            Err(AdoptError::Fresh(_)) => {}
            other => panic!("expected a fresh-path error, got {other:?}"),
        }
    }
}
