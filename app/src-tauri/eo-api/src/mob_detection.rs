//! Creature detection's review after a session: every creature and
//! suggestion the screen gave the session's kills, grouped by creature and
//! grade, and the one action that settles a group (name the creature, or
//! put back the declared mob).
//!
//! The grade stays a closed vocabulary across the boundary, and a group is
//! named back by the same typed reference the review lists it under. The
//! review service owns every check and repair; this facade maps its
//! outcomes and answers a settlement with the refreshed review.

use eo_services::mob_review::{
    DetectedCreature as ServiceCreature, DetectionGroup, DetectionGroupRef, MobReviewError,
    ReviewTarget, SettleOutcome, Settled,
};
use eo_services::tracker::MobDetectionGrade as ServiceGrade;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Api, ApiError, Nullable};

/// How sure detection was of a creature. The upper four stamped the
/// kills; the two lowest are suggestions the kills did not take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MobDetectionGrade {
    /// A listed creature, read whole and repeatedly in its own fight.
    Named,
    /// A listed creature the screen leaned to without proving it.
    Guess,
    /// A creature on no list, by the spelling read most often.
    Provisional,
    /// A creature on no list, leaned to but not proved.
    ProvisionalGuess,
    /// The listed creature read last before the kill: a suggestion only.
    LatestRead,
    /// The unlisted creature read last before the kill: a suggestion only.
    ProvisionalLatestRead,
}

impl From<ServiceGrade> for MobDetectionGrade {
    fn from(grade: ServiceGrade) -> Self {
        match grade {
            ServiceGrade::Named => Self::Named,
            ServiceGrade::Guess => Self::Guess,
            ServiceGrade::Provisional => Self::Provisional,
            ServiceGrade::ProvisionalGuess => Self::ProvisionalGuess,
            ServiceGrade::LatestRead => Self::LatestRead,
            ServiceGrade::ProvisionalLatestRead => Self::ProvisionalLatestRead,
        }
    }
}

impl From<MobDetectionGrade> for ServiceGrade {
    fn from(grade: MobDetectionGrade) -> Self {
        match grade {
            MobDetectionGrade::Named => Self::Named,
            MobDetectionGrade::Guess => Self::Guess,
            MobDetectionGrade::Provisional => Self::Provisional,
            MobDetectionGrade::ProvisionalGuess => Self::ProvisionalGuess,
            MobDetectionGrade::LatestRead => Self::LatestRead,
            MobDetectionGrade::ProvisionalLatestRead => Self::ProvisionalLatestRead,
        }
    }
}

/// A creature as a kill's stamp carries it: its display name, species and
/// maturity (both empty for a creature on no list).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCreature {
    pub name: String,
    pub species: String,
    pub maturity: String,
}

impl From<ServiceCreature> for DetectedCreature {
    fn from(creature: ServiceCreature) -> Self {
        Self {
            name: creature.name,
            species: creature.species,
            maturity: creature.maturity,
        }
    }
}

impl From<DetectedCreature> for ServiceCreature {
    fn from(creature: DetectedCreature) -> Self {
        Self {
            name: creature.name,
            species: creature.species,
            maturity: creature.maturity,
        }
    }
}

/// Which group of a session's detections: the creature, at one grade,
/// with the session's own key for a creature on no list (two such
/// creatures can share a spelling, read at different levels).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionGroupRef {
    pub grade: MobDetectionGrade,
    pub creature: DetectedCreature,
    pub provisional_key: Nullable<String>,
}

/// One group of the session's detections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionGroup {
    pub group: MobDetectionGroupRef,
    pub kill_count: i64,
    /// The text read off the plate most often for the group's kills.
    pub example_text: String,
    /// What the group's kills are recorded as now, when they agree.
    pub current_name: Nullable<String>,
    /// The player has settled the group.
    pub reviewed: bool,
}

/// Every creature and suggestion detection gave an ended session's kills,
/// the least certain first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionReview {
    pub session_id: String,
    /// The session has ended, so its detections can be settled.
    pub ended: bool,
    pub groups: Vec<MobDetectionGroup>,
}

/// What to settle a group as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionTarget {
    /// The creature named so: a listed creature as the list names it (its
    /// species and maturity, or its species alone), a name confirmed
    /// before, or a new name to keep.
    Creature { name: String },
    /// The declared mob each kill had.
    Declared,
}

/// What a group was settled as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionSettled {
    /// A creature on the bundled list (a misspelling of one is taken as it).
    Listed { creature: DetectedCreature },
    /// A name confirmed before.
    Confirmed { creature: DetectedCreature },
    /// A new name, kept from now on as a known creature.
    NewName { creature: DetectedCreature },
    /// Each kill's declared mob.
    Declared,
}

/// What settling a group came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionSettleResult {
    /// The group was settled; the session's review as it now stands.
    Settled {
        settled: MobDetectionSettled,
        review: MobDetectionReview,
    },
    /// The name typed came only loosely near this creature: nothing was
    /// changed, and the player may choose it in one action.
    DidYouMean { creature: DetectedCreature },
}

fn review_error(error: MobReviewError) -> ApiError {
    match error {
        MobReviewError::Invalid(message) => ApiError::bad_request(message),
        MobReviewError::NotFound(message) => ApiError::not_found(message),
        MobReviewError::Conflict(message) => ApiError::conflict(message),
        MobReviewError::Db(error) => ApiError::internal("mob detection review")(error),
        MobReviewError::Corrupt(detail) => ApiError::internal("mob detection review")(detail),
    }
}

fn group_view(group: DetectionGroup) -> MobDetectionGroup {
    MobDetectionGroup {
        group: MobDetectionGroupRef {
            grade: group.group.grade.into(),
            creature: group.group.creature.into(),
            provisional_key: group.group.provisional_key.into(),
        },
        kill_count: group.kill_count,
        example_text: group.example_text,
        current_name: group.current_name.into(),
        reviewed: group.reviewed,
    }
}

impl Api {
    /// The creatures and suggestions detection gave a session's kills.
    pub async fn tracking_mob_detections(
        &self,
        session_id: String,
    ) -> Result<MobDetectionReview, ApiError> {
        let review = self
            .mob_review
            .detections(&session_id)
            .await
            .map_err(review_error)?;
        Ok(MobDetectionReview {
            session_id,
            ended: review.ended,
            groups: review.groups.into_iter().map(group_view).collect(),
        })
    }

    /// Settle every kill of one group of an ended session's detections.
    pub async fn tracking_mob_detection_settle(
        &self,
        session_id: String,
        group: MobDetectionGroupRef,
        target: MobDetectionTarget,
    ) -> Result<MobDetectionSettleResult, ApiError> {
        let group = DetectionGroupRef {
            grade: group.grade.into(),
            creature: group.creature.into(),
            provisional_key: group.provisional_key.0,
        };
        let target = match target {
            MobDetectionTarget::Creature { name } => ReviewTarget::Creature(name),
            MobDetectionTarget::Declared => ReviewTarget::Declared,
        };
        let outcome = self
            .mob_review
            .settle(&session_id, group, target)
            .await
            .map_err(review_error)?;
        let settled = match outcome {
            SettleOutcome::DidYouMean(creature) => {
                return Ok(MobDetectionSettleResult::DidYouMean {
                    creature: creature.into(),
                })
            }
            SettleOutcome::Settled(settled) => settled,
        };
        let settled = match settled {
            Settled::Listed(creature) => MobDetectionSettled::Listed {
                creature: creature.into(),
            },
            Settled::Confirmed(creature) => MobDetectionSettled::Confirmed {
                creature: creature.into(),
            },
            Settled::NewName(creature) => MobDetectionSettled::NewName {
                creature: creature.into(),
            },
            Settled::Declared => MobDetectionSettled::Declared,
        };
        Ok(MobDetectionSettleResult::Settled {
            settled,
            review: self.tracking_mob_detections(session_id).await?,
        })
    }
}

/// Whether creature detection can capture the game window when a session
/// wants it, as the settings show it beside the toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MobDetectionCapture {
    /// Capture is running, needs nothing started, or will be asked for at
    /// the next look after the player turned detection on.
    Ready,
    /// Capture needs starting and is not running.
    NotRunning,
    /// The desktop refused capture, or it stopped while watching.
    Refused,
    /// Detection cannot run on this installation (no recogniser or
    /// interface profile was loaded).
    Unavailable,
}

impl From<eo_services::mob_watch::CaptureStatus> for MobDetectionCapture {
    fn from(status: eo_services::mob_watch::CaptureStatus) -> Self {
        use eo_services::mob_watch::CaptureStatus;
        match status {
            CaptureStatus::Ready => Self::Ready,
            CaptureStatus::NotRunning => Self::NotRunning,
            CaptureStatus::Refused => Self::Refused,
            CaptureStatus::Unavailable => Self::Unavailable,
        }
    }
}
