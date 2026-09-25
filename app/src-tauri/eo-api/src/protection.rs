//! Protection cost recording: the limited-set catalogue, the sessions a
//! recording could be spread over, and the two recording commands.
//!
//! These DTOs keep the armour/plate vocabulary closed at the IPC
//! boundary. The service owns persistence and allocation; this facade
//! maps domain outcomes into the generated frontend contract.

use eo_services::protection::{
    CandidateSession as ServiceCandidate, CostKind as ServiceCostKind,
    CostStatus as ServiceCostStatus, ObservationOutcome as ServiceObservationOutcome,
    ObservationSource as ServiceObservationSource,
    ProtectionCostAllocation as ServiceCostAllocation, ProtectionCostWindow as ServiceCostWindow,
    ProtectionError, ProtectionObservation as ServiceObservation,
    ProtectionOverview as ServiceOverview, ProtectionSet as ServiceSet,
    ProtectionSetKind as ServiceSetKind, ProtectionStream as ServiceStream,
    RecordingCandidates as ServiceCandidates, RepairOutcome as ServiceRepairOutcome,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Api, ApiError, Nullable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionSetKind {
    Armour,
    Plates,
}

impl From<ProtectionSetKind> for ServiceSetKind {
    fn from(value: ProtectionSetKind) -> Self {
        match value {
            ProtectionSetKind::Armour => Self::Armour,
            ProtectionSetKind::Plates => Self::Plates,
        }
    }
}

impl From<ServiceSetKind> for ProtectionSetKind {
    fn from(value: ServiceSetKind) -> Self {
        match value {
            ServiceSetKind::Armour => Self::Armour,
            ServiceSetKind::Plates => Self::Plates,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionObservationSource {
    Ocr,
    Manual,
}

impl From<ProtectionObservationSource> for ServiceObservationSource {
    fn from(value: ProtectionObservationSource) -> Self {
        match value {
            ProtectionObservationSource::Ocr => Self::Ocr,
            ProtectionObservationSource::Manual => Self::Manual,
        }
    }
}

impl From<ServiceObservationSource> for ProtectionObservationSource {
    fn from(value: ServiceObservationSource) -> Self {
        match value {
            ServiceObservationSource::Ocr => Self::Ocr,
            ServiceObservationSource::Manual => Self::Manual,
        }
    }
}

/// One independently recorded protection cost stream: the pooled
/// unlimited repairs, or one limited set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProtectionStream {
    Unlimited,
    #[serde(rename_all = "camelCase")]
    Limited {
        set_id: i64,
    },
}

impl From<ProtectionStream> for ServiceStream {
    fn from(value: ProtectionStream) -> Self {
        match value {
            ProtectionStream::Unlimited => Self::Unlimited,
            ProtectionStream::Limited { set_id } => Self::Limited { set_id },
        }
    }
}

impl From<ServiceStream> for ProtectionStream {
    fn from(value: ServiceStream) -> Self {
        match value {
            ServiceStream::Unlimited => Self::Unlimited,
            ServiceStream::Limited { set_id } => Self::Limited { set_id },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProtectionCostKind {
    LimitedDecay,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionCostStatus {
    /// Spread over at least one session.
    Booked,
    /// Kept as an explicit amount no session carries.
    Pending,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionObservation {
    pub id: String,
    pub set_id: String,
    pub tt_value_ped: f64,
    pub source: ProtectionObservationSource,
    pub raw_text: Nullable<String>,
    pub observed_at: f64,
    pub reset_reason: Nullable<String>,
}

/// A limited armour or plate set.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionSet {
    pub id: String,
    pub kind: ProtectionSetKind,
    pub name: String,
    pub markup_percent: f64,
    pub latest_observation: Nullable<ProtectionObservation>,
    /// The markup is frozen once the set has a reading.
    pub basis_locked: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionCostAllocation {
    pub session_id: String,
    pub hit_count: i64,
    pub allocation_share: f64,
    pub cost_ped: f64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionCostWindow {
    pub id: String,
    pub kind: ProtectionCostKind,
    pub set_id: Nullable<String>,
    pub set_name: Nullable<String>,
    pub consumed_tt_ped: Nullable<f64>,
    pub markup_percent: Nullable<f64>,
    pub cost_ped: f64,
    pub cost_known: bool,
    pub status: ProtectionCostStatus,
    pub reason: Nullable<String>,
    pub created_at: f64,
    pub allocations: Vec<ProtectionCostAllocation>,
}

/// Recorded hits no protection cost covers yet.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnrecordedProtection {
    pub sessions: i64,
    pub hits: i64,
}

/// One session's protection-cost standing.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionSessionStatus {
    /// Recorded hits no protection cost reaches yet; zero once any
    /// recording covers the session.
    pub unrecorded_hits: i64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionOverview {
    pub sets: Vec<ProtectionSet>,
    pub recent_cost_windows: Vec<ProtectionCostWindow>,
    pub unrecorded: UnrecordedProtection,
}

/// One session a recording could be spread over.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionCandidateSession {
    pub session_id: String,
    pub session_name: Nullable<String>,
    /// The session type it was played under; absent for none.
    pub definition_id: Nullable<String>,
    pub definition_name: Nullable<String>,
    pub started_at: f64,
    /// Absent while the session is still running.
    pub ended_at: Nullable<f64>,
    pub hit_count: i64,
    /// An earlier recording of the same stream already covers it.
    pub covered: bool,
}

/// What a recording of one stream would look back over.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionRecordingCandidates {
    pub stream: ProtectionStream,
    /// When the stream was last recorded (a limited set's baseline
    /// reading); absent before the first.
    pub since: Nullable<f64>,
    pub baseline_tt_ped: Nullable<f64>,
    /// Sessions with hits since the previous recording, oldest first.
    pub sessions: Vec<ProtectionCandidateSession>,
    /// Unlimited only: recent sessions from before the previous
    /// recording, which may be re-included. Oldest first.
    pub earlier: Vec<ProtectionCandidateSession>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionSetInput {
    pub kind: ProtectionSetKind,
    pub name: String,
    pub markup_percent: f64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionSetUpdateInput {
    pub name: String,
    pub markup_percent: f64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionObservationInput {
    pub set_id: i64,
    pub client_token: String,
    pub tt_value_ped: f64,
    pub source: ProtectionObservationSource,
    #[serde(default)]
    pub raw_text: Option<String>,
    #[serde(default)]
    pub reset_reason: Option<String>,
    /// The sessions a measured loss is spread over; ignored for a
    /// baseline or a reset, which measure nothing.
    #[serde(default)]
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionObservationOutcome {
    pub observation: ProtectionObservation,
    pub cost_window: Nullable<ProtectionCostWindow>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionRepairInput {
    pub client_token: String,
    pub cost_ped: f64,
    /// The sessions the repair is spread over.
    #[serde(default)]
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionRepairOutcome {
    pub cost_window: ProtectionCostWindow,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionScanResult {
    pub value_ped: Nullable<f64>,
    pub raw_text: Nullable<String>,
    pub confidence: Nullable<f64>,
    pub error: Nullable<String>,
    pub calibrated: bool,
}

impl Api {
    pub async fn protection_overview(&self) -> Result<ProtectionOverview, ApiError> {
        self.protection
            .overview()
            .await
            .map(Into::into)
            .map_err(protection_error)
    }

    pub async fn protection_set_create(
        &self,
        input: &ProtectionSetInput,
    ) -> Result<ProtectionOverview, ApiError> {
        self.protection
            .create_set(input.kind.into(), &input.name, input.markup_percent)
            .await
            .map_err(protection_error)?;
        self.protection_overview().await
    }

    pub async fn protection_set_update(
        &self,
        set_id: i64,
        input: &ProtectionSetUpdateInput,
    ) -> Result<ProtectionOverview, ApiError> {
        self.protection
            .update_set(set_id, &input.name, input.markup_percent)
            .await
            .map_err(protection_error)?;
        self.protection_overview().await
    }

    pub async fn protection_set_archive(
        &self,
        set_id: i64,
    ) -> Result<ProtectionOverview, ApiError> {
        self.protection
            .archive_set(set_id)
            .await
            .map_err(protection_error)?;
        self.protection_overview().await
    }

    pub async fn protection_session_status(
        &self,
        session_id: String,
    ) -> Result<ProtectionSessionStatus, ApiError> {
        self.protection
            .session_unrecorded_hits(&session_id)
            .await
            .map(|unrecorded_hits| ProtectionSessionStatus { unrecorded_hits })
            .map_err(protection_error)
    }

    pub async fn protection_recording_candidates(
        &self,
        stream: ProtectionStream,
    ) -> Result<ProtectionRecordingCandidates, ApiError> {
        self.protection
            .recording_candidates(stream.into())
            .await
            .map(Into::into)
            .map_err(protection_error)
    }

    pub async fn protection_observation_confirm(
        &self,
        input: &ProtectionObservationInput,
    ) -> Result<ProtectionObservationOutcome, ApiError> {
        self.protection
            .confirm_observation(
                input.set_id,
                &input.client_token,
                input.tt_value_ped,
                input.source.into(),
                input.raw_text.as_deref(),
                input.reset_reason.as_deref(),
                input.session_ids.clone(),
            )
            .await
            .map(Into::into)
            .map_err(protection_error)
    }

    pub async fn protection_repair_confirm(
        &self,
        input: &ProtectionRepairInput,
    ) -> Result<ProtectionRepairOutcome, ApiError> {
        self.protection
            .confirm_repair_cost(
                &input.client_token,
                input.cost_ped,
                input.session_ids.clone(),
            )
            .await
            .map(Into::into)
            .map_err(protection_error)
    }

    pub fn protection_trade_terminal_scan(&self) -> Result<ProtectionScanResult, ApiError> {
        let value = self.repair_ocr.scan_trade_terminal_value();
        Ok(ProtectionScanResult {
            value_ped: value
                .get("cost_ped")
                .and_then(serde_json::Value::as_f64)
                .into(),
            raw_text: value
                .get("raw_text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|text| !text.is_empty())
                .into(),
            confidence: value
                .get("confidence")
                .and_then(serde_json::Value::as_f64)
                .into(),
            error: value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .into(),
            calibrated: value
                .get("calibrated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

fn protection_error(error: ProtectionError) -> ApiError {
    match error {
        ProtectionError::Invalid(message) => ApiError::bad_request(message),
        ProtectionError::NotFound(message) => ApiError::not_found(message),
        ProtectionError::Conflict(message) => ApiError::conflict(message),
        ProtectionError::Db(error) => ApiError::internal("protection service")(error),
        ProtectionError::Stored(message) => ApiError::invalid_state(message),
    }
}

impl From<ServiceObservation> for ProtectionObservation {
    fn from(value: ServiceObservation) -> Self {
        Self {
            id: value.id.to_string(),
            set_id: value.set_id.to_string(),
            tt_value_ped: value.tt_value_ped,
            source: value.source.into(),
            raw_text: value.raw_text.into(),
            observed_at: value.observed_at,
            reset_reason: value.reset_reason.into(),
        }
    }
}

impl From<ServiceSet> for ProtectionSet {
    fn from(value: ServiceSet) -> Self {
        Self {
            id: value.id.to_string(),
            kind: value.kind.into(),
            name: value.name,
            markup_percent: value.markup_percent,
            latest_observation: value.latest_observation.map(Into::into).into(),
            basis_locked: value.basis_locked,
        }
    }
}

impl From<ServiceObservationOutcome> for ProtectionObservationOutcome {
    fn from(value: ServiceObservationOutcome) -> Self {
        Self {
            observation: value.observation.into(),
            cost_window: value.cost_window.map(Into::into).into(),
        }
    }
}

impl From<ServiceCostAllocation> for ProtectionCostAllocation {
    fn from(value: ServiceCostAllocation) -> Self {
        Self {
            session_id: value.session_id,
            hit_count: value.hit_count,
            allocation_share: value.allocation_share,
            cost_ped: value.cost_ped,
        }
    }
}

impl From<ServiceCostWindow> for ProtectionCostWindow {
    fn from(value: ServiceCostWindow) -> Self {
        Self {
            id: value.id.to_string(),
            kind: match value.kind {
                ServiceCostKind::LimitedDecay => ProtectionCostKind::LimitedDecay,
                ServiceCostKind::Repair => ProtectionCostKind::Repair,
            },
            set_id: value.set_id.map(|id| id.to_string()).into(),
            set_name: value.set_name.into(),
            consumed_tt_ped: value.consumed_tt_ped.into(),
            markup_percent: value.markup_percent.into(),
            cost_ped: value.cost_ped,
            cost_known: value.cost_known,
            status: match value.status {
                ServiceCostStatus::Booked => ProtectionCostStatus::Booked,
                ServiceCostStatus::Pending => ProtectionCostStatus::Pending,
            },
            reason: value.reason.into(),
            created_at: value.created_at,
            allocations: value.allocations.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ServiceRepairOutcome> for ProtectionRepairOutcome {
    fn from(value: ServiceRepairOutcome) -> Self {
        Self {
            cost_window: value.cost_window.into(),
        }
    }
}

impl From<ServiceCandidate> for ProtectionCandidateSession {
    fn from(value: ServiceCandidate) -> Self {
        Self {
            session_id: value.session_id,
            session_name: value.session_name.into(),
            definition_id: value.definition_id.map(|id| id.to_string()).into(),
            definition_name: value.definition_name.into(),
            started_at: value.started_at,
            ended_at: value.ended_at.into(),
            hit_count: value.hit_count,
            covered: value.covered,
        }
    }
}

impl From<ServiceCandidates> for ProtectionRecordingCandidates {
    fn from(value: ServiceCandidates) -> Self {
        Self {
            stream: value.stream.into(),
            since: value.since.into(),
            baseline_tt_ped: value.baseline_tt_ped.into(),
            sessions: value.sessions.into_iter().map(Into::into).collect(),
            earlier: value.earlier.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ServiceOverview> for ProtectionOverview {
    fn from(value: ServiceOverview) -> Self {
        Self {
            sets: value.sets.into_iter().map(Into::into).collect(),
            recent_cost_windows: value
                .recent_cost_windows
                .into_iter()
                .map(Into::into)
                .collect(),
            unrecorded: UnrecordedProtection {
                sessions: value.unrecorded.sessions,
                hits: value.unrecorded.hits,
            },
        }
    }
}
