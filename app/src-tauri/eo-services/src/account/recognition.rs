//! What one step between two readings did to the player's stock, read the
//! one way for every step, in a session or out of one.
//!
//! Every pattern closes exactly, on quantities and TT and never on markup:
//!
//! - **Shrapnel turned into Universal Ammo**: Shrapnel down, and a new ammo
//!   row of exactly what the Trade Terminal pays for it (the measure's own
//!   reading, [`super::measure::conversions_of`]).
//! - **Recycling into Nanocubes**: items leave and Nanocubes arrive worth
//!   the same TT, within a PEC for every row involved.
//! - **A sale to the Trade Terminal**: items leave and the PED Card rises
//!   by their TT, within a PEC for every row involved.
//! - **A purchase at auction** ([`purchase`]): one item arrives and the Card
//!   falls by more than its TT, with nothing else in the step but moves
//!   between the player's own places. It is only ever proposed, and only
//!   for a step outside every tracked session (the caller's to judge).
//!
//! What leaves the auction is a listing's outcome, and keys and strongboxes
//! leaving are an opening's, both read elsewhere: neither is a candidate
//! here, and a step that holds an opening records no recycling and no sale,
//! since what it produced may be what was recycled or sold, and the opening's
//! own flow records that. Universal Ammo is liquid and never a candidate.
//!
//! A step may close more than one way: two items of the same TT, one sold
//! and one recycled, say. Every way of closing the Card's rise by a set of
//! the items that left is weighed with every way of closing the Nanocubes'
//! arrival by another, disjoint set; what every way agrees on is
//! recognised, and the rest is left for the player. A side that no set
//! closes is read as not having happened (the Card moved for another
//! reason), never forced. Past [`MAX_BALANCE_EVALUATIONS`] ways nothing is
//! recognised for that step's sale and recycling, as an opening past it is
//! left unsettled.
//!
//! One shape is read without weighing every way: a Card rise with no
//! Nanocubes arriving, which everything that left closes together (a
//! Trade Terminal emptied of a whole bag). Any other closing set leaves out
//! only items whose combined TT fits in the slack the whole set has, so
//! every item worth more than that slack is in every closing set and is
//! read as sold; the rest is left. Exact, and however many items left.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::amount::{checked, count, Amount, Pec, MICRO_PER_PEC};
use super::composite::{recognise, MAX_BALANCE_EVALUATIONS};
use super::diff::{MoveKind, Step, StepSet};
use super::measure::conversions_of;
use super::reading::{Container, HoldingKey, Reading};
use super::roles::{is_strongbox, STRONGBOX_KEY, UNIVERSAL_AMMO};
use super::AccountError;

/// What the game's recycling turns items into.
pub const NANOCUBE: &str = "Nanocube";

/// Items of one name that left in a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOut {
    pub name: String,
    pub units: i64,
    /// Their TT, as the readings show it.
    pub amount: Amount,
}

/// Shrapnel the Trade Terminal turned into Universal Ammo in a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShrapnelConverted {
    pub shrapnel_units: i64,
    pub ammo_units: i64,
}

/// What a step did to the player's stock, as every closing way agrees.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recognised {
    pub shrapnel: Option<ShrapnelConverted>,
    /// Recycled into Nanocubes, one entry for each item.
    pub recycled: Vec<ItemOut>,
    /// Sold to the Trade Terminal, one entry for each item.
    pub sold: Vec<ItemOut>,
}

/// Something that left in a step that a sale or a recycling may take.
struct Candidate {
    name: String,
    units: i64,
    amount: Amount,
    /// The rows it was read from: a PEC each the site may have hidden.
    rows: usize,
    recyclable: bool,
}

/// What arrived or moved that a closing way must account for.
struct Target {
    amount: Amount,
    rows: usize,
}

/// Whether the candidates in `mask` close `target`: their TT is the
/// target's within a PEC for every row involved.
fn closes(candidates: &[Candidate], mask: u32, target: &Target) -> Result<bool, AccountError> {
    let mut sum: i64 = 0;
    let mut rows = target.rows;
    for (bit, candidate) in candidates.iter().enumerate() {
        if mask & (1 << bit) != 0 {
            sum = checked(sum.checked_add(candidate.amount.micro_ped()))?;
            rows += candidate.rows;
        }
    }
    let tolerance = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
    let gap = checked(
        sum.checked_sub(target.amount.micro_ped())
            .and_then(i64::checked_abs),
    )?;
    Ok(gap <= tolerance)
}

/// Every non-empty set of the candidates within `allowed` that closes
/// `target`; `None` past [`MAX_BALANCE_EVALUATIONS`].
fn closing_sets(
    candidates: &[Candidate],
    allowed: u32,
    target: &Target,
) -> Result<Option<Vec<u32>>, AccountError> {
    let n = candidates.len();
    if n >= 32 || (1usize << n) > MAX_BALANCE_EVALUATIONS {
        return Ok(None);
    }
    let mut found = Vec::new();
    for mask in 1..(1u32 << n) {
        if mask & !allowed == 0 && closes(candidates, mask, target)? {
            found.push(mask);
        }
    }
    Ok(Some(found))
}

/// The sets every closing way agrees are sold and recycled; `None` when
/// there are too many ways to weigh or no way closes both sides at once.
fn agreed(
    candidates: &[Candidate],
    rise: Option<&Target>,
    nanocubes: Option<&Target>,
) -> Result<Option<(u32, u32)>, AccountError> {
    // Past a mask's width nothing is weighed, and no mask is built.
    if candidates.len() > 31 {
        return Ok(None);
    }
    let all = if candidates.is_empty() {
        0
    } else {
        u32::MAX >> (32 - candidates.len().min(31))
    };
    let recyclable = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.recyclable)
        .fold(0u32, |mask, (bit, _)| mask | (1 << bit));
    // A side no set closes did not happen: it takes nothing.
    let side = |target: Option<&Target>, allowed: u32| match target {
        None => Ok(Some(vec![0])),
        Some(target) => {
            Ok::<_, AccountError>(closing_sets(candidates, allowed, target)?.map(|sets| {
                if sets.is_empty() {
                    vec![0]
                } else {
                    sets
                }
            }))
        }
    };
    let (Some(sales), Some(recyclings)) = (side(rise, all)?, side(nanocubes, recyclable)?) else {
        return Ok(None);
    };
    if sales.len().saturating_mul(recyclings.len()) > MAX_BALANCE_EVALUATIONS {
        return Ok(None);
    }
    let mut ways = 0usize;
    let (mut sold, mut recycled) = (u32::MAX, u32::MAX);
    for sale in &sales {
        for recycling in &recyclings {
            if sale & recycling == 0 {
                ways += 1;
                sold &= sale;
                recycled &= recycling;
            }
        }
    }
    Ok((ways > 0).then_some((sold, recycled)))
}

/// What a Card rise closed by everything that left, with nothing else to
/// close, agrees is sold: every candidate worth more than the slack the
/// whole set leaves. A closing set that leaves some out still reaches the
/// rise less its own tolerance, so what it leaves out is worth at most the
/// whole set's TT less the rise plus the whole set's tolerance; anything
/// worth more is in every closing set. `None` when the whole set does not
/// close the rise.
fn sold_in_every_way(
    candidates: &[Candidate],
    rise: &Target,
) -> Result<Option<Vec<bool>>, AccountError> {
    let mut sum: i64 = 0;
    let mut rows = rise.rows;
    for candidate in candidates {
        sum = checked(sum.checked_add(candidate.amount.micro_ped()))?;
        rows += candidate.rows;
    }
    let tolerance = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
    let over = checked(sum.checked_sub(rise.amount.micro_ped()))?;
    if checked(over.checked_abs())? > tolerance {
        return Ok(None);
    }
    let slack = checked(over.checked_add(tolerance))?;
    Ok(Some(
        candidates
            .iter()
            .map(|candidate| candidate.amount.micro_ped() > slack)
            .collect(),
    ))
}

/// The candidates `chosen` picks, one entry for each name.
fn items(
    candidates: &[Candidate],
    chosen: impl Fn(usize) -> bool,
) -> Result<Vec<ItemOut>, AccountError> {
    let mut by_name: BTreeMap<&str, (i64, Amount)> = BTreeMap::new();
    for (bit, candidate) in candidates.iter().enumerate() {
        if chosen(bit) {
            let entry = by_name
                .entry(candidate.name.as_str())
                .or_insert((0, Amount::NONE));
            entry.0 = checked(entry.0.checked_add(candidate.units))?;
            entry.1 = entry.1.plus(candidate.amount)?;
        }
    }
    Ok(by_name
        .into_iter()
        .map(|(name, (units, amount))| ItemOut {
            name: name.to_string(),
            units,
            amount,
        })
        .collect())
}

/// Whether what left under `key` may be sold or recycled: not from the
/// auction (a listing's outcome), not an opening's keys or boxes, not
/// Universal Ammo.
fn may_leave_so(key: &HoldingKey) -> bool {
    key.container != Container::Auction
        && key.name != UNIVERSAL_AMMO
        && key.name != STRONGBOX_KEY
        && !is_strongbox(&key.name)
}

/// What the step from `before` to `after` did to the player's stock. Pure.
pub fn recognise_step(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
) -> Result<Recognised, AccountError> {
    let (reserved, conversions) = conversions_of(set, before, after)?;
    let mut recognised = Recognised::default();
    if !conversions.is_empty() {
        let mut converted = ShrapnelConverted {
            shrapnel_units: 0,
            ammo_units: 0,
        };
        for conversion in &conversions {
            converted.shrapnel_units = checked(
                converted
                    .shrapnel_units
                    .checked_add(conversion.shrapnel_units),
            )?;
            converted.ammo_units =
                checked(converted.ammo_units.checked_add(conversion.ammo_units))?;
        }
        recognised.shrapnel = Some(converted);
    }
    // What an opening produced may be what was recycled or sold: its own
    // flow records that.
    if recognise(set, before, after, &reserved)?.opening.is_some() {
        return Ok(recognised);
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut rise: Option<Target> = None;
    let mut nanocubes: Option<Target> = None;
    for (index, step) in set.steps.iter().enumerate() {
        match step {
            Step::Consumed {
                key,
                units,
                amount,
                rows_touched,
                ..
            } if may_leave_so(key) && !conversions.iter().any(|c| c.index == index) => candidates
                .push(Candidate {
                    name: key.name.clone(),
                    units: *units,
                    amount: *amount,
                    rows: (*rows_touched).max(1),
                    recyclable: key.name != NANOCUBE,
                }),
            Step::Left { key, units, value } if may_leave_so(key) => candidates.push(Candidate {
                name: key.name.clone(),
                units: *units,
                amount: *value,
                rows: 1,
                recyclable: key.name != NANOCUBE,
            }),
            Step::Gained {
                key,
                amount,
                rows_touched,
                ..
            } if key.name == NANOCUBE && key.container != Container::Auction => {
                let target = nanocubes.get_or_insert(Target {
                    amount: Amount::NONE,
                    rows: 0,
                });
                target.amount = target.amount.plus(*amount)?;
                target.rows += (*rows_touched).max(1);
            }
            Step::BalanceChanged { from, to } if to.0 > from.0 => {
                rise = Some(Target {
                    amount: Amount::whole_pec(to.minus(*from)?)?,
                    rows: 0,
                });
            }
            _ => {}
        }
    }
    if candidates.is_empty() || (rise.is_none() && nanocubes.is_none()) {
        return Ok(recognised);
    }
    if let (Some(rise), None) = (&rise, &nanocubes) {
        if let Some(sold) = sold_in_every_way(&candidates, rise)? {
            recognised.sold = items(&candidates, |index| sold[index])?;
            return Ok(recognised);
        }
    }
    if let Some((sold, recycled)) = agreed(&candidates, rise.as_ref(), nanocubes.as_ref())? {
        recognised.sold = items(&candidates, |bit| sold & (1 << bit) != 0)?;
        recognised.recycled = items(&candidates, |bit| recycled & (1 << bit) != 0)?;
    }
    Ok(recognised)
}

/// An item that arrived for more than its TT: a purchase at auction, with
/// its markup paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Purchase {
    /// The item, as the account site names it, and where it arrived.
    pub key: HoldingKey,
    pub units: i64,
    /// Its TT, as the readings show it.
    pub amount: Amount,
    /// The PED Card's fall.
    pub paid: Pec,
    /// What it cost above its TT: the Card's fall less its TT.
    pub markup: Amount,
}

/// A purchase at auction in the step, when the step holds one and nothing
/// else: exactly one item arriving, the Card falling by more than its TT
/// beyond a PEC for every row of it, and otherwise only moves between the
/// player's own places. Universal Ammo is never bought so. Pure: whether
/// the step lies outside every tracked session is the caller's to judge.
pub fn purchase(set: &StepSet) -> Result<Option<Purchase>, AccountError> {
    let mut arrival: Option<(&HoldingKey, i64, Amount, usize)> = None;
    let mut paid: Option<Pec> = None;
    for step in &set.steps {
        let arrived = match step {
            Step::Gained {
                key,
                units,
                amount,
                rows_touched,
                ..
            } => (key, *units, *amount, (*rows_touched).max(1)),
            Step::Appeared { key, units, value } => (key, *units, *value, 1),
            Step::BalanceChanged { from, to } if to.0 < from.0 => {
                paid = Some(from.minus(*to)?);
                continue;
            }
            Step::Moved { from, to, .. } if MoveKind::of(from, to) == MoveKind::Transfer => {
                continue
            }
            _ => return Ok(None),
        };
        if arrival.replace(arrived).is_some() {
            return Ok(None);
        }
    }
    let (Some((key, units, amount, rows)), Some(paid)) = (arrival, paid) else {
        return Ok(None);
    };
    if key.name == UNIVERSAL_AMMO {
        return Ok(None);
    }
    let card = Amount::whole_pec(paid)?;
    let tolerance = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
    let beyond = checked(amount.micro_ped().checked_add(tolerance))?;
    if card.micro_ped() <= beyond {
        return Ok(None);
    }
    Ok(Some(Purchase {
        key: key.clone(),
        units,
        amount,
        paid,
        markup: card.minus(amount)?,
    }))
}
