//! Materialised per-session activity rollups: the read model behind the
//! Hunting analytics and stock surfaces, the session-grain sibling of
//! [`crate::daily_rollup`]. Source of truth is the raw tracking tables;
//! rows write eagerly at the mutation points and heal lazily on read.
//!
//! ## The model
//!
//! An ended session's events aggregate to cell sets no activity
//! consumer folds finer than: kill cells by `(context, species, maturity)`
//! (`session_kill_rollups`), active loot cells by `(species, shrapnel,
//! item)` (`session_loot_rollups`, species pre-folded to the empty string
//! for shrapnel rows exactly as the position reads fold it), loot-
//! composition cells by context (`session_context_loot_rollups`),
//! skill-gain cells by context (`session_pes_rollups`), and what each
//! weapon did alone ([`crate::session_weapon_rollup`]). Model-neutral
//! offensive evidence settles by `(context, species, evidence fingerprint)` in
//! `session_offensive_evidence_rollups`, so Community Model replay remains
//! independent of lifetime raw kill history. The raw tables
//! grow with total play history; the cells stay proportional to sessions,
//! species, and items, so a reader folding cells does O(cells) work
//! however long the history gets.
//!
//! `session_rollup_meta` is the settlement boundary, the counterpart of
//! the daily watermark: a session marked at [`ROLLUP_VERSION`] serves from
//! its cells, and every other session (the live one, a freshly edited one,
//! a stale version) serves raw, scoped to its own session id. Readers are
//! therefore correct regardless of heal timing; healing only keeps the
//! raw-served set small. The mutation seams (session stop, orphan
//! recovery, the loot edit flip, session delete) recompute or drop the
//! session's cells inside their own transaction, the same discipline as
//! [`crate::daily_rollup::refresh_session_days`]; a crash between write
//! and commit leaves the marker absent, and the next heal repairs it.

use crate::db::DbError;

/// Bump when a cell's meaning changes: below-version sessions are served
/// raw and heal on the next read.
pub const ROLLUP_VERSION: i64 = 5;

/// Drop one session's cells and marker. The session reads raw from this
/// commit on; the delete path wants exactly that, and [`recompute_session`]
/// starts here so a recompute can never leave mixed generations.
pub fn drop_session(conn: &rusqlite::Connection, session_id: &str) -> Result<(), DbError> {
    // Marker first: if anything below fails mid-way, an unmarked session
    // is merely raw-served, never wrong.
    conn.execute(
        "DELETE FROM session_rollup_meta WHERE session_id = ?",
        rusqlite::params![session_id],
    )?;
    for table in [
        "session_kill_rollups",
        "session_loot_rollups",
        "session_context_loot_rollups",
        "session_pes_rollups",
        "session_offensive_evidence_rollups",
    ]
    .into_iter()
    .chain(crate::session_weapon_rollup::TABLES)
    {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?"),
            rusqlite::params![session_id],
        )?;
    }
    Ok(())
}

/// Recompute one session's cells in place. An unended session settles
/// nothing (it stays raw-served); an ended one gets fresh cells and the
/// marker, in that order. The caller owns the surrounding commit.
pub fn recompute_session(conn: &rusqlite::Connection, session_id: &str) -> Result<(), DbError> {
    use rusqlite::OptionalExtension as _;

    drop_session(conn, session_id)?;
    let ended = conn
        .query_row(
            "SELECT ended_at IS NOT NULL FROM tracking_sessions WHERE id = ?",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !ended {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO session_kill_rollups \
             (session_id, context_id, mob_species, mob_maturity, kills, cycled_ped, loot_tt) \
         SELECT session_id, context_id, \
                COALESCE(mob_species, ''), COALESCE(mob_maturity, ''), \
                COUNT(*), \
                COALESCE(SUM(cost_ped + enhancer_cost), 0), \
                COALESCE(SUM(loot_total_ped), 0) \
         FROM kills WHERE session_id = ?1 \
         GROUP BY 1, 2, 3, 4",
        rusqlite::params![session_id],
    )?;
    // Shrapnel rows fold their species away here, not at read time: every
    // consumer either folds it (positions) or excludes the rows entirely
    // (composition), so the cell grain carries the flag and no species.
    conn.execute(
        "INSERT INTO session_loot_rollups \
             (session_id, mob_species, is_enhancer_shrapnel, item_name, quantity, value_ped) \
         SELECT k.session_id, \
                CASE WHEN li.is_enhancer_shrapnel = 0 THEN COALESCE(k.mob_species, '') \
                     ELSE '' END, \
                li.is_enhancer_shrapnel, li.item_name, \
                SUM(li.quantity), COALESCE(SUM(li.value_ped), 0) \
         FROM kill_loot_items li \
         JOIN kills k ON k.id = li.kill_id \
         WHERE k.session_id = ?1 AND li.deactivated_at IS NULL \
         GROUP BY 1, 2, 3, 4",
        rusqlite::params![session_id],
    )?;
    conn.execute(
        "INSERT INTO session_context_loot_rollups \
             (session_id, context_id, item_name, quantity, value_ped) \
         SELECT k.session_id, k.context_id, li.item_name, \
                SUM(li.quantity), COALESCE(SUM(li.value_ped), 0) \
         FROM kill_loot_items li \
         JOIN kills k ON k.id = li.kill_id \
         WHERE k.session_id = ?1 AND li.deactivated_at IS NULL \
           AND li.is_enhancer_shrapnel = 0 \
         GROUP BY 1, 2, 3",
        rusqlite::params![session_id],
    )?;
    conn.execute(
        "INSERT INTO session_pes_rollups (session_id, context_id, pes) \
         SELECT session_id, context_id, COALESCE(SUM(ped_value), 0) \
         FROM skill_gains \
         WHERE session_id = ?1 AND ped_value IS NOT NULL \
         GROUP BY 1, 2",
        rusqlite::params![session_id],
    )?;
    conn.execute(
        "INSERT INTO session_offensive_evidence_rollups \
             (session_id, mob_species, evidence_fingerprint, expected_economics_json, \
              shots_fired, missing_candidate_raw_tt, missing_basis_phases, context_id) \
         SELECT k.session_id, COALESCE(k.mob_species, ''), ts.evidence_fingerprint, \
                ts.expected_economics_json, \
                SUM(CASE WHEN ts.shots_fired > 0 THEN ts.shots_fired ELSE 0 END), \
                SUM(CASE WHEN ts.expected_economics_json IS NULL \
                              AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                         THEN ts.shots_fired * ts.cost_per_shot ELSE 0 END), \
                SUM(CASE WHEN ts.expected_economics_json IS NULL \
                              AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                         THEN 1 ELSE 0 END), \
                k.context_id \
         FROM kills k CROSS JOIN kill_tool_stats ts \
         WHERE k.session_id = ?1 AND ts.kill_id = k.id \
         GROUP BY k.session_id, k.context_id, 2, ts.evidence_fingerprint, \
                  ts.expected_economics_json",
        rusqlite::params![session_id],
    )?;
    crate::session_weapon_rollup::settle(conn, session_id)?;
    conn.execute(
        "INSERT INTO session_rollup_meta (session_id, rollup_version) VALUES (?, ?)",
        rusqlite::params![session_id, ROLLUP_VERSION],
    )?;
    Ok(())
}

/// Drop every cell and marker and settle the whole history again: the
/// from-scratch rebuild the maintenance surface uses to prove the
/// incremental maintenance never drifts from a clean projection
/// (`maintenance::rebuild_and_verify`, the ADR-0018 rebuildability
/// guarantee).
pub fn rebuild(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "DELETE FROM session_rollup_meta; \
         DELETE FROM session_kill_rollups; \
         DELETE FROM session_loot_rollups; \
         DELETE FROM session_context_loot_rollups; \
         DELETE FROM session_pes_rollups; \
         DELETE FROM session_offensive_evidence_rollups;",
    )?;
    for table in crate::session_weapon_rollup::TABLES {
        conn.execute(&format!("DELETE FROM {table}"), [])?;
    }
    heal(conn)
}

/// The sessions currently served raw: unmarked or below-version. The live
/// session is always here; after a heal it is the only member. Plain
/// `rusqlite` errors so raw read helpers can call it without an error
/// bridge.
pub fn unsettled_sessions(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t.id FROM tracking_sessions t \
         LEFT JOIN session_rollup_meta m \
                ON m.session_id = t.id AND m.rollup_version >= ?1 \
         WHERE m.session_id IS NULL",
    )?;
    let rows = stmt.query_map(rusqlite::params![ROLLUP_VERSION], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
}

/// One effective hunted-loot cell. Settled sessions come from the maintained
/// projection; only explicitly unsettled sessions come from raw facts, scoped
/// by session id. This is the single read boundary for consumers whose grain
/// is no finer than session, definition, species, shrapnel class and item.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EffectiveHuntingLootCell {
    pub session_id: String,
    pub definition_id: Option<i64>,
    pub mob_species: String,
    pub is_enhancer_shrapnel: bool,
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

const SETTLED_HUNTING_LOOT_SQL: &str = "SELECT r.session_id, s.definition_id, r.mob_species, \
            r.is_enhancer_shrapnel, r.item_name, r.quantity, r.value_ped \
     FROM session_loot_rollups r \
     JOIN session_rollup_meta m ON m.session_id = r.session_id \
          AND m.rollup_version >= ?2 \
     JOIN tracking_sessions s ON s.id = r.session_id \
     WHERE (?1 IS NULL OR s.started_at >= ?1)";

const UNSETTLED_HUNTING_LOOT_SQL: &str = "SELECT s.definition_id, \
            CASE WHEN li.is_enhancer_shrapnel = 0 THEN COALESCE(k.mob_species, '') \
                 ELSE '' END, \
            li.is_enhancer_shrapnel, li.item_name, \
            SUM(li.quantity), COALESCE(SUM(li.value_ped), 0) \
     FROM tracking_sessions s CROSS JOIN kills k CROSS JOIN kill_loot_items li \
     WHERE s.id = ?1 AND k.session_id = s.id AND li.kill_id = k.id \
       AND (?2 IS NULL OR s.started_at >= ?2) \
       AND li.deactivated_at IS NULL \
     GROUP BY s.definition_id, 2, li.is_enhancer_shrapnel, li.item_name";

/// Read effective hunted-loot cells for sessions starting inside the optional
/// period boundary. A fully settled request does not even prepare a statement
/// against the raw fact tables, which makes the access path authorisable in
/// tests rather than merely fast by convention.
pub(crate) fn effective_hunting_loot_cells(
    conn: &rusqlite::Connection,
    epoch_start: Option<f64>,
) -> rusqlite::Result<Vec<EffectiveHuntingLootCell>> {
    let mut cells = Vec::new();
    {
        let mut stmt = conn.prepare(SETTLED_HUNTING_LOOT_SQL)?;
        let mut rows = stmt.query(rusqlite::params![epoch_start, ROLLUP_VERSION])?;
        while let Some(row) = rows.next()? {
            cells.push(EffectiveHuntingLootCell {
                session_id: row.get(0)?,
                definition_id: row.get(1)?,
                mob_species: row.get(2)?,
                is_enhancer_shrapnel: row.get(3)?,
                item_name: row.get(4)?,
                quantity: row.get::<_, i64>(5).unwrap_or(0),
                value_ped: row.get::<_, f64>(6).unwrap_or(0.0),
            });
        }
    }

    let unsettled = unsettled_sessions(conn)?;
    if unsettled.is_empty() {
        return Ok(cells);
    }

    let mut stmt = conn.prepare(UNSETTLED_HUNTING_LOOT_SQL)?;
    for session_id in unsettled {
        let mut rows = stmt.query(rusqlite::params![session_id, epoch_start])?;
        while let Some(row) = rows.next()? {
            cells.push(EffectiveHuntingLootCell {
                session_id: session_id.clone(),
                definition_id: row.get(0)?,
                mob_species: row.get(1)?,
                is_enhancer_shrapnel: row.get(2)?,
                item_name: row.get(3)?,
                quantity: row.get::<_, i64>(4).unwrap_or(0),
                value_ped: row.get::<_, f64>(5).unwrap_or(0.0),
            });
        }
    }
    Ok(cells)
}

/// One model-neutral offensive-evidence cell. Settled sessions come from the
/// maintained projection; only explicitly unsettled sessions consult raw tool
/// phases, scoped through their session id. Keeping the original evidence JSON
/// makes the projection independent of the Community Model version that later
/// evaluates it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EffectiveOffensiveEvidenceCell {
    pub session_id: String,
    pub context_id: Option<i64>,
    pub mob_species: String,
    pub expected_economics_json: Option<String>,
    pub shots_fired: i64,
    pub missing_candidate_raw_tt: f64,
    pub missing_basis_phases: i64,
}

const SETTLED_OFFENSIVE_EVIDENCE_SQL: &str =
    "SELECT r.session_id, r.context_id, r.mob_species, r.expected_economics_json, \
            r.shots_fired, r.missing_candidate_raw_tt, r.missing_basis_phases \
     FROM session_offensive_evidence_rollups r \
     JOIN session_rollup_meta m ON m.session_id = r.session_id \
          AND m.rollup_version >= ?2 \
     JOIN tracking_sessions s ON s.id = r.session_id \
     WHERE (?1 IS NULL OR s.started_at >= ?1)";

const UNSETTLED_OFFENSIVE_EVIDENCE_SQL: &str =
    "SELECT k.context_id, COALESCE(k.mob_species, ''), ts.expected_economics_json, \
            SUM(CASE WHEN ts.shots_fired > 0 THEN ts.shots_fired ELSE 0 END), \
            SUM(CASE WHEN ts.expected_economics_json IS NULL \
                          AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                     THEN ts.shots_fired * ts.cost_per_shot ELSE 0 END), \
            SUM(CASE WHEN ts.expected_economics_json IS NULL \
                          AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                     THEN 1 ELSE 0 END) \
     FROM tracking_sessions s CROSS JOIN kills k CROSS JOIN kill_tool_stats ts \
     WHERE s.id = ?1 AND k.session_id = s.id AND ts.kill_id = k.id \
       AND (?2 IS NULL OR s.started_at >= ?2) \
     GROUP BY k.context_id, 2, ts.evidence_fingerprint, ts.expected_economics_json";

/// Read model-neutral offensive evidence inside the optional period boundary.
/// A fully settled request prepares no statement against `kills` or
/// `kill_tool_stats`; the raw leg exists only for the live or invalidated edge.
pub(crate) fn effective_offensive_evidence_cells(
    conn: &rusqlite::Connection,
    epoch_start: Option<f64>,
) -> rusqlite::Result<Vec<EffectiveOffensiveEvidenceCell>> {
    let mut cells = Vec::new();
    {
        let mut stmt = conn.prepare(SETTLED_OFFENSIVE_EVIDENCE_SQL)?;
        let mut rows = stmt.query(rusqlite::params![epoch_start, ROLLUP_VERSION])?;
        while let Some(row) = rows.next()? {
            cells.push(EffectiveOffensiveEvidenceCell {
                session_id: row.get(0)?,
                context_id: row.get(1)?,
                mob_species: row.get(2)?,
                expected_economics_json: row.get(3)?,
                shots_fired: row.get::<_, i64>(4).unwrap_or(0),
                missing_candidate_raw_tt: row.get::<_, f64>(5).unwrap_or(0.0),
                missing_basis_phases: row.get::<_, i64>(6).unwrap_or(0),
            });
        }
    }

    let unsettled = unsettled_sessions(conn)?;
    if unsettled.is_empty() {
        return Ok(cells);
    }

    let mut stmt = conn.prepare(UNSETTLED_OFFENSIVE_EVIDENCE_SQL)?;
    for session_id in unsettled {
        let mut rows = stmt.query(rusqlite::params![session_id, epoch_start])?;
        while let Some(row) = rows.next()? {
            cells.push(EffectiveOffensiveEvidenceCell {
                session_id: session_id.clone(),
                context_id: row.get(0)?,
                mob_species: row.get(1)?,
                expected_economics_json: row.get(2)?,
                shots_fired: row.get::<_, i64>(3).unwrap_or(0),
                missing_candidate_raw_tt: row.get::<_, f64>(4).unwrap_or(0.0),
                missing_basis_phases: row.get::<_, i64>(5).unwrap_or(0),
            });
        }
    }
    Ok(cells)
}

/// Bring the rollups current: settle every ended session still served
/// raw. The first call after the migration backfills the whole history;
/// steady-state calls find nothing to do beyond the marker scan. Callers
/// run this on the write connection before an activity read, exactly as
/// the Overview heals the daily rollups.
///
/// Settlement is set-based (one grouped pass per raw table over the whole
/// pending set) rather than a [`recompute_session`] loop: for the
/// backfill case that is the difference between one sequential pass over
/// each table and a random-access walk per session, and for the
/// steady-state case (one or two pending sessions) the named join order
/// makes it the same indexed work.
pub fn heal(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracking_sessions t \
         LEFT JOIN session_rollup_meta m \
                ON m.session_id = t.id AND m.rollup_version >= ?1 \
         WHERE m.session_id IS NULL AND t.ended_at IS NOT NULL",
        rusqlite::params![ROLLUP_VERSION],
        |row| row.get(0),
    )?;
    let orphaned: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session_rollup_meta \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
        |row| row.get(0),
    )?;
    if pending == 0 && orphaned == 0 {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS pending_settlement (id TEXT PRIMARY KEY); \
         DELETE FROM pending_settlement;",
    )?;
    tx.execute(
        "INSERT INTO pending_settlement (id) \
         SELECT t.id FROM tracking_sessions t \
         LEFT JOIN session_rollup_meta m \
                ON m.session_id = t.id AND m.rollup_version >= ?1 \
         WHERE m.session_id IS NULL AND t.ended_at IS NOT NULL",
        rusqlite::params![ROLLUP_VERSION],
    )?;
    // Markers first, then any stale-version cells, so a failure part-way
    // leaves the pending sessions merely unmarked and raw-served.
    for table in [
        "session_rollup_meta",
        "session_kill_rollups",
        "session_loot_rollups",
        "session_context_loot_rollups",
        "session_pes_rollups",
        "session_offensive_evidence_rollups",
    ]
    .into_iter()
    .chain(crate::session_weapon_rollup::TABLES)
    {
        tx.execute(
            &format!(
                "DELETE FROM {table} \
                 WHERE session_id IN (SELECT id FROM pending_settlement)"
            ),
            [],
        )?;
    }
    // CROSS JOIN names the join order (pending ids through the session
    // and kill indexes); a planner-chosen order could re-scan the whole
    // raw table for a single pending session.
    tx.execute(
        "INSERT INTO session_kill_rollups \
             (session_id, context_id, mob_species, mob_maturity, kills, cycled_ped, loot_tt) \
         SELECT p.id, k.context_id, \
                COALESCE(k.mob_species, ''), COALESCE(k.mob_maturity, ''), \
                COUNT(*), \
                COALESCE(SUM(k.cost_ped + k.enhancer_cost), 0), \
                COALESCE(SUM(k.loot_total_ped), 0) \
         FROM pending_settlement p CROSS JOIN kills k \
         WHERE k.session_id = p.id \
         GROUP BY 1, 2, 3, 4",
        [],
    )?;
    tx.execute(
        "INSERT INTO session_loot_rollups \
             (session_id, mob_species, is_enhancer_shrapnel, item_name, quantity, value_ped) \
         SELECT p.id, \
                CASE WHEN li.is_enhancer_shrapnel = 0 THEN COALESCE(k.mob_species, '') \
                     ELSE '' END, \
                li.is_enhancer_shrapnel, li.item_name, \
                SUM(li.quantity), COALESCE(SUM(li.value_ped), 0) \
         FROM pending_settlement p CROSS JOIN kills k CROSS JOIN kill_loot_items li \
         WHERE k.session_id = p.id AND li.kill_id = k.id AND li.deactivated_at IS NULL \
         GROUP BY 1, 2, 3, 4",
        [],
    )?;
    tx.execute(
        "INSERT INTO session_context_loot_rollups \
             (session_id, context_id, item_name, quantity, value_ped) \
         SELECT p.id, k.context_id, li.item_name, \
                SUM(li.quantity), COALESCE(SUM(li.value_ped), 0) \
         FROM pending_settlement p CROSS JOIN kills k CROSS JOIN kill_loot_items li \
         WHERE k.session_id = p.id AND li.kill_id = k.id \
           AND li.deactivated_at IS NULL AND li.is_enhancer_shrapnel = 0 \
         GROUP BY 1, 2, 3",
        [],
    )?;
    tx.execute(
        "INSERT INTO session_pes_rollups (session_id, context_id, pes) \
         SELECT p.id, sg.context_id, COALESCE(SUM(sg.ped_value), 0) \
         FROM pending_settlement p CROSS JOIN skill_gains sg \
         WHERE sg.session_id = p.id AND sg.ped_value IS NOT NULL \
         GROUP BY 1, 2",
        [],
    )?;
    tx.execute(
        "INSERT INTO session_offensive_evidence_rollups \
             (session_id, mob_species, evidence_fingerprint, expected_economics_json, \
              shots_fired, missing_candidate_raw_tt, missing_basis_phases, context_id) \
         SELECT p.id, COALESCE(k.mob_species, ''), ts.evidence_fingerprint, \
                ts.expected_economics_json, \
                SUM(CASE WHEN ts.shots_fired > 0 THEN ts.shots_fired ELSE 0 END), \
                SUM(CASE WHEN ts.expected_economics_json IS NULL \
                              AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                         THEN ts.shots_fired * ts.cost_per_shot ELSE 0 END), \
                SUM(CASE WHEN ts.expected_economics_json IS NULL \
                              AND ts.shots_fired > 0 AND ts.cost_per_shot > 0 \
                         THEN 1 ELSE 0 END), \
                k.context_id \
         FROM pending_settlement p CROSS JOIN kills k CROSS JOIN kill_tool_stats ts \
         WHERE k.session_id = p.id AND ts.kill_id = k.id \
         GROUP BY p.id, k.context_id, 2, ts.evidence_fingerprint, \
                  ts.expected_economics_json",
        [],
    )?;
    // The weapon cells attribute each skill gain to the next event in its
    // session, an ordered walk no grouped pass expresses, so they settle
    // one pending session at a time over the per-session indexes.
    let pending_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM pending_settlement")?;
        let ids = stmt.query_map([], |row| row.get(0))?;
        ids.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for session_id in &pending_ids {
        crate::session_weapon_rollup::settle(&tx, session_id)?;
    }
    tx.execute(
        "INSERT INTO session_rollup_meta (session_id, rollup_version) \
         SELECT id, ?1 FROM pending_settlement",
        rusqlite::params![ROLLUP_VERSION],
    )?;
    tx.execute("DELETE FROM pending_settlement", [])?;
    // Cells for sessions the raw tables no longer know (an external bulk
    // copy, a partial restore) would ghost into the folds; sweep them with
    // the same transaction that settles the backlog.
    tx.execute(
        "DELETE FROM session_kill_rollups \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    tx.execute(
        "DELETE FROM session_loot_rollups \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    tx.execute(
        "DELETE FROM session_context_loot_rollups \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    tx.execute(
        "DELETE FROM session_pes_rollups \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    tx.execute(
        "DELETE FROM session_offensive_evidence_rollups \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    for table in crate::session_weapon_rollup::TABLES {
        tx.execute(
            &format!(
                "DELETE FROM {table} \
                 WHERE session_id NOT IN (SELECT id FROM tracking_sessions)"
            ),
            [],
        )?;
    }
    tx.execute(
        "DELETE FROM session_rollup_meta \
         WHERE session_id NOT IN (SELECT id FROM tracking_sessions)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::collections::BTreeMap;

    async fn open_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("test.db")).await.expect("open");
        (dir, db)
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, ended: bool) {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES (?, 1000.0, ?, 0)",
            rusqlite::params![id, ended.then_some(5000.0)],
        )
        .expect("session");
    }

    fn seed_kill(conn: &rusqlite::Connection, id: &str, session: &str, species: &str) {
        conn.execute(
            "INSERT INTO kills (id, session_id, timestamp, mob_name, mob_species, \
                                mob_maturity, cost_ped, enhancer_cost, loot_total_ped) \
             VALUES (?, ?, 2000.0, ?, ?, 'Old', 1.5, 0.5, 3.25)",
            rusqlite::params![id, session, species, species],
        )
        .expect("kill");
        conn.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
                                          is_enhancer_shrapnel) \
             VALUES (?, 'Animal Hide', 2, 1.75, 0), (?, 'Shrapnel', 100, 0.01, 1)",
            rusqlite::params![id, id],
        )
        .expect("loot");
        conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES (?, 2000.0, 'Rifle', 0.5, 0.0421)",
            rusqlite::params![session],
        )
        .expect("gain");
    }

    fn seed_tool_phase(
        conn: &rusqlite::Connection,
        kill_id: &str,
        tool_name: &str,
        shots: i64,
        cost_per_shot: f64,
        evidence: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO kill_tool_stats \
             (kill_id, tool_name, shots_fired, cost_per_shot, \
              expected_economics_json, evidence_fingerprint) \
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                kill_id,
                tool_name,
                shots,
                cost_per_shot,
                evidence,
                evidence.unwrap_or("")
            ],
        )
        .expect("tool phase");
    }

    fn stamp_kill_context(
        conn: &rusqlite::Connection,
        kill_id: &str,
        session_id: &str,
        created_at: f64,
    ) -> i64 {
        conn.execute(
            "INSERT INTO session_contexts(session_id, created_at) VALUES (?, ?)",
            rusqlite::params![session_id, created_at],
        )
        .expect("context");
        let context_id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE kills SET context_id = ? WHERE id = ?",
            rusqlite::params![context_id, kill_id],
        )
        .expect("context stamp");
        context_id
    }

    #[tokio::test]
    async fn ended_sessions_settle_and_live_ones_stay_raw() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "s-ended", true);
            seed_session(conn, "s-live", false);
            seed_kill(conn, "k1", "s-ended", "Atrox");
            seed_kill(conn, "k2", "s-live", "Daikiba");
            heal(conn)?;

            let unsettled = unsettled_sessions(conn)?;
            assert_eq!(unsettled, vec!["s-live".to_string()]);

            let (kills, cycled, loot): (i64, f64, f64) = conn.query_row(
                "SELECT kills, cycled_ped, loot_tt FROM session_kill_rollups \
                 WHERE session_id = 's-ended'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(kills, 1);
            assert!((cycled - 2.0).abs() < 1e-9);
            assert!((loot - 3.25).abs() < 1e-9);

            // Shrapnel folds its species away and keeps its flag.
            let shrapnel_species: String = conn.query_row(
                "SELECT mob_species FROM session_loot_rollups \
                 WHERE session_id = 's-ended' AND is_enhancer_shrapnel = 1",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(shrapnel_species, "");

            let live_cells: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_kill_rollups WHERE session_id = 's-live'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(live_cells, 0);
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn recompute_reflects_edits_and_deactivations() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "s-1", true);
            seed_kill(conn, "k1", "s-1", "Atrox");
            heal(conn)?;

            conn.execute(
                "UPDATE kill_loot_items SET deactivated_at = 9000.0 \
                 WHERE kill_id = 'k1' AND item_name = 'Animal Hide'",
                [],
            )?;
            recompute_session(conn, "s-1")?;

            let hide_cells: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_loot_rollups \
                 WHERE session_id = 's-1' AND item_name = 'Animal Hide'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(hide_cells, 0);
            let marked: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_rollup_meta WHERE session_id = 's-1'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(marked, 1);
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn heal_sweeps_cells_of_vanished_sessions() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "s-gone", true);
            seed_kill(conn, "k1", "s-gone", "Atrox");
            heal(conn)?;
            // Simulate an external bulk copy that dropped the session but
            // left its cells behind.
            conn.execute("DELETE FROM kills WHERE session_id = 's-gone'", [])?;
            conn.execute("DELETE FROM tracking_sessions WHERE id = 's-gone'", [])?;
            heal(conn)?;

            let ghost: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_kill_rollups WHERE session_id = 's-gone'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(ghost, 0);
            let marker: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_rollup_meta WHERE session_id = 's-gone'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(marker, 0);
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn version_bump_re_serves_raw_until_healed() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "s-1", true);
            seed_kill(conn, "k1", "s-1", "Atrox");
            heal(conn)?;
            conn.execute("UPDATE session_rollup_meta SET rollup_version = 0", [])?;
            let unsettled = unsettled_sessions(conn)?;
            assert_eq!(unsettled, vec!["s-1".to_string()]);
            heal(conn)?;
            assert!(unsettled_sessions(conn)?.is_empty());
            Ok(())
        })
        .await
        .expect("writer");
    }

    type CellKey = (String, String, String, bool);
    type CellValue = (Option<i64>, i64, i64);

    fn cell_map(cells: Vec<EffectiveHuntingLootCell>) -> BTreeMap<CellKey, CellValue> {
        cells
            .into_iter()
            .map(|cell| {
                (
                    (
                        cell.session_id,
                        cell.mob_species,
                        cell.item_name,
                        cell.is_enhancer_shrapnel,
                    ),
                    (
                        cell.definition_id,
                        cell.quantity,
                        (cell.value_ped * 10_000.0).round() as i64,
                    ),
                )
            })
            .collect()
    }

    type EvidenceCellKey = (String, Option<i64>, String, Option<String>);
    type EvidenceCellValue = (i64, i64, i64);

    fn evidence_cell_map(
        cells: Vec<EffectiveOffensiveEvidenceCell>,
    ) -> BTreeMap<EvidenceCellKey, EvidenceCellValue> {
        cells
            .into_iter()
            .map(|cell| {
                (
                    (
                        cell.session_id,
                        cell.context_id,
                        cell.mob_species,
                        cell.expected_economics_json,
                    ),
                    (
                        cell.shots_fired,
                        (cell.missing_candidate_raw_tt * 10_000.0).round() as i64,
                        cell.missing_basis_phases,
                    ),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn effective_loot_cells_match_raw_and_settled_with_a_live_edge() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "ended", true);
            seed_session(conn, "live", false);
            seed_kill(conn, "ended-kill", "ended", "Atrox");
            seed_kill(conn, "live-kill", "live", "Daikiba");

            let raw = cell_map(effective_hunting_loot_cells(conn, None)?);
            heal(conn)?;
            let mixed = cell_map(effective_hunting_loot_cells(conn, None)?);
            assert_eq!(raw, mixed);

            // A raw edit to the settled session cannot leak through the live
            // edge. The live session remains immediate; the ended session
            // changes only after its ordinary invalidation/recompute seam.
            conn.execute(
                "UPDATE kill_loot_items SET value_ped = 99.0 WHERE kill_id = 'ended-kill'",
                [],
            )?;
            conn.execute(
                "UPDATE kill_loot_items SET value_ped = 4.0 WHERE kill_id = 'live-kill' \
                 AND item_name = 'Animal Hide'",
                [],
            )?;
            let after_raw_edits = cell_map(effective_hunting_loot_cells(conn, None)?);
            assert_eq!(
                after_raw_edits[&(
                    "ended".to_string(),
                    "Atrox".to_string(),
                    "Animal Hide".to_string(),
                    false,
                )]
                    .2,
                17_500
            );
            assert_eq!(
                after_raw_edits[&(
                    "live".to_string(),
                    "Daikiba".to_string(),
                    "Animal Hide".to_string(),
                    false,
                )]
                    .2,
                40_000
            );

            recompute_session(conn, "ended")?;
            let recomputed = cell_map(effective_hunting_loot_cells(conn, None)?);
            assert_eq!(
                recomputed[&(
                    "ended".to_string(),
                    "Atrox".to_string(),
                    "Animal Hide".to_string(),
                    false,
                )]
                    .2,
                990_000
            );
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn offensive_evidence_cells_match_raw_and_settled_with_a_live_edge() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "ended", true);
            seed_session(conn, "live", false);
            seed_kill(conn, "ended-kill", "ended", "Atrox");
            seed_kill(conn, "live-kill", "live", "Daikiba");
            seed_tool_phase(
                conn,
                "ended-kill",
                "Settled Rifle",
                10,
                0.04,
                Some("{\"basis\":\"settled\"}"),
            );
            seed_tool_phase(conn, "live-kill", "Legacy Rifle", 4, 0.05, None);

            let raw = evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            heal(conn)?;
            let mixed = evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            assert_eq!(raw, mixed);

            conn.execute(
                "UPDATE kill_tool_stats SET shots_fired = 99 WHERE kill_id = 'ended-kill'",
                [],
            )?;
            conn.execute(
                "UPDATE kill_tool_stats SET shots_fired = 8 WHERE kill_id = 'live-kill'",
                [],
            )?;
            let after_raw_edits =
                evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            assert_eq!(
                after_raw_edits[&(
                    "ended".to_string(),
                    None,
                    "Atrox".to_string(),
                    Some("{\"basis\":\"settled\"}".to_string()),
                )]
                    .0,
                10
            );
            assert_eq!(
                after_raw_edits[&("live".to_string(), None, "Daikiba".to_string(), None)],
                (8, 4_000, 1)
            );

            recompute_session(conn, "ended")?;
            let recomputed = evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            assert_eq!(
                recomputed[&(
                    "ended".to_string(),
                    None,
                    "Atrox".to_string(),
                    Some("{\"basis\":\"settled\"}".to_string()),
                )]
                    .0,
                99
            );
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn offensive_evidence_cells_preserve_activity_context_grain() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "ended", true);
            seed_kill(conn, "kill-a", "ended", "Atrox");
            seed_kill(conn, "kill-b", "ended", "Atrox");
            let context_a = stamp_kill_context(conn, "kill-a", "ended", 1500.0);
            let context_b = stamp_kill_context(conn, "kill-b", "ended", 2500.0);
            seed_tool_phase(conn, "kill-a", "Shared Rifle", 10, 0.04, Some("evidence"));
            seed_tool_phase(conn, "kill-b", "Shared Rifle", 20, 0.04, Some("evidence"));

            let raw = evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            heal(conn)?;
            let settled = evidence_cell_map(effective_offensive_evidence_cells(conn, None)?);
            assert_eq!(raw, settled);
            assert_eq!(
                settled[&(
                    "ended".to_string(),
                    Some(context_a),
                    "Atrox".to_string(),
                    Some("evidence".to_string()),
                )]
                    .0,
                10
            );
            assert_eq!(
                settled[&(
                    "ended".to_string(),
                    Some(context_b),
                    "Atrox".to_string(),
                    Some("evidence".to_string()),
                )]
                    .0,
                20
            );
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[tokio::test]
    async fn effective_loot_query_plans_are_projection_bounded_and_session_scoped() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "ended", true);
            seed_session(conn, "live", false);
            seed_kill(conn, "ended-kill", "ended", "Atrox");
            seed_kill(conn, "live-kill", "live", "Daikiba");
            seed_tool_phase(
                conn,
                "ended-kill",
                "Settled Rifle",
                10,
                0.04,
                Some("{\"basis\":\"settled\"}"),
            );
            seed_tool_phase(conn, "live-kill", "Legacy Rifle", 4, 0.05, None);
            heal(conn)?;

            let plan = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
                let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let rows = stmt.query_map(params, |row| row.get::<_, String>(3))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            };
            let settled = plan(
                SETTLED_HUNTING_LOOT_SQL,
                &[&Option::<f64>::None, &ROLLUP_VERSION],
            )?;
            // SQLite may prefer a table scan for this two-row fixture. The
            // invariant here is that the settled leg is projection-only; the
            // representative-database benchmark records the large-table plan.
            assert!(settled
                .iter()
                .any(|step| step.contains("SCAN r") || step.contains("SEARCH r")));
            assert!(settled.iter().all(|step| {
                !step.contains("kill_loot_items")
                    && !step.contains("SCAN k")
                    && !step.contains("SCAN li")
            }));

            let live_id = "live";
            let raw = plan(
                UNSETTLED_HUNTING_LOOT_SQL,
                &[&live_id, &Option::<f64>::None],
            )?;
            assert!(raw.iter().any(|step| step.contains("idx_kill_session")));
            assert!(raw
                .iter()
                .any(|step| step.contains("idx_kill_loot_items_kill_id")));
            assert!(raw
                .iter()
                .all(|step| !step.contains("SCAN k") && !step.contains("SCAN li")));

            let settled_evidence = plan(
                SETTLED_OFFENSIVE_EVIDENCE_SQL,
                &[&Option::<f64>::None, &ROLLUP_VERSION],
            )?;
            assert!(settled_evidence
                .iter()
                .any(|step| step.contains("SCAN r") || step.contains("SEARCH r")));
            assert!(settled_evidence.iter().all(|step| {
                !step.contains("kill_tool_stats")
                    && !step.contains("SCAN k")
                    && !step.contains("SCAN ts")
            }));

            let raw_evidence = plan(
                UNSETTLED_OFFENSIVE_EVIDENCE_SQL,
                &[&live_id, &Option::<f64>::None],
            )?;
            assert!(raw_evidence
                .iter()
                .any(|step| step.contains("idx_kill_session")));
            assert!(raw_evidence
                .iter()
                .any(|step| step.contains("idx_kill_tool_stats_covering")));
            assert!(raw_evidence
                .iter()
                .all(|step| !step.contains("SCAN k") && !step.contains("SCAN ts")));
            Ok(())
        })
        .await
        .expect("writer");
    }

    #[test]
    fn raw_hunting_loot_reads_stay_behind_the_effective_boundary() {
        let analytics = include_str!("analytics.rs");
        let positions = analytics
            .split_once("fn all_item_positions(")
            .expect("position reader")
            .1
            .split_once("fn as_source_positions(")
            .expect("position reader end")
            .0;
        assert!(!positions.contains("kill_loot_items"));
        assert!(!positions.contains("FROM kills"));
        assert!(positions.contains("EffectiveHuntingLootCell"));

        let expected = analytics
            .split_once("fn hunting_expected_aggregates(")
            .expect("expected reader")
            .1
            .split_once("#[allow(clippy::too_many_lines)]")
            .expect("expected reader end")
            .0;
        assert!(!expected.contains("kill_tool_stats"));
        assert!(!expected.contains("FROM kills"));
        assert!(expected.contains("effective_offensive_evidence_cells"));

        let market = include_str!("market_service.rs")
            .split_once("#[cfg(test)]")
            .expect("market production/test boundary")
            .0;
        assert!(!market.contains("kill_loot_items"));
        assert!(!market.contains("FROM kills"));

        let boundary = include_str!("session_rollup.rs")
            .split_once("#[cfg(test)]")
            .expect("production/test boundary")
            .0;
        assert!(boundary.contains("FROM tracking_sessions s CROSS JOIN kills k"));
        assert!(boundary.contains("WHERE s.id = ?1 AND k.session_id = s.id"));
        assert!(boundary.contains("session_offensive_evidence_rollups"));
        assert!(boundary.contains("AND ts.kill_id = k.id"));
    }
}
