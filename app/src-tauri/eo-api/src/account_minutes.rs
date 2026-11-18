//! A measured session minute by minute: each step between two consecutive
//! readings, what the readings saw against what the chat log said.
//!
//! Loot TT stays the chat log's; the series catches stock and slips in the
//! step they happen. Each step carries what was looted and what arrived;
//! what arrived that nothing recorded explains ("arrived, not looted"),
//! each with the handle the player decides it by; loot that never arrived
//! once the next step had passed ("looted, never arrived"), with what the
//! existing loot correction takes when it removes exactly those lines; and
//! the weapon and healing spend measured beside the tracked spend, with the
//! stretches where their gap moved away from its usual slope. Armour is
//! measured only.
//!
//! An arrival's handle is an arrival proposal, decided through the
//! proposal commands: recorded as an asset purchase (claim, record the
//! purchase, link the proposal to the purchase's id), recorded as stock of
//! unknown origin (one command here, taken back by another), or dismissed
//! (remembered, not offered again). Like every account command, these are
//! the main window's alone.

use eo_services::account::costs::Unmeasured;
use eo_services::account::minutes::{
    LootCorrection, LootSource, Minute, MissingLoot, NoCorrection, SessionMinutes, StepFigure,
    StepUnmeasured, Stretch, UnexplainedArrival,
};
use eo_services::account::proposals::{
    ArrivalDecision, Decided, NotProposed, ProposalId, RecordedAs,
};
use eo_services::account::recorder::ArrivalRecording;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::account::{
    account_error, amount, edge, place, span, AccountPlace, MeasuredAmount, MeasuredEdge,
    MeasuredSpan,
};
use crate::analytics::ActivityUndoInput;
use crate::{Api, ApiError};

/// Why a step's bucket has no measured figure: the step's own reasons,
/// then each of the session costs' reasons by its own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MinuteUnmeasured {
    /// The step spans a gap between readings.
    Gap,
    /// Universal Ammo was topped up in the step: what was fired in it is
    /// not known from the rows.
    AmmoToppedUp,
    NoReadings,
    PartialCoverage,
    /// How much of the ammo's change was fired is not known.
    AmmoUnsettled,
    /// A limited piece lost TT, and pricing it needs its set confirmed.
    LimitedSetUnconfirmed,
    StillRunning,
    TrackedOnly,
    PlayInGap,
    ChatLogMissesGap,
    ChatLogUnchecked,
    GapUnaccounted,
    OverheadInGap,
    GapCheckPending,
    AwaitingGapReading,
    ChatLogNotPastGap,
}

/// A step's measured figure for a bucket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MinuteFigure {
    Measured { amount: MeasuredAmount },
    Unmeasured { reason: MinuteUnmeasured },
}

/// A bucket with a tracked counterpart, in one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinuteLane {
    pub measured: MinuteFigure,
    /// What the tracked events timed in the step cost, in PED: kills by
    /// when they ended, heals by when they were seen.
    pub tracked: f64,
}

/// Why an arrival carries no handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ArrivalNotProposed {
    /// Its step reaches back to readings stored before the app recognised
    /// steps: history, never offered.
    BeforeRecognition,
    /// Its step lies wholly outside the session: a purchase then is offered
    /// with the account's purchases, not here.
    OutsideSession,
    /// The last step of a running session: what the chat log records next
    /// may still explain it.
    Settling,
    /// It could not be stored as a proposal on this read; the next read
    /// stores it.
    Awaiting,
}

/// What an arrival was recorded as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ArrivalRecordedAs {
    /// An asset purchase: `recordId` is the purchase's id.
    Purchase,
    /// Stock of unknown origin: `recordId` is what its undo names.
    UnknownOrigin,
}

/// What the player decided of an arrival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ArrivalDisposition {
    /// No handle, and why.
    NotProposed { reason: ArrivalNotProposed },
    /// Waiting for the player. `possiblyRecorded` when a recording of it
    /// was begun and never linked.
    #[serde(rename_all = "camelCase")]
    Open {
        proposal_id: i64,
        possibly_recorded: bool,
        /// Stock of unknown origin recorded from it that stands though the
        /// arrival was never marked recorded: what
        /// `account_arrival_unknown_origin_undo` takes back. A retake
        /// links it rather than recording a second.
        unlinked_record_id: Option<String>,
    },
    /// Recorded, and the record stands, as `as` says.
    #[serde(rename_all = "camelCase")]
    Recorded {
        proposal_id: i64,
        record_id: String,
        #[serde(rename = "as")]
        recorded_as: ArrivalRecordedAs,
        /// For a purchase, the name of the asset it bought into.
        asset_name: Option<String>,
    },
    /// Dismissed: not offered again unless the player takes that back
    /// (`account_arrival_unignore`).
    #[serde(rename_all = "camelCase")]
    Ignored { proposal_id: i64 },
}

/// Something that arrived in a step that no loot line, recorded quest
/// reward, conversion or opening explains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinuteArrival {
    pub item_name: String,
    pub arrived_in: AccountPlace,
    pub units: i64,
    pub tt: MeasuredAmount,
    /// The PED Card fell by its TT in the step: bought at the Trade
    /// Terminal, at no markup.
    pub bought_at_tt: bool,
    pub disposition: ArrivalDisposition,
}

/// Where a loot line was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MinuteLootSource {
    Kill,
    Harvest,
}

/// One loot line, by where it was recorded and its own row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinuteLootLine {
    pub source: MinuteLootSource,
    pub id: i64,
}

/// Why the loot correction is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MinuteNoCorrection {
    /// Harvest loot has no correction.
    HarvestLoot,
    /// The correction removes the item from the whole session, and other
    /// lines of it arrived, are not proven missing, or are an enhancer's
    /// return.
    OtherLinesArrived,
}

/// What correcting loot that never arrived takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MinuteLootCorrection {
    /// `tracking_loot_item_deactivate` with these arguments removes exactly
    /// the lines that never arrived.
    #[serde(rename_all = "camelCase")]
    Offered {
        session_id: String,
        item_name: String,
    },
    NotOffered {
        reason: MinuteNoCorrection,
    },
}

/// Loot the chat log recorded that the readings never showed arriving, a
/// step after it was looted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinuteMissingLoot {
    pub item_name: String,
    pub units: i64,
    /// The TT of the units that never arrived, from the chat log.
    pub tt: f64,
    /// The step it was looted in.
    pub looted_in: MeasuredSpan,
    pub lines: Vec<MinuteLootLine>,
    pub correction: MinuteLootCorrection,
}

/// One step between two consecutive readings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionMinute {
    /// The later reading of the step, and the one before it.
    pub reading_id: i64,
    pub previous_reading_id: i64,
    pub between: MeasuredSpan,
    /// The readings are further apart than 150 seconds: one step across
    /// the gap.
    pub gap: bool,
    /// False for the last step of a running session: what arrived in it is
    /// not flagged yet.
    pub settled: bool,
    /// The TT of the loot lines timed in the step, in PED.
    pub looted: f64,
    /// The TT of what arrived in the step, Universal Ammo aside.
    pub arrived: MeasuredAmount,
    pub arrived_not_looted: Vec<MinuteArrival>,
    pub looted_never_arrived: Vec<MinuteMissingLoot>,
    pub weapon: MinuteLane,
    pub healing: MinuteLane,
    /// Measured only: armour has no tracked counterpart.
    pub armour: MinuteFigure,
}

/// A stretch of steps where a lane's measured spend moved away from the
/// tracked spend's usual slope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinuteStretch {
    /// The steps it covers, by their position in `steps`.
    pub first_step: i64,
    pub last_step: i64,
    pub between: MeasuredSpan,
    pub measured: MeasuredAmount,
    pub tracked: f64,
    /// How far the gap moved beyond the session's usual slope, in PED:
    /// positive when the readings measured more than was tracked.
    pub drift: f64,
}

/// How well the readings cover the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MinutesCoverage {
    /// Both ends read, at least two readings, no gap.
    pub complete: bool,
    pub opening: MeasuredEdge,
    pub closing: MeasuredEdge,
    pub reading_count: i64,
    pub gap_count: i64,
}

/// A measured session, minute by minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountSessionMinutes {
    pub session_id: String,
    pub running: bool,
    pub coverage: MinutesCoverage,
    pub steps: Vec<SessionMinute>,
    pub weapon_stretches: Vec<MinuteStretch>,
    pub healing_stretches: Vec<MinuteStretch>,
    /// The TT of loot lines no step can confirm: timed after the last
    /// reading, or in the last step with none after it.
    pub loot_unconfirmed: f64,
}

/// An arrival proposal to record as stock of unknown origin. `retake`
/// takes over an earlier recording of it that was begun and never linked,
/// once the player has checked.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountArrivalInput {
    pub id: i64,
    #[serde(default)]
    pub retake: bool,
}

/// Stock of unknown origin recorded from an arrival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountArrivalRecorded {
    /// The record's own id: what taking it back names.
    pub record_id: String,
    /// Whether the arrival was marked recorded. When not, the stock stands
    /// and the arrival shows as possibly recorded already, to take over
    /// with `retake` or take back by its undo.
    pub linked: bool,
}

/// An arrival proposal, by its id.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountArrivalId {
    pub id: i64,
}

fn figure(value: StepFigure) -> MinuteFigure {
    match value {
        StepFigure::Measured(value) => MinuteFigure::Measured {
            amount: amount(value),
        },
        StepFigure::Unmeasured(reason) => MinuteFigure::Unmeasured {
            reason: unmeasured(reason),
        },
    }
}

fn unmeasured(value: StepUnmeasured) -> MinuteUnmeasured {
    match value {
        StepUnmeasured::Gap => MinuteUnmeasured::Gap,
        StepUnmeasured::AmmoToppedUp => MinuteUnmeasured::AmmoToppedUp,
        StepUnmeasured::Bucket(reason) => match reason {
            Unmeasured::NoReadings => MinuteUnmeasured::NoReadings,
            Unmeasured::PartialCoverage => MinuteUnmeasured::PartialCoverage,
            Unmeasured::Gap => MinuteUnmeasured::Gap,
            Unmeasured::AmmoUnsettled => MinuteUnmeasured::AmmoUnsettled,
            Unmeasured::LimitedSetUnconfirmed => MinuteUnmeasured::LimitedSetUnconfirmed,
            Unmeasured::StillRunning => MinuteUnmeasured::StillRunning,
            Unmeasured::TrackedOnly => MinuteUnmeasured::TrackedOnly,
            Unmeasured::PlayInGap => MinuteUnmeasured::PlayInGap,
            Unmeasured::ChatLogMissesGap => MinuteUnmeasured::ChatLogMissesGap,
            Unmeasured::ChatLogUnchecked => MinuteUnmeasured::ChatLogUnchecked,
            Unmeasured::GapUnaccounted => MinuteUnmeasured::GapUnaccounted,
            Unmeasured::OverheadInGap => MinuteUnmeasured::OverheadInGap,
            Unmeasured::GapCheckPending => MinuteUnmeasured::GapCheckPending,
            Unmeasured::AwaitingGapReading => MinuteUnmeasured::AwaitingGapReading,
            Unmeasured::ChatLogNotPastGap => MinuteUnmeasured::ChatLogNotPastGap,
        },
    }
}

fn disposition(value: &ArrivalDecision) -> ArrivalDisposition {
    match value {
        ArrivalDecision::NotProposed(reason) => ArrivalDisposition::NotProposed {
            reason: match reason {
                NotProposed::BeforeRecognition => ArrivalNotProposed::BeforeRecognition,
                NotProposed::OutsideSession => ArrivalNotProposed::OutsideSession,
                NotProposed::Settling => ArrivalNotProposed::Settling,
                NotProposed::Awaiting => ArrivalNotProposed::Awaiting,
            },
        },
        ArrivalDecision::Open {
            proposal,
            possibly_recorded,
            unlinked_record,
        } => ArrivalDisposition::Open {
            proposal_id: proposal.0,
            possibly_recorded: *possibly_recorded,
            unlinked_record_id: unlinked_record.clone(),
        },
        ArrivalDecision::Recorded {
            proposal,
            record_id,
            recorded_as,
            asset_name,
        } => ArrivalDisposition::Recorded {
            proposal_id: proposal.0,
            record_id: record_id.clone(),
            asset_name: asset_name.clone(),
            recorded_as: match recorded_as {
                RecordedAs::Purchase => ArrivalRecordedAs::Purchase,
                RecordedAs::UnknownOrigin => ArrivalRecordedAs::UnknownOrigin,
            },
        },
        ArrivalDecision::Ignored { proposal } => ArrivalDisposition::Ignored {
            proposal_id: proposal.0,
        },
    }
}

fn arrival(value: &UnexplainedArrival) -> MinuteArrival {
    MinuteArrival {
        item_name: value.key.name.clone(),
        arrived_in: place(&value.key.container),
        units: value.units,
        tt: amount(value.amount),
        bought_at_tt: value.bought_at_tt,
        disposition: disposition(&value.decision),
    }
}

fn missing(value: &MissingLoot) -> MinuteMissingLoot {
    MinuteMissingLoot {
        item_name: value.item.clone(),
        units: value.units,
        tt: value.value,
        looted_in: span(&value.looted_in),
        lines: value
            .lines
            .iter()
            .map(|(source, id)| MinuteLootLine {
                source: match source {
                    LootSource::Kill => MinuteLootSource::Kill,
                    LootSource::Harvest => MinuteLootSource::Harvest,
                },
                id: *id,
            })
            .collect(),
        correction: match &value.correction {
            LootCorrection::Offered {
                session_id,
                item_name,
            } => MinuteLootCorrection::Offered {
                session_id: session_id.clone(),
                item_name: item_name.clone(),
            },
            LootCorrection::NotOffered(reason) => MinuteLootCorrection::NotOffered {
                reason: match reason {
                    NoCorrection::HarvestLoot => MinuteNoCorrection::HarvestLoot,
                    NoCorrection::OtherLinesArrived => MinuteNoCorrection::OtherLinesArrived,
                },
            },
        },
    }
}

fn minute(value: &Minute) -> SessionMinute {
    SessionMinute {
        reading_id: value.reading.0,
        previous_reading_id: value.prev_reading.0,
        between: span(&value.between),
        gap: value.gap,
        settled: value.settled,
        looted: value.looted,
        arrived: amount(value.arrived),
        arrived_not_looted: value.arrived_not_looted.iter().map(arrival).collect(),
        looted_never_arrived: value.looted_never_arrived.iter().map(missing).collect(),
        weapon: MinuteLane {
            measured: figure(value.weapon.measured),
            tracked: value.weapon.tracked,
        },
        healing: MinuteLane {
            measured: figure(value.healing.measured),
            tracked: value.healing.tracked,
        },
        armour: figure(value.armour),
    }
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn stretch(value: &Stretch) -> MinuteStretch {
    MinuteStretch {
        first_step: count(value.first_step),
        last_step: count(value.last_step),
        between: span(&value.between),
        measured: amount(value.measured),
        tracked: value.tracked,
        drift: value.drift_micro_ped as f64 / eo_services::account::amount::MICRO_PER_PED as f64,
    }
}

fn minutes(value: &SessionMinutes) -> AccountSessionMinutes {
    AccountSessionMinutes {
        session_id: value.session_id.clone(),
        running: value.running,
        coverage: MinutesCoverage {
            complete: value.coverage.is_complete(),
            opening: edge(value.coverage.opening),
            closing: edge(value.coverage.closing),
            reading_count: count(value.coverage.reading_count),
            gap_count: count(value.coverage.gap_count),
        },
        steps: value.steps.iter().map(minute).collect(),
        weapon_stretches: value.weapon_stretches.iter().map(stretch).collect(),
        healing_stretches: value.healing_stretches.iter().map(stretch).collect(),
        loot_unconfirmed: value.loot_unconfirmed,
    }
}

impl Api {
    /// One tracked session minute by minute; not found when there is no
    /// such session. A session with no readings has no steps. Arrivals it
    /// flags are proposed as it is read, so each carries its handle.
    pub async fn account_session_minutes(
        &self,
        session_id: String,
    ) -> Result<AccountSessionMinutes, ApiError> {
        let read = self
            .account()?
            .session_minutes(session_id)
            .await
            .map_err(account_error("account session minutes"))?;
        match read {
            Some(read) => Ok(minutes(&read)),
            None => Err(ApiError::not_found("Session not found")),
        }
    }

    /// Record an arrival proposal as stock of unknown origin: unattributed
    /// stock, which funds no activity. Not found when no open arrival
    /// proposal has that id; a conflict when it cannot be claimed or the
    /// stock write is refused.
    pub async fn account_arrival_unknown_origin(
        &self,
        input: AccountArrivalInput,
    ) -> Result<AccountArrivalRecorded, ApiError> {
        let outcome = self
            .account()?
            .record_arrival_unknown_origin(ProposalId(input.id), input.retake)
            .await
            .map_err(account_error("account arrival unknown origin"))?;
        match outcome {
            ArrivalRecording::Recorded { record_id } => Ok(AccountArrivalRecorded {
                record_id,
                linked: true,
            }),
            ArrivalRecording::Unlinked { record_id } => Ok(AccountArrivalRecorded {
                record_id,
                linked: false,
            }),
            ArrivalRecording::NotOpen => Err(ApiError::not_found(
                "no open arrival with that id to record",
            )),
            ArrivalRecording::Refused(reason) => Err(ApiError::conflict(reason)),
            ArrivalRecording::WriteRefused => Err(ApiError::conflict(
                "the stock could not be recorded; nothing was changed",
            )),
        }
    }

    /// Offer a dismissed arrival again: the player takes back having said
    /// to leave it. Restoring one that is not dismissed changes nothing;
    /// not found when there is no such proposal, a conflict when it is not
    /// an arrival.
    pub async fn account_arrival_unignore(&self, input: AccountArrivalId) -> Result<(), ApiError> {
        let outcome = self
            .account()?
            .restore_arrival(ProposalId(input.id))
            .await
            .map_err(account_error("account arrival unignore"))?;
        match outcome {
            Decided::Done => Ok(()),
            Decided::NotFound => Err(ApiError::not_found("no such arrival to offer again")),
            Decided::Refused(reason) => Err(ApiError::conflict(reason)),
        }
    }

    /// Take back stock of unknown origin recorded from an arrival, by the
    /// record's id: its stock goes and the arrival is open again. Not found
    /// when no such record stands.
    pub async fn account_arrival_unknown_origin_undo(
        &self,
        input: ActivityUndoInput,
    ) -> Result<(), ApiError> {
        let undone = self
            .account()?
            .undo_arrival_unknown_origin(input.id)
            .await
            .map_err(crate::analytics::analytics_error(
                "account arrival unknown origin undo",
            ))?;
        if undone {
            Ok(())
        } else {
            Err(ApiError::not_found(
                "no stock of unknown origin with that id to take back",
            ))
        }
    }
}
