//! Reviewing a session's weapon attribution after play, and pricing the
//! shots the tracker could not.
//!
//! While a session runs, hotbar intent and each carried weapon's damage band
//! attribute every shot; a shot no single carried weapon explains is
//! recorded without a price, and the session's cost leaves it out. After
//! the session ends, each such shot can be reviewed with what the tracker
//! knew about it (its amount, when it landed, the weapon the hotbar declared,
//! and the carried weapons whose band fitted it) and assigned to one weapon.
//!
//! An assignment prices the shot from the chosen weapon as it is configured
//! now (the correction's pricing snapshot), moves it out of its kill's
//! unpriced phase into that weapon's, and repairs the kill's cost (or the
//! session's dangling cost, for a shot after the last kill), the session
//! summary, its days, and its settled cells in one transaction. It never
//! deletes: undoing it returns the shot to unpriced exactly, and the
//! correction row stays as provenance with the time it was undone. A shot
//! carries at most one live correction. Every committed change announces
//! itself through `changed`, so open surfaces re-read what they show.

mod correct;
mod read;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::time::{instant_to_epoch, resolve_local};

pub use read::session_detail_block;

/// Which stored shots a review page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotGroup {
    /// Shots no single carried weapon explained: unpriced, or priced after.
    Unresolved,
    /// Shots whose damage overrode the hotbar's weapon.
    Evidence,
}

impl ShotGroup {
    fn attribution(self) -> &'static str {
        match self {
            ShotGroup::Unresolved => "unresolved",
            ShotGroup::Evidence => "evidence",
        }
    }
}

/// One carried weapon as a stored shot remembers it.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotCandidate {
    pub equipment_id: i64,
    pub name: String,
    /// Its damage band fitted the shot.
    pub fits: bool,
}

/// One stored shot, as review lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewShot {
    pub id: String,
    pub observed_at: f64,
    /// None for a countered shot.
    pub amount: Option<f64>,
    pub critical: bool,
    pub reason: String,
    /// The weapon the hotbar declared when it landed.
    pub hotbar_tool: Option<String>,
    /// The weapon it is priced to; None while unpriced.
    pub tool_name: Option<String>,
    pub cost_per_shot: f64,
    pub candidates: Vec<ShotCandidate>,
    /// The live correction that priced it, which can be undone.
    pub correction_id: Option<String>,
    /// The live decision that repriced it while the session ran.
    pub review_decision: Option<String>,
    /// It can be assigned to a weapon: unpriced, in an ended session.
    pub correctable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewShotPage {
    pub shots: Vec<ReviewShot>,
    /// Every stored shot of the requested group in the session.
    pub total: i64,
}

/// A weapon an unpriced shot could be assigned to.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionWeapon {
    pub equipment_id: i64,
    pub name: String,
    /// The per-shot cost an assignment would book now.
    pub cost_per_shot_ped: f64,
    /// Its damage band fitted the shot when it landed.
    pub fits: bool,
}

/// A committed assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponCorrection {
    pub id: String,
    pub session_id: String,
    pub evidence_id: String,
    pub equipment_id: i64,
    pub tool_name: String,
    pub cost_per_shot: f64,
    pub corrected_at: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum WeaponReviewError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("stored weapon evidence is invalid: {0}")]
    Stored(&'static str),
}

impl From<rusqlite::Error> for WeaponReviewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

/// Told after any weapon correction commits, so other surfaces re-read.
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

/// The most shots one review page returns.
pub const MAX_PAGE: i64 = 200;

pub struct WeaponReviewService {
    db: Db,
    clock: Arc<dyn Clock>,
    changed: Option<ChangedSink>,
}

impl WeaponReviewService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self {
            db,
            clock,
            changed: None,
        }
    }

    /// Announce every committed correction through `changed`.
    pub fn with_changed(mut self, changed: ChangedSink) -> Self {
        self.changed = Some(changed);
        self
    }

    fn notify_changed(&self) {
        if let Some(changed) = &self.changed {
            changed();
        }
    }

    fn now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// Assign one unpriced shot of an ended session to a weapon.
    pub async fn assign(
        &self,
        evidence_id: &str,
        equipment_id: i64,
    ) -> Result<WeaponCorrection, WeaponReviewError> {
        let now = self.now();
        let evidence_id = evidence_id.to_string();
        let correction = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = correct::assign(&tx, &evidence_id, equipment_id, now)?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        self.notify_changed();
        Ok(correction)
    }

    /// Undo a live assignment, returning its shot to unpriced exactly.
    /// Answers with the corrected session's id.
    pub async fn undo(&self, correction_id: &str) -> Result<String, WeaponReviewError> {
        let now = self.now();
        let correction_id = correction_id.to_string();
        let session_id = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = correct::undo(&tx, &correction_id, now)?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        self.notify_changed();
        Ok(session_id)
    }

    /// One page of a session's stored shots of one group, oldest first.
    pub async fn session_shots(
        &self,
        session_id: &str,
        group: ShotGroup,
        offset: i64,
        limit: i64,
    ) -> Result<ReviewShotPage, WeaponReviewError> {
        if offset < 0 || limit < 1 {
            return Err(WeaponReviewError::Invalid(
                "A page needs a non-negative offset and a positive limit",
            ));
        }
        let session_id = session_id.to_string();
        let limit = limit.min(MAX_PAGE);
        self.db
            .with_reader(move |conn| {
                Ok(read::session_shots(conn, &session_id, group, offset, limit))
            })
            .await?
    }

    /// Which of these sessions still hold an unpriced shot, in the caller's
    /// order: their cost leaves those shots out.
    pub async fn unpriced_sessions(
        &self,
        session_ids: Vec<String>,
    ) -> Result<Vec<String>, WeaponReviewError> {
        self.db
            .with_reader(move |conn| {
                let unpriced = read::sessions_with_unpriced_shots(conn, &session_ids)?;
                Ok(session_ids
                    .into_iter()
                    .filter(|id| unpriced.contains(id))
                    .collect())
            })
            .await
            .map_err(WeaponReviewError::from)
    }

    /// The weapons an unpriced shot could be assigned to: the ones carried
    /// when it landed that are still in Equipment, those whose band fitted
    /// it first.
    pub async fn correction_weapons(
        &self,
        evidence_id: &str,
    ) -> Result<Vec<CorrectionWeapon>, WeaponReviewError> {
        let evidence_id = evidence_id.to_string();
        self.db
            .with_reader(move |conn| Ok(read::correction_weapons(conn, &evidence_id)))
            .await?
    }
}
