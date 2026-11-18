//! What a tracked session cost, as its readings measure it: per bucket and
//! per item, read from the session's measure.
//!
//! The buckets mirror the tracked ones (weapon, enhancer, harvesting,
//! healing, armour, consumables), beside three the tracked figures never
//! had (travel, field logistics, and what the app cannot place yet), which
//! no Cycled bucket includes. Every line names its item and the category
//! it costs as ([`CostCategory`]), so a wrong category shows as an item in
//! the wrong bucket rather than vanishing.
//!
//! What costs:
//!
//! - **Ammo** is what the measure's split says was burned, at TT; Shrapnel
//!   turned into ammo, and ammo a strongbox paid out, never is. A step the
//!   ammo was topped up in (see [`super::measure`]) has no burn the readings
//!   know: the weapon bucket is then measured but for those steps, whose
//!   spans it keeps ([`SessionCosts::top_ups`]) for the tracked figure to
//!   stand in for ([`Basis::Mixed`]). The tracked price of a shot holds
//!   the weapon's wear too, so what the weapon, its amplifier, attachments
//!   and implant wore in those steps is left to the tracked figure as well;
//!   every other item's wear in them is measured as in any other step.
//! - **The PED Card** is never a cost, and never decides one: a repair is
//!   settlement, whatever the Card shows it cost.
//! - **Decay** is the sum of an item's TT falls, step by step. A repair's
//!   rise is settlement, never a negative cost, and is never netted against
//!   the decay it settles (wear inside the minute of a repair, which the
//!   readings net away, is lost, and accepted as lost). A limited armour
//!   piece or plate is listed with the TT it lost; its cost is that TT at
//!   the markup set for the piece as it stood when the session started
//!   (see [`super::armour`]), or the TT itself when none was. The armour
//!   bucket is kept in parts ([`ArmourPart`]): the unlimited armour at what
//!   it lost, and each limited piece at its loss and markup.
//! - **Consumption** is the units of a stack used up, and only for a stack
//!   whose category is a consumable. It follows the player's own choice
//!   for the item in the equipment library, the one that decides whether
//!   a dose of it books a cost: an item whose doses book is charged its TT
//!   at the acquisition markup recorded for it; an item accounted through
//!   the ledger is listed and charged nothing here, since its purchase is
//!   already in the ledger; an item the library does not hold is charged
//!   its TT (any markup paid for it is not included). Any other stack
//!   leaving (a trade, a refine, loot given away) is never a cost; one
//!   nothing names is listed, charged nothing, for the player to place
//!   (see [`super::placement`]). An
//!   enhancer leaving is not one here: its break is costed from the chat
//!   log.
//!
//! A bucket is measured only when the session's measure is complete and
//! the bucket's own inputs settled; otherwise it is unmeasured with the
//! reason, and carries no amount. An unmeasured figure is never zero. A
//! sealed session whose readings leave an end unread is measured across
//! the gap from the nearest reading on its far side only when the gap is
//! shown clean (see [`super::gap`]); its costs are then a kind of their
//! own ([`Status::AcrossGap`]), recording the readings that bound them and
//! how far outside the session those reach.
//!
//! A sealed session's costs are stored as a projection of its sealed
//! measure (`account_session_costs` with its buckets and lines), written
//! in the same short write as the seal, and rebuilt when its rules
//! ([`COSTS_VERSION`]) or the measure's ([`MEASURE_VERSION`]) are not the
//! code's, when the measure was computed again since, or when the library,
//! sets or game data its items were placed by have changed (the header
//! keeps their digest): on the next read of the session, and by
//! [`heal_session_costs`], which the connection runs when it starts and
//! after each seal. A running session's costs are computed on every read
//! and never stored.

use std::collections::{BTreeMap, HashMap};

use rusqlite::OptionalExtension;

use super::amount::Amount;
use super::measure::{
    AmmoSplit, CoverageLevel, Edge, NotableEvent, SessionMeasure, SessionWindow, MEASURE_VERSION,
};
use super::reading::{Container, HoldingKey};
use super::roles::{
    cost_category, known_category, CostCategory, DosePricing, KnownRoles, Role, RoleSource, Use,
};
use super::sessions::{self, MeasureRead, ToSeal};
use super::store::{cached_measure, CachedMeasure};
use super::AccountError;
use crate::chatlog_span::ChatLogSource;
use crate::db::Db;
use crate::game_data_store::GameDataStore;

/// The version of the costs' shape and rules. A stored projection of
/// another version is rebuilt from the sealed measure. A change of the
/// rules bumps it.
// Bumped to 4 when the player's own placement of an item came to decide
// what its use costs as, items placed as travel or as no cost joined the
// categories, and a limited piece's line came to name the confirmed set
// that priced it. Bumped to 5 when the PED Card stopped holding a
// consumable's bucket back, and a weapon bucket whose ammo was topped up
// came to be measured but for the steps it was topped up in. Bumped to 6
// when a limited piece came to be billed at the markup set for it, or at
// the TT it lost when none was, never waiting on a set.
pub const COSTS_VERSION: i64 = 6;

/// A group of costs, as the tracked figures group them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostBucket {
    /// Ammo burned, and the weapon, its amplifier, attachments and the
    /// mindforce implant wearing.
    Weapon,
    /// Enhancer breaks: costed from the chat log, never measured here.
    Enhancer,
    Harvesting,
    /// Healing tools wearing and healing consumables used up.
    Healing,
    /// Armour and plates wearing, unlimited and limited.
    Armour,
    /// Stimulants and other doses used up.
    Consumables,
    /// Teleport chips wearing and the Mind Essence they burn, and what the
    /// player placed as travel.
    Travel,
    /// Portable repair and Trade Terminal units wearing.
    FieldLogistics,
    /// Whatever wore that the app cannot place yet.
    Unclassified,
}

impl CostBucket {
    pub const ALL: [CostBucket; 9] = [
        CostBucket::Weapon,
        CostBucket::Enhancer,
        CostBucket::Harvesting,
        CostBucket::Healing,
        CostBucket::Armour,
        CostBucket::Consumables,
        CostBucket::Travel,
        CostBucket::FieldLogistics,
        CostBucket::Unclassified,
    ];

    /// Whether the bucket is part of what performing the activity cost
    /// (Cycled); the rest is the session's overhead.
    pub fn is_cycled(self) -> bool {
        matches!(
            self,
            CostBucket::Weapon
                | CostBucket::Enhancer
                | CostBucket::Harvesting
                | CostBucket::Healing
                | CostBucket::Armour
                | CostBucket::Consumables
        )
    }

    /// The bucket an item of a category costs in; none for an enhancer,
    /// whose break is costed from the chat log, and none for an item the
    /// player placed as no cost.
    pub fn of(category: CostCategory) -> Option<CostBucket> {
        Some(match category {
            CostCategory::Ammo
            | CostCategory::Weapon
            | CostCategory::Amplifier
            | CostCategory::Attachment
            | CostCategory::MindforceImplant => CostBucket::Weapon,
            CostCategory::Enhancer | CostCategory::NotACost => return None,
            CostCategory::HarvestingTool => CostBucket::Harvesting,
            CostCategory::HealingTool | CostCategory::HealingConsumable => CostBucket::Healing,
            CostCategory::UnlimitedArmour
            | CostCategory::UnlimitedPlate
            | CostCategory::LimitedArmour
            | CostCategory::LimitedPlate => CostBucket::Armour,
            CostCategory::Consumable => CostBucket::Consumables,
            CostCategory::TeleportChip | CostCategory::MindEssence | CostCategory::Travel => {
                CostBucket::Travel
            }
            CostCategory::FieldLogistics => CostBucket::FieldLogistics,
            CostCategory::Unclassified => CostBucket::Unclassified,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CostBucket::Weapon => "weapon",
            CostBucket::Enhancer => "enhancer",
            CostBucket::Harvesting => "harvesting",
            CostBucket::Healing => "healing",
            CostBucket::Armour => "armour",
            CostBucket::Consumables => "consumables",
            CostBucket::Travel => "travel",
            CostBucket::FieldLogistics => "fieldLogistics",
            CostBucket::Unclassified => "unclassified",
        }
    }

    pub fn parse(value: &str) -> Option<CostBucket> {
        CostBucket::ALL
            .into_iter()
            .find(|bucket| bucket.as_str() == value)
    }
}

/// What a line measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineKind {
    /// Ammo burned, in units at TT.
    AmmoBurned,
    /// An item's TT falls, summed.
    Decay,
    /// A limited piece's TT falls, summed: what it lost, not what it cost,
    /// which is that TT at its set's markup.
    LimitedDecay,
    /// A consumable's units used up, at TT; any markup paid for them is
    /// not included.
    Consumed,
}

impl LineKind {
    pub const ALL: [LineKind; 4] = [
        LineKind::AmmoBurned,
        LineKind::Decay,
        LineKind::LimitedDecay,
        LineKind::Consumed,
    ];

    /// Whether the line counts units.
    pub fn counts_units(self) -> bool {
        matches!(self, LineKind::AmmoBurned | LineKind::Consumed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineKind::AmmoBurned => "ammoBurned",
            LineKind::Decay => "decay",
            LineKind::LimitedDecay => "limitedDecay",
            LineKind::Consumed => "consumed",
        }
    }

    pub fn parse(value: &str) -> Option<LineKind> {
        LineKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

/// Why a session or a bucket is not measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unmeasured {
    /// No reading is tagged with the session.
    NoReadings,
    /// The readings do not read one of the session's ends.
    PartialCoverage,
    /// Two consecutive readings are too far apart.
    Gap,
    /// How much of the ammo's change was burned is not known.
    AmmoUnsettled,
    /// A limited piece lost TT, and pricing it needs its set confirmed.
    LimitedSetUnconfirmed,
    /// The session is still running, or still being read to its close.
    StillRunning,
    /// The readings do not measure this bucket: its cost is the tracked
    /// one.
    TrackedOnly,
    /// An end is unread across a gap, and the chat log shows play outside
    /// the session in it.
    PlayInGap,
    /// An end is unread across a gap, and the chat log does not cover the
    /// gap.
    ChatLogMissesGap,
    /// An end is unread across a gap, and the chat log could not be
    /// checked over it.
    ChatLogUnchecked,
    /// An end is unread across a gap, and the readings across it hold a
    /// repair, or a cost of a kind the session's own play never had.
    GapUnaccounted,
    /// The session was measured across a gap, and this overhead bucket's
    /// change in the gap may be the player's after the session: it is not
    /// billed to it.
    OverheadInGap,
    /// An end is unread across a gap, and the chat log's time base is not
    /// known yet: the gap is checked on a later computation. Never stored.
    GapCheckPending,
    /// The session's end is unread, and no reading has been stored after
    /// it yet: the gap is weighed once one is. Never stored.
    AwaitingGapReading,
    /// An end is unread across a gap, and the chat log has no line after
    /// the gap yet: the game has not written past it, and the gap is
    /// weighed once it has. Never stored.
    ChatLogNotPastGap,
}

impl Unmeasured {
    pub const ALL: [Unmeasured; 15] = [
        Unmeasured::NoReadings,
        Unmeasured::PartialCoverage,
        Unmeasured::Gap,
        Unmeasured::AmmoUnsettled,
        Unmeasured::LimitedSetUnconfirmed,
        Unmeasured::StillRunning,
        Unmeasured::TrackedOnly,
        Unmeasured::PlayInGap,
        Unmeasured::ChatLogMissesGap,
        Unmeasured::ChatLogUnchecked,
        Unmeasured::GapUnaccounted,
        Unmeasured::OverheadInGap,
        Unmeasured::GapCheckPending,
        Unmeasured::AwaitingGapReading,
        Unmeasured::ChatLogNotPastGap,
    ];

    /// Whether costs unmeasured for this reason are a passing state, never
    /// stored: computed again on the next read.
    pub fn is_passing(self) -> bool {
        matches!(
            self,
            Unmeasured::StillRunning
                | Unmeasured::NoReadings
                | Unmeasured::GapCheckPending
                | Unmeasured::AwaitingGapReading
                | Unmeasured::ChatLogNotPastGap
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Unmeasured::NoReadings => "noReadings",
            Unmeasured::PartialCoverage => "partialCoverage",
            Unmeasured::Gap => "gap",
            Unmeasured::AmmoUnsettled => "ammoUnsettled",
            Unmeasured::LimitedSetUnconfirmed => "limitedSetUnconfirmed",
            Unmeasured::StillRunning => "stillRunning",
            Unmeasured::TrackedOnly => "trackedOnly",
            Unmeasured::PlayInGap => "playInGap",
            Unmeasured::ChatLogMissesGap => "chatLogMissesGap",
            Unmeasured::ChatLogUnchecked => "chatLogUnchecked",
            Unmeasured::GapUnaccounted => "gapUnaccounted",
            Unmeasured::OverheadInGap => "overheadInGap",
            Unmeasured::GapCheckPending => "gapCheckPending",
            Unmeasured::AwaitingGapReading => "awaitingGapReading",
            Unmeasured::ChatLogNotPastGap => "chatLogNotPastGap",
        }
    }

    pub fn parse(value: &str) -> Option<Unmeasured> {
        Unmeasured::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// Whether the session as a whole is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The session's own readings measure it whole.
    Measured,
    /// Measured across a gap at one end or both, from the nearest reading
    /// on the far side, the gap shown clean.
    AcrossGap(GapBounds),
    Unmeasured(Unmeasured),
}

impl Status {
    /// Whether the session's buckets may be measured.
    pub fn is_measured(self) -> bool {
        matches!(self, Status::Measured | Status::AcrossGap(_))
    }

    /// Whether the status rests on what the chat log showed over a gap.
    fn rests_on_a_gap_verdict(self) -> bool {
        matches!(
            self,
            Status::AcrossGap(_)
                | Status::Unmeasured(
                    Unmeasured::PlayInGap
                        | Unmeasured::ChatLogMissesGap
                        | Unmeasured::ChatLogUnchecked
                        | Unmeasured::GapUnaccounted
                )
        )
    }
}

/// What the chat log showed over a session's gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogVerdict {
    /// It covers the gap and shows no play in it.
    Clean,
    /// It shows play outside the session in the gap.
    PlayInGap,
    /// It starts after the gap begins.
    MissesGap,
    /// It could not be checked over the gap, and never will be.
    Unchecked,
}

impl LogVerdict {
    pub const ALL: [LogVerdict; 4] = [
        LogVerdict::Clean,
        LogVerdict::PlayInGap,
        LogVerdict::MissesGap,
        LogVerdict::Unchecked,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LogVerdict::Clean => "clean",
            LogVerdict::PlayInGap => "playInGap",
            LogVerdict::MissesGap => "missesGap",
            LogVerdict::Unchecked => "unchecked",
        }
    }

    pub fn parse(value: &str) -> Option<LogVerdict> {
        LogVerdict::ALL
            .into_iter()
            .find(|verdict| verdict.as_str() == value)
    }
}

/// The span a gap verdict was taken over: the readings either side of the
/// gap and the session's start and end, in whole milliseconds. The same
/// span is never read twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapSpan {
    pub opening_reading_id: Option<i64>,
    pub closing_reading_id: Option<i64>,
    pub started_ms: i64,
    pub ended_ms: i64,
}

/// What the chat log showed over one gap's span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapVerdict {
    pub span: GapSpan,
    pub verdict: LogVerdict,
}

/// A stored gap verdict as read: its opening and closing readings, its
/// span's ends in milliseconds, and the verdict.
type StoredVerdictRow = (Option<i64>, Option<i64>, i64, i64, String);

/// The gap verdict kept for a session, whatever span it was taken over.
fn stored_verdict(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<GapVerdict>, AccountError> {
    let row: Option<StoredVerdictRow> = conn
        .query_row(
            "SELECT opening_reading_id, closing_reading_id, started_ms, ended_ms, verdict \
             FROM account_session_gap_verdicts WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    Ok(
        row.and_then(|(opening, closing, started_ms, ended_ms, verdict)| {
            Some(GapVerdict {
                span: GapSpan {
                    opening_reading_id: opening,
                    closing_reading_id: closing,
                    started_ms,
                    ended_ms,
                },
                verdict: LogVerdict::parse(&verdict)?,
            })
        }),
    )
}

/// The verdict kept for a session over exactly `span`, if any: what the
/// chat log showed there when it was read.
pub(super) fn verdict_over(
    conn: &rusqlite::Connection,
    session_id: &str,
    span: GapSpan,
) -> Result<Option<LogVerdict>, AccountError> {
    Ok(stored_verdict(conn, session_id)?
        .filter(|kept| kept.span == span)
        .map(|kept| kept.verdict))
}

/// One end of a session measured across a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapBound {
    /// The stored reading the gap ends at.
    pub reading_id: i64,
    /// How far outside the session it was taken, in whole seconds.
    pub outside_seconds: i64,
    /// How long the readings were missing across this end, in whole
    /// seconds.
    pub gap_seconds: i64,
}

/// The readings a session measured across a gap rests on: the first and
/// last of them, and the end or ends read across the gap (at least one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapBounds {
    pub first_reading_id: i64,
    pub last_reading_id: i64,
    pub opening: Option<GapBound>,
    pub closing: Option<GapBound>,
}

impl GapBounds {
    /// How long the readings were missing, across both ends.
    pub fn gap_seconds(&self) -> i64 {
        [self.opening, self.closing]
            .into_iter()
            .flatten()
            .map(|end| end.gap_seconds)
            .sum()
    }
}

/// What a bucket's figure rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The readings measure it whole.
    Measured(Amount),
    /// The weapon only: the readings measure all of it (this amount) but
    /// the ammo fired in the steps it was topped up in
    /// ([`SessionCosts::top_ups`]), which the tracked figure stands in for.
    Mixed(Amount),
    /// They do not, for this reason: the bucket has no measured figure.
    Unmeasured(Unmeasured),
}

/// A step Universal Ammo was topped up in: after one reading, by the
/// next, in whole milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TopUp {
    pub from_ms: i64,
    pub to_ms: i64,
}

/// One bucket of a session's costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketCost {
    pub bucket: CostBucket,
    pub basis: Basis,
}

/// Why a line is charged nothing here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineReason {
    /// The player accounts for the item through the ledger, where its
    /// purchase is recorded: charging its use too would count it twice.
    BoughtThroughLedger,
    /// Units of a stack nothing names left: whether they were used up or
    /// traded away is the player's to say. Listed for them to place, and
    /// no cost until they place it as one.
    NotPlaced,
}

impl LineReason {
    pub const ALL: [LineReason; 2] = [LineReason::BoughtThroughLedger, LineReason::NotPlaced];

    pub fn as_str(self) -> &'static str {
        match self {
            LineReason::BoughtThroughLedger => "boughtThroughLedger",
            LineReason::NotPlaced => "notPlaced",
        }
    }

    pub fn parse(value: &str) -> Option<LineReason> {
        LineReason::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// What a line charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    /// TT: what an item lost, or units used up at TT (a limited piece's
    /// line is what it lost, which is not yet its cost).
    AtTt(Amount),
    /// Units used up at the acquisition markup the player recorded for the
    /// item, in hundredths of a percent of TT.
    AtMarkup {
        amount: Amount,
        markup_hundredths: i64,
    },
    /// Nothing here, for this reason.
    Nothing(LineReason),
}

impl Charge {
    /// What the line charges, when it charges anything.
    pub fn amount(self) -> Option<Amount> {
        match self {
            Charge::AtTt(amount) | Charge::AtMarkup { amount, .. } => Some(amount),
            Charge::Nothing(_) => None,
        }
    }
}

/// One item's measured use within a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostLine {
    pub bucket: CostBucket,
    pub category: CostCategory,
    pub kind: LineKind,
    pub item: String,
    /// The units, for a line that counts them.
    pub units: Option<i64>,
    pub charge: Charge,
}

impl CostLine {
    /// What the line charges, when it charges anything.
    pub fn amount(&self) -> Option<Amount> {
        self.charge.amount()
    }
}

/// One part of a session's armour as its readings bill it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmourPart {
    /// Unlimited armour and plates, billed at the TT they lost.
    Unlimited { lost: Amount },
    /// One limited piece: the TT it lost, billed at the markup the player
    /// set for it (in hundredths of a percent) when the session started,
    /// or at that TT when none was set.
    Piece {
        item: String,
        lost: Amount,
        markup_hundredths: Option<i64>,
        cost: Amount,
    },
}

impl ArmourPart {
    /// What the part bills.
    pub fn cost(&self) -> Amount {
        match self {
            ArmourPart::Unlimited { lost } => *lost,
            ArmourPart::Piece { cost, .. } => *cost,
        }
    }
}

/// A session's costs as its readings measure them: every bucket, in the
/// order of [`CostBucket::ALL`], and the lines, ordered by bucket, kind,
/// item and category; the armour in parts, for a session measured whole
/// (the unlimited part first, then each limited piece by name); with the
/// digest of the roles its items were placed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCosts {
    pub status: Status,
    pub buckets: Vec<BucketCost>,
    pub lines: Vec<CostLine>,
    pub armour: Vec<ArmourPart>,
    pub roles_fingerprint: String,
    /// What the chat log showed over the gap these costs were weighed
    /// across, when they were: kept with them, so a rebuild over the same
    /// span reads the log no more.
    pub gap_verdict: Option<GapVerdict>,
    /// The steps the ammo was topped up in, in order, when the weapon
    /// bucket is [`Basis::Mixed`]; empty otherwise.
    pub top_ups: Vec<TopUp>,
}

impl SessionCosts {
    /// The costs of a sealed measure of a session that started at
    /// `started_at`.
    pub fn sealed(
        measure: &SessionMeasure,
        roles: &KnownRoles,
        started_at: f64,
    ) -> Result<SessionCosts, AccountError> {
        SessionCosts::of(measure, roles, coverage_status(measure), started_at)
    }

    /// The costs so far of a session still running or still being read to
    /// its close: its lines as they stand, and no bucket measured.
    pub fn running(
        measure: &SessionMeasure,
        roles: &KnownRoles,
        started_at: f64,
    ) -> Result<SessionCosts, AccountError> {
        SessionCosts::of(
            measure,
            roles,
            Status::Unmeasured(Unmeasured::StillRunning),
            started_at,
        )
    }

    /// The costs of a session still running, up to the last reading its
    /// measure reads: each bucket as a measure that closed there would bill
    /// it. Only for a measure that reads the session's start and every step
    /// since with no gap (see [`super::live`]); never stored as the
    /// session's costs.
    pub(super) fn so_far(
        measure: &SessionMeasure,
        roles: &KnownRoles,
        started_at: f64,
    ) -> Result<SessionCosts, AccountError> {
        SessionCosts::of(measure, roles, Status::Measured, started_at)
    }

    /// The costs of a measure taken across a gap the caller has shown
    /// clean.
    pub(super) fn measured_across_gap(
        measure: &SessionMeasure,
        roles: &KnownRoles,
        bounds: GapBounds,
        started_at: f64,
    ) -> Result<SessionCosts, AccountError> {
        SessionCosts::of(measure, roles, Status::AcrossGap(bounds), started_at)
    }

    /// These costs, with nothing measured, for `reason`: the lines stay as
    /// the readings show them.
    pub(super) fn refused(self, reason: Unmeasured) -> SessionCosts {
        SessionCosts {
            status: Status::Unmeasured(reason),
            buckets: CostBucket::ALL
                .into_iter()
                .map(|bucket| BucketCost {
                    bucket,
                    basis: Basis::Unmeasured(bucket_reason(bucket, reason)),
                })
                .collect(),
            armour: Vec::new(),
            top_ups: Vec::new(),
            ..self
        }
    }

    /// These costs, weighed across the gap the chat log showed as
    /// `verdict`.
    pub(super) fn with_verdict(self, verdict: Option<GapVerdict>) -> SessionCosts {
        SessionCosts {
            gap_verdict: verdict,
            ..self
        }
    }

    /// No bucket measured, for `reason`, and no lines.
    pub fn unmeasured(reason: Unmeasured) -> SessionCosts {
        SessionCosts {
            status: Status::Unmeasured(reason),
            buckets: CostBucket::ALL
                .into_iter()
                .map(|bucket| BucketCost {
                    bucket,
                    basis: Basis::Unmeasured(bucket_reason(bucket, reason)),
                })
                .collect(),
            lines: Vec::new(),
            armour: Vec::new(),
            roles_fingerprint: String::new(),
            gap_verdict: None,
            top_ups: Vec::new(),
        }
    }

    fn of(
        measure: &SessionMeasure,
        roles: &KnownRoles,
        status: Status,
        started_at: f64,
    ) -> Result<SessionCosts, AccountError> {
        // The role the measure gave each item, to place what sits attached
        // to one.
        let mut held: HashMap<&str, Role> = HashMap::new();
        for item in &measure.decay {
            held.insert(item.key.name.as_str(), item.role);
        }
        for item in measure
            .other_consumption
            .iter()
            .chain(&measure.unexplained_departures)
        {
            held.insert(item.key.name.as_str(), item.role);
        }
        let parent_role = |key: &HoldingKey| match &key.container {
            Container::Attached { parent } => known_category(roles, parent)
                .map(CostCategory::role)
                .or_else(|| held.get(parent.as_str()).copied()),
            _ => None,
        };

        let mut lines: BTreeMap<(CostBucket, LineKind, String, CostCategory), (i64, Amount)> =
            BTreeMap::new();
        let mut add = |category: CostCategory,
                       kind: LineKind,
                       item: &str,
                       units: i64,
                       amount: Amount|
         -> Result<(), AccountError> {
            let Some(bucket) = CostBucket::of(category) else {
                return Ok(());
            };
            let entry = lines
                .entry((bucket, kind, item.to_string(), category))
                .or_insert((0, Amount::NONE));
            entry.0 = super::amount::checked(entry.0.checked_add(units))?;
            entry.1 = entry.1.plus(amount)?;
            Ok(())
        };

        let mut ammo_unsettled = false;
        for ammo in &measure.ammo {
            match &ammo.split {
                AmmoSplit::Settled {
                    burned_units,
                    burned,
                    ..
                } if *burned_units > 0 => {
                    add(
                        CostCategory::Ammo,
                        LineKind::AmmoBurned,
                        &ammo.name,
                        *burned_units,
                        *burned,
                    )?;
                }
                AmmoSplit::Settled { .. } => {}
                AmmoSplit::Unsettled { .. } => ammo_unsettled = true,
            }
        }
        // The steps the ammo was topped up in: their burn is the tracked
        // figure's to stand in for.
        let mut top_ups: Vec<TopUp> = measure
            .events
            .iter()
            .filter_map(|event| match event {
                NotableEvent::AmmoToppedUp { between, .. } => Some(TopUp {
                    from_ms: whole_milliseconds(between.from),
                    to_ms: whole_milliseconds(between.to),
                }),
                _ => None,
            })
            .collect();
        top_ups.sort_unstable();
        top_ups.dedup();
        // A weapon measured but for its top-ups leaves those steps whole to
        // the tracked figure, whose price per shot holds the weapon's wear
        // too: what its weapon, amplifier, attachments and implant wore in
        // them is not measured again.
        let withheld = status.is_measured() && !ammo_unsettled && !top_ups.is_empty();
        for item in &measure.decay {
            let category = cost_category(
                roles,
                &item.key,
                item.role,
                Use::Wear,
                parent_role(&item.key),
            );
            let mut decay = item.decay;
            if withheld && CostBucket::of(category) == Some(CostBucket::Weapon) {
                if let Some(held) = measure
                    .topped_up_decay
                    .iter()
                    .find(|held| held.key == item.key)
                {
                    decay = decay.minus(held.decay)?;
                }
            }
            if decay.micro_ped() <= 0 {
                continue;
            }
            let kind = match category {
                CostCategory::LimitedArmour | CostCategory::LimitedPlate => LineKind::LimitedDecay,
                _ => LineKind::Decay,
            };
            add(category, kind, &item.key.name, 0, decay)?;
        }
        for item in &measure.other_consumption {
            let category = cost_category(roles, &item.key, item.role, Use::Units, None);
            // A stack nothing names is listed (charged nothing) so the
            // player can place it: a material leaving is never a cost until
            // they say it was used up.
            // A material (loot the app recorded, stock it holds, what the
            // library or game data know as one) leaving is a trade, never a
            // use, and is not offered.
            let listed = category.is_consumable()
                || (category == CostCategory::Unclassified
                    && !super::roles::is_material(roles, &item.key.name));
            if listed && item.units > 0 {
                add(
                    category,
                    LineKind::Consumed,
                    &item.key.name,
                    item.units,
                    item.amount,
                )?;
            }
        }

        let mut charged = Vec::with_capacity(lines.len());
        for ((bucket, kind, item, category), (units, amount)) in lines {
            // A consumable used up follows the player's choice for it.
            let charge = match (kind, roles.dose_pricing(&item)) {
                (LineKind::Consumed, _) if category == CostCategory::Unclassified => {
                    Charge::Nothing(LineReason::NotPlaced)
                }
                (LineKind::Consumed, Some(DosePricing::Booked { markup_hundredths })) => {
                    Charge::AtMarkup {
                        amount: at_markup(amount, markup_hundredths)?,
                        markup_hundredths,
                    }
                }
                (LineKind::Consumed, Some(DosePricing::ThroughLedger)) => {
                    Charge::Nothing(LineReason::BoughtThroughLedger)
                }
                _ => Charge::AtTt(amount),
            };
            charged.push(CostLine {
                bucket,
                category,
                kind,
                item,
                units: kind.counts_units().then_some(units),
                charge,
            });
        }
        let lines = charged;
        let armour = if status.is_measured() {
            armour_parts(&lines, roles, started_at)?
        } else {
            Vec::new()
        };
        let mut buckets = Vec::with_capacity(CostBucket::ALL.len());
        for bucket in CostBucket::ALL {
            let in_bucket = || lines.iter().filter(move |line| line.bucket == bucket);
            let reason = match status {
                _ if bucket == CostBucket::Enhancer => Some(Unmeasured::TrackedOnly),
                Status::Unmeasured(reason) => Some(reason),
                Status::Measured | Status::AcrossGap(_) => {
                    if bucket == CostBucket::Weapon && ammo_unsettled {
                        Some(Unmeasured::AmmoUnsettled)
                    } else {
                        None
                    }
                }
            };
            let basis = match reason {
                Some(reason) => Basis::Unmeasured(reason),
                // The armour is what its parts bill: a limited piece at its
                // markup, not at the TT its line lists.
                None if bucket == CostBucket::Armour => {
                    Basis::Measured(Amount::total(armour.iter().map(ArmourPart::cost))?)
                }
                // Measured but for the steps the ammo was topped up in.
                None if bucket == CostBucket::Weapon && !top_ups.is_empty() => {
                    Basis::Mixed(Amount::total(in_bucket().filter_map(CostLine::amount))?)
                }
                None => Basis::Measured(Amount::total(in_bucket().filter_map(CostLine::amount))?),
            };
            buckets.push(BucketCost { bucket, basis });
        }
        let weapon_mixed = buckets
            .iter()
            .any(|cost| matches!(cost.basis, Basis::Mixed(_)));
        Ok(SessionCosts {
            status,
            buckets,
            lines,
            armour,
            roles_fingerprint: roles.fingerprint().to_string(),
            gap_verdict: None,
            top_ups: if weapon_mixed { top_ups } else { Vec::new() },
        })
    }

    /// The bucket's cost.
    pub fn bucket(&self, bucket: CostBucket) -> Option<&BucketCost> {
        self.buckets.iter().find(|cost| cost.bucket == bucket)
    }
}

/// A measured session's armour in parts, from its armour lines: the
/// unlimited pieces' loss, at TT; then each limited piece's loss, at the
/// markup the player set for it as it stood when the session started
/// (`at`), or at TT when none was set, by name.
fn armour_parts(
    lines: &[CostLine],
    roles: &KnownRoles,
    at: f64,
) -> Result<Vec<ArmourPart>, AccountError> {
    let mut unlimited = Amount::NONE;
    let mut pieces: BTreeMap<String, Amount> = BTreeMap::new();
    for line in lines
        .iter()
        .filter(|line| line.bucket == CostBucket::Armour)
    {
        let lost = line.charge.amount().unwrap_or(Amount::NONE);
        match line.kind {
            LineKind::LimitedDecay => {
                let entry = pieces.entry(line.item.clone()).or_insert(Amount::NONE);
                *entry = entry.plus(lost)?;
            }
            _ => unlimited = unlimited.plus(lost)?,
        }
    }
    let mut parts = vec![ArmourPart::Unlimited { lost: unlimited }];
    for (item, lost) in pieces {
        let markup_hundredths = roles.piece_markup(&item, at);
        let cost = match markup_hundredths {
            Some(markup) => at_markup(lost, markup)?,
            None => lost,
        };
        parts.push(ArmourPart::Piece {
            item,
            lost,
            markup_hundredths,
            cost,
        });
    }
    Ok(parts)
}

/// `amount` at `markup_hundredths` hundredths of a percent, to the nearest
/// micro-PED: the markup is the player's own figure, so the result keeps
/// the precision of the TT it multiplies.
fn at_markup(amount: Amount, markup_hundredths: i64) -> Result<Amount, AccountError> {
    let scaled = i128::from(amount.micro_ped()) * i128::from(markup_hundredths);
    let rounded = (scaled + scaled.signum() * 5_000) / 10_000;
    let micro_ped = i64::try_from(rounded).map_err(|_| AccountError::Overflow)?;
    Ok(match amount {
        Amount::Exact { .. } => Amount::Exact { micro_ped },
        Amount::WholePec { .. } => Amount::WholePec { micro_ped },
    })
}

/// The reason a bucket is unmeasured when the whole session is: its own
/// for the enhancer, which the readings never measure.
fn bucket_reason(bucket: CostBucket, reason: Unmeasured) -> Unmeasured {
    if bucket == CostBucket::Enhancer {
        Unmeasured::TrackedOnly
    } else {
        reason
    }
}

/// A unix time in seconds as whole milliseconds.
pub(super) fn whole_milliseconds(at: f64) -> i64 {
    // A reading's time: finite and far inside i64 once in ms.
    (at * 1_000.0).round() as i64
}

/// Whether a measure covers its session whole, and why not.
fn coverage_status(measure: &SessionMeasure) -> Status {
    let coverage = measure.coverage;
    if coverage.level == CoverageLevel::Complete {
        Status::Measured
    } else if coverage.opening == Edge::Unread || coverage.closing == Edge::Unread {
        Status::Unmeasured(Unmeasured::PartialCoverage)
    } else if coverage.gap_count > 0 {
        Status::Unmeasured(Unmeasured::Gap)
    } else {
        Status::Unmeasured(Unmeasured::PartialCoverage)
    }
}

/// Let go of a session's stored costs: when its measure is written again,
/// or the session is deleted. Run inside that write's own transaction.
pub fn clear(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<()> {
    for table in [
        "account_session_cost_top_ups",
        "account_session_cost_gaps",
        "account_session_cost_armour_parts",
        "account_session_cost_armour_pieces",
        "account_session_cost_lines",
        "account_session_cost_buckets",
        "account_session_costs",
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?1"),
            [session_id],
        )?;
    }
    Ok(())
}

/// Store a sealed session's costs, derived from its sealed measure as
/// computed at `measure_computed_at`, replacing any stored before. Written
/// only while that measure is the one stored, sealed, by the current
/// rules; `Ok(false)` when it is not, and nothing changes. Costs in a
/// passing state (a running session's, a session with no readings, a gap
/// not weighed yet: see [`Unmeasured::is_passing`]) are never stored. A
/// short write: run it inside the seal's transaction, or one of its own.
pub fn store_costs(
    conn: &rusqlite::Connection,
    session_id: &str,
    costs: &SessionCosts,
    measure_computed_at: f64,
    computed_at: f64,
) -> Result<bool, AccountError> {
    if matches!(costs.status, Status::Unmeasured(reason) if reason.is_passing()) {
        return Ok(false);
    }
    let current: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM account_session_measures \
                        WHERE session_id = ?1 AND sealed_at IS NOT NULL \
                          AND measure_version = ?2 AND computed_at = ?3)",
        rusqlite::params![session_id, MEASURE_VERSION, measure_computed_at],
        |row| row.get(0),
    )?;
    if !current {
        return Ok(false);
    }
    clear(conn, session_id)?;
    let (status, reason, bounds) = match costs.status {
        Status::Measured => ("measured", None, None),
        Status::AcrossGap(bounds) => ("acrossGap", None, Some(bounds)),
        Status::Unmeasured(reason) => ("unmeasured", Some(reason.as_str()), None),
    };
    // A measure taken across a gap rests on the readings that bound it;
    // any other on the sealed measure's.
    conn.execute(
        "INSERT INTO account_session_costs \
             (session_id, costs_version, measure_version, measure_computed_at, status, reason, \
              first_reading_id, last_reading_id, computed_at, roles_fingerprint) \
         SELECT session_id, ?2, measure_version, computed_at, ?3, ?4, \
                COALESCE(?7, first_reading_id), COALESCE(?8, last_reading_id), ?5, ?6 \
         FROM account_session_measures WHERE session_id = ?1",
        rusqlite::params![
            session_id,
            COSTS_VERSION,
            status,
            reason,
            computed_at,
            costs.roles_fingerprint,
            bounds.map(|bounds| bounds.first_reading_id),
            bounds.map(|bounds| bounds.last_reading_id),
        ],
    )?;
    if let Some(bounds) = bounds {
        let end = |end: Option<GapBound>| {
            (
                end.map(|end| end.reading_id),
                end.map(|end| end.outside_seconds),
                end.map(|end| end.gap_seconds),
            )
        };
        let (opening, opening_outside, opening_gap) = end(bounds.opening);
        let (closing, closing_outside, closing_gap) = end(bounds.closing);
        conn.execute(
            "INSERT INTO account_session_cost_gaps \
                 (session_id, opening_reading_id, opening_outside_seconds, opening_gap_seconds, \
                  closing_reading_id, closing_outside_seconds, closing_gap_seconds) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                session_id,
                opening,
                opening_outside,
                opening_gap,
                closing,
                closing_outside,
                closing_gap
            ],
        )?;
    }
    // The chat log's verdict outlives the projection: a rebuild over the
    // same span reads it rather than the log.
    if let Some(kept) = costs.gap_verdict {
        conn.execute(
            "INSERT INTO account_session_gap_verdicts \
                 (session_id, opening_reading_id, closing_reading_id, started_ms, ended_ms, \
                  verdict, taken_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(session_id) DO UPDATE SET \
                 opening_reading_id = excluded.opening_reading_id, \
                 closing_reading_id = excluded.closing_reading_id, \
                 started_ms = excluded.started_ms, ended_ms = excluded.ended_ms, \
                 verdict = excluded.verdict, taken_at = excluded.taken_at \
             WHERE account_session_gap_verdicts.opening_reading_id \
                     IS NOT excluded.opening_reading_id \
                OR account_session_gap_verdicts.closing_reading_id \
                     IS NOT excluded.closing_reading_id \
                OR account_session_gap_verdicts.started_ms != excluded.started_ms \
                OR account_session_gap_verdicts.ended_ms != excluded.ended_ms",
            rusqlite::params![
                session_id,
                kept.span.opening_reading_id,
                kept.span.closing_reading_id,
                kept.span.started_ms,
                kept.span.ended_ms,
                kept.verdict.as_str(),
                computed_at,
            ],
        )?;
    }
    let mut insert = conn.prepare(
        "INSERT INTO account_session_cost_buckets \
             (session_id, bucket, basis, reason, amount_micro_ped, precision) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for cost in &costs.buckets {
        let (basis, reason, amount) = match cost.basis {
            Basis::Measured(amount) => ("measured", None, Some(amount)),
            Basis::Mixed(amount) => ("mixed", None, Some(amount)),
            Basis::Unmeasured(reason) => ("unmeasured", Some(reason.as_str()), None),
        };
        insert.execute(rusqlite::params![
            session_id,
            cost.bucket.as_str(),
            basis,
            reason,
            amount.map(Amount::micro_ped),
            amount.map(|amount| precision_str(amount.precision())),
        ])?;
    }
    let mut insert = conn.prepare(
        "INSERT INTO account_session_cost_lines \
             (session_id, position, bucket, category, kind, item_name, units, \
              amount_micro_ped, precision, markup_hundredths, reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (position, line) in costs.lines.iter().enumerate() {
        let (markup, reason) = match line.charge {
            Charge::AtTt(_) => (None, None),
            Charge::AtMarkup {
                markup_hundredths, ..
            } => (Some(markup_hundredths), None),
            Charge::Nothing(reason) => (None, Some(reason.as_str())),
        };
        let amount = line.amount();
        insert.execute(rusqlite::params![
            session_id,
            super::amount::count(position)?,
            line.bucket.as_str(),
            line.category.as_str(),
            line.kind.as_str(),
            line.item,
            line.units,
            amount.map(Amount::micro_ped),
            amount.map(|amount| precision_str(amount.precision())),
            markup,
            reason,
        ])?;
    }
    let mut insert = conn.prepare(
        "INSERT INTO account_session_cost_top_ups (session_id, from_ms, to_ms) \
         VALUES (?1, ?2, ?3)",
    )?;
    for top_up in &costs.top_ups {
        insert.execute(rusqlite::params![session_id, top_up.from_ms, top_up.to_ms])?;
    }
    let mut unlimited = conn.prepare(
        "INSERT INTO account_session_cost_armour_parts \
             (session_id, part, set_id, lost_micro_ped, lost_precision, markup_hundredths, \
              amount_micro_ped, precision) \
         VALUES (?1, 'unlimited', NULL, ?2, ?3, NULL, ?2, ?3)",
    )?;
    let mut piece = conn.prepare(
        "INSERT INTO account_session_cost_armour_pieces \
             (session_id, item_name, lost_micro_ped, lost_precision, markup_hundredths, \
              amount_micro_ped, precision) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for part in &costs.armour {
        match part {
            ArmourPart::Unlimited { lost } => {
                unlimited.execute(rusqlite::params![
                    session_id,
                    lost.micro_ped(),
                    precision_str(lost.precision()),
                ])?;
            }
            ArmourPart::Piece {
                item,
                lost,
                markup_hundredths,
                cost,
            } => {
                piece.execute(rusqlite::params![
                    session_id,
                    item,
                    lost.micro_ped(),
                    precision_str(lost.precision()),
                    markup_hundredths,
                    cost.micro_ped(),
                    precision_str(cost.precision()),
                ])?;
            }
        }
    }
    Ok(true)
}

fn precision_str(precision: super::amount::Precision) -> &'static str {
    match precision {
        super::amount::Precision::Exact => "exact",
        super::amount::Precision::WholePec => "wholePec",
    }
}

/// A stored amount, read with its precision; `None` for a precision this
/// build does not know.
pub(super) fn amount_of(micro_ped: i64, precision: &str) -> Option<Amount> {
    match precision {
        "exact" => Some(Amount::Exact { micro_ped }),
        "wholePec" => Some(Amount::WholePec { micro_ped }),
        _ => None,
    }
}

/// Run a read of several statements as one snapshot of the database: in a
/// read transaction of its own, unless the connection is already in one.
fn in_one_read<T>(
    conn: &rusqlite::Connection,
    read: impl FnOnce() -> Result<T, AccountError>,
) -> Result<T, AccountError> {
    let snapshot = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let result = read();
    // Only read: ending it either way changes nothing.
    drop(snapshot);
    result
}

/// A session's stored costs, when they are current: by these rules, from
/// the measure stored for it now, sealed by the current rules, with its
/// items placed by the roles whose digest is `roles_fingerprint`. `None`
/// when none are stored, or they are stale, or they do not decode (a later
/// build's): any of these is rebuilt. The header, buckets and lines are
/// read as one snapshot.
pub fn stored_costs(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles_fingerprint: &str,
) -> Result<Option<SessionCosts>, AccountError> {
    in_one_read(conn, || read_stored(conn, session_id, roles_fingerprint))
}

fn read_stored(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles_fingerprint: &str,
) -> Result<Option<SessionCosts>, AccountError> {
    let header = conn
        .query_row(
            "SELECT c.status, c.reason, c.first_reading_id, c.last_reading_id \
             FROM account_session_costs c \
             JOIN account_session_measures m ON m.session_id = c.session_id \
             WHERE c.session_id = ?1 AND c.costs_version = ?2 \
               AND c.measure_version = ?3 AND m.measure_version = ?3 \
               AND m.sealed_at IS NOT NULL AND m.computed_at = c.measure_computed_at \
               AND c.roles_fingerprint = ?4",
            rusqlite::params![
                session_id,
                COSTS_VERSION,
                MEASURE_VERSION,
                roles_fingerprint
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((status, reason, first, last)) = header else {
        return Ok(None);
    };
    let status = match (status.as_str(), reason.as_deref().map(Unmeasured::parse)) {
        ("measured", None) => Status::Measured,
        ("acrossGap", None) => match (first, last, read_gap(conn, session_id)?) {
            (Some(first_reading_id), Some(last_reading_id), Some((opening, closing))) => {
                Status::AcrossGap(GapBounds {
                    first_reading_id,
                    last_reading_id,
                    opening,
                    closing,
                })
            }
            _ => return Ok(None),
        },
        ("unmeasured", Some(Some(reason))) if !reason.is_passing() => Status::Unmeasured(reason),
        _ => return Ok(None),
    };
    type RawBucket = (String, String, Option<String>, Option<i64>, Option<String>);
    let raw_buckets: Vec<RawBucket> = {
        let mut stmt = conn.prepare(
            "SELECT bucket, basis, reason, amount_micro_ped, precision \
             FROM account_session_cost_buckets WHERE session_id = ?1",
        )?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut buckets = Vec::with_capacity(raw_buckets.len());
    for (bucket, basis, reason, amount, precision) in raw_buckets {
        let Some(bucket) = CostBucket::parse(&bucket) else {
            return Ok(None);
        };
        let basis = match (basis.as_str(), reason, amount, precision) {
            ("measured", None, Some(amount), Some(precision)) => {
                match amount_of(amount, &precision) {
                    Some(amount) => Basis::Measured(amount),
                    None => return Ok(None),
                }
            }
            ("mixed", None, Some(amount), Some(precision)) if bucket == CostBucket::Weapon => {
                match amount_of(amount, &precision) {
                    Some(amount) => Basis::Mixed(amount),
                    None => return Ok(None),
                }
            }
            ("unmeasured", Some(reason), None, None) => match Unmeasured::parse(&reason) {
                Some(reason) => Basis::Unmeasured(reason),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        buckets.push(BucketCost { bucket, basis });
    }
    buckets.sort_by_key(|cost| cost.bucket);
    if buckets.iter().map(|cost| cost.bucket).ne(CostBucket::ALL) {
        return Ok(None);
    }
    type RawLine = (
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    );
    let raw_lines: Vec<RawLine> = {
        let mut stmt = conn.prepare(
            "SELECT bucket, category, kind, item_name, units, amount_micro_ped, precision, \
                    markup_hundredths, reason \
             FROM account_session_cost_lines WHERE session_id = ?1 ORDER BY position",
        )?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut lines = Vec::with_capacity(raw_lines.len());
    for (bucket, category, kind, item, units, amount, precision, markup, reason) in raw_lines {
        let (Some(bucket), Some(category), Some(kind)) = (
            CostBucket::parse(&bucket),
            CostCategory::parse(&category),
            LineKind::parse(&kind),
        ) else {
            return Ok(None);
        };
        let amount = match (amount, precision) {
            (Some(amount), Some(precision)) => match amount_of(amount, &precision) {
                Some(amount) => Some(amount),
                None => return Ok(None),
            },
            (None, None) => None,
            _ => return Ok(None),
        };
        let charge = match (amount, markup, reason.as_deref().map(LineReason::parse)) {
            (Some(amount), None, None) => Charge::AtTt(amount),
            (Some(amount), Some(markup_hundredths), None) => Charge::AtMarkup {
                amount,
                markup_hundredths,
            },
            (None, None, Some(Some(reason))) => Charge::Nothing(reason),
            _ => return Ok(None),
        };
        lines.push(CostLine {
            bucket,
            category,
            kind,
            item,
            units,
            charge,
        });
    }
    let Some(armour) = read_armour_parts(conn, session_id)? else {
        return Ok(None);
    };
    // The steps the ammo was topped up in belong to a mixed weapon bucket,
    // and to nothing else.
    let top_ups = read_top_ups(conn, session_id)?;
    let weapon_mixed = buckets
        .iter()
        .any(|cost| matches!(cost.basis, Basis::Mixed(_)));
    if weapon_mixed == top_ups.is_empty() {
        return Ok(None);
    }
    let gap_verdict = if status.rests_on_a_gap_verdict() {
        stored_verdict(conn, session_id)?
    } else {
        None
    };
    Ok(Some(SessionCosts {
        status,
        buckets,
        lines,
        armour,
        roles_fingerprint: roles_fingerprint.to_string(),
        gap_verdict,
        top_ups,
    }))
}

/// A session's stored top-up steps, in order.
fn read_top_ups(conn: &rusqlite::Connection, session_id: &str) -> Result<Vec<TopUp>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT from_ms, to_ms FROM account_session_cost_top_ups \
         WHERE session_id = ?1 ORDER BY from_ms, to_ms",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(TopUp {
                from_ms: row.get(0)?,
                to_ms: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A session's ends read across a gap: the opening's, then the closing's.
type GapEnds = (Option<GapBound>, Option<GapBound>);

/// A session's stored gap ends, opening then closing; `None` when there is
/// no row, or it does not decode.
fn read_gap(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<GapEnds>, AccountError> {
    type End = (Option<i64>, Option<i64>, Option<i64>);
    let row: Option<(End, End)> = conn
        .query_row(
            "SELECT opening_reading_id, opening_outside_seconds, opening_gap_seconds, \
                    closing_reading_id, closing_outside_seconds, closing_gap_seconds \
             FROM account_session_cost_gaps WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    (row.get(0)?, row.get(1)?, row.get(2)?),
                    (row.get(3)?, row.get(4)?, row.get(5)?),
                ))
            },
        )
        .optional()?;
    let Some((opening, closing)) = row else {
        return Ok(None);
    };
    let decode = |end: End| match end {
        (None, None, None) => Ok(None),
        (Some(reading_id), Some(outside_seconds), Some(gap_seconds)) => Ok(Some(GapBound {
            reading_id,
            outside_seconds,
            gap_seconds,
        })),
        _ => Err(()),
    };
    Ok(match (decode(opening), decode(closing)) {
        (Ok(opening), Ok(closing)) if opening.is_some() || closing.is_some() => {
            Some((opening, closing))
        }
        _ => None,
    })
}

/// A session's stored armour parts, in their order; `None` when one does
/// not decode.
fn read_armour_parts(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<Vec<ArmourPart>>, AccountError> {
    type RawPart = (String, i64, String, Option<i64>, Option<i64>);
    let unlimited: Vec<RawPart> = {
        let mut stmt = conn.prepare(
            "SELECT part, lost_micro_ped, lost_precision, set_id, amount_micro_ped \
             FROM account_session_cost_armour_parts WHERE session_id = ?1",
        )?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut parts = Vec::with_capacity(unlimited.len());
    for (part, lost, precision, set_id, amount) in unlimited {
        let Some(lost) = amount_of(lost, &precision) else {
            return Ok(None);
        };
        // Only unlimited armour is kept here; anything else is another
        // version's, so the costs are rebuilt.
        if part != "unlimited" || set_id.is_some() || amount != Some(lost.micro_ped()) {
            return Ok(None);
        }
        parts.push(ArmourPart::Unlimited { lost });
    }
    type RawPiece = (String, i64, String, Option<i64>, i64, String);
    let pieces: Vec<RawPiece> = {
        let mut stmt = conn.prepare(
            "SELECT item_name, lost_micro_ped, lost_precision, markup_hundredths, \
                    amount_micro_ped, precision \
             FROM account_session_cost_armour_pieces WHERE session_id = ?1 ORDER BY item_name",
        )?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (item, lost, lost_precision, markup_hundredths, cost, precision) in pieces {
        let (Some(lost), Some(cost)) = (
            amount_of(lost, &lost_precision),
            amount_of(cost, &precision),
        ) else {
            return Ok(None);
        };
        parts.push(ArmourPart::Piece {
            item,
            lost,
            markup_hundredths,
            cost,
        });
    }
    Ok(Some(parts))
}

/// What a read of a session's costs leaves to write.
#[derive(Debug, Clone, PartialEq)]
pub enum CostsToWrite {
    /// The read sealed the session's measure: the measure and its costs,
    /// written together.
    Seal(Box<ToSeal>),
    /// Costs derived from the sealed measure as stored, computed at
    /// `measure_computed_at`.
    Derived {
        session_id: String,
        costs: Box<SessionCosts>,
        measure_computed_at: f64,
    },
}

/// A sealed session's costs: those of its sealed measure, or, when that
/// leaves an end unread, those measured across the gap where the gap is
/// shown clean (see [`super::gap`]), against the chat log `chat_log`. A
/// read: run it on a reader, off any runtime worker.
pub fn sealed_costs(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    measure: &SessionMeasure,
    roles: &KnownRoles,
    chat_log: Option<&ChatLogSource>,
) -> Result<SessionCosts, AccountError> {
    let costs = SessionCosts::sealed(measure, roles, window.started_at)?;
    let coverage = measure.coverage;
    let unread = coverage.opening == Edge::Unread || coverage.closing == Edge::Unread;
    if unread && costs.status == Status::Unmeasured(Unmeasured::PartialCoverage) {
        return super::gap::across_gap(conn, session_id, window, roles, chat_log, costs);
    }
    Ok(costs)
}

/// Read a session's costs, on a reader: the stored ones when they are
/// current, otherwise derived from its sealed measure (see
/// [`sealed_costs`]), or from its measure read as
/// [`sessions::session_measure`] reads it (sealing it when the session has
/// ended and is not `being_read`). What the read leaves to store comes
/// back in the second place, for [`write_costs`]. `None` for a session
/// that does not exist.
pub fn read_session_costs(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles: &KnownRoles,
    chat_log: Option<&ChatLogSource>,
    now: f64,
    being_read: Option<&str>,
) -> Result<(Option<SessionCosts>, Option<CostsToWrite>), AccountError> {
    if let Some(stored) = stored_costs(conn, session_id, roles.fingerprint())? {
        return Ok((Some(stored), None));
    }
    let Some(session) = sessions::tracked_session(conn, session_id)? else {
        return Ok((None, None));
    };
    let window = session.window;
    let started_at = window.started_at;
    if let CachedMeasure::Current {
        measure,
        sealed_at: Some(_),
        computed_at,
    } = cached_measure(conn, session_id)?
    {
        let costs = sealed_costs(conn, session_id, &window, &measure, roles, chat_log)?;
        return Ok((
            Some(costs.clone()),
            Some(CostsToWrite::Derived {
                session_id: session_id.to_string(),
                costs: Box::new(costs),
                measure_computed_at: computed_at,
            }),
        ));
    }
    let (read, to_seal) =
        sessions::session_measure(conn, session_id, roles, chat_log, now, being_read)?;
    Ok(match (read, to_seal) {
        (MeasureRead::UnknownSession, _) => (None, None),
        (MeasureRead::NoReadings, _) => {
            (Some(SessionCosts::unmeasured(Unmeasured::NoReadings)), None)
        }
        (MeasureRead::Measured { .. }, Some(seal)) => (
            Some(seal.costs.clone()),
            Some(CostsToWrite::Seal(Box::new(seal))),
        ),
        (
            MeasureRead::Measured {
                measure,
                sealed: true,
            },
            None,
        ) => (
            Some(sealed_costs(
                conn, session_id, &window, &measure, roles, chat_log,
            )?),
            None,
        ),
        (
            MeasureRead::Measured {
                measure,
                sealed: false,
            },
            None,
        ) => (
            Some(SessionCosts::running(&measure, roles, started_at)?),
            None,
        ),
    })
}

/// Write what a read of costs left, only while it is still the truth, and
/// bring every figure derived from the costs up to date in the same short
/// write. `Ok(false)` when it no longer holds and nothing changed; the
/// next read computes again.
pub fn write_costs(
    conn: &mut rusqlite::Connection,
    write: CostsToWrite,
    computed_at: f64,
) -> Result<bool, AccountError> {
    match write {
        CostsToWrite::Seal(seal) => Ok(sessions::cache_sealed(conn, &[*seal], computed_at)? == 1),
        CostsToWrite::Derived {
            session_id,
            costs,
            measure_computed_at,
        } => {
            let tx = conn.transaction()?;
            let written = if same_as_stored(&tx, &session_id, &costs, measure_computed_at)? {
                // Placed by other roles to the same costs: only the roles
                // they were placed by move, and nothing derived from them.
                tx.execute(
                    "UPDATE account_session_costs SET roles_fingerprint = ?2 \
                     WHERE session_id = ?1",
                    rusqlite::params![session_id, costs.roles_fingerprint],
                )?;
                true
            } else {
                let written =
                    store_costs(&tx, &session_id, &costs, measure_computed_at, computed_at)?;
                if written {
                    crate::session_costs::refresh_derived(&tx, &session_id)?;
                }
                written
            };
            tx.commit()?;
            Ok(written)
        }
    }
}

/// Whether the costs stored for a session, derived from the measure
/// computed at `measure_computed_at` by these rules, are `costs` but for
/// the roles they were placed by: a change to the library, the sets or a
/// placement that moved none of the session's items.
fn same_as_stored(
    conn: &rusqlite::Connection,
    session_id: &str,
    costs: &SessionCosts,
    measure_computed_at: f64,
) -> Result<bool, AccountError> {
    let stored_roles: Option<String> = conn
        .query_row(
            "SELECT roles_fingerprint FROM account_session_costs \
             WHERE session_id = ?1 AND costs_version = ?2 AND measure_version = ?3 \
               AND measure_computed_at = ?4 AND roles_fingerprint IS NOT NULL",
            rusqlite::params![
                session_id,
                COSTS_VERSION,
                MEASURE_VERSION,
                measure_computed_at
            ],
            |row| row.get(0),
        )
        .optional()?;
    let Some(stored_roles) = stored_roles else {
        return Ok(false);
    };
    Ok(match read_stored(conn, session_id, &stored_roles)? {
        Some(stored) => {
            stored
                == SessionCosts {
                    roles_fingerprint: stored_roles,
                    ..costs.clone()
                }
        }
        None => false,
    })
}

/// The sessions whose stored costs are missing or not current: every
/// session with a sealed measure (by any rules) whose costs are not those
/// of that measure, by these rules, placed by the roles whose digest is
/// `roles_fingerprint`.
pub fn sessions_with_stale_costs(
    conn: &rusqlite::Connection,
    roles_fingerprint: &str,
) -> Result<Vec<String>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT m.session_id FROM account_session_measures m \
         WHERE m.sealed_at IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM account_session_costs c \
                           WHERE c.session_id = m.session_id AND c.costs_version = ?1 \
                             AND c.measure_version = ?2 AND m.measure_version = ?2 \
                             AND c.measure_computed_at = m.computed_at \
                             AND c.roles_fingerprint = ?3) \
         ORDER BY m.session_id",
    )?;
    let ids = stmt
        .query_map(
            rusqlite::params![COSTS_VERSION, MEASURE_VERSION, roles_fingerprint],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// What bringing every stale projection up to date writes: each such
/// session's costs read as [`read_session_costs`] reads them, on a reader.
pub fn costs_to_heal(
    conn: &rusqlite::Connection,
    roles: &KnownRoles,
    chat_log: Option<&ChatLogSource>,
    now: f64,
    being_read: Option<&str>,
) -> Result<Vec<CostsToWrite>, AccountError> {
    let mut writes = Vec::new();
    for session_id in sessions_with_stale_costs(conn, roles.fingerprint())? {
        let (_, write) = read_session_costs(conn, &session_id, roles, chat_log, now, being_read)?;
        writes.extend(write);
    }
    Ok(writes)
}

/// Bring every stale projection up to date: computed on a reader, then
/// each written in a short guarded write of its own (with every figure
/// derived from it), so a projection that changed meanwhile is left for
/// the next pass. Returns how many were written. Blocking: run it on the
/// blocking pool, never on a runtime worker.
pub fn heal_session_costs(
    db: &Db,
    game_data: Option<std::sync::Arc<GameDataStore>>,
    chat_log: Option<ChatLogSource>,
    now: f64,
    being_read: Option<String>,
) -> Result<usize, AccountError> {
    let writes = db
        .with_reader_blocking(move |conn| {
            let roles = KnownRoles::read(conn, game_data.as_deref());
            Ok(roles.and_then(|roles| {
                costs_to_heal(conn, &roles, chat_log.as_ref(), now, being_read.as_deref())
            }))
        })
        .map_err(AccountError::from)??;
    let mut written = 0;
    for write in writes {
        let stored = db
            .with_writer_blocking(move |conn| Ok(write_costs(conn, write, now)))
            .map_err(AccountError::from)??;
        written += usize::from(stored);
    }
    Ok(written)
}

/// A session's costs, read and, where the read leaves something to store,
/// stored, on one connection, with no chat log to show a gap clean.
#[cfg(test)]
pub(crate) fn session_costs(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    game_data: Option<&GameDataStore>,
    now: f64,
    being_read: Option<&str>,
) -> Result<Option<SessionCosts>, AccountError> {
    session_costs_with(conn, session_id, game_data, None, now, being_read)
}

/// [`session_costs`], against the chat log `chat_log`.
#[cfg(test)]
pub(crate) fn session_costs_with(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    game_data: Option<&GameDataStore>,
    chat_log: Option<&ChatLogSource>,
    now: f64,
    being_read: Option<&str>,
) -> Result<Option<SessionCosts>, AccountError> {
    let roles = KnownRoles::read(conn, game_data)?;
    let (costs, write) = read_session_costs(conn, session_id, &roles, chat_log, now, being_read)?;
    if let Some(write) = write {
        write_costs(conn, write, now)?;
    }
    Ok(costs)
}
