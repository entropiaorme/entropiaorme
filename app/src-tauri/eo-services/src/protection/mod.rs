//! Protection cost accounting at session grain.
//!
//! Nothing about protection is declared during play. The tracker records
//! every observed defensive hit (numeric damage or a deflection) with its
//! session and event context, and the player records costs afterwards, at
//! the moment they repair or scan:
//!
//! - **Unlimited** protection is one pooled stream. A confirmed repair total
//!   is consumed at raw TT; which piece or plate it came from is not asked.
//! - Each **limited** set is its own stream, because its acquisition markup
//!   is its own. Consumption is the TT lost between two Trade Terminal
//!   readings at the set's frozen markup; the first reading is a baseline.
//!
//! A recording looks back to the previous recording of the same stream and
//! is spread over the sessions the player ticks, by hit count, then within
//! each session across its contexts by hit count. Streams that cover the
//! same session add up. Session and account totals are exact; a segment's
//! share is a hit-weighted estimate, which is the accepted trade for never
//! asking what was worn.

mod read;
mod recording;
mod sets;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};

pub use recording::UNATTRIBUTED_REASON;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionSetKind {
    Armour,
    Plates,
}

impl ProtectionSetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Armour => "armour",
            Self::Plates => "plates",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "armour" => Ok(Self::Armour),
            "plates" => Ok(Self::Plates),
            _ => Err(ProtectionError::Stored("unknown protection set kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationSource {
    Ocr,
    Manual,
}

impl ObservationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::Manual => "manual",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "ocr" => Ok(Self::Ocr),
            "manual" => Ok(Self::Manual),
            _ => Err(ProtectionError::Stored("unknown observation source")),
        }
    }
}

/// Whether a recorded cost reached any session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostStatus {
    /// Spread over at least one session.
    Booked,
    /// Kept as an explicit amount no session carries: nothing was ticked,
    /// no session had hits to carry it, or (for history) a baseline reset
    /// left the amount unknown.
    Pending,
}

impl CostStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Booked => "booked",
            Self::Pending => "pending",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "booked" => Ok(Self::Booked),
            "pending" => Ok(Self::Pending),
            _ => Err(ProtectionError::Stored("unknown protection cost status")),
        }
    }
}

/// What a recorded cost measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostKind {
    /// TT lost by one limited set between two readings, at its markup.
    LimitedDecay,
    /// A confirmed unlimited repair total at raw TT.
    Repair,
}

impl CostKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::LimitedDecay => "limited_decay",
            Self::Repair => "repair",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "limited_decay" => Ok(Self::LimitedDecay),
            "repair" => Ok(Self::Repair),
            _ => Err(ProtectionError::Stored("unknown protection cost kind")),
        }
    }
}

/// One independently recorded and allocated protection cost stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionStream {
    /// Every unlimited armour piece and plate, repaired as one pool.
    Unlimited,
    /// One limited armour or plate set.
    Limited { set_id: i64 },
}

/// A limited armour or plate set: one aggregate layer with one approximate
/// acquisition markup, measured by Trade Terminal readings.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionSet {
    pub id: i64,
    pub kind: ProtectionSetKind,
    pub name: String,
    pub markup_percent: f64,
    pub created_at: f64,
    pub archived_at: Option<f64>,
    pub latest_observation: Option<ProtectionObservation>,
    /// The markup is frozen once the set has a reading.
    pub basis_locked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionObservation {
    pub id: i64,
    pub set_id: i64,
    pub tt_value_ped: f64,
    pub source: ObservationSource,
    pub raw_text: Option<String>,
    pub observed_at: f64,
    pub reset_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionCostAllocation {
    pub session_id: String,
    pub hit_count: i64,
    pub allocation_share: f64,
    pub cost_ped: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionCostWindow {
    pub id: i64,
    pub kind: CostKind,
    pub set_id: Option<i64>,
    /// The limited set's name, kept readable after the set is archived.
    pub set_name: Option<String>,
    pub consumed_tt_ped: Option<f64>,
    pub markup_percent: Option<f64>,
    pub cost_ped: f64,
    pub cost_known: bool,
    pub status: CostStatus,
    pub reason: Option<String>,
    pub created_at: f64,
    pub allocations: Vec<ProtectionCostAllocation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservationOutcome {
    pub observation: ProtectionObservation,
    /// Absent for a baseline or a reset, which measure nothing.
    pub cost_window: Option<ProtectionCostWindow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepairOutcome {
    pub cost_window: ProtectionCostWindow,
}

/// One session a recording could be spread over.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateSession {
    pub session_id: String,
    /// The session's own name, when it was given one.
    pub session_name: Option<String>,
    /// The session type it was played under, which the recording surface
    /// groups by; absent for a session recorded under none.
    pub definition_id: Option<i64>,
    pub definition_name: Option<String>,
    pub started_at: f64,
    /// Absent while the session is still running.
    pub ended_at: Option<f64>,
    /// The hits this recording would weigh the session by.
    pub hit_count: i64,
    /// An earlier recording of the same stream already covers it.
    pub covered: bool,
}

/// What a recording of one stream would look back over.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingCandidates {
    pub stream: ProtectionStream,
    /// When the stream was last recorded (for a limited set, its current
    /// baseline reading); absent before the first.
    pub since: Option<f64>,
    /// The limited set's current baseline TT value.
    pub baseline_tt_ped: Option<f64>,
    /// Sessions with hits since the previous recording, oldest first.
    pub sessions: Vec<CandidateSession>,
    /// Unlimited only: the most recent sessions from before the previous
    /// recording, which a recording may re-include (a piece left
    /// unrepaired last time). Oldest first.
    pub earlier: Vec<CandidateSession>,
}

/// Recorded hits that no protection cost covers yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnrecordedProtection {
    pub sessions: i64,
    pub hits: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionOverview {
    pub sets: Vec<ProtectionSet>,
    pub recent_cost_windows: Vec<ProtectionCostWindow>,
    pub unrecorded: UnrecordedProtection,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtectionError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("stored protection data is invalid: {0}")]
    Stored(&'static str),
}

impl From<rusqlite::Error> for ProtectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

pub struct ProtectionService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl ProtectionService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    fn now(&self) -> f64 {
        self.clock.now().and_utc().timestamp_micros() as f64 / 1_000_000.0
    }

    pub async fn overview(&self) -> Result<ProtectionOverview, ProtectionError> {
        self.db
            .with_reader(|conn| read::read_overview(conn).map_err(protection_decode))
            .await
            .map_err(ProtectionError::from)
    }

    /// The sessions a recording of `stream` would be spread over, for the
    /// recording surface to offer before the amount is confirmed.
    pub async fn recording_candidates(
        &self,
        stream: ProtectionStream,
    ) -> Result<RecordingCandidates, ProtectionError> {
        if let ProtectionStream::Limited { set_id } = stream {
            self.active_set(set_id).await?;
        }
        self.db
            .with_reader(move |conn| {
                recording::read_candidates(conn, stream).map_err(protection_decode)
            })
            .await
            .map_err(ProtectionError::from)
    }

    /// Recorded hits on one session that no protection cost reaches yet,
    /// so its armour cost can read as not recorded rather than zero.
    pub async fn session_unrecorded_hits(&self, session_id: &str) -> Result<i64, ProtectionError> {
        let session_id = session_id.to_string();
        self.db
            .with_reader(move |conn| {
                read::session_unrecorded_hits(conn, &session_id).map_err(Into::into)
            })
            .await
            .map_err(ProtectionError::from)
    }

    async fn active_set(&self, id: i64) -> Result<ProtectionSet, ProtectionError> {
        let set = self
            .db
            .with_reader(move |conn| read::read_set(conn, id).map_err(protection_decode))
            .await?;
        set.filter(|set| set.archived_at.is_none())
            .ok_or(ProtectionError::NotFound("Protection set not found"))
    }
}

fn protection_decode(error: ProtectionError) -> DbError {
    match error {
        ProtectionError::Db(error) => error,
        other => DbError::Decode {
            context: "stored protection record",
            source: serde_json::Error::io(std::io::Error::other(other.to_string())),
        },
    }
}

fn map_constraint(message: &'static str) -> impl FnOnce(DbError) -> ProtectionError {
    move |error| match error {
        DbError::Sqlite(rusqlite::Error::SqliteFailure(_, _)) => ProtectionError::Conflict(message),
        other => ProtectionError::Db(other),
    }
}
