//! The persistence base: one SQLite database behind a narrow handle.
//!
//! The database runs behind one synchronous core (see [`pool`]): a single
//! dedicated writer thread owning the write connection, and a small pool of
//! reader threads each owning a read connection against the WAL. Callers
//! submit closures ([`Db::with_reader`] / [`Db::with_writer`], with blocking
//! counterparts for plain producer threads); exclusive access to a connection
//! is the thread that owns it, so there is no pool checkout, no lock order, and
//! no async executor between a caller and SQLite. No driver type escapes this
//! module's API: callers see [`Db`], [`DbError`], and plain data.
//!
//! Design decisions:
//!
//! - **Writer/reader split**: one dedicated writer thread serialises every
//!   write in-process (so two writers queue at the writer thread rather than
//!   colliding on SQLite's single-writer lock), and a small pool of reader
//!   threads serves reads concurrently against the WAL, so a live write stream
//!   no longer stalls dashboard reads. Callers pick the role by intent
//!   ([`Db::with_reader`] for `SELECT`, [`Db::with_writer`] for mutations); no
//!   other module reaches a raw connection. (The original single-owner
//!   connection was the benchmark-justified renovation point once real databases outgrew
//!   it; the split is response-invariant, re-validated against the DB-state
//!   goldens.)
//! - **Session configuration**: WAL journal, NORMAL synchronous, a
//!   five-second busy timeout, foreign keys off (the pragma surface the
//!   schema has always run under: `REFERENCES` clauses are declarative),
//!   and a 64 MB page cache per connection.
//! - **Schema baseline**: the migration chain (an embedded runner with the
//!   inherited ledger accounting, see [`migrate`]) starts at the version-33
//!   baseline, statement text carried verbatim, so a freshly-migrated
//!   database is `sqlite_master`-identical to one created before the
//!   runner existed.
//! - **Adoption over re-creation**: opening an existing database already at
//!   version 33 marks the baseline as applied without running any DDL.
//!   Databases on older schema versions are refused: the pre-baseline
//!   upgrade chain was never carried across (the retired implementation
//!   owned those upgrades), so composition declines such a database loudly.
//! - **Column reconciliation**: because adoption trusts the existing schema
//!   rather than running the baseline DDL, a Python-lineage database can be
//!   missing a column the Rust baseline declares (the retired ladder never
//!   added it). [`reconcile_baseline_columns`] heals that drift in place on
//!   every open, adding any baseline column an adopted table lacks, so the gap
//!   cannot outlive the first query that references the column.

mod migrate;
mod pool;

use std::path::Path;
use std::time::Duration;

use pool::SyncCore;
use rusqlite::OptionalExtension;
use serde_json::{Map, Value};

/// The schema version the baseline migration reproduces.
const BASELINE_SCHEMA_VERSION: i64 = 33;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The on-disk schema predates the supported baseline (the pre-baseline
    /// upgrade chain was never carried across); composition refuses it.
    #[error("database schema version {found} predates the supported baseline {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },
    /// Any driver failure from the synchronous core.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The synchronous core's worker threads have exited; no further
    /// statements can run (the handle outlived the database's lifetime).
    #[error("the database core is closed")]
    CoreClosed,
    /// An applied migration-ledger row does not reconcile with the
    /// embedded migration chain.
    #[error("migration {version} failed validation: {problem}")]
    MigrationValidation { version: i64, problem: &'static str },
    /// A stored value that does not decode into its domain shape.
    #[error("{context}: {source}")]
    Decode {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// A snapshot read met a column type outside the emitter's catalogue.
    #[error("unsupported value type {type_name} in column {column}")]
    UnsupportedValueType { type_name: String, column: String },
}

/// The composition-root open outcome over an application database (see
/// [`Db::open_adopted`]): a pre-existing database that cannot be
/// adopted quarantines (native arm stands down, file untouched), while
/// a failure with no prior file is an ordinary environment error.
#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error(
        "existing database at {} cannot be adopted ({source}); native services stand \
         down and the file is left untouched for diagnosis",
        path.display()
    )]
    Quarantined {
        path: std::path::PathBuf,
        source: DbError,
    },
    #[error(transparent)]
    Fresh(DbError),
}

impl AdoptError {
    /// True when the decline is "the existing database is below the adoptable
    /// baseline and below the rung the native upgrade bridges"
    /// ([`DbError::UnsupportedSchemaVersion`]). The native first-launch upgrade
    /// ([`upgrade_to_baseline`]) carries the one in-the-wild rung (v32 -> v33),
    /// so a v32 database adopts cleanly and never surfaces here; only older
    /// schemas, which no database in the wild occupies, reach this. The
    /// composition root treats it as a terminal decline, exactly as it treats
    /// every other decline (a corrupt file, a driver fault).
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

/// The startup corruption probe's time budget: `PRAGMA quick_check` is far
/// cheaper than a full `integrity_check`, but still O(database), so it is
/// bounded and run off the launch path. A database large enough to exceed
/// this is left unprobed (and logged), never blocking startup.
pub const STARTUP_QUICK_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// The outcome of the budgeted startup corruption probe
/// ([`Db::quick_check_budgeted`]).
#[derive(Debug)]
pub enum QuickCheckOutcome {
    /// `PRAGMA quick_check` returned the single `ok` row: no problems found.
    Ok,
    /// The probe found problems; the payload is SQLite's own report, one
    /// finding per `; `-joined segment.
    Corrupt(String),
    /// The probe did not finish within its budget and was abandoned (the
    /// running statement is interrupted), so startup was not blocked. The
    /// database is left unprobed this launch.
    OverBudget,
    /// The probe could not run (a driver error, not a corruption verdict).
    Error(DbError),
}

/// The application database handle. Cloning shares the underlying
/// synchronous core (the composition root still opens the database exactly
/// once); a clone is a handle, never a second owner.
///
/// Reads and writes travel separate roles of the core: one dedicated writer
/// thread serialises every write in-process (so two writers queue at the
/// writer thread rather than colliding on SQLite's single-writer lock),
/// while a small pool of reader threads serves dashboard reads concurrently
/// against the WAL. This is what stops a live write stream from stalling
/// reads. See [`Db::with_reader`] and [`Db::with_writer`].
#[derive(Debug, Clone)]
pub struct Db {
    /// The synchronous core: a writer thread and a reader-thread pool over
    /// their own connections, serving the closure API.
    core: SyncCore,
    /// The database file path, retained so the budgeted quick-check can open
    /// its own throwaway read-only connection (an interruptible probe that
    /// never disturbs the core's connections).
    path: std::path::PathBuf,
}

/// The latest calibrated level per skill: the row with the greatest
/// `scanned_at`, and among rows sharing that instant the greatest id.
///
/// This is the single definition of "the current level" over
/// `skill_calibrations`. Rows are not guaranteed to arrive in time
/// order (a scan recorded from an older screenshot after a newer one, a
/// restored backup beside newer rows, a replayed chat log, a backfill),
/// so the id is never a time order here: it only breaks a tie between
/// rows written at the same instant. `source` narrows to one source's
/// anchor (`Some("scan")` for the scan anchor); `skill_names` narrows to
/// the named skills. Every consumer goes through this function so the
/// reads cannot drift apart.
pub fn latest_skill_levels(
    conn: &rusqlite::Connection,
    source: Option<&str>,
    skill_names: Option<&[String]>,
) -> rusqlite::Result<Vec<(String, f64)>> {
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();
    let mut latest_filters: Vec<String> = Vec::new();
    if let Some(source) = &source {
        params.push(source);
        latest_filters.push("source = ?".to_string());
    }
    if let Some(names) = skill_names {
        let placeholders = vec!["?"; names.len()].join(",");
        latest_filters.push(format!("skill_name IN ({placeholders})"));
        for name in names {
            params.push(name);
        }
    }
    let latest_where = if latest_filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", latest_filters.join(" AND "))
    };
    // The picking subquery joins on skill name, so only the source
    // needs restating there; its parameter binds after the CTE's.
    let pick_where = match &source {
        Some(source) => {
            params.push(source);
            "WHERE s2.source = ?".to_string()
        }
        None => String::new(),
    };
    // id-order: tiebreak (the CTE has already narrowed each skill to its
    // latest scanned_at; MAX(id) only separates rows sharing that instant).
    let sql = format!(
        "WITH latest_ts AS ( \
             SELECT skill_name, MAX(scanned_at) AS ts \
             FROM skill_calibrations {latest_where} \
             GROUP BY skill_name \
         ) \
         SELECT skill_name, level FROM skill_calibrations \
         WHERE id IN ( \
             SELECT MAX(s2.id) FROM skill_calibrations s2 \
             JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts \
             {pick_where} \
             GROUP BY s2.skill_name \
         )"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    mapped.collect()
}

impl Db {
    /// Run a read closure on the synchronous core: whichever reader
    /// thread is free runs it to completion on its own connection,
    /// concurrently with the writer under WAL. The closure sees a bare
    /// connection with the seam's session pragmas applied; multi-step
    /// reads run without an executor between the steps.
    pub async fn with_reader<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, DbError> + Send + 'static,
    {
        self.core.read(job).await
    }

    /// Run a write closure on the synchronous core's writer thread.
    /// Every write submitted anywhere in the process runs serially on
    /// the one write connection, in submission order; a multi-statement
    /// transaction is a single closure (`connection.transaction()?`),
    /// so it can never be interleaved or left half-open across an
    /// executor boundary.
    pub async fn with_writer<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, DbError> + Send + 'static,
    {
        self.core.write(job).await
    }

    /// The blocking counterpart of [`Db::with_reader`], for plain
    /// producer threads that have no async context. Never call it on an
    /// async runtime's worker thread (it parks the thread on the reply).
    pub fn with_reader_blocking<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, DbError> + Send + 'static,
    {
        self.core.read_blocking(job)
    }

    /// The blocking counterpart of [`Db::with_writer`], for plain
    /// producer threads that have no async context. Never call it on an
    /// async runtime's worker thread (it parks the thread on the reply).
    pub fn with_writer_blocking<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, DbError> + Send + 'static,
    {
        self.core.write_blocking(job)
    }

    /// Refresh the query planner's statistics via `PRAGMA optimize`, the
    /// recommended once-per-lifecycle maintenance call. Run at a quiescent
    /// boundary (shutdown, with no writes in flight), never on a hot path.
    /// Returns whether the pragma succeeded (a failure is non-fatal at exit).
    pub async fn optimize_on_shutdown(&self) -> bool {
        self.with_writer(|connection| {
            connection.execute_batch("PRAGMA optimize")?;
            Ok(())
        })
        .await
        .is_ok()
    }

    /// Checkpoint the WAL and truncate it to zero, bounding WAL growth
    /// over a long-running session. Runs on the writer (a checkpoint is a
    /// write operation). `TRUNCATE` blocks until it can reset the log,
    /// which is the intended behaviour at a quiescent boundary (session
    /// end), not on a hot path.
    pub async fn checkpoint_truncate(&self) -> Result<(), DbError> {
        self.with_writer(|connection| {
            connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
            Ok(())
        })
        .await
    }

    /// Write a compacted copy of the database to `dest` via `VACUUM INTO`,
    /// reclaiming the free pages that accumulate as rows churn. Unlike a
    /// plain `VACUUM`, this never rewrites or locks the live file for the
    /// duration: it only reads the live database and writes a fresh,
    /// defragmented copy at `dest`, so a user can trigger it off the hot
    /// path without stalling live tracking. Runs on the writer (`VACUUM`
    /// cannot execute inside a transaction and serialises with writes).
    ///
    /// `dest` must not already exist: SQLite refuses to overwrite. The
    /// caller clears any prior copy first. The path is bound as a
    /// parameter, so no path text is composed into the statement.
    pub async fn vacuum_into(&self, dest: &Path) -> Result<(), DbError> {
        let dest = dest.to_string_lossy().into_owned();
        self.with_writer(move |connection| {
            connection.execute("VACUUM INTO ?1", rusqlite::params![dest])?;
            Ok(())
        })
        .await
    }

    /// Run `PRAGMA quick_check`, abandoning it if it exceeds `budget`.
    /// `quick_check` is far cheaper than a full `integrity_check` (it skips
    /// the costly index-vs-table cross checks) but is still O(database), so
    /// it is bounded and meant to run off the launch path: a corruption
    /// signal surfaces (through the caller's logging) without ever stalling
    /// startup or crashing on a problem.
    ///
    /// The probe runs on a throwaway, read-only connection (opened
    /// `SQLITE_OPEN_READ_ONLY`, so it can never mutate) on a spawned thread.
    /// The budget genuinely cancels the probe: on timeout the connection's
    /// interrupt handle aborts the in-flight statement, so an over-budget
    /// `quick_check` on a large database stops scanning rather than merely
    /// having its result ignored while it runs on. Read-only; never mutates.
    pub async fn quick_check_budgeted(&self, budget: Duration) -> QuickCheckOutcome {
        use rusqlite::OpenFlags;

        let connection = match rusqlite::Connection::open_with_flags(
            &self.path,
            // Read-only, so the probe can never mutate. Serialized (full-mutex)
            // rather than the rusqlite default no-mutex, so interrupting from
            // this thread while the statement scans on the spawned one is safe.
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        ) {
            Ok(connection) => connection,
            Err(error) => return QuickCheckOutcome::Error(error.into()),
        };
        // Take the interrupt handle before the query starts so a timeout can
        // abort the running statement from this thread while it scans on the
        // spawned one.
        let interrupt = connection.get_interrupt_handle();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("db-quick-check".into())
            .spawn(move || {
                let _ = reply_tx.send(run_quick_check(&connection));
            })
            .expect("spawn the quick-check probe thread");

        match tokio::time::timeout(budget, reply_rx).await {
            // Over budget: interrupt the still-running statement so the probe
            // genuinely cancels rather than scanning on unobserved.
            Err(_elapsed) => {
                interrupt.interrupt();
                QuickCheckOutcome::OverBudget
            }
            // The probe thread dropped its sender without a value: it was
            // interrupted mid-flight (or the thread failed). Treat as
            // over-budget; startup is never blocked either way.
            Ok(Err(_recv)) => QuickCheckOutcome::OverBudget,
            Ok(Ok(outcome)) => outcome,
        }
    }

    /// Open (creating if missing), adopt or refuse an existing schema,
    /// and bring the migration chain up to date.
    ///
    /// The synchronous core's write connection is opened and migrated
    /// first; the core's reader connections open only after the schema is
    /// current, so no connection ever observes a pre-migration database.
    pub async fn open(path: &Path) -> Result<Db, DbError> {
        let mut write_connection = pool::open_configured(path)?;
        adopt_or_refuse(&mut write_connection)?;
        reconcile_baseline_columns(&mut write_connection)?;
        migrate::run(&mut write_connection)?;
        let core = SyncCore::start(path, write_connection, None)?;
        Ok(Db {
            core,
            path: path.to_path_buf(),
        })
    }

    /// Open a database whose reader connections reject reads from the named
    /// tables. This is an integration-test seam for structural access-path
    /// proofs; projection builders still run on the ordinary writer.
    #[doc(hidden)]
    pub async fn open_with_denied_reader_tables(
        path: &Path,
        tables: &[&str],
    ) -> Result<Db, DbError> {
        let mut write_connection = pool::open_configured(path)?;
        adopt_or_refuse(&mut write_connection)?;
        reconcile_baseline_columns(&mut write_connection)?;
        migrate::run(&mut write_connection)?;
        let denied = std::sync::Arc::new(
            tables
                .iter()
                .map(|table| (*table).to_string())
                .collect::<std::collections::HashSet<_>>(),
        );
        let core = SyncCore::start(path, write_connection, Some(denied))?;
        Ok(Db {
            core,
            path: path.to_path_buf(),
        })
    }

    /// Open the application's own database at the composition root.
    ///
    /// Distinguishes failure on a PRE-EXISTING database from failure on
    /// a fresh path: an existing file that cannot be adopted or
    /// migrated is a quarantine signal, not a bare error. The file is
    /// left exactly as found (it is the user's data); the caller declines
    /// composition and surfaces the condition loudly.
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
        self.with_reader(snapshot_rows_sync).await
    }

    /// One equipment-library row by id and item type: (id, name,
    /// properties JSON), or None when absent. The typed accessor the
    /// trifecta resolution reads through.
    pub async fn equipment_item(
        &self,
        id: i64,
        item_type: &str,
    ) -> Result<Option<(i64, String, String)>, DbError> {
        let item_type = item_type.to_string();
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT id, name, properties_json FROM equipment_library \
                     WHERE id = ?1 AND item_type = ?2",
                    rusqlite::params![id, item_type],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(DbError::from)
        })
        .await
    }

    /// One equipment-library row by id alone: `(name, item_type, properties
    /// JSON)`, or None when absent. The hotbar resolver reads it to branch on
    /// the item type the slot's bound id resolves to (a
    /// `SELECT id, name, item_type FROM equipment_library WHERE id = ?`, with
    /// the properties carried so the healing branch reads them without a
    /// second query).
    pub async fn hotbar_equipment_row(
        &self,
        id: i64,
    ) -> Result<Option<(String, String, String)>, DbError> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT name, item_type, properties_json FROM equipment_library \
                     WHERE id = ?1",
                    rusqlite::params![id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(DbError::from)
        })
        .await
    }

    /// The first weapon-row `properties_json` whose name contains the
    /// supplied fragment: a `LIKE '%fragment%'` over weapon
    /// rows, with the fragment's own `%` / `_` / `\` escaped (so an
    /// embedded wildcard cannot widen the match) under an explicit
    /// `ESCAPE '\'`. The fragment is trimmed before the query.
    pub async fn weapon_properties_by_name_fragment(
        &self,
        fragment: &str,
    ) -> Result<Option<String>, DbError> {
        let safe = fragment
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{safe}%");
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT properties_json FROM equipment_library \
                     WHERE item_type = 'weapon' AND name LIKE ?1 ESCAPE '\\'",
                    rusqlite::params![pattern],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(DbError::from)
        })
        .await
    }

    /// The stored equipment library, oldest first:
    /// `(id, name, item_type, properties_json)` per row.
    pub async fn equipment_library_rows(
        &self,
    ) -> Result<Vec<(i64, String, String, String)>, DbError> {
        self.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, item_type, properties_json FROM equipment_library \
                 ORDER BY created_at",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// The latest calibrated level per skill, by scan instant with the
    /// row id as the tiebreaker: believed-current when `source` is None,
    /// a single source's anchor otherwise (`source='scan'` for the scan
    /// anchor). The one read every consumer of "the current level" goes
    /// through; see [`latest_skill_levels`].
    pub async fn latest_skill_calibrations(
        &self,
        source: Option<String>,
    ) -> Result<Vec<(String, f64)>, DbError> {
        self.with_reader(move |conn| Ok(latest_skill_levels(conn, source.as_deref(), None)?))
            .await
    }

    /// Epoch timestamp of the most recent skill calibration, or None.
    pub async fn last_calibration_epoch(&self) -> Result<Option<f64>, DbError> {
        self.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT MAX(scanned_at) as ts FROM skill_calibrations",
                [],
                |row| row.get::<_, Option<f64>>(0),
            )?)
        })
        .await
    }

    /// Insert an equipment-library row, returning its generated id.
    pub async fn insert_equipment(
        &self,
        name: String,
        item_type: String,
        catalog_id: Option<String>,
        properties_json: String,
    ) -> Result<i64, DbError> {
        self.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
                 VALUES (?, ?, ?, ?)",
                rusqlite::params![name, item_type, catalog_id, properties_json],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// One equipment-library row by id:
    /// `(id, name, item_type, properties_json)`, or None when absent.
    pub async fn equipment_row(
        &self,
        item_id: i64,
    ) -> Result<Option<(i64, String, String, String)>, DbError> {
        self.with_reader(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id, name, item_type, properties_json FROM equipment_library \
                     WHERE id = ?",
                    rusqlite::params![item_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?)
        })
        .await
    }

    /// One equipment-library row's expanded detail by id:
    /// `(id, name, item_type, catalog_id, properties_json)`, or None.
    pub async fn equipment_detail_row(
        &self,
        item_id: i64,
    ) -> Result<Option<(i64, String, String, Option<String>, String)>, DbError> {
        self.with_reader(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id, name, item_type, catalog_id, properties_json \
                     FROM equipment_library WHERE id = ?",
                    rusqlite::params![item_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?)
        })
        .await
    }

    /// A stored equipment row's item type, or None when absent.
    pub async fn equipment_item_type(&self, item_id: i64) -> Result<Option<String>, DbError> {
        self.with_reader(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id, item_type FROM equipment_library WHERE id = ?",
                    rusqlite::params![item_id],
                    |row| row.get::<_, String>(1),
                )
                .optional()?)
        })
        .await
    }

    /// Replace a stored equipment row's configuration (name, catalogue
    /// binding, and properties; the item type is fixed).
    pub async fn update_equipment(
        &self,
        item_id: i64,
        name: String,
        catalog_id: Option<String>,
        properties_json: String,
    ) -> Result<(), DbError> {
        self.with_writer(move |conn| {
            conn.execute(
                "UPDATE equipment_library SET name = ?, catalog_id = ?, properties_json = ? \
                 WHERE id = ?",
                rusqlite::params![name, catalog_id, properties_json, item_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Delete a stored equipment row (idempotent over a missing row).
    pub async fn delete_equipment(&self, item_id: i64) -> Result<(), DbError> {
        self.with_writer(move |conn| {
            conn.execute(
                "DELETE FROM equipment_library WHERE id = ?",
                rusqlite::params![item_id],
            )?;
            Ok(())
        })
        .await
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
        let name = name.to_string();
        let item_type = item_type.to_string();
        let properties_json = properties_json.to_string();
        self.with_writer(move |connection| {
            connection.execute(
                "INSERT INTO equipment_library (id, name, item_type, properties_json) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, name, item_type, properties_json],
            )?;
            Ok(())
        })
        .await
    }

    /// The schema objects as (type, name, statement) in (type, name)
    /// order, excluding SQLite's own bookkeeping tables: the surface the
    /// schema-conformance acceptance compares across implementations.
    pub async fn schema_master(&self) -> Result<Vec<(String, String, String)>, DbError> {
        self.with_reader(|connection| {
            let mut statement = connection.prepare(
                "SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL \
                 AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?;
            Ok(rows)
        })
        .await
    }
}

/// Run `PRAGMA quick_check` to completion on the probe connection and map its
/// rows to a [`QuickCheckOutcome`]. An interrupt (the budget firing) surfaces
/// as [`QuickCheckOutcome::OverBudget`], matching the caller's own verdict.
fn run_quick_check(connection: &rusqlite::Connection) -> QuickCheckOutcome {
    let mut statement = match connection.prepare("PRAGMA quick_check") {
        Ok(statement) => statement,
        Err(error) => return QuickCheckOutcome::Error(error.into()),
    };
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .and_then(|mapped| mapped.collect::<rusqlite::Result<Vec<String>>>());
    let lines = match rows {
        Ok(lines) => lines,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            return QuickCheckOutcome::OverBudget
        }
        Err(error) => return QuickCheckOutcome::Error(error.into()),
    };
    // A healthy database answers with the single row `ok`.
    if lines.len() == 1 && lines[0] == "ok" {
        QuickCheckOutcome::Ok
    } else {
        QuickCheckOutcome::Corrupt(lines.join("; "))
    }
}

/// Mark the baseline as applied on a pre-existing database already at
/// the baseline version; refuse older schemas.
fn adopt_or_refuse(connection: &mut rusqlite::Connection) -> Result<(), DbError> {
    if !table_exists_sync(connection, "db_metadata")? {
        // A fresh (or empty) database: the migration chain owns it.
        return Ok(());
    }
    if table_exists_sync(connection, "_sqlx_migrations")? {
        // Already adopted (or natively created); the chain validates.
        return Ok(());
    }
    let version: Option<String> = connection
        .query_row(
            "SELECT value FROM db_metadata WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let version: i64 = version.and_then(|raw| raw.parse().ok()).unwrap_or_default();

    // Upgrade-and-adopt as one transaction: the in-place bridge (below) and the
    // baseline stamp commit together or not at all. A failure in either rolls
    // back the file to exactly as it was found, honouring the `open_adopted`
    // "left untouched on a decline" contract; without this, a stamp failure
    // after the bridge mutated the file would leave a half-upgraded database.
    let tx = connection.transaction()?;
    if version < BASELINE_SCHEMA_VERSION {
        // A below-baseline database: the retired sidecar that used to
        // migrate it forward to the baseline on the first launch after
        // an upgrade is gone, so the
        // upgrade runs natively here, in place, before the baseline is
        // stamped. Only the single rung an in-the-wild v0.1.0-lineage
        // database occupies is bridged; older schemas stay a refusal.
        upgrade_to_baseline(&tx, version)?;
    }

    // The ledger row the runner would have written had it created the
    // schema; the post-adoption `migrate::run` validates it (version and
    // checksum), so drift in this DDL or the row fails loudly.
    let baseline = migrate::MIGRATIONS
        .first()
        .expect("the migration chain carries the baseline");
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (\
         version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
         installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, \
         execution_time BIGINT NOT NULL)",
    )?;
    tx.execute(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (?1, ?2, TRUE, ?3, 0)",
        rusqlite::params![baseline.version, baseline.description, baseline.checksum()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Upgrade a below-baseline database up to the adoptable baseline in place,
/// then return so [`adopt_or_refuse`] stamps the baseline ledger row over the
/// now-current schema.
///
/// Only the rung the real v0.1.0-lineage database occupies is implemented:
///
/// - **v32 -> v33**: drop the unused `tt_curve_observations` table. It was
///   write-only in the v32 surface (the skill tracker wrote a row on every
///   suppressed codex skill gain, but no read path ever consumed it), so the
///   drop is lossless. This mirrors the Python ladder's v33 step, and leaves
///   the database where a freshly-created v33 one sits: no
///   `tt_curve_observations`, `db_metadata.version = 33`.
///
/// Anything below v32 is refused exactly as before. No database in the wild
/// occupies those versions, so the earlier rungs are deliberately not ported;
/// pinning the bridge to v32 (rather than `BASELINE_SCHEMA_VERSION - 1`) keeps
/// the rung's DDL coupled to the concrete v32 -> v33 delta it actually applies.
/// Runs inside [`adopt_or_refuse`]'s adopt transaction, so the bridge and the
/// subsequent baseline stamp share one commit boundary (a failure leaves the
/// file untouched).
fn upgrade_to_baseline(tx: &rusqlite::Transaction<'_>, version: i64) -> Result<(), DbError> {
    /// The one below-baseline schema version with a native upgrade path.
    const BRIDGEABLE_VERSION: i64 = 32;
    if version != BRIDGEABLE_VERSION {
        return Err(DbError::UnsupportedSchemaVersion {
            found: version,
            supported: BASELINE_SCHEMA_VERSION,
        });
    }
    // v33 rung: drop the retired write-only observations table.
    tx.execute_batch("DROP TABLE IF EXISTS tt_curve_observations")?;
    tx.execute(
        "UPDATE db_metadata SET value = ?1 WHERE key = 'version'",
        rusqlite::params![BASELINE_SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// The pre-mastery `codex_claims` shape (no `kind` / `attribute_name`), as an
/// earlier schema lineage left it. The single canonical fixture the db-layer
/// and codex-service tests both reproduce the adopted-legacy drift from, so the
/// two cannot silently diverge and mask a healing regression.
#[cfg(test)]
pub(crate) const LEGACY_CODEX_CLAIMS_DDL: &str = "\
    DROP TABLE codex_claims; \
    CREATE TABLE codex_claims ( \
        id             INTEGER PRIMARY KEY AUTOINCREMENT, \
        species_name   TEXT NOT NULL, \
        rank           INTEGER NOT NULL, \
        skill_name     TEXT NOT NULL, \
        ped_value      REAL NOT NULL, \
        claimed_at     REAL NOT NULL DEFAULT (unixepoch('now')) \
    ); \
    CREATE INDEX idx_codex_claims_species ON codex_claims(species_name);";

/// Heal an adopted database that predates a column the baseline declares.
///
/// [`adopt_or_refuse`] stamps a version-33 (or v32-bridged) Python-lineage
/// database as baseline-applied *without ever running the baseline DDL*: it
/// trusts the existing schema to already match. But the Rust baseline carries
/// columns the retired ladder never added (`codex_claims.kind` and
/// `attribute_name`), so on such a database those columns are silently absent.
/// The gap stays latent until a query first references a missing column: no
/// always-run read touched `kind` until the codex mastery reads, whereupon the
/// species list fails to load with `no such column: kind`.
///
/// The fix heals in place, on every open, for *already-adopted* databases too
/// (they carry `_sqlx_migrations` and so never re-enter the adopt path): for
/// each baseline table present in the database, add any column the baseline
/// declares but the table lacks. It is reference-driven rather than a
/// hand-maintained list: the baseline SQL is applied to a throwaway in-memory
/// database whose `PRAGMA table_info` is the authority, so this stays correct
/// as the baseline moves and covers the whole class of column drift, not just
/// the two codex columns that surfaced it.
///
/// A no-op on a healthy database: a freshly-created one has no tables yet at
/// this point in [`Db::open`] (the chain runs next), and a correctly-adopted
/// one already carries every baseline column. The heal is wrapped in one
/// transaction, so a failure rolls the file back to exactly as it was found,
/// honouring the [`Db::open_adopted`] "left untouched on a decline" contract.
fn reconcile_baseline_columns(connection: &mut rusqlite::Connection) -> Result<(), DbError> {
    if !table_exists_sync(connection, "db_metadata")? {
        // A fresh (or empty) database: the migration chain creates the schema
        // whole, so there is nothing to reconcile. Short-circuit before the
        // in-memory reference is built, off the common fresh-install path.
        return Ok(());
    }

    let reference = rusqlite::Connection::open_in_memory()?;
    let baseline = migrate::MIGRATIONS
        .first()
        .expect("the migration chain carries the baseline");
    reference.execute_batch(baseline.sql)?;

    let mut additions: Vec<String> = Vec::new();
    for table in baseline_tables(&reference)? {
        // Only a table that already exists can be missing a column; a fresh
        // database has none of them yet, so it is untouched here and the
        // migration chain creates it whole.
        if !table_exists_sync(connection, &table)? {
            continue;
        }
        let present = column_names_sync(connection, &table)?;
        for column in baseline_columns(&reference, &table)? {
            if !present.contains(&column.name) {
                additions.push(column.add_column_ddl(&table));
            }
        }
    }
    if additions.is_empty() {
        return Ok(());
    }

    let tx = connection.transaction()?;
    for ddl in &additions {
        tx.execute_batch(ddl)?;
    }
    tx.commit()?;
    Ok(())
}

/// One column the baseline declares, read back from the reference database's
/// `PRAGMA table_info` so its type, nullability, and default survive verbatim
/// when the column is re-added to a drifted table.
struct BaselineColumn {
    name: String,
    type_decl: String,
    not_null: bool,
    default: Option<String>,
}

impl BaselineColumn {
    /// The `ALTER TABLE ... ADD COLUMN` that re-creates this column. The
    /// default (`PRAGMA table_info` returns it as a ready-to-reparse literal,
    /// e.g. `'rank'`) precedes the `NOT NULL`; SQLite requires a default to add
    /// a `NOT NULL` column to a populated table, which every drifted baseline
    /// column here carries (`kind DEFAULT 'rank'`; `attribute_name` is
    /// nullable).
    fn add_column_ddl(&self, table: &str) -> String {
        let mut ddl = format!(
            "ALTER TABLE {table} ADD COLUMN {} {}",
            self.name, self.type_decl
        );
        if let Some(default) = &self.default {
            ddl.push_str(&format!(" DEFAULT {default}"));
        }
        if self.not_null {
            ddl.push_str(" NOT NULL");
        }
        ddl
    }
}

/// The baseline's own table names (its declared user tables; the migration
/// ledger and SQLite's internal tables are excluded).
fn baseline_tables(connection: &rusqlite::Connection) -> Result<Vec<String>, DbError> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name != '_sqlx_migrations' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

/// The columns a baseline table declares, in declaration order.
fn baseline_columns(
    connection: &rusqlite::Connection,
    table: &str,
) -> Result<Vec<BaselineColumn>, DbError> {
    let mut columns = Vec::new();
    connection.pragma(None, "table_info", table, |row| {
        columns.push(BaselineColumn {
            name: row.get("name")?,
            type_decl: row.get("type")?,
            not_null: row.get::<_, i64>("notnull")? != 0,
            default: row.get::<_, Option<String>>("dflt_value")?,
        });
        Ok(())
    })?;
    Ok(columns)
}

/// The set of column names a live table currently carries.
fn column_names_sync(
    connection: &rusqlite::Connection,
    table: &str,
) -> Result<std::collections::HashSet<String>, DbError> {
    let mut names = std::collections::HashSet::new();
    connection.pragma(None, "table_info", table, |row| {
        names.insert(row.get::<_, String>("name")?);
        Ok(())
    })?;
    Ok(names)
}

fn table_exists_sync(connection: &rusqlite::Connection, name: &str) -> Result<bool, DbError> {
    let found = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![name],
            |_| Ok(()),
        )
        .map(|()| true)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })?;
    Ok(found)
}

/// One row as the snapshot emitter expects it: column-ordered keys, JSON
/// values typed by the stored value (integer, real, text, null). Shared by
/// the snapshot catalogue and the projection-rebuild verifier, both of
/// which compare rows by their canonical serialisation. A stored value
/// outside the emitter's catalogue (a BLOB) is refused rather than guessed
/// at, mirroring the original driver-typed shaper's `UnsupportedValueType`.
pub(crate) fn row_to_json(row: &rusqlite::Row, columns: &[String]) -> Result<Value, DbError> {
    let mut object = Map::new();
    for (index, name) in columns.iter().enumerate() {
        let value = match row.get_ref(index)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(value) => Value::from(value),
            rusqlite::types::ValueRef::Real(value) => Value::from(value),
            rusqlite::types::ValueRef::Text(text) => {
                Value::from(String::from_utf8(text.to_vec()).expect("snapshot text is UTF-8"))
            }
            rusqlite::types::ValueRef::Blob(_) => {
                return Err(DbError::UnsupportedValueType {
                    type_name: "BLOB".to_string(),
                    column: name.clone(),
                })
            }
        };
        object.insert(name.clone(), value);
    }
    Ok(Value::Object(object))
}

/// Execute the snapshot catalogue on a reader connection: each table's query
/// with its deterministic ORDER BY, exactly as the catalogue documents.
fn snapshot_rows_sync(
    connection: &mut rusqlite::Connection,
) -> Result<Map<String, Value>, DbError> {
    let mut tables = Map::new();
    for spec in eo_wire::db_snapshot::CATALOGUE {
        // Composed exclusively from the catalogue's compile-time constants
        // (no caller input reaches this string).
        let sql = format!("{} ORDER BY {}", spec.query, spec.order_by.join(", "));
        let mut statement = connection.prepare(&sql)?;
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|&name| name.to_string())
            .collect();
        let json_rows = statement
            .query_map([], |row| Ok(row_to_json(row, &columns)))?
            .collect::<rusqlite::Result<Vec<Result<Value, DbError>>>>()?
            .into_iter()
            .collect::<Result<Vec<Value>, DbError>>()?;
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

    /// A database exactly as a version-33 backend leaves it: the baseline
    /// schema and version row, with no migration ledger and none of the
    /// post-baseline migration objects. Built by running the baseline
    /// migration SQL directly rather than the full native chain, so an
    /// adoption test then exercises the post-baseline chain against a genuine
    /// baseline, and the helper stays correct as later migrations are added.
    /// Returns a bare connection the caller seeds and then drops, so the file
    /// is closed before [`Db::open`] adopts it.
    fn backend_baseline_db(path: &std::path::Path) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(include_str!("../../migrations/0001_schema_baseline.sql"))
            .unwrap();
        connection
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
    async fn checkpoint_truncate_resets_the_wal_and_keeps_data_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        let db = Db::open(&path).await.unwrap();

        // Grow the WAL with a batch of committed writes.
        db.with_writer(|connection| {
            for i in 0..200 {
                connection.execute(
                    "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?1, ?2, 0)",
                    rusqlite::params![format!("s-{i}"), i as f64],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let wal = path.with_extension("db-wal");
        let grown = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert!(
            grown > 0,
            "the WAL should carry frames before the checkpoint"
        );

        db.checkpoint_truncate().await.unwrap();

        // TRUNCATE resets the log to zero bytes (no reader held a snapshot).
        let after = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(after, 0, "wal_checkpoint(TRUNCATE) empties the WAL");

        // The committed data is intact and readable through a reader.
        let count = db
            .with_reader(|connection| {
                Ok(
                    connection.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 200);
    }

    #[tokio::test]
    async fn vacuum_into_writes_a_valid_compacted_copy_and_leaves_the_live_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        let db = Db::open(&path).await.unwrap();

        // Seed then delete most rows, leaving free pages the compaction packs.
        db.with_writer(|connection| {
            for i in 0..500 {
                connection.execute(
                    "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?1, ?2, 0)",
                    rusqlite::params![format!("s-{i}"), i as f64],
                )?;
            }
            connection.execute("DELETE FROM tracking_sessions WHERE id != 's-0'", [])?;
            Ok(())
        })
        .await
        .unwrap();

        let dest = dir.path().join("entropia_orme-compacted.db");
        db.vacuum_into(&dest).await.unwrap();
        assert!(dest.exists(), "the compacted copy is written");

        // The copy is a standalone, readable database carrying the live row
        // (opened raw, without re-running migrations).
        let copy = rusqlite::Connection::open(&dest).unwrap();
        let copied: i64 = copy
            .query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(copied, 1, "the compacted copy carries the surviving row");
        drop(copy);

        // The live database is untouched and still serves.
        let live = db
            .with_reader(|connection| {
                Ok(
                    connection.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(live, 1);
    }

    #[tokio::test]
    async fn quick_check_reports_ok_on_a_healthy_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        let outcome = db.quick_check_budgeted(Duration::from_secs(30)).await;
        assert!(
            matches!(outcome, QuickCheckOutcome::Ok),
            "a freshly migrated database is healthy: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn quick_check_honours_its_budget_and_returns_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        // A vanishing budget forces the timeout arm on all but the fastest
        // machines; either way the call returns promptly and never hangs,
        // which is the startup guarantee under test.
        let outcome = db.quick_check_budgeted(Duration::from_nanos(1)).await;
        assert!(
            matches!(
                outcome,
                QuickCheckOutcome::OverBudget | QuickCheckOutcome::Ok
            ),
            "a starved budget yields OverBudget (or Ok if it beat the clock): {outcome:?}"
        );
    }

    #[tokio::test]
    async fn baseline_creates_the_full_schema_surface_and_version_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        let count = |kind: &'static str| {
            let db = db.clone();
            async move {
                db.with_reader(move |connection| {
                    Ok(connection.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND sql IS NOT NULL \
                         AND name != '_sqlx_migrations' AND name NOT LIKE 'sqlite_%'",
                        rusqlite::params![kind],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap()
            }
        };
        // The fresh backend schema at version 33 (23 declared tables;
        // sqlite_sequence arrives automatically) plus the daily-rollup
        // migration's three projection tables, the market migration's
        // two feed tables, the harvest migration's two activity
        // tables, the map-pins table, the named-map table, four navigation
        // tables, the pin-configuration table, and the auction-sales
        // migration's three tables (listings, conversions, movements), which
        // also retired the harvest-stock overlay table it replaced;
        // index migrations: 18 baseline
        // + 4 analytical + the ledger date index + 2 market + 2 harvest
        // + the pin planet index + 2 named-map indexes + 4 navigation indexes
        // + 2 pin-configuration indexes + 2 harvest-yield indexes
        // + 5 auction-sales indexes + the live-listing index the undone-entry
        // migration adds + the session-interval migration's 3 interval
        // tables (intervals, contexts, membership) with 7 indexes (2 on
        // intervals, 1 on contexts, 1 on membership, and 3 on the event
        // tables' new context stamp) + the quest-families migration's
        // table and member index + the session-definitions migration's
        // 2 tables (definitions, roster) with 2 indexes (roster
        // definition, session definition stamp) + the hunting-provenance
        // migration's species index on the rebuilt movement ledger
        // + the loot item-name migration's 2 partial indexes + the
        // session-rollup migration's 4 tables (kill, loot, and PES cells
        // plus the settlement marker) with 4 indexes (session on each
        // cell table, item on the loot cells) + the hunting-definition
        // provenance index on the movement ledger + the quest reward
        // provenance index + the context-loot and quest-reward-item
        // tables with one index each + the private-sale and stock-removal
        // tables with their three indexes + the central-inventory
        // migration's 2 subject-and-state indexes
        // + the typed-reward, durable-run, unit-price, and reward-review
        // migrations' 6 tables and 5 indexes + the quest-run ownership
        // migration's 2 one-to-one indexes and 2 reciprocal-pair triggers
        // + the protection catalogue, loadout, observation, reconciliation,
        // interval-snapshot, and defensive-evidence migration's 7 tables and
        // 7 indexes + deferred protection settlement's 4 tables and 4 indexes
        // + manual hand-in's 2 raw-clump tables and 5 source, journal, and
        // waiting indexes + canonical quest rewards' 3 attribution,
        // reversal, and cooldown tables with 3 indexes, plus 2 provenance
        // indexes on the rebuilt movement ledger + healing attribution's 3
        // activation, effect-window, and output tables with 3 session indexes
        // + expected-hunting's offensive-evidence rollup table and 2 indexes
        // = 80 tables, 103 indexes, 10 triggers.
        assert_eq!(count("table").await, 80);
        assert_eq!(count("index").await, 103);
        assert_eq!(count("trigger").await, 10);

        let version = db
            .with_reader(|connection| {
                Ok(connection.query_row(
                    "SELECT value FROM db_metadata WHERE key = 'version'",
                    [],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(version, "33");
    }

    #[tokio::test]
    async fn quest_runs_and_completions_have_one_to_one_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        db.with_writer(|connection| {
            connection.execute(
                "INSERT INTO quests(id, name) VALUES(1, 'One'), (2, 'Two')",
                [],
            )?;
            connection.execute(
                "INSERT INTO session_quest_completions(id, session_id, quest_id, completed_at) \
                 VALUES(11, 's1', 1, 10.0), (12, 's2', 2, 20.0)",
                [],
            )?;
            connection.execute(
                "INSERT INTO quest_runs(id, quest_id, status, started_at, completed_at, completion_id) \
                 VALUES(21, 1, 'completed', 1.0, 10.0, 11), \
                       (22, 2, 'completed', 2.0, 20.0, 12)",
                [],
            )?;
            connection.execute(
                "UPDATE session_quest_completions SET quest_run_id = 21 WHERE id = 11",
                [],
            )?;

            let duplicate_completion = connection
                .execute("UPDATE quest_runs SET completion_id = 11 WHERE id = 22", [])
                .unwrap_err();
            assert_eq!(
                duplicate_completion.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ConstraintViolation)
            );

            let duplicate_run = connection
                .execute(
                    "UPDATE session_quest_completions SET quest_run_id = 21 WHERE id = 12",
                    [],
                )
                .unwrap_err();
            assert_eq!(
                duplicate_run.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ConstraintViolation)
            );

            connection.execute(
                "UPDATE session_quest_completions SET quest_run_id = NULL WHERE id = 12",
                [],
            )?;
            connection.execute("UPDATE quest_runs SET completion_id = NULL WHERE id = 22", [])?;
            let crossed_pair = connection
                .execute(
                    "UPDATE session_quest_completions SET quest_run_id = 22 WHERE id = 11",
                    [],
                )
                .unwrap_err();
            assert_eq!(
                crossed_pair.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ConstraintViolation)
            );
            Ok(())
        })
        .await
        .unwrap();
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
        db.with_writer(|connection| {
            connection.execute(
                "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES \
                 ('s-2', 200.0, 0), ('s-1', 100.0, 1)",
                [],
            )?;
            Ok(())
        })
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
        // A backend-created version-33 database: the baseline schema, seeded,
        // with no migration ledger (exactly what the backend leaves).
        {
            let connection = backend_baseline_db(&path);
            connection
                .execute(
                    "INSERT INTO tracking_sessions (id, started_at, is_active) \
                     VALUES ('kept', 1.0, 0)",
                    [],
                )
                .unwrap();
        }

        // Re-open: adoption marks the baseline applied without DDL, then the
        // post-baseline chain runs, and the migrator validates every ledger row.
        let db = Db::open(&path).await.unwrap();
        let (kept, ledger) = db
            .with_reader(|connection| {
                let kept: String =
                    connection
                        .query_row("SELECT id FROM tracking_sessions", [], |row| row.get(0))?;
                let ledger: i64 =
                    connection.query_row("SELECT COUNT(*) FROM _sqlx_migrations", [], |row| {
                        row.get(0)
                    })?;
                Ok((kept, ledger))
            })
            .await
            .unwrap();
        assert_eq!(kept, "kept");
        // The baseline stamp plus every post-baseline migration.
        assert_eq!(ledger, migrate::MIGRATIONS.len() as i64);
    }

    #[tokio::test]
    async fn pre_baseline_schema_versions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        {
            let db = Db::open(&path).await.unwrap();
            db.with_writer(|connection| {
                connection.execute(
                    "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                     VALUES ('keep-me', '2026-01-01', 'markup', 'survives refusal', 1.25, 'manual')",
                    [],
                )?;
                connection
                    .execute("UPDATE db_metadata SET value = '28' WHERE key = 'version'", [])?;
                connection.execute("DROP TABLE _sqlx_migrations", [])?;
                Ok(())
            })
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
        let connection = rusqlite::Connection::open(&path).unwrap();
        let (description, amount): (String, f64) = connection
            .query_row(
                "SELECT description, amount FROM ledger_entries WHERE id = 'keep-me'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(description, "survives refusal");
        assert_eq!(amount, 1.25);
        let version: String = connection
            .query_row(
                "SELECT value FROM db_metadata WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "28", "the stamp is left for the upgrade owner");
    }

    #[tokio::test]
    async fn below_baseline_v32_database_upgrades_to_the_baseline_with_data_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        // Synthesise the real in-the-wild surface: a v0.1.0-lineage database
        // last owned by the Python backend at version 32. Start from the
        // backend baseline (v33 schema, no migration ledger), then walk it back to
        // v32: re-create the table v33 dropped (with a row, to prove the drop
        // is the only loss) and stamp the version row back to 32.
        {
            let connection = backend_baseline_db(&path);
            connection
                .execute(
                    "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                     VALUES ('keep-me', '2026-01-01', 'markup', 'survives upgrade', 4.2, 'manual')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "CREATE TABLE tt_curve_observations (id INTEGER PRIMARY KEY, value REAL)",
                    [],
                )
                .unwrap();
            connection
                .execute("INSERT INTO tt_curve_observations (value) VALUES (1.0)", [])
                .unwrap();
            connection
                .execute(
                    "UPDATE db_metadata SET value = '32' WHERE key = 'version'",
                    [],
                )
                .unwrap();
        }

        // Re-open: the v32 rung runs, then adoption stamps the baseline and the
        // post-adoption migrator validates the ledger. No refusal.
        let db = Db::open(&path).await.unwrap();

        let (has_retired, version, ledger, description, amount) = db
            .with_reader(|connection| {
                let has_retired = table_exists_sync(connection, "tt_curve_observations")?;
                let version: String = connection.query_row(
                    "SELECT value FROM db_metadata WHERE key = 'version'",
                    [],
                    |row| row.get(0),
                )?;
                let ledger: i64 =
                    connection.query_row("SELECT COUNT(*) FROM _sqlx_migrations", [], |row| {
                        row.get(0)
                    })?;
                let (description, amount): (String, f64) = connection.query_row(
                    "SELECT description, amount FROM ledger_entries WHERE id = 'keep-me'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                Ok((has_retired, version, ledger, description, amount))
            })
            .await
            .unwrap();

        // The retired table is gone, and the version row now matches a fresh
        // v33 database.
        assert!(!has_retired, "the v33 rung drops tt_curve_observations");
        assert_eq!(
            version, "33",
            "the upgrade bumps the version row to the baseline"
        );
        assert_eq!(
            ledger,
            migrate::MIGRATIONS.len() as i64,
            "the baseline is stamped once, then the post-baseline chain runs"
        );
        // The user's data survives the upgrade untouched.
        assert_eq!((description.as_str(), amount), ("survives upgrade", 4.2));
    }

    /// Rebuild `codex_claims` in its pre-mastery shape (no `kind` /
    /// `attribute_name`) on a backend baseline, reproducing the real
    /// Python-lineage drift: a version-33 database the retired ladder left
    /// without the columns the Rust baseline declares.
    fn drop_codex_mastery_columns(connection: &rusqlite::Connection) {
        connection
            .execute_batch(super::LEGACY_CODEX_CLAIMS_DDL)
            .unwrap();
    }

    #[tokio::test]
    async fn adopted_database_missing_baseline_columns_is_healed_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        // A version-33 database as the retired ladder left it: the baseline
        // schema, but codex_claims still lacks kind/attribute_name, with an
        // existing rank claim (no kind) to prove the 'rank' default backfills.
        {
            let connection = backend_baseline_db(&path);
            drop_codex_mastery_columns(&connection);
            connection
                .execute(
                    "INSERT INTO codex_claims (species_name, rank, skill_name, ped_value) \
                     VALUES ('Boar', 1, 'Rifle', 1.0)",
                    [],
                )
                .unwrap();
        }

        let db = Db::open(&path).await.unwrap();

        // The columns the baseline declares are present after open, and the
        // pre-existing row backfilled to the 'rank' default.
        let (has_kind, has_attr, backfilled) = db
            .with_reader(|connection| {
                let columns = column_names_sync(connection, "codex_claims")?;
                let backfilled: String = connection.query_row(
                    "SELECT kind FROM codex_claims WHERE species_name = 'Boar' AND rank = 1",
                    [],
                    |row| row.get(0),
                )?;
                Ok((
                    columns.contains("kind"),
                    columns.contains("attribute_name"),
                    backfilled,
                ))
            })
            .await
            .unwrap();
        assert!(has_kind, "the missing kind column is healed");
        assert!(has_attr, "the missing attribute_name column is healed");
        assert_eq!(backfilled, "rank", "the existing rank claim backfills");

        // The exact species read that failed on the drifted database now runs:
        // no mastery claims yet, so it returns no rows rather than erroring on
        // the once-missing column.
        let mastery_rows = db
            .with_reader(|connection| {
                connection
                    .query_row(
                        "SELECT species_name, COUNT(*) FROM codex_claims \
                         WHERE kind = 'mastery' GROUP BY species_name",
                        [],
                        |row| row.get::<_, i64>(1),
                    )
                    .optional()
                    .map_err(DbError::from)
            })
            .await
            .unwrap();
        assert_eq!(
            mastery_rows, None,
            "the healed read runs and finds no mastery claims"
        );

        // Mastery/meta claims (which write the healed columns) record.
        db.with_writer(|connection| {
            connection.execute(
                "INSERT INTO codex_claims \
                 (species_name, rank, skill_name, ped_value, claimed_at, kind) \
                 VALUES ('Boar', 26, 'Rifle', 2.0, 1.0, 'mastery')",
                [],
            )?;
            connection.execute(
                "INSERT INTO codex_claims \
                 (species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name) \
                 VALUES ('', 0, '', 0.5, 1.0, 'meta', 'Agility')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reconcile_leaves_a_healthy_adopted_database_untouched() {
        // A correctly-adopted database already carries every baseline column;
        // reconciliation must not attempt to re-add one (a duplicate ADD COLUMN
        // would error). Adoption succeeding with the mastery claim intact proves
        // the no-op.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        {
            let connection = backend_baseline_db(&path);
            connection
                .execute(
                    "INSERT INTO codex_claims \
                     (species_name, rank, skill_name, ped_value, kind) \
                     VALUES ('Boar', 26, 'Rifle', 2.0, 'mastery')",
                    [],
                )
                .unwrap();
        }
        let db = Db::open(&path).await.unwrap();
        let kept: String = db
            .with_reader(|connection| {
                Ok(connection.query_row(
                    "SELECT kind FROM codex_claims WHERE species_name = 'Boar'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(kept, "mastery");
    }

    #[tokio::test]
    async fn schema_versions_below_the_v32_bridge_are_still_refused() {
        // The bridge is the single v32 rung; v31 (and anything older) remains a
        // terminal refusal, with the user's rows left untouched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entropia_orme.db");
        {
            let db = Db::open(&path).await.unwrap();
            db.with_writer(|connection| {
                connection.execute(
                    "UPDATE db_metadata SET value = '31' WHERE key = 'version'",
                    [],
                )?;
                connection.execute("DROP TABLE _sqlx_migrations", [])?;
                Ok(())
            })
            .await
            .unwrap();
        }
        let err = Db::open(&path).await.unwrap_err();
        match err {
            DbError::UnsupportedSchemaVersion { found, supported } => {
                assert_eq!((found, supported), (31, 33));
            }
            other => panic!("expected a schema-version refusal, got {other}"),
        }
    }

    #[tokio::test]
    async fn schema_master_lists_the_real_objects_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let master = db.schema_master().await.unwrap();
        // 40 declared tables (23 baseline + 3 daily-rollup projection +
        // 2 market feed + 2 harvest activity + map pins + map views + 4
        // navigation tables + pin configs + 3 auction-sales tables, the
        // harvest-stock overlay having been retired into them) + the
        // migration ledger + 44 indexes
        // (18 baseline + 4 analytical + the ledger date index + 2 market + 2
        // harvest + the pin planet index + 2 map-view indexes + 4 navigation
        // indexes + 2 pin-configuration indexes + 2 harvest-yield indexes +
        // 5 auction-sales indexes + the undone-entry migration's
        // live-listing index) + the session-interval migration's 3
        // interval tables and 7 indexes + the quest-families migration's
        // table and member index + the session-definitions migration's
        // 2 tables and 2 indexes + the hunting-provenance migration's
        // species index on the rebuilt movement ledger + the loot
        // item-name migration's 2 partial indexes + the session-rollup
        // migration's 4 tables and 4 indexes + the hunting-definition
        // provenance index + the quest reward provenance index + the
        // context-loot and quest-reward-item tables with one index each +
        // the private-sale and stock-removal tables with three indexes +
        // the central-inventory migration's two indexes +
        // the quest-run ownership migration's 2 indexes and 2 triggers + 8
        // earlier triggers + the protection and deferred-settlement
        // migrations' 11 tables and 11 indexes + the manual-hand-in
        // migration's 2 tables and 5 indexes + canonical quest rewards'
        // 3 tables and 5 attribution and provenance indexes + healing
        // attribution's 3 activation, effect-window, and output tables with
        // 3 session indexes + expected-hunting's offensive-evidence rollup
        // table and 2 indexes (only SQLite's own bookkeeping is excluded; the
        // conformance comparison filters the ledger externally as its one
        // deliberate difference).
        assert_eq!(master.len(), 194);
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
        // baseline (version below it, no migration ledger): the first-launch-
        // after-upgrade race. open_adopted quarantines it, and
        // is_below_baseline() flags it as the retry-worthy case.
        let below = dir.path().join("below.db");
        {
            let db = Db::open(&below).await.unwrap();
            db.with_writer(|connection| {
                connection.execute(
                    "UPDATE db_metadata SET value = '28' WHERE key = 'version'",
                    [],
                )?;
                connection.execute("DROP TABLE _sqlx_migrations", [])?;
                Ok(())
            })
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
        assert!(!AdoptError::Fresh(DbError::CoreClosed).is_below_baseline());
    }

    #[test]
    fn adopt_error_display_carries_the_path_and_the_stand_down() {
        // A driver-level source (SQLite's own "file is not a database"),
        // wrapped as the quarantine's cause.
        let not_a_database = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
            Some("file is not a database".to_string()),
        );
        let quarantined = AdoptError::Quarantined {
            path: std::path::PathBuf::from("somewhere/entropia_orme.db"),
            source: DbError::Sqlite(not_a_database),
        };
        let rendered = quarantined.to_string();
        assert!(rendered.contains("somewhere"), "{rendered}");
        assert!(rendered.contains("cannot be adopted"), "{rendered}");
        assert!(rendered.contains("file is not a database"), "{rendered}");
        assert!(rendered.contains("left untouched"), "{rendered}");

        // `Fresh` renders its source plainly, adding nothing of its own.
        assert_eq!(
            AdoptError::Fresh(DbError::CoreClosed).to_string(),
            DbError::CoreClosed.to_string()
        );

        // A contextualising variant carries its cause as a walkable source
        // chain, not a flattened string.
        let parse_err = serde_json::from_str::<Value>("not json").unwrap_err();
        let err = DbError::Decode {
            context: "equipment properties parse",
            source: parse_err,
        };
        assert!(err.to_string().starts_with("equipment properties parse: "));
        assert!(std::error::Error::source(&err).is_some());
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

    #[tokio::test]
    async fn sync_core_connections_carry_the_session_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        // Both roles: a reader connection and the writer connection.
        for role in ["reader", "writer"] {
            let probe = |connection: &mut rusqlite::Connection| {
                let journal: String =
                    connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                let synchronous: i64 =
                    connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
                let cache: i64 = connection.query_row("PRAGMA cache_size", [], |row| row.get(0))?;
                let foreign_keys: i64 =
                    connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                let busy: i64 =
                    connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
                Ok((journal, synchronous, cache, foreign_keys, busy))
            };
            let (journal, synchronous, cache, foreign_keys, busy) = if role == "reader" {
                db.with_reader(probe).await.unwrap()
            } else {
                db.with_writer(probe).await.unwrap()
            };
            assert_eq!(journal, "wal", "{role} journal_mode");
            assert_eq!(synchronous, 1, "{role} synchronous NORMAL");
            assert_eq!(cache, -64000, "{role} cache_size 64 MB");
            assert_eq!(foreign_keys, 0, "{role} foreign keys stay off");
            assert_eq!(busy, 5000, "{role} busy_timeout");
        }
    }

    #[tokio::test]
    async fn sync_core_serialises_writes_and_serves_concurrent_readers() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        // Concurrent write closures all land; the single writer thread
        // serialises them (interleaving is structurally impossible).
        let writes: Vec<_> = (0..32)
            .map(|i| {
                let db = db.clone();
                tokio::spawn(async move {
                    db.with_writer(move |connection| {
                        connection.execute(
                            "INSERT INTO tracking_sessions (id, started_at, is_active) \
                             VALUES (?1, ?2, 0)",
                            rusqlite::params![format!("s-{i}"), i as f64],
                        )?;
                        Ok(())
                    })
                    .await
                })
            })
            .collect();
        for handle in writes {
            handle.await.unwrap().unwrap();
        }

        // Concurrent read closures observe the committed state.
        let reads: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move {
                    db.with_reader(|connection| {
                        let count: i64 = connection.query_row(
                            "SELECT COUNT(*) FROM tracking_sessions",
                            [],
                            |row| row.get(0),
                        )?;
                        Ok(count)
                    })
                    .await
                })
            })
            .collect();
        for handle in reads {
            assert_eq!(handle.await.unwrap().unwrap(), 32);
        }
    }

    #[tokio::test]
    async fn sync_core_write_transactions_are_one_closure() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        // A multi-statement transaction commits atomically inside one
        // closure; a rolled-back one leaves nothing.
        db.with_writer(|connection| {
            let tx = connection.transaction()?;
            for i in 0..3 {
                tx.execute(
                    "INSERT INTO tracking_sessions (id, started_at, is_active) \
                     VALUES (?1, ?2, 0)",
                    rusqlite::params![format!("kept-{i}"), i as f64],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        db.with_writer(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('gone', 9.0, 0)",
                [],
            )?;
            drop(tx); // an uncommitted transaction rolls back
            Ok(())
        })
        .await
        .unwrap();
        let count = db
            .with_reader(|connection| {
                let count: i64 =
                    connection.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                        row.get(0)
                    })?;
                Ok(count)
            })
            .await
            .unwrap();
        assert_eq!(count, 3, "the committed rows and nothing else");
    }

    #[tokio::test]
    async fn sync_core_propagates_a_closure_panic_and_survives_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        // The panic propagates to the awaiting caller (spawned so this
        // test observes it as a JoinError rather than dying with it)...
        let doomed = {
            let db = db.clone();
            tokio::spawn(async move {
                db.with_writer(|_| -> Result<(), DbError> { panic!("closure panic") })
                    .await
            })
        };
        assert!(doomed.await.unwrap_err().is_panic());

        // ...and the worker thread survives to serve the next job.
        let alive = db
            .with_writer(|connection| {
                let one: i64 = connection.query_row("SELECT 1", [], |row| row.get(0))?;
                Ok(one)
            })
            .await
            .unwrap();
        assert_eq!(alive, 1);
    }

    #[tokio::test]
    async fn sync_core_blocking_variants_serve_plain_threads() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let worker = {
            let db = db.clone();
            std::thread::spawn(move || {
                db.with_writer_blocking(|connection| {
                    connection.execute(
                        "INSERT INTO tracking_sessions (id, started_at, is_active) \
                         VALUES ('from-a-thread', 1.0, 0)",
                        [],
                    )?;
                    Ok(())
                })?;
                db.with_reader_blocking(|connection| {
                    let count: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM tracking_sessions",
                        [],
                        |row| row.get(0),
                    )?;
                    Ok(count)
                })
            })
        };
        assert_eq!(worker.join().unwrap().unwrap(), 1);
    }

    /// The cross-runner equivalence pin: a database freshly migrated by
    /// the embedded runner carries a `_sqlx_migrations` ledger
    /// byte-identical in accounting (versions, descriptions, checksums)
    /// to the chain's own derivation, which the checksum test in
    /// `migrate` pins to the digests already in the wild.
    #[tokio::test]
    async fn migration_ledger_reproduces_the_inherited_accounting() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        let rows = db
            .with_reader(|connection| {
                let mut statement = connection.prepare(
                    "SELECT version, description, checksum FROM _sqlx_migrations ORDER BY version",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<(i64, String, Vec<u8>)>>>()?;
                Ok(rows)
            })
            .await
            .unwrap();
        assert_eq!(rows.len(), migrate::MIGRATIONS.len());
        for (row, embedded) in rows.iter().zip(migrate::MIGRATIONS) {
            assert_eq!(row.0, embedded.version);
            assert_eq!(row.1, embedded.description);
            assert_eq!(row.2, embedded.checksum(), "checksum for {}", row.1);
        }
    }

    #[tokio::test]
    async fn equipment_insert_read_update_and_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        // The public insert returns the autoincrement id, growing from 1.
        let first = db
            .insert_equipment(
                "Opalo".into(),
                "weapon".into(),
                Some("cat-1".into()),
                r#"{"weapon_entity":{}}"#.into(),
            )
            .await
            .unwrap();
        let second = db
            .insert_equipment(
                "Healer".into(),
                "healing".into(),
                None,
                r#"{"tool":1}"#.into(),
            )
            .await
            .unwrap();
        assert_eq!((first, second), (1, 2));

        // equipment_row: the full four-tuple by id, None when absent.
        assert_eq!(
            db.equipment_row(first).await.unwrap(),
            Some((
                1,
                "Opalo".into(),
                "weapon".into(),
                r#"{"weapon_entity":{}}"#.into(),
            )),
        );
        assert_eq!(db.equipment_row(999).await.unwrap(), None);

        // equipment_detail_row: carries the optional catalogue id verbatim.
        assert_eq!(
            db.equipment_detail_row(first).await.unwrap(),
            Some((
                1,
                "Opalo".into(),
                "weapon".into(),
                Some("cat-1".into()),
                r#"{"weapon_entity":{}}"#.into(),
            )),
        );
        assert_eq!(
            db.equipment_detail_row(second).await.unwrap(),
            Some((
                2,
                "Healer".into(),
                "healing".into(),
                None,
                r#"{"tool":1}"#.into()
            )),
        );
        assert_eq!(db.equipment_detail_row(999).await.unwrap(), None);

        // equipment_item_type: the type string alone, None when absent.
        assert_eq!(
            db.equipment_item_type(first).await.unwrap(),
            Some("weapon".into())
        );
        assert_eq!(
            db.equipment_item_type(second).await.unwrap(),
            Some("healing".into())
        );
        assert_eq!(db.equipment_item_type(999).await.unwrap(), None);

        // equipment_library_rows: every stored row (order-independent here).
        let mut rows = db.equipment_library_rows().await.unwrap();
        rows.sort_by_key(|row| row.0);
        assert_eq!(
            rows,
            vec![
                (
                    1,
                    "Opalo".into(),
                    "weapon".into(),
                    r#"{"weapon_entity":{}}"#.into()
                ),
                (2, "Healer".into(), "healing".into(), r#"{"tool":1}"#.into()),
            ],
        );

        // update_equipment replaces name, catalogue binding and properties;
        // the item type is fixed.
        db.update_equipment(
            first,
            "Opalo Mk II".into(),
            None,
            r#"{"weapon_entity":{"v":2}}"#.into(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.equipment_detail_row(first).await.unwrap(),
            Some((
                1,
                "Opalo Mk II".into(),
                "weapon".into(),
                None,
                r#"{"weapon_entity":{"v":2}}"#.into(),
            )),
        );

        // delete_equipment removes the row (and is idempotent over a miss).
        db.delete_equipment(first).await.unwrap();
        assert_eq!(db.equipment_row(first).await.unwrap(), None);
        db.delete_equipment(first).await.unwrap();
        assert_eq!(db.equipment_library_rows().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn skill_calibrations_read_the_latest_per_skill_and_the_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;

        // No rows yet: no epoch, empty calibrations.
        assert_eq!(db.last_calibration_epoch().await.unwrap(), None);
        assert!(db.latest_skill_calibrations(None).await.unwrap().is_empty());

        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES \
                 ('Rifle', 10.0, 'scan', 100.0), \
                 ('Rifle', 12.0, 'manual', 200.0), \
                 ('Agility', 5.0, 'scan', 150.0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Believed-current (source = None): the newest row per skill wins,
        // so Rifle reads the manual 12.0, not the earlier scan.
        let mut current = db.latest_skill_calibrations(None).await.unwrap();
        current.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            current,
            vec![("Agility".into(), 5.0), ("Rifle".into(), 12.0)]
        );

        // The scan anchor: only scan rows, so Rifle reads its scan 10.0.
        let mut scan = db
            .latest_skill_calibrations(Some("scan".into()))
            .await
            .unwrap();
        scan.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(scan, vec![("Agility".into(), 5.0), ("Rifle".into(), 10.0)]);

        // The epoch is the maximum scan instant across every source.
        assert_eq!(db.last_calibration_epoch().await.unwrap(), Some(200.0));
    }

    #[tokio::test]
    async fn optimize_on_shutdown_reports_success_on_a_healthy_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        assert!(
            db.optimize_on_shutdown().await,
            "PRAGMA optimize succeeds on a live writer"
        );
    }

    #[tokio::test]
    async fn debug_render_names_the_synchronous_core() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_db(dir.path()).await;
        // The handle's Debug delegates to the core's own formatter, so the
        // core must name itself rather than render empty.
        assert!(
            format!("{db:?}").contains("SyncCore"),
            "the core's Debug names it: {db:?}"
        );
    }
}
