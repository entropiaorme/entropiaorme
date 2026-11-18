//! What the readings propose: a strongbox opening, a listing sold at a
//! price the player says, a listing outcome read from a long step, or a
//! purchase at auction, seen in the step between two consecutive readings.
//!
//! A proposal is a guess. The app never records one on its own: it fills
//! in the existing strongbox review, listing confirmation or asset purchase,
//! and the player records it through the existing commands or dismisses
//! it. Nothing here writes or sums an accounting figure. The strongbox
//! openings, the auction listings and the asset purchases are read, never
//! written:
//! to match a listing, and to tell whether the record a proposal became
//! still stands. What the readings settle leaves nothing to guess and is no
//! proposal: a Shrapnel conversion, a recycling, a sale to the Trade
//! Terminal, a listing that came back, and a listing sold at the one price
//! that fits it ([`settled`]) are recorded on its own (see
//! [`super::follow`]).
//!
//! Each stored reading is examined once, against the reading stored before
//! it: what the step proposes is worked out on a reader ([`prepare`]) and
//! stored with the reading's examined mark in one short write ([`commit`]).
//! [`examine`] is the one pass, for a reading just stored and for those a
//! previous run left. A reading stored as the app closed is examined at the
//! next start; the
//! readings stored before proposals existed were marked examined by the
//! migration that brought them, so history is never offered. A step whose
//! own content cannot be read is passed, its reading marked examined; a
//! database error leaves the reading to be examined again. Examining a
//! reading again adds nothing: there is one proposal per kind, reading and
//! subject.
//!
//! - **An opening** is proposed whenever a step holds one, settled or not.
//!   An unsettled one carries why it did not settle and no ammo figure;
//!   what arrived that may be its contents is listed apart, never claimed.
//! - **Openings seen across consecutive steps outside a session** (the
//!   readings taken while signed in and otherwise idle, or for a strongbox
//!   run) are read as one, and so is a single such step whose opening the
//!   step-by-step reading cannot settle. When a reading's step held nothing but an
//!   opening, and so did the steps before it back to where the opening
//!   began, each still proposed and neither decided nor claimed, the whole
//!   span is read as one opening (see [`super::composite::whole_opening`]).
//!   If it balances, one settled proposal across the span (at most
//!   [`WHOLE_OPENING_STEPS`] steps) takes the place of the step's own and
//!   of every proposal it spans, which are stored all the same and no
//!   longer offered; if not, the step is proposed on its own as before. A
//!   span whose earlier proposals were decided or claimed meanwhile is not
//!   read whole when it is stored. An opening recorded another way that is
//!   only part of one read whole turns it back into its steps (see
//!   [`pending`]).
//! - **A purchase at auction** is proposed when a step outside every
//!   tracked session holds one item arriving for more than its TT and
//!   nothing else ([`super::recognition::purchase`]), and the chat log of no
//!   session looted that item inside the step. Its markup is what the
//!   player paid above TT, so it is never recorded on its own.
//! - **A listing outcome** is proposed only when exactly one of the
//!   player's listings on auction from before the step matches it by item
//!   name and quantity (and TT, where the listing has one), the departure
//!   is the only change of that item's quantity in the step, and the step
//!   is not one whose settled outcomes are recorded on their own
//!   ([`super::follow::settles_listings`]) or its outcome is not settled.
//!   Sold: the item left the auction with no return to another place. The
//!   PED Card's rise is offered only when the Card rose in that same step,
//!   nothing else in the step could move the Card, and the two readings
//!   are at most [`PROCEEDS_WITHIN_SECONDS`] apart. Returned: exactly what
//!   left the auction arrived in another place, so it expired or was
//!   withdrawn.
//!
//! Recording an opening from proposals is three steps, so an interruption
//! is visible rather than silent: [`claim`] the proposals, record the
//! opening through its own command, then [`link`] them to it as one (or
//! [`release`] them if the recording did not happen). A claim with no link
//! keeps the proposals listed, marked as possibly recorded already.
//!
//! Whether a proposal is open is read at the moment of reading from the
//! record it names: a recorded opening taken back, or a listing's sale
//! reverted, opens it again; a listing resolved by hand settles it. An
//! opening recorded another way (pasted or typed) accounts for at most one
//! group of open proposals of its box, as [`pending`] describes.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::amount::{Amount, Pec, Precision, MICRO_PER_PEC, MICRO_PER_PED};
use super::composite::{
    new_ammo_rows, only_an_opening, recognise, whole_opening, NewAmmoRow, OpenedBoxes,
    OpenedContent, Opening, Settlement,
};
use super::diff::{diff, MoveKind, Span, Step, StepSet};
use super::measure::conversion_rows;
use super::reading::{Container, HoldingKey, Reading};
use super::recognition::{purchase, Purchase};
use super::roles::{RoleBook, RoleSource};
use super::store::{self, Purpose, ReadingId, StoredReading};
use super::AccountError;

/// The version of a stored proposal's payload. A payload of another
/// version is not offered.
pub const PAYLOAD_VERSION: i64 = 1;

/// The longest step whose PED Card rise is offered as a sale's proceeds.
pub const PROCEEDS_WITHIN_SECONDS: f64 = 180.0;

/// The most steps (pairs of consecutive readings) an opening is read whole
/// across.
pub const WHOLE_OPENING_STEPS: i64 = 60;

/// What a proposal is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalKind {
    Opening,
    ListingSold,
    ListingReturned,
    Purchase,
    /// Something that arrived during a tracked session that no loot line,
    /// recorded reward, conversion or opening explains.
    Arrival,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalKind::Opening => "opening",
            ProposalKind::ListingSold => "listing_sold",
            ProposalKind::ListingReturned => "listing_returned",
            ProposalKind::Purchase => "purchase",
            ProposalKind::Arrival => "arrival",
        }
    }

    pub fn parse(value: &str) -> Option<ProposalKind> {
        match value {
            "opening" => Some(ProposalKind::Opening),
            "listing_sold" => Some(ProposalKind::ListingSold),
            "listing_returned" => Some(ProposalKind::ListingReturned),
            "purchase" => Some(ProposalKind::Purchase),
            "arrival" => Some(ProposalKind::Arrival),
            _ => None,
        }
    }
}

/// What the player decided, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Pending,
    Recorded,
    Dismissed,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Pending => "pending",
            Decision::Recorded => "recorded",
            Decision::Dismissed => "dismissed",
        }
    }

    pub fn parse(value: &str) -> Option<Decision> {
        match value {
            "pending" => Some(Decision::Pending),
            "recorded" => Some(Decision::Recorded),
            "dismissed" => Some(Decision::Dismissed),
            _ => None,
        }
    }
}

/// A stored proposal's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProposalId(pub i64);

/// Why an opening did not settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Doubt {
    /// More than one kind of box left in the step.
    SeveralKinds,
    /// Boxes left with no keys beside them.
    NoKeys,
    /// The keys and the boxes left in different counts.
    CountsDiffer,
    /// No one reading of the ammo and the arrivals pays the keys back.
    Unbalanced,
}

/// The ammo an opening paid back, when it settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedAmmo {
    pub units: i64,
    pub amount: Amount,
}

/// An opening, as the readings show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpeningProposal {
    /// The boxes that left, one entry for each kind.
    pub boxes: Vec<OpenedBoxes>,
    pub keys_used: i64,
    /// The keys' TT.
    pub keys: Amount,
    /// The ammo it paid back; only when it settled.
    pub ammo: Option<ProposedAmmo>,
    /// What every reading of the step takes as its contents.
    pub contents: Vec<OpenedContent>,
    /// What arrived that may be its contents and may not.
    pub possible_contents: Vec<OpenedContent>,
    pub settlement: Settlement,
    /// Why it did not settle; only when it did not.
    pub doubt: Option<Doubt>,
    /// How many boxes paid only ammo, a row of exactly one key's TT each:
    /// only when the new ammo rows show it, one row a box.
    pub paid_ammo_alone: Option<i64>,
}

/// A listing's outcome, as the readings show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingProposal {
    /// The listing it matched.
    pub listing_id: String,
    /// The item, as the account site names it.
    pub name: String,
    pub units: i64,
    /// The TT the readings showed leaving the auction.
    pub amount: Amount,
    /// For a sale: the PED Card's rise in the same step, when nothing else
    /// could have moved it and the step was short.
    pub proceeds: Option<Pec>,
    /// For a return: where it went.
    pub returned_to: Option<Container>,
}

/// Something that arrived in a step of a tracked session that nothing the
/// app recorded explains: no loot line of the session's chat log, no
/// recorded quest reward, no Shrapnel conversion and no strongbox opening.
/// A purchase, a trade, a gift or a pickup: the player says which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrivalProposal {
    /// The item, as the account site names it, and where it arrived.
    pub key: HoldingKey,
    pub units: i64,
    /// Its TT, as the readings show it.
    pub amount: Amount,
    /// The PED Card fell by its TT in the step: bought at the Trade
    /// Terminal, at no markup.
    pub bought_at_tt: bool,
}

/// What a proposal holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Opening(OpeningProposal),
    Listing(ListingProposal),
    Purchase(Purchase),
    Arrival(ArrivalProposal),
}

impl Detail {
    /// Whether this is what a proposal of `kind` holds.
    fn is_of(&self, kind: ProposalKind) -> bool {
        matches!(
            (kind, self),
            (ProposalKind::Opening, Detail::Opening(_))
                | (
                    ProposalKind::ListingSold | ProposalKind::ListingReturned,
                    Detail::Listing(_)
                )
                | (ProposalKind::Purchase, Detail::Purchase(_))
                | (ProposalKind::Arrival, Detail::Arrival(_))
        )
    }
}

/// A pending proposal, as offered.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub id: ProposalId,
    pub kind: ProposalKind,
    /// The reading it was seen in, and the reading stored before it.
    pub reading: ReadingId,
    pub prev_reading: ReadingId,
    /// When those two readings were taken: it happened in between.
    pub between: Span,
    /// The tracked session it fell in.
    pub session_id: Option<String>,
    /// Claimed for a recording that was never linked: it may be recorded
    /// already.
    pub claimed: bool,
    /// When it was found (unix seconds): what the player has seen of the
    /// account's activity is read against it.
    pub detected_at: f64,
    pub detail: Detail,
}

// ── What one step holds ─────────────────────────────────────────────

/// Something that left the auction in a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionDeparture {
    pub name: String,
    pub units: i64,
    pub amount: Amount,
    /// The rows it was read from: one PEC each the site may have hidden.
    pub rows: usize,
    /// Where it went, when it went somewhere: a return.
    pub returned_to: Option<Container>,
    /// For a sale: the Card's rise, when it can be read as the proceeds.
    pub proceeds: Option<Pec>,
    /// For a sale: the Card's rise in a step of any length when nothing
    /// else in it could have moved the Card. Only ever matched against the
    /// prices the listing could have sold at, never offered as a figure.
    pub card_rise: Option<Pec>,
}

/// What one step between two readings shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepOutcomes {
    pub opening: Option<OpeningProposal>,
    pub departures: Vec<AuctionDeparture>,
    /// A purchase at auction, if the step holds one and nothing else;
    /// proposed only outside every tracked session.
    pub purchase: Option<Purchase>,
}

/// Read the step from `before` to `after`: an opening, and what left the
/// auction. Pure.
pub fn step_outcomes(
    before: &Reading,
    after: &Reading,
    roles: &dyn RoleSource,
) -> Result<StepOutcomes, AccountError> {
    let set = step_between(before, after, roles)?;
    let reserved = conversion_rows(&set, before, after)?;
    let composites = recognise(&set, before, after, &reserved)?;
    let opening = match &composites.opening {
        Some(opening) => {
            let rows = new_ammo_rows(&set, before, after, &reserved)?;
            Some(opening_proposal(opening, &rows))
        }
        None => None,
    };
    Ok(StepOutcomes {
        opening,
        departures: departures(&set, before),
        purchase: purchase(&set)?,
    })
}

fn opening_proposal(opening: &Opening, rows: &[NewAmmoRow]) -> OpeningProposal {
    let settled = opening.settlement == Settlement::Settled;
    let doubt = if settled {
        None
    } else {
        Some(match opening.boxes.as_slice() {
            [_, _, ..] => Doubt::SeveralKinds,
            _ if opening.keys_used == 0 => Doubt::NoKeys,
            [only] if only.count != opening.keys_used => Doubt::CountsDiffer,
            _ => Doubt::Unbalanced,
        })
    };
    OpeningProposal {
        boxes: opening.boxes.clone(),
        keys_used: opening.keys_used,
        keys: opening.keys,
        ammo: settled.then_some(ProposedAmmo {
            units: opening.ammo_units,
            amount: opening.ammo_from_rows,
        }),
        contents: opening.contents.clone(),
        possible_contents: opening.possible_contents.clone(),
        settlement: opening.settlement,
        doubt,
        paid_ammo_alone: paid_ammo_alone(opening, rows),
    }
}

/// How many boxes paid ammo alone, when the new rows show it exactly: the
/// opening settled on these very rows, there is at most one row a box, no
/// row holds more than one key's TT, and every row is known exactly. A row
/// of exactly one key's TT is a box that paid nothing else.
fn paid_ammo_alone(opening: &Opening, rows: &[NewAmmoRow]) -> Option<i64> {
    if opening.settlement != Settlement::Settled {
        return None;
    }
    let [boxes] = opening.boxes.as_slice() else {
        return None;
    };
    let units: i64 = rows.iter().map(|row| row.units).sum();
    if units != opening.ammo_units || rows.len() != opening.ammo_rows || rows.is_empty() {
        return None;
    }
    if i64::try_from(rows.len()).ok()? > boxes.count || opening.keys_used <= 0 {
        return None;
    }
    let keys = opening.keys.micro_ped();
    if keys % opening.keys_used != 0 {
        return None;
    }
    let key = keys / opening.keys_used;
    let readable = rows
        .iter()
        .all(|row| row.amount.precision() == Precision::Exact && row.amount.micro_ped() <= key);
    if !readable {
        return None;
    }
    i64::try_from(
        rows.iter()
            .filter(|row| row.amount.micro_ped() == key)
            .count(),
    )
    .ok()
}

/// Whether the step at `index` is the only one in the step set that
/// changes how much of `name` is held anywhere. Wear and restoration change
/// no quantity, so they never count against it.
fn alone_of_its_name(set: &StepSet, index: usize, name: &str) -> bool {
    set.steps.iter().enumerate().all(|(other, step)| {
        other == index
            || !step
                .quantity_changes()
                .iter()
                .any(|(key, _)| key.name == name)
    })
}

/// What left the auction in a step: sold (gone) or returned (moved out to
/// another place, exactly as many as left). Only a departure that is the
/// one change of its item's quantity in the step: anything else of that
/// name gained, lost, consumed or moved alongside (loot of the same item,
/// say) makes it impossible to tell what happened, so nothing is read.
pub(super) fn departures(set: &StepSet, before: &Reading) -> Vec<AuctionDeparture> {
    let mut found = Vec::new();
    for (index, step) in set.steps.iter().enumerate() {
        let alone = |name: &str| alone_of_its_name(set, index, name);
        match step {
            Step::Consumed {
                key,
                units,
                amount,
                rows_touched,
                ..
            } if key.container == Container::Auction && alone(&key.name) => {
                found.push(AuctionDeparture {
                    name: key.name.clone(),
                    units: *units,
                    amount: *amount,
                    rows: (*rows_touched).max(1),
                    returned_to: None,
                    proceeds: proceeds(set, index),
                    card_rise: card_rise(set, index),
                })
            }
            Step::Left { key, units, value }
                if key.container == Container::Auction && alone(&key.name) =>
            {
                found.push(AuctionDeparture {
                    name: key.name.clone(),
                    units: *units,
                    amount: *value,
                    rows: 1,
                    returned_to: None,
                    proceeds: proceeds(set, index),
                    card_rise: card_rise(set, index),
                })
            }
            Step::Moved {
                name,
                units,
                amount,
                from: Container::Auction,
                to,
            } if *to != Container::Auction && alone(name) => {
                let rows = before
                    .holding(&HoldingKey {
                        name: name.clone(),
                        container: Container::Auction,
                    })
                    .map_or(1, |holding| holding.row_count().max(1));
                found.push(AuctionDeparture {
                    name: name.clone(),
                    units: *units,
                    amount: *amount,
                    rows,
                    returned_to: Some(to.clone()),
                    proceeds: None,
                    card_rise: None,
                });
            }
            _ => {}
        }
    }
    found
}

/// The PED Card's rise in the step, as the proceeds of the sale at
/// `sold`: only when the step is short, the Card rose, and nothing else in
/// it could have moved the Card. Wear and moves between the player's own
/// places never do; anything consumed, gained, arriving, leaving,
/// repaired, listed or returned might.
fn proceeds(set: &StepSet, sold: usize) -> Option<Pec> {
    let span = set.span.to - set.span.from;
    if !(0.0..=PROCEEDS_WITHIN_SECONDS).contains(&span) {
        return None;
    }
    card_rise(set, sold)
}

/// The PED Card's rise in the step, when the Card rose and nothing else in
/// it but the sale at `sold` could have moved the Card, however long the
/// step.
fn card_rise(set: &StepSet, sold: usize) -> Option<Pec> {
    let mut rise = None;
    for (index, step) in set.steps.iter().enumerate() {
        if index == sold {
            continue;
        }
        match step {
            Step::BalanceChanged { from, to } if to.0 > from.0 => rise = Some(Pec(to.0 - from.0)),
            Step::Decayed { .. } => {}
            Step::Moved { from, to, .. } if MoveKind::of(from, to) == MoveKind::Transfer => {}
            _ => return None,
        }
    }
    rise
}

// ── Matching a listing ──────────────────────────────────────────────

/// The local calendar day of a reading, as a listing's date is written.
pub(super) fn local_day(at: f64) -> String {
    crate::time::epoch_to_instant(at)
        .with_timezone(&chrono::Local)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

/// Whether a listing was on auction before the step began: its own instant
/// at or before the earlier reading, or, for a listing recorded by its day
/// alone, a day no later than the later reading's.
fn listed_before(listed_at: &str, listed_instant: Option<f64>, between: &Span) -> bool {
    match listed_instant {
        Some(instant) => instant <= between.from,
        None => listed_at
            .get(..10)
            .is_some_and(|day| day <= local_day(between.to).as_str()),
    }
}

/// A listing a departure matched, with the terms it was posted on.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ListedTerms {
    pub id: String,
    /// The whole listing's TT, in PED.
    pub tt_value: f64,
    pub starting_bid: f64,
    pub buyout: Option<f64>,
}

/// What the readings settle about a listing on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListingSettled {
    /// It came back from the auction: expired or withdrawn.
    Returned,
    /// It sold at this price, with the fee the game charges for selling
    /// above the starting bid.
    Sold { final_price: Pec, sale_fee: Pec },
}

fn pec_of(ped: f64) -> Pec {
    Pec((ped * 100.0).round() as i64)
}

/// The fee the game charges at the point of sale: the listing fee the
/// final price's markup would have carried, less the one already paid on
/// the starting bid's ([`crate::auction_fee::listing_fee_ped`]).
fn point_of_sale_fee(terms: &ListedTerms, final_price: f64) -> Pec {
    let fee = |price: f64| pec_of(crate::auction_fee::listing_fee_ped(price - terms.tt_value));
    Pec((fee(final_price).0 - fee(terms.starting_bid).0).max(0))
}

/// The outcome of a matched listing the readings settle with nothing left
/// to guess. A return always. A sale only when one price fits: a listing
/// whose buyout is its starting bid can have sold at nothing else; any
/// other sells at its starting bid or its buyout only when the Card's rise
/// in the step is exactly one of them, as paid or less its point-of-sale
/// fee (it may be taken from the proceeds). A sale between the two, or a
/// rise that fits both or neither, is the player's to price.
pub(super) fn settled(terms: &ListedTerms, departure: &AuctionDeparture) -> Option<ListingSettled> {
    if departure.returned_to.is_some() {
        return Some(ListingSettled::Returned);
    }
    let start = pec_of(terms.starting_bid);
    let mut prices = vec![(start, Pec::ZERO)];
    let buyout = terms.buyout.filter(|buyout| buyout.is_finite());
    if let Some(buyout) = buyout.filter(|buyout| pec_of(*buyout).0 > start.0) {
        prices.push((pec_of(buyout), point_of_sale_fee(terms, buyout)));
    }
    let (final_price, sale_fee) = match departure.card_rise {
        Some(rise) => {
            let fits: Vec<(Pec, Pec)> = prices
                .into_iter()
                .filter(|(price, fee)| rise.0 == price.0 || rise.0 == price.0 - fee.0)
                .collect();
            match fits.as_slice() {
                [only] => *only,
                _ => return None,
            }
        }
        None if buyout.is_some_and(|buyout| pec_of(buyout).0 == start.0) => (start, Pec::ZERO),
        None => return None,
    };
    Some(ListingSettled::Sold {
        final_price,
        sale_fee,
    })
}

/// The one listing still on auction a departure matches, by item name and
/// quantity, and by TT where the listing has one, among the listings made
/// before the step; `None` for no match or more than one.
pub(super) fn matching_listing(
    conn: &rusqlite::Connection,
    departure: &AuctionDeparture,
    between: &Span,
) -> Result<Option<ListedTerms>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT id, item_name, tt_value, listed_at, listed_instant, starting_bid, buyout \
         FROM auction_listings \
         WHERE status = 'pending' AND undone_at IS NULL AND channel = 'auction' \
           AND quantity = ?1",
    )?;
    let candidates = stmt
        .query_map([departure.units], |row| {
            Ok((
                ListedTerms {
                    id: row.get(0)?,
                    tt_value: row.get(2)?,
                    starting_bid: row.get(5)?,
                    buyout: row.get(6)?,
                },
                row.get::<_, String>(1)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let wanted = departure.name.trim().to_lowercase();
    let tolerance = i64::try_from(departure.rows)
        .unwrap_or(i64::MAX)
        .saturating_mul(MICRO_PER_PEC);
    let matching: Vec<ListedTerms> = candidates
        .into_iter()
        .filter(|(_, _, listed_at, instant)| listed_before(listed_at, *instant, between))
        .filter(|(_, name, _, _)| name.trim().to_lowercase() == wanted)
        .filter(|(terms, _, _, _)| {
            let tt = terms.tt_value;
            if !tt.is_finite() || tt <= 0.0 {
                return true;
            }
            let listed = (tt * MICRO_PER_PED as f64).round() as i64;
            listed.abs_diff(departure.amount.micro_ped()) <= tolerance.unsigned_abs()
        })
        .map(|(terms, _, _, _)| terms)
        .collect();
    Ok(match matching.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    })
}

// ── Examining readings ──────────────────────────────────────────────

/// A proposal found in a step, ready to store.
#[derive(Debug, Clone)]
struct Found {
    kind: ProposalKind,
    subject: String,
    payload: String,
}

fn payload<T: Serialize>(value: &T) -> Result<String, AccountError> {
    serde_json::to_string(value).map_err(|_| AccountError::Stored("a proposal does not serialise"))
}

/// Whether no tracked session's play overlaps `between`.
fn outside_every_session(
    conn: &rusqlite::Connection,
    between: &Span,
) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM tracking_sessions \
                            WHERE started_at < ?2 AND COALESCE(ended_at, ?2) > ?1)",
        rusqlite::params![between.from, between.to],
        |row| row.get::<_, bool>(0),
    )?)
}

/// Whether the chat log of any session looted `name` inside `between`.
fn looted_inside(
    conn: &rusqlite::Connection,
    name: &str,
    between: &Span,
) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        // every-kill: spend (any row's loot, a claimed reward's arrival
        // included: an item that came in either way was never bought)
        "SELECT EXISTS (SELECT 1 FROM kill_loot_items l JOIN kills k ON k.id = l.kill_id \
                        WHERE l.item_name = ?1 AND k.timestamp >= ?2 AND k.timestamp <= ?3) \
             OR EXISTS (SELECT 1 FROM harvest_loot_items l \
                        JOIN harvest_events e ON e.id = l.harvest_id \
                        WHERE l.item_name = ?1 AND e.timestamp >= ?2 AND e.timestamp <= ?3)",
        rusqlite::params![name, between.from, between.to],
        |row| row.get::<_, bool>(0),
    )?)
}

/// The proposals the step from `before` to `after` holds. `session` is the
/// tracked session the step fell in, if any.
fn found_in_step(
    conn: &rusqlite::Connection,
    before: &Reading,
    after: &Reading,
    session: Option<&str>,
    settles_listings: bool,
    roles: &dyn RoleSource,
) -> Result<Vec<Found>, AccountError> {
    let outcomes = step_outcomes(before, after, roles)?;
    let between = Span {
        from: before.taken_at(),
        to: after.taken_at(),
    };
    let mut found = Vec::new();
    if let Some(opening) = &outcomes.opening {
        // Outside a session, a step that holds nothing but an opening the
        // step-by-step reading cannot settle (its ammo merged into a stack
        // already held, say) is read whole: every arrival a content, the
        // ammo as the Universal Ammo gained.
        let whole = if session.is_none() && opening.settlement == Settlement::Unsettled {
            let set = step_between(before, after, roles)?;
            match whole_opening(&set)? {
                Some(whole) => {
                    let rows = new_ammo_rows(&set, before, after, &[])?;
                    Some(opening_proposal(&whole, &rows))
                }
                None => None,
            }
        } else {
            None
        };
        found.push(opening_found(whole.as_ref().unwrap_or(opening))?);
    }
    // A purchase only outside every tracked session, and never of an item
    // a session looted meanwhile: loot arriving beside a repair or a fee
    // looks the same.
    if let Some(bought) = &outcomes.purchase {
        if session.is_none()
            && outside_every_session(conn, &between)?
            && !looted_inside(conn, &bought.key.name, &between)?
        {
            found.push(Found {
                kind: ProposalKind::Purchase,
                subject: bought.key.name.clone(),
                payload: payload(bought)?,
            });
        }
    }
    for departure in &outcomes.departures {
        let Some(terms) = matching_listing(conn, departure, &between)? else {
            continue;
        };
        // What the readings settle is recorded on its own by the pass that
        // follows the stock (see [`super::follow`]); only a sale whose price
        // is the player's to say is proposed.
        if settles_listings && settled(&terms, departure).is_some() {
            continue;
        }
        let listing_id = terms.id;
        let kind = if departure.returned_to.is_some() {
            ProposalKind::ListingReturned
        } else {
            ProposalKind::ListingSold
        };
        found.push(Found {
            kind,
            subject: listing_id.clone(),
            payload: payload(&ListingProposal {
                listing_id,
                name: departure.name.clone(),
                units: departure.units,
                amount: departure.amount,
                proceeds: departure.proceeds,
                returned_to: departure.returned_to.clone(),
            })?,
        });
    }
    Ok(found)
}

/// An opening proposal, ready to store: its subject is its boxes' names.
fn opening_found(opening: &OpeningProposal) -> Result<Found, AccountError> {
    let subject = opening
        .boxes
        .iter()
        .map(|boxes| boxes.name.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    Ok(Found {
        kind: ProposalKind::Opening,
        subject,
        payload: payload(opening)?,
    })
}

/// An opening read whole across consecutive steps, ready to store.
#[derive(Debug, Clone)]
struct Whole {
    /// The reading before its first step, and when it was taken.
    prev: ReadingId,
    from: f64,
    /// The readings whose own opening proposals it takes the place of.
    spans: Vec<ReadingId>,
    found: Found,
}

/// Whether a reading was taken outside every tracked session: while signed
/// in and otherwise idle, or for a strongbox run.
fn is_idle_reading(header: &store::ReadingHeader) -> bool {
    matches!(header.purpose, Purpose::Ambient | Purpose::Run) && header.session_id.is_none()
}

/// The step from `before` to `after`, its items' stacking resolved from
/// both.
pub(super) fn step_between(
    before: &Reading,
    after: &Reading,
    roles: &dyn RoleSource,
) -> Result<StepSet, AccountError> {
    let pair = [before.clone(), after.clone()];
    let book = RoleBook::resolve(roles, &pair);
    diff(before, after, &|name| book.stacking(name))
}

/// The reading before the widest span of the opening proposals seen at
/// `reading` (the step's own, and an opening read whole that ends there),
/// while every one of them is still pending and unclaimed; `None` when
/// there is none, or one is decided or claimed.
fn open_opening_at(
    conn: &rusqlite::Connection,
    reading: ReadingId,
) -> Result<Option<ReadingId>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT prev_reading_id, status, claimed_at FROM account_proposals \
         WHERE kind = ?1 AND reading_id = ?2",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![ProposalKind::Opening.as_str(), reading.0],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let undecided = rows.iter().all(|(_, status, claimed)| {
        claimed.is_none() && Decision::parse(status) == Some(Decision::Pending)
    });
    Ok(if undecided {
        rows.iter().map(|(prev, _, _)| *prev).min().map(ReadingId)
    } else {
        None
    })
}

/// How many steps lie between two stored readings: the readings stored
/// after `from`, up to and including `to`.
fn steps_between(
    conn: &rusqlite::Connection,
    from: ReadingId,
    to: ReadingId,
) -> Result<i64, AccountError> {
    Ok(conn.query_row(
        // id-order: insertion (the chain follows the order readings were stored)
        "SELECT COUNT(*) FROM account_readings WHERE id > ?1 AND id <= ?2",
        [from.0, to.0],
        |row| row.get(0),
    )?)
}

/// The opening the step from `before` to `after` ends, read whole back to
/// where it began: `after` was taken while idle and its step held nothing
/// but an opening, as did every step back to the start of the span, each
/// taken while idle and still proposed, pending and unclaimed, and never
/// more than [`WHOLE_OPENING_STEPS`] steps in all. A proposal spanning one
/// step is taken only when that step held nothing but its opening; one
/// spanning more was read whole already. `None` when there is no earlier
/// step to join, or the span does not balance.
fn whole_across(
    conn: &rusqlite::Connection,
    before: &StoredReading,
    after: &StoredReading,
    roles: &dyn RoleSource,
) -> Result<Option<Whole>, AccountError> {
    if !is_idle_reading(&after.header)
        || !only_an_opening(&step_between(&before.reading, &after.reading, roles)?)
    {
        return Ok(None);
    }
    let mut earliest = before.clone();
    let mut spans = Vec::new();
    while is_idle_reading(&earliest.header) {
        let Some(start) = open_opening_at(conn, earliest.header.id)? else {
            break;
        };
        if steps_between(conn, start, after.header.id)? > WHOLE_OPENING_STEPS {
            break;
        }
        let Some(start_reading) = store::load_reading(conn, start)? else {
            break;
        };
        if Some(start) == earliest.header.prev
            && !only_an_opening(&step_between(
                &start_reading.reading,
                &earliest.reading,
                roles,
            )?)
        {
            break;
        }
        spans.push(earliest.header.id);
        earliest = start_reading;
    }
    if spans.is_empty() {
        return Ok(None);
    }
    let set = step_between(&earliest.reading, &after.reading, roles)?;
    let Some(opening) = whole_opening(&set)? else {
        return Ok(None);
    };
    let rows = new_ammo_rows(&set, &earliest.reading, &after.reading, &[])?;
    let mut found = opening_found(&opening_proposal(&opening, &rows))?;
    // Beside the step's own proposal at the same reading: its subject also
    // names the reading its span starts after.
    found.subject = format!("{} after reading {}", found.subject, earliest.header.id.0);
    Ok(Some(Whole {
        prev: earliest.header.id,
        from: earliest.reading.taken_at(),
        spans,
        found,
    }))
}

/// The tracked session a step fell in: the session the later reading was
/// taken for, unless that reading is the session's baseline (taken as it
/// started, so the step before it is the session's past).
fn session_of(after: &StoredReading) -> Option<String> {
    match after.header.purpose {
        Purpose::Baseline => None,
        _ => after.header.session_id.clone(),
    }
}

fn is_examined(conn: &rusqlite::Connection, reading: ReadingId) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM account_readings_examined WHERE reading_id = ?1)",
        [reading.0],
        |row| row.get::<_, bool>(0),
    )?)
}

/// What examining one reading found, computed on a reader and stored by a
/// short write of its own.
#[derive(Debug, Clone)]
pub struct Examination {
    reading: ReadingId,
    step: Option<ExaminedStep>,
}

#[derive(Debug, Clone)]
struct ExaminedStep {
    prev: ReadingId,
    between: Span,
    session: Option<String>,
    found: Vec<Found>,
    /// The opening this step ends, read whole across the steps before it.
    whole: Option<Whole>,
}

impl Examination {
    /// Nothing could be read from this reading's step: it is marked
    /// examined, and adds nothing.
    pub fn nothing(reading: ReadingId) -> Examination {
        Examination {
            reading,
            step: None,
        }
    }

    pub fn reading(&self) -> ReadingId {
        self.reading
    }
}

/// Examine one stored reading against the reading stored before it: a
/// read, so run it on a reader. `None` when the reading was examined
/// already, or is gone.
pub fn prepare(
    conn: &rusqlite::Connection,
    reading: ReadingId,
    roles: &dyn RoleSource,
) -> Result<Option<Examination>, AccountError> {
    if is_examined(conn, reading)? {
        return Ok(None);
    }
    prepare_again(conn, reading, roles)
}

/// [`prepare`], whether or not the reading was examined before.
fn prepare_again(
    conn: &rusqlite::Connection,
    reading: ReadingId,
    roles: &dyn RoleSource,
) -> Result<Option<Examination>, AccountError> {
    let Some(after) = store::load_reading(conn, reading)? else {
        return Ok(None);
    };
    let Some(prev) = after.header.prev else {
        return Ok(Some(Examination::nothing(reading)));
    };
    let before = store::load_reading(conn, prev)?
        .ok_or(AccountError::Stored("a reading's predecessor is missing"))?;
    let session = session_of(&after);
    let found = found_in_step(
        conn,
        &before.reading,
        &after.reading,
        session.as_deref(),
        super::follow::settles_listings(conn, &before, &after)?,
        roles,
    )?;
    let whole = whole_across(conn, &before, &after, roles)?;
    Ok(Some(Examination {
        reading,
        step: Some(ExaminedStep {
            prev,
            between: Span {
                from: before.reading.taken_at(),
                to: after.reading.taken_at(),
            },
            session,
            found,
            whole,
        }),
    }))
}

/// Store what examining a reading found, and mark it examined, in one
/// short transaction. A proposal already stored, or a reading examined
/// meanwhile, changes nothing. Returns how many proposals were added.
pub fn commit(
    conn: &mut rusqlite::Connection,
    examination: &Examination,
    now: f64,
) -> Result<usize, AccountError> {
    let tx = conn.transaction()?;
    let mut added = 0;
    if let Some(step) = &examination.step {
        // An opening read whole is stored only while every proposal it
        // spans is still pending and unclaimed.
        let mut whole = step.whole.as_ref();
        if let Some(read_whole) = whole {
            for spanned in &read_whole.spans {
                if open_opening_at(&tx, *spanned)?.is_none() {
                    whole = None;
                    break;
                }
            }
        }
        // The step's own proposals are stored whether or not an opening is
        // read whole across it: the whole one takes their place while it
        // stands, and they are there should it give way to its steps.
        let mut stored: Vec<(&Found, ReadingId, f64)> = step
            .found
            .iter()
            .map(|found| (found, step.prev, step.between.from))
            .collect();
        if let Some(whole) = whole {
            stored.push((&whole.found, whole.prev, whole.from));
        }
        for (found, prev, from) in stored {
            added += tx.execute(
                "INSERT INTO account_proposals \
                     (kind, reading_id, prev_reading_id, seen_from, seen_to, session_id, \
                      subject, payload_version, payload_json, status, detected_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, \
                         (SELECT id FROM tracking_sessions WHERE id = ?6), \
                         ?7, ?8, ?9, ?10, ?11) \
                 ON CONFLICT (kind, reading_id, subject) DO NOTHING",
                rusqlite::params![
                    found.kind.as_str(),
                    examination.reading.0,
                    prev.0,
                    from,
                    step.between.to,
                    step.session,
                    found.subject,
                    PAYLOAD_VERSION,
                    found.payload,
                    Decision::Pending.as_str(),
                    now,
                ],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO account_readings_examined (reading_id, examined_at) VALUES (?1, ?2) \
         ON CONFLICT (reading_id) DO NOTHING",
        rusqlite::params![examination.reading.0, now],
    )?;
    tx.commit()?;
    Ok(added)
}

/// What reading a strongbox run whole came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunWhole {
    /// An opening read whole across the run was stored (or stood already).
    pub read_whole: bool,
    /// How many of the run's box the last reading still holds, outside the
    /// auction: none left means no more can have been opened.
    pub boxes_left: i64,
}

/// Read a strongbox run whole, from the reading before its boxes (`from`)
/// to its latest (`to`): every arrival a content and the ammo as the
/// Universal Ammo gained, so a run whose step-by-step reading cannot
/// settle (its ammo merged into a stack, or its boxes opened across
/// minutes) still does. Stored as one opening proposal across the span
/// when it holds nothing but an opening, balances, lies outside every
/// tracked session and spans more than one step (a single step is read
/// whole as it is examined), and while every opening proposal inside it is
/// still pending and unclaimed; it then takes their place. Reading it
/// again adds nothing.
pub fn read_run_whole(
    conn: &mut rusqlite::Connection,
    from: ReadingId,
    to: ReadingId,
    box_item: &str,
    roles: &dyn RoleSource,
    now: f64,
) -> Result<RunWhole, AccountError> {
    let (Some(before), Some(after)) = (
        store::load_reading(conn, from)?,
        store::load_reading(conn, to)?,
    ) else {
        return Ok(RunWhole {
            read_whole: false,
            boxes_left: 0,
        });
    };
    let wanted = box_item.trim();
    let boxes_left = after
        .reading
        .holdings()
        .iter()
        .filter(|(key, _)| key.name.trim() == wanted && key.container != Container::Auction)
        .map(|(_, holding)| holding.quantity())
        .sum();
    let unread = RunWhole {
        read_whole: false,
        boxes_left,
    };
    if steps_between(conn, from, to)? < 2 {
        return Ok(unread);
    }
    let tagged: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM account_readings \
                        WHERE id > ?1 AND id <= ?2 AND session_id IS NOT NULL)",
        [from.0, to.0],
        |row| row.get(0),
    )?;
    if tagged {
        return Ok(unread);
    }
    let set = step_between(&before.reading, &after.reading, roles)?;
    let Some(opening) = whole_opening(&set)? else {
        return Ok(unread);
    };
    let rows = new_ammo_rows(&set, &before.reading, &after.reading, &[])?;
    let mut found = opening_found(&opening_proposal(&opening, &rows))?;
    found.subject = format!("{} after reading {}", found.subject, from.0);
    let tx = conn.transaction()?;
    // Every opening proposal inside the span must still be open for the
    // whole to take their place.
    let inside: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT reading_id FROM account_proposals \
             WHERE kind = ?1 AND reading_id > ?2 AND reading_id <= ?3 AND prev_reading_id >= ?2 \
               AND subject != ?4",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![ProposalKind::Opening.as_str(), from.0, to.0, found.subject],
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for reading in inside {
        if open_opening_at(&tx, ReadingId(reading))?.is_none() {
            return Ok(unread);
        }
    }
    tx.execute(
        "INSERT INTO account_proposals \
             (kind, reading_id, prev_reading_id, seen_from, seen_to, session_id, \
              subject, payload_version, payload_json, status, detected_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT (kind, reading_id, subject) DO NOTHING",
        rusqlite::params![
            found.kind.as_str(),
            to.0,
            from.0,
            before.reading.taken_at(),
            after.reading.taken_at(),
            found.subject,
            PAYLOAD_VERSION,
            found.payload,
            Decision::Pending.as_str(),
            now,
        ],
    )?;
    tx.commit()?;
    Ok(RunWhole {
        read_whole: true,
        boxes_left,
    })
}

/// Examine one stored reading whether or not it was examined before:
/// proposals already stored for it stay as they are, and none is added
/// twice. Returns how many proposals were added.
pub fn detect(
    conn: &mut rusqlite::Connection,
    reading: ReadingId,
    roles: &dyn RoleSource,
    now: f64,
) -> Result<usize, AccountError> {
    match prepare_again(conn, reading, roles)? {
        Some(examination) => commit(conn, &examination, now),
        None => Ok(0),
    }
}

/// The oldest stored reading not yet examined.
pub fn next_unexamined(conn: &rusqlite::Connection) -> Result<Option<ReadingId>, AccountError> {
    Ok(conn
        .query_row(
            // id-order: insertion (the chain follows the order readings were stored)
            "SELECT r.id FROM account_readings r \
             WHERE NOT EXISTS (SELECT 1 FROM account_readings_examined e \
                               WHERE e.reading_id = r.id) \
             ORDER BY r.id LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(ReadingId))
}

/// Where an examination reads and writes: the database's readers and its
/// writer in the app; one connection in a test.
pub trait Connections {
    fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce(&rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError>;

    fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError>;
}

impl Connections for crate::db::Db {
    fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce(&rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError> {
        self.with_reader_blocking(move |conn| Ok(job(conn)))
            .map_err(AccountError::from)
            .and_then(|done| done)
    }

    fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError> {
        self.with_writer_blocking(move |conn| Ok(job(conn)))
            .map_err(AccountError::from)
            .and_then(|done| done)
    }
}

/// Which reading an examination takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// This one, just stored.
    Reading(ReadingId),
    /// The oldest stored and not yet examined.
    Oldest,
}

/// What examining one reading came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Examined {
    pub reading: ReadingId,
    pub added: usize,
    /// Its step could not be read: it was marked examined and passed.
    pub passed: bool,
}

/// Whether an error is the step's own: what the stored readings hold
/// cannot be read (a figure out of range, a refused row or balance, a
/// stored value that does not decode, a missing predecessor). Such a
/// reading is passed. Any other error (the database itself) leaves it to
/// be examined again.
pub(super) fn is_the_step_s_own(error: &AccountError) -> bool {
    !matches!(error, AccountError::Db(_))
}

/// Examine one reading: what its step proposes is worked out on a reader,
/// never while the writer is held, and stored with the reading's examined
/// mark in one short write. A step whose own content cannot be read is
/// logged (by its reading's id, never its contents), marked examined and
/// passed, so it never holds up the readings after it; a database error
/// leaves the reading unexamined and is returned. `None` when there is
/// nothing to examine: the reading was examined already or is gone, or no
/// reading waits.
pub fn examine(
    conns: &impl Connections,
    game_data: Option<std::sync::Arc<crate::game_data_store::GameDataStore>>,
    which: Which,
    now: f64,
) -> Result<Option<Examined>, AccountError> {
    let prepared = conns.read(move |conn| {
        let reading = match which {
            Which::Reading(reading) => reading,
            Which::Oldest => match next_unexamined(conn)? {
                Some(reading) => reading,
                None => return Ok(None),
            },
        };
        let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
        match prepare(conn, reading, &roles) {
            Ok(Some(examination)) => Ok(Some((examination, false))),
            Ok(None) => Ok(None),
            Err(error) if is_the_step_s_own(&error) => {
                tracing::warn!(target: "eo::account", %error, reading = reading.0, "a stored reading's step could not be read; it is passed");
                Ok(Some((Examination::nothing(reading), true)))
            }
            Err(error) => Err(error),
        }
    })?;
    let Some((examination, passed)) = prepared else {
        return Ok(None);
    };
    let reading = examination.reading();
    let added = conns.write(move |conn| commit(conn, &examination, now))?;
    Ok(Some(Examined {
        reading,
        added,
        passed,
    }))
}

// ── Reading proposals back ──────────────────────────────────────────

struct Row {
    id: i64,
    kind: String,
    reading: i64,
    prev: i64,
    from: f64,
    to: f64,
    session: Option<String>,
    subject: String,
    version: i64,
    payload: String,
    decision: String,
    record: Option<String>,
    claimed_at: Option<f64>,
    detected_at: f64,
}

const ROW_COLUMNS: &str = "id, kind, reading_id, prev_reading_id, seen_from, seen_to, \
                           session_id, subject, payload_version, payload_json, status, \
                           record_id, claimed_at, detected_at";

fn raw(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        kind: row.get(1)?,
        reading: row.get(2)?,
        prev: row.get(3)?,
        from: row.get(4)?,
        to: row.get(5)?,
        session: row.get(6)?,
        subject: row.get(7)?,
        version: row.get(8)?,
        payload: row.get(9)?,
        decision: row.get(10)?,
        record: row.get(11)?,
        claimed_at: row.get(12)?,
        detected_at: row.get(13)?,
    })
}

/// Whether a strongbox opening stands: recorded, and neither it nor its
/// conversion taken back.
fn opening_stands(conn: &rusqlite::Connection, id: &str) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM strongbox_openings o \
                        JOIN stock_conversions c ON c.id = o.opening_conversion_id \
                        WHERE o.id = ?1 AND o.undone_at IS NULL AND c.undone_at IS NULL)",
        [id],
        |row| row.get::<_, bool>(0),
    )?)
}

/// Whether the asset purchase a purchase proposal was recorded as (by the
/// purchase's own id) still stands: bought, not made by a conversion, and
/// not taken back. Another purchase of the same asset, or another asset
/// going, does not touch it.
fn purchase_stands(conn: &rusqlite::Connection, id: &str) -> Result<bool, AccountError> {
    let Ok(id) = id.trim().parse::<i64>() else {
        return Ok(false);
    };
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM asset_purchases WHERE id = ?1 AND conversion_id IS NULL)",
        [id],
        |row| row.get::<_, bool>(0),
    )?)
}

/// Whether stock of unknown origin recorded from an arrival (by its own
/// id) still stands: recorded, and not taken back.
fn unknown_origin_stands(conn: &rusqlite::Connection, id: &str) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_unknown_origins WHERE id = ?1 AND undone_at IS NULL)",
        [id.trim()],
        |row| row.get::<_, bool>(0),
    )?)
}

/// The standing stock of unknown origin recorded from the arrival
/// proposal `proposal`, whether or not the proposal was marked recorded.
fn standing_unknown_origin_of(
    conn: &rusqlite::Connection,
    proposal: i64,
) -> Result<Option<String>, AccountError> {
    Ok(conn
        .query_row(
            "SELECT id FROM stock_unknown_origins \
             WHERE proposal_id = ?1 AND undone_at IS NULL LIMIT 1",
            [proposal],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// The name of the asset an asset purchase (by its id) bought into.
fn purchased_asset_name(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<String>, AccountError> {
    let Ok(id) = id.trim().parse::<i64>() else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT COALESCE( \
                 (SELECT i.name FROM inventory_items i WHERE i.id = p.position_id), \
                 (SELECT i.name FROM inventory_items i WHERE i.position_id = p.position_id \
                  LIMIT 1)) \
             FROM asset_purchases p WHERE p.id = ?1",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Whether what an arrival was recorded as still stands: an asset
/// purchase (by the purchase's id), or stock of unknown origin (by its own).
fn arrival_record_stands(conn: &rusqlite::Connection, id: &str) -> Result<bool, AccountError> {
    Ok(purchase_stands(conn, id)? || unknown_origin_stands(conn, id)?)
}

/// A listing's status while it stands (not undone); `None` when it does
/// not.
fn listing_status(conn: &rusqlite::Connection, id: &str) -> Result<Option<String>, AccountError> {
    Ok(conn
        .query_row(
            "SELECT status FROM auction_listings WHERE id = ?1 AND undone_at IS NULL",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// A standing strongbox opening no proposal is linked to: recorded another
/// way (pasted, typed, or recorded and never linked).
struct UnlinkedOpening {
    box_item: String,
    count: f64,
    created_at: f64,
}

fn unlinked_openings(conn: &rusqlite::Connection) -> Result<Vec<UnlinkedOpening>, AccountError> {
    let mut stmt = conn.prepare(
        // id-order: tiebreak (the latest recorded first; id splits equal instants)
        "SELECT o.box_item, o.box_count, o.created_at FROM strongbox_openings o \
         JOIN stock_conversions c ON c.id = o.opening_conversion_id \
         WHERE o.undone_at IS NULL AND c.undone_at IS NULL \
           AND o.id NOT IN (SELECT record_id FROM account_proposals \
                            WHERE record_id IS NOT NULL AND kind = 'opening' \
                              AND status = 'recorded') \
         ORDER BY o.created_at DESC, o.id DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(UnlinkedOpening {
                box_item: row.get(0)?,
                count: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// An opening proposal still open, as the rule below weighs it.
struct Candidate {
    id: i64,
    reading: i64,
    prev: i64,
    from: f64,
    box_item: String,
    count: i64,
    /// When it was claimed for a recording, if it was.
    claimed_at: Option<f64>,
}

/// The latest group in `pool` (latest first) of proposals from
/// consecutive steps whose counts add up exactly to `count`.
fn latest_group(pool: &[&Candidate], count: f64) -> Option<Vec<i64>> {
    for end in 0..pool.len() {
        let mut group = vec![pool[end].id];
        let mut sum = pool[end].count;
        let mut earliest = pool[end];
        loop {
            if (sum as f64 - count).abs() < 1e-9 {
                return Some(group);
            }
            if sum as f64 > count {
                break;
            }
            let Some(before) = pool[end + 1..]
                .iter()
                .find(|candidate| candidate.reading == earliest.prev)
            else {
                break;
            };
            group.push(before.id);
            sum += before.count;
            earliest = before;
        }
    }
    None
}

/// The open opening proposals that openings recorded another way already
/// account for. Each such opening accounts for at most one group of its
/// one box, from consecutive steps, each seen starting no later than the
/// opening was recorded, whose counts add up exactly to its count. A group
/// claimed for a recording before the opening was recorded is taken first
/// (it is that recording, never linked); otherwise the latest such run of
/// proposals. Openings are weighed latest first, and a proposal is
/// accounted for at most once; what no opening accounts for stays open.
/// Also answers which openings found no group.
fn accounted_for<'a>(
    openings: &'a [UnlinkedOpening],
    candidates: &[Candidate],
) -> (std::collections::HashSet<i64>, Vec<&'a UnlinkedOpening>) {
    let mut taken = std::collections::HashSet::new();
    let mut unmatched = Vec::new();
    for opening in openings {
        let wanted = opening.box_item.trim().to_lowercase();
        // Latest first.
        let mut pool: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| !taken.contains(&candidate.id))
            .filter(|candidate| candidate.box_item.trim().to_lowercase() == wanted)
            .filter(|candidate| candidate.from <= opening.created_at)
            .collect();
        pool.sort_by_key(|candidate| std::cmp::Reverse(candidate.reading));
        let claimed: Vec<&Candidate> = pool
            .iter()
            .copied()
            .filter(|candidate| {
                candidate
                    .claimed_at
                    .is_some_and(|claimed| claimed <= opening.created_at)
            })
            .collect();
        let group =
            latest_group(&claimed, opening.count).or_else(|| latest_group(&pool, opening.count));
        match group {
            Some(group) => taken.extend(group),
            None => unmatched.push(opening),
        }
    }
    (taken, unmatched)
}

fn decode(row: &Row, kind: ProposalKind) -> Option<Detail> {
    if row.version != PAYLOAD_VERSION {
        return None;
    }
    match kind {
        ProposalKind::Opening => serde_json::from_str(&row.payload).ok().map(Detail::Opening),
        ProposalKind::ListingSold | ProposalKind::ListingReturned => {
            serde_json::from_str(&row.payload).ok().map(Detail::Listing)
        }
        ProposalKind::Purchase => serde_json::from_str(&row.payload)
            .ok()
            .map(Detail::Purchase),
        ProposalKind::Arrival => serde_json::from_str(&row.payload).ok().map(Detail::Arrival),
    }
}

/// The span of readings each opening proposal was seen across, whatever
/// became of it: the reading before its first step, and its own.
fn opening_spans(conn: &rusqlite::Connection) -> Result<Vec<(i64, i64, i64)>, AccountError> {
    let mut stmt = conn
        .prepare("SELECT id, prev_reading_id, reading_id FROM account_proposals WHERE kind = ?1")?;
    let spans = stmt
        .query_map([ProposalKind::Opening.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(spans)
}

/// Whether the span of readings `(prev, reading)` lies strictly inside
/// `(from, to)`.
fn strictly_inside(prev: i64, reading: i64, from: i64, to: i64) -> bool {
    from <= prev && to >= reading && (from < prev || to > reading)
}

/// Whether an opening read whole across a wider span of readings took the
/// place of the proposal `(id, prev, reading)`, whatever became of it.
fn spanned_by_a_wider(spans: &[(i64, i64, i64)], id: i64, prev: i64, reading: i64) -> bool {
    spans
        .iter()
        .any(|&(other, from, to)| other != id && strictly_inside(prev, reading, from, to))
}

/// Whether the proposal `(id, prev, reading)` spans any narrower one: an
/// opening read whole.
fn spans_a_narrower(spans: &[(i64, i64, i64)], id: i64, prev: i64, reading: i64) -> bool {
    spans
        .iter()
        .any(|&(other, from, to)| other != id && strictly_inside(from, to, prev, reading))
}

/// Whether an opening seen from `from` may hold one of `parts`, openings
/// recorded another way that no group accounts for: one of its box,
/// recorded since it began.
fn may_hold_a_part(detail: &Detail, from: f64, parts: &[&UnlinkedOpening]) -> bool {
    let Detail::Opening(opening) = detail else {
        return false;
    };
    let [only] = opening.boxes.as_slice() else {
        return false;
    };
    let wanted = only.name.trim().to_lowercase();
    parts
        .iter()
        .any(|part| part.box_item.trim().to_lowercase() == wanted && from <= part.created_at)
}

/// The open opening proposals as the accounting below weighs them: each
/// with a single kind of box.
fn candidates_of<'a>(
    open: impl Iterator<Item = &'a (Row, ProposalKind, Detail)>,
) -> Vec<Candidate> {
    open.filter_map(|(row, _, detail)| match detail {
        Detail::Opening(opening) => match opening.boxes.as_slice() {
            [only] => Some(Candidate {
                id: row.id,
                reading: row.reading,
                prev: row.prev,
                from: row.from,
                box_item: only.name.clone(),
                count: only.count,
                claimed_at: row.claimed_at,
            }),
            _ => None,
        },
        Detail::Listing(_) | Detail::Purchase(_) | Detail::Arrival(_) => None,
    })
    .collect()
}

/// Every open proposal, the latest seen first, with whether each is
/// claimed. Open: a pending opening, or a recorded one whose opening was
/// taken back, unless an opening recorded another way accounts for it or
/// an opening read whole across a wider span took its place. An opening of
/// its box recorded another way since that span began, which no group
/// accounts for, may be a part of it: such an opening read whole gives way
/// to the steps it spans (the narrowest proposals inside it), so a part
/// recorded by hand is never offered again; a
/// listing's proposal exactly while its listing is still on auction (a
/// listing resolved by hand settles it, and a sale reverted opens it
/// again).
fn open_proposals(conn: &rusqlite::Connection) -> Result<Vec<(Proposal, bool)>, AccountError> {
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare(&format!(
            // id-order: tiebreak (the latest seen first; id splits equal instants)
            "SELECT {ROW_COLUMNS} FROM account_proposals WHERE status <> 'dismissed' \
             ORDER BY seen_to DESC, id DESC"
        ))?;
        let rows = stmt
            .query_map([], raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut open: Vec<(Row, ProposalKind, Detail)> = Vec::new();
    for row in rows {
        let (Some(kind), Some(decision)) = (
            ProposalKind::parse(&row.kind),
            Decision::parse(&row.decision),
        ) else {
            continue;
        };
        let Some(detail) = decode(&row, kind) else {
            continue;
        };
        let is_open = match (kind, decision) {
            _ if !detail.is_of(kind) => false,
            (_, Decision::Dismissed) => false,
            (
                ProposalKind::Opening | ProposalKind::Purchase | ProposalKind::Arrival,
                Decision::Pending,
            ) => true,
            (ProposalKind::Opening, Decision::Recorded) => match &row.record {
                Some(record) => !opening_stands(conn, record)?,
                None => true,
            },
            (ProposalKind::Purchase, Decision::Recorded) => match &row.record {
                Some(record) => !purchase_stands(conn, record)?,
                None => true,
            },
            (ProposalKind::Arrival, Decision::Recorded) => match &row.record {
                Some(record) => !arrival_record_stands(conn, record)?,
                None => true,
            },
            (ProposalKind::ListingSold | ProposalKind::ListingReturned, _) => {
                listing_status(conn, &row.subject)?.as_deref() == Some("pending")
            }
        };
        if is_open {
            open.push((row, kind, detail));
        }
    }
    let spans = if open
        .iter()
        .any(|(_, kind, _)| *kind == ProposalKind::Opening)
    {
        opening_spans(conn)?
    } else {
        Vec::new()
    };
    // Shown: every open proposal no opening read whole has taken the place of.
    let mut shown: Vec<bool> = open
        .iter()
        .map(|(row, kind, _)| {
            *kind != ProposalKind::Opening
                || !spanned_by_a_wider(&spans, row.id, row.prev, row.reading)
        })
        .collect();
    let candidates = candidates_of(open.iter().zip(&shown).filter(|(_, s)| **s).map(|(o, _)| o));
    let unlinked = if candidates.is_empty() {
        Vec::new()
    } else {
        unlinked_openings(conn)?
    };
    let (mut accounted, unmatched) = accounted_for(&unlinked, &candidates);
    if !unmatched.is_empty() {
        // An opening read whole that one of them may be a part of gives way
        // to the narrowest proposals inside it.
        let wholes: Vec<(i64, i64)> = open
            .iter()
            .zip(&shown)
            .filter(|((row, _, detail), shown)| {
                **shown
                    && spans_a_narrower(&spans, row.id, row.prev, row.reading)
                    && may_hold_a_part(detail, row.from, &unmatched)
            })
            .map(|((row, _, _), _)| (row.prev, row.reading))
            .collect();
        if !wholes.is_empty() {
            for ((row, kind, _), shown) in open.iter().zip(shown.iter_mut()) {
                if *kind != ProposalKind::Opening {
                    continue;
                }
                let inside = wholes
                    .iter()
                    .any(|&(from, to)| from <= row.prev && row.reading <= to);
                if inside {
                    *shown = !spans_a_narrower(&spans, row.id, row.prev, row.reading);
                }
            }
            let candidates =
                candidates_of(open.iter().zip(&shown).filter(|(_, s)| **s).map(|(o, _)| o));
            accounted = accounted_for(&unlinked, &candidates).0;
        }
    }
    Ok(open
        .into_iter()
        .zip(shown)
        .filter(|((row, _, _), shown)| *shown && !accounted.contains(&row.id))
        .map(|(open, _)| open)
        .map(|(row, kind, detail)| {
            (
                Proposal {
                    id: ProposalId(row.id),
                    kind,
                    reading: ReadingId(row.reading),
                    prev_reading: ReadingId(row.prev),
                    between: Span {
                        from: row.from,
                        to: row.to,
                    },
                    session_id: row.session,
                    claimed: row.claimed_at.is_some(),
                    detected_at: row.detected_at,
                    detail,
                },
                row.claimed_at.is_some(),
            )
        })
        .collect())
}

/// Every open proposal, of one kind or all, the latest seen first. A
/// stored row of an unknown kind or decision, or a payload of another
/// version, is passed over. A claimed proposal is listed, marked as
/// possibly recorded already.
pub fn pending(
    conn: &rusqlite::Connection,
    kind: Option<ProposalKind>,
) -> Result<Vec<Proposal>, AccountError> {
    Ok(open_proposals(conn)?
        .into_iter()
        .map(|(proposal, _)| proposal)
        .filter(|proposal| kind.is_none_or(|kind| proposal.kind == kind))
        .collect())
}

// ── Deciding ────────────────────────────────────────────────────────

/// What deciding proposals came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decided {
    /// Stored, or already so.
    Done,
    NotFound,
    /// It cannot be decided so: why, in words for the player.
    Refused(&'static str),
}

fn stored_row(conn: &rusqlite::Connection, id: ProposalId) -> Result<Option<Row>, AccountError> {
    Ok(conn
        .query_row(
            &format!("SELECT {ROW_COLUMNS} FROM account_proposals WHERE id = ?1"),
            [id.0],
            raw,
        )
        .optional()?)
}

/// Every proposal of `ids`, or which of them is missing.
fn stored_rows(
    conn: &rusqlite::Connection,
    ids: &[ProposalId],
) -> Result<Result<Vec<Row>, Decided>, AccountError> {
    if ids.is_empty() {
        return Ok(Err(Decided::Refused("no proposal was named")));
    }
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        match stored_row(conn, *id)? {
            Some(row) => rows.push(row),
            None => return Ok(Err(Decided::NotFound)),
        }
    }
    Ok(Ok(rows))
}

/// Claim proposals before their opening is recorded, all in one
/// transaction: every one must be open, and not claimed already unless it
/// is among `retake` (a claim left by a recording never linked, which the
/// player has checked and is taking over). A claim with no link after it
/// (the app closed, or the link failed) leaves the proposals listed as
/// possibly recorded already.
pub fn claim(
    conn: &mut rusqlite::Connection,
    ids: &[ProposalId],
    retake: &[ProposalId],
    now: f64,
) -> Result<Decided, AccountError> {
    let tx = conn.transaction()?;
    let rows = match stored_rows(&tx, ids)? {
        Ok(rows) => rows,
        Err(decided) => return Ok(decided),
    };
    let open = open_proposals(&tx)?;
    for row in &rows {
        let retaken = retake.contains(&ProposalId(row.id));
        match open.iter().find(|(proposal, _)| proposal.id.0 == row.id) {
            None => return Ok(Decided::Refused("that proposal is no longer open")),
            Some((proposal, true)) if !retaken => {
                return Ok(Decided::Refused(match proposal.kind {
                    ProposalKind::Purchase => {
                        "that purchase may already be recorded; check your assets first"
                    }
                    ProposalKind::Arrival => {
                        "that arrival may already be recorded; check your assets and stock first"
                    }
                    _ => "that proposal may already be recorded; check your openings first",
                }))
            }
            Some(_) => {}
        }
    }
    for row in &rows {
        tx.execute(
            "UPDATE account_proposals SET claimed_at = ?1 WHERE id = ?2",
            rusqlite::params![now, row.id],
        )?;
    }
    tx.commit()?;
    Ok(Decided::Done)
}

/// Release proposals claimed for a recording that did not happen, all in
/// one transaction. A proposal not claimed is left as it is.
pub fn release(
    conn: &mut rusqlite::Connection,
    ids: &[ProposalId],
) -> Result<Decided, AccountError> {
    let tx = conn.transaction()?;
    if let Err(decided) = stored_rows(&tx, ids)? {
        return Ok(decided);
    }
    for id in ids {
        tx.execute(
            "UPDATE account_proposals SET claimed_at = NULL WHERE id = ?1",
            [id.0],
        )?;
    }
    tx.commit()?;
    Ok(Decided::Done)
}

/// Whether `record` stands as the outcome `row` proposed.
fn record_fits(
    conn: &rusqlite::Connection,
    row: &Row,
    kind: ProposalKind,
    record: &str,
) -> Result<bool, AccountError> {
    Ok(match kind {
        ProposalKind::Opening => opening_stands(conn, record)?,
        ProposalKind::ListingSold => {
            record == row.subject && listing_status(conn, record)?.as_deref() == Some("sold")
        }
        ProposalKind::ListingReturned => {
            record == row.subject && listing_status(conn, record)?.as_deref() == Some("expired")
        }
        ProposalKind::Purchase => purchase_stands(conn, record)?,
        ProposalKind::Arrival => arrival_record_stands(conn, record)?,
    })
}

/// Link proposals to the record they became (the strongbox opening's id,
/// or the listing's), all in one transaction, so a set read as one opening
/// is never half linked. The record must stand and be the outcome each
/// proposed; linking again to the same record changes nothing. Linking
/// lifts any claim.
pub fn link(
    conn: &mut rusqlite::Connection,
    ids: &[ProposalId],
    record: &str,
    now: f64,
) -> Result<Decided, AccountError> {
    let record = record.trim();
    let tx = conn.transaction()?;
    let rows = match stored_rows(&tx, ids)? {
        Ok(rows) => rows,
        Err(decided) => return Ok(decided),
    };
    for row in &rows {
        let (Some(kind), Some(decision)) = (
            ProposalKind::parse(&row.kind),
            Decision::parse(&row.decision),
        ) else {
            return Ok(Decided::Refused(
                "that proposal is not one this version reads",
            ));
        };
        if decision == Decision::Dismissed {
            return Ok(Decided::Refused("that proposal was dismissed"));
        }
        if decision == Decision::Recorded && row.record.as_deref() == Some(record) {
            continue;
        }
        if !record_fits(&tx, row, kind, record)? {
            return Ok(Decided::Refused(
                "that record does not stand as the outcome the proposal is of",
            ));
        }
        if decision == Decision::Recorded {
            if let Some(earlier) = &row.record {
                let stands = match kind {
                    ProposalKind::Opening => opening_stands(&tx, earlier)?,
                    ProposalKind::Purchase => purchase_stands(&tx, earlier)?,
                    ProposalKind::Arrival => arrival_record_stands(&tx, earlier)?,
                    ProposalKind::ListingSold | ProposalKind::ListingReturned => false,
                };
                if stands {
                    return Ok(Decided::Refused("that proposal is already recorded"));
                }
            }
        }
    }
    for row in &rows {
        tx.execute(
            "UPDATE account_proposals \
             SET status = ?1, decided_at = ?2, record_id = ?3, claimed_at = NULL \
             WHERE id = ?4",
            rusqlite::params![Decision::Recorded.as_str(), now, record, row.id],
        )?;
    }
    tx.commit()?;
    Ok(Decided::Done)
}

/// Dismiss a proposal: the player says it is not what happened. Dismissing
/// it again changes nothing; one whose record stands is not dismissed.
pub fn dismiss(
    conn: &mut rusqlite::Connection,
    id: ProposalId,
    now: f64,
) -> Result<Decided, AccountError> {
    let tx = conn.transaction()?;
    let Some(row) = stored_row(&tx, id)? else {
        return Ok(Decided::NotFound);
    };
    match Decision::parse(&row.decision) {
        Some(Decision::Dismissed) => return Ok(Decided::Done),
        Some(Decision::Recorded) => {
            let stands = match (ProposalKind::parse(&row.kind), &row.record) {
                (Some(ProposalKind::Opening), Some(record)) => opening_stands(&tx, record)?,
                (Some(ProposalKind::Purchase), Some(record)) => purchase_stands(&tx, record)?,
                (Some(ProposalKind::Arrival), Some(record)) => arrival_record_stands(&tx, record)?,
                (Some(ProposalKind::ListingSold | ProposalKind::ListingReturned), _) => {
                    listing_status(&tx, &row.subject)?.as_deref() != Some("pending")
                }
                _ => false,
            };
            if stands {
                return Ok(Decided::Refused("that proposal is recorded"));
            }
        }
        Some(Decision::Pending) => {}
        None => {
            return Ok(Decided::Refused(
                "that proposal is not one this version reads",
            ))
        }
    }
    // Stock of unknown origin recorded from it stands though it was never
    // marked recorded: that is taken back, or the arrival marked, first.
    if ProposalKind::parse(&row.kind) == Some(ProposalKind::Arrival) {
        if let Some(standing) = standing_unknown_origin_of(&tx, row.id)? {
            if row.record.as_deref() != Some(standing.as_str()) {
                return Ok(Decided::Refused(
                    "stock of unknown origin recorded from it still stands; take that back, \
                     or mark the arrival recorded, first",
                ));
            }
        }
    }
    // A recording of it was begun: it is finished, or released, first.
    if row.claimed_at.is_some() {
        return Ok(Decided::Refused(
            "that proposal is being recorded; finish or release that first",
        ));
    }
    tx.execute(
        "UPDATE account_proposals SET status = ?1, decided_at = ?2, claimed_at = NULL \
         WHERE id = ?3",
        rusqlite::params![Decision::Dismissed.as_str(), now, id.0],
    )?;
    tx.commit()?;
    Ok(Decided::Done)
}

// ── Arrivals in a tracked session ───────────────────────────────────

/// An arrival a session's minutes found that nothing the app recorded
/// explains, with the step it was seen in.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrivalSeen {
    /// The later reading of the step, and the one before it.
    pub reading: ReadingId,
    pub prev_reading: ReadingId,
    pub between: Span,
    pub arrival: ArrivalProposal,
}

/// Why an arrival carries no proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotProposed {
    /// Its step reaches back to readings stored before the app recognised
    /// steps: history, never offered.
    BeforeRecognition,
    /// Its step lies wholly outside the session: what arrived then is the
    /// purchase recogniser's to read, not the session's.
    OutsideSession,
    /// The last step of a running session: what arrives in it may still be
    /// explained by what the chat log records next.
    Settling,
    /// Found on this read, and stored as a proposal as the read finishes.
    Awaiting,
}

/// What an arrival was recorded as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedAs {
    /// An asset purchase, by the purchase's id.
    Purchase,
    /// Stock of unknown origin, by its own id.
    UnknownOrigin,
}

/// What became of an arrival's proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrivalDecision {
    NotProposed(NotProposed),
    /// Waiting for the player: pending, or recorded as something since
    /// taken back. `possibly_recorded` when a recording of it was begun and
    /// never linked.
    Open {
        proposal: ProposalId,
        possibly_recorded: bool,
        /// Stock of unknown origin recorded from it that stands though the
        /// arrival was never marked recorded: what its undo names.
        unlinked_record: Option<String>,
    },
    /// Recorded, and the record stands.
    Recorded {
        proposal: ProposalId,
        record_id: String,
        recorded_as: RecordedAs,
        /// For a purchase, the asset it bought into.
        asset_name: Option<String>,
    },
    /// The player said to leave it: it is not offered again unless they
    /// take that back.
    Ignored {
        proposal: ProposalId,
    },
}

/// An arrival proposal stored for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArrival {
    pub id: ProposalId,
    /// The later reading of its step.
    pub reading: ReadingId,
    /// The item's name.
    pub subject: String,
    pub decision: ArrivalDecision,
    /// Pending and unclaimed: withdrawn when its flag is gone.
    pub withdrawable: bool,
}

/// Store arrivals of the session `session_id` as pending proposals, one for
/// each step and item (one stored already is left as it is, whatever was
/// decided of it), and withdraw the pending, unclaimed arrival proposals
/// among `withdraw` whose flag is gone. One short write. Returns how many
/// proposals were added or withdrawn.
pub fn sync_arrivals(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    seen: &[ArrivalSeen],
    withdraw: &[ProposalId],
    now: f64,
) -> Result<usize, AccountError> {
    let tx = conn.transaction()?;
    let mut changed = 0;
    for arrival in seen {
        changed += tx.execute(
            "INSERT INTO account_proposals \
                 (kind, reading_id, prev_reading_id, seen_from, seen_to, session_id, \
                  subject, payload_version, payload_json, status, detected_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, \
                     (SELECT id FROM tracking_sessions WHERE id = ?6), \
                     ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT (kind, reading_id, subject) DO NOTHING",
            rusqlite::params![
                ProposalKind::Arrival.as_str(),
                arrival.reading.0,
                arrival.prev_reading.0,
                arrival.between.from,
                arrival.between.to,
                session_id,
                arrival.arrival.key.name,
                PAYLOAD_VERSION,
                payload(&arrival.arrival)?,
                Decision::Pending.as_str(),
                now,
            ],
        )?;
    }
    for id in withdraw {
        changed += tx.execute(
            "DELETE FROM account_proposals \
             WHERE id = ?1 AND kind = ?2 AND status = ?3 AND claimed_at IS NULL",
            rusqlite::params![
                id.0,
                ProposalKind::Arrival.as_str(),
                Decision::Pending.as_str()
            ],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

/// Every arrival proposal stored for the session `session_id`, with what
/// became of it. A row of an unknown decision is passed over.
pub fn session_arrivals(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<StoredArrival>, AccountError> {
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT id, reading_id, subject, status, record_id, claimed_at \
             FROM account_proposals WHERE kind = ?1 AND session_id = ?2",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![ProposalKind::Arrival.as_str(), session_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<f64>>(5)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut stored = Vec::with_capacity(rows.len());
    for (id, reading, subject, status, record, claimed_at) in rows {
        let proposal = ProposalId(id);
        let decision = match (Decision::parse(&status), record) {
            (Some(Decision::Dismissed), _) => ArrivalDecision::Ignored { proposal },
            (Some(Decision::Recorded), Some(record)) if purchase_stands(conn, &record)? => {
                ArrivalDecision::Recorded {
                    proposal,
                    asset_name: purchased_asset_name(conn, &record)?,
                    record_id: record,
                    recorded_as: RecordedAs::Purchase,
                }
            }
            (Some(Decision::Recorded), Some(record)) if unknown_origin_stands(conn, &record)? => {
                ArrivalDecision::Recorded {
                    proposal,
                    record_id: record,
                    recorded_as: RecordedAs::UnknownOrigin,
                    asset_name: None,
                }
            }
            (Some(Decision::Recorded | Decision::Pending), _) => ArrivalDecision::Open {
                proposal,
                possibly_recorded: claimed_at.is_some(),
                unlinked_record: standing_unknown_origin_of(conn, id)?,
            },
            (None, _) => continue,
        };
        stored.push(StoredArrival {
            id: proposal,
            reading: ReadingId(reading),
            subject,
            withdrawable: Decision::parse(&status) == Some(Decision::Pending)
                && claimed_at.is_none(),
            decision,
        });
    }
    Ok(stored)
}

/// An arrival proposal that is open, for recording it as stock of unknown
/// origin: what to record, and when its step ended.
pub fn open_arrival(
    conn: &rusqlite::Connection,
    id: ProposalId,
) -> Result<Option<(ArrivalProposal, f64)>, AccountError> {
    Ok(open_proposals(conn)?
        .into_iter()
        .find(|(proposal, _)| proposal.id == id)
        .and_then(|(proposal, _)| match proposal.detail {
            Detail::Arrival(arrival) => Some((arrival, proposal.between.to)),
            _ => None,
        }))
}

/// Offer a dismissed arrival again: the player takes back having said to
/// leave it. Pending again, as it was first proposed. Restoring one that
/// is not dismissed changes nothing; only an arrival is restored so.
pub fn restore_arrival(
    conn: &mut rusqlite::Connection,
    id: ProposalId,
) -> Result<Decided, AccountError> {
    let tx = conn.transaction()?;
    let Some(row) = stored_row(&tx, id)? else {
        return Ok(Decided::NotFound);
    };
    if ProposalKind::parse(&row.kind) != Some(ProposalKind::Arrival) {
        return Ok(Decided::Refused("only an arrival is offered again so"));
    }
    match Decision::parse(&row.decision) {
        Some(Decision::Dismissed) => {}
        Some(Decision::Pending | Decision::Recorded) => return Ok(Decided::Done),
        None => {
            return Ok(Decided::Refused(
                "that proposal is not one this version reads",
            ))
        }
    }
    tx.execute(
        "UPDATE account_proposals \
         SET status = ?1, decided_at = NULL, record_id = NULL, claimed_at = NULL \
         WHERE id = ?2",
        rusqlite::params![Decision::Pending.as_str(), id.0],
    )?;
    tx.commit()?;
    Ok(Decided::Done)
}
