//! The two corrections and their undo, each inside the caller's transaction.
//!
//! Every function returns the refusal as the inner error, so the caller
//! commits only a correction that was actually made; a database failure is
//! the outer error and rolls everything back.

use rusqlite::OptionalExtension;

use super::{CorrectionKind, CorrectionTarget, HealingCorrection, HealingReviewError};
use crate::db::DbError;
use crate::equipment_pricing::{heal_cost_from_props, healing_profile_from_props};

/// Ticks may land this long after an effect's expiry, as the tracker allows.
const DELIVERY_TAIL_SECONDS: f64 = 1.25;

const NOT_PAID_REASON: &str = "its activation was marked as not a paid use";
const PAID_REASON: &str = "marked as a paid use";
const PAID_TICK_REASON: &str = "a tick of an effect marked as a paid use";

type Outcome<T> = Result<Result<T, HealingReviewError>, DbError>;

pub(super) fn apply(
    tx: &rusqlite::Transaction<'_>,
    target: &CorrectionTarget,
    now: f64,
) -> Outcome<HealingCorrection> {
    match target {
        CorrectionTarget::NotPaidUse { activation_id } => not_paid_use(tx, activation_id, now),
        CorrectionTarget::PaidUse {
            output_id,
            equipment_id,
        } => paid_use(tx, output_id, *equipment_id, now),
    }
}

/// Refuse a correction to a missing or still-running session: a running
/// session's heal cost is the tracker's until it stops.
fn session_refusal(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<Option<HealingReviewError>, DbError> {
    let active = tx
        .query_row(
            "SELECT is_active FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(match active {
        None => Some(HealingReviewError::NotFound("Session not found")),
        Some(0) => None,
        Some(_) => Some(HealingReviewError::Conflict(
            "Stop the session before correcting its healing",
        )),
    })
}

/// Refuse a correction that would move evidence a running session recorded.
/// An effect window outlives the session that paid for it, so a running
/// session can hold ticks of an ended session's activation; rewriting them
/// under the tracker would leave its live readout stale. Stopping the
/// session makes the same correction possible.
fn running_session_refusal(
    tx: &rusqlite::Transaction<'_>,
    activation_id: &str,
    correction_id: Option<&str>,
) -> Result<Option<HealingReviewError>, DbError> {
    let touches_running: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM healing_outputs o \
                        JOIN tracking_sessions s ON s.id = o.session_id \
                        WHERE s.is_active = 1 \
                          AND (o.activation_id = ?1 OR o.correction_id = ?2))",
        rusqlite::params![activation_id, correction_id],
        |row| row.get(0),
    )?;
    Ok(touches_running.then_some(HealingReviewError::Conflict(
        "A running session still shows this heal's effect; stop it before correcting",
    )))
}

/// Move a session's heal cost by `delta` and repair everything derived from
/// it: the summary, the days it touches, and its settled cells.
fn adjust_session_heal(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    delta: f64,
) -> Result<(), DbError> {
    tx.execute(
        "UPDATE tracking_sessions SET heal_cost = \
             CASE WHEN abs(COALESCE(heal_cost, 0) + ?1) < 1e-9 THEN 0 \
                  ELSE COALESCE(heal_cost, 0) + ?1 END \
         WHERE id = ?2",
        rusqlite::params![delta, session_id],
    )?;
    crate::session_summary::write_session_summary(tx, session_id)?;
    crate::daily_rollup::refresh_session_days(tx, session_id)?;
    crate::session_rollup::recompute_session(tx, session_id)?;
    Ok(())
}

/// Record what an output was before a correction moves it, then move it.
/// Only an output no live correction owns can move, so its recorded prior
/// state is always the uncorrected one an undo must restore; anything else
/// fails the whole correction.
#[allow(clippy::too_many_arguments)]
fn move_output(
    tx: &rusqlite::Transaction<'_>,
    output_id: &str,
    classification: &str,
    activation_id: Option<&str>,
    effect_window_id: Option<&str>,
    reason: &str,
    correction_id: &str,
) -> Result<(), DbError> {
    let moved = tx.execute(
        "UPDATE healing_outputs SET \
             prior_classification = classification, \
             prior_activation_id = activation_id, \
             prior_effect_window_id = effect_window_id, \
             prior_reason = reason, \
             classification = ?1, activation_id = ?2, effect_window_id = ?3, \
             reason = ?4, correction_id = ?5 \
         WHERE id = ?6 AND correction_id IS NULL",
        rusqlite::params![
            classification,
            activation_id,
            effect_window_id,
            reason,
            correction_id,
            output_id
        ],
    )?;
    if moved != 1 {
        return Err(DbError::from(rusqlite::Error::QueryReturnedNoRows));
    }
    Ok(())
}

fn insert_correction(
    tx: &rusqlite::Transaction<'_>,
    correction: &HealingCorrection,
) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO healing_corrections \
         (id, session_id, kind, activation_id, output_id, cost_delta_ped, corrected_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            correction.id,
            correction.session_id,
            correction.kind.as_str(),
            correction.activation_id,
            correction.output_id,
            correction.cost_delta_ped,
            correction.corrected_at,
        ],
    )?;
    Ok(())
}

fn not_paid_use(
    tx: &rusqlite::Transaction<'_>,
    activation_id: &str,
    now: f64,
) -> Outcome<HealingCorrection> {
    let Some((session_id, cost, superseded, minted_by, confirming)) = tx
        .query_row(
            "SELECT session_id, cost_ped, superseded_at IS NOT NULL, correction_id, \
                    confirming_output_id \
             FROM healing_activations WHERE id = ?1",
            [activation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(HealingReviewError::NotFound(
            "Healing activation not found",
        )));
    };
    if superseded {
        return Ok(Err(HealingReviewError::Conflict(
            "This heal is already marked as not a paid use",
        )));
    }
    if minted_by.is_some() {
        return Ok(Err(HealingReviewError::Conflict(
            "This paid use came from a correction; undo that correction instead",
        )));
    }
    if let Some(refusal) = session_refusal(tx, &session_id)? {
        return Ok(Err(refusal));
    }
    if let Some(refusal) = running_session_refusal(tx, activation_id, None)? {
        return Ok(Err(refusal));
    }

    let correction = HealingCorrection {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        kind: CorrectionKind::NotPaidUse,
        activation_id: activation_id.to_string(),
        output_id: confirming,
        cost_delta_ped: -cost,
        corrected_at: now,
        undone_at: None,
    };
    insert_correction(tx, &correction)?;
    tx.execute(
        "UPDATE healing_activations SET superseded_at = ?1 WHERE id = ?2",
        rusqlite::params![now, activation_id],
    )?;
    tx.execute(
        "UPDATE healing_effect_windows SET superseded_at = ?1 \
         WHERE activation_id = ?2 AND superseded_at IS NULL",
        rusqlite::params![now, activation_id],
    )?;
    // A buff the heal granted on use goes with it.
    crate::consumables::set_activation_doses_removed(tx, activation_id, Some(now))?;
    let explained: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM healing_outputs \
             WHERE activation_id = ?1 AND correction_id IS NULL",
        )?;
        let rows = stmt.query_map([activation_id], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for output_id in explained {
        move_output(
            tx,
            &output_id,
            "unattributed",
            None,
            None,
            NOT_PAID_REASON,
            &correction.id,
        )?;
    }
    adjust_session_heal(tx, &session_id, -cost)?;
    Ok(Ok(correction))
}

fn paid_use(
    tx: &rusqlite::Transaction<'_>,
    output_id: &str,
    equipment_id: i64,
    now: f64,
) -> Outcome<HealingCorrection> {
    let Some((session_id, observed_at, chat_timestamp, context_id, moved_by)) = tx
        .query_row(
            "SELECT session_id, observed_at, chat_timestamp, context_id, correction_id \
             FROM healing_outputs WHERE id = ?1",
            [output_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(HealingReviewError::NotFound("Heal not found")));
    };
    if moved_by.is_some() {
        return Ok(Err(HealingReviewError::Conflict(
            "A correction already moved this heal; undo it first",
        )));
    }
    let bills: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM healing_activations \
                        WHERE confirming_output_id = ?1 AND superseded_at IS NULL)",
        [output_id],
        |row| row.get(0),
    )?;
    if bills {
        return Ok(Err(HealingReviewError::Conflict(
            "This heal is already a paid use",
        )));
    }
    if let Some(refusal) = session_refusal(tx, &session_id)? {
        return Ok(Err(refusal));
    }
    let Some((tool_name, item_type, properties_json)) = tx
        .query_row(
            "SELECT name, item_type, properties_json FROM equipment_library WHERE id = ?1",
            [equipment_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(HealingReviewError::NotFound("Healing item not found")));
    };
    if item_type != "healing" {
        return Ok(Err(HealingReviewError::Invalid(
            "Only a healing item can be a paid use",
        )));
    }

    // The correction snapshots the item's pricing and profile as it is
    // configured now, exactly as a live activation snapshots them.
    let (cost, _) = heal_cost_from_props(&properties_json);
    let profile = healing_profile_from_props(&properties_json);
    let profile_json = serde_json::to_string(&profile).map_err(|source| DbError::Decode {
        context: "healing correction profile encode",
        source,
    })?;
    let activation_id = uuid::Uuid::new_v4().to_string();
    let correction = HealingCorrection {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        kind: CorrectionKind::PaidUse,
        activation_id: activation_id.clone(),
        output_id: Some(output_id.to_string()),
        cost_delta_ped: cost,
        corrected_at: now,
        undone_at: None,
    };

    tx.execute(
        "INSERT INTO healing_activations \
         (id, session_id, equipment_id, tool_name, intent_at, observed_at, chat_timestamp, \
          context_id, cost_ped, profile_json, provenance, confirming_output_id, correction_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, 'direct', ?10, ?11)",
        rusqlite::params![
            activation_id,
            session_id,
            equipment_id,
            tool_name,
            observed_at,
            chat_timestamp,
            context_id,
            cost,
            profile_json,
            output_id,
            correction.id,
        ],
    )?;
    let window = profile
        .effect_duration()
        .map(|duration| (uuid::Uuid::new_v4().to_string(), observed_at + duration));
    if let Some((window_id, expires_at)) = &window {
        tx.execute(
            "INSERT INTO healing_effect_windows \
             (id, activation_id, session_id, equipment_id, tool_name, started_at, expires_at, \
              tick_min, tick_max, tick_seconds, context_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                window_id,
                activation_id,
                session_id,
                equipment_id,
                tool_name,
                observed_at,
                expires_at,
                profile.tick_min,
                profile.tick_max,
                profile.tick_seconds,
                context_id,
            ],
        )?;
    }
    insert_correction(tx, &correction)?;

    let window_id = window.as_ref().map(|(id, _)| id.as_str());
    let classification = if window.is_some() && !profile.mode.has_direct() {
        "effect"
    } else {
        "direct"
    };
    move_output(
        tx,
        output_id,
        classification,
        Some(&activation_id),
        window_id,
        PAID_REASON,
        &correction.id,
    )?;

    // Later unexplained outputs the new effect explains become its ticks.
    // Only unattributed ones: a passive output already has an explanation,
    // and an output another correction moved stays with that correction.
    if let Some((window_id, expires_at)) = &window {
        let candidates: Vec<(String, f64)> = {
            let mut stmt = tx.prepare(
                "SELECT id, amount FROM healing_outputs \
                 WHERE session_id = ?1 AND id <> ?2 AND classification = 'unattributed' \
                   AND correction_id IS NULL AND observed_at > ?3 AND observed_at <= ?4 \
                 ORDER BY observed_at, id",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![
                    session_id,
                    output_id,
                    observed_at,
                    expires_at + DELIVERY_TAIL_SECONDS
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (tick_id, amount) in candidates {
            if profile.tick_matches(amount) {
                move_output(
                    tx,
                    &tick_id,
                    "effect",
                    Some(&activation_id),
                    Some(window_id),
                    PAID_TICK_REASON,
                    &correction.id,
                )?;
            }
        }
    }
    adjust_session_heal(tx, &session_id, cost)?;
    Ok(Ok(correction))
}

pub(super) fn undo(
    tx: &rusqlite::Transaction<'_>,
    correction_id: &str,
    now: f64,
) -> Outcome<String> {
    let Some((session_id, kind, activation_id, cost_delta, undone)) = tx
        .query_row(
            "SELECT session_id, kind, activation_id, cost_delta_ped, undone_at IS NOT NULL \
             FROM healing_corrections WHERE id = ?1",
            [correction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(HealingReviewError::NotFound("Correction not found")));
    };
    if undone {
        return Ok(Err(HealingReviewError::Conflict(
            "This correction was already undone",
        )));
    }
    let kind = match CorrectionKind::parse(&kind) {
        Ok(kind) => kind,
        Err(error) => return Ok(Err(error)),
    };
    if let Some(refusal) = session_refusal(tx, &session_id)? {
        return Ok(Err(refusal));
    }
    if let Some(refusal) = running_session_refusal(tx, &activation_id, Some(correction_id))? {
        return Ok(Err(refusal));
    }

    tx.execute(
        "UPDATE healing_outputs SET \
             classification = COALESCE(prior_classification, classification), \
             activation_id = prior_activation_id, \
             effect_window_id = prior_effect_window_id, \
             reason = COALESCE(prior_reason, reason), \
             correction_id = NULL, prior_classification = NULL, prior_activation_id = NULL, \
             prior_effect_window_id = NULL, prior_reason = NULL \
         WHERE correction_id = ?1",
        [correction_id],
    )?;
    // Undoing "not a paid use" makes the activation live again; undoing a
    // "paid use" supersedes the activation it minted. Either way the rows
    // stay, and the correction keeps when it was undone.
    let superseded_at = match kind {
        CorrectionKind::NotPaidUse => None,
        CorrectionKind::PaidUse => Some(now),
    };
    tx.execute(
        "UPDATE healing_activations SET superseded_at = ?1 WHERE id = ?2",
        rusqlite::params![superseded_at, activation_id],
    )?;
    tx.execute(
        "UPDATE healing_effect_windows SET superseded_at = ?1 WHERE activation_id = ?2",
        rusqlite::params![superseded_at, activation_id],
    )?;
    crate::consumables::set_activation_doses_removed(tx, &activation_id, superseded_at)?;
    tx.execute(
        "UPDATE healing_corrections SET undone_at = ?1 WHERE id = ?2",
        rusqlite::params![now, correction_id],
    )?;
    adjust_session_heal(tx, &session_id, -cost_delta)?;
    Ok(Ok(session_id))
}
