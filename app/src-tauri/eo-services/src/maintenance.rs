//! On-demand read-model maintenance: rebuild every projection from the raw
//! tracking tables and prove the result matches the incrementally-maintained
//! rows.
//!
//! The Overview's `daily_rollups` / `daily_ledger_rollups` and the Activity
//! and session-list `session_summaries` are CQRS-style read models: the raw
//! tracking tables are the source of truth, and each projection is a pure
//! function of them, maintained eagerly on write and healed lazily on read.
//! This module makes that guarantee runnable: heal the projections current,
//! snapshot them, drop and rebuild them from the raw tables alone, then
//! assert the rebuilt rows are byte-identical to the incrementally-maintained
//! ones. A mismatch is a projection-staleness bug (a missed write hook),
//! named and surfaced instead of silently served.
//!
//! Each projection row carries a `computed_at` bookkeeping timestamp that is
//! wall-clock at recompute and so differs between an incremental write and a
//! rebuild; it is excluded from the comparison. Every column that carries
//! meaning is included.

use serde_json::{Map, Value};

use crate::db::{Db, DbError};
use crate::{daily_rollup, session_rollup, session_summary};

/// A projection table and the deterministic, `computed_at`-excluding
/// snapshot query used to compare its incremental and rebuilt states.
struct Projection {
    table: &'static str,
    snapshot_sql: &'static str,
}

/// The read models, each rebuildable from the raw tracking tables.
const PROJECTIONS: [Projection; 9] = [
    Projection {
        table: "session_summaries",
        snapshot_sql: "SELECT session_id, summary_version, started_at, ended_at, \
             duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, \
             heal_cost, dangling_cost, cycled_ped, regular_skill_ped_json, \
             attribute_levels_json, regular_skill_tt, attribute_levels_total, \
             dominant_mob, dominant_tag, dominant_weapon, dominant_mob_kills, \
             dominant_tag_kills, activity_skill_tt, primary_mobs_json, \
             primary_weapons_json, globals, hofs, harvest_swings, \
             harvest_successes, harvest_loot_tt, harvest_cost, consumable_cost \
             FROM session_summaries ORDER BY session_id",
    },
    Projection {
        table: "daily_rollups",
        snapshot_sql: "SELECT day, rollup_version, dirty, has_rows, loot_tt, weapon_cost, \
             enhancer_cost, armour_cost, heal_cost, dangling_cost, skill_tt, codex_pes, \
             quest_pes, harvest_loot_tt, harvest_cost, consumable_cost FROM daily_rollups \
             ORDER BY day",
    },
    Projection {
        table: "daily_ledger_rollups",
        snapshot_sql: "SELECT day, entry_type, tag, amount FROM daily_ledger_rollups \
             ORDER BY day, entry_type, tag",
    },
    Projection {
        table: "session_kill_rollups",
        snapshot_sql: "SELECT session_id, context_id, mob_species, mob_maturity, kills, \
             cycled_ped, loot_tt FROM session_kill_rollups \
             ORDER BY session_id, context_id, mob_species, mob_maturity",
    },
    Projection {
        table: "session_loot_rollups",
        snapshot_sql: "SELECT session_id, mob_species, is_enhancer_shrapnel, item_name, \
             quantity, value_ped FROM session_loot_rollups \
             ORDER BY session_id, mob_species, is_enhancer_shrapnel, item_name",
    },
    Projection {
        table: "session_context_loot_rollups",
        snapshot_sql: "SELECT session_id, context_id, item_name, quantity, value_ped \
             FROM session_context_loot_rollups \
             ORDER BY session_id, context_id, item_name",
    },
    Projection {
        table: "session_pes_rollups",
        snapshot_sql: "SELECT session_id, context_id, pes FROM session_pes_rollups \
             ORDER BY session_id, context_id",
    },
    Projection {
        table: "session_offensive_evidence_rollups",
        snapshot_sql: "SELECT session_id, context_id, mob_species, evidence_fingerprint, \
             expected_economics_json, shots_fired, missing_candidate_raw_tt, \
             missing_basis_phases FROM session_offensive_evidence_rollups \
             ORDER BY session_id, context_id, mob_species, evidence_fingerprint",
    },
    Projection {
        table: "session_rollup_meta",
        snapshot_sql: "SELECT session_id, rollup_version FROM session_rollup_meta \
             ORDER BY session_id",
    },
];

/// One projection table's verdict from a rebuild-and-verify run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableVerdict {
    /// The projection table's name.
    pub table: &'static str,
    /// Whether the rebuilt rows were byte-identical to the
    /// incrementally-maintained ones.
    pub matched: bool,
    /// The row count after the rebuild (equal to the incremental count when
    /// `matched`).
    pub row_count: usize,
}

/// The result of rebuilding every read model and comparing it against the
/// incrementally-maintained rows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RebuildReport {
    /// One verdict per projection, in a stable order.
    pub tables: Vec<TableVerdict>,
}

impl RebuildReport {
    /// True when every projection rebuilt byte-identically: the CQRS
    /// rebuildability guarantee holds.
    pub fn all_matched(&self) -> bool {
        self.tables.iter().all(|verdict| verdict.matched)
    }
}

/// One rusqlite row to the same canonical, stored-value-typed JSON the
/// snapshot catalogue's [`crate::db::row_to_json`] produces: integer,
/// real, text, or null keyed by column name. Both the incremental and the
/// rebuilt snapshots pass through this one shaper, so the equality proof
/// compares like with like.
fn row_to_json_sync(row: &rusqlite::Row, columns: &[String]) -> Result<Value, DbError> {
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

/// Snapshot one projection's rows in its deterministic order, each row
/// canonically serialised (typed by its stored value) for comparison.
fn snapshot(conn: &rusqlite::Connection, sql: &'static str) -> Result<Vec<Value>, DbError> {
    let mut stmt = conn.prepare(sql)?;
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|&name| name.to_string())
        .collect();
    let rows = stmt
        .query_map([], |row| Ok(row_to_json_sync(row, &columns)))?
        .collect::<rusqlite::Result<Vec<Result<Value, DbError>>>>()?;
    rows.into_iter().collect()
}

/// Heal the read models current, then drop and rebuild every one of them
/// from the raw tracking tables, asserting the rebuilt rows are byte-
/// identical to the incrementally-maintained ones. This is the proof that
/// the projections are a pure function of the raw tracking tables (the CQRS
/// rebuildability guarantee recorded in ADR-0018) and the standing
/// mitigation for projection staleness: a mismatch names the table whose
/// incremental maintenance drifted from a rebuild.
///
/// Runs entirely on the writer: the rebuild is a write, and each snapshot
/// reads back its own committed writes on the same serialised connection,
/// so there is no cross-connection visibility window. Off the hot path.
pub async fn rebuild_and_verify(db: &Db, now: f64) -> Result<RebuildReport, DbError> {
    db.with_writer(move |conn| {
        // Bring the incremental projections fully current, so the comparison is
        // against an up-to-date maintained state rather than a mid-heal one.
        daily_rollup::heal_rollups(conn, now)?;
        session_summary::heal_summaries(conn)?;
        session_rollup::heal(conn)?;

        let mut incremental = Vec::with_capacity(PROJECTIONS.len());
        for projection in &PROJECTIONS {
            incremental.push(snapshot(conn, projection.snapshot_sql)?);
        }

        // Rebuild every read model from the raw tables alone.
        daily_rollup::rebuild_rollups(conn, now)?;
        session_summary::rebuild_summaries(conn)?;
        session_rollup::rebuild(conn)?;

        let mut tables = Vec::with_capacity(PROJECTIONS.len());
        for (projection, maintained) in PROJECTIONS.iter().zip(incremental) {
            let rebuilt = snapshot(conn, projection.snapshot_sql)?;
            tables.push(TableVerdict {
                table: projection.table,
                matched: rebuilt == maintained,
                row_count: rebuilt.len(),
            });
        }
        Ok(RebuildReport { tables })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// A fixed clock well past any seeded day, so the heal watermark covers
    /// the seeded calendar.
    const NOW: f64 = 1_000_000_000.0;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    async fn run(db: &Db, sql: &str) {
        db.with_writer({
            let sql = sql.to_string();
            move |conn| {
                conn.execute_batch(&sql)?;
                Ok(())
            }
        })
        .await
        .unwrap();
    }

    /// Seed a session with kills, loot, skill gains and ledger entries, so
    /// every projection family is non-empty.
    async fn seed(db: &Db) {
        run(
            db,
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) \
             VALUES ('s1', 999648000.0, 999658800.0, 0, 0.07, 0.11, 0.0)",
        )
        .await;
        run(
            db,
            "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
             VALUES ('k1', 's1', 'Atrox', 999652000.0, 0.02, 2.0), \
                    ('k2', 's1', 'Atrox', 999653000.0, 0.02, 4.5)",
        )
        .await;
        run(
            db,
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES ('k1', 'Rifle', 30, 0.05)",
        )
        .await;
        run(
            db,
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('s1', 999652100.0, 'Rifle', 1.0, 0.5)",
        )
        .await;
        run(
            db,
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Animal Hide', 3, 1.2, 0)",
        )
        .await;
        run(
            db,
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) VALUES \
             ('l1', '2001-09-05', 'markup', 'sale', 3.0, 'manual'), \
             ('l2', '2001-09-05', 'expense', 'repair', 1.0, 'repair')",
        )
        .await;
    }

    #[tokio::test]
    async fn rebuild_and_verify_matches_the_incrementally_maintained_models() {
        let (_dir, db) = db().await;
        seed(&db).await;

        let report = rebuild_and_verify(&db, NOW).await.unwrap();
        assert!(
            report.all_matched(),
            "every projection is a pure function of the raw tables: {report:?}"
        );
        // Every projection is covered and non-trivial.
        assert_eq!(report.tables.len(), 9);
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_summaries" && t.row_count == 1));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "daily_rollups" && t.row_count > 0));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "daily_ledger_rollups" && t.row_count == 2));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_kill_rollups" && t.row_count == 1));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_loot_rollups" && t.row_count == 1));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_context_loot_rollups" && t.row_count == 1));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_pes_rollups" && t.row_count == 1));
        assert!(report
            .tables
            .iter()
            .any(|t| t.table == "session_rollup_meta" && t.row_count == 1));
    }

    #[tokio::test]
    async fn a_corrupted_projection_row_is_caught_as_a_mismatch() {
        let (_dir, db) = db().await;
        seed(&db).await;
        // Heal so the incremental rows exist, then poison one: this is the
        // projection-staleness class the verifier exists to catch.
        db.with_writer(move |conn| daily_rollup::heal_rollups(conn, NOW))
            .await
            .unwrap();
        run(
            &db,
            "UPDATE daily_rollups SET loot_tt = 999.0 WHERE day = '2001-09-05'",
        )
        .await;

        let report = rebuild_and_verify(&db, NOW).await.unwrap();
        // The rebuild wipes the poisoned value, so the snapshot taken before
        // the rebuild (the poisoned one) differs from the rebuilt truth.
        assert!(
            !report.all_matched(),
            "a drifted daily_rollups row must fail the equality proof"
        );
        let rollups = report
            .tables
            .iter()
            .find(|t| t.table == "daily_rollups")
            .unwrap();
        assert!(!rollups.matched);
    }
}
