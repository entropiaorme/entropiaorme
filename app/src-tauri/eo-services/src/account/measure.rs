//! The measured account of one tracked session, from its readings.
//!
//! A pure function of the session's window, its readings in the order they
//! were stored, and the roles of what they hold. It stands beside the
//! session's tracked figures and never feeds them: it reads and writes no
//! accounting table.
//!
//! The readings it uses are those taken inside the session, with the
//! reading stored just before them when it was taken at or before the
//! session's start and at most 90 seconds before it, and the reading
//! stored just after them when it was taken at or after the session's end
//! and at most 90 seconds after it. The measure is complete only with both
//! of those, at least two readings, and no gap between consecutive
//! readings above 150 seconds; otherwise it is partial, with its gaps
//! counted. A gap is never filled with an estimate: what changed across it
//! is measured as one step, and the coverage says so.
//!
//! Nothing that arrives or leaves vanishes from the measure, and nothing
//! is booked as a cost or a gain on a guess:
//!
//! - departures the PED Card's rise pays for are a sale to the Trade
//!   Terminal, not consumption;
//! - Shrapnel that leaves without such a sale was turned into ammo, which
//!   is credited to it rather than counted as gained or burned;
//! - a stack leaving the auction with no matching move is a listing that
//!   sold;
//! - arrivals the Card's fall pays for (with no repair beside them) are a
//!   purchase, not loot;
//! - single items that arrive count in what was gained, marked as items;
//! - what a strongbox opening could not place stays in what was gained,
//!   marked as possibly its contents, with a total of its own;
//! - an opening that did not settle makes its ammo type's split into
//!   burned and paid back unsettled, as a type the caller must match.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::amount::{checked, Amount, Pec};
use super::composite::{recognise, Claim, Opening, Repair, Settlement};
use super::diff::{diff, exact_unit, MoveKind, Span, Step};
use super::reading::{Container, HoldingKey, Reading};
use super::roles::{Role, RoleBook, RoleSource, Stacking, SHRAPNEL, UNIVERSAL_AMMO};
use super::AccountError;

/// How far before its start a session's opening reading may be taken.
pub const OPENING_WITHIN_SECONDS: f64 = 90.0;
/// How far after its end a session's closing reading may be taken.
pub const CLOSING_WITHIN_SECONDS: f64 = 90.0;
/// The longest interval between consecutive readings that is not a gap.
pub const GAP_ABOVE_SECONDS: f64 = 150.0;

/// The Trade Terminal turns Shrapnel into Universal Ammo at 101% of its
/// TT: every 100 units of Shrapnel become 101 units of ammo, rounded down.
pub const SHRAPNEL_TO_AMMO_PERCENT: i64 = 101;

/// The version of the measure's shape and rules. A cached measure of
/// another version is recomputed from its readings.
pub const MEASURE_VERSION: i64 = 1;

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
    /// An opening in the window did not settle, so how much of the net
    /// change it paid back is not known. The units its rows suggest are
    /// given only as that reading of the rows, with no amount.
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

/// Whether something gained is certainly not a strongbox's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GainMark {
    /// Arrived, and no opening has a claim on it.
    Arrived,
    /// Arrived in a strongbox opening's step without being placed: it may
    /// be the opening's contents.
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
    /// Things sold to the Trade Terminal: the PED Card rose by what they
    /// held.
    Sale {
        between: Span,
        items: Vec<EventItem>,
        amount: Amount,
        card_rise: Pec,
    },
    /// Things bought at TT: the PED Card fell by what they hold.
    Purchase {
        between: Span,
        items: Vec<EventItem>,
        amount: Amount,
        card_fall: Pec,
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
/// measure uses.
fn select(window: &SessionWindow, readings: &[Reading]) -> (Range<usize>, Edge, Edge) {
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
    let opening = before.filter(|&index| {
        at(index) <= window.started_at && window.started_at - at(index) <= OPENING_WITHIN_SECONDS
    });
    let closing = window.ended_at.and_then(|ended| {
        after.filter(|&index| {
            index < n && at(index) >= ended && at(index) - ended <= CLOSING_WITHIN_SECONDS
        })
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
    let edge = |found: Option<usize>| {
        if found.is_some() {
            Edge::Read
        } else {
            Edge::Unread
        }
    };
    (first..last.max(first), edge(opening), edge(closing))
}

/// Measure a session over its readings, in the order they were stored.
pub fn measure_session(
    window: &SessionWindow,
    readings: &[Reading],
    roles: &dyn RoleSource,
) -> Result<MeasureOutcome, AccountError> {
    let (used, opening, closing) = select(window, readings);
    let span = &readings[used.clone()];
    let book = RoleBook::resolve(roles, span);
    let gap_count = span
        .windows(2)
        .filter(|pair| pair[1].taken_at() - pair[0].taken_at() > GAP_ABOVE_SECONDS)
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

/// Something that left in a step, before it is read as sold or consumed.
struct Departure {
    key: HoldingKey,
    units: i64,
    amount: Amount,
    rows: usize,
    single: bool,
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

#[derive(Default)]
struct Tally {
    ammo: BTreeMap<String, AmmoTally>,
    decay: BTreeMap<HoldingKey, (Role, Amount)>,
    repairs: Vec<TimedRepair>,
    consumed: BTreeMap<HoldingKey, (Role, i64, Amount)>,
    gained: BTreeMap<(HoldingKey, GainMark), (Role, Stacking, i64, Amount)>,
    events: Vec<NotableEvent>,
}

/// Ammo of one type's net change within one step.
struct AmmoStep {
    key: HoldingKey,
    units: i64,
    amount: Amount,
}

impl Tally {
    fn add_step(
        &mut self,
        before: &Reading,
        after: &Reading,
        book: &RoleBook,
    ) -> Result<(), AccountError> {
        let set = diff(before, after, &|name| book.stacking(name))?;
        let composites = recognise(&set, before, after)?;
        let between = set.span;
        let opening = composites.opening.as_ref();
        let is_opening_input = |key: &HoldingKey| {
            opening.is_some_and(|opening| {
                key.name == crate::inventory_changes::STRONGBOX_KEY || key.name == opening.box_name
            })
        };
        let restorations = set
            .steps
            .iter()
            .any(|step| matches!(step, Step::Repaired { .. }));
        let mut card: Option<(Pec, Pec)> = None;
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
                } => self.events.push(NotableEvent::Moved {
                    between,
                    name: name.clone(),
                    units: *units,
                    amount: *amount,
                    move_kind: MoveKind::of(from, to),
                    from: from.clone(),
                    to: to.clone(),
                }),
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
                    key, units, amount, ..
                } if book.role(&key.name) == Role::Ammo => {
                    let entry = ammo_steps.entry(key.name.clone()).or_insert(AmmoStep {
                        key: key.clone(),
                        units: 0,
                        amount: Amount::NONE,
                    });
                    entry.units = checked(entry.units.checked_sub(*units))?;
                    entry.amount = entry.amount.minus(*amount)?;
                }
                Step::Gained {
                    key, units, amount, ..
                } if book.role(&key.name) == Role::Ammo => {
                    let entry = ammo_steps.entry(key.name.clone()).or_insert(AmmoStep {
                        key: key.clone(),
                        units: 0,
                        amount: Amount::NONE,
                    });
                    entry.units = checked(entry.units.checked_add(*units))?;
                    entry.amount = entry.amount.plus(*amount)?;
                }
                Step::Consumed { key, .. } if is_opening_input(key) => {}
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
                    single: false,
                }),
                Step::Left { key, units, value } => departures.push(Departure {
                    key: key.clone(),
                    units: *units,
                    amount: *value,
                    rows: 1,
                    single: true,
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

        let mut card_explained = composites
            .repair
            .as_ref()
            .is_some_and(|repair| repair.cost.is_some());

        // A sale to the Trade Terminal: the Card rose by what left.
        if let Some((from, to)) = card.filter(|(from, to)| to > from) {
            let rise = to.minus(from)?;
            if !departures.is_empty() {
                let total = Amount::total(departures.iter().map(|d| d.amount))?;
                let rows = departures.iter().map(|d| d.rows).sum();
                if total.agrees_with(Amount::whole_pec(rise)?, rows)? {
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
                        amount: total,
                        card_rise: rise,
                    });
                    card_explained = true;
                }
            }
        }

        // Shrapnel that left without a sale was turned into ammo.
        let mut converted_units = 0i64;
        let mut converted = Amount::NONE;
        let ammo_unit = exact_unit(
            before
                .holdings()
                .iter()
                .chain(after.holdings())
                .filter(|(key, _)| key.name == UNIVERSAL_AMMO)
                .flat_map(|(_, holding)| holding.rows()),
        );
        let mut kept = Vec::with_capacity(departures.len());
        for departure in departures {
            if departure.key.name != SHRAPNEL || departure.single {
                kept.push(departure);
                continue;
            }
            let ammo_units = checked(
                departure
                    .units
                    .checked_mul(SHRAPNEL_TO_AMMO_PERCENT)
                    .map(|scaled| scaled / 100),
            )?;
            let ammo = match ammo_unit {
                Some(unit) => Amount::exact_units(ammo_units, unit)?,
                None => Amount::WholePec {
                    micro_ped: checked(
                        departure
                            .amount
                            .micro_ped()
                            .checked_mul(SHRAPNEL_TO_AMMO_PERCENT)
                            .map(|scaled| scaled / 100),
                    )?,
                },
            };
            converted_units = checked(converted_units.checked_add(ammo_units))?;
            converted = converted.plus(ammo)?;
            self.events.push(NotableEvent::ShrapnelConverted {
                between,
                shrapnel_units: departure.units,
                shrapnel: departure.amount,
                ammo_units,
                ammo,
            });
        }
        // What is left of the departures: stacks consumed, items gone.
        for departure in kept {
            if departure.single {
                self.events.push(NotableEvent::Left {
                    between,
                    key: departure.key,
                    units: departure.units,
                    value: departure.amount,
                });
            } else {
                let entry = self.consumed.entry(departure.key.clone()).or_insert((
                    book.role(&departure.key.name),
                    0,
                    Amount::NONE,
                ));
                entry.1 = checked(entry.1.checked_add(departure.units))?;
                entry.2 = entry.2.plus(departure.amount)?;
            }
        }

        // Ammo: what openings and Shrapnel paid back is credited, the rest
        // of a fall was burned, and a rise beyond the credits arrived.
        if converted_units != 0 && !ammo_steps.contains_key(UNIVERSAL_AMMO) {
            ammo_steps.insert(
                UNIVERSAL_AMMO.to_string(),
                AmmoStep {
                    key: HoldingKey {
                        name: UNIVERSAL_AMMO.to_string(),
                        container: Container::Carried,
                    },
                    units: 0,
                    amount: Amount::NONE,
                },
            );
        }
        for (name, step) in ammo_steps {
            let tally = self.ammo.entry(name.clone()).or_default();
            tally.net_units = checked(tally.net_units.checked_add(step.units))?;
            tally.net = tally.net.plus(step.amount)?;
            let (mut credited_units, mut credited) = (0i64, Amount::NONE);
            if name == UNIVERSAL_AMMO {
                if let Some(opening) = opening {
                    tally.strongbox_units =
                        checked(tally.strongbox_units.checked_add(opening.ammo_units))?;
                    tally.strongbox = tally.strongbox.plus(opening.ammo_from_rows)?;
                    if opening.settlement == Settlement::Unsettled {
                        tally.unsettled = true;
                    }
                    credited_units = opening.ammo_units;
                    credited = opening.ammo_from_rows;
                }
                tally.converted_units =
                    checked(tally.converted_units.checked_add(converted_units))?;
                tally.converted = tally.converted.plus(converted)?;
                credited_units = checked(credited_units.checked_add(converted_units))?;
                credited = credited.plus(converted)?;
            }
            let burned_units = checked(credited_units.checked_sub(step.units))?;
            let burned = credited.minus(step.amount)?;
            if burned_units > 0 {
                tally.burned_units = checked(tally.burned_units.checked_add(burned_units))?;
                tally.burned = tally.burned.plus(burned)?;
            } else if burned_units < 0 {
                // Beside an opening that did not settle, ammo beyond its
                // rows' reading may still be its payout.
                let unsettled_opening = name == UNIVERSAL_AMMO
                    && opening.is_some_and(|opening| opening.settlement == Settlement::Unsettled);
                arrivals.push(Arrival {
                    key: step.key,
                    units: checked(burned_units.checked_neg())?,
                    amount: burned.negated()?,
                    rows: 1,
                    stacking: Stacking::Stack,
                    mark: if unsettled_opening {
                        GainMark::PossiblyStrongbox
                    } else {
                        GainMark::Arrived
                    },
                    appeared: false,
                });
            }
        }

        // A purchase at TT: the Card fell by what arrived, with no repair.
        if let Some((from, to)) = card.filter(|(from, to)| to < from) {
            let fall = from.minus(to)?;
            if !restorations && !arrivals.is_empty() {
                let total = Amount::total(arrivals.iter().map(|a| a.amount))?;
                let rows = arrivals.iter().map(|a| a.rows).sum();
                if total.agrees_with(Amount::whole_pec(fall)?, rows)? {
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
                        amount: total,
                        card_fall: fall,
                    });
                    card_explained = true;
                }
            }
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

        if let Some((from, to)) = card {
            if !card_explained {
                self.events
                    .push(NotableEvent::CardChanged { between, from, to });
            }
        }
        if let Some(opening) = composites.opening {
            self.events
                .push(NotableEvent::StrongboxOpening { between, opening });
        }
        if let Some(repair) = composites.repair {
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
            other_consumption: self
                .consumed
                .into_iter()
                .map(|(key, (role, units, amount))| ItemUnits {
                    key,
                    role,
                    units,
                    amount,
                })
                .collect(),
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
    fn ammo_that_arrived_beyond_any_credit_is_gained_not_burned() {
        let readings = [
            reading(0.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
            reading(60.0, 0, &[("Universal Ammo", C, 500_000, 5_000)]),
            reading(120.0, 0, &[("Universal Ammo", C, 400_000, 4_000)]),
        ];
        let window = SessionWindow {
            started_at: 0.0,
            ended_at: Some(120.0),
        };
        let measure = measure_session(&window, &readings, &OneWeapon)
            .unwrap()
            .measure;
        assert_eq!(measure.ammo.len(), 1);
        assert_eq!(measure.ammo[0].net_units, 300_000);
        assert_eq!(settled(&measure.ammo[0]), (100_000, 0, 0));
        assert_eq!(measure.gained.len(), 1);
        assert_eq!(measure.gained[0].role, Role::Ammo);
        assert_eq!(measure.gained[0].stacking, Stacking::Stack);
        assert_eq!(measure.gained[0].units, 400_000);
        assert_eq!(measure.gained[0].amount, Amount::exact(40_000_000));
        assert_eq!(measure.gained_total, Amount::exact(40_000_000));
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
        let [NotableEvent::Sale {
            items,
            amount,
            card_rise,
            ..
        }] = measure.events.as_slice()
        else {
            panic!("one sale: {:?}", measure.events);
        };
        assert_eq!(items.len(), 2);
        assert_eq!(amount.micro_ped(), 5_400_000);
        assert_eq!(*card_rise, Pec(540));
    }

    #[test]
    fn a_rise_that_does_not_balance_leaves_the_departures_as_they_were() {
        let measure = measure_step((1_000, 1_100), &[("Animal Oil Residue", C, 4_000, 40)], &[]);
        assert_eq!(measure.other_consumption.len(), 1);
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::CardChanged { .. }]
        ));
    }

    #[test]
    fn arrivals_the_card_s_fall_pays_for_are_a_purchase_not_loot() {
        let measure = measure_step(
            (5_000, 4_000),
            &[("Universal Ammo", C, 50_000, 500)],
            &[("Universal Ammo", C, 150_000, 1_500)],
        );
        assert!(measure.gained.is_empty());
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::Purchase {
                card_fall: Pec(1_000),
                ..
            }]
        ));
        // A fall that does not balance leaves the arrival gained and the
        // fall unexplained.
        let measure = measure_step(
            (5_000, 4_500),
            &[("Universal Ammo", C, 50_000, 500)],
            &[("Universal Ammo", C, 150_000, 1_500)],
        );
        assert_eq!(measure.gained.len(), 1);
        assert!(matches!(
            measure.events.as_slice(),
            [NotableEvent::CardChanged { .. }]
        ));
    }

    #[test]
    fn a_step_with_a_repair_is_never_read_as_a_purchase() {
        let measure = measure_step(
            (5_000, 3_993),
            &[("Opalo", C, 1, 493), ("Shrapnel", C, 10, 0)],
            &[("Opalo", C, 1, 500), ("Shrapnel", C, 100_010, 1_000)],
        );
        assert_eq!(measure.gained.len(), 1, "the arrival stays gained");
        assert_eq!(measure.repairs.len(), 1);
        assert!(measure
            .events
            .iter()
            .all(|event| !matches!(event, NotableEvent::Purchase { .. })));
    }

    #[test]
    fn shrapnel_that_leaves_without_a_sale_was_turned_into_ammo() {
        let measure = measure_step(
            (1_000, 1_000),
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
            &[("Universal Ammo", C, 105_100, 1_051)],
        );
        assert!(measure.other_consumption.is_empty(), "not consumption");
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
        assert_eq!(measure.repairs[0].repair.cost, Some(Pec(7)));
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
    fn loot_beside_an_unsettled_opening_stays_gained_marked_and_the_split_is_unsettled() {
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
    fn loot_of_no_tt_beside_an_opening_alone_is_its_contents() {
        let measure = opening_with(&[("Halloween Fireworks", C, 1, 0)], 7);
        assert!(measure.gained.is_empty());
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
    fn fewer_than_two_readings_are_partial_whatever_the_window() {
        for window in [
            SessionWindow {
                started_at: 100.0,
                ended_at: Some(100.0),
            },
            SessionWindow {
                started_at: 100.0,
                ended_at: Some(40.0),
            },
        ] {
            let outcome = measure_session(&window, &at(&[100.0]), &OneWeapon).unwrap();
            assert_eq!(outcome.measure.coverage.reading_count, 1);
            assert_eq!(outcome.measure.coverage.level, CoverageLevel::Partial);
        }
        let outcome = measure_session(&WHOLE, &[], &OneWeapon).unwrap();
        assert_eq!(outcome.used, 0..0);
        assert_eq!(outcome.measure.coverage.level, CoverageLevel::Partial);
        assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
        assert_eq!(outcome.measure.coverage.closing, Edge::Unread);
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
