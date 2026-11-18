//! What travel and the like cost between tracked sessions, as the readings
//! taken meanwhile measure it.
//!
//! The readings taken while signed in and otherwise idle follow the
//! inventory between sessions. What they show wearing or used up that a
//! session's overhead would bill (teleport chips and the Mind Essence they
//! burn, what the player placed as travel, field logistics, and what the
//! app cannot place yet) is billed on the day it happened: in the
//! Overview's Costs and Net, credited to no activity, never in Cycled, a
//! rate or an expected figure. What performing an activity costs (a weapon
//! wearing, ammo burned) is left: play no session tracked is the
//! reconciliation's.
//!
//! Each step is measured once, when the pass that follows the stock looks
//! at it ([`super::follow`]), and kept with its measure. What it bills is
//! priced from that measure by the same rules and roles as a session's
//! costs ([`SessionCosts`]), and priced again when either changes
//! ([`heal`]). A step is one between sessions when its later reading was
//! taken for no session, or as a session started (its first reading), and
//! no strongbox run read both its ends. Whether it counts is decided when
//! it is read (the `overhead_billed_between_sessions` view, which the cost
//! resolver hands to the day rollup and the Overview): a step a session's
//! measure, or its costs read across a gap, came to cover is that
//! session's, never billed twice.

use rusqlite::OptionalExtension as _;

use super::amount::MICRO_PER_PED;
use super::costs::{CostBucket, SessionCosts, COSTS_VERSION};
use super::measure::{measure_used, Edge, SessionMeasure, MEASURE_VERSION};
use super::roles::{KnownRoles, RoleSource};
use super::store::{self, Purpose, ReadingId, StoredReading};
use super::AccountError;

/// The most steps one pass of [`heal`] prices again; the rest wait for the
/// next pass.
pub const STEPS_PER_HEAL: usize = 500;

/// A step between sessions, measured.
#[derive(Debug, Clone, PartialEq)]
pub struct BetweenStep {
    /// The later reading, and the one before it.
    pub reading: ReadingId,
    pub prev: ReadingId,
    /// When the two were taken (unix seconds).
    pub from: f64,
    pub to: f64,
    pub measure: SessionMeasure,
}

/// What a step bills as overhead, by category, in micro-PED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overhead {
    pub travel: i64,
    pub field_logistics: i64,
    pub unclassified: i64,
}

impl Overhead {
    fn ped(micro: i64) -> f64 {
        micro as f64 / MICRO_PER_PED as f64
    }
}

/// Whether the step from `before` to `after` lies between sessions: its
/// later reading was taken for no session, or as one started, and no
/// strongbox run read both its ends.
pub fn is_between(before: &StoredReading, after: &StoredReading) -> bool {
    if before.header.purpose == Purpose::Run && after.header.purpose == Purpose::Run {
        return false;
    }
    after.header.session_id.is_none() || after.header.purpose == Purpose::Baseline
}

/// Measure the step from `before` to `after` when it lies between sessions
/// and anything in it wore or was used up; `None` otherwise. A read.
pub fn measure(
    before: &StoredReading,
    after: &StoredReading,
    roles: &dyn RoleSource,
) -> Result<Option<BetweenStep>, AccountError> {
    if !is_between(before, after) {
        return Ok(None);
    }
    let from = before.reading.taken_at();
    let to = after.reading.taken_at();
    if to < from {
        return Ok(None);
    }
    let measure = measure_used(
        &[before.reading.clone(), after.reading.clone()],
        Edge::Read,
        Edge::Read,
        roles,
    )?;
    if measure.decay.is_empty() && measure.other_consumption.is_empty() {
        return Ok(None);
    }
    Ok(Some(BetweenStep {
        reading: after.header.id,
        prev: before.header.id,
        from,
        to,
        measure,
    }))
}

/// What a measure bills as overhead, by the roles given: each line of the
/// travel, field logistics and unclassified buckets that charges anything.
pub fn overhead_of(
    measure: &SessionMeasure,
    roles: &KnownRoles,
    from: f64,
) -> Result<Overhead, AccountError> {
    let costs = SessionCosts::running(measure, roles, from)?;
    let mut overhead = Overhead::default();
    for line in &costs.lines {
        let Some(amount) = line.amount() else {
            continue;
        };
        let into = match line.bucket {
            CostBucket::Travel => &mut overhead.travel,
            CostBucket::FieldLogistics => &mut overhead.field_logistics,
            CostBucket::Unclassified => &mut overhead.unclassified,
            _ => continue,
        };
        *into = into
            .checked_add(amount.micro_ped())
            .ok_or(AccountError::Overflow)?;
    }
    Ok(overhead)
}

/// The rollup day a step's overhead lands on: the UTC day it ended.
fn day_of(to: f64) -> String {
    crate::daily_rollup::epoch_day(to)
}

/// Keep a step measured between sessions, priced by `roles`, and bring its
/// day's rollup up to date. Keeping it again changes nothing.
pub fn store(
    conn: &rusqlite::Connection,
    step: &BetweenStep,
    roles: &KnownRoles,
    now: f64,
) -> Result<(), AccountError> {
    let overhead = overhead_of(&step.measure, roles, step.from)?;
    let json = serde_json::to_string(&step.measure)
        .map_err(|_| AccountError::Stored("a measure does not serialise"))?;
    let added = conn.execute(
        "INSERT INTO account_between_sessions_steps \
             (reading_id, prev_reading_id, seen_from, seen_to, measure_version, measure_json, \
              travel_ped, field_logistics_ped, unclassified_ped, costs_version, \
              roles_fingerprint, computed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT (reading_id) DO NOTHING",
        rusqlite::params![
            step.reading.0,
            step.prev.0,
            step.from,
            step.to,
            MEASURE_VERSION,
            json,
            Overhead::ped(overhead.travel),
            Overhead::ped(overhead.field_logistics),
            Overhead::ped(overhead.unclassified),
            COSTS_VERSION,
            roles.fingerprint(),
            now,
        ],
    )?;
    if added > 0 {
        crate::daily_rollup::refresh_days(conn, [day_of(step.to)])?;
    }
    Ok(())
}

/// One step whose price is not the code's or the roles'.
struct Stale {
    reading: i64,
    prev: i64,
    from: f64,
    to: f64,
    measure_version: i64,
    measure_json: String,
}

/// The measure a stale step is priced from: its own, or, when that was
/// taken by other rules, the step measured again from its readings; `None`
/// when they are gone or no longer measure anything.
fn current_measure(
    conn: &rusqlite::Connection,
    stale: &Stale,
    roles: &KnownRoles,
) -> Result<Option<SessionMeasure>, AccountError> {
    if stale.measure_version == MEASURE_VERSION {
        if let Ok(measure) = serde_json::from_str::<SessionMeasure>(&stale.measure_json) {
            return Ok(Some(measure));
        }
    }
    let before = store::load_reading(conn, ReadingId(stale.prev))?;
    let after = store::load_reading(conn, ReadingId(stale.reading))?;
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(None);
    };
    Ok(measure(&before, &after, roles)?.map(|step| step.measure))
}

/// Price again the steps between sessions priced by other rules or other
/// roles than `roles`, at most [`STEPS_PER_HEAL`] in one write, and bring
/// their days' rollups up to date. A step whose readings are gone keeps its
/// price, marked current. Returns how many were priced again.
pub fn heal(
    conn: &mut rusqlite::Connection,
    roles: &KnownRoles,
    now: f64,
) -> Result<usize, AccountError> {
    let tx = conn.transaction()?;
    let stale: Vec<Stale> = {
        let mut stmt = tx.prepare(
            // id-order: insertion (the oldest steps first; a pass takes a bounded batch)
            "SELECT reading_id, prev_reading_id, seen_from, seen_to, measure_version, \
                    measure_json \
             FROM account_between_sessions_steps \
             WHERE costs_version != ?1 OR roles_fingerprint != ?2 OR measure_version != ?3 \
             ORDER BY reading_id LIMIT ?4",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    COSTS_VERSION,
                    roles.fingerprint(),
                    MEASURE_VERSION,
                    i64::try_from(STEPS_PER_HEAL).unwrap_or(i64::MAX),
                ],
                |row| {
                    Ok(Stale {
                        reading: row.get(0)?,
                        prev: row.get(1)?,
                        from: row.get(2)?,
                        to: row.get(3)?,
                        measure_version: row.get(4)?,
                        measure_json: row.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut days = std::collections::BTreeSet::new();
    for step in &stale {
        match current_measure(&tx, step, roles)? {
            Some(measure) => {
                let overhead = overhead_of(&measure, roles, step.from)?;
                let json = serde_json::to_string(&measure)
                    .map_err(|_| AccountError::Stored("a measure does not serialise"))?;
                tx.execute(
                    "UPDATE account_between_sessions_steps \
                     SET measure_version = ?2, measure_json = ?3, travel_ped = ?4, \
                         field_logistics_ped = ?5, unclassified_ped = ?6, costs_version = ?7, \
                         roles_fingerprint = ?8, computed_at = ?9 \
                     WHERE reading_id = ?1",
                    rusqlite::params![
                        step.reading,
                        MEASURE_VERSION,
                        json,
                        Overhead::ped(overhead.travel),
                        Overhead::ped(overhead.field_logistics),
                        Overhead::ped(overhead.unclassified),
                        COSTS_VERSION,
                        roles.fingerprint(),
                        now,
                    ],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE account_between_sessions_steps \
                     SET measure_version = ?2, costs_version = ?3, roles_fingerprint = ?4, \
                         computed_at = ?5 \
                     WHERE reading_id = ?1",
                    rusqlite::params![
                        step.reading,
                        MEASURE_VERSION,
                        COSTS_VERSION,
                        roles.fingerprint(),
                        now,
                    ],
                )?;
            }
        }
        days.insert(day_of(step.to));
    }
    crate::daily_rollup::refresh_days(&tx, days)?;
    tx.commit()?;
    Ok(stale.len())
}

/// Whether any step between sessions waits to be priced again by `roles`.
pub fn any_stale(conn: &rusqlite::Connection, roles: &KnownRoles) -> Result<bool, AccountError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM account_between_sessions_steps \
             WHERE costs_version != ?1 OR roles_fingerprint != ?2 OR measure_version != ?3 \
             LIMIT 1",
            rusqlite::params![COSTS_VERSION, roles.fingerprint(), MEASURE_VERSION],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
