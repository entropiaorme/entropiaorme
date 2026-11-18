//! Session stats exclusion: which recorded sessions count towards the
//! app-wide statistics.
//!
//! A session can be excluded from one accounting scope while staying on
//! record. Its own list row and detail keep every figure (that is the point
//! of keeping it), but no cross-session aggregate of that scope may see it.
//!
//! ## The invariant
//!
//! **A session excluded from a scope is indistinguishable, in every
//! cross-session figure of that scope, from a session that was never
//! recorded.** The facade suite proves it by comparing every aggregate
//! command against a database that never held the session.
//!
//! * [`StatsScope::Economy`] carries the PED side: loot and harvest TT,
//!   every cost bucket, the kills, time, and pace the activity surfaces
//!   use as evidence, the expected-return evidence, protection cost
//!   allocations, the session-derived ledger entries (`source_session_id`),
//!   and the activity-attributed markup of sales drawn from its loot.
//! * [`StatsScope::Progression`] carries the PES of its skill gains.
//!
//! A ratio counts a session only when every scope its terms draw on
//! includes it (PES per PED cycled needs both), so its numerator and
//! denominator always describe the same sessions.
//!
//! Deliberately outside both scopes: skill levels (calibrations, never
//! derived from gains), physical stock holdings, confirmed transactions in
//! the global ledger, quest completions and their rewards, and codex.
//!
//! Every aggregate reader filters through the helpers here rather than
//! spelling its own predicate, so the scope's meaning lives in one place.

use std::collections::HashSet;

/// One independently excludable accounting scope of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatsScope {
    /// The PED side: loot, costs, activity evidence, derived ledger entries.
    Economy,
    /// The PES side: skill gains.
    Progression,
}

impl StatsScope {
    /// Both scopes, in display order.
    pub const ALL: [StatsScope; 2] = [StatsScope::Economy, StatsScope::Progression];

    /// The `tracking_sessions` column stamping this scope's exclusion.
    pub fn column(self) -> &'static str {
        match self {
            StatsScope::Economy => "economy_excluded_at",
            StatsScope::Progression => "progression_excluded_at",
        }
    }
}

/// SQL predicate: the session named by `session_expr` counts in `scope`.
///
/// Correlated on the session id rather than joined, so it reads correctly
/// for rows whose session has no `tracking_sessions` row (they count, as
/// they always have) and never multiplies rows. `NOT EXISTS` rather than
/// `NOT IN`, which would silently drop a NULL session id.
pub fn counts(scope: StatsScope, session_expr: &str) -> String {
    let column = scope.column();
    format!(
        "NOT EXISTS (SELECT 1 FROM tracking_sessions xs_{column} \
         WHERE xs_{column}.id = {session_expr} AND xs_{column}.{column} IS NOT NULL)"
    )
}

/// SQL predicate: the session named by `session_expr` is excluded from
/// `scope`. The complement of [`counts`], for a reader that must keep an
/// excluded row in a denominator while dropping it from the credit (a sale
/// share drawn from an excluded session's loot stays unclaimed).
pub fn excluded(scope: StatsScope, session_expr: &str) -> String {
    format!("NOT {}", counts(scope, session_expr))
}

/// SQL expression naming the session a stock movement's activity context
/// belongs to (NULL for a movement no context stamped).
pub const STOCK_MOVEMENT_SESSION: &str =
    "(SELECT sc.session_id FROM session_contexts sc WHERE sc.id = stock_movements.activity_context_id)";

/// SQL predicate for a query that already has the session row under
/// `alias`: the session counts in `scope`.
pub fn counts_on(scope: StatsScope, alias: &str) -> String {
    format!("{alias}.{} IS NULL", scope.column())
}

/// SQL predicate: the session counts in every scope listed (the rule for
/// a ratio whose terms span scopes).
pub fn counts_in_all(scopes: &[StatsScope], session_expr: &str) -> String {
    if scopes.is_empty() {
        return "1".to_string();
    }
    scopes
        .iter()
        .map(|scope| counts(*scope, session_expr))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The ids of every session excluded from `scope`, for readers that
/// assemble their session set in Rust.
pub fn excluded_ids(
    conn: &rusqlite::Connection,
    scope: StatsScope,
) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM tracking_sessions WHERE {} IS NOT NULL",
        scope.column()
    ))?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(ids)
}

/// A session's exclusion state, as stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionExclusion {
    pub economy_excluded_at: Option<f64>,
    pub progression_excluded_at: Option<f64>,
    pub note: Option<String>,
}

impl SessionExclusion {
    /// Whether the session is excluded from `scope`.
    pub fn excludes(&self, scope: StatsScope) -> bool {
        match scope {
            StatsScope::Economy => self.economy_excluded_at.is_some(),
            StatsScope::Progression => self.progression_excluded_at.is_some(),
        }
    }

    /// Whether the session is excluded from any scope.
    pub fn any(&self) -> bool {
        self.economy_excluded_at.is_some() || self.progression_excluded_at.is_some()
    }
}

/// The columns [`read_exclusion`] expects, for selects that read them
/// beside other session fields.
pub const EXCLUSION_COLUMNS: &str = "economy_excluded_at, progression_excluded_at, exclusion_note";

/// Read the three exclusion columns starting at `offset` of a row
/// selected with [`EXCLUSION_COLUMNS`].
pub fn read_exclusion(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<SessionExclusion> {
    Ok(SessionExclusion {
        economy_excluded_at: row.get(offset)?,
        progression_excluded_at: row.get(offset + 1)?,
        note: row.get(offset + 2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tracking_sessions (id TEXT PRIMARY KEY, \
               economy_excluded_at REAL, progression_excluded_at REAL, exclusion_note TEXT);
             CREATE TABLE rows (session_id TEXT, v REAL);
             INSERT INTO tracking_sessions VALUES ('in', NULL, NULL, NULL), \
               ('econ', 10.0, NULL, 'bad hotbar'), ('prog', NULL, 11.0, NULL), \
               ('both', 12.0, 13.0, NULL);
             INSERT INTO rows VALUES ('in', 1), ('econ', 2), ('prog', 4), ('both', 8), \
               ('orphan', 16), (NULL, 32);",
        )
        .unwrap();
        conn
    }

    fn sum(conn: &rusqlite::Connection, predicate: &str) -> f64 {
        conn.query_row(
            &format!("SELECT COALESCE(SUM(v), 0) FROM rows r WHERE {predicate}"),
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn counts_drops_only_sessions_excluded_from_the_scope() {
        let conn = conn();
        // Orphan and NULL session ids still count: NOT EXISTS, not NOT IN.
        assert_eq!(
            sum(&conn, &counts(StatsScope::Economy, "r.session_id")),
            1.0 + 4.0 + 16.0 + 32.0
        );
        assert_eq!(
            sum(&conn, &counts(StatsScope::Progression, "r.session_id")),
            1.0 + 2.0 + 16.0 + 32.0
        );
    }

    #[test]
    fn counts_in_all_needs_every_scope() {
        let conn = conn();
        assert_eq!(
            sum(&conn, &counts_in_all(&StatsScope::ALL, "r.session_id")),
            1.0 + 16.0 + 32.0
        );
        assert_eq!(sum(&conn, &counts_in_all(&[], "r.session_id")), 63.0);
    }

    #[test]
    fn counts_on_reads_the_joined_session_row() {
        let conn = conn();
        let total: f64 = conn
            .query_row(
                &format!(
                    "SELECT SUM(r.v) FROM rows r JOIN tracking_sessions s ON s.id = r.session_id \
                     WHERE {}",
                    counts_on(StatsScope::Economy, "s")
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 1.0 + 4.0);
    }

    #[test]
    fn excluded_ids_and_the_row_reader_agree() {
        let conn = conn();
        let economy = excluded_ids(&conn, StatsScope::Economy).unwrap();
        assert_eq!(
            economy,
            HashSet::from(["econ".to_string(), "both".to_string()])
        );
        let exclusion = conn
            .query_row(
                &format!("SELECT {EXCLUSION_COLUMNS} FROM tracking_sessions WHERE id = 'econ'"),
                [],
                |row| read_exclusion(row, 0),
            )
            .unwrap();
        assert!(exclusion.excludes(StatsScope::Economy));
        assert!(!exclusion.excludes(StatsScope::Progression));
        assert!(exclusion.any());
        assert_eq!(exclusion.note.as_deref(), Some("bad hotbar"));
    }
}
