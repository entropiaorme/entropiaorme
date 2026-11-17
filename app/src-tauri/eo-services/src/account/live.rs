//! What a tracked session still running has cost so far, as its readings
//! measure it up to the latest of them: published after each of its
//! readings, so the live figures can take the measured costs up to that
//! reading and the tracked events after it, rather than stepping when the
//! session's costs are sealed at its close.
//!
//! The figures are those a measure that closed at that reading would bill
//! ([`SessionCosts::so_far`]), and only for a measure that reads the
//! session's start and every step since with no gap. Before the first
//! reading after the baseline, or once a gap opens, nothing new is
//! published: a row already published stays true up to its own reading,
//! and the tracked events after it carry the live figures on. A bucket the
//! readings cannot settle so far (the ammo's split, a limited piece no
//! confirmed set holds, a top-up) is published as not measured, and the
//! live figure keeps it tracked. The enhancer, which the readings never
//! measure, is never published.
//!
//! Computed on a reader; the write is short, and stands only while the
//! session still runs, unsealed, and the reading is still its latest. The
//! row stays from the session's stop to its seal (the resolver keeps the
//! session's figures on it meanwhile), and goes at the seal, whatever the
//! sealed costs come to, and with the session.
//!
//! The rows are plain: only the cost resolver reads them
//! ([`crate::session_costs::measured_so_far`]), and nothing on the
//! accounting surface knows of readings.

use std::sync::Arc;

use super::costs::{Basis, CostBucket, SessionCosts, COSTS_VERSION};
use super::measure::{Edge, MEASURE_VERSION};
use super::roles::KnownRoles;
use super::store::{self, ReadingId};
use super::{sessions, AccountError};
use crate::db::Db;
use crate::game_data_store::GameDataStore;

const TABLE: &str = "account_live_session_costs";

/// Each published bucket's column: every bucket but the enhancer.
const COLUMNS: [(CostBucket, &str); 8] = [
    (CostBucket::Weapon, "weapon_micro_ped"),
    (CostBucket::Harvesting, "harvesting_micro_ped"),
    (CostBucket::Healing, "healing_micro_ped"),
    (CostBucket::Armour, "armour_micro_ped"),
    (CostBucket::Consumables, "consumables_micro_ped"),
    (CostBucket::Travel, "travel_micro_ped"),
    (CostBucket::FieldLogistics, "field_logistics_micro_ped"),
    (CostBucket::Unclassified, "unclassified_micro_ped"),
];

/// A running session's costs so far.
#[derive(Debug, Clone, PartialEq)]
pub struct SoFar {
    pub session_id: String,
    /// The reading they run to, and when it was taken (unix seconds).
    pub reading: ReadingId,
    pub through: f64,
    pub costs: SessionCosts,
}

impl SoFar {
    /// What the readings measured of a bucket so far, in micro-PED;
    /// `None` where they could not settle it, and for the enhancer.
    pub fn measured(&self, bucket: CostBucket) -> Option<i64> {
        if bucket == CostBucket::Enhancer {
            return None;
        }
        match self.costs.bucket(bucket)?.basis {
            Basis::Measured(amount) => Some(amount.micro_ped()),
            Basis::Mixed(_) | Basis::Unmeasured(_) => None,
        }
    }
}

/// A running session's costs up to its latest reading, read on a reader.
/// `None` for a session that is not running (or is already sealed), has
/// no reading after its baseline, or whose readings leave its start unread
/// or open a gap.
pub fn so_far(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles: &KnownRoles,
) -> Result<Option<SoFar>, AccountError> {
    let Some(session) = sessions::tracked_session(conn, session_id)? else {
        return Ok(None);
    };
    if !session.active
        || sessions::is_sealed(conn, session_id)?
        || !sessions::has_readings(conn, session_id)?
    {
        return Ok(None);
    }
    let computed = store::compute_session_measure(conn, session_id, &session.window, roles)?;
    let coverage = computed.measure.coverage;
    if coverage.opening != Edge::Read || coverage.gap_count > 0 || coverage.reading_count < 2 {
        return Ok(None);
    }
    let Some(reading) = computed.last_reading() else {
        return Ok(None);
    };
    let Some(header) = store::header(conn, reading)? else {
        return Ok(None);
    };
    let costs = SessionCosts::so_far(&computed.measure, roles, session.window.started_at)?;
    Ok(Some(SoFar {
        session_id: session_id.to_string(),
        reading,
        through: header.taken_at,
        costs,
    }))
}

/// Publish a running session's costs so far, replacing what was published
/// before, in a short write of its own: only while the session still
/// runs, unsealed, and no reading of it newer than the one they run to is
/// stored (that one publishes its own). `Ok(false)` when nothing was
/// written.
pub fn publish(
    conn: &mut rusqlite::Connection,
    so_far: &SoFar,
    computed_at: f64,
) -> Result<bool, AccountError> {
    let tx = conn.transaction()?;
    // id-order: insertion (a reading of the session stored after the one
    // the costs run to)
    let current: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE id = ?1 AND is_active = 1) \
            AND NOT EXISTS (SELECT 1 FROM account_session_measures \
                            WHERE session_id = ?1 AND sealed_at IS NOT NULL) \
            AND NOT EXISTS (SELECT 1 FROM account_readings WHERE session_id = ?1 AND id > ?2)",
        rusqlite::params![so_far.session_id, so_far.reading.0],
        |row| row.get(0),
    )?;
    if !current {
        return Ok(false);
    }
    let columns: Vec<&str> = COLUMNS.iter().map(|(_, column)| *column).collect();
    let amounts: Vec<Option<i64>> = COLUMNS
        .iter()
        .map(|(bucket, _)| so_far.measured(*bucket))
        .collect();
    let placeholders: Vec<String> = (6..6 + columns.len()).map(|n| format!("?{n}")).collect();
    let sql = format!(
        "INSERT OR REPLACE INTO {TABLE} \
             (session_id, costs_version, measure_version, reading_id, measured_through, {}, \
              computed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, {}, ?{})",
        columns.join(", "),
        placeholders.join(", "),
        6 + columns.len(),
    );
    let mut values: Vec<rusqlite::types::Value> = vec![
        so_far.session_id.clone().into(),
        COSTS_VERSION.into(),
        MEASURE_VERSION.into(),
        so_far.reading.0.into(),
        so_far.through.into(),
    ];
    values.extend(amounts.into_iter().map(|amount| match amount {
        Some(micro_ped) => rusqlite::types::Value::Integer(micro_ped),
        None => rusqlite::types::Value::Null,
    }));
    values.push(computed_at.into());
    tx.execute(&sql, rusqlite::params_from_iter(values))?;
    tx.commit()?;
    Ok(true)
}

/// Drop what was published of a session: when it is sealed, whatever its
/// costs came to, or with the session. Run inside that write's own
/// transaction. Whether there was anything to drop.
pub fn forget(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<bool> {
    let dropped = conn.execute(
        &format!("DELETE FROM {TABLE} WHERE session_id = ?1"),
        [session_id],
    )?;
    Ok(dropped > 0)
}

/// Compute a running session's costs so far on a reader and publish them
/// in a short write of their own. `Ok(true)` when something was written.
/// Blocking: run it on the blocking pool, never on a runtime worker.
pub fn publish_so_far(
    db: &Db,
    game_data: Option<Arc<GameDataStore>>,
    session_id: String,
    now: f64,
) -> Result<bool, AccountError> {
    let so_far = db
        .with_reader_blocking(move |conn| {
            let roles = KnownRoles::read(conn, game_data.as_deref());
            Ok(roles.and_then(|roles| so_far(conn, &session_id, &roles)))
        })
        .map_err(AccountError::from)??;
    let Some(so_far) = so_far else {
        return Ok(false);
    };
    db.with_writer_blocking(move |conn| Ok(publish(conn, &so_far, now)))
        .map_err(AccountError::from)?
}
