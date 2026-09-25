//! Undoing a protection recording.
//!
//! Only the latest live recording of a stream can be undone, the way an
//! undo stack works. That keeps every stream's chain sound: an unlimited
//! repair's successor looks back to its predecessor's position, and a
//! limited reading's successor measured its loss against it, so taking
//! back an earlier link would leave a later one standing on nothing.
//!
//! Undoing never deletes. The recording and its reading are marked
//! superseded and keep their spread as provenance, while every session it
//! reached gives its share back in the same transaction, with the session
//! summaries and daily rollups repaired alongside. The stream's position
//! then falls back to the previous live recording, so the next recording
//! offers the same sessions again: correcting a recording is undoing it
//! and recording it afresh.

use rusqlite::OptionalExtension;

use super::read::{latest_live_observation_id, latest_live_repair_id};
use super::recording::adjust_session_armour;
use super::{ProtectionError, ProtectionService, UndoTarget};
use crate::db::DbError;

/// Why an undo was refused before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Missing,
    AlreadyUndone,
    NotLatest,
    ReadingMeasured,
}

impl Refusal {
    fn into_error(self) -> ProtectionError {
        match self {
            Self::Missing => ProtectionError::NotFound("Recording not found"),
            Self::AlreadyUndone => ProtectionError::Conflict("This recording was already undone"),
            Self::NotLatest => ProtectionError::Conflict(
                "Only the latest recording of each armour stream can be undone",
            ),
            Self::ReadingMeasured => {
                ProtectionError::Conflict("This reading booked a cost; undo that cost instead")
            }
        }
    }
}

/// Whether a later live recording measured its loss from `observation_id`.
/// Checked directly rather than inferred from reading times, so a clock
/// that stepped backwards can never let an undo pull a baseline out from
/// under a recording built on it.
fn reading_is_built_on(
    tx: &rusqlite::Transaction<'_>,
    observation_id: i64,
) -> Result<bool, DbError> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM protection_cost_windows \
                        WHERE opening_observation_id = ?1 AND superseded_at IS NULL)",
        [observation_id],
        |row| row.get(0),
    )?)
}

/// Supersede one cost window and hand its shares back to their sessions.
fn supersede_window(
    tx: &rusqlite::Transaction<'_>,
    window_id: i64,
    now: f64,
) -> Result<(), DbError> {
    tx.execute(
        "UPDATE protection_cost_windows SET superseded_at = ?1 WHERE id = ?2",
        rusqlite::params![now, window_id],
    )?;
    let mut stmt = tx.prepare(
        "SELECT session_id, cost_ped FROM protection_cost_allocations WHERE window_id = ?1",
    )?;
    let shares = stmt
        .query_map([window_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (session_id, cost) in shares {
        adjust_session_armour(tx, &session_id, -cost)?;
    }
    Ok(())
}

fn supersede_observation(
    tx: &rusqlite::Transaction<'_>,
    observation_id: i64,
    now: f64,
) -> Result<(), DbError> {
    tx.execute(
        "UPDATE protection_observations SET superseded_at = ?1 WHERE id = ?2",
        rusqlite::params![now, observation_id],
    )?;
    Ok(())
}

fn undo_recording(
    tx: &rusqlite::Transaction<'_>,
    window_id: i64,
    now: f64,
) -> Result<Result<(), Refusal>, DbError> {
    let Some((kind, set_id, closing, superseded)) = tx
        .query_row(
            "SELECT kind, set_id, closing_observation_id, superseded_at IS NOT NULL \
             FROM protection_cost_windows WHERE id = ?1",
            [window_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(Err(Refusal::Missing));
    };
    if superseded {
        return Ok(Err(Refusal::AlreadyUndone));
    }
    match (kind.as_str(), set_id, closing) {
        ("repair", _, _) => {
            if latest_live_repair_id(tx)? != Some(window_id) {
                return Ok(Err(Refusal::NotLatest));
            }
        }
        ("limited_decay", Some(set_id), Some(closing)) => {
            if latest_live_observation_id(tx, set_id)? != Some(closing)
                || reading_is_built_on(tx, closing)?
            {
                return Ok(Err(Refusal::NotLatest));
            }
            supersede_observation(tx, closing, now)?;
        }
        _ => return Ok(Err(Refusal::NotLatest)),
    }
    supersede_window(tx, window_id, now)?;
    Ok(Ok(()))
}

fn undo_reading(
    tx: &rusqlite::Transaction<'_>,
    observation_id: i64,
    now: f64,
) -> Result<Result<(), Refusal>, DbError> {
    let Some((set_id, superseded)) = tx
        .query_row(
            "SELECT set_id, superseded_at IS NOT NULL \
             FROM protection_observations WHERE id = ?1",
            [observation_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()?
    else {
        return Ok(Err(Refusal::Missing));
    };
    if superseded {
        return Ok(Err(Refusal::AlreadyUndone));
    }
    if latest_live_observation_id(tx, set_id)? != Some(observation_id)
        || reading_is_built_on(tx, observation_id)?
    {
        return Ok(Err(Refusal::NotLatest));
    }
    let measured: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM protection_cost_windows \
                        WHERE closing_observation_id = ?1 AND superseded_at IS NULL)",
        [observation_id],
        |row| row.get(0),
    )?;
    if measured {
        return Ok(Err(Refusal::ReadingMeasured));
    }
    supersede_observation(tx, observation_id, now)?;
    Ok(Ok(()))
}

impl ProtectionService {
    /// Undo the latest recording of one stream: an unlimited repair, a
    /// limited reading and the loss it measured, or a limited baseline.
    pub async fn undo(&self, target: UndoTarget) -> Result<(), ProtectionError> {
        let now = self.now();
        let outcome = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = match target {
                    UndoTarget::Recording { window_id } => undo_recording(&tx, window_id, now)?,
                    UndoTarget::Reading { observation_id } => {
                        undo_reading(&tx, observation_id, now)?
                    }
                };
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await?;
        outcome.map_err(Refusal::into_error)?;
        self.notify_changed();
        Ok(())
    }
}
