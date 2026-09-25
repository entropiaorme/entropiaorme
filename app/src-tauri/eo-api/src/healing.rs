//! Healing review: a session's healing outputs by classification, the
//! healing items an output could be corrected to, and the two corrections
//! with their undo.
//!
//! The vocabulary stays closed at the IPC boundary. The service owns the
//! corrections and every repair they make; this facade maps its outcomes
//! into the generated frontend contract and answers a correction with the
//! corrected session's refreshed detail.

use eo_services::healing_review::{
    CorrectionKind as ServiceCorrectionKind, CorrectionTarget as ServiceCorrectionTarget,
    CorrectionTool as ServiceCorrectionTool, HealingOutput as ServiceOutput,
    HealingOutputPage as ServiceOutputPage, HealingReviewError,
    OutputClassification as ServiceClassification,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracking::SessionDetail;
use crate::{Api, ApiError, Nullable};

/// How one healing output is explained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealingOutputClassification {
    /// It confirmed a paid activation.
    Direct,
    /// A tick of an effect an activation already paid for.
    Effect,
    /// Correlated with damage dealt: likely lifesteal.
    Passive,
    /// Nothing explains it; it carries no cost.
    Unattributed,
}

impl From<HealingOutputClassification> for ServiceClassification {
    fn from(value: HealingOutputClassification) -> Self {
        match value {
            HealingOutputClassification::Direct => Self::Direct,
            HealingOutputClassification::Effect => Self::Effect,
            HealingOutputClassification::Passive => Self::Passive,
            HealingOutputClassification::Unattributed => Self::Unattributed,
        }
    }
}

impl From<ServiceClassification> for HealingOutputClassification {
    fn from(value: ServiceClassification) -> Self {
        match value {
            ServiceClassification::Direct => Self::Direct,
            ServiceClassification::Effect => Self::Effect,
            ServiceClassification::Passive => Self::Passive,
            ServiceClassification::Unattributed => Self::Unattributed,
        }
    }
}

/// What a correction asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HealingCorrectionKind {
    /// A billed activation was not a paid use.
    NotPaidUse,
    /// An uncosted output was a paid use.
    PaidUse,
}

impl From<ServiceCorrectionKind> for HealingCorrectionKind {
    fn from(value: ServiceCorrectionKind) -> Self {
        match value {
            ServiceCorrectionKind::NotPaidUse => Self::NotPaidUse,
            ServiceCorrectionKind::PaidUse => Self::PaidUse,
        }
    }
}

/// The live correction that moved an activation or output, which can be
/// undone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingCorrectionRef {
    pub id: String,
    pub kind: HealingCorrectionKind,
}

/// One correction to make.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HealingCorrectionTarget {
    /// Take a billed activation back: it was not a paid use.
    #[serde(rename_all = "camelCase")]
    NotPaidUse { activation_id: String },
    /// Bill an uncosted output as one paid use of a healing item.
    #[serde(rename_all = "camelCase")]
    PaidUse {
        output_id: String,
        equipment_id: i64,
    },
}

impl From<HealingCorrectionTarget> for ServiceCorrectionTarget {
    fn from(value: HealingCorrectionTarget) -> Self {
        match value {
            HealingCorrectionTarget::NotPaidUse { activation_id } => {
                Self::NotPaidUse { activation_id }
            }
            HealingCorrectionTarget::PaidUse {
                output_id,
                equipment_id,
            } => Self::PaidUse {
                output_id,
                equipment_id,
            },
        }
    }
}

/// One healing output, as review lists it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingOutput {
    pub id: String,
    pub observed_at: f64,
    pub amount: f64,
    pub classification: HealingOutputClassification,
    pub reason: String,
    /// The healing item whose activation it confirmed or ticked for.
    pub tool_name: Nullable<String>,
    pub correction: Nullable<HealingCorrectionRef>,
    /// It can be marked as a paid use: nothing bills it already and no
    /// live correction moved it.
    pub correctable: bool,
}

impl From<ServiceOutput> for HealingOutput {
    fn from(value: ServiceOutput) -> Self {
        let correction = value
            .correction_id
            .zip(value.correction_kind)
            .map(|(id, kind)| HealingCorrectionRef {
                id,
                kind: kind.into(),
            });
        Self {
            id: value.id,
            observed_at: value.observed_at,
            amount: value.amount,
            classification: value.classification.into(),
            reason: value.reason,
            tool_name: value.tool_name.into(),
            correction: correction.into(),
            correctable: value.correctable,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingOutputPage {
    pub outputs: Vec<HealingOutput>,
    /// Every output of the requested classification in the session.
    pub total: i64,
}

impl From<ServiceOutputPage> for HealingOutputPage {
    fn from(value: ServiceOutputPage) -> Self {
        Self {
            outputs: value.outputs.into_iter().map(Into::into).collect(),
            total: value.total,
        }
    }
}

/// A healing item an output could be marked as a paid use of.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealingCorrectionTool {
    pub equipment_id: i64,
    pub name: String,
    /// The per-use cost the correction would book, at today's pricing.
    pub cost_per_use_ped: f64,
    /// The item's configured interval explains the heal's amount.
    pub fits: bool,
}

impl From<ServiceCorrectionTool> for HealingCorrectionTool {
    fn from(value: ServiceCorrectionTool) -> Self {
        Self {
            equipment_id: value.equipment_id,
            name: value.name,
            cost_per_use_ped: value.cost_per_use_ped,
            fits: value.fits,
        }
    }
}

fn healing_error(error: HealingReviewError) -> ApiError {
    match error {
        HealingReviewError::Invalid(message) => ApiError::bad_request(message),
        HealingReviewError::NotFound(message) => ApiError::not_found(message),
        HealingReviewError::Conflict(message) => ApiError::conflict(message),
        HealingReviewError::Db(error) => ApiError::internal("healing review")(error),
        HealingReviewError::Stored(message) => ApiError::invalid_state(message),
    }
}

impl Api {
    /// One page of a session's healing outputs of one classification.
    pub async fn healing_outputs(
        &self,
        session_id: String,
        classification: HealingOutputClassification,
        offset: i64,
        limit: i64,
    ) -> Result<HealingOutputPage, ApiError> {
        self.healing_review
            .session_outputs(&session_id, classification.into(), offset, limit)
            .await
            .map(Into::into)
            .map_err(healing_error)
    }

    /// The healing items an output could be marked as a paid use of.
    pub async fn healing_correction_tools(
        &self,
        output_id: String,
    ) -> Result<Vec<HealingCorrectionTool>, ApiError> {
        self.healing_review
            .correction_tools(&output_id)
            .await
            .map(|tools| tools.into_iter().map(Into::into).collect())
            .map_err(healing_error)
    }

    /// Correct one ended session's healing evidence; answers with the
    /// session's refreshed detail.
    pub async fn healing_correct(
        &self,
        target: HealingCorrectionTarget,
    ) -> Result<SessionDetail, ApiError> {
        let correction = self
            .healing_review
            .correct(target.into())
            .await
            .map_err(healing_error)?;
        self.tracking_session_detail(correction.session_id).await
    }

    /// Undo a live healing correction; answers with the session's refreshed
    /// detail.
    pub async fn healing_correction_undo(
        &self,
        correction_id: String,
    ) -> Result<SessionDetail, ApiError> {
        let session_id = self
            .healing_review
            .undo(&correction_id)
            .await
            .map_err(healing_error)?;
        self.tracking_session_detail(session_id).await
    }
}
