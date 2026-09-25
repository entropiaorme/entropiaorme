//! Read models for the protection surfaces.

use rusqlite::OptionalExtension;

use super::recording::RESET_UNPRICED_REASON;
use super::{
    sets, CostKind, CostStatus, ObservationSource, ProtectionCostAllocation, ProtectionCostWindow,
    ProtectionError, ProtectionObservation, ProtectionOverview, ProtectionSet,
    UnrecordedProtection,
};

/// How many recent recordings Equipment lists.
const RECENT_COST_WINDOWS: i64 = 12;

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
            "SELECT id, set_id, tt_value_ped, source, raw_text, observed_at, reset_reason \
             FROM protection_observations WHERE id = ?1",
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
    })
}

pub(super) fn read_latest_observation(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<Option<ProtectionObservation>, ProtectionError> {
    let id = conn
        .query_row(
            "SELECT id FROM protection_observations WHERE set_id = ?1 \
             ORDER BY observed_at DESC, id DESC LIMIT 1",
            [set_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    id.map(|id| read_observation(conn, id)).transpose()
}

pub(super) fn read_cost_window(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionCostWindow, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT w.id, w.kind, w.set_id, s.name, w.consumed_tt_ped, w.markup_percent, \
                    w.cost_ped, w.status, w.reason, w.created_at \
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
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection cost window"))?;
    let mut stmt = conn.prepare(
        "SELECT a.session_id, a.hit_count, a.allocation_share, a.cost_ped \
         FROM protection_cost_allocations a \
         JOIN tracking_sessions s ON s.id = a.session_id \
         WHERE a.window_id = ?1 ORDER BY s.started_at, a.session_id",
    )?;
    let allocations = stmt
        .query_map([id], |allocation| {
            Ok(ProtectionCostAllocation {
                session_id: allocation.get(0)?,
                hit_count: allocation.get(1)?,
                allocation_share: allocation.get(2)?,
                cost_ped: allocation.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ProtectionCostWindow {
        id: row.0,
        kind: CostKind::parse(&row.1)?,
        set_id: row.2,
        set_name: row.3,
        consumed_tt_ped: row.4,
        markup_percent: row.5,
        cost_ped: row.6,
        cost_known: row.8.as_deref() != Some(RESET_UNPRICED_REASON),
        status: CostStatus::parse(&row.7)?,
        reason: row.8,
        created_at: row.9,
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
        "SELECT COUNT(DISTINCT d.session_id), COUNT(d.id) \
         FROM protection_defence_events d \
         JOIN tracking_sessions s ON s.id = d.session_id \
         WHERE NOT EXISTS (SELECT 1 FROM protection_cost_allocations a \
                           WHERE a.session_id = d.session_id)",
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

pub(super) fn read_overview(
    conn: &rusqlite::Connection,
) -> Result<ProtectionOverview, ProtectionError> {
    let mut set_stmt = conn.prepare(
        "SELECT id FROM protection_sets \
         WHERE archived_at IS NULL AND economy_kind = 'limited' \
         ORDER BY kind, lower(name), id",
    )?;
    let set_ids = set_stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let sets = set_ids
        .into_iter()
        .map(|id| read_set(conn, id)?.ok_or(ProtectionError::Stored("missing set")))
        .collect::<Result<Vec<_>, _>>()?;

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
        "SELECT COUNT(d.id) FROM protection_defence_events d \
         WHERE d.session_id = ?1 \
           AND NOT EXISTS (SELECT 1 FROM protection_cost_allocations a \
                           WHERE a.session_id = d.session_id)",
        [session_id],
        |row| row.get(0),
    )
}
