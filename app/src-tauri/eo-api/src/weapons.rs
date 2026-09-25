//! Weapon attribution: the live decision on a standing mismatch, and the
//! post-play review of a session's stored shots with the assignment of an
//! unpriced shot and its undo.
//!
//! The vocabulary stays closed at the IPC boundary. The tracker owns the
//! live decision and the review service owns the corrections and every
//! repair they make; this facade maps their outcomes into the generated
//! frontend contract and answers a correction with the corrected session's
//! refreshed detail.

use eo_services::tracker::{MismatchDecision, WeaponDecisionError};
use eo_services::weapon_review::{
    CorrectionWeapon as ServiceCorrectionWeapon, ReviewShot, ReviewShotPage,
    ShotCandidate as ServiceCandidate, ShotGroup, WeaponReviewError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracking::SessionDetail;
use crate::{Api, ApiError, Nullable};

/// What the player decided about a standing weapon mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WeaponMismatchDecision {
    /// The damage evidence is right: record its weapon from now, and
    /// reprice the shots it plausibly fired since the last hotbar press.
    Confirm,
    /// The hotbar is right: reprice the evidence shots back to its weapon.
    Keep,
}

impl From<WeaponMismatchDecision> for MismatchDecision {
    fn from(value: WeaponMismatchDecision) -> Self {
        match value {
            WeaponMismatchDecision::Confirm => Self::Confirm,
            WeaponMismatchDecision::Keep => Self::Keep,
        }
    }
}

/// Which stored shots a review page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WeaponShotGroup {
    /// Shots no single carried weapon explained.
    Unresolved,
    /// Shots whose damage overrode the hotbar's weapon.
    Evidence,
}

impl From<WeaponShotGroup> for ShotGroup {
    fn from(value: WeaponShotGroup) -> Self {
        match value {
            WeaponShotGroup::Unresolved => Self::Unresolved,
            WeaponShotGroup::Evidence => Self::Evidence,
        }
    }
}

/// How a live decision went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WeaponReviewDecision {
    Confirmed,
    Kept,
}

/// One carried weapon as a stored shot remembers it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponShotCandidate {
    pub equipment_id: i64,
    pub name: String,
    /// Its damage band fitted the shot.
    pub fits: bool,
}

impl From<ServiceCandidate> for WeaponShotCandidate {
    fn from(value: ServiceCandidate) -> Self {
        Self {
            equipment_id: value.equipment_id,
            name: value.name,
            fits: value.fits,
        }
    }
}

/// One stored shot, as review lists it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponShot {
    pub id: String,
    pub observed_at: f64,
    /// Null for a jam, dodge, or evade: a shot with no damage figure.
    pub amount: Nullable<f64>,
    pub critical: bool,
    pub reason: String,
    /// The weapon the hotbar declared when it landed.
    pub hotbar_tool: Nullable<String>,
    /// The weapon it is priced to; null while unpriced.
    pub tool_name: Nullable<String>,
    pub cost_per_shot: f64,
    /// The weapons carried when it landed.
    pub candidates: Vec<WeaponShotCandidate>,
    /// The live assignment that priced it, which can be undone.
    pub correction_id: Nullable<String>,
    /// The decision on a mismatch that repriced it while the session ran.
    pub review_decision: Nullable<WeaponReviewDecision>,
    /// It can be assigned to a weapon: unpriced, in an ended session.
    pub correctable: bool,
}

fn review_decision(value: Option<String>) -> Result<Option<WeaponReviewDecision>, ApiError> {
    match value.as_deref() {
        None => Ok(None),
        Some("confirmed") => Ok(Some(WeaponReviewDecision::Confirmed)),
        Some("kept") => Ok(Some(WeaponReviewDecision::Kept)),
        Some(_) => Err(ApiError::invalid_state("unknown weapon decision")),
    }
}

impl TryFrom<ReviewShot> for WeaponShot {
    type Error = ApiError;

    fn try_from(value: ReviewShot) -> Result<Self, ApiError> {
        Ok(Self {
            review_decision: review_decision(value.review_decision)?.into(),
            id: value.id,
            observed_at: value.observed_at,
            amount: value.amount.into(),
            critical: value.critical,
            reason: value.reason,
            hotbar_tool: value.hotbar_tool.into(),
            tool_name: value.tool_name.into(),
            cost_per_shot: value.cost_per_shot,
            candidates: value.candidates.into_iter().map(Into::into).collect(),
            correction_id: value.correction_id.into(),
            correctable: value.correctable,
        })
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponShotPage {
    pub shots: Vec<WeaponShot>,
    /// Every stored shot of the requested group in the session.
    pub total: i64,
}

impl TryFrom<ReviewShotPage> for WeaponShotPage {
    type Error = ApiError;

    fn try_from(value: ReviewShotPage) -> Result<Self, ApiError> {
        Ok(Self {
            shots: value
                .shots
                .into_iter()
                .map(WeaponShot::try_from)
                .collect::<Result<_, _>>()?,
            total: value.total,
        })
    }
}

/// A weapon an unpriced shot could be assigned to.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeaponCorrectionWeapon {
    pub equipment_id: i64,
    pub name: String,
    /// The per-shot cost an assignment would book, at today's pricing.
    pub cost_per_shot_ped: f64,
    /// Its damage band fitted the shot when it landed.
    pub fits: bool,
}

impl From<ServiceCorrectionWeapon> for WeaponCorrectionWeapon {
    fn from(value: ServiceCorrectionWeapon) -> Self {
        Self {
            equipment_id: value.equipment_id,
            name: value.name,
            cost_per_shot_ped: value.cost_per_shot_ped,
            fits: value.fits,
        }
    }
}

fn review_error(error: WeaponReviewError) -> ApiError {
    match error {
        WeaponReviewError::Invalid(message) => ApiError::bad_request(message),
        WeaponReviewError::NotFound(message) => ApiError::not_found(message),
        WeaponReviewError::Conflict(message) => ApiError::conflict(message),
        WeaponReviewError::Db(error) => ApiError::internal("weapon review")(error),
        WeaponReviewError::Stored(message) => ApiError::invalid_state(message),
    }
}

impl Api {
    /// Decide the running session's standing weapon mismatch. False when
    /// none stands (a stale control), which changes nothing.
    pub async fn tracking_weapon_decide(
        &self,
        decision: WeaponMismatchDecision,
    ) -> Result<bool, ApiError> {
        self.tracker
            .decide_weapon_mismatch(decision.into())
            .await
            .map_err(|error| match error {
                WeaponDecisionError::NoActiveSession => ApiError::conflict(error.to_string()),
                WeaponDecisionError::NotSaved => ApiError::invalid_state(error.to_string()),
            })
    }

    /// One page of a session's stored shots of one group, oldest first.
    pub async fn weapon_shots(
        &self,
        session_id: String,
        group: WeaponShotGroup,
        offset: i64,
        limit: i64,
    ) -> Result<WeaponShotPage, ApiError> {
        self.weapon_review
            .session_shots(&session_id, group.into(), offset, limit)
            .await
            .map_err(review_error)?
            .try_into()
    }

    /// Which of these sessions still hold an unpriced shot, in the
    /// caller's order.
    pub async fn weapon_unpriced_sessions(
        &self,
        session_ids: Vec<String>,
    ) -> Result<Vec<String>, ApiError> {
        self.weapon_review
            .unpriced_sessions(session_ids)
            .await
            .map_err(review_error)
    }

    /// The weapons an unpriced shot could be assigned to, fitting ones
    /// first.
    pub async fn weapon_correction_weapons(
        &self,
        evidence_id: String,
    ) -> Result<Vec<WeaponCorrectionWeapon>, ApiError> {
        self.weapon_review
            .correction_weapons(&evidence_id)
            .await
            .map(|weapons| weapons.into_iter().map(Into::into).collect())
            .map_err(review_error)
    }

    /// Assign one unpriced shot of an ended session to a weapon; answers
    /// with the session's refreshed detail.
    pub async fn weapon_assign(
        &self,
        evidence_id: String,
        equipment_id: i64,
    ) -> Result<SessionDetail, ApiError> {
        let correction = self
            .weapon_review
            .assign(&evidence_id, equipment_id)
            .await
            .map_err(review_error)?;
        self.tracking_session_detail(correction.session_id).await
    }

    /// Undo a live assignment; answers with the session's refreshed detail.
    pub async fn weapon_assignment_undo(
        &self,
        correction_id: String,
    ) -> Result<SessionDetail, ApiError> {
        let session_id = self
            .weapon_review
            .undo(&correction_id)
            .await
            .map_err(review_error)?;
        self.tracking_session_detail(session_id).await
    }
}
