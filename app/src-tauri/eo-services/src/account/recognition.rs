//! What one step between two readings did to the player's stock, read the
//! one way for every step, in a session or out of one.
//!
//! Every pattern closes exactly, on quantities and TT and never on markup:
//!
//! - **Shrapnel turned into Universal Ammo**: Shrapnel down, and a new ammo
//!   row of exactly what the Trade Terminal pays for it, or a row grown by
//!   exactly that much (the measure's own reading,
//!   [`super::measure::conversions_of`]). Shrapnel that left with neither,
//!   in a step Universal Ammo arrived in and that no sale or recycling
//!   takes, is presumed converted ([`Recognised::shrapnel_presumed`]): it
//!   closes nothing exactly, so whether to record it is the caller's to
//!   judge.
//! - **Recycling into Nanocubes**: items leave and Nanocubes arrive worth
//!   the same TT, within a PEC for every row involved.
//! - **A sale to the Trade Terminal**: items leave and the PED Card rises
//!   by their TT, within a PEC for every row involved.
//! - **A purchase** ([`purchase`]), at auction or from another player: one
//!   item arrives and the Card falls by more than its TT, with nothing else
//!   in the step but what is already explained exactly ([`beside_a_trade`]):
//!   wear, moves between the player's own places, and arrivals the chat log
//!   looted. It is only ever proposed, and only for a step outside every
//!   tracked session (the caller's to judge).
//! - **Boxes bought and opened in one step** ([`bought_and_opened`]):
//!   keys leave, one zero-TT strongbox changes by less than the keys used
//!   would open, and the Card falls. The boxes the keys opened beyond those
//!   that left were bought first: the purchase is read at the Card's fall,
//!   and the opening over the step as it would stand had the purchase come
//!   first. Only ever proposed, like a purchase.
//! - **A sale to another player** ([`sale_to_player`]): one item leaves and
//!   the Card rises by more than its TT, beside nothing but what
//!   [`beside_a_trade`] sets aside. A rise of exactly the TT is the Trade
//!   Terminal's, recorded above, and never this. Only ever proposed, like a
//!   purchase.
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
use super::composite::{recognise, Settlement, MAX_BALANCE_EVALUATIONS};
use super::diff::{diff, MoveKind, Step, StepSet};
use super::measure::{conversion_rows, conversions_of};
use super::reading::{Container, Holding, HoldingKey, Reading, Row};
use super::roles::{is_strongbox, Stacking, SHRAPNEL, STRONGBOX_KEY, UNIVERSAL_AMMO};
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
    /// Units of Shrapnel that left with no ammo row of exactly what they
    /// pay, in a step Universal Ammo arrived in beyond what the exact
    /// conversions paid, and that no sale or recycling takes: presumed
    /// converted. Never part of what makes a step whole.
    pub shrapnel_presumed: Option<i64>,
    /// Recycled into Nanocubes, one entry for each item.
    pub recycled: Vec<ItemOut>,
    /// Sold to the Trade Terminal, one entry for each item.
    pub sold: Vec<ItemOut>,
    /// Whether what is recognised is everything the step holds: every
    /// change in it is one of the patterns above, closed one way, with
    /// nothing left over but moves between the player's own places. Such a
    /// step holds no play the readings did not see, however long it is.
    pub whole: bool,
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
    // The step each candidate was read from.
    let mut candidate_steps: Vec<usize> = Vec::new();
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
            } if may_leave_so(key) && !conversions.iter().any(|c| c.index == index) => {
                candidate_steps.push(index);
                candidates.push(Candidate {
                    name: key.name.clone(),
                    units: *units,
                    amount: *amount,
                    rows: (*rows_touched).max(1),
                    recyclable: key.name != NANOCUBE,
                });
            }
            Step::Left { key, units, value } if may_leave_so(key) => {
                candidate_steps.push(index);
                candidates.push(Candidate {
                    name: key.name.clone(),
                    units: *units,
                    amount: *value,
                    rows: 1,
                    recyclable: key.name != NANOCUBE,
                });
            }
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
    // Which candidates are sold and which recycled, as every way agrees.
    let mut sold = vec![false; candidates.len()];
    let mut recycled = vec![false; candidates.len()];
    if !candidates.is_empty() && (rise.is_some() || nanocubes.is_some()) {
        let mut settled = false;
        if let (Some(rise), None) = (&rise, &nanocubes) {
            if let Some(every_way) = sold_in_every_way(&candidates, rise)? {
                sold = every_way;
                settled = true;
            }
        }
        if !settled {
            if let Some((sold_mask, recycled_mask)) =
                agreed(&candidates, rise.as_ref(), nanocubes.as_ref())?
            {
                for bit in 0..candidates.len() {
                    sold[bit] = sold_mask & (1 << bit) != 0;
                    recycled[bit] = recycled_mask & (1 << bit) != 0;
                }
            }
        }
        recognised.sold = items(&candidates, |index| sold[index])?;
        recognised.recycled = items(&candidates, |index| recycled[index])?;
    }
    recognised.shrapnel_presumed =
        presumed_converted(set, &conversions, &candidates, &sold, &recycled)?;
    recognised.whole = accounts_for_everything(
        set,
        &conversions,
        &candidate_steps,
        &sold,
        &recycled,
        &recognised,
    )?;
    Ok(recognised)
}

/// The Shrapnel among the candidates neither sold nor recycled, when
/// Universal Ammo arrived in the step beyond what the exact conversions
/// paid: presumed converted, however much of the payout was fired or
/// stacked away before the reading.
fn presumed_converted(
    set: &StepSet,
    conversions: &[super::measure::Conversion],
    candidates: &[Candidate],
    sold: &[bool],
    recycled: &[bool],
) -> Result<Option<i64>, AccountError> {
    let mut arrived: i64 = 0;
    for step in &set.steps {
        match step {
            Step::Gained { key, units, .. }
                if key.name == UNIVERSAL_AMMO && key.container != Container::Auction =>
            {
                arrived = checked(arrived.checked_add(*units))?;
            }
            Step::Consumed { key, units, .. }
                if key.name == UNIVERSAL_AMMO && key.container != Container::Auction =>
            {
                arrived = checked(arrived.checked_sub(*units))?;
            }
            _ => {}
        }
    }
    for conversion in conversions {
        arrived = checked(arrived.checked_sub(conversion.ammo_units))?;
    }
    if arrived <= 0 {
        return Ok(None);
    }
    let mut units: i64 = 0;
    for (at, candidate) in candidates.iter().enumerate() {
        if candidate.name == SHRAPNEL && !sold[at] && !recycled[at] {
            units = checked(units.checked_add(candidate.units))?;
        }
    }
    Ok((units > 0).then_some(units))
}

/// Whether the patterns recognised account for every change in the step:
/// each Shrapnel conversion with the ammo it paid and no other ammo, every
/// item that left sold or recycled, the Nanocubes' arrival only beside a
/// recycling and the Card's rise only beside a sale, and nothing else but
/// moves between the player's own places.
fn accounts_for_everything(
    set: &StepSet,
    conversions: &[super::measure::Conversion],
    candidate_steps: &[usize],
    sold: &[bool],
    recycled: &[bool],
    recognised: &Recognised,
) -> Result<bool, AccountError> {
    if recognised.shrapnel.is_none() && recognised.sold.is_empty() && recognised.recycled.is_empty()
    {
        return Ok(false);
    }
    let mut ammo_paid: i64 = 0;
    for conversion in conversions {
        ammo_paid = checked(ammo_paid.checked_add(conversion.ammo_units))?;
    }
    let mut ammo_gained: i64 = 0;
    for (index, step) in set.steps.iter().enumerate() {
        if conversions.iter().any(|c| c.index == index) {
            continue;
        }
        if let Some(at) = candidate_steps.iter().position(|step| *step == index) {
            if sold[at] || recycled[at] {
                continue;
            }
            return Ok(false);
        }
        let accounted = match step {
            Step::Gained { key, units, .. } if key.name == UNIVERSAL_AMMO => {
                ammo_gained = checked(ammo_gained.checked_add(*units))?;
                true
            }
            Step::Gained { key, .. } if key.name == NANOCUBE => !recognised.recycled.is_empty(),
            Step::BalanceChanged { from, to } => to.0 > from.0 && !recognised.sold.is_empty(),
            Step::Moved { from, to, .. } => MoveKind::of(from, to) == MoveKind::Transfer,
            _ => false,
        };
        if !accounted {
            return Ok(false);
        }
    }
    Ok(ammo_gained == ammo_paid)
}

/// An item that arrived for more than its TT: a purchase at auction or from
/// another player, with its markup paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Purchase {
    /// The item, as the account site names it, and where it arrived (where
    /// the most of it did, when it arrived in more than one place).
    pub key: HoldingKey,
    pub units: i64,
    /// Its TT, as the readings show it.
    pub amount: Amount,
    /// The PED Card's fall.
    pub paid: Pec,
    /// What it cost above its TT: the Card's fall less its TT.
    pub markup: Amount,
}

/// How late the account site may show play: what the chat log looted up to
/// this long before a step began may still land in that step, since the
/// site showed it late to the reading the step began with.
pub const LOOT_SHOWN_LATE_SECONDS: f64 = 120.0;

/// What the chat log looted around a step, by item name: from
/// [`LOOT_SHOWN_LATE_SECONDS`] before the step began to its end. The
/// caller reads it; the readings never guess it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Looted {
    /// Units on the loot lines still active: what explains an arrival.
    pub active: BTreeMap<String, i64>,
    /// Units on every loot line, deactivated or not: an item with any is
    /// never the one traded. The cautious side, so it counts every line.
    pub every: BTreeMap<String, i64>,
}

impl Looted {
    /// Nothing looted.
    pub fn new() -> Looted {
        Looted::default()
    }

    /// Whether the active loot lines account for `units` of `name`.
    fn explains(&self, name: &str, units: i64) -> bool {
        self.active.get(name).is_some_and(|looted| *looted >= units)
    }

    /// Whether any loot line, active or not, names `name`.
    fn any_of(&self, name: &str) -> bool {
        self.every.get(name).is_some_and(|looted| *looted > 0)
    }
}

/// What arrived, or left, under one name in a step, across its places.
struct Flow<'a> {
    /// Where the most of it arrived or left, and how many there.
    key: &'a HoldingKey,
    most: i64,
    units: i64,
    amount: Amount,
    /// The rows it was read from: a PEC each the site may have hidden.
    rows: usize,
}

fn add_flow<'a>(
    flows: &mut BTreeMap<&'a str, Flow<'a>>,
    key: &'a HoldingKey,
    units: i64,
    amount: Amount,
    rows: usize,
) -> Result<(), AccountError> {
    match flows.get_mut(key.name.as_str()) {
        Some(flow) => {
            flow.units = checked(flow.units.checked_add(units))?;
            flow.amount = flow.amount.plus(amount)?;
            flow.rows += rows;
            if units > flow.most {
                flow.key = key;
                flow.most = units;
            }
        }
        None => {
            flows.insert(
                key.name.as_str(),
                Flow {
                    key,
                    most: units,
                    units,
                    amount,
                    rows,
                },
            );
        }
    }
    Ok(())
}

/// What a step holds beside a trade, once what is already explained
/// exactly is set aside.
struct Beside<'a> {
    /// What arrived that the chat log does not explain, by name.
    arrived: BTreeMap<&'a str, Flow<'a>>,
    /// What left, by name.
    left: BTreeMap<&'a str, Flow<'a>>,
    /// The PED Card's balance before and after, when it moved.
    card: Option<(Pec, Pec)>,
}

/// What the step holds beside a trade, once what is already explained
/// exactly is set aside: wear (it never moves the Card), moves between the
/// player's own places, and every arrival of an item the chat log's active
/// loot lines in `looted` hold at least as many units of. An arrival only
/// partly explained is kept, and so is everything else. `None` when the
/// step holds something that refuses a trade's reading: a repair, a listing
/// or a listing's outcome (their fee or proceeds move the Card), a change
/// in Universal Ammo. What is left is the caller's to judge.
///
/// A known limit: an item looted inside the window and also bought in the
/// step, in no more units than were looted, is set aside as loot; the
/// purchase beside it is then never read as one (the readings cannot tell
/// the two apart).
fn beside_a_trade<'a>(
    set: &'a StepSet,
    looted: &Looted,
) -> Result<Option<Beside<'a>>, AccountError> {
    let mut beside = Beside {
        arrived: BTreeMap::new(),
        left: BTreeMap::new(),
        card: None,
    };
    for step in &set.steps {
        match step {
            Step::Decayed { .. } => {}
            Step::Moved { from, to, .. } if MoveKind::of(from, to) == MoveKind::Transfer => {}
            Step::BalanceChanged { from, to } => beside.card = Some((*from, *to)),
            Step::Gained { key, .. }
            | Step::Appeared { key, .. }
            | Step::Consumed { key, .. }
            | Step::Left { key, .. }
                if key.name == UNIVERSAL_AMMO || key.container == Container::Auction =>
            {
                return Ok(None)
            }
            Step::Gained {
                key,
                units,
                amount,
                rows_touched,
                ..
            } => add_flow(
                &mut beside.arrived,
                key,
                *units,
                *amount,
                (*rows_touched).max(1),
            )?,
            Step::Appeared { key, units, value } => {
                add_flow(&mut beside.arrived, key, *units, *value, 1)?
            }
            Step::Consumed {
                key,
                units,
                amount,
                rows_touched,
                ..
            } => add_flow(
                &mut beside.left,
                key,
                *units,
                *amount,
                (*rows_touched).max(1),
            )?,
            Step::Left { key, units, value } => add_flow(&mut beside.left, key, *units, *value, 1)?,
            Step::Repaired { .. } | Step::Moved { .. } => return Ok(None),
        }
    }
    beside
        .arrived
        .retain(|name, flow| !looted.explains(name, flow.units));
    Ok(Some(beside))
}

/// Whether the Card's move of `card` micro-PED is more than `flow`'s TT
/// beyond a PEC for every row of it.
fn beyond_its_tt(card: Amount, flow: &Flow) -> Result<bool, AccountError> {
    let tolerance = checked(count(flow.rows)?.checked_mul(MICRO_PER_PEC))?;
    let beyond = checked(flow.amount.micro_ped().checked_add(tolerance))?;
    Ok(card.micro_ped() > beyond)
}

/// A purchase in the step, at auction or from another player, when the
/// step holds one and nothing else that is not already explained
/// ([`beside_a_trade`]): one item name arriving (in any number of rows and
/// places: several trades of one item are one purchase), nothing leaving,
/// and the Card falling by more than its TT beyond a PEC for every row of
/// it. More than one name arriving has no one price, and an item any loot
/// line in `looted` names (active or not) is never bought so.
/// Universal Ammo is never bought so. Pure: whether the step lies outside
/// every tracked session is the caller's to judge.
pub fn purchase(set: &StepSet, looted: &Looted) -> Result<Option<Purchase>, AccountError> {
    let Some(beside) = beside_a_trade(set, looted)? else {
        return Ok(None);
    };
    let Some((from, to)) = beside.card.filter(|(from, to)| to.0 < from.0) else {
        return Ok(None);
    };
    if !beside.left.is_empty() {
        return Ok(None);
    }
    let mut arrived = beside.arrived.into_values();
    let (Some(flow), None) = (arrived.next(), arrived.next()) else {
        return Ok(None);
    };
    if looted.any_of(&flow.key.name) {
        return Ok(None);
    }
    let paid = from.minus(to)?;
    let card = Amount::whole_pec(paid)?;
    if !beyond_its_tt(card, &flow)? {
        return Ok(None);
    }
    Ok(Some(Purchase {
        key: flow.key.clone(),
        units: flow.units,
        amount: flow.amount,
        paid,
        markup: card.minus(flow.amount)?,
    }))
}

/// How many keys one strongbox takes to open: one, as the opening reading
/// settles only keys and boxes leaving in the same count.
pub const KEYS_PER_BOX: i64 = 1;

/// Strongboxes bought and opened in one step.
#[derive(Debug, Clone, PartialEq)]
pub struct BoughtAndOpened {
    /// The boxes bought, at the Card's fall.
    pub purchase: Purchase,
    /// How many boxes the keys used opened.
    pub opened: i64,
    /// The reading the step began with, as it would stand had the purchase
    /// come first: the boxes bought held beside what was, as a row of their
    /// own, and the Card already down by what they cost. The opening is
    /// read from it.
    pub before: Reading,
}

/// Boxes bought and opened in one step, read when the step holds only
/// that and what the opening produced: `Strongbox Key` leaving, exactly one
/// zero-TT strongbox changing in one place by less than the keys used open
/// ([`KEYS_PER_BOX`]) would take away, the Card falling, and otherwise only
/// wear, moves between the player's own places, and arrivals. Opened: the
/// keys used over [`KEYS_PER_BOX`]. Bought: the box's change plus the boxes
/// opened, at the Card's fall.
///
/// The opening is then read, by the one opening reading, over the step as
/// it would stand had the purchase come first (`stacking` reads its items
/// as the step's own diff does), and both stand only when that opening
/// settled on those boxes and keys and explains the rest of the step:
/// every arrival but the boxes bought is its ammo payout, one of its
/// contents or possible contents, or loot the chat log's active lines in
/// `looted` explain, and no more Universal Ammo arrived than it paid. An
/// arrival left unexplained (another purchase, ammo bought) would put its
/// price on the boxes, so it refuses both.
///
/// Anything else refuses it too: a second box changing (or the box in two
/// places), keys arriving, a repair, a listing or what leaves the auction,
/// Universal Ammo or any other stack used up, the Card rising or still or
/// falling by no more than the PEC a row the site may hide, and a box any
/// loot line in `looted` names. A step in which at least as many boxes left
/// as the keys open is an ordinary opening, never this. Nor is a step with
/// as many boxes bought as opened: the box's count did not change, so its
/// name is not in the step to be read. Pure: whether the step lies outside
/// every tracked session is the caller's to judge.
pub fn bought_and_opened(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    stacking: &dyn Fn(&str) -> Stacking,
    looted: &Looted,
) -> Result<Option<BoughtAndOpened>, AccountError> {
    let mut keys_used: i64 = 0;
    // The one box that changed: where, by how much, and from how many rows.
    let mut boxes: Option<(&HoldingKey, i64, usize)> = None;
    let mut paid: Option<Pec> = None;
    for step in &set.steps {
        match step {
            Step::Decayed { .. } => {}
            Step::Moved { from, to, .. } if MoveKind::of(from, to) == MoveKind::Transfer => {}
            Step::BalanceChanged { from, to } if to.0 < from.0 => paid = Some(from.minus(*to)?),
            Step::Gained { key, .. }
            | Step::Appeared { key, .. }
            | Step::Consumed { key, .. }
            | Step::Left { key, .. }
                if key.container == Container::Auction =>
            {
                return Ok(None)
            }
            Step::Consumed { key, units, .. } if key.name == STRONGBOX_KEY => {
                keys_used = checked(keys_used.checked_add(*units))?;
            }
            Step::Gained {
                key,
                units,
                amount,
                rows_touched,
                ..
            } if is_strongbox(&key.name) => {
                if amount.micro_ped() != 0 || boxes.replace((key, *units, *rows_touched)).is_some()
                {
                    return Ok(None);
                }
            }
            Step::Consumed {
                key,
                units,
                amount,
                rows_touched,
                ..
            } if is_strongbox(&key.name) => {
                let change = checked(units.checked_neg())?;
                if amount.micro_ped() != 0 || boxes.replace((key, change, *rows_touched)).is_some()
                {
                    return Ok(None);
                }
            }
            Step::Gained { key, .. } | Step::Appeared { key, .. }
                if key.name == STRONGBOX_KEY || is_strongbox(&key.name) =>
            {
                return Ok(None)
            }
            // The opening's payout or contents, or loot: weighed below.
            Step::Gained { .. } | Step::Appeared { .. } => {}
            _ => return Ok(None),
        }
    }
    let (Some((key, change, rows)), Some(paid)) = (boxes, paid) else {
        return Ok(None);
    };
    // Keys that open no whole number of boxes are no opening of these.
    let opened = checked(keys_used.checked_div(KEYS_PER_BOX))?;
    if keys_used <= 0 || opened.checked_mul(KEYS_PER_BOX) != Some(keys_used) {
        return Ok(None);
    }
    let bought = checked(change.checked_add(opened))?;
    if bought <= 0 || looted.any_of(&key.name) {
        return Ok(None);
    }
    let card = Amount::whole_pec(paid)?;
    let flow = Flow {
        key,
        most: bought,
        units: bought,
        amount: Amount::NONE,
        rows: rows.max(1),
    };
    if !beyond_its_tt(card, &flow)? {
        return Ok(None);
    }
    let mut holdings = before.holdings().clone();
    let mut box_rows: Vec<Row> = holdings
        .get(key)
        .map(|holding| holding.rows().to_vec())
        .unwrap_or_default();
    box_rows.push(Row {
        quantity: bought,
        value: Pec::ZERO,
    });
    holdings.insert(key.clone(), Holding::new(box_rows)?);
    let rebuilt = Reading::new(before.taken_at(), before.balance().minus(paid)?, holdings)?;
    if !opening_explains_the_rest(
        &rebuilt, after, stacking, &key.name, opened, keys_used, looted,
    )? {
        return Ok(None);
    }
    Ok(Some(BoughtAndOpened {
        purchase: Purchase {
            key: key.clone(),
            units: bought,
            amount: Amount::NONE,
            paid,
            markup: card,
        },
        opened,
        before: rebuilt,
    }))
}

/// Whether the step from `rebuilt` to `after` holds an opening of `opened`
/// of `box_name` with `keys_used` keys that settled, and explains every
/// arrival in it: its ammo payout and no more Universal Ammo, its contents
/// and possible contents, and loot the active lines in `looted` account
/// for.
fn opening_explains_the_rest(
    rebuilt: &Reading,
    after: &Reading,
    stacking: &dyn Fn(&str) -> Stacking,
    box_name: &str,
    opened: i64,
    keys_used: i64,
    looted: &Looted,
) -> Result<bool, AccountError> {
    let set = diff(rebuilt, after, stacking)?;
    let reserved = conversion_rows(&set, rebuilt, after)?;
    let composites = recognise(&set, rebuilt, after, &reserved)?;
    let Some(opening) = composites.opening else {
        return Ok(false);
    };
    let one_box =
        matches!(opening.boxes.as_slice(), [only] if only.name == box_name && only.count == opened);
    if opening.settlement != Settlement::Settled || !one_box || opening.keys_used != keys_used {
        return Ok(false);
    }
    let mut ammo_arrived: i64 = 0;
    let mut unclaimed: BTreeMap<&str, i64> = BTreeMap::new();
    for (index, step) in set.steps.iter().enumerate() {
        let (key, units) = match step {
            Step::Gained { key, units, .. } | Step::Appeared { key, units, .. } => (key, *units),
            _ => continue,
        };
        if key.name == UNIVERSAL_AMMO {
            ammo_arrived = checked(ammo_arrived.checked_add(units))?;
        } else if !composites.claims.contains_key(&index) {
            let entry = unclaimed.entry(key.name.as_str()).or_insert(0);
            *entry = checked(entry.checked_add(units))?;
        }
    }
    Ok(ammo_arrived == opening.ammo_units
        && unclaimed
            .iter()
            .all(|(name, units)| looted.explains(name, *units)))
}

/// An item that left for more than its TT: a sale to another player, with
/// the markup it fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaleToPlayer {
    /// The item, as the account site names it, and where it left from
    /// (where the most of it did, when it left more than one place).
    pub key: HoldingKey,
    pub units: i64,
    /// Its TT, as the readings show it.
    pub amount: Amount,
    /// The PED Card's rise.
    pub received: Pec,
    /// What it fetched above its TT: the Card's rise less its TT.
    pub markup: Amount,
}

/// A sale to another player in the step, when the step holds one and
/// nothing else that is not already explained ([`beside_a_trade`]): one
/// item name leaving (in any number of rows and places), nothing arriving,
/// and the Card rising by more than its TT beyond a PEC for every row of
/// it. A rise within that is the Trade Terminal's sale, read by
/// [`recognise_step`] and never this. More than one name leaving has no
/// one price. Never an item any loot line in `looted` names, active or not
/// (loot landing on the stack it left from hides how much left), nor beside a
/// strongbox opening (keys leaving alone are no opening, and may be sold
/// so). Universal Ammo and what leaves the auction never leave so. Pure:
/// whether the step lies outside every tracked session is the caller's to
/// judge.
pub fn sale_to_player(
    set: &StepSet,
    before: &Reading,
    after: &Reading,
    looted: &Looted,
) -> Result<Option<SaleToPlayer>, AccountError> {
    let Some(beside) = beside_a_trade(set, looted)? else {
        return Ok(None);
    };
    let Some((from, to)) = beside.card.filter(|(from, to)| to.0 > from.0) else {
        return Ok(None);
    };
    if !beside.arrived.is_empty() {
        return Ok(None);
    }
    let mut left = beside.left.into_values();
    let (Some(flow), None) = (left.next(), left.next()) else {
        return Ok(None);
    };
    if looted.any_of(&flow.key.name) {
        return Ok(None);
    }
    let reserved = conversion_rows(set, before, after)?;
    if recognise(set, before, after, &reserved)?.opening.is_some() {
        return Ok(None);
    }
    let received = to.minus(from)?;
    let card = Amount::whole_pec(received)?;
    if !beyond_its_tt(card, &flow)? {
        return Ok(None);
    }
    Ok(Some(SaleToPlayer {
        key: flow.key.clone(),
        units: flow.units,
        amount: flow.amount,
        received,
        markup: card.minus(flow.amount)?,
    }))
}
