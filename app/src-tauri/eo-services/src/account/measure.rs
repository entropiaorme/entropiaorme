//! The measured account of one tracked session, from its readings.
//!
//! A pure function of the session's window, its readings in the order they
//! were stored, and the roles of what they hold. It stands beside the
//! session's tracked figures and never feeds them: it reads and writes no
//! accounting table.
//!
//! A session's readings are its own: those the connection tagged with it,
//! with the reading stored just before them and the one stored just after
//! them where an end needs them (see [`measure_own_readings`]). Readings
//! that carry no tag (a recording, say) are chosen by the session's window
//! instead: those taken inside it, with the reading stored just before them
//! and the one stored just after them (see [`measure_session`]).
//!
//! The start is read by a reading taken at or before it, at most 90
//! seconds before, or by the session's own baseline (the reading taken
//! because the session started, which lands a moment after it) when that
//! is its first reading and was taken at most 10 seconds after the start.
//! The end is read by a reading taken at or after it, at most 90 seconds
//! after. The session's closing readings after that one stay in its
//! measure: the site can lag a minute behind the game, so a later closing
//! reading may still be catching up on the session's own play. A reading
//! taken across another session's play never belongs to this one: one
//! taken before the session before stopped does not read the start, and
//! none taken at or after the next session's start is in the measure (see
//! [`Neighbours`]). So a session started inside the reading floor opens
//! with the reading that closed the one before, and no change is measured
//! for two sessions, or for the time between them. The
//! measure is complete only with both ends read, at least two readings,
//! and no gap between consecutive readings: an interval above 150
//! seconds, or time running backwards between two readings, is a gap,
//! counted. A gap is never filled with an estimate: what changed across it
//! is measured as one step, and the coverage says so. A session that has
//! ended with an end unread may also be measured across the gap there,
//! from the nearest reading on the far side however far it was taken (see
//! [`measure_across_gap`]): a measure of its own kind, never a complete
//! one, which the caller bills only once the gap is shown clean.
//!
//! Nothing that arrives or leaves vanishes from the measure, and nothing
//! is booked as a cost or a gain on a guess:
//!
//! - Shrapnel that leaves beside a new ammo row of exactly what the Trade
//!   Terminal pays for it was turned into ammo, credited as converted;
//!   Shrapnel that leaves without that row is an unexplained departure,
//!   and its ammo type's split is unsettled;
//! - the PED Card is never a cost, and never decides one. Its change in a
//!   step is read only for the uses that name it: what left may be a sale,
//!   what arrived a purchase, and a repair costs within what its
//!   restorations allow (shown beside the repair, never billed). When
//!   exactly one combination fits, that is what happened; a movement no
//!   combination explains is ignored, and what left or arrived in its step
//!   counts as in any other step;
//! - Universal Ammo that arrives in a step (a new row, or a row grown past
//!   what the step's other changes allow) with no Shrapnel leaving and no
//!   strongbox opening behind it was topped up. How much was fired in that
//!   step is then not known from the rows, so the step's burn is left out
//!   of the split and the step is listed ([`NotableEvent::AmmoToppedUp`])
//!   for the tracked figure to stand in for; what arrived is never gained.
//!   The trigger is the arrival, never a net rise: a top-up nets to a fall
//!   whenever more was fired than bought;
//! - a stack leaving the auction with no matching move is a listing that
//!   sold;
//! - single items that arrive count in what was gained, marked as items;
//! - what a strongbox opening may have produced stays in what was gained,
//!   marked as possibly its contents, with a total of its own;
//! - keys that leave are never consumption: an opening's, a sale's, or an
//!   unexplained departure;
//! - an opening whose ammo did not settle makes its ammo type's split into
//!   burned and paid back unsettled, as a type the caller must match.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::amount::{checked, count, Amount, Pec, MICRO_PER_PEC};
use super::composite::{recognise, Claim, Opening, Repair, RepairCost, Settlement};
use super::diff::{added_rows, canonical_rows, diff, exact_unit, MoveKind, Span, Step, StepSet};
use super::reading::{Container, Holding, HoldingKey, Reading, Row};
use super::roles::{Role, RoleBook, RoleSource, Stacking, SHRAPNEL, STRONGBOX_KEY, UNIVERSAL_AMMO};
use super::AccountError;

/// How far before its start a session's opening reading may be taken.
pub const OPENING_WITHIN_SECONDS: f64 = 90.0;
/// How far after its start a session's own baseline may be taken and still
/// read the start. The baseline usually lands within a second; a longer
/// allowance would claim coverage the readings do not have.
pub const BASELINE_WITHIN_SECONDS: f64 = 10.0;
/// How far after its end a session's closing reading may be taken.
pub const CLOSING_WITHIN_SECONDS: f64 = 90.0;
/// The longest interval between consecutive readings that is not a gap.
pub const GAP_ABOVE_SECONDS: f64 = 150.0;

/// The Trade Terminal turns Shrapnel into Universal Ammo at 101% of its
/// TT: every 100 units of Shrapnel become 101 units of ammo, rounded down.
pub const SHRAPNEL_TO_AMMO_PERCENT: i64 = 101;

/// The Universal Ammo the Trade Terminal pays for `units` of Shrapnel:
/// 101 for every 100, rounded down.
pub fn ammo_for_shrapnel(units: i64) -> Result<i64, AccountError> {
    checked(
        units
            .checked_mul(SHRAPNEL_TO_AMMO_PERCENT)
            .map(|scaled| scaled / 100),
    )
}

/// The version of the measure's shape and rules. A cached measure of
/// another version is recomputed from its readings. At 5 no reading taken
/// across a neighbouring session's play reads an end or belongs to the
/// measure. A change of the rules bumps it.
// Bumped to 6 when the PED Card stopped deciding whether what left a step
// was used up or what arrived was loot, and a step Universal Ammo was
// topped up in came to be left out of the ammo's split and listed.
pub const MEASURE_VERSION: i64 = 6;

/// A tracked session's span, unix seconds; no end while it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionWindow {
    pub started_at: f64,
    pub ended_at: Option<f64>,
}

/// The sessions either side of one, as far as reading its ends needs
/// them: a reading taken across another session's play reads neither.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Neighbours {
    /// When the latest stop of a session that started before this one
    /// fell: a reading taken earlier does not read this one's start.
    pub previous_ended_at: Option<f64>,
    /// When the first session that started after this one started: no
    /// reading taken then or later is this one's.
    pub next_started_at: Option<f64>,
}

/// Whether one end of a session is bracketed by a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    /// A reading close enough to this end is part of the measure.
    Read,
    /// No reading close enough to this end (or the session still runs).
    Unread,
}

/// The closed set of coverage levels, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverageLevel {
    Complete,
    Partial,
}

impl CoverageLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageLevel::Complete => "complete",
            CoverageLevel::Partial => "partial",
        }
    }
}

/// How well the readings cover the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub level: CoverageLevel,
    pub opening: Edge,
    pub closing: Edge,
    pub reading_count: usize,
    pub gap_count: usize,
}

/// How an ammo type's change splits into burned and paid back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "split",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AmmoSplit {
    /// Every strongbox opening in the window settled: what was burned is
    /// the net change less what openings and Shrapnel paid back, over every
    /// step but those the ammo was topped up in (listed as
    /// [`NotableEvent::AmmoToppedUp`]), whose burn is not known.
    Settled {
        burned_units: i64,
        burned: Amount,
        strongbox_units: i64,
        strongbox: Amount,
        converted_units: i64,
        converted: Amount,
    },
    /// An opening in the window did not settle, or Shrapnel left without
    /// the ammo row its conversion pays, so how much of the net change was
    /// paid back is not known. The units the rows suggest are given only
    /// as that reading of them, with no amount.
    Unsettled {
        burned_units_by_rows: i64,
        strongbox_units_by_rows: i64,
        converted_units: i64,
    },
}

/// The ammo of one type a session used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmmoUse {
    pub name: String,
    /// The change in units across the session's steps, moves aside:
    /// always exact.
    pub net_units: i64,
    pub net: Amount,
    pub split: AmmoSplit,
}

/// One item's wear across the session, never netted against repairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDecay {
    pub key: HoldingKey,
    pub role: Role,
    pub decay: Amount,
}

/// One stackable's units across the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUnits {
    pub key: HoldingKey,
    pub role: Role,
    pub units: i64,
    pub amount: Amount,
}

/// What is known of where something gained came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GainMark {
    /// Arrived, and nothing in its step casts doubt on it.
    Arrived,
    /// Arrived in a strongbox opening's step, and some way of closing the
    /// opening's balance takes it: it may be the opening's contents.
    PossiblyStrongbox,
}

/// What arrived of one holding across the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemGain {
    pub key: HoldingKey,
    pub role: Role,
    /// Units of a stackable, or single items.
    pub stacking: Stacking,
    pub mark: GainMark,
    pub units: i64,
    pub amount: Amount,
}

/// A repair, with when it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedRepair {
    pub between: Span,
    pub repair: Repair,
}

/// One line of a sale or a purchase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventItem {
    pub key: HoldingKey,
    pub units: i64,
    pub amount: Amount,
}

/// Something worth listing that is not a cost or a gain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NotableEvent {
    StrongboxOpening {
        between: Span,
        opening: Opening,
    },
    Moved {
        between: Span,
        name: String,
        units: i64,
        amount: Amount,
        from: Container,
        to: Container,
        move_kind: MoveKind,
    },
    /// A single item arrived that no opening claimed and no purchase or
    /// move explains. It also counts in what was gained.
    Appeared {
        between: Span,
        key: HoldingKey,
        units: i64,
        value: Amount,
    },
    /// A single item went with no matching move or sale. Its value is the
    /// TT it last showed; whether that TT was spent (a limited item worn
    /// out) or the item went elsewhere is not known.
    Left {
        between: Span,
        key: HoldingKey,
        units: i64,
        value: Amount,
    },
    /// Things sold to the Trade Terminal: the one reading of the PED
    /// Card's change in the step takes everything that left as sold.
    Sale {
        between: Span,
        items: Vec<EventItem>,
        amount: Amount,
    },
    /// Things bought at TT: the one reading of the PED Card's change in
    /// the step takes everything that arrived as bought.
    Purchase {
        between: Span,
        items: Vec<EventItem>,
        amount: Amount,
    },
    /// Shrapnel turned into Universal Ammo at the Trade Terminal.
    ShrapnelConverted {
        between: Span,
        shrapnel_units: i64,
        shrapnel: Amount,
        ammo_units: i64,
        ammo: Amount,
    },
    /// A stack left the auction with no matching move: its listing sold.
    ListingSold {
        between: Span,
        key: HoldingKey,
        units: i64,
        amount: Amount,
    },
    /// Universal Ammo arrived with no Shrapnel conversion and no strongbox
    /// opening behind it: topped up. What was fired in this step is not in
    /// the ammo's split.
    AmmoToppedUp {
        between: Span,
        name: String,
    },
}

/// A session's measured account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeasure {
    pub coverage: Coverage,
    pub ammo: Vec<AmmoUse>,
    pub decay: Vec<ItemDecay>,
    /// Of the decay, what each item lost in the steps Universal Ammo was
    /// topped up in: the weapon's share of it is in the tracked figure that
    /// stands in for those steps.
    #[serde(default)]
    pub topped_up_decay: Vec<ItemDecay>,
    pub repairs: Vec<TimedRepair>,
    /// Stackables consumed that are not ammo, not an opening's keys or
    /// boxes, not sold, and not Shrapnel turned into ammo.
    pub other_consumption: Vec<ItemUnits>,
    /// Stackables that left with nothing to explain them: keys with no box
    /// beside them, Shrapnel with no ammo row for it, and stacks the library
    /// or the game data know as never used up by the unit (an enhancer
    /// traded away, say). Never a cost: what became of them is the
    /// reconciliation's to settle.
    pub unexplained_departures: Vec<ItemUnits>,
    /// What arrived that no opening claimed, no purchase paid for, and no
    /// move brought: the figure tracked loot compares with.
    pub gained: Vec<ItemGain>,
    pub gained_total: Amount,
    /// Of the gained total, what may be strongbox contents.
    pub gained_possibly_strongbox: Amount,
    pub events: Vec<NotableEvent>,
}

/// A measure and the readings it was taken over.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureOutcome {
    pub measure: SessionMeasure,
    /// The positions, in the readings given, of the readings used.
    pub used: Range<usize>,
}

/// Which of the readings, in the order they were stored, a session's
/// measure uses, chosen by its window.
fn select(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    baseline: Option<usize>,
) -> (Range<usize>, Edge, Edge) {
    let n = readings.len();
    let at = |index: usize| readings[index].taken_at();
    let inside = |reading: &Reading| {
        reading.taken_at() > window.started_at
            && window
                .ended_at
                .is_none_or(|ended| reading.taken_at() < ended)
    };
    let (before, after, core) = match readings.iter().position(inside) {
        Some(first) => {
            let last = readings.iter().rposition(inside).unwrap_or(first);
            (first.checked_sub(1), Some(last + 1), Some(first..last + 1))
        }
        None => {
            let opening = readings
                .iter()
                .rposition(|reading| reading.taken_at() <= window.started_at);
            (opening, Some(opening.map_or(0, |index| index + 1)), None)
        }
    };
    let opening = before.filter(|&index| reads_the_start(window, neighbours, at(index)));
    // Without a reading close before the start, the session's own
    // baseline reads it, when it is the first reading inside the session
    // and was taken the moment the session started.
    let baseline = baseline.filter(|&index| {
        core.as_ref().is_some_and(|core| core.start == index)
            && at(index) - window.started_at <= BASELINE_WITHIN_SECONDS
    });
    let closing = window.ended_at.and_then(|ended| {
        after.filter(|&index| index < n && reads_the_end(ended, neighbours, at(index)))
    });
    let first = opening
        .or(core.as_ref().map(|core| core.start))
        .or(closing)
        .unwrap_or(0);
    let last = closing
        .map(|index| index + 1)
        .or(core.as_ref().map(|core| core.end))
        .or(opening.map(|index| index + 1))
        .unwrap_or(first);
    (
        first..last.max(first),
        edge(opening.or(baseline)),
        edge(closing),
    )
}

fn edge(found: Option<usize>) -> Edge {
    if found.is_some() {
        Edge::Read
    } else {
        Edge::Unread
    }
}

/// Whether a reading taken at `at` reads the session's start: close
/// enough before it, and not before the session before it stopped.
fn reads_the_start(window: &SessionWindow, neighbours: &Neighbours, at: f64) -> bool {
    at <= window.started_at
        && window.started_at - at <= OPENING_WITHIN_SECONDS
        && neighbours
            .previous_ended_at
            .is_none_or(|stopped| at >= stopped)
}

/// Whether a reading taken at `at` reads the session's end: close enough
/// after it, and before the session after it started.
fn reads_the_end(ended: f64, neighbours: &Neighbours, at: f64) -> bool {
    at >= ended && at - ended <= CLOSING_WITHIN_SECONDS && before_the_next(neighbours, at)
}

/// Whether a reading taken at `at` was taken before the next session
/// started, so it can be this one's.
fn before_the_next(neighbours: &Neighbours, at: f64) -> bool {
    neighbours.next_started_at.is_none_or(|next| at < next)
}

/// Which readings a session's measure uses when its own readings are
/// known: `own` is their range among `readings`, which hold besides them
/// at most the reading stored just before them and the one stored just
/// after them. Those two are used only to read an end the session's own
/// readings leave unread. The session's own readings run through its
/// closing ones, and stop before any taken once the next session started.
fn select_own(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    own: Range<usize>,
    baseline: Option<usize>,
) -> (Range<usize>, Edge, Edge) {
    let n = readings.len();
    let at = |index: usize| readings[index].taken_at();
    if own.is_empty() {
        return (own, Edge::Unread, Edge::Unread);
    }
    let by_own = reads_the_start(window, neighbours, at(own.start)).then_some(own.start);
    let by_before = own
        .start
        .checked_sub(1)
        .filter(|&index| by_own.is_none() && reads_the_start(window, neighbours, at(index)));
    // The baseline reads the start only when it was taken inside the
    // session: one that came back after the session stopped saw its end.
    let by_baseline = baseline.filter(|&index| {
        index == own.start
            && at(index) - window.started_at <= BASELINE_WITHIN_SECONDS
            && window.ended_at.is_none_or(|ended| at(index) < ended)
    });
    // A reading of its own taken once the next session started is that
    // session's play, never this one's: the measure stops before it.
    let own_end = own
        .clone()
        .find(|&index| !before_the_next(neighbours, at(index)))
        .unwrap_or(own.end);
    let (closing, last) = match window.ended_at {
        None => (None, own_end),
        Some(ended) => match (own.start..own_end).find(|&index| at(index) >= ended) {
            Some(index) => (
                reads_the_end(ended, neighbours, at(index)).then_some(index),
                own_end,
            ),
            None if own_end == own.end
                && own.end < n
                && reads_the_end(ended, neighbours, at(own.end)) =>
            {
                (Some(own.end), own.end + 1)
            }
            None => (None, own_end),
        },
    };
    let first = by_before.unwrap_or(own.start);
    (
        first..last.max(first),
        edge(by_own.or(by_before).or(by_baseline)),
        edge(closing),
    )
}

/// Measure a session over its readings, in the order they were stored,
/// chosen by its window, none of them its baseline.
pub fn measure_session(
    window: &SessionWindow,
    readings: &[Reading],
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    measure_session_with_baseline(window, &Neighbours::default(), readings, None, roles)
}

/// Measure a session over its readings, in the order they were stored,
/// chosen by its window, between its `neighbours`; `baseline` is the
/// position of the session's own baseline among them, when it has one.
pub fn measure_session_with_baseline(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    baseline: Option<usize>,
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let (used, opening, closing) = select(window, neighbours, readings, baseline);
    measure_selected(readings, used, opening, closing, roles)
}

/// Measure a session over its own readings, between its `neighbours`:
/// `own` is their range among `readings`, which also hold the reading
/// stored just before them and the one stored just after them, when there
/// are such readings; `baseline` is the position of the session's own
/// baseline, when it has one.
pub fn measure_own_readings(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    own: Range<usize>,
    baseline: Option<usize>,
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let (used, opening, closing) = select_own(window, neighbours, readings, own, baseline);
    measure_selected(readings, used, opening, closing, roles)
}

/// Which of the readings a session's measure uses, and whether they read
/// its ends: by its own readings when `own` names them (see
/// [`measure_own_readings`]), by its window otherwise (see
/// [`measure_session_with_baseline`]).
pub fn select_readings(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    own: Option<Range<usize>>,
    baseline: Option<usize>,
) -> (Range<usize>, Edge, Edge) {
    match own {
        Some(own) => select_own(window, neighbours, readings, own, baseline),
        None => select(window, neighbours, readings, baseline),
    }
}

/// Measure a session over the readings already chosen for it (see
/// [`select_readings`]), which read its ends as `opening` and `closing`.
pub fn measure_used(
    readings: &[Reading],
    opening: Edge,
    closing: Edge,
    roles: &dyn RoleSource,
) -> Result<SessionMeasure, AccountError> {
    Ok(measure_selected(readings, 0..readings.len(), opening, closing, roles)?.measure)
}

/// Each step between consecutive `readings` measured alone, in order, by
/// the roles the whole run of readings resolves (so an item is read the
/// same way in every step). A step longer than [`GAP_ABOVE_SECONDS`], or
/// with time running backwards, is measured as one step and its coverage
/// counts it as a gap.
pub fn measure_steps(
    readings: &[Reading],
    roles: &dyn RoleSource,
) -> Result<Vec<SessionMeasure>, AccountError> {
    let book = RoleBook::resolve(roles, readings);
    let mut steps = Vec::with_capacity(readings.len().saturating_sub(1));
    for pair in readings.windows(2) {
        let interval = pair[1].taken_at() - pair[0].taken_at();
        let gap_count = usize::from(!(0.0..=GAP_ABOVE_SECONDS).contains(&interval));
        let mut tally = Tally::default();
        tally.add_step(&pair[0], &pair[1], &book)?;
        steps.push(tally.finish(Coverage {
            level: if gap_count == 0 {
                CoverageLevel::Complete
            } else {
                CoverageLevel::Partial
            },
            opening: Edge::Read,
            closing: Edge::Read,
            reading_count: 2,
            gap_count,
        })?);
    }
    Ok(steps)
}

/// One end of a session read across a gap: by the nearest reading on its
/// far side, however far outside the session that was taken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GapEnd {
    /// The reading's position among the readings given.
    pub index: usize,
    /// How far outside the session it was taken, in seconds.
    pub outside_seconds: f64,
    /// How long the readings were missing across this end: the interval
    /// of the step between it and the session's nearest reading.
    pub gap_seconds: f64,
}

/// A session measured across a gap at one end or both. A distinct kind of
/// measure, never a complete one: what changed in a bracketing step is the
/// session's only when the time the step spends outside the session is
/// shown to hold no other play, which is for the caller to show.
#[derive(Debug, Clone, PartialEq)]
pub struct AcrossGap {
    /// The measure over the bracketed readings.
    pub measure: SessionMeasure,
    /// The positions of the readings it uses.
    pub used: Range<usize>,
    /// The ends read across a gap; at least one is.
    pub opening: Option<GapEnd>,
    pub closing: Option<GapEnd>,
    /// Each bracketing step measured alone, the opening's first.
    pub steps: Vec<SessionMeasure>,
}

/// Whether a sealed session can be measured across a gap.
#[derive(Debug, Clone, PartialEq)]
pub enum GapMeasure {
    /// Its measure reads both ends, or an end it leaves unread has no
    /// reading on its far side that is the session's to use (none before
    /// it, or one taken across another session's play), or the readings
    /// stop for a while inside the session as well.
    NotOffered,
    /// Its end is unread and no reading has been stored after it yet: the
    /// reading the gap ends at may still come.
    Awaiting,
    Offered(Box<AcrossGap>),
}

/// Measure a session that has ended across the gap at an end its own
/// readings leave unread (see [`measure_own_readings`] for `own` and
/// `baseline`). An unread opening is read by the last reading taken at or
/// before the start, provided the session before had stopped by then; an
/// unread closing by the first reading taken at or after the end, provided
/// it was taken before the next session started, or is that session's own
/// baseline (`next_baseline`, its position among `readings`: the reading
/// the next session took as it started, before any play of its own); either
/// however far outside the session. Only the steps those readings bound
/// may be gaps.
pub fn measure_across_gap(
    window: &SessionWindow,
    neighbours: &Neighbours,
    readings: &[Reading],
    own: Range<usize>,
    baseline: Option<usize>,
    next_baseline: Option<usize>,
    roles: &dyn RoleSource,
) -> Result<GapMeasure, AccountError> {
    let Some(ended) = window.ended_at else {
        return Ok(GapMeasure::NotOffered);
    };
    if own.is_empty() {
        return Ok(GapMeasure::NotOffered);
    }
    let n = readings.len();
    let at = |index: usize| readings[index].taken_at();
    let (read, opening_edge, closing_edge) =
        select_own(window, neighbours, readings, own.clone(), baseline);
    if opening_edge == Edge::Read && closing_edge == Edge::Read {
        return Ok(GapMeasure::NotOffered);
    }
    let own_end = own
        .clone()
        .find(|&index| !before_the_next(neighbours, at(index)))
        .unwrap_or(own.end);

    let (first, opening) = if opening_edge == Edge::Read {
        (read.start, None)
    } else {
        let earliest = own.start.saturating_sub(1);
        let Some(index) = (earliest..own_end)
            .rev()
            .find(|&index| at(index) <= window.started_at)
        else {
            return Ok(GapMeasure::NotOffered);
        };
        let taken = at(index);
        let after_the_last = neighbours
            .previous_ended_at
            .is_none_or(|stopped| taken >= stopped);
        if !after_the_last || index + 1 >= n {
            return Ok(GapMeasure::NotOffered);
        }
        (
            index,
            Some(GapEnd {
                index,
                outside_seconds: window.started_at - taken,
                gap_seconds: at(index + 1) - taken,
            }),
        )
    };

    let (last, closing) = if closing_edge == Edge::Read {
        (read.end, None)
    } else {
        let index = match (own.start..own_end).find(|&index| at(index) >= ended) {
            Some(index) => index,
            None if own_end == own.end && own.end < n => own.end,
            None if own_end == own.end => return Ok(GapMeasure::Awaiting),
            None => return Ok(GapMeasure::NotOffered),
        };
        let taken = at(index);
        let this_one_s = before_the_next(neighbours, taken) || next_baseline == Some(index);
        if taken < ended || !this_one_s || index <= first {
            return Ok(GapMeasure::NotOffered);
        }
        (
            index + 1,
            Some(GapEnd {
                index,
                outside_seconds: taken - ended,
                gap_seconds: taken - at(index - 1),
            }),
        )
    };
    if last < first + 2 {
        return Ok(GapMeasure::NotOffered);
    }
    // Time running backwards, or a gap anywhere but a bracketing step.
    for step in first..last - 1 {
        let interval = at(step + 1) - at(step);
        let bracketing = opening.is_some_and(|end| end.index == step)
            || closing.is_some_and(|end| end.index == step + 1);
        if interval < 0.0 || (interval > GAP_ABOVE_SECONDS && !bracketing) {
            return Ok(GapMeasure::NotOffered);
        }
    }

    let measure = |range: Range<usize>| {
        measure_selected(readings, range, Edge::Read, Edge::Read, roles)
            .map(|outcome| outcome.measure)
    };
    let mut steps = Vec::with_capacity(2);
    if let Some(end) = opening {
        steps.push(measure(end.index..end.index + 2)?);
    }
    if let Some(end) = closing {
        steps.push(measure(end.index - 1..end.index + 1)?);
    }
    Ok(GapMeasure::Offered(Box::new(AcrossGap {
        measure: measure(first..last)?,
        used: first..last,
        opening,
        closing,
        steps,
    })))
}

fn measure_selected(
    readings: &[Reading],
    used: Range<usize>,
    opening: Edge,
    closing: Edge,
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let span = &readings[used.clone()];
    let book = RoleBook::resolve(roles, span);
    // Time running backwards between two readings hides an interval of
    // unknown length: a gap too.
    let gap_count = span
        .windows(2)
        .filter(|pair| {
            let interval = pair[1].taken_at() - pair[0].taken_at();
            !(0.0..=GAP_ABOVE_SECONDS).contains(&interval)
        })
        .count();
    let level =
        if opening == Edge::Read && closing == Edge::Read && gap_count == 0 && span.len() >= 2 {
            CoverageLevel::Complete
        } else {
            CoverageLevel::Partial
        };
    let mut tally = Tally::default();
    for pair in span.windows(2) {
        tally.add_step(&pair[0], &pair[1], &book)?;
    }
    Ok(MeasureOutcome {
        measure: tally.finish(Coverage {
            level,
            opening,
            closing,
            reading_count: span.len(),
            gap_count,
        })?,
        used,
    })
}

struct AmmoTally {
    net_units: i64,
    net: Amount,
    burned_units: i64,
    burned: Amount,
    strongbox_units: i64,
    strongbox: Amount,
    converted_units: i64,
    converted: Amount,
    unsettled: bool,
}

impl Default for AmmoTally {
    fn default() -> AmmoTally {
        AmmoTally {
            net_units: 0,
            net: Amount::NONE,
            burned_units: 0,
            burned: Amount::NONE,
            strongbox_units: 0,
            strongbox: Amount::NONE,
            converted_units: 0,
            converted: Amount::NONE,
            unsettled: false,
        }
    }
}

/// Why something left, before it is read as sold, consumed or unexplained.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaving {
    /// A stack consumed.
    Stack,
    /// Keys with no box leaving beside them: never consumption.
    Keys,
    /// Shrapnel with no ammo row for its conversion.
    Shrapnel,
    /// A stack known never to be used up by the unit.
    Material,
    /// A single item gone.
    Item,
}

/// Something that left in a step.
struct Departure {
    key: HoldingKey,
    units: i64,
    amount: Amount,
    rows: usize,
    leaving: Leaving,
}

/// Something that arrived in a step, before it is read as bought or
/// gained.
struct Arrival {
    key: HoldingKey,
    units: i64,
    amount: Amount,
    rows: usize,
    stacking: Stacking,
    mark: GainMark,
    /// A single item, listed as having appeared if it stays gained.
    appeared: bool,
    /// Universal Ammo topped up: read with the Card as a purchase may be,
    /// and never gained.
    topped_up: bool,
}

/// One ammo type's movement within one step.
struct AmmoStep {
    key: HoldingKey,
    units: i64,
    amount: Amount,
    /// The rows new in the step, as the diff reads them.
    new_units: i64,
    new_amount: Amount,
    new_rows: usize,
    /// A row vanished that a new row is larger than: it may have grown
    /// into it by stacking, so the new rows may not all have arrived.
    grew: bool,
}

/// A Shrapnel departure read as a conversion by its ammo row.
pub(crate) struct Conversion {
    /// The Shrapnel's departure, by its position in the step.
    pub(crate) index: usize,
    pub(crate) shrapnel_units: i64,
    pub(crate) shrapnel: Amount,
    pub(crate) ammo_units: i64,
    pub(crate) ammo: Amount,
}

/// How the PED Card's change in a step is read.
enum CardReading {
    /// Exactly one combination of sale, purchase and repair explains it.
    Explained {
        sale: bool,
        purchase: bool,
        cost: RepairCost,
    },
    /// No use claims it: the Card did not move and nothing was restored,
    /// or no one combination explains what it did. Ignored.
    Unclaimed,
}

#[derive(Default)]
struct Tally {
    ammo: BTreeMap<String, AmmoTally>,
    decay: BTreeMap<HoldingKey, (Role, Amount)>,
    topped_up_decay: BTreeMap<HoldingKey, (Role, Amount)>,
    repairs: Vec<TimedRepair>,
    consumed: BTreeMap<HoldingKey, (Role, i64, Amount)>,
    unexplained: BTreeMap<HoldingKey, (Role, i64, Amount)>,
    gained: BTreeMap<(HoldingKey, GainMark), (Role, Stacking, i64, Amount)>,
    events: Vec<NotableEvent>,
}

fn add_units(
    into: &mut BTreeMap<HoldingKey, (Role, i64, Amount)>,
    key: HoldingKey,
    role: Role,
    units: i64,
    amount: Amount,
) -> Result<(), AccountError> {
    let entry = into.entry(key).or_insert((role, 0, Amount::NONE));
    entry.1 = checked(entry.1.checked_add(units))?;
    entry.2 = entry.2.plus(amount)?;
    Ok(())
}

fn rows_of(holding: Option<&Holding>) -> &[Row] {
    holding.map(Holding::rows).unwrap_or_default()
}

/// Read the PED Card's change `delta` in a step jointly: everything that
/// left may be a sale, everything that arrived a purchase, and a repair
/// costs within what its restorations allow. Each combination of the
/// candidates present is tried; exactly one that fits within one PEC per
/// row it involves is what happened.
fn read_the_card(
    delta: Amount,
    departures: &[Departure],
    arrivals: &[Arrival],
    repair: Option<&Repair>,
    listing_moved: bool,
) -> Result<CardReading, AccountError> {
    if delta.micro_ped() == 0 && repair.is_none() {
        return Ok(CardReading::Unclaimed);
    }
    // A listing made or returned moves the Card by a fee nothing here
    // reads: no combination can explain the step.
    if listing_moved {
        return Ok(CardReading::Unclaimed);
    }
    let sold = Amount::total(departures.iter().map(|d| d.amount))?;
    let sold_rows: usize = departures.iter().map(|d| d.rows).sum();
    let bought = Amount::total(arrivals.iter().map(|a| a.amount))?;
    let bought_rows: usize = arrivals.iter().map(|a| a.rows).sum();
    let (low, high) = match repair {
        Some(repair) => (repair.least_cost()?, repair.restored),
        None => (Amount::NONE, Amount::NONE),
    };
    let mut fits = Vec::new();
    for sale in [false, true] {
        if sale && departures.is_empty() {
            continue;
        }
        for purchase in [false, true] {
            if purchase && arrivals.is_empty() {
                continue;
            }
            // The Card's change is sales less purchases less the repair's
            // cost, so the cost is what the rest leaves.
            let mut cost = delta.negated()?;
            let mut rows = 0;
            if sale {
                cost = cost.plus(sold)?;
                rows += sold_rows;
            }
            if purchase {
                cost = cost.minus(bought)?;
                rows += bought_rows;
            }
            let slack = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
            let at_least = checked(low.micro_ped().checked_sub(slack))?;
            let at_most = checked(high.micro_ped().checked_add(slack))?;
            if (at_least..=at_most).contains(&cost.micro_ped()) {
                fits.push((sale, purchase, cost));
            }
        }
    }
    let [(sale, purchase, cost)] = fits.as_slice() else {
        return Ok(CardReading::Unclaimed);
    };
    let cost = match repair {
        None => RepairCost::Unknown,
        Some(_) if !sale && !purchase => RepairCost::Paid {
            pec: Pec(cost.micro_ped() / MICRO_PER_PEC),
        },
        Some(_) => RepairCost::Derived { amount: *cost },
    };
    Ok(CardReading::Explained {
        sale: *sale,
        purchase: *purchase,
        cost,
    })
}

impl Tally {
    fn add_step(
        &mut self,
        before: &Reading,
        after: &Reading,
        book: &RoleBook,
    ) -> Result<(), AccountError> {
        let set = diff(before, after, &|name| book.stacking(name))?;
        let between = set.span;

        let (reserved, conversions) = conversions_of(&set, before, after)?;
        let composites = recognise(&set, before, after, &reserved)?;
        let opening = composites.opening.as_ref();
        let opening_input = |key: &HoldingKey| {
            opening.is_some_and(|opening| {
                key.name == STRONGBOX_KEY || opening.boxes.iter().any(|b| b.name == key.name)
            })
        };
        let mut card: Option<(Pec, Pec)> = None;
        let mut listing_moved = false;
        let mut ammo_steps: BTreeMap<String, AmmoStep> = BTreeMap::new();
        let mut departures: Vec<Departure> = Vec::new();
        let mut arrivals: Vec<Arrival> = Vec::new();
        let mut step_decay: Vec<(HoldingKey, Amount)> = Vec::new();

        for (index, step) in set.steps.iter().enumerate() {
            match step {
                Step::Decayed { key, by } => {
                    let entry = self
                        .decay
                        .entry(key.clone())
                        .or_insert((book.role(&key.name), Amount::NONE));
                    entry.1 = entry.1.plus(*by)?;
                    step_decay.push((key.clone(), *by));
                }
                Step::Repaired { .. } => {}
                Step::Moved {
                    name,
                    units,
                    amount,
                    from,
                    to,
                } => {
                    let move_kind = MoveKind::of(from, to);
                    listing_moved |= move_kind != MoveKind::Transfer;
                    self.events.push(NotableEvent::Moved {
                        between,
                        name: name.clone(),
                        units: *units,
                        amount: *amount,
                        move_kind,
                        from: from.clone(),
                        to: to.clone(),
                    });
                }
                Step::Consumed {
                    key, units, amount, ..
                } if key.container == Container::Auction => {
                    self.events.push(NotableEvent::ListingSold {
                        between,
                        key: key.clone(),
                        units: *units,
                        amount: *amount,
                    });
                }
                Step::Consumed {
                    key,
                    units,
                    amount,
                    new_rows,
                    ..
                }
                | Step::Gained {
                    key,
                    units,
                    amount,
                    new_rows,
                    ..
                } if book.role(&key.name) == Role::Ammo => {
                    let consumed = matches!(step, Step::Consumed { .. });
                    let (new, vanished) =
                        canonical_rows(rows_of(before.holding(key)), rows_of(after.holding(key)));
                    let grew = vanished
                        .iter()
                        .any(|gone| new.iter().any(|row| row.quantity > gone.quantity));
                    let entry = ammo_steps.entry(key.name.clone()).or_insert(AmmoStep {
                        key: key.clone(),
                        units: 0,
                        amount: Amount::NONE,
                        new_units: 0,
                        new_amount: Amount::NONE,
                        new_rows: 0,
                        grew: false,
                    });
                    entry.grew |= grew;
                    if consumed {
                        entry.units = checked(entry.units.checked_sub(*units))?;
                        entry.amount = entry.amount.minus(*amount)?;
                    } else {
                        entry.units = checked(entry.units.checked_add(*units))?;
                        entry.amount = entry.amount.plus(*amount)?;
                    }
                    entry.new_units = checked(entry.new_units.checked_add(new_rows.units))?;
                    entry.new_amount = entry.new_amount.plus(new_rows.amount)?;
                    entry.new_rows += new_rows.rows.len();
                }
                Step::Consumed { .. } if conversions.iter().any(|c| c.index == index) => {}
                Step::Consumed { key, .. } if opening_input(key) => {}
                Step::Consumed {
                    key,
                    units,
                    amount,
                    rows_touched,
                    ..
                } => departures.push(Departure {
                    key: key.clone(),
                    units: *units,
                    amount: *amount,
                    rows: *rows_touched,
                    leaving: if key.name == STRONGBOX_KEY {
                        Leaving::Keys
                    } else if key.name == SHRAPNEL {
                        Leaving::Shrapnel
                    } else if book.is_known_material(&key.name) {
                        Leaving::Material
                    } else {
                        Leaving::Stack
                    },
                }),
                Step::Left { key, units, value } => departures.push(Departure {
                    key: key.clone(),
                    units: *units,
                    amount: *value,
                    rows: 1,
                    leaving: Leaving::Item,
                }),
                Step::Gained {
                    key,
                    units,
                    amount,
                    rows_touched,
                    ..
                } => {
                    if let Some(mark) = mark_of(composites.claims.get(&index)) {
                        arrivals.push(Arrival {
                            key: key.clone(),
                            units: *units,
                            amount: *amount,
                            rows: *rows_touched,
                            stacking: Stacking::Stack,
                            mark,
                            appeared: false,
                            topped_up: false,
                        });
                    }
                }
                Step::Appeared { key, units, value } => {
                    if let Some(mark) = mark_of(composites.claims.get(&index)) {
                        arrivals.push(Arrival {
                            key: key.clone(),
                            units: *units,
                            amount: *value,
                            rows: 1,
                            stacking: Stacking::Single,
                            mark,
                            appeared: true,
                            topped_up: false,
                        });
                    }
                }
                Step::BalanceChanged { from, to } => card = Some((*from, *to)),
            }
        }

        // Ammo: what openings and conversions paid back is credited; new
        // rows beyond that arrived from elsewhere (a purchase, read with
        // the Card, a top-up, or a gain).
        let converted_units = conversions.iter().try_fold(0i64, |total, conversion| {
            checked(total.checked_add(conversion.ammo_units))
        })?;
        let converted = Amount::total(conversions.iter().map(|conversion| conversion.ammo))?;
        if (converted_units != 0 || opening.is_some()) && !ammo_steps.contains_key(UNIVERSAL_AMMO) {
            ammo_steps.insert(
                UNIVERSAL_AMMO.to_string(),
                AmmoStep {
                    key: HoldingKey {
                        name: UNIVERSAL_AMMO.to_string(),
                        container: Container::Carried,
                    },
                    units: 0,
                    amount: Amount::NONE,
                    new_units: 0,
                    new_amount: Amount::NONE,
                    new_rows: 0,
                    grew: false,
                },
            );
        }
        let unsettled_opening =
            opening.is_some_and(|opening| opening.settlement == Settlement::Unsettled);
        // Universal Ammo arriving with nothing to pay it back (no Shrapnel
        // leaving, converted or not, and no opening) was topped up: bought,
        // or handed over. What was fired beside it is not known.
        let shrapnel_left = !conversions.is_empty()
            || departures
                .iter()
                .any(|departure| departure.leaving == Leaving::Shrapnel);
        let topped_up = opening.is_none()
            && !shrapnel_left
            && ammo_steps
                .get(UNIVERSAL_AMMO)
                .is_some_and(|step| step.new_units > 0);
        if topped_up {
            for (key, by) in step_decay {
                let entry = self
                    .topped_up_decay
                    .entry(key)
                    .or_insert_with_key(|key| (book.role(&key.name), Amount::NONE));
                entry.1 = entry.1.plus(by)?;
            }
        }
        let mut credits: BTreeMap<String, (i64, Amount)> = BTreeMap::new();
        let mut uncertain_growth: Vec<String> = Vec::new();
        for (name, step) in &ammo_steps {
            let topped_up = topped_up && name == UNIVERSAL_AMMO;
            let (mut credited_units, mut credited) = (0i64, Amount::NONE);
            if name == UNIVERSAL_AMMO {
                if let Some(opening) = opening {
                    credited_units = opening.ammo_units;
                    credited = opening.ammo_from_rows;
                }
                credited_units = checked(credited_units.checked_add(converted_units))?;
                credited = credited.plus(converted)?;
            }
            let beyond = checked(step.new_units.checked_sub(credited_units))?;
            if beyond > 0 && step.grew && !topped_up {
                uncertain_growth.push(name.clone());
            }
            if beyond > 0 {
                arrivals.push(Arrival {
                    key: step.key.clone(),
                    units: beyond,
                    amount: step.new_amount.minus(credited)?,
                    rows: step.new_rows.max(1),
                    stacking: Stacking::Stack,
                    mark: if name == UNIVERSAL_AMMO && unsettled_opening {
                        GainMark::PossiblyStrongbox
                    } else {
                        GainMark::Arrived
                    },
                    appeared: false,
                    topped_up,
                });
                credited_units = checked(credited_units.checked_add(beyond))?;
                credited = credited.plus(step.new_amount.minus(credited)?)?;
            }
            credits.insert(name.clone(), (credited_units, credited));
        }

        // The PED Card, read jointly with what left and what arrived.
        let delta = match card {
            Some((from, to)) => Amount::whole_pec(to.minus(from)?)?,
            None => Amount::NONE,
        };
        let reading = read_the_card(
            delta,
            &departures,
            &arrivals,
            composites.repair.as_ref(),
            listing_moved,
        )?;
        let (sold, bought, cost) = match reading {
            CardReading::Explained {
                sale,
                purchase,
                cost,
            } => (sale, purchase, cost),
            CardReading::Unclaimed => (false, false, RepairCost::Unknown),
        };

        let mut shrapnel_left_unread = false;
        if sold {
            let amount = Amount::total(departures.iter().map(|d| d.amount))?;
            self.events.push(NotableEvent::Sale {
                between,
                items: departures
                    .drain(..)
                    .map(|d| EventItem {
                        key: d.key,
                        units: d.units,
                        amount: d.amount,
                    })
                    .collect(),
                amount,
            });
        }
        for departure in departures {
            let role = book.role(&departure.key.name);
            match departure.leaving {
                Leaving::Item => self.events.push(NotableEvent::Left {
                    between,
                    key: departure.key,
                    units: departure.units,
                    value: departure.amount,
                }),
                Leaving::Stack => add_units(
                    &mut self.consumed,
                    departure.key,
                    role,
                    departure.units,
                    departure.amount,
                )?,
                leaving => {
                    shrapnel_left_unread |= leaving == Leaving::Shrapnel;
                    add_units(
                        &mut self.unexplained,
                        departure.key,
                        role,
                        departure.units,
                        departure.amount,
                    )?;
                }
            }
        }
        if bought {
            let amount = Amount::total(arrivals.iter().map(|a| a.amount))?;
            self.events.push(NotableEvent::Purchase {
                between,
                items: arrivals
                    .drain(..)
                    .map(|a| EventItem {
                        key: a.key,
                        units: a.units,
                        amount: a.amount,
                    })
                    .collect(),
                amount,
            });
        }

        // The ammo tally: burned is what came in less the net change.
        for (name, step) in ammo_steps {
            let (credited_units, credited) =
                credits.get(&name).copied().unwrap_or((0, Amount::NONE));
            let tally = self.ammo.entry(name.clone()).or_default();
            tally.net_units = checked(tally.net_units.checked_add(step.units))?;
            tally.net = tally.net.plus(step.amount)?;
            if topped_up && name == UNIVERSAL_AMMO {
                // Nothing paid it back, so nothing is credited; the burn is
                // the tracked figure's to stand in for.
                self.events
                    .push(NotableEvent::AmmoToppedUp { between, name });
                continue;
            }
            if name == UNIVERSAL_AMMO {
                if let Some(opening) = opening {
                    tally.strongbox_units =
                        checked(tally.strongbox_units.checked_add(opening.ammo_units))?;
                    tally.strongbox = tally.strongbox.plus(opening.ammo_from_rows)?;
                }
                tally.unsettled |= unsettled_opening || shrapnel_left_unread;
                tally.converted_units =
                    checked(tally.converted_units.checked_add(converted_units))?;
                tally.converted = tally.converted.plus(converted)?;
            }
            if uncertain_growth.contains(&name) {
                tally.unsettled = true;
            }
            let burned_units = checked(credited_units.checked_sub(step.units))?;
            let burned = credited.minus(step.amount)?;
            if burned_units > 0 {
                tally.burned_units = checked(tally.burned_units.checked_add(burned_units))?;
                tally.burned = tally.burned.plus(burned)?;
            } else if burned_units < 0 {
                // More arrived than the new rows show: stacked onto a row.
                arrivals.push(Arrival {
                    key: step.key,
                    units: checked(burned_units.checked_neg())?,
                    amount: burned.negated()?,
                    rows: 1,
                    stacking: Stacking::Stack,
                    mark: GainMark::Arrived,
                    appeared: false,
                    topped_up: false,
                });
            }
        }

        for conversion in conversions {
            self.events.push(NotableEvent::ShrapnelConverted {
                between,
                shrapnel_units: conversion.shrapnel_units,
                shrapnel: conversion.shrapnel,
                ammo_units: conversion.ammo_units,
                ammo: conversion.ammo,
            });
        }

        // What is left of the arrivals was gained; ammo topped up never is.
        for arrival in arrivals.into_iter().filter(|arrival| !arrival.topped_up) {
            if arrival.appeared {
                self.events.push(NotableEvent::Appeared {
                    between,
                    key: arrival.key.clone(),
                    units: arrival.units,
                    value: arrival.amount,
                });
            }
            let role = book.role(&arrival.key.name);
            let entry = self.gained.entry((arrival.key, arrival.mark)).or_insert((
                role,
                arrival.stacking,
                0,
                Amount::NONE,
            ));
            entry.2 = checked(entry.2.checked_add(arrival.units))?;
            entry.3 = entry.3.plus(arrival.amount)?;
        }

        if let Some(opening) = composites.opening {
            self.events
                .push(NotableEvent::StrongboxOpening { between, opening });
        }
        if let Some(mut repair) = composites.repair {
            repair.cost = cost;
            self.repairs.push(TimedRepair { between, repair });
        }
        Ok(())
    }

    fn finish(self, coverage: Coverage) -> Result<SessionMeasure, AccountError> {
        let gained: Vec<ItemGain> = self
            .gained
            .into_iter()
            .map(|((key, mark), (role, stacking, units, amount))| ItemGain {
                key,
                role,
                stacking,
                mark,
                units,
                amount,
            })
            .collect();
        let gained_total = Amount::total(gained.iter().map(|gain| gain.amount))?;
        let gained_possibly_strongbox = Amount::total(
            gained
                .iter()
                .filter(|gain| gain.mark == GainMark::PossiblyStrongbox)
                .map(|gain| gain.amount),
        )?;
        let units = |map: BTreeMap<HoldingKey, (Role, i64, Amount)>| -> Vec<ItemUnits> {
            map.into_iter()
                .map(|(key, (role, units, amount))| ItemUnits {
                    key,
                    role,
                    units,
                    amount,
                })
                .collect()
        };
        Ok(SessionMeasure {
            coverage,
            ammo: self
                .ammo
                .into_iter()
                .map(|(name, tally)| AmmoUse {
                    name,
                    net_units: tally.net_units,
                    net: tally.net,
                    split: if tally.unsettled {
                        AmmoSplit::Unsettled {
                            burned_units_by_rows: tally.burned_units,
                            strongbox_units_by_rows: tally.strongbox_units,
                            converted_units: tally.converted_units,
                        }
                    } else {
                        AmmoSplit::Settled {
                            burned_units: tally.burned_units,
                            burned: tally.burned,
                            strongbox_units: tally.strongbox_units,
                            strongbox: tally.strongbox,
                            converted_units: tally.converted_units,
                            converted: tally.converted,
                        }
                    },
                })
                .collect(),
            decay: self
                .decay
                .into_iter()
                .map(|(key, (role, decay))| ItemDecay { key, role, decay })
                .collect(),
            topped_up_decay: self
                .topped_up_decay
                .into_iter()
                .map(|(key, (role, decay))| ItemDecay { key, role, decay })
                .collect(),
            repairs: self.repairs,
            other_consumption: units(self.consumed),
            unexplained_departures: units(self.unexplained),
            gained,
            gained_total,
            gained_possibly_strongbox,
            events: self.events,
        })
    }
}

/// Ammo rows another pattern claims, by the holding they sit in.
pub(crate) type ClaimedRows = Vec<(HoldingKey, Row)>;

/// Shrapnel conversions in one step, read from the ammo row each pays: a
/// new Universal Ammo row of exactly what the Trade Terminal gives. With
/// them, the ammo rows they claim, which an opening in the same step never
/// reads as its payout.
pub(crate) fn conversions_of(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<(ClaimedRows, Vec<Conversion>), AccountError> {
    let mut ammo_arrived: Vec<(HoldingKey, Vec<Row>)> = Vec::new();
    for step in &set.steps {
        if let Step::Consumed { key, .. } | Step::Gained { key, .. } = step {
            if key.name == UNIVERSAL_AMMO {
                ammo_arrived.push((
                    key.clone(),
                    added_rows(rows_of(before.holding(key)), rows_of(after.holding(key))),
                ));
            }
        }
    }
    let ammo_unit = exact_unit(
        before
            .holdings()
            .iter()
            .chain(after.holdings())
            .filter(|(key, _)| key.name == UNIVERSAL_AMMO)
            .flat_map(|(_, holding)| holding.rows()),
    );
    let mut reserved: Vec<(HoldingKey, Row)> = Vec::new();
    let mut conversions: Vec<Conversion> = Vec::new();
    for (index, step) in set.steps.iter().enumerate() {
        let Step::Consumed {
            key, units, amount, ..
        } = step
        else {
            continue;
        };
        if key.name != SHRAPNEL || key.container == Container::Auction {
            continue;
        }
        let ammo_units = ammo_for_shrapnel(*units)?;
        let row = ammo_arrived.iter_mut().find_map(|(ammo_key, rows)| {
            let at = rows.iter().position(|row| row.quantity == ammo_units)?;
            Some((ammo_key.clone(), rows.remove(at)))
        });
        if let Some((ammo_key, row)) = row {
            let ammo = match ammo_unit {
                Some(unit) => Amount::exact_units(ammo_units, unit)?,
                None => Amount::whole_pec(row.value)?,
            };
            reserved.push((ammo_key, row));
            conversions.push(Conversion {
                index,
                shrapnel_units: *units,
                shrapnel: *amount,
                ammo_units,
                ammo,
            });
        }
    }

    Ok((reserved, conversions))
}

/// The ammo rows one step's Shrapnel conversions claim: rows an opening in
/// the same step never reads as its payout.
pub(crate) fn conversion_rows(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<ClaimedRows, AccountError> {
    Ok(conversions_of(set, before, after)?.0)
}

/// How an arrival counts given an opening's claim on it: not at all when
/// it is the opening's contents.
fn mark_of(claim: Option<&Claim>) -> Option<GainMark> {
    match claim {
        Some(Claim::Content) => None,
        Some(Claim::Possibly) => Some(GainMark::PossiblyStrongbox),
        None => Some(GainMark::Arrived),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::roles::CostCategory;
    use crate::account::test_support::reading;

    /// Knows one weapon and one enhancer by name, and nothing else.
    struct OneWeapon;

    impl RoleSource for OneWeapon {
        fn library_category(&self, _: &str) -> Option<CostCategory> {
            None
        }
        fn catalogue_category(&self, name: &str) -> Option<CostCategory> {
            match name {
                "Opalo" => Some(CostCategory::Weapon),
                "T1 Weapon Damage Enhancer" => Some(CostCategory::Enhancer),
                _ => None,
            }
        }
    }

    const WHOLE: SessionWindow = SessionWindow {
        started_at: 0.0,
        ended_at: Some(60.0),
    };

    type Rows<'a> = &'a [(&'static str, Container, i64, i64)];

    /// The measure of one step from `before` to `after`.
    fn measure_step(balances: (i64, i64), before: Rows, after: Rows) -> SessionMeasure {
        let readings = [
            reading(0.0, balances.0, before),
            reading(60.0, balances.1, after),
        ];
        measure_session(&WHOLE, &readings, &OneWeapon)
            .unwrap()
            .measure
    }

    fn carried(name: &str) -> HoldingKey {
        HoldingKey {
            name: name.to_string(),
            container: Container::Carried,
        }
    }

    fn pec(value: i64) -> Amount {
        Amount::whole_pec(Pec(value)).unwrap()
    }

    const C: Container = Container::Carried;

    fn settled(ammo: &AmmoUse) -> (i64, i64, i64) {
        match &ammo.split {
            AmmoSplit::Settled {
                burned_units,
                strongbox_units,
                converted_units,
                ..
            } => (*burned_units, *strongbox_units, *converted_units),
            AmmoSplit::Unsettled { .. } => panic!("an unsettled split: {ammo:?}"),
        }
    }

    fn topped_up(measure: &SessionMeasure) -> Vec<Span> {
        measure
            .events
            .iter()
            .filter_map(|event| match event {
                NotableEvent::AmmoToppedUp { between, name } if name == UNIVERSAL_AMMO => {
                    Some(*between)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ammo_arriving_in_a_new_row_with_nothing_behind_it_is_topped_up_and_its_step_left_out() {
        let readings = [
            reading(0.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
            reading(
                60.0,
                0,
                &[
                    ("Universal Ammo", C, 90_000, 900),
                    ("Universal Ammo", C, 400_000, 4_000),
                ],
            ),
            reading(
                120.0,
                0,
                &[
                    ("Universal Ammo", C, 90_000, 900),
                    ("Universal Ammo", C, 300_000, 3_000),
                ],
            ),
        ];
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(120.0),
        };
        let measure = measure_session(&window, &readings, &OneWeapon)
            .unwrap()
            .measure;
        assert_eq!(measure.ammo.len(), 1);
        assert_eq!(measure.ammo[0].net_units, 290_000, "the net is exact");
        // Only the second minute's burn is known; the first's is the
        // tracked figure's to stand in for.
        assert_eq!(settled(&measure.ammo[0]), (100_000, 0, 0));
        assert_eq!(
            topped_up(&measure),
            [Span {
                from: 0.0,
                to: 60.0
            }]
        );
        assert!(measure.gained.is_empty(), "a top-up is never loot");
        assert_eq!(measure.gained_total, Amount::NONE);
    }

    #[test]
    fn ammo_stacked_onto_its_row_with_nothing_behind_it_is_a_top_up_however_it_nets() {
        // Grown past anything fired: the arrival is all the rows show.
        let grown = measure_step(
            (0, 0),
            &[("Universal Ammo", C, 100_000, 1_000)],
            &[("Universal Ammo", C, 500_000, 5_000)],
        );
        assert_eq!(grown.ammo[0].net_units, 400_000, "the net is exact");
        assert_eq!(settled(&grown.ammo[0]), (0, 0, 0));
        assert_eq!(
            topped_up(&grown),
            [Span {
                from: 0.0,
                to: 60.0
            }]
        );
        assert!(grown.gained.is_empty());
        assert!(grown.topped_up_decay.is_empty(), "nothing wore");
        // Bought mid-session and more fired than bought: the session nets
        // to a fall, and the minute it arrived in is still a top-up.
        let readings = [
            reading(
                0.0,
                0,
                &[("Universal Ammo", C, 100_000, 1_000), ("Opalo", C, 1, 500)],
            ),
            reading(
                60.0,
                0,
                &[("Universal Ammo", C, 90_000, 900), ("Opalo", C, 1, 499)],
            ),
            reading(
                120.0,
                0,
                &[("Universal Ammo", C, 95_000, 950), ("Opalo", C, 1, 498)],
            ),
            reading(
                180.0,
                0,
                &[("Universal Ammo", C, 80_000, 800), ("Opalo", C, 1, 497)],
            ),
        ];
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(180.0),
        };
        let bought = measure_session(&window, &readings, &OneWeapon)
            .unwrap()
            .measure;
        assert_eq!(bought.ammo[0].net_units, -20_000);
        assert_eq!(settled(&bought.ammo[0]), (25_000, 0, 0));
        assert_eq!(
            topped_up(&bought),
            [Span {
                from: 60.0,
                to: 120.0
            }]
        );
        // Every minute's wear is in the decay; the top-up minute's is also
        // kept apart, for the tracked figure that stands in for it.
        assert_eq!(bought.decay.len(), 1);
        assert_eq!(bought.decay[0].decay, pec(3));
        assert_eq!(bought.topped_up_decay.len(), 1);
        assert_eq!(bought.topped_up_decay[0].key, carried("Opalo"));
        assert_eq!(bought.topped_up_decay[0].decay, pec(1));
    }

    #[test]
    fn a_stack_known_never_to_be_used_up_leaving_is_never_consumption() {
        // An enhancer the game data knows, traded: the reconciliation's.
        let traded = measure_step(
            (0, 0),
            &[("T1 Weapon Damage Enhancer", C, 10, 1_000)],
            &[("T1 Weapon Damage Enhancer", C, 8, 800)],
        );
        assert!(traded.other_consumption.is_empty());
        assert_eq!(traded.unexplained_departures.len(), 1);
        assert_eq!(traded.unexplained_departures[0].units, 2);
        // A stack nothing names is used up, for the costs to offer for
        // placement.
        let unknown = measure_step((0, 0), &[("Animal Oil Residue", C, 4_000, 40)], &[]);
        assert_eq!(unknown.other_consumption.len(), 1);
        assert!(unknown.unexplained_departures.is_empty());
    }

    #[test]
    fn neither_a_conversion_nor_an_opening_is_a_top_up() {
        // Shrapnel turned into a row of ammo of its own.
        let converted = measure_step(
            (0, 0),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[
                ("Universal Ammo", C, 95_000, 950),
                ("Universal Ammo", C, 10_100, 101),
            ],
        );
        assert!(topped_up(&converted).is_empty());
        assert_eq!(settled(&converted.ammo[0]), (5_000, 0, 10_100));
        // Shrapnel turned into ammo stacked onto the row being fired.
        let stacked = measure_step(
            (0, 0),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[("Universal Ammo", C, 105_100, 1_051)],
        );
        assert!(topped_up(&stacked).is_empty());
        // A strongbox opening paying out ammo.
        let opened = measure_step(
            (0, 0),
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
            &[
                ("Universal Ammo", C, 40_000, 400),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(topped_up(&opened).is_empty());
        assert_eq!(settled(&opened.ammo[0]), (10_000, 100_000, 0));
    }

    #[test]
    fn an_item_leaving_is_listed_and_a_card_rise_no_use_claims_is_ignored() {
        let measure = measure_step((1_000, 1_400), &[("Opalo", C, 1, 500)], &[]);
        assert!(measure.gained.is_empty());
        assert!(measure.other_consumption.is_empty());
        assert!(measure.decay.is_empty());
        let between = Span {
            from: 0.0,
            to: 60.0,
        };
        assert_eq!(
            measure.events,
            vec![NotableEvent::Left {
                between,
                key: carried("Opalo"),
                units: 1,
                value: pec(500),
            },]
        );
    }

    #[test]
    fn departures_the_card_s_rise_pays_for_are_a_sale_not_consumption() {
        let measure = measure_step(
            (1_000, 1_540),
            &[("Opalo", C, 1, 500), ("Animal Oil Residue", C, 4_000, 40)],
            &[],
        );
        assert!(measure.other_consumption.is_empty());
        let [NotableEvent::Sale { items, amount, .. }] = measure.events.as_slice() else {
            panic!("one sale: {:?}", measure.events);
        };
        assert_eq!(items.len(), 2);
        assert_eq!(amount.micro_ped(), 5_400_000);
    }

    #[test]
    fn a_card_movement_no_use_claims_changes_nothing_in_the_measure() {
        // A stack leaving and one arriving, with the Card moving by what
        // neither a sale nor a purchase of them would.
        let rows_before: Rows = &[("Animal Oil Residue", C, 4_000, 40)];
        let rows_after: Rows = &[("Odd Pill", C, 100, 1_000)];
        let moved = measure_step((1_000, 1_100), rows_before, rows_after);
        let still = measure_step((1_000, 1_000), rows_before, rows_after);
        assert_eq!(moved, still);
        assert_eq!(moved.other_consumption.len(), 1, "used up, as any stack");
        assert_eq!(moved.other_consumption[0].units, 4_000);
        assert!(moved.unexplained_departures.is_empty());
        assert_eq!(moved.gained.len(), 1);
        assert_eq!(moved.gained[0].mark, GainMark::Arrived);
        assert!(moved.events.is_empty());
    }

    #[test]
    fn arrivals_the_card_s_fall_pays_for_are_a_purchase_not_loot() {
        let bought = measure_step((5_000, 4_000), &[], &[("Odd Pill", C, 100, 1_000)]);
        assert!(bought.gained.is_empty());
        assert!(matches!(
            bought.events.as_slice(),
            [NotableEvent::Purchase { .. }]
        ));
        // A fall that does not balance leaves the arrival gained, as any.
        let unbalanced = measure_step((5_000, 4_500), &[], &[("Odd Pill", C, 100, 1_000)]);
        assert_eq!(unbalanced.gained.len(), 1);
        assert_eq!(unbalanced.gained[0].mark, GainMark::Arrived);
        assert!(unbalanced.events.is_empty());
        // Ammo bought at the Trade Terminal is still listed as bought, and
        // its minute is a top-up whatever the Card shows.
        let ammo = measure_step(
            (5_000, 4_000),
            &[("Universal Ammo", C, 50_000, 500)],
            &[
                ("Universal Ammo", C, 40_000, 400),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(ammo.gained.is_empty());
        assert!(matches!(
            ammo.events.as_slice(),
            [
                NotableEvent::Purchase { .. },
                NotableEvent::AmmoToppedUp { .. }
            ]
        ));
        assert_eq!(settled(&ammo.ammo[0]), (0, 0, 0));
    }

    const OIL: (&str, Container, i64, i64) = ("Animal Oil Residue", C, 4_000, 40);

    /// A minute at the Trade Terminal: Opalo restored 7 PEC when `repair`,
    /// 4000 oil (40 PEC) gone when `sell`, a stack of 100 pills (1000 PEC)
    /// new when `buy`, and the Card moving by `change`.
    fn terminal(repair: bool, sell: bool, buy: bool, change: i64) -> SessionMeasure {
        let mut before = vec![("Opalo", C, 1, 493), ("Universal Ammo", C, 50_000, 500)];
        let mut after = vec![
            ("Opalo", C, 1, if repair { 500 } else { 493 }),
            ("Universal Ammo", C, 50_000, 500),
        ];
        if sell {
            before.push(OIL);
        }
        if buy {
            after.push(("Odd Pill", C, 100, 1_000));
        }
        measure_step((5_000, 5_000 + change), &before, &after)
    }

    fn kinds(measure: &SessionMeasure) -> Vec<&'static str> {
        measure
            .events
            .iter()
            .map(|event| match event {
                NotableEvent::Sale { .. } => "sale",
                NotableEvent::Purchase { .. } => "purchase",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn the_trade_terminal_minute_is_read_jointly() {
        // Sell, buy and repair: 40 - 1000 - 7.
        let all = terminal(true, true, true, -967);
        assert_eq!(kinds(&all), vec!["sale", "purchase"]);
        assert_eq!(
            all.repairs[0].repair.cost,
            RepairCost::Derived { amount: pec(7) }
        );
        assert!(all.gained.is_empty() && all.other_consumption.is_empty());
        // Sell and repair: 40 - 7.
        let sold = terminal(true, true, false, 33);
        assert_eq!(kinds(&sold), vec!["sale"]);
        assert_eq!(
            sold.repairs[0].repair.cost,
            RepairCost::Derived { amount: pec(7) }
        );
        // Buy and repair: -1000 - 7.
        let bought = terminal(true, false, true, -1_007);
        assert_eq!(kinds(&bought), vec!["purchase"]);
        assert_eq!(
            bought.repairs[0].repair.cost,
            RepairCost::Derived { amount: pec(7) }
        );
        // Sell and buy: 40 - 1000.
        let traded = terminal(false, true, true, -960);
        assert_eq!(kinds(&traded), vec!["sale", "purchase"]);
        // The repair alone: its cost is the Card's fall.
        let repaired = terminal(true, false, false, -7);
        assert_eq!(
            repaired.repairs[0].repair.cost,
            RepairCost::Paid { pec: Pec(7) }
        );
        assert!(kinds(&repaired).is_empty());
    }

    #[test]
    fn a_trade_terminal_minute_no_combination_explains_counts_as_any_other() {
        let measure = terminal(false, true, true, -500);
        assert!(kinds(&measure).is_empty());
        assert_eq!(measure.other_consumption.len(), 1);
        assert!(measure.unexplained_departures.is_empty());
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].mark, GainMark::Arrived);
        let with_repair = terminal(true, true, true, -500);
        assert_eq!(with_repair.repairs[0].repair.cost, RepairCost::Unknown);
    }

    #[test]
    fn shrapnel_beside_the_ammo_row_it_pays_was_turned_into_ammo() {
        let measure = measure_step(
            (1_000, 1_000),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[
                ("Universal Ammo", C, 95_000, 950),
                ("Universal Ammo", C, 10_100, 101),
            ],
        );
        assert!(measure.other_consumption.is_empty(), "not consumption");
        assert!(measure.unexplained_departures.is_empty());
        assert!(measure.gained.is_empty(), "the ammo is not loot");
        assert_eq!(measure.ammo[0].net_units, 5_100);
        // 10000 Shrapnel became 10100 ammo, and 5000 were fired.
        assert_eq!(settled(&measure.ammo[0]), (5_000, 0, 10_100));
        let [NotableEvent::ShrapnelConverted {
            shrapnel_units,
            ammo_units,
            ammo,
            ..
        }] = measure.events.as_slice()
        else {
            panic!("one conversion: {:?}", measure.events);
        };
        assert_eq!((*shrapnel_units, *ammo_units), (10_000, 10_100));
        assert_eq!(*ammo, Amount::exact(1_010_000));
        // The same departure with a Card rise that pays for it is a sale.
        let sold = measure_step(
            (1_000, 1_100),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[("Universal Ammo", C, 100_000, 1_000)],
        );
        assert!(matches!(
            sold.events.as_slice(),
            [NotableEvent::Sale { .. }]
        ));
        assert!(sold.ammo.is_empty());
    }

    #[test]
    fn shrapnel_without_its_ammo_row_is_unexplained_and_unsettles_the_ammo() {
        // The ammo was stacked onto the row being fired: no row of 10100.
        let measure = measure_step(
            (1_000, 1_000),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[("Universal Ammo", C, 105_100, 1_051)],
        );
        assert_eq!(measure.unexplained_departures.len(), 1);
        assert!(measure.other_consumption.is_empty());
        assert!(matches!(
            measure.ammo[0].split,
            AmmoSplit::Unsettled {
                converted_units: 0,
                ..
            }
        ));
        assert!(measure
            .events
            .iter()
            .all(|event| !matches!(event, NotableEvent::ShrapnelConverted { .. })));
    }

    #[test]
    fn a_conversion_row_beside_an_opening_is_the_conversion_s_alone() {
        let before: Rows = &[
            ("Halloween Strongbox", C, 2, 0),
            ("Strongbox Key", C, 2, 2_000),
            ("Universal Ammo", C, 50_000, 500),
            ("Shrapnel", C, 10_000, 100),
        ];
        let unstacked = measure_step(
            (0, 0),
            before,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 10_100, 101),
            ],
        );
        assert_eq!(settled(&unstacked.ammo[0]), (0, 100_000, 10_100));
        assert!(unstacked.gained.is_empty());
        // Stacked onto the row it lands beside: no conversion row to read.
        let stacked = measure_step(
            (0, 0),
            before,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 60_100, 601),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(matches!(stacked.ammo[0].split, AmmoSplit::Unsettled { .. }));
        assert_eq!(stacked.unexplained_departures.len(), 1);
    }

    #[test]
    fn keys_are_never_consumption() {
        // A single key with a single box: an opening.
        let opened = measure_step(
            (0, 0),
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
            &[
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(opened.other_consumption.is_empty());
        assert_eq!(settled(&opened.ammo[0]), (0, 100_000, 0));
        assert!(opened.gained.is_empty(), "the payout is not loot");
        // Keys with no box: traded, an unexplained departure.
        let traded = measure_step((0, 0), &[("Strongbox Key", C, 3, 3_000)], &[]);
        assert!(traded.other_consumption.is_empty());
        assert_eq!(traded.unexplained_departures.len(), 1);
        // Keys the Card's rise pays for: sold.
        let sold = measure_step((0, 3_000), &[("Strongbox Key", C, 3, 3_000)], &[]);
        assert!(matches!(
            sold.events.as_slice(),
            [NotableEvent::Sale { .. }]
        ));
    }

    #[test]
    fn two_kinds_of_box_leaving_together_unsettle_the_ammo() {
        let measure = measure_step(
            (0, 0),
            &[
                ("Halloween Strongbox", C, 3, 0),
                ("Easter Strongbox", C, 3, 0),
                ("Strongbox Key", C, 4, 4_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Easter Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(matches!(measure.ammo[0].split, AmmoSplit::Unsettled { .. }));
        assert!(measure.other_consumption.is_empty());
        let opening = measure
            .events
            .iter()
            .find_map(|event| match event {
                NotableEvent::StrongboxOpening { opening, .. } => Some(opening),
                _ => None,
            })
            .expect("still an opening");
        assert_eq!(opening.boxes.len(), 2);
        assert_eq!(opening.settlement, Settlement::Unsettled);
    }

    #[test]
    fn a_stack_leaving_the_auction_is_a_listing_that_sold() {
        let measure = measure_step(
            (1_000, 1_000),
            &[("Animal Oil Residue", Container::Auction, 4_000, 40)],
            &[],
        );
        assert!(measure.other_consumption.is_empty());
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::ListingSold { units: 4_000, .. }]
        ));
    }

    #[test]
    fn an_unknown_single_thing_arrives_and_leaves_as_an_item() {
        let arrived = measure_step((0, 0), &[], &[("Mayhem Token", C, 1, 0)]);
        assert_eq!(arrived.gained.len(), 1);
        assert_eq!(arrived.gained[0].stacking, Stacking::Single);
        assert_eq!(arrived.gained[0].mark, GainMark::Arrived);
        assert!(matches!(
            arrived.events.as_slice(),
            [NotableEvent::Appeared { units: 1, .. }]
        ));
        let left = measure_step((0, 0), &[("Christmas Coat 2025 (M)", C, 1, 100)], &[]);
        assert!(
            left.other_consumption.is_empty(),
            "a departure of one is never consumption"
        );
        assert!(matches!(
            left.events.as_slice(),
            [NotableEvent::Left { .. }]
        ));
    }

    #[test]
    fn a_looted_weapon_counts_in_what_was_gained_as_an_item() {
        let measure = measure_step((0, 0), &[], &[("Opalo", C, 1, 500)]);
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].role, Role::Weapon);
        assert_eq!(measure.gained[0].stacking, Stacking::Single);
        assert_eq!(measure.gained_total, pec(500));
    }

    #[test]
    fn a_listing_is_a_move_and_its_fee_is_ignored() {
        let measure = measure_step(
            (1_000, 995),
            &[("Opalo", C, 1, 500)],
            &[("Opalo", Container::Auction, 1, 500)],
        );
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::Moved {
                move_kind: MoveKind::Listing,
                units: 1,
                ..
            },]
        ));
        assert!(measure.gained.is_empty() && measure.other_consumption.is_empty());
    }

    #[test]
    fn wear_is_never_netted_against_a_repair() {
        let readings = [
            reading(0.0, 1_000, &[("Opalo", C, 1, 500)]),
            reading(60.0, 1_000, &[("Opalo", C, 1, 497)]),
            reading(120.0, 993, &[("Opalo", C, 1, 504)]),
        ];
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(120.0),
        };
        let measure = measure_session(&window, &readings, &OneWeapon)
            .unwrap()
            .measure;
        assert_eq!(measure.decay.len(), 1);
        assert_eq!(measure.decay[0].role, Role::Weapon);
        assert_eq!(measure.decay[0].decay, pec(3));
        assert_eq!(measure.repairs.len(), 1);
        assert_eq!(measure.repairs[0].repair.restored, pec(7));
        assert_eq!(
            measure.repairs[0].repair.cost,
            RepairCost::Paid { pec: Pec(7) }
        );
        assert!(measure.events.is_empty(), "the repair explains the Card");
    }

    #[test]
    fn stacks_consumed_outside_an_opening_are_other_consumption_with_their_role() {
        let measure = measure_step(
            (0, 0),
            &[("Mind Essence", C, 53_833, 538)],
            &[("Mind Essence", C, 52_933, 529)],
        );
        assert_eq!(measure.other_consumption.len(), 1);
        assert_eq!(measure.other_consumption[0].role, Role::Material);
        assert_eq!(measure.other_consumption[0].units, 900);
        assert_eq!(measure.other_consumption[0].amount, Amount::exact(90_000));
    }

    #[test]
    fn a_vanished_small_ammo_row_is_burned_and_the_new_row_credited() {
        let measure = measure_step(
            (0, 0),
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 5_000, 50),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert_eq!(settled(&measure.ammo[0]), (5_000, 100_000, 0));
        assert!(measure.gained.is_empty());
        assert!(measure.other_consumption.is_empty());
    }

    #[test]
    fn when_the_rows_allow_two_readings_the_keys_decide_the_split() {
        let measure = measure_step(
            (0, 0),
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 95_100, 951),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert_eq!(settled(&measure.ammo[0]), (4_900, 100_000, 0));
    }

    fn opening_with(extra: Rows, ammo_rows: usize) -> SessionMeasure {
        let mut after = vec![
            ("Halloween Strongbox", C, 16, 0),
            ("Strongbox Key", C, 17, 17_000),
            ("Universal Ammo", C, 443_923, 4_439),
        ];
        for _ in 0..ammo_rows {
            after.push(("Universal Ammo", C, 100_000, 1_000));
        }
        after.extend_from_slice(extra);
        measure_step(
            (25_000, 25_000),
            &[
                ("Halloween Strongbox", C, 23, 0),
                ("Strongbox Key", C, 24, 24_000),
                ("Universal Ammo", C, 443_923, 4_439),
            ],
            &after,
        )
    }

    #[test]
    fn loot_beside_an_opening_settled_by_subset_stays_gained_and_the_unplaced_is_marked() {
        let measure = opening_with(
            &[
                ("Skeleton Pants Red (F)", C, 1, 1_000),
                ("Shrapnel", C, 281_816, 2_818),
                ("Mayhem Token", C, 1, 0),
            ],
            6,
        );
        let marks: Vec<(&str, GainMark)> = measure
            .gained
            .iter()
            .map(|gain| (gain.key.name.as_str(), gain.mark))
            .collect();
        assert_eq!(
            marks,
            vec![
                ("Mayhem Token", GainMark::PossiblyStrongbox),
                ("Shrapnel", GainMark::Arrived),
            ]
        );
        assert_eq!(
            measure.gained_total,
            Amount::exact(28_181_600).plus(pec(0)).unwrap()
        );
        assert_eq!(measure.gained_possibly_strongbox.micro_ped(), 0);
        assert_eq!(settled(&measure.ammo[0]), (0, 600_000, 0));
    }

    #[test]
    fn loot_two_closing_ways_share_stays_possibly_and_the_split_settles() {
        let measure = opening_with(
            &[
                ("Skeleton Shirt (F)", C, 1, 1_000),
                ("Shrapnel", C, 100_000, 1_000),
            ],
            6,
        );
        assert_eq!(measure.gained.len(), 2);
        assert!(measure
            .gained
            .iter()
            .all(|gain| gain.mark == GainMark::PossiblyStrongbox));
        assert_eq!(measure.gained_possibly_strongbox, measure.gained_total);
        assert_eq!(measure.gained_total.micro_ped(), 20_000_000);
        assert_eq!(settled(&measure.ammo[0]), (0, 600_000, 0));
    }

    #[test]
    fn loot_beside_an_opening_that_does_not_close_stays_possibly_and_unsettles_the_split() {
        let measure = opening_with(&[("Skeleton Shirt (F)", C, 1, 3_000)], 6);
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].mark, GainMark::PossiblyStrongbox);
        assert!(matches!(
            measure.ammo[0].split,
            AmmoSplit::Unsettled {
                strongbox_units_by_rows: 600_000,
                ..
            }
        ));
        assert_eq!(
            measure.ammo[0].net_units, 600_000,
            "the net is exact whatever the split"
        );
    }

    #[test]
    fn an_arrival_of_no_tt_beside_an_opening_alone_is_possibly_its() {
        let measure = opening_with(&[("Halloween Fireworks", C, 1, 0)], 7);
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].mark, GainMark::PossiblyStrongbox);
        assert_eq!(settled(&measure.ammo[0]), (0, 700_000, 0));
    }

    fn at(times: &[f64]) -> Vec<Reading> {
        times.iter().map(|t| reading(*t, 0, &[])).collect()
    }

    #[test]
    fn the_opening_and_closing_readings_may_be_ninety_seconds_away_and_no_more() {
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(200.0),
        };
        let coverage = |times: &[f64]| {
            measure_session(&window, &at(times), &OneWeapon)
                .unwrap()
                .measure
                .coverage
        };
        let exactly = coverage(&[10.0, 150.0, 290.0]);
        assert_eq!((exactly.opening, exactly.closing), (Edge::Read, Edge::Read));
        assert_eq!(exactly.level, CoverageLevel::Complete);
        let beyond = coverage(&[9.5, 150.0, 290.5]);
        assert_eq!(
            (beyond.opening, beyond.closing),
            (Edge::Unread, Edge::Unread)
        );
        assert_eq!(beyond.level, CoverageLevel::Partial);
        assert_eq!(beyond.reading_count, 1);
    }

    #[test]
    fn only_the_session_s_own_baseline_within_ten_seconds_reads_its_start() {
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(200.0),
        };
        let measured = |first: f64, baseline: Option<usize>| {
            measure_session_with_baseline(
                &window,
                &Neighbours::default(),
                &at(&[first, 160.0, 220.0]),
                baseline,
                &OneWeapon,
            )
            .unwrap()
            .measure
            .coverage
        };
        // The baseline taken the moment the session started: the start is
        // read and the session is complete.
        let prompt = measured(100.4, Some(0));
        assert_eq!(prompt.opening, Edge::Read);
        assert_eq!(prompt.level, CoverageLevel::Complete);
        assert_eq!(prompt.reading_count, 3);
        assert_eq!(measured(105.0, Some(0)).opening, Edge::Read);
        assert_eq!(measured(110.0, Some(0)).opening, Edge::Read);
        // A baseline that landed later than that does not read it.
        assert_eq!(measured(110.5, Some(0)).opening, Edge::Unread);
        assert_eq!(measured(120.0, Some(0)).opening, Edge::Unread);
        // Nor does any other reading, however early: a connect part-way
        // through is partial at 5, 20 or 100 seconds in.
        for first in [105.0, 120.0, 200.0] {
            let coverage = measured(first, None);
            assert_eq!(coverage.opening, Edge::Unread, "{first}");
            assert_eq!(coverage.level, CoverageLevel::Partial, "{first}");
        }
        // A baseline that is not the first reading inside the session does
        // not read it either.
        let outcome = measure_session_with_baseline(
            &window,
            &Neighbours::default(),
            &at(&[100.4, 103.0, 160.0, 220.0]),
            Some(1),
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
        // An older reading close before the start is still the one used.
        let outcome = measure_session_with_baseline(
            &window,
            &Neighbours::default(),
            &at(&[50.0, 100.4, 160.0, 220.0]),
            Some(1),
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.used, 0..4);
        assert_eq!(outcome.measure.coverage.opening, Edge::Read);
    }

    #[test]
    fn an_interval_of_150_seconds_is_not_a_gap_and_longer_is() {
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(400.0),
        };
        let coverage = |times: &[f64]| {
            measure_session(&window, &at(times), &OneWeapon)
                .unwrap()
                .measure
                .coverage
        };
        let at_most = coverage(&[0.0, 150.0, 300.0, 400.0]);
        assert_eq!(at_most.gap_count, 0);
        assert_eq!(at_most.level, CoverageLevel::Complete);
        let longer = coverage(&[0.0, 150.5, 300.0, 400.0]);
        assert_eq!(longer.gap_count, 1);
        assert_eq!(longer.level, CoverageLevel::Partial);
    }

    #[test]
    fn one_reading_that_reads_both_ends_is_still_partial() {
        // A session stopped the moment it started: its one reading, taken
        // at that moment, reads both ends.
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(100.0),
        };
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &at(&[100.0]),
            0..1,
            None,
            &OneWeapon,
        )
        .unwrap();
        let coverage = outcome.measure.coverage;
        assert_eq!(
            (coverage.opening, coverage.closing),
            (Edge::Read, Edge::Read)
        );
        assert_eq!(coverage.reading_count, 1);
        assert_eq!(coverage.level, CoverageLevel::Partial);
    }

    #[test]
    fn a_session_s_own_readings_are_measured_with_their_neighbours_only_for_unread_ends() {
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(300.0),
        };
        // The reading before reads the start; the session's own late
        // closing reading reads the end, so the reading after is not used.
        let readings = at(&[40.0, 110.0, 170.0, 230.0, 320.0, 330.0]);
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            1..5,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.used, 0..5);
        assert_eq!(outcome.measure.coverage.level, CoverageLevel::Complete);
        // Without a closing reading of its own, the reading after reads it.
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            1..4,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.used, 0..5);
        assert_eq!(outcome.measure.coverage.closing, Edge::Read);
        // A reading before that is too old is not used.
        let readings = at(&[5.0, 110.0, 170.0, 230.0, 320.0]);
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            1..5,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.used, 1..5);
        assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
    }

    #[test]
    fn a_baseline_that_came_back_after_the_stop_does_not_read_the_start() {
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(105.0),
        };
        let readings = at(&[108.0, 168.0]);
        let late = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            0..2,
            Some(0),
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(late.measure.coverage.opening, Edge::Unread);
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(200.0),
        };
        let readings = at(&[108.0, 168.0, 210.0]);
        let prompt = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            0..3,
            Some(0),
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(prompt.measure.coverage.opening, Edge::Read);
        assert_eq!(prompt.measure.coverage.level, CoverageLevel::Complete);
    }

    /// Readings a minute apart of one weapon wearing a PEC a minute, from
    /// 500 PEC at the first.
    fn wearing(times: &[f64]) -> Vec<Reading> {
        times
            .iter()
            .enumerate()
            .map(|(index, at)| reading(*at, 0, &[("Opalo", C, 1, 500 - index as i64)]))
            .collect()
    }

    fn decayed(outcome: &MeasureOutcome) -> i64 {
        outcome
            .measure
            .decay
            .iter()
            .map(|item| item.decay.micro_ped())
            .sum()
    }

    #[test]
    fn the_measure_runs_through_its_closing_readings_until_the_next_session_starts() {
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(300.0),
        };
        // Two more closing readings follow the first, and the item wears
        // across them: the site catching up on the session's own play.
        let readings = wearing(&[40.0, 110.0, 170.0, 230.0, 320.0, 380.0, 440.0]);
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            1..7,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.used, 0..7);
        assert_eq!(outcome.measure.coverage.level, CoverageLevel::Complete);
        assert_eq!(decayed(&outcome), 6 * MICRO_PER_PEC);
        // The next session started at 380: the closing readings taken then
        // or later are not this one's.
        let followed = Neighbours {
            previous_ended_at: None,
            next_started_at: Some(380.0),
        };
        let outcome =
            measure_own_readings(&window, &followed, &readings, 1..7, None, &OneWeapon).unwrap();
        assert_eq!(outcome.used, 0..5);
        assert_eq!(outcome.measure.coverage.closing, Edge::Read);
        assert_eq!(decayed(&outcome), 4 * MICRO_PER_PEC);
    }

    #[test]
    fn a_reading_taken_across_a_neighbouring_session_s_play_reads_neither_end() {
        // The session before stopped at 100; this one started at 105 and
        // its first reading of its own came at 150.
        let window = SessionWindow {
            started_at: 105.0,
            ended_at: Some(200.0),
        };
        let readings = wearing(&[90.0, 150.0, 210.0]);
        let after_another = Neighbours {
            previous_ended_at: Some(100.0),
            next_started_at: None,
        };
        let outcome =
            measure_own_readings(&window, &after_another, &readings, 1..3, None, &OneWeapon)
                .unwrap();
        assert_eq!(outcome.used, 1..3, "the reading before is the other's");
        assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
        let alone = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            1..3,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(alone.used, 0..3, "with nothing before, it reads the start");
        // The session before: its own readings end before its stop at 100,
        // and the reading after it came once this one had started.
        let before = SessionWindow {
            started_at: 0.0,
            ended_at: Some(100.0),
        };
        let readings = wearing(&[0.0, 60.0, 90.0, 150.0]);
        let followed = Neighbours {
            previous_ended_at: None,
            next_started_at: Some(105.0),
        };
        let outcome =
            measure_own_readings(&before, &followed, &readings, 0..3, None, &OneWeapon).unwrap();
        assert_eq!(outcome.used, 0..3);
        assert_eq!(outcome.measure.coverage.closing, Edge::Unread);
        // The same through the window, for readings tagged with no session.
        let by_window =
            measure_session_with_baseline(&before, &followed, &readings, None, &OneWeapon).unwrap();
        assert_eq!(by_window.used, 0..3);
        assert_eq!(by_window.measure.coverage.closing, Edge::Unread);
        let by_window = measure_session_with_baseline(
            &window,
            &after_another,
            &wearing(&[90.0, 150.0, 210.0]),
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(by_window.used, 1..3);
    }

    #[test]
    fn a_session_started_inside_the_floor_opens_with_the_reading_that_closed_the_one_before() {
        // The session before stopped at 100 and was closed by the reading
        // at 120; this one started at 130, inside the floor, so its first
        // reading of its own came at 180.
        let readings = wearing(&[60.0, 120.0, 180.0, 240.0]);
        let earlier = measure_own_readings(
            &SessionWindow {
                started_at: 0.0,
                ended_at: Some(100.0),
            },
            &Neighbours {
                previous_ended_at: None,
                next_started_at: Some(130.0),
            },
            &readings,
            0..2,
            None,
            &OneWeapon,
        )
        .unwrap();
        let later = measure_own_readings(
            &SessionWindow {
                started_at: 130.0,
                ended_at: Some(230.0),
            },
            &Neighbours {
                previous_ended_at: Some(100.0),
                next_started_at: None,
            },
            &readings,
            2..4,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(earlier.used, 0..2);
        assert_eq!(later.used, 1..4);
        assert_eq!(earlier.measure.coverage.closing, Edge::Read);
        assert_eq!(later.measure.coverage.opening, Edge::Read);
        assert_eq!(
            earlier.used.end - 1,
            later.used.start,
            "they share the one reading and no step"
        );
    }

    #[test]
    fn time_running_backwards_between_two_readings_is_a_gap() {
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(400.0),
        };
        let readings = at(&[0.0, 100.0, 40.0, 140.0, 280.0, 400.0]);
        let outcome = measure_own_readings(
            &window,
            &Neighbours::default(),
            &readings,
            0..6,
            None,
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.measure.coverage.gap_count, 1);
        assert_eq!(outcome.measure.coverage.level, CoverageLevel::Partial);
    }

    #[test]
    fn readings_are_taken_in_the_order_stored_when_the_clock_steps_back() {
        // The clock stepped back an hour after the second reading.
        let readings = at(&[3_600.0, 3_660.0, 70.0, 130.0]);
        let window = SessionWindow {
            started_at: 3_590.0,
            ended_at: None,
        };
        let outcome = measure_session(&window, &readings, &OneWeapon).unwrap();
        assert_eq!(
            outcome.used,
            0..2,
            "only the readings taken inside the window"
        );
    }

    // ── Across a gap ─────────────────────────────────────────────────

    /// The session the gap tests read: from 1 000 to 2 000.
    const DROPPED: SessionWindow = SessionWindow {
        started_at: 1_000.0,
        ended_at: Some(2_000.0),
    };

    /// Readings of the weapon losing a PEC at each, at these times.
    fn worn_at(times: &[f64]) -> Vec<Reading> {
        times
            .iter()
            .enumerate()
            .map(|(k, at)| reading(*at, 0, &[("Opalo", C, 1, 5_000 - k as i64)]))
            .collect()
    }

    fn across(
        readings: &[Reading],
        own: Range<usize>,
        neighbours: Neighbours,
        next_baseline: Option<usize>,
    ) -> GapMeasure {
        measure_across_gap(
            &DROPPED,
            &neighbours,
            readings,
            own,
            None,
            next_baseline,
            &OneWeapon,
        )
        .unwrap()
    }

    fn offered(measure: GapMeasure) -> AcrossGap {
        match measure {
            GapMeasure::Offered(gap) => *gap,
            other => panic!("offered: {other:?}"),
        }
    }

    /// Readings that stop at 1 120 and the next one at 3 800: the gap at
    /// the end is read by that one, 1 800 seconds after the stop, and its
    /// step alone is measured beside the whole.
    #[test]
    fn an_unread_end_is_read_across_the_gap_by_the_next_reading() {
        let readings = worn_at(&[990.0, 1_060.0, 1_120.0, 3_800.0]);
        let gap = offered(across(&readings, 1..3, Neighbours::default(), None));
        assert_eq!(gap.used, 0..4);
        assert_eq!(gap.opening, None);
        assert_eq!(
            gap.closing,
            Some(GapEnd {
                index: 3,
                outside_seconds: 1_800.0,
                gap_seconds: 2_680.0,
            })
        );
        assert_eq!(gap.steps.len(), 1);
        let wear = |measure: &SessionMeasure| -> i64 {
            measure
                .decay
                .iter()
                .map(|item| item.decay.micro_ped())
                .sum()
        };
        assert_eq!(wear(&gap.measure), 3 * MICRO_PER_PEC);
        assert_eq!(wear(&gap.steps[0]), MICRO_PER_PEC);
        // Before that reading is stored, the gap waits for it.
        assert_eq!(
            across(&readings[..3], 1..3, Neighbours::default(), None),
            GapMeasure::Awaiting
        );
    }

    /// A reading taken across another session's play reads neither end,
    /// unless it is the next session's own baseline.
    #[test]
    fn a_reading_across_a_neighbour_s_play_reads_no_gap() {
        let readings = worn_at(&[990.0, 1_060.0, 1_120.0, 3_800.0]);
        let next = Neighbours {
            previous_ended_at: None,
            next_started_at: Some(3_000.0),
        };
        assert_eq!(across(&readings, 1..3, next, None), GapMeasure::NotOffered);
        assert!(matches!(
            across(&readings, 1..3, next, Some(3)),
            GapMeasure::Offered(_)
        ));
        // Before the start, one taken before the session before stopped.
        let readings = worn_at(&[
            500.0, 1_400.0, 1_460.0, 1_520.0, 1_580.0, 1_640.0, 1_700.0, 1_760.0, 1_820.0, 1_880.0,
            1_940.0, 2_000.0,
        ]);
        let previous = Neighbours {
            previous_ended_at: Some(800.0),
            next_started_at: None,
        };
        assert_eq!(
            across(&readings, 1..12, previous, None),
            GapMeasure::NotOffered
        );
        let gap = offered(across(&readings, 1..12, Neighbours::default(), None));
        assert_eq!(gap.opening.map(|end| end.index), Some(0));
        assert_eq!(gap.closing, None);
    }

    /// Only the steps the far readings bound may be gaps: a session whose
    /// readings also stop inside it is not measured across a gap, and one
    /// read whole needs no gap at all.
    #[test]
    fn a_gap_inside_the_session_or_none_at_all_offers_nothing() {
        let readings = worn_at(&[990.0, 1_060.0, 1_500.0, 3_800.0]);
        assert_eq!(
            across(&readings, 1..3, Neighbours::default(), None),
            GapMeasure::NotOffered
        );
        let minutes = worn_at(&[
            990.0, 1_060.0, 1_120.0, 1_180.0, 1_240.0, 1_300.0, 1_360.0, 1_420.0, 1_480.0, 1_540.0,
            1_600.0, 1_660.0, 1_720.0, 1_780.0, 1_840.0, 1_900.0, 1_960.0, 2_010.0,
        ]);
        assert_eq!(
            across(&minutes, 1..18, Neighbours::default(), None),
            GapMeasure::NotOffered
        );
    }
}
