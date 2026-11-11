//! The skill-scan family: the scan that collects the skills window's
//! pages by itself while the player pages through them, and the review
//! of what it read.
//!
//! Typed DTOs over the composed [`SkillScanService`]. The status is one
//! of four phases, each carrying only what that phase has: an idle scan
//! says whether one can run and why the last one stopped by itself, a
//! collecting scan how far it has come and what, if anything, stops the
//! current view being collected, a finished scan that it awaits review,
//! and an accepted one that it is being saved. Starting or cancelling
//! answers the status that results, with the reason when it was refused.
//! Accepting and rejecting name the review they settle; accepting a
//! review whose figures do not add up says so explicitly.
//!
//! [`SkillScanService`]: eo_services::skill_scan::SkillScanService

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;

use eo_services::skill_scan::{
    GuessedSkill, NearMissSkill, ScanHint, ScanProgress, ScanRefusal, ScanStatus, ScanStop,
    SkillScanReview as Review, TotalCheck, UnsavedSkill,
};

use crate::Nullable;
use crate::{Api, ApiError};

/// The skill scan's state.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum SkillScanStatus {
    /// No scan is collecting or awaiting review.
    #[serde(rename_all = "camelCase")]
    Idle {
        /// A scan can run: the recogniser and the interface data loaded.
        available: bool,
        /// Why the last scan stopped by itself, when it did.
        stopped: Nullable<SkillScanStop>,
    },
    /// The scan is collecting pages as the player shows them.
    #[serde(rename_all = "camelCase")]
    Collecting {
        /// Pages collected so far.
        pages_collected: u32,
        /// How many pages the list has, once a page has shown it.
        page_count: Nullable<u32>,
        /// The page numbers still to collect, once the count is known.
        missing_pages: Vec<u32>,
        /// What stops the view in the game being collected, when
        /// something does.
        hint: Nullable<SkillScanHint>,
    },
    /// Every page is in; the result waits for review.
    AwaitingReview,
    /// The accepted review is being saved.
    Saving,
}

/// Why a scan stopped collecting by itself, with no review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SkillScanStop {
    /// Reading the skills window failed.
    ReadFailed,
    /// Screen capture was not allowed; starting again asks once more.
    CaptureNotAllowed,
    /// A while passed with no new page.
    NoNewPage,
}

/// What stops the game's view, as it is now, being collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SkillScanHint {
    /// No game window is showing.
    GameWindowNotFound,
    /// The game window lies wholly outside what can be captured.
    GameWindowOffScreen,
    /// The game window was found but could not be captured.
    CaptureFailed,
    /// The skills window is not open, or its title is covered.
    SkillsWindowNotOpen,
    /// Another window covers part of the skills window's table.
    SkillsWindowCovered,
    /// The skills window runs off the edge of the game window.
    SkillsWindowOffScreen,
    /// The skills window shows another tab than SKILLS.
    NotSkillsTab,
    /// The skills window is too short to show a whole page.
    WindowTooShort,
    /// The page is in view but some of it did not read.
    PageNotRead,
    /// The page is from another category, or another page count, than the
    /// pages collected.
    OtherListing,
}

/// Why a scan verb was refused. A refused verb changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SkillScanRefusal {
    /// There is no review to settle.
    NoReview,
    /// The review named is not the one held: the one shown is out of date.
    StaleReview,
    /// The figures do not add up to the game's total, or could not be
    /// checked against it, and saving them anyway was not asked for.
    TotalNotConfirmed,
    /// A review is being saved.
    Saving,
    /// The scan's window is not open.
    SurfaceClosed,
    /// The scan could not start.
    WorkerUnavailable,
    /// Saving failed; the review is kept for another try.
    SaveFailed,
}

impl From<&ScanRefusal> for SkillScanRefusal {
    fn from(refusal: &ScanRefusal) -> Self {
        match refusal {
            ScanRefusal::NoReview => Self::NoReview,
            ScanRefusal::StaleReview => Self::StaleReview,
            ScanRefusal::TotalNotConfirmed => Self::TotalNotConfirmed,
            ScanRefusal::Saving => Self::Saving,
            ScanRefusal::SurfaceClosed => Self::SurfaceClosed,
            ScanRefusal::WorkerUnavailable => Self::WorkerUnavailable,
            ScanRefusal::SaveFailed(_) => Self::SaveFailed,
        }
    }
}

/// The answer to starting or cancelling: the status that results, and
/// why the verb was refused when it was.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanReply {
    pub status: SkillScanStatus,
    pub refused: Nullable<SkillScanRefusal>,
}

impl From<ScanHint> for SkillScanHint {
    fn from(hint: ScanHint) -> Self {
        match hint {
            ScanHint::GameWindowNotFound => Self::GameWindowNotFound,
            ScanHint::GameWindowOffScreen => Self::GameWindowOffScreen,
            ScanHint::CaptureFailed => Self::CaptureFailed,
            ScanHint::SkillsWindowNotOpen => Self::SkillsWindowNotOpen,
            ScanHint::SkillsWindowCovered => Self::SkillsWindowCovered,
            ScanHint::SkillsWindowOffScreen => Self::SkillsWindowOffScreen,
            ScanHint::NotSkillsTab => Self::NotSkillsTab,
            ScanHint::WindowTooShort => Self::WindowTooShort,
            ScanHint::PageNotRead => Self::PageNotRead,
            ScanHint::OtherListing => Self::OtherListing,
        }
    }
}

impl From<ScanStop> for SkillScanStop {
    fn from(stop: ScanStop) -> Self {
        match stop {
            ScanStop::ReadFailed => Self::ReadFailed,
            ScanStop::CaptureNotAllowed => Self::CaptureNotAllowed,
            ScanStop::NoNewPage => Self::NoNewPage,
        }
    }
}

impl From<ScanStatus> for SkillScanStatus {
    fn from(status: ScanStatus) -> Self {
        match status {
            ScanStatus::Idle { available, stopped } => Self::Idle {
                available,
                stopped: stopped.map(SkillScanStop::from).into(),
            },
            ScanStatus::Collecting(ScanProgress {
                collected,
                page_count,
                missing,
                hint,
            }) => Self::Collecting {
                pages_collected: collected,
                page_count: page_count.into(),
                missing_pages: missing,
                hint: hint.map(SkillScanHint::from).into(),
            },
            ScanStatus::AwaitingReview => Self::AwaitingReview,
            ScanStatus::Saving => Self::Saving,
        }
    }
}

/// A finished scan awaiting review.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanReview {
    /// The scan this review is of: accepting or rejecting it names it.
    pub scan: u64,
    /// What accepting saves: each skill's name to its points.
    pub skills: BTreeMap<String, f64>,
    /// Whether the figures add up to the game's own total.
    pub total: SkillScanTotal,
    /// Saved skills whose names were not read exactly.
    pub near_misses: Vec<SkillScanNearMiss>,
    /// Names this version does not know that are probably one of its
    /// skills; not saved.
    pub guesses: Vec<SkillScanGuess>,
    /// Skills the game shows that this version does not know; not saved.
    pub unknown: Vec<SkillScanUnsaved>,
    /// Rows whose name could be either of two skills; not saved.
    pub unplaced: Vec<SkillScanUnsaved>,
    /// Skills read more than once; not saved.
    pub repeated: Vec<String>,
}

/// Whether a scan's figures add up to the game's own total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "check", rename_all = "camelCase")]
pub enum SkillScanTotal {
    /// They add up.
    Matches,
    /// They do not: `sum` is what the figures read add up to, `total`
    /// what the game shows.
    Mismatch { sum: u64, total: u64 },
    /// There was nothing to check them against.
    Unchecked,
}

/// A saved skill whose name was not read exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanNearMiss {
    /// The skill it was saved as.
    pub skill: String,
    /// What its name was read as.
    pub read_as: String,
}

/// A name this version does not know that is probably one of its skills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanGuess {
    /// What its name was read as.
    pub read_as: String,
    /// The skill it probably is.
    pub probably: String,
    /// The points exactly as the game prints them.
    pub points_text: String,
}

/// A skill the game shows that the scan does not save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillScanUnsaved {
    /// The name as it was read.
    pub name: String,
    /// The points exactly as the game prints them.
    pub points_text: String,
}

impl From<&UnsavedSkill> for SkillScanUnsaved {
    fn from(skill: &UnsavedSkill) -> Self {
        Self {
            name: skill.name.clone(),
            points_text: skill.points_text.clone(),
        }
    }
}

impl From<&NearMissSkill> for SkillScanNearMiss {
    fn from(skill: &NearMissSkill) -> Self {
        Self {
            skill: skill.skill.clone(),
            read_as: skill.read_as.clone(),
        }
    }
}

impl From<&GuessedSkill> for SkillScanGuess {
    fn from(skill: &GuessedSkill) -> Self {
        Self {
            read_as: skill.read_as.clone(),
            probably: skill.probably.clone(),
            points_text: skill.points_text.clone(),
        }
    }
}

impl From<TotalCheck> for SkillScanTotal {
    fn from(check: TotalCheck) -> Self {
        match check {
            TotalCheck::Matches => Self::Matches,
            TotalCheck::Mismatch { sum, total } => Self::Mismatch { sum, total },
            TotalCheck::Unchecked => Self::Unchecked,
        }
    }
}

impl From<&Review> for SkillScanReview {
    fn from(review: &Review) -> Self {
        Self {
            scan: review.scan,
            skills: review
                .skills
                .iter()
                .map(|scanned| (scanned.skill.clone(), scanned.level))
                .collect(),
            total: review.total.into(),
            near_misses: review.near_misses.iter().map(Into::into).collect(),
            guesses: review.guesses.iter().map(Into::into).collect(),
            unknown: review.unknown.iter().map(Into::into).collect(),
            unplaced: review.unplaced.iter().map(Into::into).collect(),
            repeated: review.repeated.clone(),
        }
    }
}

/// The accept verb's result: `{ok, skills_persisted}` on success; on a
/// refusal, why (`refused`) and the message to show (`error`). Each key is
/// skipped when absent.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AcceptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_persisted: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<SkillScanRefusal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The reject verb's result: `{ok: true}` on success; on a refusal, why
/// and the message to show.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RejectResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<SkillScanRefusal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Api {
    /// The scan's current status.
    pub fn scan_status(&self) -> Result<SkillScanStatus, ApiError> {
        Ok(self.skill_scan.status().into())
    }

    fn reply(&self, answer: Result<ScanStatus, ScanRefusal>) -> SkillScanReply {
        match answer {
            Ok(status) => SkillScanReply {
                status: status.into(),
                refused: None.into(),
            },
            Err(refusal) => SkillScanReply {
                status: self.skill_scan.status().into(),
                refused: Some(SkillScanRefusal::from(&refusal)).into(),
            },
        }
    }

    /// Start collecting. A scan already under way is left as it is, and
    /// none starts when the reader is unavailable; refused while a review
    /// is being saved or the scan's window is closed.
    pub fn scan_start(&self) -> Result<SkillScanReply, ApiError> {
        Ok(self.reply(self.skill_scan.start()))
    }

    /// Stop collecting; refused while a review is being saved.
    pub fn scan_cancel(&self) -> Result<SkillScanReply, ApiError> {
        Ok(self.reply(self.skill_scan.cancel()))
    }

    /// Shell bookkeeping for the scan's floating window: it is showing, so
    /// a scan may be started from it. Not an IPC operation.
    pub fn skill_scan_window_shown_for_shell(&self) {
        self.skill_scan.surface_shown();
    }

    /// Shell teardown used when the scan's floating window is hidden or
    /// closed: the watching stops with it. A finished scan awaiting review
    /// keeps its result, since the review lives in the main window. Not an
    /// IPC operation.
    pub fn skill_scan_window_closed_for_shell(&self) {
        self.skill_scan.surface_closed();
    }

    /// Save the review of scan `scan`. One whose figures do not add up to
    /// the game's total, or could not be checked, is saved only with
    /// `save_anyway`.
    pub fn scan_accept(&self, scan: u64, save_anyway: bool) -> Result<AcceptResult, ApiError> {
        Ok(match self.skill_scan.accept(scan, save_anyway) {
            Ok(persisted) => AcceptResult {
                ok: Some(true),
                skills_persisted: Some(persisted as i64),
                refused: None,
                error: None,
            },
            Err(refusal) => AcceptResult {
                ok: None,
                skills_persisted: None,
                refused: Some((&refusal).into()),
                error: Some(refusal.to_string()),
            },
        })
    }

    /// Discard the review of scan `scan`.
    pub fn scan_reject(&self, scan: u64) -> Result<RejectResult, ApiError> {
        Ok(match self.skill_scan.reject(scan) {
            Ok(()) => RejectResult {
                ok: Some(true),
                refused: None,
                error: None,
            },
            Err(refusal) => RejectResult {
                ok: None,
                refused: Some((&refusal).into()),
                error: Some(refusal.to_string()),
            },
        })
    }

    /// The finished scan awaiting review, or `None` when none awaits.
    pub fn scan_pending(&self) -> Result<Option<SkillScanReview>, ApiError> {
        Ok(self.skill_scan.pending().as_ref().map(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eo_services::scan_completion::ScannedLevel;
    use serde_json::json;

    #[test]
    fn each_phase_carries_only_what_it_has() {
        let idle = SkillScanStatus::from(ScanStatus::Idle {
            available: true,
            stopped: Some(ScanStop::CaptureNotAllowed),
        });
        assert_eq!(
            serde_json::to_value(idle).unwrap(),
            json!({"phase": "idle", "available": true, "stopped": "captureNotAllowed"})
        );
        let collecting = SkillScanStatus::from(ScanStatus::Collecting(ScanProgress {
            collected: 7,
            page_count: Some(13),
            missing: vec![2, 9],
            hint: Some(ScanHint::WindowTooShort),
        }));
        assert_eq!(
            serde_json::to_value(collecting).unwrap(),
            json!({
                "phase": "collecting",
                "pagesCollected": 7,
                "pageCount": 13,
                "missingPages": [2, 9],
                "hint": "windowTooShort",
            })
        );
        let starting = SkillScanStatus::from(ScanStatus::Collecting(ScanProgress::default()));
        assert_eq!(
            serde_json::to_value(starting).unwrap(),
            json!({
                "phase": "collecting",
                "pagesCollected": 0,
                "pageCount": null,
                "missingPages": [],
                "hint": null,
            })
        );
        assert_eq!(
            serde_json::to_value(SkillScanStatus::from(ScanStatus::AwaitingReview)).unwrap(),
            json!({"phase": "awaitingReview"})
        );
        assert_eq!(
            serde_json::to_value(SkillScanStatus::from(ScanStatus::Saving)).unwrap(),
            json!({"phase": "saving"})
        );
    }

    #[test]
    fn every_hint_stop_and_refusal_crosses_the_wire_by_name() {
        for (hint, name) in [
            (ScanHint::GameWindowNotFound, "gameWindowNotFound"),
            (ScanHint::GameWindowOffScreen, "gameWindowOffScreen"),
            (ScanHint::CaptureFailed, "captureFailed"),
            (ScanHint::SkillsWindowNotOpen, "skillsWindowNotOpen"),
            (ScanHint::SkillsWindowCovered, "skillsWindowCovered"),
            (ScanHint::SkillsWindowOffScreen, "skillsWindowOffScreen"),
            (ScanHint::NotSkillsTab, "notSkillsTab"),
            (ScanHint::WindowTooShort, "windowTooShort"),
            (ScanHint::PageNotRead, "pageNotRead"),
            (ScanHint::OtherListing, "otherListing"),
        ] {
            assert_eq!(
                serde_json::to_value(SkillScanHint::from(hint)).unwrap(),
                json!(name)
            );
        }
        for (stop, name) in [
            (ScanStop::ReadFailed, "readFailed"),
            (ScanStop::CaptureNotAllowed, "captureNotAllowed"),
            (ScanStop::NoNewPage, "noNewPage"),
        ] {
            assert_eq!(
                serde_json::to_value(SkillScanStop::from(stop)).unwrap(),
                json!(name)
            );
        }
        for (refusal, name) in [
            (ScanRefusal::NoReview, "noReview"),
            (ScanRefusal::StaleReview, "staleReview"),
            (ScanRefusal::TotalNotConfirmed, "totalNotConfirmed"),
            (ScanRefusal::Saving, "saving"),
            (ScanRefusal::SurfaceClosed, "surfaceClosed"),
            (ScanRefusal::WorkerUnavailable, "workerUnavailable"),
            (ScanRefusal::SaveFailed("disk full".into()), "saveFailed"),
        ] {
            assert_eq!(
                serde_json::to_value(SkillScanRefusal::from(&refusal)).unwrap(),
                json!(name)
            );
        }
    }

    #[test]
    fn the_review_carries_its_scan_what_is_saved_and_what_is_not() {
        let review = Review {
            scan: 3,
            skills: vec![
                ScannedLevel {
                    skill: "Aim".into(),
                    level: 1268.53,
                    scanned_at: 100.0,
                },
                ScannedLevel {
                    skill: "Agility".into(),
                    level: 73.71,
                    scanned_at: 160.0,
                },
            ],
            total: TotalCheck::Mismatch {
                sum: 4427,
                total: 4500,
            },
            near_misses: vec![NearMissSkill {
                skill: "Aim".into(),
                read_as: "Aim.".into(),
            }],
            guesses: vec![GuessedSkill {
                read_as: "Anatomv".into(),
                probably: "Anatomy".into(),
                points_text: "40.00".into(),
            }],
            unknown: vec![UnsavedSkill {
                name: "Food Technology".into(),
                points_text: "5.25".into(),
            }],
            unplaced: Vec::new(),
            repeated: vec!["Analysis".into()],
        };
        assert_eq!(
            serde_json::to_value(SkillScanReview::from(&review)).unwrap(),
            json!({
                "scan": 3,
                "skills": {"Agility": 73.71, "Aim": 1268.53},
                "total": {"check": "mismatch", "sum": 4427, "total": 4500},
                "nearMisses": [{"skill": "Aim", "readAs": "Aim."}],
                "guesses": [{"readAs": "Anatomv", "probably": "Anatomy", "pointsText": "40.00"}],
                "unknown": [{"name": "Food Technology", "pointsText": "5.25"}],
                "unplaced": [],
                "repeated": ["Analysis"],
            })
        );
        assert_eq!(
            serde_json::to_value(SkillScanTotal::from(TotalCheck::Matches)).unwrap(),
            json!({"check": "matches"})
        );
        assert_eq!(
            serde_json::to_value(SkillScanTotal::from(TotalCheck::Unchecked)).unwrap(),
            json!({"check": "unchecked"})
        );
    }
}
