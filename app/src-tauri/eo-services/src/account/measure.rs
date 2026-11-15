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
//! after. The measure is complete only with both ends read, at least two
//! readings, and no gap between consecutive readings: an interval above
//! 150 seconds, or time running backwards between two readings, is a gap,
//! counted. A gap is never filled with an estimate: what changed across it
//! is measured as one step, and the coverage says so.
//!
//! Nothing that arrives or leaves vanishes from the measure, and nothing
//! is booked as a cost or a gain on a guess:
//!
//! - Shrapnel that leaves beside a new ammo row of exactly what the Trade
//!   Terminal pays for it was turned into ammo, credited as converted;
//!   Shrapnel that leaves without that row is an unexplained departure,
//!   and its ammo type's split is unsettled;
//! - the PED Card's change in a step is explained jointly: what left may be
//!   a sale, what arrived a purchase, and a repair costs within what its
//!   restorations allow. When exactly one combination fits, that is what
//!   happened; when the Card moved and none (or more than one) fits,
//!   nothing in the step is booked as a plain cost or plain loot;
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
use super::diff::{added_rows, canonical_rows, diff, exact_unit, MoveKind, Span, Step};
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

/// The version of the measure's shape and rules. A cached measure of
/// another version is recomputed from its readings.
pub const MEASURE_VERSION: i64 = 4;

/// A tracked session's span, unix seconds; no end while it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionWindow {
    pub started_at: f64,
    pub ended_at: Option<f64>,
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
    /// the net change less what openings and Shrapnel paid back.
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
    /// Arrived in a step where the PED Card moved and nothing explains
    /// the movement: it may have been bought.
    CardMovedUnexplained,
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
    /// The PED Card moved with nothing in the step known to explain it.
    CardChanged {
        between: Span,
        from: Pec,
        to: Pec,
    },
}

/// A session's measured account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeasure {
    pub coverage: Coverage,
    pub ammo: Vec<AmmoUse>,
    pub decay: Vec<ItemDecay>,
    pub repairs: Vec<TimedRepair>,
    /// Stackables consumed that are not ammo, not an opening's keys or
    /// boxes, not sold, and not Shrapnel turned into ammo.
    pub other_consumption: Vec<ItemUnits>,
    /// Stackables that left with nothing to explain them: keys with no box
    /// beside them, Shrapnel with no ammo row for it, and whatever left in
    /// a step whose Card movement nothing explains. Never a cost.
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
    let opening = before.filter(|&index| reads_the_start(window, at(index)));
    // Without a reading close before the start, the session's own
    // baseline reads it, when it is the first reading inside the session
    // and was taken the moment the session started.
    let baseline = baseline.filter(|&index| {
        core.as_ref().is_some_and(|core| core.start == index)
            && at(index) - window.started_at <= BASELINE_WITHIN_SECONDS
    });
    let closing = window
        .ended_at
        .and_then(|ended| after.filter(|&index| index < n && reads_the_end(ended, at(index))));
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

/// Whether a reading taken at `at` reads the session's start.
fn reads_the_start(window: &SessionWindow, at: f64) -> bool {
    at <= window.started_at && window.started_at - at <= OPENING_WITHIN_SECONDS
}

/// Whether a reading taken at `at` reads the session's end.
fn reads_the_end(ended: f64, at: f64) -> bool {
    at >= ended && at - ended <= CLOSING_WITHIN_SECONDS
}

/// Which readings a session's measure uses when its own readings are
/// known: `own` is their range among `readings`, which hold besides them
/// at most the reading stored just before them and the one stored just
/// after them. Those two are used only to read an end the session's own
/// readings leave unread.
fn select_own(
    window: &SessionWindow,
    readings: &[Reading],
    own: Range<usize>,
    baseline: Option<usize>,
) -> (Range<usize>, Edge, Edge) {
    let n = readings.len();
    let at = |index: usize| readings[index].taken_at();
    if own.is_empty() {
        return (own, Edge::Unread, Edge::Unread);
    }
    let by_own = reads_the_start(window, at(own.start)).then_some(own.start);
    let by_before = own
        .start
        .checked_sub(1)
        .filter(|&index| by_own.is_none() && reads_the_start(window, at(index)));
    // The baseline reads the start only when it was taken inside the
    // session: one that came back after the session stopped saw its end.
    let by_baseline = baseline.filter(|&index| {
        index == own.start
            && at(index) - window.started_at <= BASELINE_WITHIN_SECONDS
            && window.ended_at.is_none_or(|ended| at(index) < ended)
    });
    let (closing, extended) = match window.ended_at {
        None => (None, false),
        Some(ended) => match own.clone().find(|&index| at(index) >= ended) {
            Some(index) => (reads_the_end(ended, at(index)).then_some(index), false),
            None => {
                let after = (own.end < n && reads_the_end(ended, at(own.end))).then_some(own.end);
                (after, after.is_some())
            }
        },
    };
    let first = by_before.unwrap_or(own.start);
    let last = if extended { own.end + 1 } else { own.end };
    (
        first..last,
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
    measure_session_with_baseline(window, readings, None, roles)
}

/// Measure a session over its readings, in the order they were stored,
/// chosen by its window; `baseline` is the position of the session's own
/// baseline among them, when it has one.
pub fn measure_session_with_baseline(
    window: &SessionWindow,
    readings: &[Reading],
    baseline: Option<usize>,
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let (used, opening, closing) = select(window, readings, baseline);
    measure_selected(readings, used, opening, closing, roles)
}

/// Measure a session over its own readings: `own` is their range among
/// `readings`, which also hold the reading stored just before them and the
/// one stored just after them, when there are such readings; `baseline` is
/// the position of the session's own baseline, when it has one.
pub fn measure_own_readings(
    window: &SessionWindow,
    readings: &[Reading],
    own: Range<usize>,
    baseline: Option<usize>,
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let (used, opening, closing) = select_own(window, readings, own, baseline);
    measure_selected(readings, used, opening, closing, roles)
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
struct Conversion {
    index: usize,
    shrapnel_units: i64,
    shrapnel: Amount,
    ammo_units: i64,
    ammo: Amount,
}

/// How the PED Card's change in a step is read.
enum CardReading {
    /// The Card did not move and nothing was restored.
    Still,
    /// Exactly one combination of sale, purchase and repair explains it.
    Explained {
        sale: bool,
        purchase: bool,
        cost: RepairCost,
    },
    /// The Card did not move, and no one combination costs the repair.
    StillUncosted,
    /// The Card moved, and no one combination explains it.
    Unexplained,
}

#[derive(Default)]
struct Tally {
    ammo: BTreeMap<String, AmmoTally>,
    decay: BTreeMap<HoldingKey, (Role, Amount)>,
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
    let still = delta.micro_ped() == 0;
    if still && repair.is_none() {
        return Ok(CardReading::Still);
    }
    let unread = if still {
        CardReading::StillUncosted
    } else {
        CardReading::Unexplained
    };
    // A listing made or returned moves the Card by a fee nothing here
    // reads: no combination can explain the step.
    if listing_moved {
        return Ok(unread);
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
        return Ok(unread);
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

        // Shrapnel conversions, read from the ammo row each pays: a new
        // Universal Ammo row of exactly what the Trade Terminal gives.
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
            let ammo_units = checked(
                units
                    .checked_mul(SHRAPNEL_TO_AMMO_PERCENT)
                    .map(|scaled| scaled / 100),
            )?;
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

        for (index, step) in set.steps.iter().enumerate() {
            match step {
                Step::Decayed { key, by } => {
                    let entry = self
                        .decay
                        .entry(key.clone())
                        .or_insert((book.role(&key.name), Amount::NONE));
                    entry.1 = entry.1.plus(*by)?;
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
                        });
                    }
                }
                Step::BalanceChanged { from, to } => card = Some((*from, *to)),
            }
        }

        // Ammo: what openings and conversions paid back is credited; new
        // rows beyond that arrived from elsewhere (a purchase, read with
        // the Card, or a gain).
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
        let mut credits: BTreeMap<String, (i64, Amount)> = BTreeMap::new();
        let mut uncertain_growth: Vec<String> = Vec::new();
        for (name, step) in &ammo_steps {
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
            if beyond > 0 && step.grew {
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
        let (sold, bought, card_unexplained, cost) = match reading {
            CardReading::Still => (false, false, false, RepairCost::Unknown),
            CardReading::StillUncosted => (false, false, false, RepairCost::Unknown),
            CardReading::Explained {
                sale,
                purchase,
                cost,
            } => (sale, purchase, false, cost),
            CardReading::Unexplained => (false, false, true, RepairCost::Unknown),
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
                Leaving::Stack if !card_unexplained => add_units(
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

        // What is left of the arrivals was gained.
        for arrival in arrivals {
            if arrival.appeared {
                self.events.push(NotableEvent::Appeared {
                    between,
                    key: arrival.key.clone(),
                    units: arrival.units,
                    value: arrival.amount,
                });
            }
            let mark = if card_unexplained && arrival.mark == GainMark::Arrived {
                GainMark::CardMovedUnexplained
            } else {
                arrival.mark
            };
            let role = book.role(&arrival.key.name);
            let entry = self.gained.entry((arrival.key, mark)).or_insert((
                role,
                arrival.stacking,
                0,
                Amount::NONE,
            ));
            entry.2 = checked(entry.2.checked_add(arrival.units))?;
            entry.3 = entry.3.plus(arrival.amount)?;
        }

        if let (Some((from, to)), true) = (card, card_unexplained) {
            self.events
                .push(NotableEvent::CardChanged { between, from, to });
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
    use crate::account::test_support::reading;

    /// Knows one weapon by name, and nothing else.
    struct OneWeapon;

    impl RoleSource for OneWeapon {
        fn library_role(&self, _: &str) -> Option<Role> {
            None
        }
        fn catalogue_role(&self, name: &str) -> Option<Role> {
            (name == "Opalo").then_some(Role::Weapon)
        }
        fn protection_role(&self, _: &str) -> Option<Role> {
            None
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

    #[test]
    fn ammo_that_arrived_in_a_new_row_beyond_any_credit_is_gained_not_burned() {
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
        assert_eq!(measure.ammo[0].net_units, 290_000);
        assert_eq!(settled(&measure.ammo[0]), (110_000, 0, 0));
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].role, Role::Ammo);
        assert_eq!(measure.gained[0].stacking, Stacking::Stack);
        assert_eq!(measure.gained[0].units, 400_000);
        assert_eq!(measure.gained[0].amount, Amount::exact(40_000_000));
        assert_eq!(measure.gained_total, Amount::exact(40_000_000));
    }

    #[test]
    fn ammo_that_may_have_been_stacked_onto_a_row_unsettles_the_split() {
        let readings = [
            reading(0.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
            reading(60.0, 0, &[("Universal Ammo", C, 500_000, 5_000)]),
        ];
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(60.0),
        };
        let measure = measure_session(&window, &readings, &OneWeapon)
            .unwrap()
            .measure;
        assert_eq!(measure.ammo[0].net_units, 400_000, "the net is exact");
        assert!(matches!(measure.ammo[0].split, AmmoSplit::Unsettled { .. }));
    }

    #[test]
    fn an_item_leaving_and_an_unexplained_card_rise_are_listed_and_never_counted() {
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
            vec![
                NotableEvent::Left {
                    between,
                    key: carried("Opalo"),
                    units: 1,
                    value: pec(500),
                },
                NotableEvent::CardChanged {
                    between,
                    from: Pec(1_000),
                    to: Pec(1_400),
                },
            ]
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
    fn a_card_movement_nothing_explains_books_nothing_as_a_plain_cost() {
        let measure = measure_step((1_000, 1_100), &[("Animal Oil Residue", C, 4_000, 40)], &[]);
        assert!(measure.other_consumption.is_empty());
        assert_eq!(measure.unexplained_departures.len(), 1);
        assert_eq!(measure.unexplained_departures[0].units, 4_000);
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::CardChanged { .. }]
        ));
        // The same departure with the Card still is plain consumption.
        let still = measure_step((1_000, 1_000), &[("Animal Oil Residue", C, 4_000, 40)], &[]);
        assert_eq!(still.other_consumption.len(), 1);
        assert!(still.unexplained_departures.is_empty());
    }

    #[test]
    fn arrivals_the_card_s_fall_pays_for_are_a_purchase_not_loot() {
        // A bought row of ammo, read as a new row.
        let measure = measure_step(
            (5_000, 4_000),
            &[("Universal Ammo", C, 50_000, 500)],
            &[
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(measure.gained.is_empty());
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::Purchase { .. }]
        ));
        assert_eq!(settled(&measure.ammo[0]), (0, 0, 0));
        // Ammo fired in the same minute does not hide the bought row.
        let fired = measure_step(
            (5_000, 4_000),
            &[("Universal Ammo", C, 50_000, 500)],
            &[
                ("Universal Ammo", C, 40_000, 400),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert!(matches!(
            fired.events.as_slice(),
            [NotableEvent::Purchase { .. }]
        ));
        assert_eq!(settled(&fired.ammo[0]), (10_000, 0, 0));
        // A fall that does not balance leaves the arrival gained, marked,
        // and the fall unexplained.
        let measure = measure_step(
            (5_000, 4_500),
            &[("Universal Ammo", C, 50_000, 500)],
            &[
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].mark, GainMark::CardMovedUnexplained);
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::CardChanged { .. }]
        ));
    }

    const OIL: (&str, Container, i64, i64) = ("Animal Oil Residue", C, 4_000, 40);

    /// A minute at the Trade Terminal: Opalo restored 7 PEC when `repair`,
    /// 4000 oil (40 PEC) gone when `sell`, a 100000 row of ammo (1000 PEC)
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
            after.push(("Universal Ammo", C, 100_000, 1_000));
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
                NotableEvent::CardChanged { .. } => "card",
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
    fn a_trade_terminal_minute_no_combination_explains_is_left_unexplained() {
        let measure = terminal(false, true, true, -500);
        assert_eq!(kinds(&measure), vec!["card"]);
        assert!(measure.other_consumption.is_empty());
        assert_eq!(measure.unexplained_departures.len(), 1);
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].mark, GainMark::CardMovedUnexplained);
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
    fn a_listing_is_a_move_and_its_fee_an_unexplained_card_fall() {
        let measure = measure_step(
            (1_000, 995),
            &[("Opalo", C, 1, 500)],
            &[("Opalo", Container::Auction, 1, 500)],
        );
        assert!(matches!(
            measure.events.as_slice(),
            [
                NotableEvent::Moved {
                    move_kind: MoveKind::Listing,
                    units: 1,
                    ..
                },
                NotableEvent::CardChanged { .. },
            ]
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
            &at(&[100.4, 103.0, 160.0, 220.0]),
            Some(1),
            &OneWeapon,
        )
        .unwrap();
        assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
        // An older reading close before the start is still the one used.
        let outcome = measure_session_with_baseline(
            &window,
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
        let outcome = measure_own_readings(&window, &at(&[100.0]), 0..1, None, &OneWeapon).unwrap();
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
        let outcome = measure_own_readings(&window, &readings, 1..5, None, &OneWeapon).unwrap();
        assert_eq!(outcome.used, 0..5);
        assert_eq!(outcome.measure.coverage.level, CoverageLevel::Complete);
        // Without a closing reading of its own, the reading after reads it.
        let outcome = measure_own_readings(&window, &readings, 1..4, None, &OneWeapon).unwrap();
        assert_eq!(outcome.used, 0..5);
        assert_eq!(outcome.measure.coverage.closing, Edge::Read);
        // A reading before that is too old is not used.
        let readings = at(&[5.0, 110.0, 170.0, 230.0, 320.0]);
        let outcome = measure_own_readings(&window, &readings, 1..5, None, &OneWeapon).unwrap();
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
        let late = measure_own_readings(&window, &readings, 0..2, Some(0), &OneWeapon).unwrap();
        assert_eq!(late.measure.coverage.opening, Edge::Unread);
        let window = SessionWindow {
            started_at: 100.0,
            ended_at: Some(200.0),
        };
        let readings = at(&[108.0, 168.0, 210.0]);
        let prompt = measure_own_readings(&window, &readings, 0..3, Some(0), &OneWeapon).unwrap();
        assert_eq!(prompt.measure.coverage.opening, Edge::Read);
        assert_eq!(prompt.measure.coverage.level, CoverageLevel::Complete);
    }

    #[test]
    fn time_running_backwards_between_two_readings_is_a_gap() {
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(400.0),
        };
        let readings = at(&[0.0, 100.0, 40.0, 140.0, 280.0, 400.0]);
        let outcome = measure_own_readings(&window, &readings, 0..6, None, &OneWeapon).unwrap();
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
}
