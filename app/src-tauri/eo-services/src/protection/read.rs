//! Read models for the protection surfaces.
//!
//! An undone recording (and the reading that measured it) is kept as
//! provenance with its `superseded_at` set. Every read of coverage, stream
//! position, or cost goes through the live rows only.

use std::collections::HashSet;

use rusqlite::OptionalExtension;

use super::recording::RESET_UNPRICED_REASON;
use super::{
    sets, ContextShare, CostKind, CostStatus, ObservationSource, ProtectionCostAllocation,
    ProtectionCostWindow, ProtectionError, ProtectionObservation, ProtectionOverview,
    ProtectionSet, UnrecordedProtection,
};

/// How many recent recordings Equipment lists.
const RECENT_COST_WINDOWS: i64 = 12;

/// How many removed sets Equipment offers to restore.
const REMOVED_SETS: i64 = 20;

/// A subquery selecting any live recording's share of the session named by
/// the SQL expression `session`: wrap it in `EXISTS (...)`.
pub(super) fn live_allocation(session: &str) -> String {
    format!(
        "SELECT 1 FROM protection_cost_allocations a \
         JOIN protection_cost_windows w ON w.id = a.window_id \
         WHERE a.session_id = {session} AND w.superseded_at IS NULL"
    )
}

pub(super) fn read_set(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<ProtectionSet>, ProtectionError> {
    sets::read_set_row(conn, id)
}

pub(super) fn read_observation(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionObservation, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT o.id, o.set_id, o.tt_value_ped, o.source, o.raw_text, o.observed_at, \
                    o.reset_reason, \
                    EXISTS (SELECT 1 FROM protection_cost_windows w \
                            WHERE w.closing_observation_id = o.id \
                              AND w.superseded_at IS NULL) \
             FROM protection_observations o WHERE o.id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, bool>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection observation"))?;
    Ok(ProtectionObservation {
        id: row.0,
        set_id: row.1,
        tt_value_ped: row.2,
        source: ObservationSource::parse(&row.3)?,
        raw_text: row.4,
        observed_at: row.5,
        reset_reason: row.6,
        measured: row.7,
    })
}

/// The set's current baseline: its latest reading that was not undone.
pub(super) fn latest_live_observation_id(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<Option<i64>, rusqlite::Error> {
    conn.query_row(
        "SELECT id FROM protection_observations \
         WHERE set_id = ?1 AND superseded_at IS NULL \
         ORDER BY observed_at DESC, id DESC LIMIT 1",
        [set_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

pub(super) fn read_latest_observation(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<Option<ProtectionObservation>, ProtectionError> {
    latest_live_observation_id(conn, set_id)?
        .map(|id| read_observation(conn, id))
        .transpose()
}

/// The unlimited stream's latest live repair.
pub(super) fn latest_live_repair_id(
    conn: &rusqlite::Connection,
) -> Result<Option<i64>, rusqlite::Error> {
    conn.query_row(
        "SELECT id FROM protection_cost_windows \
         WHERE kind = 'repair' AND superseded_at IS NULL \
         ORDER BY created_at DESC, id DESC LIMIT 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

fn read_contexts(
    conn: &rusqlite::Connection,
    window_id: i64,
    session_id: &str,
) -> Result<Vec<ContextShare>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT ca.hit_count, ca.cost_ped, \
                (SELECT group_concat(label, ' · ') FROM ( \
                    SELECT i.label FROM session_context_intervals ci \
                    JOIN session_intervals i ON i.id = ci.interval_id \
                    WHERE ci.context_id = ca.context_id \
                      AND i.kind IN ('segment', 'quest') AND i.label IS NOT NULL \
                    ORDER BY i.kind DESC, i.started_at, i.id)) \
         FROM protection_cost_context_allocations ca \
         WHERE ca.window_id = ?1 AND ca.session_id = ?2 \
         ORDER BY ca.context_key",
    )?;
    let contexts = stmt
        .query_map(rusqlite::params![window_id, session_id], |row| {
            Ok(ContextShare {
                hit_count: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                cost_ped: row.get(1)?,
                label: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(contexts)
}

pub(super) fn read_cost_window(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionCostWindow, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT w.id, w.kind, w.set_id, s.name, w.consumed_tt_ped, w.markup_percent, \
                    w.cost_ped, w.status, w.reason, w.created_at, w.superseded_at, \
                    w.closing_observation_id \
             FROM protection_cost_windows w \
             LEFT JOIN protection_sets s ON s.id = w.set_id \
             WHERE w.id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, f64>(9)?,
                    row.get::<_, Option<f64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection cost window"))?;
    let kind = CostKind::parse(&row.1)?;
    let superseded_at = row.10;
    let undoable = superseded_at.is_none()
        && match kind {
            CostKind::Repair => latest_live_repair_id(conn)? == Some(row.0),
            CostKind::LimitedDecay => match (row.2, row.11) {
                (Some(set_id), Some(closing)) => {
                    latest_live_observation_id(conn, set_id)? == Some(closing)
                }
                _ => false,
            },
        };

    let mut stmt = conn.prepare(
        "SELECT a.session_id, s.session_name, def.name, s.started_at, a.hit_count, \
                a.allocation_share, a.cost_ped \
         FROM protection_cost_allocations a \
         JOIN tracking_sessions s ON s.id = a.session_id \
         LEFT JOIN session_definitions def ON def.id = s.definition_id \
         WHERE a.window_id = ?1 ORDER BY s.started_at, a.session_id",
    )?;
    let sessions = stmt
        .query_map([id], |allocation| {
            Ok((
                allocation.get::<_, String>(0)?,
                allocation.get::<_, Option<String>>(1)?,
                allocation.get::<_, Option<String>>(2)?,
                allocation.get::<_, f64>(3)?,
                allocation.get::<_, Option<i64>>(4)?.unwrap_or(0),
                allocation.get::<_, f64>(5)?,
                allocation.get::<_, f64>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let allocations = sessions
        .into_iter()
        .map(
            |(session_id, session_name, definition_name, started_at, hits, share, cost)| {
                let contexts = read_contexts(conn, id, &session_id)?;
                Ok(ProtectionCostAllocation {
                    session_id,
                    session_name,
                    definition_name,
                    started_at,
                    hit_count: hits,
                    allocation_share: share,
                    cost_ped: cost,
                    contexts,
                })
            },
        )
        .collect::<Result<Vec<_>, rusqlite::Error>>()?;
    Ok(ProtectionCostWindow {
        id: row.0,
        kind,
        set_id: row.2,
        set_name: row.3,
        consumed_tt_ped: row.4,
        markup_percent: row.5,
        cost_ped: row.6,
        cost_known: row.8.as_deref() != Some(RESET_UNPRICED_REASON),
        status: CostStatus::parse(&row.7)?,
        reason: row.8,
        created_at: row.9,
        superseded_at,
        undoable,
        allocations,
    })
}

/// Sessions with recorded hits that no protection cost has reached. A
/// session one stream covers counts as recorded even though another stream
/// may still add to it: the app cannot know which protection was worn, so
/// "some cost recorded" is the most it can truthfully say.
pub(super) fn read_unrecorded(
    conn: &rusqlite::Connection,
) -> Result<UnrecordedProtection, ProtectionError> {
    conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT d.session_id), COUNT(d.id) \
             FROM protection_defence_events d \
             JOIN tracking_sessions s ON s.id = d.session_id \
             WHERE NOT EXISTS ({})",
            live_allocation("d.session_id")
        ),
        [],
        |row| {
            Ok(UnrecordedProtection {
                sessions: row.get(0)?,
                hits: row.get(1)?,
            })
        },
    )
    .map_err(Into::into)
}

fn read_set_ids(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<i64>, ProtectionError> {
    let mut stmt = conn.prepare(sql)?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn read_sets(
    conn: &rusqlite::Connection,
    ids: Vec<i64>,
) -> Result<Vec<ProtectionSet>, ProtectionError> {
    ids.into_iter()
        .map(|id| read_set(conn, id)?.ok_or(ProtectionError::Stored("missing set")))
        .collect()
}

pub(super) fn read_overview(
    conn: &rusqlite::Connection,
) -> Result<ProtectionOverview, ProtectionError> {
    let sets = read_sets(
        conn,
        read_set_ids(
            conn,
            "SELECT id FROM protection_sets \
             WHERE archived_at IS NULL AND economy_kind = 'limited' \
             ORDER BY kind, lower(name), id",
        )?,
    )?;
    let removed_sets = read_sets(
        conn,
        read_set_ids(
            conn,
            &format!(
                "SELECT id FROM protection_sets \
                 WHERE archived_at IS NOT NULL AND economy_kind = 'limited' \
                 ORDER BY archived_at DESC, id DESC LIMIT {REMOVED_SETS}"
            ),
        )?,
    )?;

    let mut window_stmt = conn.prepare(
        "SELECT id FROM protection_cost_windows ORDER BY created_at DESC, id DESC LIMIT ?1",
    )?;
    let window_ids = window_stmt
        .query_map([RECENT_COST_WINDOWS], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let recent_cost_windows = window_ids
        .into_iter()
        .map(|id| read_cost_window(conn, id))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProtectionOverview {
        sets,
        removed_sets,
        unlimited: super::recording::read_backlog(conn, super::ProtectionStream::Unlimited)?,
        recent_cost_windows,
        unrecorded: read_unrecorded(conn)?,
    })
}

/// Recorded hits on one session that no protection cost covers yet, for
/// the session's own cost readout. Zero once any recording reaches it.
pub(super) fn session_unrecorded_hits(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        &format!(
            "SELECT COUNT(d.id) FROM protection_defence_events d \
             WHERE d.session_id = ?1 AND NOT EXISTS ({})",
            live_allocation("d.session_id")
        ),
        [session_id],
        |row| row.get(0),
    )
}

/// Which of `session_ids` have recorded hits that no protection cost
/// covers yet, so a session list can read their cost as incomplete.
pub(super) fn sessions_with_unrecorded_hits(
    conn: &rusqlite::Connection,
    session_ids: &[String],
) -> Result<HashSet<String>, rusqlite::Error> {
    if session_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders = vec!["?"; session_ids.len()].join(", ");
    let sql = format!(
        "SELECT DISTINCT d.session_id FROM protection_defence_events d \
         WHERE d.session_id IN ({placeholders}) AND NOT EXISTS ({})",
        live_allocation("d.session_id")
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(session_ids), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(ids)
}
