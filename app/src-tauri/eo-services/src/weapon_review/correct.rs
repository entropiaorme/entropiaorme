//! The corrections and their undo, each inside the caller's transaction.
//!
//! Two corrections, one undo:
//!
//! - **Priced**: a stored shot left without a price (an unresolved shot, or
//!   an effect tick the player says was a paid shot after all) is priced
//!   from a carried weapon. An unresolved shot already counts as a shot in
//!   its kill's unpriced phase and moves out of it; a tick counted no shot,
//!   so pricing it adds one. A boosted or reduced hit is already priced to
//!   the weapon that was held, and moves from that phase to the chosen
//!   weapon's, the correction recording the cost it replaced.
//! - **Effect tick**: an unresolved hit an open effect could equally have
//!   ticked is marked as that effect's tick: it stops counting as a shot.
//!
//! Every function returns the refusal as the inner error, so the caller
//! commits only a correction that was actually made; a database failure is
//! the outer error and rolls everything back.
//!
//! The `*_shot` forms book one shot and leave the session's derived figures
//! (summary, days, settled cells) for the caller to [`repair`]: a batch
//! corrects many shots and repairs once, which lands on the same figures as
//! repairing after each, since every repair recomputes from the shots.

use rusqlite::OptionalExtension;

use super::read::{
    is_margin, parse_candidates, parse_effect_candidates, stored_margin, weapon_price, PriceBook,
    Pricing,
};
use super::{WeaponCorrection, WeaponCorrectionKind, WeaponReviewError};
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
    /// The weapon the hotbar declared: what a margin hit was priced to.
    hotbar_tool: Option<String>,
    cost_per_shot: f64,
    amount: Option<f64>,
    critical: bool,
    correction_id: Option<String>,
    /// The live decision that repriced it.
    review_id: Option<String>,
    candidates: String,
    effect_candidates: String,
    /// The reload speed in effect when the shot was charged; None for a
    /// shot stored before it was kept.
    reload_speed_percent: Option<f64>,
}

impl StoredShot {
    fn damage(&self) -> f64 {
        self.amount.unwrap_or(0.0)
    }

    /// A hit priced to the held weapon from outside its band.
    fn in_margin(&self) -> bool {
        is_margin(&self.attribution)
    }
}

fn stored_shot(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
) -> Result<Option<StoredShot>, DbError> {
    Ok(tx
        .query_row(
            "SELECT session_id, kill_id, attribution, tool_name, amount, critical, correction_id, \
                    candidates_json, effect_candidates_json, reload_speed_percent, hotbar_tool, \
                    cost_per_shot, review_id \
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
                    candidates: row.get(7)?,
                    effect_candidates: row.get(8)?,
                    reload_speed_percent: row.get(9)?,
                    hotbar_tool: row.get(10)?,
                    cost_per_shot: row.get(11)?,
                    review_id: row.get(12)?,
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
/// claim the loadout it was fired with. `restoring` undoes such a move, so
/// the shot rejoins the phase it left, whatever evidence that carries.
fn put_shot(
    tx: &rusqlite::Transaction<'_>,
    kill_id: &str,
    tool: &str,
    cost: f64,
    amount: f64,
    critical: bool,
    restoring: bool,
) -> Result<(), DbError> {
    // id-order: insertion (the first-written phase at this cost and tool)
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM kill_tool_stats \
             WHERE kill_id = ?1 AND tool_name = ?2 AND abs(cost_per_shot - ?3) < 1e-9 \
               AND (?4 OR expected_economics_json IS NULL) \
             ORDER BY id LIMIT 1",
            rusqlite::params![kill_id, tool, cost, restoring],
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

/// Move the kill's (or, after the last kill, the session's dangling) cost
/// by `cost_delta`, and its shot and critical counts by `shots_delta`. The
/// moved cost is summed at twelve decimals, far below any PEC fraction a
/// price carries, so undoing a correction lands on the exact figure it
/// started from rather than a binary-float neighbour of it.
fn adjust_totals(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    kill_id: Option<&str>,
    cost_delta: f64,
    shots_delta: i64,
    critical: bool,
) -> Result<(), DbError> {
    match kill_id {
        Some(kill_id) => {
            tx.execute(
                "UPDATE kills SET cost_ped = ROUND(COALESCE(cost_ped, 0) + ?1, 12), \
                     shots_fired = COALESCE(shots_fired, 0) + ?2, \
                     critical_hits = COALESCE(critical_hits, 0) + ?3 \
                 WHERE id = ?4",
                rusqlite::params![
                    cost_delta,
                    shots_delta,
                    if critical { shots_delta } else { 0 },
                    kill_id
                ],
            )?;
        }
        None => {
            tx.execute(
                "UPDATE tracking_sessions \
                 SET dangling_cost = ROUND(COALESCE(dangling_cost, 0) + ?1, 12) \
                 WHERE id = ?2",
                rusqlite::params![cost_delta, session_id],
            )?;
        }
    }
    Ok(())
}

/// Recompute everything derived from the session's cost and shots.
pub(super) fn repair(tx: &rusqlite::Transaction<'_>, session_id: &str) -> Result<(), DbError> {
    crate::session_summary::write_session_summary(tx, session_id)?;
    crate::daily_rollup::refresh_session_days(tx, session_id)?;
    crate::session_rollup::recompute_session(tx, session_id)?;
    Ok(())
}

/// Book a shot's move between shot and no-shot, or between phases: take it
/// out of `from` (a phase, or None: it counted no shot) and put it into `to`
/// likewise (`restoring` as [`put_shot`] takes it). False when the shot is
/// missing from its phase.
fn book(
    tx: &rusqlite::Transaction<'_>,
    shot: &StoredShot,
    from: Option<(&str, f64)>,
    to: Option<(&str, f64)>,
    restoring: bool,
) -> Result<bool, DbError> {
    let amount = shot.damage();
    let from_cost = from.map_or(0.0, |(_, cost)| cost);
    let to_cost = to.map_or(0.0, |(_, cost)| cost);
    let shots_delta = i64::from(to.is_some()) - i64::from(from.is_some());
    if let Some(kill_id) = shot.kill_id.as_deref() {
        if let Some((tool, cost)) = from {
            if !take_shot(tx, kill_id, tool, cost, amount, shot.critical)? {
                return Ok(false);
            }
        }
        if let Some((tool, cost)) = to {
            put_shot(tx, kill_id, tool, cost, amount, shot.critical, restoring)?;
        }
    }
    adjust_totals(
        tx,
        &shot.session_id,
        shot.kill_id.as_deref(),
        to_cost - from_cost,
        shots_delta,
        shot.critical,
    )?;
    Ok(true)
}

/// Refuse a correction to a shot already corrected, to one already priced
/// (a margin hit stands priced to its held weapon, and that is what the
/// correction moves), or to a session still running.
fn uncorrected_refusal(
    tx: &rusqlite::Transaction<'_>,
    shot: &StoredShot,
) -> Result<Option<WeaponReviewError>, DbError> {
    if shot.in_margin() {
        if shot.correction_id.is_some() {
            return Ok(Some(WeaponReviewError::Conflict(
                "This shot is already reassigned",
            )));
        }
        if shot.review_id.is_some() {
            return Ok(Some(WeaponReviewError::Conflict(
                "A decision during the session already moved this shot",
            )));
        }
    } else {
        if shot.tool_name.is_some() {
            return Ok(Some(WeaponReviewError::Conflict(
                "This shot is already priced",
            )));
        }
        if shot.correction_id.is_some() {
            return Ok(Some(WeaponReviewError::Conflict(
                "This shot is already marked as an effect's tick",
            )));
        }
    }
    session_refusal(tx, &shot.session_id)
}

fn insert_correction(
    tx: &rusqlite::Transaction<'_>,
    correction: &WeaponCorrection,
    batch_id: Option<&str>,
    replaced_cost: Option<f64>,
) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO weapon_attribution_corrections \
         (id, session_id, evidence_id, equipment_id, tool_name, cost_per_shot, corrected_at, \
          kind, effect_window_id, batch_id, replaced_cost_per_shot) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            correction.id,
            correction.session_id,
            correction.evidence_id,
            correction.equipment_id,
            correction.tool_name,
            correction.cost_per_shot,
            correction.corrected_at,
            correction.kind.as_str(),
            correction.effect_window_id,
            batch_id,
            replaced_cost,
        ],
    )?;
    Ok(())
}

/// Repair the session a committed correction moved, passing the outcome on.
fn repaired<T>(
    tx: &rusqlite::Transaction<'_>,
    outcome: Result<T, WeaponReviewError>,
    session_id: impl Fn(&T) -> &str,
) -> Outcome<T> {
    if let Ok(done) = &outcome {
        repair(tx, session_id(done))?;
    }
    Ok(outcome)
}

pub(super) fn assign(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
    equipment_id: i64,
    now: f64,
    pricing: Pricing<'_>,
) -> Outcome<WeaponCorrection> {
    let outcome = assign_shot(tx, evidence_id, equipment_id, now, pricing, None)?;
    repaired(tx, outcome, |correction| &correction.session_id)
}

/// Price one shot from a weapon, or move a margin hit to it, as one of
/// `batch_id`'s when given, without repairing its session.
pub(super) fn assign_shot(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
    equipment_id: i64,
    now: f64,
    pricing: Pricing<'_>,
    batch_id: Option<&str>,
) -> Outcome<WeaponCorrection> {
    let Some(shot) = stored_shot(tx, evidence_id)? else {
        return Ok(Err(WeaponReviewError::NotFound("Shot not found")));
    };
    // An unresolved shot counts as a shot in the unpriced phase; a tick
    // counted none, and pricing it says it was a paid shot after all.
    let from = match shot.attribution.as_str() {
        "unresolved" => Some((UNPRICED_TOOL, 0.0)),
        "effect_tick" => None,
        "boosted" | "reduced" => match shot.tool_name.as_deref() {
            Some(tool) => Some((tool, shot.cost_per_shot)),
            None => {
                return Ok(Err(WeaponReviewError::Stored(
                    "a margin hit lost its weapon",
                )));
            }
        },
        _ => {
            return Ok(Err(WeaponReviewError::Invalid(
                "Only a shot no weapon explained can be assigned",
            )));
        }
    };
    if let Some(refusal) = uncorrected_refusal(tx, &shot)? {
        return Ok(Err(refusal));
    }
    // The review offers only the weapons carried when the shot landed; the
    // same rule holds here, whatever the caller sends.
    match parse_candidates(&shot.candidates) {
        Ok(candidates) if candidates.iter().any(|c| c.equipment_id == equipment_id) => {}
        Ok(_) => {
            return Ok(Err(WeaponReviewError::Invalid(
                "Only a weapon carried when the shot landed can be assigned",
            )));
        }
        Err(refusal) => return Ok(Err(refusal)),
    }
    let Some(weapon) = (match weapon_price(tx, equipment_id, pricing, shot.reload_speed_percent) {
        Ok(price) => price,
        Err(WeaponReviewError::Db(error)) => return Err(error),
        Err(refusal) => return Ok(Err(refusal)),
    }) else {
        return Ok(Err(WeaponReviewError::NotFound(
            "That weapon is no longer in Equipment",
        )));
    };
    let (name, cost) = (weapon.name, weapon.cost);
    if shot.in_margin() && shot.tool_name.as_deref() == Some(name.as_str()) {
        return Ok(Err(WeaponReviewError::Invalid(
            "This shot is already priced to that weapon",
        )));
    }

    if !book(tx, &shot, from, Some((&name, cost)), false)? {
        return Ok(Err(WeaponReviewError::Stored(
            "the shot is missing from its kill",
        )));
    }
    let replaced_cost = shot.in_margin().then_some(shot.cost_per_shot);
    let correction = WeaponCorrection {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: shot.session_id,
        evidence_id: evidence_id.to_string(),
        kind: WeaponCorrectionKind::Priced,
        equipment_id: Some(equipment_id),
        tool_name: name,
        cost_per_shot: cost,
        effect_window_id: None,
        corrected_at: now,
    };
    insert_correction(tx, &correction, batch_id, replaced_cost)?;
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

pub(super) fn mark_effect_tick(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
    window_id: &str,
    now: f64,
) -> Outcome<WeaponCorrection> {
    let outcome = mark_effect_tick_shot(tx, evidence_id, window_id, now)?;
    repaired(tx, outcome, |correction| &correction.session_id)
}

fn mark_effect_tick_shot(
    tx: &rusqlite::Transaction<'_>,
    evidence_id: &str,
    window_id: &str,
    now: f64,
) -> Outcome<WeaponCorrection> {
    let Some(shot) = stored_shot(tx, evidence_id)? else {
        return Ok(Err(WeaponReviewError::NotFound("Shot not found")));
    };
    if shot.attribution != "unresolved" || shot.amount.is_none() {
        return Ok(Err(WeaponReviewError::Invalid(
            "Only an unresolved hit can be marked as an effect's tick",
        )));
    }
    if let Some(refusal) = uncorrected_refusal(tx, &shot)? {
        return Ok(Err(refusal));
    }
    // Only an effect that was open and explained the hit when it landed.
    match parse_effect_candidates(&shot.effect_candidates) {
        Ok(candidates) if candidates.iter().any(|c| c.window_id == window_id) => {}
        Ok(_) => {
            return Ok(Err(WeaponReviewError::Invalid(
                "Only an effect open when the hit landed can claim it",
            )));
        }
        Err(refusal) => return Ok(Err(refusal)),
    }
    let window: Option<(Option<i64>, String, Option<f64>)> = tx
        .query_row(
            "SELECT equipment_id, tool_name, withdrawn_at FROM weapon_effect_windows WHERE id = ?1",
            [window_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((equipment_id, tool_name, withdrawn_at)) = window else {
        return Ok(Err(WeaponReviewError::NotFound(
            "That effect's session was deleted",
        )));
    };
    if withdrawn_at.is_some() {
        return Ok(Err(WeaponReviewError::Conflict(
            "That cast was taken back while the session ran",
        )));
    }

    if !book(tx, &shot, Some((UNPRICED_TOOL, 0.0)), None, false)? {
        return Ok(Err(WeaponReviewError::Stored(
            "the shot is missing from its kill",
        )));
    }
    let correction = WeaponCorrection {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: shot.session_id,
        evidence_id: evidence_id.to_string(),
        kind: WeaponCorrectionKind::EffectTick,
        equipment_id,
        tool_name,
        cost_per_shot: 0.0,
        effect_window_id: Some(window_id.to_string()),
        corrected_at: now,
    };
    insert_correction(tx, &correction, None, None)?;
    tx.execute(
        "UPDATE weapon_shot_evidence SET correction_id = ?1 WHERE id = ?2",
        rusqlite::params![correction.id, evidence_id],
    )?;
    Ok(Ok(correction))
}

pub(super) fn undo(
    tx: &rusqlite::Transaction<'_>,
    correction_id: &str,
    now: f64,
) -> Outcome<String> {
    let outcome = undo_shot(tx, correction_id, now)?;
    repaired(tx, outcome, String::as_str)
}

/// Undo one live correction without repairing its session. Answers with
/// the session's id.
pub(super) fn undo_shot(
    tx: &rusqlite::Transaction<'_>,
    correction_id: &str,
    now: f64,
) -> Outcome<String> {
    type StoredCorrection = (
        String,
        String,
        String,
        f64,
        Option<f64>,
        String,
        Option<f64>,
    );
    let row: Option<StoredCorrection> = tx
        .query_row(
            "SELECT session_id, evidence_id, tool_name, cost_per_shot, undone_at, kind, \
                    replaced_cost_per_shot \
             FROM weapon_attribution_corrections WHERE id = ?1",
            [correction_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((session_id, evidence_id, tool_name, cost, undone_at, kind, replaced_cost)) = row
    else {
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
    let Some(kind) = WeaponCorrectionKind::parse(&kind) else {
        return Ok(Err(WeaponReviewError::Stored("unknown correction kind")));
    };
    // Put the shot back where it stood before the correction.
    // A margin hit goes back to the phase it left, at the price it stood at.
    let held = if shot.in_margin() {
        match (shot.hotbar_tool.clone(), replaced_cost) {
            (Some(tool), Some(replaced)) => Some((tool, replaced)),
            _ => {
                return Ok(Err(WeaponReviewError::Stored(
                    "the correction does not record what it replaced",
                )));
            }
        }
    } else {
        None
    };
    let (from, to) = match (kind, shot.attribution.as_str()) {
        (WeaponCorrectionKind::Priced, "unresolved") => {
            (Some((tool_name.as_str(), cost)), Some((UNPRICED_TOOL, 0.0)))
        }
        (WeaponCorrectionKind::Priced, "boosted" | "reduced") => (
            Some((tool_name.as_str(), cost)),
            held.as_ref()
                .map(|(tool, replaced)| (tool.as_str(), *replaced)),
        ),
        (WeaponCorrectionKind::Priced, "effect_tick") => (Some((tool_name.as_str(), cost)), None),
        (WeaponCorrectionKind::EffectTick, "unresolved") => (None, Some((UNPRICED_TOOL, 0.0))),
        _ => {
            return Ok(Err(WeaponReviewError::Stored(
                "the correction does not match its shot",
            )));
        }
    };
    if !book(tx, &shot, from, to, held.is_some())? {
        return Ok(Err(WeaponReviewError::Stored(
            "the corrected shot is missing from its kill",
        )));
    }
    let (restored_tool, restored_cost) = match &held {
        Some((tool, replaced)) => (Some(tool.as_str()), *replaced),
        None => (None, 0.0),
    };
    tx.execute(
        "UPDATE weapon_shot_evidence SET tool_name = ?1, cost_per_shot = ?2, \
             correction_id = NULL WHERE id = ?3",
        rusqlite::params![restored_tool, restored_cost, evidence_id],
    )?;
    tx.execute(
        "UPDATE weapon_attribution_corrections SET undone_at = ?1 WHERE id = ?2",
        rusqlite::params![now, correction_id],
    )?;
    Ok(Ok(session_id))
}

/// The session's unpriced shots (unresolved, without a price or a
/// correction) the weapon explains, inside its band or in the margin around
/// it, oldest first, and how many others it would leave. Refuses a missing
/// or running session, and a weapon no longer in Equipment.
pub(super) fn fitting_shots(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    equipment_id: i64,
    pricing: Pricing<'_>,
) -> Outcome<(Vec<String>, i64)> {
    if let Some(refusal) = session_refusal(tx, session_id)? {
        return Ok(Err(refusal));
    }
    match weapon_price(tx, equipment_id, pricing, None) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Ok(Err(WeaponReviewError::NotFound(
                "That weapon is no longer in Equipment",
            )));
        }
        Err(WeaponReviewError::Db(error)) => return Err(error),
        Err(refusal) => return Ok(Err(refusal)),
    }
    let mut stmt = tx.prepare(
        "SELECT id, candidates_json, amount, critical, reload_speed_percent \
         FROM weapon_shot_evidence \
         WHERE session_id = ?1 AND attribution = 'unresolved' AND tool_name IS NULL \
           AND correction_id IS NULL \
         ORDER BY observed_at, id",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<f64>>(2)?,
            row.get::<_, bool>(3)?,
            row.get::<_, Option<f64>>(4)?,
        ))
    })?;
    let mut book = PriceBook::new(tx, pricing);
    let mut fitting = Vec::new();
    let mut unfit = 0;
    for row in rows {
        let (id, candidates, amount, critical, reload_speed_percent) = row?;
        let candidates = match parse_candidates(&candidates) {
            Ok(candidates) => candidates,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let band = match book.price(equipment_id, reload_speed_percent) {
            Ok(weapon) => weapon.and_then(|weapon| weapon.band),
            Err(WeaponReviewError::Db(error)) => return Err(error),
            Err(refusal) => return Ok(Err(refusal)),
        };
        let explained = candidates.iter().any(|candidate| {
            candidate.equipment_id == equipment_id
                && (candidate.fits || stored_margin(candidate, amount, critical, band).is_some())
        });
        if explained {
            fitting.push(id);
        } else {
            unfit += 1;
        }
    }
    Ok(Ok((fitting, unfit)))
}

/// A batch's session and its corrections still live, oldest first. Refuses
/// a batch that never existed, and one whose session is running.
pub(super) fn live_batch(
    tx: &rusqlite::Transaction<'_>,
    batch_id: &str,
) -> Outcome<(String, Vec<String>)> {
    let session_id: Option<String> = tx
        .query_row(
            "SELECT session_id FROM weapon_attribution_corrections WHERE batch_id = ?1 LIMIT 1",
            [batch_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(session_id) = session_id else {
        return Ok(Err(WeaponReviewError::NotFound("Batch not found")));
    };
    if let Some(refusal) = session_refusal(tx, &session_id)? {
        return Ok(Err(refusal));
    }
    let mut stmt = tx.prepare(
        "SELECT id FROM weapon_attribution_corrections \
         WHERE batch_id = ?1 AND undone_at IS NULL ORDER BY corrected_at, id",
    )?;
    let live = stmt
        .query_map([batch_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Ok((session_id, live)))
}
