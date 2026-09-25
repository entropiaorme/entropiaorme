//! The assignment and its undo, each inside the caller's transaction.
//!
//! Every function returns the refusal as the inner error, so the caller
//! commits only a correction that was actually made; a database failure is
//! the outer error and rolls everything back.

use rusqlite::OptionalExtension;

use super::read::weapon_price;
use super::{WeaponCorrection, WeaponReviewError};
use crate::db::DbError;

/// The phase an unpriced shot is counted under in its kill.
const UNPRICED_TOOL: &str = "Unknown";

type Outcome<T> = Result<Result<T, WeaponReviewError>, DbError>;

/// A stored shot as a correction needs it.
struct StoredShot {
    session_id: String,
    kill_id: Option<String>,
    attribution: String,
    tool_name: Option<String>,
    amount: Option<f64>,
    critical: bool,
    correction_id: Option<String>,
}

fn stored_shot(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
) -> Result<Option<StoredShot>, DbError> {
    Ok(tx
        .query_row(
            "SELECT session_id, kill_id, attribution, tool_name, amount, critical, correction_id \
             FROM weapon_shot_evidence WHERE id = ?1",
            [evidence_id],
            |row| {
                Ok(StoredShot {
                    session_id: row.get(0)?,
                    kill_id: row.get(1)?,
                    attribution: row.get(2)?,
                    tool_name: row.get(3)?,
                    amount: row.get(4)?,
                    critical: row.get(5)?,
                    correction_id: row.get(6)?,
                })
            },
        )
        .optional()?)
}

/// Refuse a correction to a missing or still-running session: a running
/// session's costs are the tracker's until it stops.
fn session_refusal(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<WeaponReviewError>, DbError> {
    let active = tx
        .query_row(
            "SELECT is_active FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(match active {
        None => Some(WeaponReviewError::NotFound("Session not found")),
        Some(0) => None,
        Some(_) => Some(WeaponReviewError::Conflict(
            "Stop the session before assigning its shots",
        )),
    })
}

/// Take one shot out of a kill's phase for `tool` at `cost`, deleting the
/// phase if it empties. False when no phase holds such a shot.
fn take_shot(
    tx: &rusqlite::Transaction<'_>,
    kill_id: &str,
    tool: &str,
    cost: f64,
    amount: f64,
    critical: bool,
) -> Result<bool, DbError> {
    // id-order: insertion (a kill's phases carry no time of their own; the
    // first-written phase at this cost is the one the shot was counted in)
    let taken = tx.execute(
        "UPDATE kill_tool_stats SET shots_fired = shots_fired - 1, \
             damage_dealt = COALESCE(damage_dealt, 0) - ?1, \
             critical_hits = COALESCE(critical_hits, 0) - ?2 \
         WHERE id = (SELECT id FROM kill_tool_stats \
                     WHERE kill_id = ?3 AND tool_name = ?4 AND abs(cost_per_shot - ?5) < 1e-9 \
                       AND shots_fired > 0 \
                     ORDER BY id LIMIT 1)",
        rusqlite::params![amount, i64::from(critical), kill_id, tool, cost],
    )?;
    tx.execute(
        "DELETE FROM kill_tool_stats WHERE kill_id = ?1 AND shots_fired <= 0 \
             AND abs(COALESCE(damage_dealt, 0)) < 1e-9",
        [kill_id],
    )?;
    Ok(taken == 1)
}

/// Count one shot into a kill's phase for `tool` at `cost`, creating the
/// phase when the kill has none at that cost. A corrected phase carries no
/// expected-return evidence: the correction prices the shot, it does not
/// claim the loadout it was fired with.
fn put_shot(
    tx: &rusqlite::Transaction<'_>,
    kill_id: &str,
    tool: &str,
    cost: f64,
    amount: f64,
    critical: bool,
) -> Result<(), DbError> {
    // id-order: insertion (the first-written phase at this cost and tool)
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM kill_tool_stats \
             WHERE kill_id = ?1 AND tool_name = ?2 AND abs(cost_per_shot - ?3) < 1e-9 \
               AND expected_economics_json IS NULL \
             ORDER BY id LIMIT 1",
            rusqlite::params![kill_id, tool, cost],
            |row| row.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => {
            tx.execute(
                "UPDATE kill_tool_stats SET shots_fired = shots_fired + 1, \
                     damage_dealt = COALESCE(damage_dealt, 0) + ?1, \
                     critical_hits = COALESCE(critical_hits, 0) + ?2 \
                 WHERE id = ?3",
                rusqlite::params![amount, i64::from(critical), id],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO kill_tool_stats \
                 (kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot, \
                  expected_economics_json, evidence_fingerprint) \
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, NULL, '')",
                rusqlite::params![kill_id, tool, amount, i64::from(critical), cost],
            )?;
        }
    }
    Ok(())
}

/// Move a shot's cost from one phase to another and repair everything
/// derived from the session's cost. The moved cost is summed at twelve
/// decimals, far below any PEC fraction a price carries, so undoing a
/// correction lands on the exact figure it started from rather than a
/// binary-float neighbour of it.
#[allow(clippy::too_many_arguments)]
fn move_shot(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    kill_id: Option<&str>,
    from: (&str, f64),
    to: (&str, f64),
    amount: f64,
    critical: bool,
) -> Result<bool, DbError> {
    let delta = to.1 - from.1;
    match kill_id {
        Some(kill_id) => {
            if !take_shot(tx, kill_id, from.0, from.1, amount, critical)? {
                return Ok(false);
            }
            put_shot(tx, kill_id, to.0, to.1, amount, critical)?;
            tx.execute(
                "UPDATE kills SET cost_ped = ROUND(COALESCE(cost_ped, 0) + ?1, 12) \
                 WHERE id = ?2",
                rusqlite::params![delta, kill_id],
            )?;
        }
        None => {
            tx.execute(
                "UPDATE tracking_sessions \
                 SET dangling_cost = ROUND(COALESCE(dangling_cost, 0) + ?1, 12) \
                 WHERE id = ?2",
                rusqlite::params![delta, session_id],
            )?;
        }
    }
    crate::session_summary::write_session_summary(tx, session_id)?;
    crate::daily_rollup::refresh_session_days(tx, session_id)?;
    crate::session_rollup::recompute_session(tx, session_id)?;
    Ok(true)
}

pub(super) fn assign(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
    equipment_id: i64,
    now: f64,
) -> Outcome<WeaponCorrection> {
    let Some(shot) = stored_shot(tx, evidence_id)? else {
        return Ok(Err(WeaponReviewError::NotFound("Shot not found")));
    };
    if shot.attribution != "unresolved" {
        return Ok(Err(WeaponReviewError::Invalid(
            "Only a shot no weapon explained can be assigned",
        )));
    }
    if shot.tool_name.is_some() || shot.correction_id.is_some() {
        return Ok(Err(WeaponReviewError::Conflict(
            "This shot is already priced",
        )));
    }
    if let Some(refusal) = session_refusal(tx, &shot.session_id)? {
        return Ok(Err(refusal));
    }
    let Some((name, cost)) = (match weapon_price(tx, equipment_id) {
        Ok(price) => price,
        Err(WeaponReviewError::Db(error)) => return Err(error),
        Err(refusal) => return Ok(Err(refusal)),
    }) else {
        return Ok(Err(WeaponReviewError::NotFound(
            "That weapon is no longer in Equipment",
        )));
    };

    let moved = move_shot(
        tx,
        &shot.session_id,
        shot.kill_id.as_deref(),
        (UNPRICED_TOOL, 0.0),
        (&name, cost),
        shot.amount.unwrap_or(0.0),
        shot.critical,
    )?;
    if !moved {
        return Ok(Err(WeaponReviewError::Stored(
            "the shot is missing from its kill",
        )));
    }
    let correction = WeaponCorrection {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: shot.session_id,
        evidence_id: evidence_id.to_string(),
        equipment_id,
        tool_name: name,
        cost_per_shot: cost,
        corrected_at: now,
    };
    tx.execute(
        "INSERT INTO weapon_attribution_corrections \
         (id, session_id, evidence_id, equipment_id, tool_name, cost_per_shot, corrected_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            correction.id,
            correction.session_id,
            correction.evidence_id,
            correction.equipment_id,
            correction.tool_name,
            correction.cost_per_shot,
            correction.corrected_at,
        ],
    )?;
    tx.execute(
        "UPDATE weapon_shot_evidence SET tool_name = ?1, cost_per_shot = ?2, correction_id = ?3 \
         WHERE id = ?4",
        rusqlite::params![
            correction.tool_name,
            correction.cost_per_shot,
            correction.id,
            evidence_id
        ],
    )?;
    Ok(Ok(correction))
}

pub(super) fn undo(
    tx: &rusqlite::Transaction<'_>,
    correction_id: &str,
    now: f64,
) -> Outcome<String> {
    let row: Option<(String, String, String, f64, Option<f64>)> = tx
        .query_row(
            "SELECT session_id, evidence_id, tool_name, cost_per_shot, undone_at \
             FROM weapon_attribution_corrections WHERE id = ?1",
            [correction_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((session_id, evidence_id, tool_name, cost, undone_at)) = row else {
        return Ok(Err(WeaponReviewError::NotFound("Correction not found")));
    };
    if undone_at.is_some() {
        return Ok(Err(WeaponReviewError::Conflict(
            "This correction is already undone",
        )));
    }
    if let Some(refusal) = session_refusal(tx, &session_id)? {
        return Ok(Err(refusal));
    }
    let Some(shot) = stored_shot(tx, &evidence_id)? else {
        return Ok(Err(WeaponReviewError::Stored("the corrected shot is gone")));
    };
    if shot.correction_id.as_deref() != Some(correction_id) {
        return Ok(Err(WeaponReviewError::Stored(
            "the corrected shot no longer names its correction",
        )));
    }
    let moved = move_shot(
        tx,
        &session_id,
        shot.kill_id.as_deref(),
        (&tool_name, cost),
        (UNPRICED_TOOL, 0.0),
        shot.amount.unwrap_or(0.0),
        shot.critical,
    )?;
    if !moved {
        return Ok(Err(WeaponReviewError::Stored(
            "the corrected shot is missing from its kill",
        )));
    }
    tx.execute(
        "UPDATE weapon_shot_evidence SET tool_name = NULL, cost_per_shot = 0, \
             correction_id = NULL WHERE id = ?1",
        [&evidence_id],
    )?;
    tx.execute(
        "UPDATE weapon_attribution_corrections SET undone_at = ?1 WHERE id = ?2",
        rusqlite::params![now, correction_id],
    )?;
    Ok(Ok(session_id))
}
