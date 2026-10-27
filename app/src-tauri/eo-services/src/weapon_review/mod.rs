//! Reviewing a session's weapon attribution after play, and pricing the
//! shots the tracker could not.
//!
//! While a session runs, hotbar intent and each carried weapon's damage band
//! attribute every shot; a shot no single carried weapon explains is
//! recorded without a price, and the session's cost leaves it out, and the
//! ticks of a damage-over-time effect are recorded as its outcomes, at no
//! cost. After the session ends, each such stored shot can be reviewed with
//! what the tracker knew about it (its amount, when it landed, the weapon
//! the hotbar declared, the carried weapons whose band fitted it, and the
//! effects open then) and corrected:
//!
//! - an unresolved shot, or a tick the player says was a paid shot after
//!   all, is assigned to one carried weapon;
//! - an unresolved hit an open effect could equally have ticked is marked
//!   as that effect's tick, and stops counting as a shot.
//!
//! An assignment prices the shot from the chosen weapon as it is configured
//! now (the correction's pricing snapshot), prepared by the same `pricing`
//! the live tracker's props pass through, under the reload speed in effect
//! when the shot was charged (a dose may have changed it since), so a
//! review never prices a shot differently from how play would have. Every correction moves the shot
//! between its kill's phases (or in or out of them), and repairs the kill's
//! cost and shot count (or the session's dangling cost, for a shot after
//! the last kill), the session summary, its days, and its settled cells in
//! one transaction. It never deletes: undoing it returns the shot exactly to
//! where it stood, and the correction row stays as provenance with the time
//! it was undone. A shot carries at most one live correction. Every
//! committed change announces itself through `changed`, so open surfaces
//! re-read what they show.
//!
//! A session's unpriced shots can also be assigned all at once to one
//! weapon: every one that weapon fitted, through the same per-shot
//! correction, each keeping its own undoable row, tied together as a batch
//! that can be undone as a whole. Until they are assigned, unpriced shots
//! cost nothing in the session's cost, and the exposure read bounds what
//! they could have cost: each at the dearest weapon that fitted it.

mod correct;
mod read;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::attack_rate::WeaponPricing;
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
    /// Ticks of a damage-over-time effect an earlier paid hit started.
    EffectTick,
}

impl ShotGroup {
    fn attribution(self) -> &'static str {
        match self {
            ShotGroup::Unresolved => "unresolved",
            ShotGroup::Evidence => "evidence",
            ShotGroup::EffectTick => "effect_tick",
        }
    }
}

/// What a correction did to its shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponCorrectionKind {
    /// Priced it from a carried weapon.
    Priced,
    /// Marked it as a tick of an open effect.
    EffectTick,
}

impl WeaponCorrectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Priced => "priced",
            Self::EffectTick => "effect_tick",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "priced" => Some(Self::Priced),
            "effect_tick" => Some(Self::EffectTick),
            _ => None,
        }
    }
}

/// One open effect as a stored shot remembers it: the effect the shot was
/// (or may have been) a tick of.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectCandidate {
    pub window_id: String,
    pub tool_name: String,
    /// When the paid hit that started it landed.
    pub activated_at: f64,
    /// The effect still stands: its window exists and no decision took it
    /// back, so a hit can be marked as its tick. Read, never stored.
    #[serde(default)]
    pub standing: bool,
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
    /// How it was classified as it landed.
    pub group: ShotGroup,
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
    /// The open effects that explained it when it landed.
    pub effect_candidates: Vec<EffectCandidate>,
    /// The one effect a tick belongs to; None when several explained it.
    pub effect_window_id: Option<String>,
    /// The live correction, which can be undone.
    pub correction_id: Option<String>,
    pub correction_kind: Option<WeaponCorrectionKind>,
    /// For a live effect-tick correction, the effect it named.
    pub correction_window_id: Option<String>,
    /// The live decision that repriced it while the session ran.
    pub review_decision: Option<String>,
    /// It can be corrected: without a price or correction, in an ended
    /// session.
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

/// What a session's unpriced shots could have cost at most. The session's
/// cost leaves them out; this bounds the gap without guessing a price.
#[derive(Debug, Clone, PartialEq)]
pub struct UnpricedExposure {
    pub session_id: String,
    /// Shots still without a price or correction.
    pub shots: i64,
    /// Of those, the ones no carried weapon can price: none fitted them (a
    /// countered shot fits none), or the ones that did have left Equipment.
    pub no_fit_shots: i64,
    /// Each other shot priced at the dearest weapon that fitted it, as an
    /// assignment would price it now.
    pub cost_bound_ped: f64,
}

/// One weapon a session's unpriced shots could all be assigned to at once.
#[derive(Debug, Clone, PartialEq)]
pub struct BulkAssignOption {
    pub equipment_id: i64,
    pub name: String,
    /// The unpriced shots its damage band fitted: the ones it would price.
    pub fitting_shots: i64,
    /// The session's other unpriced shots, which it would leave.
    pub skipped_shots: i64,
    /// What pricing the fitting shots would add to the weapon cost, priced
    /// as the weapon is configured now.
    pub added_cost_ped: f64,
}

/// What a bulk assignment did.
#[derive(Debug, Clone, PartialEq)]
pub struct BulkAssignment {
    pub session_id: String,
    /// The batch its corrections share; None when it priced nothing.
    pub batch_id: Option<String>,
    pub assigned: i64,
    /// Unpriced shots it left: the weapon did not fit them, or one was
    /// corrected meanwhile.
    pub skipped: i64,
    pub added_cost_ped: f64,
}

/// What undoing a batch did.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchUndo {
    pub session_id: String,
    /// Its corrections still live, now undone; any undone singly before
    /// are left as they are.
    pub undone: i64,
}

/// A committed correction.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponCorrection {
    pub id: String,
    pub session_id: String,
    pub evidence_id: String,
    pub kind: WeaponCorrectionKind,
    /// The weapon priced from, or the one whose effect claims the tick
    /// (None when that weapon has left Equipment).
    pub equipment_id: Option<i64>,
    pub tool_name: String,
    /// Zero for an effect tick.
    pub cost_per_shot: f64,
    /// The effect an effect-tick correction names.
    pub effect_window_id: Option<String>,
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

/// Unresolved shots landing with no weapon declared before a session counts
/// as having run undeclared: past what a few stray hits before the first
/// press can reach, well short of a stretch of play. The live warning and
/// the review's record share it.
pub const UNDECLARED_WARNING_SHOTS: i64 = 25;

/// Shots one bulk transaction corrects. Each transaction commits whole
/// shots or none, and the writer is free between them, so a bulk change of
/// thousands of shots never holds play's writes back for long.
const BULK_CHUNK: usize = 250;

pub struct WeaponReviewService {
    db: Db,
    clock: Arc<dyn Clock>,
    changed: Option<ChangedSink>,
    pricing: Option<WeaponPricing>,
}

impl WeaponReviewService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self {
            db,
            clock,
            changed: None,
            pricing: None,
        }
    }

    /// Prepare every weapon's props through `pricing` before pricing them;
    /// unset, stored props price as they are.
    pub fn with_pricing(mut self, pricing: WeaponPricing) -> Self {
        self.pricing = Some(pricing);
        self
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

    /// Assign one shot of an ended session left without a price (an
    /// unresolved shot, or an effect tick) to a weapon.
    pub async fn assign(
        &self,
        evidence_id: &str,
        equipment_id: i64,
    ) -> Result<WeaponCorrection, WeaponReviewError> {
        let now = self.now();
        let evidence_id = evidence_id.to_string();
        let pricing = self.pricing.clone();
        let correction = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome =
                    correct::assign(&tx, &evidence_id, equipment_id, now, pricing.as_ref())?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        self.notify_changed();
        Ok(correction)
    }

    /// Mark an unresolved hit of an ended session as a tick of an effect
    /// that was open and explained it when it landed.
    pub async fn mark_effect_tick(
        &self,
        evidence_id: &str,
        window_id: &str,
    ) -> Result<WeaponCorrection, WeaponReviewError> {
        let now = self.now();
        let evidence_id = evidence_id.to_string();
        let window_id = window_id.to_string();
        let correction = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = correct::mark_effect_tick(&tx, &evidence_id, &window_id, now)?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        self.notify_changed();
        Ok(correction)
    }

    /// Undo a live correction, returning its shot exactly to where it
    /// stood. Answers with the corrected session's id.
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

    /// The sessions among these still holding an unpriced shot, in the
    /// caller's order, each with the most those shots could have cost.
    pub async fn unpriced_exposure(
        &self,
        session_ids: Vec<String>,
    ) -> Result<Vec<UnpricedExposure>, WeaponReviewError> {
        let pricing = self.pricing.clone();
        self.db
            .with_reader(move |conn| {
                let mut exposures = read::unpriced_exposure(conn, &session_ids, pricing.as_ref());
                if let Ok(exposures) = &mut exposures {
                    let order: std::collections::HashMap<&str, usize> = session_ids
                        .iter()
                        .enumerate()
                        .map(|(index, id)| (id.as_str(), index))
                        .collect();
                    exposures.sort_by_key(|exposure| order.get(exposure.session_id.as_str()));
                }
                Ok(exposures)
            })
            .await?
    }

    /// The weapons a session's unpriced shots could all be assigned to at
    /// once, the one fitting the most first.
    pub async fn bulk_assign_options(
        &self,
        session_id: &str,
    ) -> Result<Vec<BulkAssignOption>, WeaponReviewError> {
        let session_id = session_id.to_string();
        let pricing = self.pricing.clone();
        self.db
            .with_reader(move |conn| {
                Ok(read::bulk_assign_options(
                    conn,
                    &session_id,
                    pricing.as_ref(),
                ))
            })
            .await?
    }

    /// Assign every unpriced shot of an ended session that the weapon's band
    /// fitted to the weapon, each through the single-shot correction, as one
    /// batch. Shots it did not fit are left and counted as skipped; running
    /// it again assigns nothing new.
    ///
    /// The shots are corrected a chunk per transaction: a failure part-way
    /// leaves every shot either fully corrected (an earlier chunk, undoable
    /// with the rest of the batch) or untouched.
    pub async fn assign_fitting(
        &self,
        session_id: &str,
        equipment_id: i64,
    ) -> Result<BulkAssignment, WeaponReviewError> {
        let now = self.now();
        let owned_session = session_id.to_string();
        let pricing = self.pricing.clone();
        let (fitting, unfit) = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                correct::fitting_shots(&tx, &owned_session, equipment_id, pricing.as_ref())
            })
            .await??;
        let mut done = BulkAssignment {
            session_id: session_id.to_string(),
            batch_id: None,
            assigned: 0,
            skipped: unfit,
            added_cost_ped: 0.0,
        };
        if fitting.is_empty() {
            return Ok(done);
        }
        let batch_id = uuid::Uuid::new_v4().to_string();
        done.batch_id = Some(batch_id.clone());
        for chunk in fitting.chunks(BULK_CHUNK) {
            let chunk = chunk.to_vec();
            let session_id = session_id.to_string();
            let batch_id = batch_id.clone();
            let pricing = self.pricing.clone();
            let committed = self
                .db
                .with_writer(move |conn| {
                    let tx = conn.transaction()?;
                    let mut assigned = 0;
                    let mut refused = 0;
                    let mut added = 0.0;
                    for evidence_id in &chunk {
                        match correct::assign_shot(
                            &tx,
                            evidence_id,
                            equipment_id,
                            now,
                            pricing.as_ref(),
                            Some(&batch_id),
                        )? {
                            Ok(correction) => {
                                assigned += 1;
                                added += correction.cost_per_shot;
                            }
                            // Corrected since it was listed, or no longer
                            // where its kill counts it: left as it stands.
                            Err(_) => refused += 1,
                        }
                    }
                    correct::repair(&tx, &session_id)?;
                    tx.commit()?;
                    Ok((assigned, refused, added))
                })
                .await;
            let (assigned, refused, added) = match committed {
                Ok(counts) => counts,
                Err(error) => {
                    if done.assigned > 0 {
                        self.notify_changed();
                    }
                    return Err(error.into());
                }
            };
            done.assigned += assigned;
            done.skipped += refused;
            done.added_cost_ped += added;
        }
        if done.assigned == 0 {
            done.batch_id = None;
        } else {
            self.notify_changed();
        }
        Ok(done)
    }

    /// Undo every correction of a batch still live, a chunk per
    /// transaction. Corrections already undone one by one stay as they are,
    /// so undoing a batch twice changes nothing the second time.
    pub async fn undo_batch(&self, batch_id: &str) -> Result<BatchUndo, WeaponReviewError> {
        let now = self.now();
        let owned_batch = batch_id.to_string();
        let (session_id, live) = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                correct::live_batch(&tx, &owned_batch)
            })
            .await??;
        let mut done = BatchUndo {
            session_id: session_id.clone(),
            undone: 0,
        };
        for chunk in live.chunks(BULK_CHUNK) {
            let chunk = chunk.to_vec();
            let session_id = session_id.clone();
            let committed = self
                .db
                .with_writer(move |conn| {
                    let tx = conn.transaction()?;
                    let mut undone = 0;
                    for correction_id in &chunk {
                        if correct::undo_shot(&tx, correction_id, now)?.is_ok() {
                            undone += 1;
                        }
                    }
                    correct::repair(&tx, &session_id)?;
                    tx.commit()?;
                    Ok(undone)
                })
                .await;
            match committed {
                Ok(undone) => done.undone += undone,
                Err(error) => {
                    if done.undone > 0 {
                        self.notify_changed();
                    }
                    return Err(error.into());
                }
            }
        }
        if done.undone > 0 {
            self.notify_changed();
        }
        Ok(done)
    }

    /// The weapons an unpriced shot could be assigned to: the ones carried
    /// when it landed that are still in Equipment, those whose band fitted
    /// it first.
    pub async fn correction_weapons(
        &self,
        evidence_id: &str,
    ) -> Result<Vec<CorrectionWeapon>, WeaponReviewError> {
        let evidence_id = evidence_id.to_string();
        let pricing = self.pricing.clone();
        self.db
            .with_reader(move |conn| {
                Ok(read::correction_weapons(
                    conn,
                    &evidence_id,
                    pricing.as_ref(),
                ))
            })
            .await?
    }
}
