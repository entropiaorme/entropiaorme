//! Reviewing and correcting a session's healing evidence after play.
//!
//! The tracker bills a heal only when a paid-healer intent and a compatible
//! observed output agree, so its mistakes are narrow and nameable: a paid
//! activation that was not really one (a heal from elsewhere that happened to
//! fit a pressed healer), or an output that was really a paid use the tracker
//! could not confirm (a healer used without its hotkey, or a tick that was
//! actually a fresh application). Each has one correction:
//!
//! - **Not a paid use** supersedes an activation and its effect window. Its
//!   cost leaves the session, and the outputs it explained return to
//!   unattributed evidence.
//! - **Paid use** mints one activation for an output, priced from the chosen
//!   healing item as it is configured now (the correction's pricing
//!   snapshot), and opens its effect window when the item has one. Later
//!   unattributed outputs inside that window which fit its ticks become its
//!   zero-cost ticks.
//!
//! A correction never deletes. It is a row of its own; every output it moves
//! keeps its previous state, so undoing the correction restores the evidence
//! exactly, and the correction stays as provenance with the time it was
//! undone. Each activation or output is moved by at most one live correction:
//! correcting it again means undoing the correction first, which keeps every
//! undo exact. Sessions are corrected only once they have ended, because a
//! running session's heal cost belongs to the tracker until it stops.
//!
//! Every correction repairs the session's heal cost, its summary, its daily
//! rollup, and its settled cells in the same transaction, and announces
//! itself through `changed` so the tracker re-reads its live effect windows
//! and open surfaces re-read what they show.

mod correct;
mod read;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::time::{instant_to_epoch, resolve_local};

/// What a correction asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionKind {
    /// A billed activation was not a paid use.
    NotPaidUse,
    /// An output the tracker left uncosted was a paid use.
    PaidUse,
}

impl CorrectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotPaidUse => "not_paid_use",
            Self::PaidUse => "paid_use",
        }
    }

    fn parse(value: &str) -> Result<Self, HealingReviewError> {
        match value {
            "not_paid_use" => Ok(Self::NotPaidUse),
            "paid_use" => Ok(Self::PaidUse),
            _ => Err(HealingReviewError::Stored(
                "unknown healing correction kind",
            )),
        }
    }
}

/// How the tracker (or a correction) explained one healing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputClassification {
    /// It confirmed a paid activation.
    Direct,
    /// A tick of an effect window some activation already paid for.
    Effect,
    /// Correlated with damage dealt: likely lifesteal.
    Passive,
    /// Nothing explains it; it carries no cost.
    Unattributed,
}

impl OutputClassification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Effect => "effect",
            Self::Passive => "passive",
            Self::Unattributed => "unattributed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, HealingReviewError> {
        match value {
            "direct" => Ok(Self::Direct),
            "effect" => Ok(Self::Effect),
            "passive" => Ok(Self::Passive),
            "unattributed" => Ok(Self::Unattributed),
            _ => Err(HealingReviewError::Stored(
                "unknown healing output classification",
            )),
        }
    }
}

/// One correction to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectionTarget {
    NotPaidUse {
        activation_id: String,
    },
    PaidUse {
        output_id: String,
        equipment_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HealingCorrection {
    pub id: String,
    pub session_id: String,
    pub kind: CorrectionKind,
    /// The activation superseded (not a paid use) or minted (paid use).
    pub activation_id: String,
    /// The output the correction started from, when it had one.
    pub output_id: Option<String>,
    pub cost_delta_ped: f64,
    pub corrected_at: f64,
    pub undone_at: Option<f64>,
}

/// One healing output, as review lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct HealingOutput {
    pub id: String,
    pub observed_at: f64,
    pub amount: f64,
    pub classification: OutputClassification,
    pub reason: String,
    /// The paid activation it confirmed or ticked for, if any.
    pub activation_id: Option<String>,
    pub tool_name: Option<String>,
    /// The live correction that last moved it.
    pub correction_id: Option<String>,
    pub correction_kind: Option<CorrectionKind>,
    /// It can be marked as a paid use: nothing already bills it and no live
    /// correction moved it.
    pub correctable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HealingOutputPage {
    pub outputs: Vec<HealingOutput>,
    /// Every output of the requested classification in the session.
    pub total: i64,
}

/// A healing item an output could be corrected to.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionTool {
    pub equipment_id: i64,
    pub name: String,
    /// The per-use cost a correction to it would book now.
    pub cost_per_use_ped: f64,
    /// The item's configured interval explains the output's amount.
    pub fits: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HealingReviewError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("stored healing data is invalid: {0}")]
    Stored(&'static str),
}

impl From<rusqlite::Error> for HealingReviewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

/// Told after any healing correction commits, so other surfaces re-read.
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

/// The most outputs one review page returns.
pub const MAX_PAGE: i64 = 200;

pub struct HealingReviewService {
    db: Db,
    clock: Arc<dyn Clock>,
    changed: Option<ChangedSink>,
}

impl HealingReviewService {
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

    /// The injected clock as the tracker stamps healing evidence.
    fn now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// Make one correction to an ended session's healing evidence.
    pub async fn correct(
        &self,
        target: CorrectionTarget,
    ) -> Result<HealingCorrection, HealingReviewError> {
        let now = self.now();
        let correction = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = correct::apply(&tx, &target, now)?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        self.notify_changed();
        Ok(correction)
    }

    /// Undo a live correction, restoring the evidence it moved exactly.
    /// Answers with the corrected session's id.
    pub async fn undo(&self, correction_id: &str) -> Result<String, HealingReviewError> {
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

    /// One page of a session's outputs of one classification, oldest first.
    pub async fn session_outputs(
        &self,
        session_id: &str,
        classification: OutputClassification,
        offset: i64,
        limit: i64,
    ) -> Result<HealingOutputPage, HealingReviewError> {
        if offset < 0 || limit < 1 {
            return Err(HealingReviewError::Invalid(
                "A page needs a non-negative offset and a positive limit",
            ));
        }
        let session_id = session_id.to_string();
        let limit = limit.min(MAX_PAGE);
        self.db
            .with_reader(move |conn| {
                Ok(read::session_outputs(
                    conn,
                    &session_id,
                    classification,
                    offset,
                    limit,
                ))
            })
            .await?
    }

    /// The healing items an output could be marked as a paid use of, the
    /// ones whose interval explains its amount first.
    pub async fn correction_tools(
        &self,
        output_id: &str,
    ) -> Result<Vec<CorrectionTool>, HealingReviewError> {
        let output_id = output_id.to_string();
        self.db
            .with_reader(move |conn| Ok(read::correction_tools(conn, &output_id)))
            .await?
    }
}
