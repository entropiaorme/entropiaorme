//! Materialised per-day analytics rollups: the read model behind the
//! Overview and its breakdowns. Source of truth is the raw tracking
//! tables; rows write eagerly at the mutation points and heal lazily
//! (versioned) on read, following the `session_summaries` discipline.
//! The rollup tables sit outside the DB-state snapshot catalogue, so
//! parity surfaces through the analytics HTTP responses.
//!
//! ## The model
//!
//! One `daily_rollups` row per UTC day stores each aggregate family's
//! per-day SUM verbatim (NULL when the day had no contributing rows),
//! plus `has_rows`, which decides the day's membership in the
//! timeline/monthly point sets: NULL sums erase the difference between
//! "no rows" and "rows that summed to NULL", and both wire distinctions
//! matter. `daily_ledger_rollups` carries the per-day ledger sums by
//! entry type and tag, unrounded.
//!
//! The `daily_rollup_meta.rolled_through` watermark is the single split
//! boundary: every day from the earliest data day up to and including
//! it has a rollup row (empty days included, keeping the range
//! contiguous), and readers serve anything after it from the raw
//! tables. Healing advances the watermark to
//! yesterday, so the current day is never served from rollups and every
//! day is recomputed once after it completes; a hook missed on the
//! in-flight day can therefore never freeze a stale row. Writes dated
//! at or before the watermark (a backdated ledger entry, an unclaim, a
//! loot adjustment) mark their day dirty in the writing transaction and
//! recompute it eagerly; the dirty flag survives a crash between the
//! two, and the next heal repairs it.
//!
//! Ledger dates are user-entered TEXT and may not name a canonical
//! calendar day; the heal sweeps such stray keys into rollup rows of
//! their own (ledger sums only, epoch families NULL), so the read path
//! reproduces the raw bucketing byte for byte. A key that parses but is
//! not canonically formatted (`2026-6-5`) is deliberately treated as
//! stray rather than mapped to its calendar day: giving it that day's
//! epoch windows would double-count the epoch families against the
//! canonical row.

use chrono::NaiveDate;
use rusqlite::OptionalExtension as _;

use crate::db::DbError;

/// Bump when a rollup column's meaning changes: below-version rows heal
/// on the next read.
// Bumped to 3 when confirmed non-ammo quest-item TT joined the projection;
// to 4 when protection costs were re-attributed by hit count; to 5 when
// consumed doses gained their own cost bucket. Below-version rows heal on the
// next read.
pub const ROLLUP_VERSION: i64 = 5;

/// The UTC day of an epoch second, rendered as SQLite's
/// `date(epoch, 'unixepoch')` renders it (`YYYY-MM-DD`).
pub fn epoch_day(epoch: f64) -> String {
    chrono::DateTime::from_timestamp(epoch.floor() as i64, 0)
        .expect("epoch within chrono range")
        .format("%Y-%m-%d")
        .to_string()
}

/// The canonical calendar day named by a rollup key, or None for a
/// stray key. Round-trips the format so a parseable-but-non-canonical
/// spelling (`2026-6-5`) stays stray instead of aliasing its calendar
/// day's epoch windows.
fn canonical_day(day: &str) -> Option<NaiveDate> {
    let parsed = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    (parsed.format("%Y-%m-%d").to_string() == day).then_some(parsed)
}

/// The `[00:00, next 00:00)` UTC epoch window of a canonical day key,
/// or None for a stray key. The read path partitions its windows on
/// these boundaries.
pub fn day_range(day: &str) -> Option<(f64, f64)> {
    canonical_day(day).map(day_bounds)
}

/// The `[00:00, next 00:00)` UTC epoch window of a canonical day.
fn day_bounds(date: NaiveDate) -> (f64, f64) {
    let start = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    let end = start + chrono::Duration::days(1);
    (
        start.and_utc().timestamp() as f64,
        end.and_utc().timestamp() as f64,
    )
}

fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// One `SELECT COUNT(*), SUM(a) [, SUM(b), ...]` over an epoch window,
/// returning the row count and each sum verbatim (NULL preserved).
fn window_sums(
    conn: &rusqlite::Connection,
    sql: &str,
    start: f64,
    end: f64,
    sums: usize,
) -> Result<(i64, Vec<Option<f64>>), DbError> {
    let out = conn.query_row(sql, rusqlite::params![start, end], |row| {
        let count: i64 = row.get(0)?;
        let mut values = Vec::with_capacity(sums);
        for index in 0..sums {
            values.push(row.get::<_, Option<f64>>(index + 1)?);
        }
        Ok((count, values))
    })?;
    Ok(out)
}

/// Recompute one day's rollup row and ledger rollup rows from the raw
/// tables. The caller owns the surrounding commit semantics (the write
/// hooks run this inside their transaction; the heal wraps its own).
pub fn recompute_day(conn: &rusqlite::Connection, day: &str) -> Result<(), DbError> {
    let mut has_rows = false;
    let mut families: [Option<f64>; 13] = [None; 13];

    if let Some(date) = canonical_day(day) {
        let (start, end) = day_bounds(date);

        let (kill_count, kill_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(loot_total_ped), SUM(enhancer_cost) \
             FROM kills WHERE timestamp >= ? AND timestamp < ?",
            start,
            end,
            2,
        )?;
        let (weapon_count, weapon_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(ts.cost_per_shot * ts.shots_fired) \
             FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
             WHERE k.timestamp >= ? AND k.timestamp < ?",
            start,
            end,
            1,
        )?;
        let (session_count, session_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(armour_cost), SUM(heal_cost), SUM(dangling_cost), \
             SUM(consumable_cost) \
             FROM tracking_sessions WHERE started_at >= ? AND started_at < ?",
            start,
            end,
            4,
        )?;
        let (skill_count, skill_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(ped_value) \
             FROM skill_gains WHERE timestamp >= ? AND timestamp < ?",
            start,
            end,
            1,
        )?;
        let (codex_count, codex_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(ped_value) \
             FROM codex_claims WHERE claimed_at >= ? AND claimed_at < ?",
            start,
            end,
            1,
        )?;
        let (quest_count, quest_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(ped_value) \
             FROM quest_claims WHERE claimed_at >= ? AND claimed_at < ?",
            start,
            end,
            1,
        )?;
        let (harvest_count, harvest_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(loot_total_ped), SUM(cost_ped) \
             FROM harvest_events WHERE timestamp >= ? AND timestamp < ?",
            start,
            end,
            2,
        )?;
        let (quest_item_count, quest_item_sums) = window_sums(
            conn,
            "SELECT COUNT(*), SUM(ri.value_ped) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             WHERE ri.accounting_kind = 'stock' \
               AND c.completed_at >= ? AND c.completed_at < ? \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id) \
               AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = c.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            c.reward_outcome) = 'confirmed'",
            start,
            end,
            1,
        )?;

        families = [
            kill_sums[0],   // loot_tt
            weapon_sums[0], // weapon_cost
            kill_sums[1],   // enhancer_cost
            session_sums[0],
            session_sums[1],
            session_sums[2],
            skill_sums[0],
            codex_sums[0],
            quest_sums[0],
            harvest_sums[0], // harvest_loot_tt
            harvest_sums[1], // harvest_cost
            quest_item_sums[0],
            session_sums[3], // consumable_cost
        ];
        has_rows = kill_count > 0
            || weapon_count > 0
            || session_count > 0
            || skill_count > 0
            || codex_count > 0
            || quest_count > 0
            || harvest_count > 0
            || quest_item_count > 0;
    }

    conn.execute(
        "DELETE FROM daily_ledger_rollups WHERE day = ?",
        rusqlite::params![day],
    )?;
    let ledger_rows = conn.execute(
        "INSERT INTO daily_ledger_rollups (day, entry_type, tag, amount) \
         SELECT date, type, tag, SUM(amount) FROM ledger_entries \
         WHERE date = ? GROUP BY type, tag",
        rusqlite::params![day],
    )?;
    has_rows = has_rows || ledger_rows > 0;

    conn.execute(
        "INSERT OR REPLACE INTO daily_rollups (\
         day, rollup_version, dirty, has_rows, loot_tt, weapon_cost, \
         enhancer_cost, armour_cost, heal_cost, dangling_cost, skill_tt, \
         codex_pes, quest_pes, harvest_loot_tt, harvest_cost, quest_item_tt, consumable_cost, \
         computed_at) \
         VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch('now'))",
        rusqlite::params![
            day,
            ROLLUP_VERSION,
            has_rows,
            families[0],
            families[1],
            families[2],
            families[3],
            families[4],
            families[5],
            families[6],
            families[7],
            families[8],
            families[9],
            families[10],
            families[11],
            families[12],
        ],
    )?;
    Ok(())
}

/// Mark a day's rollup row dirty (minting a stub when absent), so the
/// next heal recomputes it even if the eager recompute never runs. Run
/// inside the transaction that writes the raw rows.
pub fn mark_day_dirty(conn: &rusqlite::Connection, day: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO daily_rollups (day, rollup_version, dirty, has_rows) \
         VALUES (?, 0, 1, 0) \
         ON CONFLICT(day) DO UPDATE SET dirty = 1",
        rusqlite::params![day],
    )?;
    Ok(())
}

/// The write-hook entry: for each distinct day at or before the
/// watermark, mark it dirty and recompute it eagerly. Days after the
/// watermark are served raw and need nothing; before the first heal
/// there is no watermark and the backfill covers everything. The caller
/// owns the surrounding commit semantics.
pub fn refresh_days<I, S>(conn: &rusqlite::Connection, days: I) -> Result<(), DbError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let Some(watermark) = rolled_through(conn)? else {
        return Ok(());
    };
    let mut seen = std::collections::BTreeSet::new();
    for day in days {
        let day = day.as_ref();
        if day <= watermark.as_str() && seen.insert(day.to_string()) {
            mark_day_dirty(conn, day)?;
            recompute_day(conn, day)?;
        }
    }
    Ok(())
}

/// Refresh every day a session's rows touch: its start and end days
/// plus the distinct days of its kills and skill gains (a session can
/// span midnight). The session-stop, orphan-recovery and loot-edit
/// transactions run this after their writes; the auto-generated ledger
/// entries those paths add are not enumerated here because their
/// creators refresh their own date keys (orphan recovery backdates
/// them below the watermark).
pub fn refresh_session_days(conn: &rusqlite::Connection, session_id: &str) -> Result<(), DbError> {
    let days: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT date(timestamp, 'unixepoch') FROM kills WHERE session_id = ? \
             UNION SELECT DISTINCT date(timestamp, 'unixepoch') FROM skill_gains WHERE session_id = ? \
             UNION SELECT DISTINCT date(timestamp, 'unixepoch') FROM harvest_events WHERE session_id = ? \
             UNION SELECT date(started_at, 'unixepoch') FROM tracking_sessions WHERE id = ? \
             UNION SELECT date(ended_at, 'unixepoch') FROM tracking_sessions \
                   WHERE id = ? AND ended_at IS NOT NULL",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![session_id, session_id, session_id, session_id, session_id],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    refresh_days(conn, days)
}

fn rolled_through(conn: &rusqlite::Connection) -> Result<Option<String>, DbError> {
    conn.query_row(
        "SELECT rolled_through FROM daily_rollup_meta WHERE id = 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(DbError::from)
}

/// The earliest calendar day carrying raw data, or None on an empty
/// database. Ledger dates that do not name a canonical day are ignored
/// here; the stray-key sweep in [`heal_rollups`] picks their rows up.
fn earliest_data_day(conn: &rusqlite::Connection) -> Result<Option<NaiveDate>, DbError> {
    let epoch_min: Option<f64> = conn.query_row(
        "SELECT MIN(t) FROM (\
         SELECT MIN(timestamp) AS t FROM kills \
         UNION ALL SELECT MIN(started_at) FROM tracking_sessions \
         UNION ALL SELECT MIN(timestamp) FROM skill_gains \
         UNION ALL SELECT MIN(timestamp) FROM harvest_events \
         UNION ALL SELECT MIN(claimed_at) FROM codex_claims \
         UNION ALL SELECT MIN(claimed_at) FROM quest_claims)",
        [],
        |row| row.get::<_, Option<f64>>(0),
    )?;
    let mut earliest = epoch_min.and_then(|epoch| canonical_day(&epoch_day(epoch)));

    let ledger_days: Vec<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT date FROM ledger_entries")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for date in &ledger_days {
        let Some(date) = canonical_day(date) else {
            continue;
        };
        if earliest.is_none_or(|current| date < current) {
            earliest = Some(date);
        }
    }
    Ok(earliest)
}

/// Bring the rollups current: advance the watermark to yesterday
/// (recomputing every day it crosses, empty days included), repair
/// dirty and below-version rows, and sweep stray ledger date keys into
/// rows of their own. Returns the watermark, the split boundary the
/// reader serves rollups up to. Runs as one transaction; idempotent.
pub fn heal_rollups(conn: &mut rusqlite::Connection, now: f64) -> Result<String, DbError> {
    let tx = conn.transaction()?;

    let today = canonical_day(&epoch_day(now)).expect("epoch_day is canonical");
    let yesterday = today - chrono::Duration::days(1);

    let watermark = match rolled_through(&tx)? {
        Some(day) => day,
        None => {
            // First heal: start the walk just before the earliest data
            // day, or collapse it entirely on an empty database.
            let start = match earliest_data_day(&tx)? {
                Some(earliest) => earliest - chrono::Duration::days(1),
                None => yesterday,
            };
            let day = iso(start.min(yesterday));
            tx.execute(
                "INSERT INTO daily_rollup_meta (id, rolled_through) VALUES (1, ?)",
                rusqlite::params![day],
            )?;
            day
        }
    };

    // Walk the watermark forward to yesterday. A watermark already at or
    // past yesterday (a clock regression) is left alone: the reader
    // serves everything after it raw, so nothing double-counts.
    let mut watermark_date = canonical_day(&watermark).expect("watermark is canonical");
    while watermark_date < yesterday {
        watermark_date += chrono::Duration::days(1);
        recompute_day(&tx, &iso(watermark_date))?;
    }
    let watermark = iso(watermark_date);
    tx.execute(
        "UPDATE daily_rollup_meta SET rolled_through = ? WHERE id = 1",
        rusqlite::params![watermark],
    )?;

    // Repair rows a write hook marked (or a version bump staled).
    let stale: Vec<String> = {
        let mut stmt =
            tx.prepare("SELECT day FROM daily_rollups WHERE dirty = 1 OR rollup_version < ?")?;
        let rows = stmt.query_map(rusqlite::params![ROLLUP_VERSION], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for day in &stale {
        recompute_day(&tx, day)?;
    }

    // Stray ledger date keys (non-canonical spellings) at or before the
    // watermark get rollup rows of their own; later ones stay raw.
    let strays: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT date FROM ledger_entries \
             WHERE date <= ? AND date NOT IN (SELECT day FROM daily_rollups)",
        )?;
        let rows = stmt.query_map(rusqlite::params![watermark], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for day in &strays {
        recompute_day(&tx, day)?;
    }

    tx.commit()?;
    Ok(watermark)
}

/// Drop and regenerate every rollup row: the proof the projection is a
/// pure function of the raw tables.
pub fn rebuild_rollups(conn: &mut rusqlite::Connection, now: f64) -> Result<String, DbError> {
    {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM daily_rollups", [])?;
        tx.execute("DELETE FROM daily_ledger_rollups", [])?;
        tx.execute("DELETE FROM daily_rollup_meta", [])?;
        tx.commit()?;
    }
    heal_rollups(conn, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// Fixed clock: 2001-09-09T01:46:40Z. Today is 2001-09-09; the heal
    /// watermark lands on 2001-09-08.
    const NOW: f64 = 1_000_000_000.0;
    /// Midnight UTC starting each seeded day.
    const DAY_05: f64 = 999_648_000.0; // 2001-09-05
    const DAY_07: f64 = 999_820_800.0; // 2001-09-07
    const DAY_08: f64 = 999_907_200.0; // 2001-09-08
    const DAY_09: f64 = 999_993_600.0; // 2001-09-09 (today)

    /// A day's rollup row as read back for assertions.
    struct DayRollup {
        rollup_version: i64,
        dirty: i64,
        has_rows: i64,
        /// The 12 aggregate families, in column order (index 0 = `loot_tt`,
        /// index 9 = `harvest_loot_tt`, index 10 = `harvest_cost`, index 11 =
        /// `quest_item_tt`). Call sites pass the SQL column index instead;
        /// `family` rebases it by 3.
        families: Vec<Option<f64>>,
    }

    /// A real database over a temp file; the projection functions under
    /// test run on the synchronous core via `db.with_writer`/`with_reader`.
    async fn env() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Heal the rollups on the synchronous core, returning the watermark.
    async fn heal(db: &Db, now: f64) -> String {
        db.with_writer(move |conn| heal_rollups(conn, now))
            .await
            .unwrap()
    }

    async fn run(db: &Db, sql: &str) {
        let sql = sql.to_string();
        db.with_writer(move |conn| {
            conn.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    }

    /// Fetch `(entry_type, tag, amount)` rows for an arbitrary ledger-rollup
    /// query, preserving the caller's exact SQL text.
    async fn ledger_rows(db: &Db, sql: &str) -> Vec<(String, String, f64)> {
        let sql = sql.to_string();
        db.with_reader(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap()
    }

    /// Data across the fixed calendar: a full day (09-05), an empty gap
    /// day (09-06), a NULL-sum day (09-07: only attribute gains, whose
    /// ped_value is NULL), a plain day (09-08, yesterday), today's
    /// in-flight rows (09-09), a swept stray ledger key and an unswept
    /// lexically-greater one.
    async fn seed_calendar(db: &Db) {
        run(
            db,
            &format!(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) \
                 VALUES ('s1', {}, {}, 0, 0.07, 0.11, NULL)",
                DAY_05 + 3600.0,
                DAY_05 + 10_800.0
            ),
        )
        .await;
        run(
            db,
            &format!(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('k1', 's1', 'Atrox', {}, 0.02, 2.0), \
                        ('k2', 's1', 'Atrox', {}, 0.02, NULL), \
                        ('k3', 's1', 'Snable', {}, 0.05, 4.5), \
                        ('k4', 's1', 'Snable', {}, 0.01, 1.5)",
                DAY_05 + 4000.0,
                DAY_05 + 5000.0,
                DAY_08 + 4000.0,
                DAY_09 + 400.0
            ),
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
            &format!(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES ('s1', {}, 'Rifle', 1.0, 0.5), \
                        ('s1', {}, 'Agility', 0.25, NULL), \
                        ('s1', {}, 'Strength', 0.5, NULL)",
                DAY_05 + 4100.0,
                DAY_07 + 100.0,
                DAY_07 + 200.0
            ),
        )
        .await;
        run(
            db,
            &format!(
                "INSERT INTO codex_claims (species_name, rank, skill_name, ped_value, claimed_at) \
                 VALUES ('Atrox', 1, 'Rifle', 1.25, {})",
                DAY_05 + 4200.0
            ),
        )
        .await;
        run(
            db,
            &format!(
                "INSERT INTO quest_claims (quest_name, ped_value, claimed_at) \
                 VALUES ('Iron Atrox', 2.5, {})",
                DAY_05 + 4300.0
            ),
        )
        .await;
        run(
            db,
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) VALUES \
             ('l1', '2001-09-05', 'markup', 'sale', 3.0, 'manual'), \
             ('l2', '2001-09-05', 'markup', 'sale', 2.0, 'manual'), \
             ('l3', '2001-09-05', 'expense', 'repair', 1.0, 'repair'), \
             ('l4', '2001-08-99', 'expense', 'stray but below the watermark', 7.0, 'stray'), \
             ('l5', '2001-9-2', 'markup', 'stray above the watermark, stays raw', 9.0, 'stray')",
        )
        .await;
    }

    async fn rollup_row(db: &Db, day: &str) -> Option<DayRollup> {
        let day = day.to_string();
        db.with_reader(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT rollup_version, dirty, has_rows, loot_tt, weapon_cost, enhancer_cost, \
                     armour_cost, heal_cost, dangling_cost, skill_tt, codex_pes, quest_pes, \
                     harvest_loot_tt, harvest_cost, quest_item_tt \
                     FROM daily_rollups WHERE day = ?1",
                    rusqlite::params![day],
                    |row| {
                        let mut families = Vec::with_capacity(12);
                        for index in 3..=14 {
                            families.push(row.get::<_, Option<f64>>(index)?);
                        }
                        Ok(DayRollup {
                            rollup_version: row.get(0)?,
                            dirty: row.get(1)?,
                            has_rows: row.get(2)?,
                            families,
                        })
                    },
                )
                .optional()?)
        })
        .await
        .unwrap()
    }

    fn family(row: &DayRollup, index: usize) -> Option<f64> {
        row.families[index - 3]
    }

    #[tokio::test]
    async fn epoch_day_matches_sqlite_date_rendering() {
        let (_dir, db) = env().await;
        for epoch in [DAY_05, DAY_09 - 0.1, NOW, NOW + 0.5, DAY_07 + 86_399.0] {
            let sqlite: String = db
                .with_reader(move |conn| {
                    Ok(conn.query_row(
                        "SELECT date(?1, 'unixepoch')",
                        rusqlite::params![epoch],
                        |row| row.get::<_, String>(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(epoch_day(epoch), sqlite, "epoch {epoch}");
        }
    }

    /// Run a recompute on the synchronous core's writer connection.
    async fn recompute(db: &Db, day: &str) {
        let day = day.to_string();
        db.with_writer(move |conn| recompute_day(conn, &day))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn recompute_day_stores_verbatim_sums_and_membership() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;

        // The full day: every family present, NULL dangling preserved.
        recompute(&db, "2001-09-05").await;
        let row = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(row.rollup_version, ROLLUP_VERSION);
        assert_eq!(row.dirty, 0, "not dirty");
        assert_eq!(row.has_rows, 1, "has rows");
        assert_eq!(family(&row, 3), Some(2.0), "loot: SUM skips the NULL");
        assert_eq!(family(&row, 4), Some(1.5), "weapon: 30 shots at 0.05");
        assert_eq!(family(&row, 5), Some(0.04));
        assert_eq!(family(&row, 6), Some(0.07));
        assert_eq!(family(&row, 7), Some(0.11));
        assert_eq!(family(&row, 8), None, "dangling: NULL sum survives");
        assert_eq!(family(&row, 9), Some(0.5));
        assert_eq!(family(&row, 10), Some(1.25));
        assert_eq!(family(&row, 11), Some(2.5));
        let ledger = ledger_rows(
            &db,
            "SELECT entry_type, tag, amount FROM daily_ledger_rollups \
             WHERE day = '2001-09-05' ORDER BY entry_type, tag",
        )
        .await;
        assert_eq!(
            ledger,
            [
                ("expense".into(), "repair".into(), 1.0),
                ("markup".into(), "manual".into(), 5.0),
            ]
        );

        // The empty gap day: an all-NULL row with no membership.
        recompute(&db, "2001-09-06").await;
        let row = rollup_row(&db, "2001-09-06").await.unwrap();
        assert_eq!(row.has_rows, 0, "no rows");
        for index in 3..=13 {
            assert_eq!(family(&row, index), None);
        }

        // The attribute-only day: rows existed, so the day is a member,
        // but the sum over all-NULL ped_value stays NULL.
        recompute(&db, "2001-09-07").await;
        let row = rollup_row(&db, "2001-09-07").await.unwrap();
        assert_eq!(row.has_rows, 1);
        assert_eq!(family(&row, 9), None, "skill_tt: NULL-sum with rows");

        // A stray key: no epoch window, ledger sums only.
        recompute(&db, "2001-08-99").await;
        let row = rollup_row(&db, "2001-08-99").await.unwrap();
        assert_eq!(row.has_rows, 1);
        for index in 3..=13 {
            assert_eq!(family(&row, index), None);
        }
        let stray_ledger = ledger_rows(
            &db,
            "SELECT entry_type, tag, amount FROM daily_ledger_rollups WHERE day = '2001-08-99'",
        )
        .await;
        assert_eq!(stray_ledger, [("expense".into(), "stray".into(), 7.0)]);
    }

    #[tokio::test]
    async fn recompute_replaces_a_days_ledger_rows() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;
        recompute(&db, "2001-09-05").await;

        run(&db, "DELETE FROM ledger_entries WHERE id = 'l3'").await;
        recompute(&db, "2001-09-05").await;
        let ledger = ledger_rows(
            &db,
            "SELECT entry_type, tag, amount FROM daily_ledger_rollups WHERE day = '2001-09-05'",
        )
        .await;
        assert_eq!(
            ledger,
            [("markup".into(), "manual".into(), 5.0)],
            "the deleted expense's row is gone, not stale"
        );
    }

    #[tokio::test]
    async fn heal_backfills_to_yesterday_and_never_today() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;

        let watermark = heal(&db, NOW).await;
        assert_eq!(watermark, "2001-09-08");

        let days: Vec<String> = db
            .with_reader(|conn| {
                let mut stmt = conn.prepare("SELECT day FROM daily_rollups ORDER BY day")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        // The walk covers earliest..yesterday contiguously (the empty
        // 09-06 included); the below-watermark stray is swept; today and
        // the lexically-greater stray are not materialised.
        assert_eq!(
            days,
            [
                "2001-08-99",
                "2001-09-05",
                "2001-09-06",
                "2001-09-07",
                "2001-09-08"
            ]
        );
        let yesterday = rollup_row(&db, "2001-09-08").await.unwrap();
        assert_eq!(family(&yesterday, 3), Some(4.5));

        // Idempotent: a second heal changes nothing.
        let watermark = heal(&db, NOW).await;
        assert_eq!(watermark, "2001-09-08");
        let row = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(family(&row, 3), Some(2.0));
    }

    #[tokio::test]
    async fn heal_repairs_dirty_and_below_version_rows() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;
        heal(&db, NOW).await;

        run(
            &db,
            "UPDATE daily_rollups SET loot_tt = 99.0, dirty = 1 WHERE day = '2001-09-05'",
        )
        .await;
        run(
            &db,
            "UPDATE daily_rollups SET loot_tt = 88.0, rollup_version = 0 WHERE day = '2001-09-08'",
        )
        .await;
        heal(&db, NOW).await;

        let row = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(family(&row, 3), Some(2.0));
        assert_eq!(row.dirty, 0);
        let row = rollup_row(&db, "2001-09-08").await.unwrap();
        assert_eq!(family(&row, 3), Some(4.5));
        assert_eq!(row.rollup_version, ROLLUP_VERSION);
    }

    #[tokio::test]
    async fn a_dirty_stub_from_marking_heals_into_a_full_row() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;
        heal(&db, NOW).await;

        // A crash between the mark and the eager recompute leaves only
        // the stub; the next heal completes it.
        run(&db, "DELETE FROM daily_rollups WHERE day = '2001-09-05'").await;
        db.with_writer(move |conn| mark_day_dirty(conn, "2001-09-05"))
            .await
            .unwrap();
        let stub = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(stub.dirty, 1, "dirty");
        assert_eq!(stub.rollup_version, 0, "pre-version");

        heal(&db, NOW).await;
        let row = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(row.dirty, 0);
        assert_eq!(family(&row, 3), Some(2.0));
    }

    #[tokio::test]
    async fn refresh_days_respects_the_watermark() {
        let (_dir, db) = env().await;

        // Before any heal there is no watermark: a refresh is a no-op.
        seed_calendar(&db).await;
        db.with_writer(move |conn| refresh_days(conn, ["2001-09-05"]))
            .await
            .unwrap();
        assert!(rollup_row(&db, "2001-09-05").await.is_none());

        heal(&db, NOW).await;

        // A backdated ledger write, then the hook: the day recomputes
        // eagerly; today (beyond the watermark) is ignored.
        run(
            &db,
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES ('l9', '2001-09-06', 'expense', 'backdated', 2.5, 'manual')",
        )
        .await;
        db.with_writer(move |conn| refresh_days(conn, ["2001-09-06", "2001-09-09"]))
            .await
            .unwrap();
        let row = rollup_row(&db, "2001-09-06").await.unwrap();
        assert_eq!(row.dirty, 0, "recomputed, clean");
        assert_eq!(row.has_rows, 1, "ledger row joined");
        assert!(rollup_row(&db, "2001-09-09").await.is_none());
    }

    #[tokio::test]
    async fn refresh_session_days_relands_every_day_the_session_touches() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;
        heal(&db, NOW).await;

        // Retroactive edits on two of the session's days, then the hook:
        // both reland; the session's today-side kill day stays unrolled.
        run(&db, "UPDATE kills SET loot_total_ped = 9.5 WHERE id = 'k3'").await;
        run(
            &db,
            "UPDATE tracking_sessions SET armour_cost = 0.5 WHERE id = 's1'",
        )
        .await;
        db.with_writer(move |conn| refresh_session_days(conn, "s1"))
            .await
            .unwrap();
        let row = rollup_row(&db, "2001-09-08").await.unwrap();
        assert_eq!(family(&row, 3), Some(9.5), "the kill day relanded");
        let row = rollup_row(&db, "2001-09-05").await.unwrap();
        assert_eq!(family(&row, 6), Some(0.5), "the start day relanded");
        assert!(rollup_row(&db, "2001-09-09").await.is_none());
    }

    #[tokio::test]
    async fn rebuild_regenerates_identical_content() {
        let (_dir, db) = env().await;
        seed_calendar(&db).await;
        heal(&db, NOW).await;

        type RollupRow = (String, i64, i64, i64, Option<f64>, Option<f64>);
        let snapshot = |db: Db| async move {
            let rollups: Vec<RollupRow> = db
                .with_reader(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT day, rollup_version, dirty, has_rows, loot_tt, skill_tt \
                         FROM daily_rollups ORDER BY day",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<f64>>(4)?,
                            row.get::<_, Option<f64>>(5)?,
                        ))
                    })?;
                    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
                })
                .await
                .unwrap();
            let ledger: Vec<(String, String, String, f64)> = db
                .with_reader(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT day, entry_type, tag, amount FROM daily_ledger_rollups \
                         ORDER BY day, entry_type, tag",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                        ))
                    })?;
                    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
                })
                .await
                .unwrap();
            (rollups, ledger)
        };
        let before = snapshot(db.clone()).await;

        run(
            &db,
            "UPDATE daily_rollups SET loot_tt = 77.0, has_rows = 0 WHERE day = '2001-09-05'",
        )
        .await;
        let watermark = db
            .with_writer(move |conn| rebuild_rollups(conn, NOW))
            .await
            .unwrap();
        assert_eq!(watermark, "2001-09-08");
        assert_eq!(snapshot(db.clone()).await, before);
    }

    #[tokio::test]
    async fn day_range_is_the_canonical_days_epoch_window() {
        // A canonical key maps to its `[midnight, next midnight)` window.
        assert_eq!(day_range("2001-09-05"), Some((DAY_05, DAY_05 + 86_400.0)));
        assert_eq!(day_range("2001-09-09"), Some((DAY_09, DAY_09 + 86_400.0)));
        // A non-canonical spelling and pure junk are both stray: no window.
        assert_eq!(day_range("2001-9-2"), None);
        assert_eq!(day_range("not-a-day"), None);
    }

    #[tokio::test]
    async fn earliest_data_day_prefers_the_smallest_canonical_source() {
        let (_dir, db) = env().await;
        // Epoch-derived data lands on 09-05, but a canonical ledger key
        // sits earlier at 09-01; a stray ledger key is ignored entirely.
        run(
            &db,
            &format!(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('k1', 'ghost', 'Atrox', {}, 0.0, 1.0)",
                DAY_05 + 100.0
            ),
        )
        .await;
        run(
            &db,
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) VALUES \
             ('l1', '2001-09-01', 'markup', 'earlier canonical', 1.0, 'manual'), \
             ('l2', '2001-08-99', 'expense', 'stray, ignored', 1.0, 'stray')",
        )
        .await;
        let earliest = db
            .with_reader(|conn| earliest_data_day(conn))
            .await
            .unwrap();
        assert_eq!(earliest, NaiveDate::from_ymd_opt(2001, 9, 1));
    }

    #[tokio::test]
    async fn recompute_membership_isolates_each_contributing_family() {
        let (_dir, db) = env().await;
        // Days past the seed window, each carrying exactly one family so
        // that day's membership hinges on that single count alone. The weapon
        // family is deliberately absent: `weapon_count` joins
        // `kill_tool_stats` to `kills` and filters on the kill's timestamp, and
        // a tool-stat row has no timestamp of its own, so any counted tool-stat
        // row implies a kill already counted in the same window. It can never
        // be the sole contributor, so it cannot be isolated here.
        const DAY_10: f64 = 1_000_080_000.0; // kill only
        const DAY_11: f64 = 1_000_166_400.0; // session only
        const DAY_12: f64 = 1_000_252_800.0; // codex only
        const DAY_13: f64 = 1_000_339_200.0; // quest only
        const DAY_14: f64 = 1_000_425_600.0; // harvest only

        // A kill with no tool stats, whose session_id names no session row.
        run(
            &db,
            &format!(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('k1', 'ghost', 'Atrox', {}, 0.0, 1.0)",
                DAY_10 + 100.0
            ),
        )
        .await;
        run(
            &db,
            &format!(
                "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('s1', {}, 0)",
                DAY_11 + 100.0
            ),
        )
        .await;
        run(
            &db,
            &format!(
                "INSERT INTO codex_claims (species_name, rank, skill_name, ped_value, claimed_at) \
                 VALUES ('Atrox', 1, 'Rifle', 1.0, {})",
                DAY_12 + 100.0
            ),
        )
        .await;
        run(
            &db,
            &format!(
                "INSERT INTO quest_claims (quest_name, ped_value, claimed_at) VALUES ('Iron', 1.0, {})",
                DAY_13 + 100.0
            ),
        )
        .await;

        // A harvest swing whose session_id names no session row, so the day
        // carries harvest rows and nothing else.
        run(
            &db,
            &format!(
                "INSERT INTO harvest_events \
                 (id, session_id, timestamp, success, cost_ped, loot_total_ped) \
                 VALUES ('h1', 'ghost', {}, 1, 0.5, 1.0)",
                DAY_14 + 100.0
            ),
        )
        .await;

        for day in [
            "2001-09-10",
            "2001-09-11",
            "2001-09-12",
            "2001-09-13",
            "2001-09-14",
        ] {
            recompute(&db, day).await;
            let row = rollup_row(&db, day).await.unwrap();
            assert_eq!(
                row.has_rows, 1,
                "{day}: its single family confers membership"
            );
        }

        // The harvest day's own aggregates land in the two harvest families,
        // which no other test reads: membership alone would pass even if the
        // projection wrote nothing into them.
        let harvest_day = rollup_row(&db, "2001-09-14").await.unwrap();
        assert_eq!(family(&harvest_day, 12), Some(1.0), "harvest_loot_tt");
        assert_eq!(family(&harvest_day, 13), Some(0.5), "harvest_cost");

        // An empty day past them all stays a non-member.
        recompute(&db, "2001-09-15").await;
        assert_eq!(rollup_row(&db, "2001-09-15").await.unwrap().has_rows, 0);
    }
}
