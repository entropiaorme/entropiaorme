//! What the readings propose: a strongbox opening, a listing sold, or a
//! listing returned, seen in the step between two consecutive readings.
//!
//! A proposal is a guess. The app never records one on its own: it fills
//! in the existing strongbox review or listing confirmation, and the player
//! records it through the existing commands or dismisses it. Nothing here
//! writes or sums an accounting figure. The strongbox openings and the
//! auction listings are read, never written: to match a listing, and to
//! tell whether the record a proposal became still stands.
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
//! - **A listing outcome** is proposed only when exactly one of the
//!   player's listings on auction from before the step matches it by item
//!   name and quantity (and TT, where the listing has one), and the
//!   departure is the only change of that item's quantity in the step.
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
    new_ammo_rows, recognise, NewAmmoRow, OpenedBoxes, OpenedContent, Opening, Settlement,
};
use super::diff::{diff, MoveKind, Span, Step, StepSet};
use super::measure::conversion_rows;
use super::reading::{Container, HoldingKey, Reading};
use super::roles::{RoleBook, RoleSource};
use super::store::{self, Purpose, ReadingId, StoredReading};
use super::AccountError;

/// The version of a stored proposal's payload. A payload of another
/// version is not offered.
pub const PAYLOAD_VERSION: i64 = 1;

/// The longest step whose PED Card rise is offered as a sale's proceeds.
pub const PROCEEDS_WITHIN_SECONDS: f64 = 180.0;

/// What a proposal is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalKind {
    Opening,
    ListingSold,
    ListingReturned,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalKind::Opening => "opening",
            ProposalKind::ListingSold => "listing_sold",
            ProposalKind::ListingReturned => "listing_returned",
        }
    }

    pub fn parse(value: &str) -> Option<ProposalKind> {
        match value {
            "opening" => Some(ProposalKind::Opening),
            "listing_sold" => Some(ProposalKind::ListingSold),
            "listing_returned" => Some(ProposalKind::ListingReturned),
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

/// What a proposal holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Opening(OpeningProposal),
    Listing(ListingProposal),
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
}

/// What one step between two readings shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepOutcomes {
    pub opening: Option<OpeningProposal>,
    pub departures: Vec<AuctionDeparture>,
}

/// Read the step from `before` to `after`: an opening, and what left the
/// auction. Pure.
pub fn step_outcomes(
    before: &Reading,
    after: &Reading,
    roles: &dyn RoleSource,
) -> Result<StepOutcomes, AccountError> {
    let pair = [before.clone(), after.clone()];
    let book = RoleBook::resolve(roles, &pair);
    let set = diff(before, after, &|name| book.stacking(name))?;
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
fn departures(set: &StepSet, before: &Reading) -> Vec<AuctionDeparture> {
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
fn local_day(at: f64) -> String {
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

/// The one listing still on auction a departure matches, by item name and
/// quantity, and by TT where the listing has one, among the listings made
/// before the step; `None` for no match or more than one.
fn matching_listing(
    conn: &rusqlite::Connection,
    departure: &AuctionDeparture,
    between: &Span,
) -> Result<Option<String>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT id, item_name, tt_value, listed_at, listed_instant FROM auction_listings \
         WHERE status = 'pending' AND undone_at IS NULL AND channel = 'auction' \
           AND quantity = ?1",
    )?;
    let candidates = stmt
        .query_map([departure.units], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let wanted = departure.name.trim().to_lowercase();
    let tolerance = i64::try_from(departure.rows)
        .unwrap_or(i64::MAX)
        .saturating_mul(MICRO_PER_PEC);
    let matching: Vec<String> = candidates
        .into_iter()
        .filter(|(_, _, _, listed_at, instant)| listed_before(listed_at, *instant, between))
        .map(|(id, name, tt, _, _)| (id, name, tt))
        .filter(|(_, name, _)| name.trim().to_lowercase() == wanted)
        .filter(|(_, _, tt)| {
            if !tt.is_finite() || *tt <= 0.0 {
                return true;
            }
            let listed = (tt * MICRO_PER_PED as f64).round() as i64;
            listed.abs_diff(departure.amount.micro_ped()) <= tolerance.unsigned_abs()
        })
        .map(|(id, _, _)| id)
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

/// The proposals the step from `before` to `after` holds.
fn found_in_step(
    conn: &rusqlite::Connection,
    before: &Reading,
    after: &Reading,
    roles: &dyn RoleSource,
) -> Result<Vec<Found>, AccountError> {
    let outcomes = step_outcomes(before, after, roles)?;
    let between = Span {
        from: before.taken_at(),
        to: after.taken_at(),
    };
    let mut found = Vec::new();
    if let Some(opening) = &outcomes.opening {
        let subject = opening
            .boxes
            .iter()
            .map(|boxes| boxes.name.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        found.push(Found {
            kind: ProposalKind::Opening,
            subject,
            payload: payload(opening)?,
        });
    }
    for departure in &outcomes.departures {
        let Some(listing_id) = matching_listing(conn, departure, &between)? else {
            continue;
        };
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
    let found = found_in_step(conn, &before.reading, &after.reading, roles)?;
    Ok(Some(Examination {
        reading,
        step: Some(ExaminedStep {
            prev,
            between: Span {
                from: before.reading.taken_at(),
                to: after.reading.taken_at(),
            },
            session: session_of(&after),
            found,
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
        for found in &step.found {
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
                    step.prev.0,
                    step.between.from,
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
fn is_the_step_s_own(error: &AccountError) -> bool {
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
}

const ROW_COLUMNS: &str = "id, kind, reading_id, prev_reading_id, seen_from, seen_to, \
                           session_id, subject, payload_version, payload_json, status, \
                           record_id, claimed_at";

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
fn accounted_for(
    openings: &[UnlinkedOpening],
    candidates: &[Candidate],
) -> std::collections::HashSet<i64> {
    let mut taken = std::collections::HashSet::new();
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
        if let Some(group) = group {
            taken.extend(group);
        }
    }
    taken
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
    }
}

/// Every open proposal, the latest seen first, with whether each is
/// claimed. Open: a pending opening, or a recorded one whose opening was
/// taken back, unless an opening recorded another way accounts for it; a
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
        let is_open = match (kind, decision, &detail) {
            (_, Decision::Dismissed, _) => false,
            (ProposalKind::Opening, _, Detail::Listing(_))
            | (ProposalKind::ListingSold | ProposalKind::ListingReturned, _, Detail::Opening(_)) => {
                false
            }
            (ProposalKind::Opening, Decision::Pending, _) => true,
            (ProposalKind::Opening, Decision::Recorded, _) => match &row.record {
                Some(record) => !opening_stands(conn, record)?,
                None => true,
            },
            (ProposalKind::ListingSold | ProposalKind::ListingReturned, _, _) => {
                listing_status(conn, &row.subject)?.as_deref() == Some("pending")
            }
        };
        if is_open {
            open.push((row, kind, detail));
        }
    }
    let candidates: Vec<Candidate> = open
        .iter()
        .filter_map(|(row, _, detail)| match detail {
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
            Detail::Listing(_) => None,
        })
        .collect();
    let accounted = if candidates.is_empty() {
        std::collections::HashSet::new()
    } else {
        accounted_for(&unlinked_openings(conn)?, &candidates)
    };
    Ok(open
        .into_iter()
        .filter(|(row, _, _)| !accounted.contains(&row.id))
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
            Some((_, true)) if !retaken => {
                return Ok(Decided::Refused(
                    "that proposal may already be recorded; check your openings first",
                ))
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
        if decision == Decision::Recorded && kind == ProposalKind::Opening {
            if let Some(earlier) = &row.record {
                if opening_stands(&tx, earlier)? {
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
    tx.execute(
        "UPDATE account_proposals SET status = ?1, decided_at = ?2, claimed_at = NULL \
         WHERE id = ?3",
        rusqlite::params![Decision::Dismissed.as_str(), now, id.0],
    )?;
    tx.commit()?;
    Ok(Decided::Done)
}
