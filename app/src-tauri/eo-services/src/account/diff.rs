//! The steps between two consecutive readings.
//!
//! Every holding whose rows differ becomes one or more [`Step`]s, read at
//! row level:
//!
//! - a single item whose value falls or rises in place has
//!   [`Step::Decayed`] or [`Step::Repaired`];
//! - a single item gone from one container and one of the same name new in
//!   another is [`Step::Moved`], with the wear or restoration on the way as
//!   its own step;
//! - a single item arriving or leaving with no matching move is
//!   [`Step::Appeared`] or [`Step::Left`], a change of what the player
//!   holds;
//! - a stackable falling in one container while rising in another is a
//!   move of the smaller figure, and only the remainder is consumed or
//!   gained;
//! - a stackable whose quantity falls or rises is [`Step::Consumed`] or
//!   [`Step::Gained`], carrying the rows that are new in this step;
//! - the PED Card moving is [`Step::BalanceChanged`].
//!
//! A stackable's amount is its quantity change times its unit value when
//! the unit value is the one value per unit (in micro-PED) that reproduces
//! every truncated row value of the holding in both readings
//! ([`Amount::Exact`]); otherwise it is the change in truncated values
//! ([`Amount::WholePec`]).
//!
//! Rows only shrink by being fired or used and only grow by being stacked
//! onto, so a row new in a step is an after-row that no before-row at least
//! as large can account for. When the rows allow more than one such
//! reading, [`new_row_readings`] lists them all, for the strongbox opening
//! to choose between by its keys' balance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::amount::{checked, Amount, Pec, MICRO_PER_PEC};
use super::reading::{Container, Holding, HoldingKey, Reading, Row};
use super::roles::Stacking;
use super::AccountError;

/// The most readings of a holding's new rows [`new_row_readings`] weighs
/// before it gives up and claims none.
pub const MAX_NEW_ROW_READINGS: usize = 4_096;

/// The span of time a step happened in: after one reading, by the next.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub from: f64,
    pub to: f64,
}

/// The rows of a stackable that are new in a step, with what they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRows {
    pub rows: Vec<Row>,
    pub units: i64,
    pub amount: Amount,
}

impl NewRows {
    /// No row is new: exactly nothing arrived.
    pub fn none() -> NewRows {
        NewRows {
            rows: Vec::new(),
            units: 0,
            amount: Amount::NONE,
        }
    }

    /// These rows of a holding whose exact unit value, if any, is `unit`.
    pub fn of(rows: Vec<Row>, unit: Option<i64>) -> Result<NewRows, AccountError> {
        let units = super::amount::checked_sum(rows.iter().map(|row| row.quantity))?;
        let amount = match unit {
            Some(unit) => Amount::exact_units(units, unit)?,
            None => Amount::whole_pec(Pec::total(rows.iter().map(|row| row.value))?)?,
        };
        Ok(NewRows {
            rows,
            units,
            amount,
        })
    }
}

/// What a move between containers is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveKind {
    /// Into the auction: a listing.
    Listing,
    /// Out of the auction into another container: a listing returned.
    ListingReturned,
    /// Between storage, the carried inventory, attachments, and other
    /// places.
    Transfer,
}

impl MoveKind {
    pub fn of(from: &Container, to: &Container) -> MoveKind {
        match (from, to) {
            (_, Container::Auction) => MoveKind::Listing,
            (Container::Auction, _) => MoveKind::ListingReturned,
            _ => MoveKind::Transfer,
        }
    }
}

/// One change between two consecutive readings.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A single item's value fell: wear. The amount is positive.
    Decayed { key: HoldingKey, by: Amount },
    /// A single item's value rose. The amount is positive.
    Repaired { key: HoldingKey, by: Amount },
    /// A stackable's quantity fell, including a holding that vanished.
    Consumed {
        key: HoldingKey,
        units: i64,
        amount: Amount,
        new_rows: NewRows,
        /// Rows that changed, vanished, or are new.
        rows_touched: usize,
    },
    /// A stackable's quantity rose, including a holding that appeared.
    Gained {
        key: HoldingKey,
        units: i64,
        amount: Amount,
        new_rows: NewRows,
        /// Rows that changed, vanished, or are new.
        rows_touched: usize,
    },
    /// Units or an item that left one container and arrived in another.
    Moved {
        name: String,
        units: i64,
        amount: Amount,
        from: Container,
        to: Container,
    },
    /// A single item arrived with no matching move.
    Appeared {
        key: HoldingKey,
        units: i64,
        value: Amount,
    },
    /// A single item went with no matching move.
    Left {
        key: HoldingKey,
        units: i64,
        value: Amount,
    },
    /// The PED Card balance moved.
    BalanceChanged { from: Pec, to: Pec },
}

impl Step {
    /// The signed quantity change this step makes to each holding.
    pub fn quantity_changes(&self) -> Vec<(HoldingKey, i64)> {
        match self {
            Step::Decayed { .. } | Step::Repaired { .. } | Step::BalanceChanged { .. } => vec![],
            Step::Consumed { key, units, .. } | Step::Left { key, units, .. } => {
                vec![(key.clone(), -units)]
            }
            Step::Gained { key, units, .. } | Step::Appeared { key, units, .. } => {
                vec![(key.clone(), *units)]
            }
            Step::Moved {
                name,
                units,
                from,
                to,
                ..
            } => vec![
                (
                    HoldingKey {
                        name: name.clone(),
                        container: from.clone(),
                    },
                    -units,
                ),
                (
                    HoldingKey {
                        name: name.clone(),
                        container: to.clone(),
                    },
                    *units,
                ),
            ],
        }
    }
}

/// All the steps between two consecutive readings.
#[derive(Debug, Clone, PartialEq)]
pub struct StepSet {
    pub span: Span,
    pub steps: Vec<Step>,
}

/// The rows two states of one holding do not share.
#[derive(Debug, Default)]
struct RowChange {
    removed: Vec<Row>,
    added: Vec<Row>,
}

fn row_change(before: &[Row], after: &[Row]) -> RowChange {
    let mut removed: Vec<Row> = before.to_vec();
    let mut added: Vec<Row> = Vec::new();
    for row in after {
        match removed.iter().position(|kept| kept == row) {
            Some(index) => {
                removed.swap_remove(index);
            }
            None => added.push(*row),
        }
    }
    removed.sort_unstable();
    added.sort_unstable();
    RowChange { removed, added }
}

fn rows_of(holding: Option<&Holding>) -> &[Row] {
    holding.map(Holding::rows).unwrap_or_default()
}

/// The after-rows not identical to a before-row: rows that changed or are
/// new, in canonical order.
pub fn added_rows(before: &[Row], after: &[Row]) -> Vec<Row> {
    row_change(before, after).added
}

/// The gap between two rows' values, checked.
fn value_gap(a: &Row, b: &Row) -> Result<i64, AccountError> {
    checked(a.value.0.checked_sub(b.value.0).and_then(i64::checked_abs))
}

/// The rows new in a step, read the one canonical way: identical rows are
/// the same row; each remaining after-row, largest first, is a before-row
/// at least as large that shrank; an after-row no such before-row can
/// account for is new.
pub fn canonical_new_rows(before: &[Row], after: &[Row]) -> Vec<Row> {
    canonical_rows(before, after).0
}

/// The canonical reading of a holding's rows: the rows new in the step
/// (as [`canonical_new_rows`] reads them), and the before-rows no
/// after-row accounts for. A vanished row smaller than a new one may have
/// been fired out or grown into it by stacking: the rows cannot tell.
pub fn canonical_rows(before: &[Row], after: &[Row]) -> (Vec<Row>, Vec<Row>) {
    let change = row_change(before, after);
    let mut available: Vec<Option<Row>> = change.removed.into_iter().map(Some).collect();
    let mut new = Vec::new();
    for row in change.added.into_iter().rev() {
        let shrank_from = available
            .iter()
            .position(|before| before.is_some_and(|before| before.quantity >= row.quantity));
        match shrank_from {
            Some(index) => available[index] = None,
            None => new.push(row),
        }
    }
    new.sort_unstable();
    (new, available.into_iter().flatten().collect())
}

/// Whether every after-row can be a distinct before-row at least as large.
fn shrinks_into(after: &mut [i64], before: &[i64]) -> bool {
    after.sort_unstable_by(|a, b| b.cmp(a));
    after.len() <= before.len() && after.iter().zip(before).all(|(a, b)| a <= b)
}

/// Every reading of which rows are new in a step, as distinct sets of rows,
/// that accounts for as many after-rows as possible by before-rows at least
/// as large. `None` when there are more than [`MAX_NEW_ROW_READINGS`] to
/// weigh.
pub fn new_row_readings(before: &[Row], after: &[Row]) -> Option<Vec<Vec<Row>>> {
    let mut before_quantities: Vec<i64> = before.iter().map(|row| row.quantity).collect();
    before_quantities.sort_unstable_by(|a, b| b.cmp(a));
    // As many after-rows as possible are accounted for by before-rows;
    // the rest are new.
    let mut after_quantities: Vec<i64> = after.iter().map(|row| row.quantity).collect();
    after_quantities.sort_unstable_by(|a, b| b.cmp(a));
    let new_count = after.len() - maximum_shrinks(&after_quantities, &before_quantities);

    let mut groups: Vec<(Row, usize)> = Vec::new();
    for row in after {
        match groups.iter_mut().find(|(kept, _)| kept == row) {
            Some((_, n)) => *n += 1,
            None => groups.push((*row, 1)),
        }
    }
    groups.sort_unstable();
    let mut readings: Vec<Vec<Row>> = Vec::new();
    let mut weighed = 0usize;
    let mut chosen: Vec<usize> = vec![0; groups.len()];
    if !choose(
        &groups,
        0,
        new_count,
        &mut chosen,
        &mut weighed,
        &mut |chosen| {
            let mut remaining: Vec<i64> = Vec::new();
            let mut new_rows: Vec<Row> = Vec::new();
            for ((row, n), taken) in groups.iter().zip(chosen) {
                new_rows.extend(std::iter::repeat_n(*row, *taken));
                remaining.extend(std::iter::repeat_n(row.quantity, n - taken));
            }
            if shrinks_into(&mut remaining, &before_quantities) && !readings.contains(&new_rows) {
                readings.push(new_rows);
            }
        },
    ) {
        return None;
    }
    Some(readings)
}

/// The size of the largest set of after-rows that distinct before-rows at
/// least as large can account for. Both lists are largest first.
fn maximum_shrinks(after: &[i64], before: &[i64]) -> usize {
    // Largest after-rows first, each taking the largest free before-row:
    // when the largest free before-row is too small for this after-row, no
    // other is larger, so the after-row is new.
    let mut free = 0;
    let mut matched = 0;
    for quantity in after {
        if free < before.len() && before[free] >= *quantity {
            free += 1;
            matched += 1;
        }
    }
    matched
}

/// Walk every way of taking `left` rows from the groups, calling `visit`
/// with the counts taken from each. False when the walk grew beyond
/// [`MAX_NEW_ROW_READINGS`].
fn choose(
    groups: &[(Row, usize)],
    index: usize,
    left: usize,
    chosen: &mut Vec<usize>,
    weighed: &mut usize,
    visit: &mut dyn FnMut(&[usize]),
) -> bool {
    if index == groups.len() {
        if left == 0 {
            *weighed += 1;
            if *weighed > MAX_NEW_ROW_READINGS {
                return false;
            }
            visit(chosen);
        }
        return true;
    }
    let most = groups[index].1.min(left);
    for taken in 0..=most {
        chosen[index] = taken;
        if !choose(groups, index + 1, left - taken, chosen, weighed, visit) {
            return false;
        }
    }
    chosen[index] = 0;
    true
}

fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    let quotient = numerator / denominator;
    if numerator % denominator > 0 {
        quotient + 1
    } else {
        quotient
    }
}

/// The one unit value in micro-PED that reproduces every row's truncated
/// value (`value_pec == floor(quantity * unit / 1 PEC)`), when exactly one
/// does.
pub fn exact_unit<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Option<i64> {
    let pec = i128::from(MICRO_PER_PEC);
    let mut low: i128 = 0;
    let mut high: i128 = i128::MAX;
    let mut constrained = false;
    for row in rows {
        let quantity = i128::from(row.quantity);
        let value = i128::from(row.value.0) * pec;
        if quantity == 0 {
            if row.value.0 != 0 {
                return None;
            }
            continue;
        }
        constrained = true;
        low = low.max(ceil_div(value, quantity));
        high = high.min(ceil_div(value + pec, quantity) - 1);
    }
    (constrained && low == high)
        .then(|| i64::try_from(low).ok())
        .flatten()
}

/// The steps from `before` to `after`. `stacking` says, per item name,
/// whether its rows count units or hold one item each.
pub fn diff(
    before: &Reading,
    after: &Reading,
    stacking: &dyn Fn(&str) -> Stacking,
) -> Result<StepSet, AccountError> {
    let mut by_name: BTreeMap<&str, Vec<&HoldingKey>> = BTreeMap::new();
    for key in before.holdings().keys().chain(after.holdings().keys()) {
        if before.holding(key) != after.holding(key) {
            let keys = by_name.entry(key.name.as_str()).or_default();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    let mut steps = Vec::new();
    for (name, keys) in by_name {
        match stacking(name) {
            Stacking::Single => single_steps(name, &keys, before, after, &mut steps)?,
            Stacking::Stack => stack_steps(name, &keys, before, after, &mut steps)?,
        }
    }
    if before.balance() != after.balance() {
        steps.push(Step::BalanceChanged {
            from: before.balance(),
            to: after.balance(),
        });
    }
    Ok(StepSet {
        span: Span {
            from: before.taken_at(),
            to: after.taken_at(),
        },
        steps,
    })
}

/// Pair rows of equal quantity, closest value first, each at most once.
/// Returns the pairs and the unpaired rows on each side.
/// Rows paired in place, and the rows left unpaired on each side.
type InPlace = (Vec<(Row, Row)>, Vec<Row>, Vec<Row>);

fn pair_in_place(removed: &[Row], added: &[Row]) -> Result<InPlace, AccountError> {
    let mut candidates: Vec<(i64, usize, usize)> = Vec::new();
    for (i, before) in removed.iter().enumerate() {
        for (j, after) in added.iter().enumerate() {
            if before.quantity == after.quantity {
                candidates.push((value_gap(before, after)?, i, j));
            }
        }
    }
    candidates.sort_unstable();
    let mut used_removed = vec![false; removed.len()];
    let mut used_added = vec![false; added.len()];
    let mut pairs = Vec::new();
    for (_, i, j) in candidates {
        if !used_removed[i] && !used_added[j] {
            used_removed[i] = true;
            used_added[j] = true;
            pairs.push((removed[i], added[j]));
        }
    }
    let left = |rows: &[Row], used: &[bool]| -> Vec<Row> {
        rows.iter()
            .zip(used)
            .filter(|(_, used)| !**used)
            .map(|(row, _)| *row)
            .collect()
    };
    Ok((
        pairs,
        left(removed, &used_removed),
        left(added, &used_added),
    ))
}

/// Rows still to be matched; a matched row is taken out as `None`.
type Slots = Vec<Option<Row>>;

fn single_steps(
    name: &str,
    keys: &[&HoldingKey],
    before: &Reading,
    after: &Reading,
    steps: &mut Vec<Step>,
) -> Result<(), AccountError> {
    let mut worn = vec![0i64; keys.len()];
    let mut restored = vec![0i64; keys.len()];
    // Per holding: the rows gone and the rows new, each taken once.
    let mut unmatched: Vec<(Slots, Slots)> = Vec::new();
    // In place first: a holding that still holds an item of this name had
    // that item wear or be restored. Two plates wearing past each other's
    // values are two items wearing, not one moving.
    for (index, key) in keys.iter().enumerate() {
        let change = row_change(rows_of(before.holding(key)), rows_of(after.holding(key)));
        let (pairs, left, arrived) = pair_in_place(&change.removed, &change.added)?;
        for (b, a) in pairs {
            let change = checked(a.value.0.checked_sub(b.value.0))?;
            if change < 0 {
                worn[index] = checked(worn[index].checked_sub(change))?;
            } else {
                restored[index] = checked(restored[index].checked_add(change))?;
            }
        }
        unmatched.push((
            left.into_iter().map(Some).collect(),
            arrived.into_iter().map(Some).collect(),
        ));
    }
    // Then an item gone from one container and one new in another is the
    // same item moved, identical first, then closest in value; the wear or
    // restoration on the way stays with the container it left.
    let mut candidates: Vec<(i64, usize, usize, usize, usize)> = Vec::new();
    for (source, (left, _)) in unmatched.iter().enumerate() {
        for (destination, (_, arrived)) in unmatched.iter().enumerate() {
            if source == destination {
                continue;
            }
            for (a, gone) in left.iter().enumerate() {
                for (b, new) in arrived.iter().enumerate() {
                    if let (Some(gone), Some(new)) = (gone, new) {
                        if gone.quantity == new.quantity {
                            candidates.push((value_gap(gone, new)?, source, a, destination, b));
                        }
                    }
                }
            }
        }
    }
    candidates.sort_unstable();
    let mut moves = Vec::new();
    for (_, source, a, destination, b) in candidates {
        let (Some(gone), Some(new)) = (unmatched[source].0[a], unmatched[destination].1[b]) else {
            continue;
        };
        unmatched[source].0[a] = None;
        unmatched[destination].1[b] = None;
        let change = checked(new.value.0.checked_sub(gone.value.0))?;
        if change < 0 {
            worn[source] = checked(worn[source].checked_sub(change))?;
        } else {
            restored[source] = checked(restored[source].checked_add(change))?;
        }
        moves.push(Step::Moved {
            name: name.to_string(),
            units: gone.quantity,
            amount: Amount::whole_pec(gone.value)?,
            from: keys[source].container.clone(),
            to: keys[destination].container.clone(),
        });
    }
    for (index, key) in keys.iter().enumerate() {
        if worn[index] > 0 {
            steps.push(Step::Decayed {
                key: (*key).clone(),
                by: Amount::whole_pec(Pec(worn[index]))?,
            });
        }
        if restored[index] > 0 {
            steps.push(Step::Repaired {
                key: (*key).clone(),
                by: Amount::whole_pec(Pec(restored[index]))?,
            });
        }
    }
    steps.extend(moves);
    for (key, (left, arrived)) in keys.iter().zip(unmatched) {
        for row in left.into_iter().flatten() {
            steps.push(Step::Left {
                key: (*key).clone(),
                units: row.quantity,
                value: Amount::whole_pec(row.value)?,
            });
        }
        for row in arrived.into_iter().flatten() {
            steps.push(Step::Appeared {
                key: (*key).clone(),
                units: row.quantity,
                value: Amount::whole_pec(row.value)?,
            });
        }
    }
    Ok(())
}

/// A stackable holding's change, as moves are taken out of it.
struct StackChange {
    key: HoldingKey,
    /// Signed.
    units: i64,
    /// Signed.
    amount: Amount,
    unit: Option<i64>,
    before: Vec<Row>,
    after: Vec<Row>,
}

fn stack_steps(
    name: &str,
    keys: &[&HoldingKey],
    before: &Reading,
    after: &Reading,
    steps: &mut Vec<Step>,
) -> Result<(), AccountError> {
    let mut changes: Vec<StackChange> = Vec::with_capacity(keys.len());
    for key in keys {
        let was = before.holding(key);
        let now = after.holding(key);
        let quantity = |h: Option<&Holding>| h.map_or(0, Holding::quantity);
        let value = |h: Option<&Holding>| h.map_or(Pec::ZERO, Holding::value);
        let units = checked(quantity(now).checked_sub(quantity(was)))?;
        let unit = exact_unit(rows_of(was).iter().chain(rows_of(now)));
        let amount = match unit {
            Some(unit) => Amount::exact_units(units, unit)?,
            None => Amount::whole_pec(value(now).minus(value(was))?)?,
        };
        changes.push(StackChange {
            key: (*key).clone(),
            units,
            amount,
            unit,
            before: rows_of(was).to_vec(),
            after: rows_of(now).to_vec(),
        });
    }
    // Row evidence first: a row that arrived in one container is a move
    // when an identical row vanished from another, or failing that, when
    // another container fell by exactly its units.
    for by_identical_row in [true, false] {
        for destination in 0..changes.len() {
            let arrived = added_rows(&changes[destination].before, &changes[destination].after);
            for row in arrived {
                let source = (0..changes.len()).find(|&source| {
                    let from = &changes[source];
                    source != destination
                        && if by_identical_row {
                            from.before.contains(&row) && !from.after.contains(&row)
                        } else {
                            from.units < 0 && from.units == -row.quantity
                        }
                });
                let Some(source) = source else {
                    continue;
                };
                let amount = match changes[source].unit.or(changes[destination].unit) {
                    Some(unit) => Amount::exact_units(row.quantity, unit)?,
                    None => Amount::whole_pec(row.value)?,
                };
                steps.push(Step::Moved {
                    name: name.to_string(),
                    units: row.quantity,
                    amount,
                    from: changes[source].key.container.clone(),
                    to: changes[destination].key.container.clone(),
                });
                let from = &mut changes[source];
                from.units = checked(from.units.checked_add(row.quantity))?;
                from.amount = from.amount.plus(amount)?;
                if by_identical_row {
                    if let Some(index) = from.before.iter().position(|kept| *kept == row) {
                        from.before.remove(index);
                    }
                }
                let to = &mut changes[destination];
                to.units = checked(to.units.checked_sub(row.quantity))?;
                to.amount = to.amount.minus(amount)?;
                if let Some(index) = to.after.iter().position(|kept| *kept == row) {
                    to.after.remove(index);
                }
            }
        }
    }
    // Then a fall in one container beside a rise in another is a move of
    // the smaller figure; only the remainder is consumed or gained.
    for source in 0..changes.len() {
        for destination in 0..changes.len() {
            let (from, to) = (&changes[source], &changes[destination]);
            if source == destination || from.units >= 0 || to.units <= 0 {
                continue;
            }
            let leaving = -from.units;
            let units = leaving.min(to.units);
            // A unit value either side pins is the item's: the moved units
            // are exact at it. Otherwise the smaller side's own figure.
            let amount = match from.unit.or(to.unit) {
                Some(unit) => Amount::exact_units(units, unit)?,
                None if leaving <= to.units => from.amount.negated()?,
                None => to.amount,
            };
            steps.push(Step::Moved {
                name: name.to_string(),
                units,
                amount,
                from: from.key.container.clone(),
                to: to.key.container.clone(),
            });
            let from = &mut changes[source];
            from.units = checked(from.units.checked_add(units))?;
            from.amount = from.amount.plus(amount)?;
            let to = &mut changes[destination];
            to.units = checked(to.units.checked_sub(units))?;
            to.amount = to.amount.minus(amount)?;
        }
    }
    for change in changes {
        if change.units == 0 {
            continue;
        }
        let new_rows = NewRows::of(
            canonical_new_rows(&change.before, &change.after),
            change.unit,
        )?;
        let touched = row_change(&change.before, &change.after);
        let rows_touched = touched.removed.len().max(touched.added.len());
        if change.units < 0 {
            steps.push(Step::Consumed {
                units: -change.units,
                amount: change.amount.negated()?,
                new_rows,
                rows_touched,
                key: change.key,
            });
        } else {
            steps.push(Step::Gained {
                units: change.units,
                amount: change.amount,
                new_rows,
                rows_touched,
                key: change.key,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::test_support::{reading, ReadingSpec};
    use proptest::prelude::*;

    fn stack(_: &str) -> Stacking {
        Stacking::Stack
    }

    fn by_name(name: &str) -> Stacking {
        if name.ends_with("(M)") || name.starts_with("Armor") || name.starts_with("Opalo") {
            Stacking::Single
        } else {
            Stacking::Stack
        }
    }

    fn key(name: &str, container: Container) -> HoldingKey {
        HoldingKey {
            name: name.to_string(),
            container,
        }
    }

    fn row(quantity: i64, value: i64) -> Row {
        Row {
            quantity,
            value: Pec(value),
        }
    }

    fn pec(value: i64) -> Amount {
        Amount::whole_pec(Pec(value)).unwrap()
    }

    fn steps(before: &Reading, after: &Reading, stacking: &dyn Fn(&str) -> Stacking) -> Vec<Step> {
        diff(before, after, stacking).unwrap().steps
    }

    #[test]
    fn the_unit_value_is_exact_only_when_one_value_reproduces_every_row() {
        assert_eq!(exact_unit(&[row(965_587, 9_655)]), Some(100));
        assert_eq!(
            exact_unit(&[row(99_500, 995), row(100_000, 1_000)]),
            Some(100)
        );
        // Four units at 1.60 PED admit many unit values.
        assert_eq!(exact_unit(&[row(4, 160)]), None);
        // Rows that disagree admit none.
        assert_eq!(
            exact_unit(&[row(100_000, 1_000), row(100_000, 2_000)]),
            None
        );
        // Enough units at no value pin a unit value of nothing.
        assert_eq!(exact_unit(&[row(20_000, 0)]), Some(0));
        assert_eq!(exact_unit(&[row(28, 0)]), None);
        assert_eq!(exact_unit(&[row(0, 5)]), None);
        assert_eq!(exact_unit(&[]), None);
    }

    #[test]
    fn a_stackable_burning_down_is_consumed_exactly() {
        let before = reading(
            0.0,
            25_000,
            &[("Universal Ammo", Container::Carried, 965_587, 9_655)],
        );
        let after = reading(
            60.0,
            25_000,
            &[("Universal Ammo", Container::Carried, 808_723, 8_087)],
        );
        let set = diff(&before, &after, &stack).unwrap();
        assert_eq!(
            set.span,
            Span {
                from: 0.0,
                to: 60.0
            }
        );
        assert_eq!(
            set.steps,
            vec![Step::Consumed {
                key: key("Universal Ammo", Container::Carried),
                units: 156_864,
                amount: Amount::exact(15_686_400),
                new_rows: NewRows::none(),
                rows_touched: 1,
            }]
        );
    }

    #[test]
    fn a_stackable_without_an_exact_unit_value_falls_back_to_its_truncated_values() {
        let before = reading(0.0, 0, &[]);
        let after = reading(
            60.0,
            0,
            &[("Output Amplifier Component", Container::Carried, 4, 160)],
        );
        let found = steps(&before, &after, &stack);
        let [Step::Gained {
            units,
            amount,
            new_rows,
            rows_touched,
            ..
        }] = found.as_slice()
        else {
            panic!("one gain");
        };
        assert_eq!(*units, 4);
        assert_eq!(*amount, pec(160));
        assert_eq!(new_rows.rows, vec![row(4, 160)]);
        assert_eq!(*rows_touched, 1);
    }

    #[test]
    fn a_row_new_in_a_step_is_one_no_larger_before_row_accounts_for() {
        // A vanished small row cannot have grown into a large one.
        assert_eq!(
            canonical_new_rows(
                &[row(5_000, 50), row(100_000, 1_000)],
                &[row(100_000, 1_000), row(100_000, 1_000)]
            ),
            vec![row(100_000, 1_000)]
        );
        // The row being fired shrinks; the new rows stand apart.
        let mut after = vec![row(343_923, 3_439)];
        after.extend([row(100_000, 1_000); 6]);
        assert_eq!(
            canonical_new_rows(&[row(443_923, 4_439)], &after),
            vec![row(100_000, 1_000); 6]
        );
        // Identical rows first: the shrunk row is read as the new one.
        assert_eq!(
            canonical_new_rows(
                &[row(100_000, 1_000), row(100_000, 1_000)],
                &[row(95_100, 951), row(100_000, 1_000), row(100_000, 1_000)],
            ),
            vec![row(95_100, 951)]
        );
        // A row that grew by stacking is new as a whole.
        assert_eq!(
            canonical_new_rows(&[row(81_368, 813)], &[row(176_370, 1_763)]),
            vec![row(176_370, 1_763)]
        );
    }

    #[test]
    fn every_reading_of_the_new_rows_is_listed_when_the_rows_allow_more_than_one() {
        let readings = new_row_readings(
            &[row(100_000, 1_000), row(100_000, 1_000)],
            &[row(95_100, 951), row(100_000, 1_000), row(100_000, 1_000)],
        )
        .expect("few enough to weigh");
        assert_eq!(readings.len(), 2);
        assert!(readings.contains(&vec![row(95_100, 951)]));
        assert!(readings.contains(&vec![row(100_000, 1_000)]));
        // Only one reading when the small row cannot have been a large one.
        assert_eq!(
            new_row_readings(
                &[row(5_000, 50), row(100_000, 1_000)],
                &[row(100_000, 1_000), row(100_000, 1_000)]
            ),
            Some(vec![vec![row(100_000, 1_000)]])
        );
        // Nothing new when every after-row is accounted for.
        assert_eq!(
            new_row_readings(&[row(9, 0)], &[row(4, 0)]),
            Some(vec![vec![]])
        );
    }

    #[test]
    fn too_many_readings_of_the_new_rows_claim_none() {
        let before: Vec<Row> = (1..=12).map(|q| row(q * 1_000_000, 0)).collect();
        let after: Vec<Row> = (1..=24).map(|q| row(q * 1_000, 0)).collect();
        assert_eq!(new_row_readings(&before, &after), None);
    }

    #[test]
    fn merging_two_rows_into_one_is_no_change() {
        let before = reading(
            0.0,
            0,
            &[
                ("Shrapnel", Container::Carried, 100_000, 1_000),
                ("Shrapnel", Container::Carried, 100_000, 1_000),
            ],
        );
        let after = reading(60.0, 0, &[("Shrapnel", Container::Carried, 200_000, 2_000)]);
        assert!(steps(&before, &after, &stack).is_empty());
    }

    #[test]
    fn a_single_item_wears_and_is_restored_in_place() {
        let arm = key("Paneleon Arm Guards (M)", Container::Carried);
        let worn = steps(
            &reading(
                0.0,
                0,
                &[("Paneleon Arm Guards (M)", Container::Carried, 1, 2_150)],
            ),
            &reading(
                60.0,
                0,
                &[("Paneleon Arm Guards (M)", Container::Carried, 1, 2_147)],
            ),
            &by_name,
        );
        assert_eq!(
            worn,
            vec![Step::Decayed {
                key: arm.clone(),
                by: pec(3)
            }]
        );
        let restored = steps(
            &reading(
                0.0,
                0,
                &[("Paneleon Arm Guards (M)", Container::Carried, 1, 2_143)],
            ),
            &reading(
                60.0,
                0,
                &[("Paneleon Arm Guards (M)", Container::Carried, 1, 2_150)],
            ),
            &by_name,
        );
        assert_eq!(
            restored,
            vec![Step::Repaired {
                key: arm,
                by: pec(7)
            }]
        );
    }

    #[test]
    fn two_items_wearing_past_each_other_s_values_wear_in_place() {
        let arm = Container::Attached {
            parent: "Arm Guards (M)".into(),
        };
        let chest = Container::Attached {
            parent: "Chest Guard (M)".into(),
        };
        let found = steps(
            &reading(
                0.0,
                0,
                &[
                    ("Armor Plating Mk. 6A", arm.clone(), 1, 2_499),
                    ("Armor Plating Mk. 6A", chest.clone(), 1, 2_500),
                ],
            ),
            &reading(
                60.0,
                0,
                &[
                    ("Armor Plating Mk. 6A", arm.clone(), 1, 2_498),
                    ("Armor Plating Mk. 6A", chest.clone(), 1, 2_499),
                ],
            ),
            &by_name,
        );
        assert_eq!(
            found,
            vec![
                Step::Decayed {
                    key: key("Armor Plating Mk. 6A", arm),
                    by: pec(1)
                },
                Step::Decayed {
                    key: key("Armor Plating Mk. 6A", chest),
                    by: pec(1)
                },
            ]
        );
    }

    #[test]
    fn a_single_item_moved_unchanged_is_a_move_and_the_auction_is_a_listing() {
        let storage = Container::Storage {
            planet: "Calypso".into(),
        };
        let found = steps(
            &reading(0.0, 0, &[("Opalo", Container::Carried, 1, 500)]),
            &reading(60.0, 0, &[("Opalo", storage.clone(), 1, 500)]),
            &by_name,
        );
        assert_eq!(
            found,
            vec![Step::Moved {
                name: "Opalo".into(),
                units: 1,
                amount: pec(500),
                from: Container::Carried,
                to: storage.clone(),
            }]
        );
        assert_eq!(
            MoveKind::of(&Container::Carried, &Container::Auction),
            MoveKind::Listing
        );
        assert_eq!(
            MoveKind::of(&Container::Auction, &Container::Carried),
            MoveKind::ListingReturned
        );
        assert_eq!(
            MoveKind::of(&Container::Carried, &storage),
            MoveKind::Transfer
        );
        let attach = Container::Attached {
            parent: "Opalo".into(),
        };
        assert_eq!(
            MoveKind::of(&attach, &Container::Carried),
            MoveKind::Transfer
        );
    }

    #[test]
    fn a_single_item_moved_while_it_wears_is_a_move_and_its_wear() {
        let worn = steps(
            &reading(0.0, 0, &[("Opalo", Container::Carried, 1, 500)]),
            &reading(60.0, 0, &[("Opalo", Container::Auction, 1, 499)]),
            &by_name,
        );
        assert_eq!(
            worn,
            vec![
                Step::Decayed {
                    key: key("Opalo", Container::Carried),
                    by: pec(1)
                },
                Step::Moved {
                    name: "Opalo".into(),
                    units: 1,
                    amount: pec(500),
                    from: Container::Carried,
                    to: Container::Auction,
                },
            ]
        );
        let restored = steps(
            &reading(0.0, 0, &[("Opalo", Container::Carried, 1, 490)]),
            &reading(
                60.0,
                0,
                &[(
                    "Opalo",
                    Container::Storage {
                        planet: "Calypso".into(),
                    },
                    1,
                    500,
                )],
            ),
            &by_name,
        );
        assert!(matches!(
            restored.as_slice(),
            [Step::Repaired { by, .. }, Step::Moved { .. }] if *by == pec(10)
        ));
    }

    #[test]
    fn part_of_a_stack_moved_into_storage_is_a_move() {
        let storage = Container::Storage {
            planet: "Arkadia".into(),
        };
        let found = steps(
            &reading(
                0.0,
                0,
                &[("Universal Ammo", Container::Carried, 965_587, 9_655)],
            ),
            &reading(
                60.0,
                0,
                &[
                    ("Universal Ammo", Container::Carried, 865_587, 8_655),
                    ("Universal Ammo", storage.clone(), 100_000, 1_000),
                ],
            ),
            &stack,
        );
        assert_eq!(
            found,
            vec![Step::Moved {
                name: "Universal Ammo".into(),
                units: 100_000,
                amount: Amount::exact(10_000_000),
                from: Container::Carried,
                to: storage,
            }]
        );
    }

    #[test]
    fn a_stack_partly_moved_while_it_also_changes_moves_the_smaller_figure() {
        let storage = Container::Storage {
            planet: "Arkadia".into(),
        };
        let found = steps(
            &reading(
                0.0,
                0,
                &[("Universal Ammo", Container::Carried, 965_587, 9_655)],
            ),
            &reading(
                60.0,
                0,
                &[
                    ("Universal Ammo", Container::Carried, 964_087, 9_640),
                    ("Universal Ammo", storage.clone(), 1_000, 10),
                ],
            ),
            &stack,
        );
        assert_eq!(found.len(), 2);
        assert!(
            matches!(&found[0], Step::Moved { units: 1_000, amount, .. } if *amount == Amount::exact(100_000))
        );
        assert!(
            matches!(&found[1], Step::Consumed { units: 500, amount, .. } if *amount == Amount::exact(50_000))
        );
        // And the other way round: storage down 1500 into carried up 1000.
        let found = steps(
            &reading(0.0, 0, &[("Shrapnel", storage.clone(), 1_500, 0)]),
            &reading(60.0, 0, &[("Shrapnel", Container::Carried, 1_000, 0)]),
            &stack,
        );
        assert!(matches!(&found[0], Step::Moved { units: 1_000, .. }));
        assert!(matches!(&found[1], Step::Consumed { units: 500, .. }));
    }

    #[test]
    fn a_whole_row_moved_while_its_new_container_also_changes_is_moved_by_its_row() {
        let storage = Container::Storage {
            planet: "Arkadia".into(),
        };
        let found = steps(
            &reading(
                0.0,
                0,
                &[
                    ("Animal Oil Residue", Container::Carried, 1_000, 10),
                    ("Animal Oil Residue", storage.clone(), 500, 5),
                ],
            ),
            &reading(
                60.0,
                0,
                &[
                    ("Animal Oil Residue", storage.clone(), 300, 3),
                    ("Animal Oil Residue", storage.clone(), 1_000, 10),
                ],
            ),
            &stack,
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(matches!(
            &found[0],
            Step::Moved {
                units: 1_000,
                from: Container::Carried,
                ..
            }
        ));
        assert!(
            matches!(&found[1], Step::Consumed { key, units: 200, .. } if key.container == storage)
        );
    }

    #[test]
    fn a_listing_sold_beside_a_new_listing_is_read_by_the_new_row() {
        let found = steps(
            &reading(
                0.0,
                0,
                &[
                    ("Animal Oil Residue", Container::Carried, 5_000, 50),
                    ("Animal Oil Residue", Container::Auction, 500, 5),
                ],
            ),
            &reading(
                60.0,
                0,
                &[
                    ("Animal Oil Residue", Container::Carried, 4_000, 40),
                    ("Animal Oil Residue", Container::Auction, 1_000, 10),
                ],
            ),
            &stack,
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(matches!(
            &found[0],
            Step::Moved {
                units: 1_000,
                from: Container::Carried,
                to: Container::Auction,
                ..
            }
        ));
        assert!(matches!(
            &found[1],
            Step::Consumed { key, units: 500, .. } if key.container == Container::Auction
        ));
    }

    #[test]
    fn a_gap_in_values_that_would_overflow_is_an_error() {
        let found = diff(
            &reading(0.0, 0, &[("Opalo", Container::Carried, 1, 0)]),
            &reading(60.0, 0, &[("Opalo", Container::Auction, 1, 0)]),
            &by_name,
        );
        assert!(found.is_ok());
        assert!(matches!(
            value_gap(&row(1, i64::MIN), &row(1, 1)),
            Err(AccountError::Overflow)
        ));
    }

    #[test]
    fn the_card_moving_is_a_step_of_its_own() {
        let found = steps(
            &reading(0.0, 25_000, &[]),
            &reading(60.0, 24_952, &[]),
            &stack,
        );
        assert_eq!(
            found,
            vec![Step::BalanceChanged {
                from: Pec(25_000),
                to: Pec(24_952)
            }]
        );
    }

    #[test]
    fn a_vanished_stack_is_consumed_and_a_vanished_item_left() {
        let found = steps(
            &reading(
                0.0,
                0,
                &[
                    ("Animal Oil Residue", Container::Carried, 40, 40),
                    ("Opalo", Container::Carried, 1, 500),
                ],
            ),
            &reading(60.0, 0, &[]),
            &by_name,
        );
        assert!(matches!(found[0], Step::Consumed { units: 40, .. }));
        assert!(matches!(found[1], Step::Left { units: 1, .. }));
    }

    fn spec_strategy() -> impl Strategy<Value = ReadingSpec> {
        let names = prop::sample::select(vec![
            "Universal Ammo",
            "Shrapnel",
            "Opalo",
            "Armor Plating Mk. 6A",
            "Gloves (M)",
        ]);
        let containers = prop::sample::select(vec![
            Container::Carried,
            Container::Auction,
            Container::Storage {
                planet: "Calypso".into(),
            },
            Container::Attached {
                parent: "Gloves (M)".into(),
            },
            Container::Other {
                name: "Estate".into(),
            },
        ]);
        (
            0i64..30_000,
            prop::collection::vec((names, containers, 0i64..2_000_000, 0i64..20_000), 0..12),
        )
            .prop_map(|(balance, rows)| ReadingSpec { balance, rows })
    }

    proptest! {
        /// A reading compared with itself has no steps.
        #[test]
        fn a_reading_has_no_steps_from_itself(spec in spec_strategy()) {
            let one = spec.reading(0.0);
            prop_assert!(steps(&one, &one, &by_name).is_empty());
            prop_assert!(steps(&one, &one, &stack).is_empty());
        }

        /// Applying every step's quantity change to the first reading's
        /// totals gives the second's, whatever the stacking.
        #[test]
        fn the_steps_carry_every_quantity_change(
            first in spec_strategy(),
            second in spec_strategy(),
            all_stack in any::<bool>(),
        ) {
            let before = first.reading(0.0);
            let after = second.reading(60.0);
            let stacking: &dyn Fn(&str) -> Stacking = if all_stack { &stack } else { &by_name };
            let set = diff(&before, &after, stacking).unwrap();
            let mut totals: BTreeMap<HoldingKey, i64> = before
                .holdings()
                .iter()
                .map(|(key, holding)| (key.clone(), holding.quantity()))
                .collect();
            for step in &set.steps {
                for (key, change) in step.quantity_changes() {
                    *totals.entry(key).or_default() += change;
                }
            }
            totals.retain(|_, quantity| *quantity != 0);
            let expected: BTreeMap<HoldingKey, i64> = after
                .holdings()
                .iter()
                .map(|(key, holding)| (key.clone(), holding.quantity()))
                .filter(|(_, quantity)| *quantity != 0)
                .collect();
            prop_assert_eq!(totals, expected);
            let balance_steps = set
                .steps
                .iter()
                .filter(|step| matches!(step, Step::BalanceChanged { .. }))
                .count();
            prop_assert_eq!(balance_steps, usize::from(before.balance() != after.balance()));
        }
    }
}
