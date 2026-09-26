//! Dose persistence: the reads every surface shares, and the write steps the
//! tracker composes inside its own transactions.

use rusqlite::{Connection, OptionalExtension};

use crate::db::DbError;

use super::board::{DoseSource, LiveDose};

use super::profile::DoseEffect;

/// Who removed a dose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoseRemoval {
    /// The player, from the overlay or History (a misclicked key).
    Player,
    /// A healing correction that said the heal opening it was not a paid
    /// use; only undoing that correction brings it back.
    HealCorrection,
}

impl DoseRemoval {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::HealCorrection => "heal_correction",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "player" => Self::Player,
            "heal_correction" => Self::HealCorrection,
            _ => return None,
        })
    }
}

/// One stored dose, as review and the readouts show it.
#[derive(Debug, Clone, PartialEq)]
pub struct DoseRecord {
    pub id: String,
    pub equipment_id: Option<i64>,
    pub item_name: String,
    pub source: DoseSource,
    pub session_id: Option<String>,
    pub context_id: Option<i64>,
    pub interval_id: Option<i64>,
    pub started_at: f64,
    pub expires_at: f64,
    pub cost_ped: f64,
    pub cost_tracked: bool,
    pub effects: Vec<DoseEffect>,
    pub healing_activation_id: Option<String>,
    pub supersedes_dose_id: Option<String>,
    pub superseded_at: Option<f64>,
    pub removed_at: Option<f64>,
    pub removed_by: Option<DoseRemoval>,
}

impl DoseRecord {
    /// When the dose's effect ended or will end: its expiry, or earlier when
    /// a re-dose of the same item replaced it.
    pub fn ends_at(&self) -> f64 {
        self.superseded_at
            .map_or(self.expires_at, |at| at.min(self.expires_at))
    }

    /// Whether the dose's effect is in force at `now`.
    pub fn in_effect_at(&self, now: f64) -> bool {
        self.removed_at.is_none() && self.started_at <= now && now < self.ends_at()
    }

    pub fn live(&self) -> LiveDose {
        LiveDose {
            id: self.id.clone(),
            equipment_id: self.equipment_id,
            item_name: self.item_name.clone(),
            source: self.source,
            session_id: self.session_id.clone(),
            started_at: self.started_at,
            expires_at: self.ends_at(),
            cost_ped: self.cost_ped,
            effects: self.effects.clone(),
        }
    }
}

const DOSE_COLUMNS: &str = "id, equipment_id, item_name, source, session_id, context_id, \
     interval_id, started_at, expires_at, cost_ped, cost_tracked, effects_json, \
     healing_activation_id, supersedes_dose_id, superseded_at, removed_at, removed_by";

fn dose_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DoseRecord> {
    let source: String = row.get(3)?;
    let effects: String = row.get(11)?;
    let removed_by: Option<String> = row.get(16)?;
    Ok(DoseRecord {
        id: row.get(0)?,
        equipment_id: row.get(1)?,
        item_name: row.get(2)?,
        // The column's CHECK admits only the three sources.
        source: DoseSource::parse(&source).unwrap_or(DoseSource::Manual),
        session_id: row.get(4)?,
        context_id: row.get(5)?,
        interval_id: row.get(6)?,
        started_at: row.get(7)?,
        expires_at: row.get(8)?,
        cost_ped: row.get(9)?,
        cost_tracked: row.get::<_, i64>(10)? != 0,
        // An unreadable effect list reads as none: the dose still books and
        // shows, it just adds no reload speed.
        effects: serde_json::from_str(&effects).unwrap_or_default(),
        healing_activation_id: row.get(12)?,
        supersedes_dose_id: row.get(13)?,
        superseded_at: row.get(14)?,
        removed_at: row.get(15)?,
        removed_by: removed_by.as_deref().and_then(DoseRemoval::parse),
    })
}

fn query_doses(
    conn: &Connection,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<DoseRecord>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOSE_COLUMNS} FROM consumable_doses {filter} ORDER BY started_at, id"
    ))?;
    let rows = stmt.query_map(params, dose_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The doses whose effect has not ended at `now`: what the board carries.
pub fn read_running_doses(conn: &Connection, now: f64) -> Result<Vec<DoseRecord>, DbError> {
    query_doses(
        conn,
        "WHERE removed_at IS NULL AND superseded_at IS NULL AND expires_at > ?1",
        [now],
    )
}

/// The doses not removed whose effect ended no earlier than `since`: the
/// live ones plus the recently expired, which the readouts offer to re-dose.
pub fn read_recent_doses(conn: &Connection, since: f64) -> Result<Vec<DoseRecord>, DbError> {
    query_doses(
        conn,
        "WHERE removed_at IS NULL AND superseded_at IS NULL AND expires_at >= ?1",
        [since],
    )
}

/// Every dose taken in a session, removed ones included.
pub fn read_session_doses(conn: &Connection, session_id: &str) -> Result<Vec<DoseRecord>, DbError> {
    query_doses(conn, "WHERE session_id = ?1", [session_id])
}

pub fn read_dose(conn: &Connection, id: &str) -> Result<Option<DoseRecord>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DOSE_COLUMNS} FROM consumable_doses WHERE id = ?1"),
            [id],
            dose_from_row,
        )
        .optional()?)
}

/// The dose of this item still running at `at` (at most one: a re-dose
/// ends the earlier one).
pub fn read_running_dose_of(
    conn: &Connection,
    equipment_id: i64,
    at: f64,
) -> Result<Option<DoseRecord>, DbError> {
    Ok(query_doses(
        conn,
        "WHERE equipment_id = ?1 AND removed_at IS NULL AND superseded_at IS NULL \
           AND started_at <= ?2 AND expires_at > ?2",
        rusqlite::params![equipment_id, at],
    )?
    .pop())
}

/// The doses a paid heal opened.
pub fn read_doses_of_activation(
    conn: &Connection,
    activation_id: &str,
) -> Result<Vec<DoseRecord>, DbError> {
    query_doses(conn, "WHERE healing_activation_id = ?1", [activation_id])
}

/// Write a new dose.
pub fn insert_dose(conn: &Connection, dose: &DoseRecord) -> Result<(), DbError> {
    let effects = serde_json::to_string(&dose.effects).expect("dose effects serialise");
    conn.execute(
        &format!(
            "INSERT INTO consumable_doses ({DOSE_COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ),
        rusqlite::params![
            dose.id,
            dose.equipment_id,
            dose.item_name,
            dose.source.as_str(),
            dose.session_id,
            dose.context_id,
            dose.interval_id,
            dose.started_at,
            dose.expires_at,
            dose.cost_ped,
            i64::from(dose.cost_tracked),
            effects,
            dose.healing_activation_id,
            dose.supersedes_dose_id,
            dose.superseded_at,
            dose.removed_at,
            dose.removed_by.map(DoseRemoval::as_str),
        ],
    )?;
    Ok(())
}

/// End a running dose early (a re-dose of its item at `at`), or give an
/// ended one its full run back with `None`.
pub fn set_superseded(conn: &Connection, id: &str, at: Option<f64>) -> Result<(), DbError> {
    conn.execute(
        "UPDATE consumable_doses SET superseded_at = ?1 WHERE id = ?2",
        rusqlite::params![at, id],
    )?;
    Ok(())
}

/// Mark a dose removed, or restore it with `None`.
pub fn set_removed(
    conn: &Connection,
    id: &str,
    removed: Option<(f64, DoseRemoval)>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE consumable_doses SET removed_at = ?1, removed_by = ?2 WHERE id = ?3",
        rusqlite::params![
            removed.map(|(at, _)| at),
            removed.map(|(_, by)| by.as_str()),
            id
        ],
    )?;
    Ok(())
}

/// Point a dose at the context interval now standing for it.
pub fn set_interval(conn: &Connection, id: &str, interval_id: Option<i64>) -> Result<(), DbError> {
    conn.execute(
        "UPDATE consumable_doses SET interval_id = ?1 WHERE id = ?2",
        rusqlite::params![interval_id, id],
    )?;
    Ok(())
}

/// Move a session's consumed-dose cost by `delta`. An ended session's
/// summary, days, and settled cells are repaired with it; a running
/// session's are written when it stops, as every other bucket's are.
pub fn adjust_session_consumable_cost(
    conn: &Connection,
    session_id: &str,
    delta: f64,
) -> Result<(), DbError> {
    let ended: Option<bool> = conn
        .query_row(
            "SELECT ended_at IS NOT NULL AND is_active = 0 FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(ended) = ended else {
        return Err(DbError::from(rusqlite::Error::QueryReturnedNoRows));
    };
    conn.execute(
        "UPDATE tracking_sessions SET consumable_cost = \
             CASE WHEN abs(COALESCE(consumable_cost, 0) + ?1) < 1e-9 THEN 0 \
                  ELSE COALESCE(consumable_cost, 0) + ?1 END \
         WHERE id = ?2",
        rusqlite::params![delta, session_id],
    )?;
    if ended {
        crate::session_summary::write_session_summary(conn, session_id)?;
        crate::daily_rollup::refresh_session_days(conn, session_id)?;
        crate::session_rollup::recompute_session(conn, session_id)?;
    }
    Ok(())
}

/// Take a paid heal's on-use buffs away with it (`Some(at)`: a correction
/// said it was not a paid use), or give them back (`None`: the correction
/// was undone). A buff the heal already lost otherwise is left as it is.
pub fn set_activation_doses_removed(
    conn: &Connection,
    activation_id: &str,
    removed_at: Option<f64>,
) -> Result<(), DbError> {
    match removed_at {
        Some(at) => conn.execute(
            "UPDATE consumable_doses SET removed_at = ?1, removed_by = 'heal_correction' \
             WHERE healing_activation_id = ?2 AND removed_at IS NULL",
            rusqlite::params![at, activation_id],
        )?,
        None => conn.execute(
            "UPDATE consumable_doses SET removed_at = NULL, removed_by = NULL \
             WHERE healing_activation_id = ?1 AND removed_by = 'heal_correction'",
            [activation_id],
        )?,
    };
    Ok(())
}

/// Detach a deleted session's doses: the doses were still taken, and a
/// running one keeps its effect, but what they booked left with the
/// session.
pub fn detach_session(conn: &Connection, session_id: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE consumable_doses SET session_id = NULL, context_id = NULL, \
             interval_id = NULL, cost_ped = 0 \
         WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}
