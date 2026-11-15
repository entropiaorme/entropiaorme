//! Patterns over one step's changes: a repair and a strongbox opening.
//!
//! A repair is the restorations of one step with the PED Card's fall
//! beside them. The fall is the repair's cost only when nothing else in
//! the step can move the Card and the fall lies within what the readings
//! allow (the restored total, less at most one PEC per item restored);
//! otherwise the cost is unknown, never a guess.
//!
//! A strongbox opening is `Strongbox Key` down by some count and exactly
//! one zero-TT strongbox down by the same count. A key is a deposit of its
//! TT and the box pays the TT back, mostly as Universal Ammo, each box's
//! ammo arriving as a row of its own. The opening claims only what that
//! balance claims: among the step's arrivals it takes as its contents the
//! ones whose TT, with the ammo in new rows, pays the keys back within one
//! PEC per output row. When the ammo rows allow more than one reading of
//! which rows are new, the keys decide between them. When neither the
//! contents nor the ammo can be settled on one reading, the opening is
//! reported unsettled, claims nothing, and every arrival of its step is
//! marked as possibly its contents: nothing is forced to balance, and
//! nothing that arrived is dropped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, Pec};
use super::diff::{exact_unit, new_row_readings, MoveKind, NewRows, Step, StepSet};
use super::reading::{Holding, HoldingKey, Reading, Row};
use super::roles::UNIVERSAL_AMMO;
use super::AccountError;
use crate::inventory_changes::STRONGBOX_KEY;

/// The most arrivals with TT an opening weighs as possible contents; with
/// more, it claims none.
pub const MAX_CONTENT_CANDIDATES: usize = 16;

/// One item's share of a figure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemAmount {
    pub key: HoldingKey,
    pub amount: Amount,
}

/// The restorations of one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repair {
    pub items: Vec<ItemAmount>,
    /// The sum of the items' whole-PEC rises.
    pub restored: Amount,
    /// What the PED Card paid, when nothing else in the step could have
    /// moved it and the fall lies within what the restorations allow.
    pub cost: Option<Pec>,
}

/// Whether an opening's balance closed on one reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Settlement {
    /// One reading of its ammo and contents pays the keys back within one
    /// PEC per output row.
    Settled,
    /// None does, or more than one does.
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

/// A run of strongbox openings within one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opening {
    pub box_name: String,
    pub boxes: i64,
    /// The TT of the keys used: the deposit the boxes pay back.
    pub keys: Amount,
    /// Universal Ammo in rows new in the step: the reading the keys chose
    /// when settled, the canonical reading of the rows otherwise.
    pub ammo_units: i64,
    pub ammo_rows: usize,
    pub ammo_from_rows: Amount,
    /// The keys' TT less the TT of the contents claimed.
    pub ammo_from_keys: Amount,
    /// What the opening claims besides ammo; nothing when unsettled.
    pub contents: Vec<OpenedContent>,
    pub settlement: Settlement,
}

/// What an opening says of one arrival in its step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Part of the opening's contents.
    Content,
    /// Could not be placed: possibly the opening's contents, possibly not.
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

fn is_strongbox(name: &str) -> bool {
    name.trim().to_ascii_lowercase().ends_with("strongbox")
}

/// Recognise the repair and the strongbox opening in one step from
/// `before` to `after`.
pub fn recognise(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<Composites, AccountError> {
    let mut composites = Composites {
        repair: repair(set)?,
        ..Composites::default()
    };
    if let Some((opening, claims)) = opening(set, before, after)? {
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
    let fall = set.steps.iter().find_map(|step| match step {
        Step::BalanceChanged { from, to } if to < from => Some(from.minus(*to)),
        _ => None,
    });
    let fall = fall.transpose()?;
    let card_explained_alone = !set.steps.iter().any(can_move_the_card);
    let cost = match fall {
        Some(fall) if card_explained_alone => {
            let fall = Amount::whole_pec(fall)?;
            let within = fall.micro_ped() <= restored.micro_ped()
                && fall.agrees_with(restored, items.len())?;
            within.then(|| Pec(fall.micro_ped() / super::amount::MICRO_PER_PEC))
        }
        _ => None,
    };
    Ok(Some(Repair {
        items,
        restored,
        cost,
    }))
}

/// Whether a step could move the PED Card besides a repair: a stack
/// leaving with TT (sold), an item leaving (sold), something arriving with
/// TT (bought), or a listing made or returned (its fee).
fn can_move_the_card(step: &Step) -> bool {
    match step {
        Step::Consumed { amount, .. } | Step::Gained { amount, .. } => amount.micro_ped() != 0,
        Step::Left { .. } => true,
        Step::Appeared { value, .. } => value.micro_ped() != 0,
        Step::Moved { from, to, .. } => MoveKind::of(from, to) != MoveKind::Transfer,
        Step::Decayed { .. } | Step::Repaired { .. } | Step::BalanceChanged { .. } => false,
    }
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

/// Every reading of the Universal Ammo rows new in the step, across the
/// holdings that changed; `None` when there are too many to weigh.
fn ammo_readings(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<Option<Vec<AmmoReading>>, AccountError> {
    let mut combined = vec![AmmoReading {
        units: 0,
        rows: 0,
        amount: Amount::NONE,
    }];
    for step in &set.steps {
        let (Step::Consumed { key, .. } | Step::Gained { key, .. }) = step else {
            continue;
        };
        if key.name != UNIVERSAL_AMMO {
            continue;
        }
        let was = rows_of(before.holding(key));
        let now = rows_of(after.holding(key));
        let unit = exact_unit(was.iter().chain(now));
        let Some(readings) = new_row_readings(was, now) else {
            return Ok(None);
        };
        let mut next = Vec::new();
        for held in &combined {
            for rows in &readings {
                let new_rows = NewRows::of(rows.clone(), unit)?;
                let reading = AmmoReading {
                    units: super::amount::checked(held.units.checked_add(new_rows.units))?,
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
fn canonical_ammo(set: &StepSet) -> Result<AmmoReading, AccountError> {
    let mut reading = AmmoReading {
        units: 0,
        rows: 0,
        amount: Amount::NONE,
    };
    for step in &set.steps {
        if let Step::Consumed { key, new_rows, .. } | Step::Gained { key, new_rows, .. } = step {
            if key.name == UNIVERSAL_AMMO {
                reading.units = super::amount::checked(reading.units.checked_add(new_rows.units))?;
                reading.rows += new_rows.rows.len();
                reading.amount = reading.amount.plus(new_rows.amount)?;
            }
        }
    }
    Ok(reading)
}

/// Whether one reading of the ammo with these contents pays the keys back.
fn closes(keys: Amount, ammo: &AmmoReading, contents: &[&Arrival]) -> Result<bool, AccountError> {
    let produced = Amount::total(
        std::iter::once(ammo.amount).chain(contents.iter().map(|arrival| arrival.amount)),
    )?;
    let rows = ammo.rows + contents.iter().map(|arrival| arrival.rows).sum::<usize>();
    produced.agrees_with(keys, rows)
}

type Solution<'a> = (AmmoReading, Vec<&'a Arrival>);

/// What an opening says of arrivals, by their position in the step.
type Claims = BTreeMap<usize, Claim>;

fn opening(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<Option<(Opening, Claims)>, AccountError> {
    let mut keys_used: i64 = 0;
    let mut keys = Amount::NONE;
    let mut boxes: Vec<(&str, i64)> = Vec::new();
    for step in &set.steps {
        if let Step::Consumed {
            key, units, amount, ..
        } = step
        {
            if key.name == STRONGBOX_KEY {
                keys_used = super::amount::checked(keys_used.checked_add(*units))?;
                keys = keys.plus(*amount)?;
            } else if is_strongbox(&key.name) && amount.micro_ped() == 0 {
                boxes.push((key.name.as_str(), *units));
            }
        }
    }
    let [(box_name, opened)] = boxes.as_slice() else {
        return Ok(None);
    };
    if keys_used == 0 || *opened != keys_used {
        return Ok(None);
    }

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

    let mut chosen: Option<Solution> = None;
    if let Some(readings) = ammo_readings(set, before, after)? {
        // Every arrival as contents first: the opening alone in its step.
        let everything: Vec<&Arrival> = arrivals.iter().collect();
        let mut whole = Vec::new();
        for ammo in &readings {
            if closes(keys, ammo, &everything)? {
                whole.push((ammo.clone(), everything.clone()));
            }
        }
        if whole.len() == 1 {
            chosen = whole.pop();
        } else if whole.is_empty() {
            // Otherwise the one subset of the arrivals with TT that closes
            // the balance, when exactly one does.
            let with_tt: Vec<&Arrival> = arrivals
                .iter()
                .filter(|arrival| arrival.amount.micro_ped() != 0)
                .collect();
            if with_tt.len() <= MAX_CONTENT_CANDIDATES {
                let mut solutions: Vec<Solution> = Vec::new();
                for mask in 0u32..(1u32 << with_tt.len()) {
                    let subset: Vec<&Arrival> = with_tt
                        .iter()
                        .enumerate()
                        .filter(|(bit, _)| mask & (1 << bit) != 0)
                        .map(|(_, arrival)| *arrival)
                        .collect();
                    for ammo in &readings {
                        if closes(keys, ammo, &subset)? {
                            solutions.push((ammo.clone(), subset.clone()));
                        }
                    }
                    if solutions.len() > 1 {
                        break;
                    }
                }
                if solutions.len() == 1 {
                    chosen = solutions.pop();
                }
            }
        }
    }

    let mut claims = BTreeMap::new();
    let (ammo, contents, settlement) = match chosen {
        Some((ammo, contents)) => {
            for arrival in &arrivals {
                let claimed = contents
                    .iter()
                    .any(|content| content.index == arrival.index);
                if claimed {
                    claims.insert(arrival.index, Claim::Content);
                } else if arrival.amount.micro_ped() == 0 {
                    claims.insert(arrival.index, Claim::Possibly);
                }
            }
            (ammo, contents, Settlement::Settled)
        }
        None => {
            for arrival in &arrivals {
                claims.insert(arrival.index, Claim::Possibly);
            }
            (canonical_ammo(set)?, Vec::new(), Settlement::Unsettled)
        }
    };
    let ammo_from_keys = keys.minus(Amount::total(
        contents.iter().map(|content| content.amount),
    )?)?;
    let opening = Opening {
        box_name: (*box_name).to_string(),
        boxes: *opened,
        keys,
        ammo_units: ammo.units,
        ammo_rows: ammo.rows,
        ammo_from_rows: ammo.amount,
        ammo_from_keys,
        contents: contents
            .iter()
            .map(|content| OpenedContent {
                key: content.key.clone(),
                units: content.units,
                amount: content.amount,
            })
            .collect(),
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
        recognise(&diff(before, after, &stacking).unwrap(), before, after).unwrap()
    }

    const ARM: &str = "Paneleon Arm Guards (M)";

    type Rows<'a> = &'a [(&'static str, Container, i64, i64)];

    /// A repair of 7 PEC on one item, the Card falling by `fall`, with
    /// extra rows before and after.
    fn repair_with(fall: i64, before_extra: Rows, after_extra: Rows) -> Option<Repair> {
        let mut before_rows = vec![(ARM, Container::Carried, 1, 2_143)];
        before_rows.extend_from_slice(before_extra);
        let mut after_rows = vec![(ARM, Container::Carried, 1, 2_150)];
        after_rows.extend_from_slice(after_extra);
        recognised(
            &reading(0.0, 25_000, &before_rows),
            &reading(60.0, 25_000 - fall, &after_rows),
        )
        .repair
    }

    #[test]
    fn a_card_fall_beside_restorations_is_the_repair_s_cost() {
        let repair = repair_with(7, &[], &[]).expect("a repair");
        assert_eq!(repair.restored, pec(7));
        assert_eq!(repair.cost, Some(Pec(7)));
        assert_eq!(repair.items.len(), 1);
        // Within one PEC per item restored below the restored total.
        assert_eq!(repair_with(6, &[], &[]).unwrap().cost, Some(Pec(6)));
    }

    #[test]
    fn a_fall_outside_what_the_restorations_allow_is_no_known_cost() {
        assert_eq!(
            repair_with(8, &[], &[]).unwrap().cost,
            None,
            "more than restored"
        );
        assert_eq!(
            repair_with(5, &[], &[]).unwrap().cost,
            None,
            "more than a PEC an item below"
        );
        assert_eq!(
            repair_with(0, &[], &[]).unwrap().cost,
            None,
            "no fall at all"
        );
        assert_eq!(
            repair_with(-3, &[], &[]).unwrap().cost,
            None,
            "the Card rose"
        );
    }

    #[test]
    fn every_other_way_the_card_can_move_leaves_the_repair_s_cost_unknown() {
        let carried = || Container::Carried;
        // A stack consumed with TT (sold).
        assert_eq!(
            repair_with(7, &[("Oil", carried(), 40, 40)], &[])
                .unwrap()
                .cost,
            None
        );
        // A stack gained with TT (bought).
        assert_eq!(
            repair_with(7, &[], &[("Oil", carried(), 40, 40)])
                .unwrap()
                .cost,
            None
        );
        // An item that left.
        assert_eq!(
            repair_with(7, &[("Pants (F)", carried(), 1, 0)], &[])
                .unwrap()
                .cost,
            None
        );
        // An item that arrived with TT.
        assert_eq!(
            repair_with(7, &[], &[("Pants (F)", carried(), 1, 1_000)])
                .unwrap()
                .cost,
            None
        );
        // A listing made, and one returned.
        assert_eq!(
            repair_with(
                7,
                &[("Coat (M)", carried(), 1, 100)],
                &[("Coat (M)", Container::Auction, 1, 100)]
            )
            .unwrap()
            .cost,
            None
        );
        assert_eq!(
            repair_with(
                7,
                &[("Coat (M)", Container::Auction, 1, 100)],
                &[("Coat (M)", carried(), 1, 100)]
            )
            .unwrap()
            .cost,
            None
        );
        // What cannot move the Card leaves the cost known: an arrival of no
        // TT, and a move into storage.
        assert_eq!(
            repair_with(7, &[], &[("Mayhem Token", carried(), 1, 0)])
                .unwrap()
                .cost,
            Some(Pec(7))
        );
        assert_eq!(
            repair_with(
                7,
                &[("Coat (M)", carried(), 1, 100)],
                &[(
                    "Coat (M)",
                    Container::Storage {
                        planet: "Calypso".into()
                    },
                    1,
                    100
                )]
            )
            .unwrap()
            .cost,
            Some(Pec(7))
        );
    }

    #[test]
    fn a_step_without_restorations_holds_no_repair() {
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
                ("Halloween Strongbox", Container::Carried, 23, 0),
                ("Strongbox Key", Container::Carried, 24, 24_000),
                ("Universal Ammo", Container::Carried, 443_923, 4_439),
            ],
        );
        let mut after = vec![
            ("Halloween Strongbox", Container::Carried, 16, 0),
            ("Strongbox Key", Container::Carried, 17, 17_000),
            ("Universal Ammo", Container::Carried, 443_923, 4_439),
        ];
        for (quantity, value) in ammo_rows {
            after.push(("Universal Ammo", Container::Carried, *quantity, *value));
        }
        after.extend_from_slice(extra);
        recognised(&before, &reading(60.0, 25_000, &after))
    }

    const PANTS: (&str, Container, i64, i64) =
        ("Skeleton Pants Red (F)", Container::Carried, 1, 1_000);

    #[test]
    fn keys_and_one_box_down_together_are_an_opening_read_from_its_new_ammo_rows() {
        let found = opening_of(&[(100_000, 1_000); 6], &[PANTS]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.box_name, "Halloween Strongbox");
        assert_eq!(opening.boxes, 7);
        assert_eq!(opening.keys, pec(7_000));
        assert_eq!(opening.ammo_units, 600_000);
        assert_eq!(opening.ammo_rows, 6);
        assert_eq!(opening.ammo_from_rows, Amount::exact(60_000_000));
        assert_eq!(opening.ammo_from_keys.micro_ped(), 60_000_000);
        assert_eq!(opening.contents.len(), 1);
        assert_eq!(opening.contents[0].amount, pec(1_000));
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
            "an unsettled opening claims nothing"
        );
        assert_eq!(opening.ammo_from_rows, Amount::exact(59_920_000));
        assert_eq!(
            found.claims.values().collect::<Vec<_>>(),
            vec![&Claim::Possibly]
        );
    }

    #[test]
    fn loot_in_the_opening_s_step_stays_loot_when_one_subset_closes_the_balance() {
        let shrapnel = ("Shrapnel", Container::Carried, 281_816, 2_818);
        let token = ("Mayhem Token", Container::Carried, 1, 0);
        let found = opening_of(
            &[(100_000, 1_000); 6],
            &[PANTS, shrapnel.clone(), token.clone()],
        );
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.contents.len(), 1);
        assert_eq!(opening.contents[0].key.name, "Skeleton Pants Red (F)");
        let claims: Vec<Claim> = found.claims.values().copied().collect();
        assert_eq!(claims.len(), 2, "the shrapnel is not the opening's at all");
        assert!(claims.contains(&Claim::Content));
        assert!(
            claims.contains(&Claim::Possibly),
            "the token cannot be placed"
        );
    }

    #[test]
    fn loot_of_no_tt_beside_a_pure_ammo_opening_is_all_contents() {
        let token = ("Mayhem Token", Container::Carried, 1, 0);
        let found = opening_of(&[(100_000, 1_000); 7], &[token]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(opening.contents.len(), 1);
    }

    #[test]
    fn when_two_subsets_close_the_balance_the_opening_is_unsettled_and_claims_nothing() {
        let a = ("Skeleton Shirt (F)", Container::Carried, 1, 1_000);
        let shrapnel = ("Shrapnel", Container::Carried, 100_000, 1_000);
        let found = opening_of(&[(100_000, 1_000); 6], &[a, shrapnel]);
        let opening = found.opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
        assert!(opening.contents.is_empty());
        assert!(found.claims.values().all(|claim| *claim == Claim::Possibly));
        assert_eq!(found.claims.len(), 2);
    }

    #[test]
    fn a_vanished_small_row_does_not_pair_with_a_large_new_one() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 2, 0),
                ("Strongbox Key", Container::Carried, 2, 2_000),
                ("Universal Ammo", Container::Carried, 5_000, 50),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 1, 0),
                ("Strongbox Key", Container::Carried, 1, 1_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
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
                ("Halloween Strongbox", Container::Carried, 2, 0),
                ("Strongbox Key", Container::Carried, 2, 2_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 1, 0),
                ("Strongbox Key", Container::Carried, 1, 1_000),
                ("Universal Ammo", Container::Carried, 95_100, 951),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
            ],
        );
        let opening = recognised(&before, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Settled);
        assert_eq!(
            opening.ammo_units, 100_000,
            "not the 95100 the identical rows suggest"
        );
        // Two keys' worth of new rows: both readings are short, so neither
        // settles.
        let two_keys = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 3, 0),
                ("Strongbox Key", Container::Carried, 3, 3_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
                ("Universal Ammo", Container::Carried, 100_000, 1_000),
            ],
        );
        let opening = recognised(&two_keys, &after).opening.expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
    }

    #[test]
    fn keys_without_a_matching_box_count_and_two_kinds_of_box_are_no_opening() {
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 23, 0),
                ("Strongbox Key", Container::Carried, 24, 24_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 17, 0),
                ("Strongbox Key", Container::Carried, 17, 17_000),
            ],
        );
        assert_eq!(recognised(&before, &after).opening, None);
        let before = reading(
            0.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 23, 0),
                ("Easter Strongbox", Container::Carried, 5, 0),
                ("Strongbox Key", Container::Carried, 24, 24_000),
            ],
        );
        let after = reading(
            60.0,
            0,
            &[
                ("Halloween Strongbox", Container::Carried, 22, 0),
                ("Easter Strongbox", Container::Carried, 4, 0),
                ("Strongbox Key", Container::Carried, 23, 23_000),
            ],
        );
        assert_eq!(recognised(&before, &after).opening, None);
        let set = StepSet {
            span: Span { from: 0.0, to: 1.0 },
            steps: vec![],
        };
        let empty = reading(0.0, 0, &[]);
        assert_eq!(
            recognise(&set, &empty, &empty).unwrap(),
            Composites::default()
        );
    }
}
