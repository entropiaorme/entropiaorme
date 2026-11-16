//! Patterns over one step's changes: a repair and a strongbox opening.
//!
//! A repair is the restorations of one step. What it cost is read with the
//! rest of the PED Card's movement in that step (see the measure): this
//! module only gathers what it restored.
//!
//! A strongbox opening is `Strongbox Key` down by some count and exactly
//! one zero-TT strongbox down by the same count. A key is a deposit of its
//! TT and the box pays the TT back, mostly as Universal Ammo, each box's
//! ammo arriving as a row of its own. Keys and boxes leaving in any other
//! shape (two kinds of box, counts that differ, boxes with no key) are
//! still an opening's, reported unsettled with what left.
//!
//! The opening claims only what its balance claims. Every way of closing
//! the keys' balance is weighed: a reading of which ammo rows are new,
//! with a subset of the step's other arrivals as contents, that pays the
//! keys back within one PEC per output row. The ammo is settled when every
//! closing way agrees on the same reading of the rows; an arrival is
//! certainly the opening's when every closing way takes it, possibly the
//! opening's when some do, and not the opening's when none does. With no
//! closing way, or more ways than [`MAX_BALANCE_EVALUATIONS`] allows to
//! weigh, the opening is unsettled, claims nothing, and every arrival of
//! its step is marked as possibly its contents: nothing is forced to
//! balance, and nothing that arrived is dropped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::amount::{checked, count, Amount, Pec, MICRO_PER_PEC};
use super::diff::{canonical_new_rows, exact_unit, new_row_readings, NewRows, Step, StepSet};
use super::reading::{Holding, HoldingKey, Reading, Row};
use super::roles::{is_strongbox, STRONGBOX_KEY, UNIVERSAL_AMMO};
use super::AccountError;

/// The most ways of closing an opening's balance (readings of its new
/// ammo rows times subsets of its step's arrivals) weighed for one step;
/// beyond it the opening is unsettled.
pub const MAX_BALANCE_EVALUATIONS: usize = 100_000;

/// One item's share of a figure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemAmount {
    pub key: HoldingKey,
    pub amount: Amount,
}

/// What a repair cost, as far as the PED Card shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RepairCost {
    /// The Card's fall, with nothing else in the step moving it, within
    /// what the restorations allow: what the repair cost.
    Paid { pec: Pec },
    /// What remains of the Card's change once the step's sale and
    /// purchase are taken out. It lies within what the restorations allow,
    /// but rests on those figures too.
    Derived { amount: Amount },
    /// The Card's movement cannot be read as the repair's cost.
    Unknown,
}

/// The restorations of one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repair {
    pub items: Vec<ItemAmount>,
    /// The sum of the items' whole-PEC rises.
    pub restored: Amount,
    pub cost: RepairCost,
}

impl Repair {
    /// The least the restorations could have cost: one PEC per item below
    /// the restored total, as the site truncates each value.
    pub fn least_cost(&self) -> Result<Amount, AccountError> {
        let slack = checked(count(self.items.len())?.checked_mul(MICRO_PER_PEC))?;
        self.restored.minus(Amount::WholePec { micro_ped: slack })
    }
}

/// Whether an opening's ammo was read on one reading of its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Settlement {
    /// Every way of closing the keys' balance reads the same rows as new.
    Settled,
    /// No way closes it, the ways disagree on the rows, the keys and boxes
    /// left in no opening's shape, or there were too many ways to weigh.
    Unsettled,
}

/// One thing a strongbox opening produced besides its ammo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedContent {
    pub key: HoldingKey,
    pub units: i64,
    pub amount: Amount,
}

/// Boxes of one kind that left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedBoxes {
    pub name: String,
    pub count: i64,
}

/// A run of strongbox openings within one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opening {
    /// The boxes that left, one entry for each kind.
    pub boxes: Vec<OpenedBoxes>,
    pub keys_used: i64,
    /// The TT of the keys used: the deposit the boxes pay back.
    pub keys: Amount,
    /// Universal Ammo in rows new in the step: the reading every closing
    /// way agrees on when settled, the canonical reading of the rows
    /// otherwise.
    pub ammo_units: i64,
    pub ammo_rows: usize,
    pub ammo_from_rows: Amount,
    /// The keys' TT less the TT of the certain contents.
    pub ammo_from_keys: Amount,
    /// What every way of closing the balance takes besides ammo.
    pub contents: Vec<OpenedContent>,
    /// What some ways take and others do not: possibly the opening's.
    pub possible_contents: Vec<OpenedContent>,
    pub settlement: Settlement,
}

/// What an opening says of one arrival in its step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Part of the opening's contents.
    Content,
    /// Possibly the opening's contents, possibly not.
    Possibly,
}

/// The patterns one step holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Composites {
    pub repair: Option<Repair>,
    pub opening: Option<Opening>,
    /// What the opening says of arrivals, by their position in the step.
    /// An arrival not listed is not the opening's.
    pub claims: BTreeMap<usize, Claim>,
}

/// Rows of a holding claimed before the opening reads its new rows (a
/// Shrapnel conversion's ammo).
pub type Reserved = [(HoldingKey, Row)];

/// Recognise the repair and the strongbox opening in one step from
/// `before` to `after`. `reserved` rows are another pattern's and never
/// read as an opening's payout.
pub fn recognise(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    reserved: &Reserved,
) -> Result<Composites, AccountError> {
    let mut composites = Composites {
        repair: repair(set)?,
        ..Composites::default()
    };
    if let Some((opening, claims)) = opening(set, before, after, reserved)? {
        composites.opening = Some(opening);
        composites.claims = claims;
    }
    Ok(composites)
}

fn repair(set: &StepSet) -> Result<Option<Repair>, AccountError> {
    let items: Vec<ItemAmount> = set
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Repaired { key, by } => Some(ItemAmount {
                key: key.clone(),
                amount: *by,
            }),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return Ok(None);
    }
    let restored = Amount::total(items.iter().map(|item| item.amount))?;
    Ok(Some(Repair {
        items,
        restored,
        cost: RepairCost::Unknown,
    }))
}

/// One arrival an opening may claim.
struct Arrival {
    index: usize,
    key: HoldingKey,
    units: i64,
    amount: Amount,
    rows: usize,
}

/// One reading of the step's new ammo rows.
#[derive(Clone, PartialEq)]
struct AmmoReading {
    units: i64,
    rows: usize,
    amount: Amount,
}

fn rows_of(holding: Option<&Holding>) -> &[Row] {
    holding.map(Holding::rows).unwrap_or_default()
}

/// A holding's after-rows less the rows another pattern claimed.
fn unreserved(key: &HoldingKey, rows: &[Row], reserved: &Reserved) -> Vec<Row> {
    let mut rows = rows.to_vec();
    for (held, row) in reserved {
        if held == key {
            if let Some(index) = rows.iter().position(|kept| kept == row) {
                rows.remove(index);
            }
        }
    }
    rows
}

/// The Universal Ammo holdings that changed in the step.
fn ammo_keys(set: &StepSet) -> Vec<&HoldingKey> {
    set.steps
        .iter()
        .filter_map(|step| match step {
            Step::Consumed { key, .. } | Step::Gained { key, .. } if key.name == UNIVERSAL_AMMO => {
                Some(key)
            }
            _ => None,
        })
        .collect()
}

/// Every reading of the Universal Ammo rows new in the step, across the
/// holdings that changed; `None` when there are too many to weigh.
fn ammo_readings(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    reserved: &Reserved,
) -> Result<Option<Vec<AmmoReading>>, AccountError> {
    let mut combined = vec![AmmoReading {
        units: 0,
        rows: 0,
        amount: Amount::NONE,
    }];
    for key in ammo_keys(set) {
        let was = rows_of(before.holding(key));
        let now = unreserved(key, rows_of(after.holding(key)), reserved);
        let unit = exact_unit(was.iter().chain(&now));
        let Some(readings) = new_row_readings(was, &now) else {
            return Ok(None);
        };
        let mut next = Vec::new();
        for held in &combined {
            for rows in &readings {
                let new_rows = NewRows::of(rows.clone(), unit)?;
                let reading = AmmoReading {
                    units: checked(held.units.checked_add(new_rows.units))?,
                    rows: held.rows + new_rows.rows.len(),
                    amount: held.amount.plus(new_rows.amount)?,
                };
                if !next.contains(&reading) {
                    next.push(reading);
                }
            }
        }
        if next.len() > super::diff::MAX_NEW_ROW_READINGS {
            return Ok(None);
        }
        combined = next;
    }
    Ok(Some(combined))
}

/// The canonical reading of the step's new ammo rows.
fn canonical_ammo(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    reserved: &Reserved,
) -> Result<AmmoReading, AccountError> {
    let mut reading = AmmoReading {
        units: 0,
        rows: 0,
        amount: Amount::NONE,
    };
    for key in ammo_keys(set) {
        let was = rows_of(before.holding(key));
        let now = unreserved(key, rows_of(after.holding(key)), reserved);
        let unit = exact_unit(was.iter().chain(&now));
        let new_rows = NewRows::of(canonical_new_rows(was, &now), unit)?;
        reading.units = checked(reading.units.checked_add(new_rows.units))?;
        reading.rows += new_rows.rows.len();
        reading.amount = reading.amount.plus(new_rows.amount)?;
    }
    Ok(reading)
}

/// One row of Universal Ammo new in a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewAmmoRow {
    pub units: i64,
    pub amount: Amount,
}

/// The Universal Ammo rows new in the step as the canonical reading reads
/// them, each with what it holds; `reserved` rows are another pattern's and
/// left out. When an opening settled on the canonical reading these are its
/// payout rows, one for each box that paid ammo.
pub fn new_ammo_rows(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    reserved: &Reserved,
) -> Result<Vec<NewAmmoRow>, AccountError> {
    let mut found = Vec::new();
    for key in ammo_keys(set) {
        let was = rows_of(before.holding(key));
        let now = unreserved(key, rows_of(after.holding(key)), reserved);
        let unit = exact_unit(was.iter().chain(&now));
        for row in canonical_new_rows(was, &now) {
            let read = NewRows::of(vec![row], unit)?;
            found.push(NewAmmoRow {
                units: read.units,
                amount: read.amount,
            });
        }
    }
    Ok(found)
}

type Claims = BTreeMap<usize, Claim>;

/// What weighing every way of closing the balance found.
struct Weighed {
    /// The readings of the ammo that some closing way uses.
    closing_readings: Vec<usize>,
    /// The arrivals every closing way takes.
    in_every: u64,
    /// The arrivals some closing way takes.
    in_some: u64,
}

/// Weigh every reading of the ammo with every subset of the arrivals;
/// `None` when that is more than [`MAX_BALANCE_EVALUATIONS`] ways.
fn weigh(
    keys: Amount,
    readings: &[AmmoReading],
    arrivals: &[Arrival],
) -> Result<Option<Weighed>, AccountError> {
    let n = arrivals.len();
    let subsets = if n < 32 { 1usize << n } else { usize::MAX };
    if readings.len().saturating_mul(subsets) > MAX_BALANCE_EVALUATIONS {
        return Ok(None);
    }
    let target = keys.micro_ped();
    let mut weighed = Weighed {
        closing_readings: Vec::new(),
        in_every: u64::MAX,
        in_some: 0,
    };
    for (index, reading) in readings.iter().enumerate() {
        for mask in 0..subsets {
            let mut produced = reading.amount.micro_ped();
            let mut rows = reading.rows;
            for (bit, arrival) in arrivals.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    produced = checked(produced.checked_add(arrival.amount.micro_ped()))?;
                    rows += arrival.rows;
                }
            }
            let tolerance = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
            let gap = checked(produced.checked_sub(target).and_then(i64::checked_abs))?;
            if gap <= tolerance {
                if !weighed.closing_readings.contains(&index) {
                    weighed.closing_readings.push(index);
                }
                weighed.in_every &= mask as u64;
                weighed.in_some |= mask as u64;
            }
        }
    }
    Ok(Some(weighed))
}

fn content(arrival: &Arrival) -> OpenedContent {
    OpenedContent {
        key: arrival.key.clone(),
        units: arrival.units,
        amount: arrival.amount,
    }
}

fn opening(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    reserved: &Reserved,
) -> Result<Option<(Opening, Claims)>, AccountError> {
    let mut keys_used: i64 = 0;
    let mut keys = Amount::NONE;
    let mut boxes: Vec<OpenedBoxes> = Vec::new();
    for step in &set.steps {
        if let Step::Consumed {
            key, units, amount, ..
        } = step
        {
            if key.name == STRONGBOX_KEY {
                keys_used = checked(keys_used.checked_add(*units))?;
                keys = keys.plus(*amount)?;
            } else if is_strongbox(&key.name) && amount.micro_ped() == 0 {
                match boxes.iter_mut().find(|held| held.name == key.name) {
                    Some(held) => held.count = checked(held.count.checked_add(*units))?,
                    None => boxes.push(OpenedBoxes {
                        name: key.name.clone(),
                        count: *units,
                    }),
                }
            }
        }
    }
    // Keys with no box leaving are not an opening (sold or traded keys
    // are read with the rest of the step's departures).
    if boxes.is_empty() {
        return Ok(None);
    }
    let clean = keys_used > 0 && matches!(boxes.as_slice(), [only] if only.count == keys_used);

    let arrivals: Vec<Arrival> = set
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| match step {
            Step::Gained {
                key,
                units,
                amount,
                rows_touched,
                ..
            } if key.name != UNIVERSAL_AMMO
                && key.name != STRONGBOX_KEY
                && !is_strongbox(&key.name) =>
            {
                Some(Arrival {
                    index,
                    key: key.clone(),
                    units: *units,
                    amount: *amount,
                    rows: *rows_touched,
                })
            }
            Step::Appeared { key, units, value } => Some(Arrival {
                index,
                key: key.clone(),
                units: *units,
                amount: *value,
                rows: 1,
            }),
            _ => None,
        })
        .collect();

    let readings = if clean {
        ammo_readings(set, before, after, reserved)?
    } else {
        None
    };
    let weighed = match &readings {
        Some(readings) => weigh(keys, readings, &arrivals)?,
        None => None,
    };

    let mut claims = BTreeMap::new();
    let mut contents = Vec::new();
    let mut possible_contents = Vec::new();
    let (ammo, settlement) = match (weighed, readings) {
        (Some(weighed), Some(readings)) if !weighed.closing_readings.is_empty() => {
            for (bit, arrival) in arrivals.iter().enumerate() {
                if weighed.in_every & (1 << bit) != 0 {
                    claims.insert(arrival.index, Claim::Content);
                    contents.push(content(arrival));
                } else if weighed.in_some & (1 << bit) != 0 {
                    claims.insert(arrival.index, Claim::Possibly);
                    possible_contents.push(content(arrival));
                }
            }
            match weighed.closing_readings.as_slice() {
                [only] => (readings[*only].clone(), Settlement::Settled),
                _ => (
                    canonical_ammo(set, before, after, reserved)?,
                    Settlement::Unsettled,
                ),
            }
        }
        _ => {
            for arrival in &arrivals {
                claims.insert(arrival.index, Claim::Possibly);
                possible_contents.push(content(arrival));
            }
            (
                canonical_ammo(set, before, after, reserved)?,
                Settlement::Unsettled,
            )
        }
    };
    let ammo_from_keys = keys.minus(Amount::total(
        contents.iter().map(|content| content.amount),
    )?)?;
    let opening = Opening {
        boxes,
        keys_used,
        keys,
        ammo_units: ammo.units,
        ammo_rows: ammo.rows,
        ammo_from_rows: ammo.amount,
        ammo_from_keys,
        contents,
        possible_contents,
        settlement,
    };
    Ok(Some((opening, claims)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::diff::{diff, Span};
    use crate::account::reading::Container;
    use crate::account::roles::Stacking;
    use crate::account::test_support::reading;

    fn stacking(name: &str) -> Stacking {
        if name.ends_with("(M)") || name.ends_with("(F)") || name.contains("Token") {
            Stacking::Single
        } else {
            Stacking::Stack
        }
    }

    fn pec(value: i64) -> Amount {
        Amount::whole_pec(Pec(value)).unwrap()
    }

    fn recognised(before: &Reading, after: &Reading) -> Composites {
        recognise(&diff(before, after, &stacking).unwrap(), before, after, &[]).unwrap()
    }

    const C: Container = Container::Carried;

    type Rows<'a> = &'a [(&'static str, Container, i64, i64)];

    #[test]
    fn restorations_are_gathered_with_their_least_cost() {
        let repair = recognised(
            &reading(
                0.0,
                25_000,
                &[
                    ("Arm Guards (M)", C, 1, 2_143),
                    ("Thigh Guards (M)", C, 1, 2_145),
                ],
            ),
            &reading(
                60.0,
                24_988,
                &[
                    ("Arm Guards (M)", C, 1, 2_150),
                    ("Thigh Guards (M)", C, 1, 2_150),
                ],
            ),
        )
        .repair
        .expect("a repair");
        assert_eq!(repair.items.len(), 2);
        assert_eq!(repair.restored, pec(12));
        assert_eq!(
            repair.least_cost().unwrap().micro_ped(),
            pec(10).micro_ped()
        );
        assert_eq!(
            repair.cost,
            RepairCost::Unknown,
            "the measure reads the cost"
        );
        assert_eq!(
            recognised(&reading(0.0, 25_000, &[]), &reading(60.0, 24_000, &[])).repair,
            None
        );
    }

    /// Seven boxes opened from 23, with extra rows after, and the ammo
    /// rows given new in the step.
    fn opening_of(ammo_rows: &[(i64, i64)], extra: Rows) -> Composites {
        let before = reading(
            0.0,
            25_000,
            &[
                ("Halloween Strongbox", C, 23, 0),
                ("Strongbox Key", C, 24, 24_000),
                ("Universal Ammo", C, 443_923, 4_439),
            ],
        );
        let mut after = vec![
            ("Halloween Strongbox", C, 16, 0),
            ("Strongbox Key", C, 17, 17_000),
            ("Universal Ammo", C, 443_923, 4_439),
        ];
        for (quantity, value) in ammo_rows {
            after.push(("Universal Ammo", C, *quantity, *value));
        }
        after.extend_from_slice(extra);
        recognised(&before, &reading(60.0, 25_000, &after))
    }

    const PANTS: (&str, Container, i64, i64) = ("Skeleton Pants Red (F)", C, 1, 1_000);

    #[test]
    fn keys_and_one_box_down_together_are_an_opening_read_from_its_new_ammo_rows() {
        let found = opening_of(&[(100_000, 1_000); 6], &[PANTS]);
        let opening = found.opening.expect("an opening");
        assert_eq!(
            opening.boxes,
            vec![OpenedBoxes {
                name: "Halloween Strongbox".into(),
                count: 7
            }]
        );
        assert_eq!(opening.keys_used, 7);
        assert_eq!(opening.keys, pec(7_000));
        assert_eq!(opening.ammo_units, 600_000);
        assert_eq!(opening.ammo_rows, 6);
        assert_eq!(opening.ammo_from_rows, Amount::exact(60_000_000));
        assert_eq!(opening.ammo_from_keys.micro_ped(), 60_000_000);
        assert_eq!(opening.contents.len(), 1);
        assert_eq!(opening.contents[0].amount, pec(1_000));
        assert!(opening.possible_contents.is_empty());
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(
            found.claims.values().collect::<Vec<_>>(),
            vec![&Claim::Content]
        );
    }

    #[test]
    fn a_shortfall_within_a_pec_per_output_row_settles_and_a_wider_one_does_not() {
        let mut rows = vec![(100_000, 1_000); 5];
        rows.push((99_300, 993));
        assert_eq!(
            opening_of(&rows, &[PANTS]).opening.unwrap().settlement,
            Settlement::Settled
        );
        rows[5] = (99_200, 992);
        let found = opening_of(&rows, &[PANTS]);
        let opening = found.opening.expect("still an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
        assert!(
            opening.contents.is_empty(),
            "an unsettled opening claims nothing for certain"
        );
        assert_eq!(opening.ammo_from_rows, Amount::exact(59_920_000));
        assert_eq!(
            found.claims.values().collect::<Vec<_>>(),
            vec![&Claim::Possibly]
        );
    }

    #[test]
    fn loot_in_the_opening_s_step_stays_loot_when_no_closing_way_takes_it() {
        let shrapnel = ("Shrapnel", C, 281_816, 2_818);
        let token = ("Mayhem Token", C, 1, 0);
        let found = opening_of(&[(100_000, 1_000); 6], &[PANTS, shrapnel, token]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.contents.len(), 1);
        assert_eq!(opening.contents[0].key.name, "Skeleton Pants Red (F)");
        assert_eq!(
            opening.possible_contents.len(),
            1,
            "the token, with or without it"
        );
        let claims: Vec<Claim> = found.claims.values().copied().collect();
        assert_eq!(claims.len(), 2, "the shrapnel is not the opening's at all");
        assert!(claims.contains(&Claim::Content));
        assert!(claims.contains(&Claim::Possibly));
    }

    #[test]
    fn an_arrival_of_no_tt_beside_an_opening_is_only_possibly_its() {
        let token = ("Mayhem Token", C, 1, 0);
        let found = opening_of(&[(100_000, 1_000); 7], &[token]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert!(opening.contents.is_empty());
        assert_eq!(opening.possible_contents.len(), 1);
    }

    #[test]
    fn two_subsets_closing_on_one_reading_settle_the_ammo_and_claim_neither_for_certain() {
        let shirt = ("Skeleton Shirt (F)", C, 1, 1_000);
        let shrapnel = ("Shrapnel", C, 100_000, 1_000);
        let found = opening_of(&[(100_000, 1_000); 6], &[shirt, shrapnel]);
        let opening = found.opening.expect("an opening");
        assert_eq!(
            opening.settlement,
            Settlement::Settled,
            "both ways read the same rows"
        );
        assert_eq!(opening.ammo_units, 600_000);
        assert!(opening.contents.is_empty());
        assert_eq!(opening.possible_contents.len(), 2);
        assert!(found.claims.values().all(|claim| *claim == Claim::Possibly));
    }

    #[test]
    fn every_arrival_and_a_strict_subset_both_closing_leave_the_rest_possibly() {
        // Seven keys: the trousers and a 0.05 PED stack both close within
        // the eight PEC eight output rows allow, with or without the stack.
        let small = ("Animal Oil Residue", C, 5, 5);
        let found = opening_of(&[(100_000, 1_000); 6], &[PANTS, small]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.contents.len(), 1);
        assert_eq!(opening.contents[0].key.name, "Skeleton Pants Red (F)");
        assert_eq!(opening.possible_contents.len(), 1);
        assert_eq!(opening.possible_contents[0].key.name, "Animal Oil Residue");
    }

    #[test]
    fn readings_that_disagree_between_closing_ways_unsettle_the_ammo() {
        // One key: the new row is the 95100 or a fresh 100000 beside a
        // fired one; a 4.90 PED arrival closes the first reading, nothing
        // closes the second.
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 95_100, 951),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Skeleton Shirt (F)", C, 1, 49),
            ],
        );
        let opening = recognised(&before, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
        assert_eq!(opening.possible_contents.len(), 1);
    }

    #[test]
    fn a_vanished_small_row_does_not_pair_with_a_large_new_one() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 5_000, 50),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let opening = recognised(&before, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.ammo_units, 100_000);
    }

    #[test]
    fn when_the_rows_allow_two_readings_the_keys_decide() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 95_100, 951),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let opening = recognised(&before, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(
            opening.ammo_units, 100_000,
            "not the 95100 the identical rows suggest"
        );
        let two_keys = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 3, 0),
                ("Strongbox Key", C, 3, 3_000),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let opening = recognised(&two_keys, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
    }

    #[test]
    fn a_single_box_opened_with_one_key_is_an_opening() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        );
        let opening = recognised(&before, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.keys_used, 1);
        assert_eq!(opening.ammo_units, 100_000);
    }

    #[test]
    fn keys_and_boxes_leaving_in_no_opening_s_shape_are_an_unsettled_opening() {
        let shapes: [(Rows, Rows); 3] = [
            // Two kinds of box.
            (
                &[
                    ("Halloween Strongbox", C, 23, 0),
                    ("Easter Strongbox", C, 5, 0),
                    ("Strongbox Key", C, 24, 24_000),
                ],
                &[
                    ("Halloween Strongbox", C, 22, 0),
                    ("Easter Strongbox", C, 4, 0),
                    ("Strongbox Key", C, 22, 22_000),
                ],
            ),
            // Counts that differ.
            (
                &[
                    ("Halloween Strongbox", C, 23, 0),
                    ("Strongbox Key", C, 24, 24_000),
                ],
                &[
                    ("Halloween Strongbox", C, 17, 0),
                    ("Strongbox Key", C, 17, 17_000),
                ],
            ),
            // Boxes with no key.
            (
                &[("Halloween Strongbox", C, 23, 0)],
                &[("Halloween Strongbox", C, 20, 0)],
            ),
        ];
        for (before, after) in shapes {
            let found = recognised(
                &reading(0.0, 0, before),
                &reading(60.0, 0, &[after, &[PANTS][..]].concat()),
            );
            let opening = found.opening.expect("still an opening");
            assert_eq!(opening.settlement, Settlement::Unsettled);
            assert!(opening.contents.is_empty());
            assert_eq!(
                found.claims.values().collect::<Vec<_>>(),
                vec![&Claim::Possibly]
            );
        }
        let two = recognised(
            &reading(0.0, 0, shapes[0].0),
            &reading(60.0, 0, shapes[0].1),
        )
        .opening
        .unwrap();
        assert_eq!(two.boxes.len(), 2);
        assert_eq!(two.keys_used, 2);
    }

    #[test]
    fn keys_leaving_with_no_box_are_no_opening() {
        let found = recognised(
            &reading(0.0, 0, &[("Strongbox Key", C, 24, 24_000)]),
            &reading(60.0, 0, &[("Strongbox Key", C, 17, 17_000)]),
        );
        assert_eq!(found.opening, None);
        let set = StepSet {
            span: Span { from: 0.0, to: 1.0 },
            steps: vec![],
        };
        let empty = reading(0.0, 0, &[]);
        assert_eq!(
            recognise(&set, &empty, &empty, &[]).unwrap(),
            Composites::default()
        );
    }

    #[test]
    fn a_reserved_ammo_row_is_never_read_as_the_opening_s_payout() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", C, 2, 0),
                ("Strongbox Key", C, 2, 2_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", C, 1, 0),
                ("Strongbox Key", C, 1, 1_000),
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 10_100, 101),
            ],
        );
        let set = diff(&before, &after, &stacking).unwrap();
        let ammo = HoldingKey {
            name: UNIVERSAL_AMMO.into(),
            container: C,
        };
        let reserved = [(
            ammo,
            Row {
                quantity: 10_100,
                value: Pec(101),
            },
        )];
        let opening = recognise(&set, &before, &after, &reserved)
            .unwrap()
            .opening
            .unwrap();
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.ammo_units, 100_000);
        // Unreserved, the conversion's row is read as payout too, and the
        // keys' balance no longer closes.
        let unreserved = recognise(&set, &before, &after, &[])
            .unwrap()
            .opening
            .unwrap();
        assert_eq!(unreserved.settlement, Settlement::Unsettled);
        assert_eq!(unreserved.ammo_units, 110_100);
    }

    #[test]
    fn too_many_ways_to_weigh_unsettle_the_opening() {
        let mut extra: Vec<(&'static str, Container, i64, i64)> = Vec::new();
        const NAMES: [&str; 18] = [
            "A (F)", "B (F)", "C (F)", "D (F)", "E (F)", "F (F)", "G (F)", "H (F)", "I (F)",
            "J (F)", "K (F)", "L (F)", "M (F)", "N (F)", "O (F)", "P (F)", "Q (F)", "R (F)",
        ];
        for name in NAMES {
            extra.push((name, C, 1, 1));
        }
        let found = opening_of(&[(100_000, 1_000); 7], &extra);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
        assert_eq!(found.claims.len(), 18);
        assert!(found.claims.values().all(|claim| *claim == Claim::Possibly));
    }
}
