//! Stock records the account readings make on their own.
//!
//! The account side reads the player's inventory and recognises, in the
//! step between two consecutive readings, a Shrapnel conversion, a
//! recycling into Nanocubes, or a sale to the Trade Terminal. Each is
//! recorded here exactly as the player's own entry would be, through the
//! same conversion and trade writes and the same weighted allocation, so an
//! item's origin carries forward the same way. Three things differ:
//!
//! - **Only against stock the app holds.** The amount is cut to what the
//!   open positions hold, inside the write, and nothing is recorded when
//!   they hold none. What the app never held is left for the player.
//! - **Never twice.** The record and where it came from (the two readings,
//!   when they were taken, and which part of the step it is) are written in
//!   one transaction, one record per part of a step. The source stays when
//!   the record is undone, so an undone record is never made again.
//! - **Never over the player's own entry.** When the player recorded an
//!   outflow of an item by hand since shortly before the step began (the
//!   caller says how far back: the site can show a change a reading later
//!   than the player entered it), that item's part of the step is theirs
//!   and is left.
//! - **Stock still landing is waited for.** Loot the chat log shows may be
//!   recorded a moment after the reading that shows it gone, so while the
//!   step is young (until the caller's instant) a part the open positions
//!   cannot cover in full is not recorded yet; after it, the held part is.
//!
//! A record is dated by the day its step ended, never by when it was
//! recognised. Nothing here knows what a reading is: a step arrives as plain
//! numbers.

use std::collections::HashMap;

use serde::Serialize;
use uuid::Uuid;

use super::{
    confirm_listing_within, epoch_to_iso, expire_listing_within, is_universal_ammo, item_positions,
    naive_to_epoch, write_private_sale, write_ratio_conversion, ActivityHistoryRow, AnalyticsError,
    AnalyticsService, PrivateSale, Profession, RatioConversion, STOCK_EPSILON,
};
use crate::db::DbError;

/// How far back a conversion or a sale recorded from the account is offered
/// beside the player's own conversion or trade form.
pub const RECENT_FROM_ACCOUNT_SECONDS: f64 = 60.0 * 60.0;

const SHRAPNEL: &str = "Shrapnel";
const UNIVERSAL_AMMO: &str = "Universal Ammo";
const NANOCUBE: &str = "Nanocube";

/// The step between two consecutive readings a record was seen in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccountStep {
    /// The later reading, and the one before it.
    pub reading_id: i64,
    pub prev_reading_id: i64,
    /// When the two were taken (unix seconds): it happened in between.
    pub seen_from: f64,
    pub seen_to: f64,
    /// An outflow of an item the player recorded by hand at or after this
    /// instant (unix seconds, no later than `seen_from`) is the step's part
    /// of that item, recorded already.
    pub hand_entries_since: f64,
    /// Until this instant (unix seconds), stock the open positions cannot
    /// cover in full may still be landing: the part waits.
    pub stock_settles_by: f64,
    /// How far the PED Card moved across the step, in PEC; `None` when the
    /// caller does not know it.
    pub card_change_pec: Option<i64>,
}

/// What a step showed, to record.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountRecord {
    /// Shrapnel turned into Universal Ammo at the Trade Terminal.
    ShrapnelConversion { quantity: f64 },
    /// Items recycled into Nanocubes, recorded as one conversion.
    Recycling { inputs: Vec<(String, f64)> },
    /// An item sold to the Trade Terminal, at its TT.
    TerminalSale { item_name: String, quantity: f64 },
}

impl AccountRecord {
    /// Which part of its step this is: one record per part.
    fn part(&self) -> String {
        match self {
            AccountRecord::ShrapnelConversion { .. } => "shrapnel_conversion".to_string(),
            AccountRecord::Recycling { .. } => "recycling".to_string(),
            AccountRecord::TerminalSale { item_name, .. } => {
                format!("terminal_sale:{}", item_name.trim())
            }
        }
    }

    /// What it takes out of stock, item by item.
    fn inputs(&self) -> Vec<(String, f64)> {
        match self {
            AccountRecord::ShrapnelConversion { quantity } => {
                vec![(SHRAPNEL.to_string(), *quantity)]
            }
            AccountRecord::Recycling { inputs } => inputs
                .iter()
                .map(|(name, quantity)| (name.trim().to_string(), *quantity))
                .collect(),
            AccountRecord::TerminalSale {
                item_name,
                quantity,
            } => vec![(item_name.trim().to_string(), *quantity)],
        }
    }

    fn record_kind(&self) -> &'static str {
        match self {
            AccountRecord::ShrapnelConversion { .. } | AccountRecord::Recycling { .. } => {
                "conversion"
            }
            AccountRecord::TerminalSale { .. } => "trade",
        }
    }
}

/// A listing's outcome the readings settled on their own (see
/// [`AnalyticsService::record_listing_from_account_blocking`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ListingFromAccount {
    /// It sold: at this price (PED), with the fee the game charges for
    /// selling above the starting bid.
    Sold {
        listing_id: String,
        final_price: f64,
        sale_fee: f64,
    },
    /// It came back from the auction: expired or withdrawn.
    Returned { listing_id: String },
}

impl ListingFromAccount {
    pub fn listing_id(&self) -> &str {
        match self {
            ListingFromAccount::Sold { listing_id, .. }
            | ListingFromAccount::Returned { listing_id } => listing_id,
        }
    }

    fn outcome(&self) -> &'static str {
        match self {
            ListingFromAccount::Sold { .. } => "sold",
            ListingFromAccount::Returned { .. } => "returned",
        }
    }
}

/// What recording a part of a step came to.
#[derive(Debug, Clone, PartialEq)]
pub enum FromAccount {
    /// Recorded: the record's id, and what it took out of stock.
    Recorded {
        record_id: String,
        taken: Vec<(String, f64)>,
    },
    /// This part of the step was recorded before, whether or not that
    /// record still stands.
    AlreadyRecorded,
    /// The player recorded an outflow of its items by hand since the step
    /// began.
    RecordedByHand,
    /// The app holds none of what it would take.
    NotHeld,
    /// The open positions do not yet cover all of it and the step is young:
    /// nothing was recorded, and the part may be recorded later.
    Waiting,
}

/// That a record came from the account, and the step it was seen in.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedFromAccount {
    /// The later reading of the step: what one step recorded belongs
    /// together.
    pub reading_id: i64,
    pub seen_from: f64,
    pub seen_to: f64,
    /// When the record was made (unix seconds), which may be after the step
    /// ended: stock still landing is waited for.
    pub recorded_at: f64,
    /// How far the PED Card moved across the step, in PED; `None` when it
    /// was not known as the record was made.
    pub card_change: Option<f64>,
}

/// The columns a [`RecordedFromAccount`] is read from, off a provenance
/// row aliased `s`: the step, when it was recorded, and the Card's move
/// across the step as the row keeps it.
const STEP_COLUMNS: &str = "s.reading_id, s.seen_from, s.seen_to, s.created_at, s.card_change_pec";

/// A [`RecordedFromAccount`] from [`STEP_COLUMNS`], starting at `first`.
fn step_from(row: &rusqlite::Row, first: usize) -> rusqlite::Result<RecordedFromAccount> {
    Ok(RecordedFromAccount {
        reading_id: row.get(first)?,
        seen_from: row.get(first + 1)?,
        seen_to: row.get(first + 2)?,
        recorded_at: row.get(first + 3)?,
        card_change: row
            .get::<_, Option<i64>>(first + 4)?
            .map(|pec| pec as f64 / 100.0),
    })
}

/// A conversion or a sale of an item recorded from the account lately.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentFromAccount {
    pub record_id: String,
    /// How much of the item it took, and that quantity's TT.
    pub quantity: f64,
    pub tt_value: f64,
    /// When its step ended (unix seconds).
    pub seen_to: f64,
}

/// An item a record from the account took that the player has since
/// taken back.
#[derive(Debug, Clone, PartialEq)]
pub struct TakenBack {
    pub item_name: String,
    pub record_id: String,
    /// When the step it was recorded from ended (unix seconds).
    pub seen_to: f64,
}

fn check_step(step: &AccountStep) -> Result<(), AnalyticsError> {
    let finite = [
        step.seen_from,
        step.seen_to,
        step.hand_entries_since,
        step.stock_settles_by,
    ]
    .iter()
    .all(|instant| instant.is_finite());
    if !finite || step.seen_to < step.seen_from || step.reading_id == step.prev_reading_id {
        return Err(AnalyticsError::InvalidInput(
            "a step lies between two different readings, the later taken last",
        ));
    }
    if step.hand_entries_since > step.seen_from {
        return Err(AnalyticsError::InvalidInput(
            "hand entries are looked for from no later than the step began",
        ));
    }
    Ok(())
}

fn check_inputs(inputs: &[(String, f64)]) -> Result<(), AnalyticsError> {
    if inputs.is_empty() {
        return Err(AnalyticsError::InvalidInput(
            "a record from the account takes something out of stock",
        ));
    }
    for (name, quantity) in inputs {
        if name.trim().is_empty() || !quantity.is_finite() || *quantity <= 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "each item a record takes needs a name and a positive quantity",
            ));
        }
        if is_universal_ammo(name) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held as stock",
            ));
        }
    }
    Ok(())
}

/// Whether the player recorded an outflow of `item` by hand since `since`:
/// a movement out of stock that no record from the account made.
fn recorded_by_hand_since(
    conn: &rusqlite::Connection,
    item: &str,
    since: f64,
) -> Result<bool, DbError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_movements m \
                        WHERE m.item_name = ?1 AND m.quantity < 0 AND m.created_at >= ?2 \
                          AND NOT EXISTS (SELECT 1 FROM stock_records_from_account s \
                                          WHERE s.record_id = m.ref_id))",
        rusqlite::params![item, since],
        |row| row.get::<_, bool>(0),
    )?)
}

/// How much of `item` the open positions hold.
pub(super) fn held(conn: &rusqlite::Connection, item: &str) -> Result<f64, DbError> {
    let (positions, _) = item_positions(conn, item)?;
    Ok(positions
        .iter()
        .map(|position| position.quantity)
        .filter(|quantity| *quantity > STOCK_EPSILON)
        .sum())
}

/// Every record from the account, by its record's id.
fn sources(conn: &rusqlite::Connection) -> Result<HashMap<String, RecordedFromAccount>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.record_id, {STEP_COLUMNS} FROM stock_records_from_account s"
    ))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, step_from(row, 1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(rows)
}

/// Mark the history entries a record from the account made, a listing
/// whose outcome the readings settled among them.
pub(super) fn attach_sources(
    conn: &rusqlite::Connection,
    rows: &mut [ActivityHistoryRow],
) -> Result<(), DbError> {
    let sources = sources(conn)?;
    let listings = listing_sources(conn)?;
    if sources.is_empty() && listings.is_empty() {
        return Ok(());
    }
    for row in rows.iter_mut() {
        if row.kind == "conversion" || row.kind == "trade" {
            row.from_account = sources.get(&row.id).copied();
        } else if row.kind == "listing" {
            row.from_account = listings.get(&row.id).copied();
        }
    }
    Ok(())
}

/// One part of a recognised step, recorded in one transaction: refused
/// once already recorded, against held stock only, and kept out of what
/// the player recorded by hand since the step's look-back.
fn record_in(
    conn: &mut rusqlite::Connection,
    step: AccountStep,
    record: &AccountRecord,
    now: f64,
) -> Result<Result<FromAccount, String>, DbError> {
    let tx = conn.transaction()?;
    let part = record.part();
    let recorded: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_records_from_account \
                        WHERE reading_id = ?1 AND part = ?2)",
        rusqlite::params![step.reading_id, part],
        |row| row.get(0),
    )?;
    if recorded {
        return Ok(Ok(FromAccount::AlreadyRecorded));
    }
    let young = now < step.stock_settles_by;
    let mut taken: Vec<(String, f64)> = Vec::new();
    let mut by_hand = false;
    for (item, quantity) in record.inputs() {
        if recorded_by_hand_since(&tx, &item, step.hand_entries_since)? {
            by_hand = true;
            continue;
        }
        let held = held(&tx, &item)?;
        if young && held + STOCK_EPSILON < quantity {
            return Ok(Ok(FromAccount::Waiting));
        }
        let quantity = quantity.min(held);
        if quantity > STOCK_EPSILON {
            taken.push((item, quantity));
        }
    }
    if taken.is_empty() {
        return Ok(Ok(if by_hand {
            FromAccount::RecordedByHand
        } else {
            FromAccount::NotHeld
        }));
    }

    let id = Uuid::new_v4().to_string();
    let day = epoch_to_iso(step.seen_to);
    let written = match record {
        AccountRecord::ShrapnelConversion { .. } => write_ratio_conversion(
            &tx,
            &id,
            Profession::Inventory,
            &RatioConversion {
                inputs: &taken,
                target: UNIVERSAL_AMMO,
                value_ratio: 1.01,
            },
            &day,
            now,
        )?,
        AccountRecord::Recycling { .. } => write_ratio_conversion(
            &tx,
            &id,
            Profession::Inventory,
            &RatioConversion {
                inputs: &taken,
                target: NANOCUBE,
                value_ratio: 1.0,
            },
            &day,
            now,
        )?,
        AccountRecord::TerminalSale { .. } => {
            let (item, quantity) = &taken[0];
            write_private_sale(
                &tx,
                &PrivateSale {
                    id: &id,
                    profession: Profession::Inventory,
                    item,
                    quantity: *quantity,
                    price: None,
                    sold_at: &day,
                },
                now,
            )?;
            Ok(())
        }
    };
    if let Err(reason) = written {
        return Ok(Err(reason));
    }
    tx.execute(
        "INSERT INTO stock_records_from_account \
             (record_kind, record_id, reading_id, prev_reading_id, seen_from, \
              seen_to, part, item_name, created_at, card_change_pec) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            record.record_kind(),
            id,
            step.reading_id,
            step.prev_reading_id,
            step.seen_from,
            step.seen_to,
            part,
            taken[0].0,
            now,
            step.card_change_pec,
        ],
    )?;
    tx.commit()?;
    Ok(Ok(FromAccount::Recorded {
        record_id: id,
        taken,
    }))
}

/// A listing's outcome the readings settled, in one transaction: through
/// the same writes as the player's own confirmation or expiry, refused once
/// recorded from the step already, and left when the listing no longer
/// waits on the auction (the player resolved it, or took it back).
fn record_listing_in(
    conn: &mut rusqlite::Connection,
    step: AccountStep,
    outcome: &ListingFromAccount,
    resolved_on: &str,
    now: f64,
) -> Result<FromAccount, DbError> {
    let tx = conn.transaction()?;
    let listing_id = outcome.listing_id();
    let recorded: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM listing_outcomes_from_account \
                        WHERE reading_id = ?1 AND listing_id = ?2)",
        rusqlite::params![step.reading_id, listing_id],
        |row| row.get(0),
    )?;
    if recorded {
        return Ok(FromAccount::AlreadyRecorded);
    }
    let resolved = match outcome {
        ListingFromAccount::Sold {
            final_price,
            sale_fee,
            ..
        } => confirm_listing_within(&tx, listing_id, *final_price, *sale_fee, resolved_on)?,
        ListingFromAccount::Returned { .. } => {
            expire_listing_within(&tx, listing_id, resolved_on, now)?
        }
    };
    let Some(listing) = resolved else {
        return Ok(FromAccount::RecordedByHand);
    };
    tx.execute(
        "INSERT INTO listing_outcomes_from_account \
             (listing_id, outcome, reading_id, prev_reading_id, seen_from, seen_to, \
              created_at, card_change_pec) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            listing_id,
            outcome.outcome(),
            step.reading_id,
            step.prev_reading_id,
            step.seen_from,
            step.seen_to,
            now,
            step.card_change_pec,
        ],
    )?;
    tx.commit()?;
    Ok(FromAccount::Recorded {
        record_id: listing.id,
        taken: Vec::new(),
    })
}

/// When the readings settled each listing whose outcome stands, by the
/// listing's id: the step of the latest outcome recorded from them.
pub(super) fn listing_sources(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, RecordedFromAccount>, DbError> {
    let mut stmt = conn.prepare(&format!(
        // id-order: insertion (a later outcome of the same listing replaces an earlier)
        "SELECT s.listing_id, {STEP_COLUMNS} \
         FROM listing_outcomes_from_account s \
         JOIN auction_listings l ON l.id = s.listing_id \
         WHERE l.undone_at IS NULL \
           AND l.status = CASE s.outcome WHEN 'sold' THEN 'sold' ELSE 'expired' END \
         ORDER BY s.id"
    ))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, step_from(row, 1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(rows)
}

impl AnalyticsService {
    /// Record a listing's outcome the account readings settled, through the
    /// same writes as the player's own confirmation or expiry, dated
    /// `resolved_on` (the local day its step ended), with the step it came
    /// from, all in one transaction. Recording it again from the same step
    /// records nothing; a listing the player resolved meanwhile is left.
    ///
    /// Blocking, like [`Self::record_from_account_blocking`]: for the
    /// account connection's pass over the steps it read.
    pub fn record_listing_from_account_blocking(
        &self,
        step: AccountStep,
        outcome: ListingFromAccount,
        resolved_on: String,
    ) -> Result<FromAccount, AnalyticsError> {
        check_step(&step)?;
        if outcome.listing_id().trim().is_empty() {
            return Err(AnalyticsError::InvalidInput(
                "a listing outcome names its listing",
            ));
        }
        if let ListingFromAccount::Sold {
            final_price,
            sale_fee,
            ..
        } = &outcome
        {
            if !final_price.is_finite()
                || !sale_fee.is_finite()
                || *final_price < 0.0
                || *sale_fee < 0.0
            {
                return Err(AnalyticsError::InvalidInput(
                    "a sale price and fee cannot be negative",
                ));
            }
        }
        let now = naive_to_epoch(self.clock.now());
        Ok(self.db.with_writer_blocking(move |conn| {
            record_listing_in(conn, step, &outcome, &resolved_on, now)
        })?)
    }

    /// Record one part of a step the account readings recognised, through
    /// the same writes as the player's own entry, against held stock only,
    /// with where it came from, all in one transaction. Recording the same
    /// part of the same step again records nothing.
    pub async fn record_from_account(
        &self,
        step: AccountStep,
        record: AccountRecord,
    ) -> Result<FromAccount, AnalyticsError> {
        check_step(&step)?;
        check_inputs(&record.inputs())?;
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| record_in(conn, step, &record, now))
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// The blocking counterpart of [`Self::record_from_account`], for work
    /// already on a blocking thread (the account connection's pass over
    /// the steps it read). Never call it on an async runtime's worker.
    pub fn record_from_account_blocking(
        &self,
        step: AccountStep,
        record: AccountRecord,
    ) -> Result<FromAccount, AnalyticsError> {
        check_step(&step)?;
        check_inputs(&record.inputs())?;
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer_blocking(move |conn| record_in(conn, step, &record, now))?
            .map_err(AnalyticsError::Rejected)
    }

    /// The latest standing conversion of `item_name` the account readings
    /// recorded, when its step ended within the last
    /// [`RECENT_FROM_ACCOUNT_SECONDS`]: what the player's own conversion
    /// form shows beside the amount, so the same conversion is not entered
    /// twice.
    pub async fn recent_conversion_from_account(
        &self,
        item_name: &str,
    ) -> Result<Option<RecentFromAccount>, AnalyticsError> {
        let item = item_name.trim().to_string();
        if item.is_empty() {
            return Ok(None);
        }
        let since = naive_to_epoch(self.clock.now()) - RECENT_FROM_ACCOUNT_SECONDS;
        Ok(self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(
                        // id-order: tiebreak (the latest step first; id splits equal instants)
                        "SELECT s.record_id, SUM(l.quantity), SUM(l.tt_value), s.seen_to \
                         FROM stock_records_from_account s \
                         JOIN stock_conversions c ON c.id = s.record_id \
                         JOIN stock_conversion_lines l ON l.conversion_id = c.id \
                         WHERE s.record_kind = 'conversion' AND c.undone_at IS NULL \
                           AND l.side = 'input' AND l.item_name = ?1 AND s.seen_to >= ?2 \
                         GROUP BY s.id \
                         ORDER BY s.seen_to DESC, s.id DESC LIMIT 1",
                        rusqlite::params![item, since],
                        |row| {
                            Ok(RecentFromAccount {
                                record_id: row.get(0)?,
                                quantity: row.get(1)?,
                                tt_value: row.get(2)?,
                                seen_to: row.get(3)?,
                            })
                        },
                    )
                    .optional()?)
            })
            .await?)
    }

    /// The latest standing sale of `item_name` to the Trade Terminal the
    /// account readings recorded, when its step ended within the last
    /// [`RECENT_FROM_ACCOUNT_SECONDS`]: what the player's own trade form
    /// shows beside the amount, so the same sale is not entered twice.
    pub async fn recent_sale_from_account(
        &self,
        item_name: &str,
    ) -> Result<Option<RecentFromAccount>, AnalyticsError> {
        let item = item_name.trim().to_string();
        if item.is_empty() {
            return Ok(None);
        }
        let since = naive_to_epoch(self.clock.now()) - RECENT_FROM_ACCOUNT_SECONDS;
        Ok(self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(
                        // id-order: tiebreak (the latest step first; id splits equal instants)
                        "SELECT s.record_id, p.quantity, p.tt_value, s.seen_to \
                         FROM stock_records_from_account s \
                         JOIN private_sales p ON p.id = s.record_id \
                         WHERE s.record_kind = 'trade' AND p.undone_at IS NULL \
                           AND p.item_name = ?1 AND s.seen_to >= ?2 \
                         ORDER BY s.seen_to DESC, s.id DESC LIMIT 1",
                        rusqlite::params![item, since],
                        |row| {
                            Ok(RecentFromAccount {
                                record_id: row.get(0)?,
                                quantity: row.get(1)?,
                                tt_value: row.get(2)?,
                                seen_to: row.get(3)?,
                            })
                        },
                    )
                    .optional()?)
            })
            .await?)
    }

    /// Every item a record from the account took that the player has since
    /// undone, the earliest step first: what a reconciliation leaves as it
    /// is unless told otherwise.
    pub async fn account_records_taken_back(&self) -> Result<Vec<TakenBack>, AnalyticsError> {
        Ok(self.db.with_reader(|conn| Ok(taken_back(conn)?)).await?)
    }
}

/// Every item a record from the account took that the player has since
/// undone, the earliest step first.
pub(super) fn taken_back(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<TakenBack>> {
    let mut stmt = conn.prepare(
        // id-order: tiebreak (the earliest step first; id splits equal instants)
        "SELECT l.item_name, s.record_id, s.seen_to, s.id \
         FROM stock_records_from_account s \
         JOIN stock_conversions c ON c.id = s.record_id \
         JOIN stock_conversion_lines l ON l.conversion_id = c.id \
         WHERE s.record_kind = 'conversion' AND c.undone_at IS NOT NULL \
           AND l.side = 'input' \
         UNION ALL \
         SELECT p.item_name, s.record_id, s.seen_to, s.id \
         FROM stock_records_from_account s \
         JOIN private_sales p ON p.id = s.record_id \
         WHERE s.record_kind = 'trade' AND p.undone_at IS NOT NULL \
         ORDER BY 3, 4",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TakenBack {
                item_name: row.get(0)?,
                record_id: row.get(1)?,
                seen_to: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// What taking back stock of unknown origin came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownOriginUndo {
    /// Taken back: its stock is gone.
    Undone,
    /// No such record stands: none was made, or it was taken back already.
    NotFound,
}

/// Stock of unknown origin recorded from an arrival.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOriginRecorded {
    pub record_id: String,
    /// False when a record from the same arrival already stood: it is that
    /// one, and nothing new was written.
    pub written: bool,
}

/// The standing stock of unknown origin recorded from the arrival
/// proposal `proposal`, if any.
fn standing_from(conn: &rusqlite::Connection, proposal: i64) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    conn.query_row(
        "SELECT id FROM stock_unknown_origins \
         WHERE proposal_id = ?1 AND undone_at IS NULL LIMIT 1",
        [proposal],
        |row| row.get(0),
    )
    .optional()
}

impl AnalyticsService {
    /// Hold `quantity` of `item_name`, worth `tt_value` in TT, as stock of
    /// unknown origin: something the account readings saw arrive that the
    /// player says came from nowhere the app knows. Its stock is one
    /// unattributed movement, which funds no activity, so its TT is never
    /// income and a later sale above TT books only the markup. Dated by the
    /// day its step ended (`seen_to`, unix seconds). `from_proposal` names
    /// the arrival proposal it is recorded from: while a record from that
    /// arrival stands, recording it again writes nothing and hands that
    /// record back.
    pub async fn record_unknown_origin(
        &self,
        item_name: &str,
        quantity: f64,
        tt_value: f64,
        seen_to: f64,
        from_proposal: Option<i64>,
    ) -> Result<UnknownOriginRecorded, AnalyticsError> {
        let item = item_name.trim().to_string();
        if item.is_empty() || !quantity.is_finite() || quantity <= 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "stock of unknown origin needs an item and a positive quantity",
            ));
        }
        if !tt_value.is_finite() || tt_value < 0.0 || !seen_to.is_finite() {
            return Err(AnalyticsError::InvalidInput(
                "stock of unknown origin needs a TT of at least zero and when it arrived",
            ));
        }
        if is_universal_ammo(&item) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held as stock",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let day = epoch_to_iso(seen_to);
        let now = naive_to_epoch(self.clock.now());
        Ok(self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Some(proposal) = from_proposal {
                    if let Some(standing) = standing_from(&tx, proposal)? {
                        return Ok(UnknownOriginRecorded {
                            record_id: standing,
                            written: false,
                        });
                    }
                }
                write_unknown_origin(&tx, &id, &item, quantity, tt_value, &day, now)?;
                tx.execute(
                    "UPDATE stock_unknown_origins SET proposal_id = ?1 WHERE id = ?2",
                    rusqlite::params![from_proposal, id],
                )?;
                tx.commit()?;
                Ok(UnknownOriginRecorded {
                    record_id: id,
                    written: true,
                })
            })
            .await?)
    }

    /// Take back stock of unknown origin: its movement goes and the record
    /// is marked undone, as a stock removal is taken back. What it was
    /// recorded from is then open to record again. Refused for stock a
    /// standing reconciliation recorded, which is undone with it.
    pub async fn undo_unknown_origin(
        &self,
        record_id: &str,
    ) -> Result<UnknownOriginUndo, AnalyticsError> {
        let record = record_id.trim().to_string();
        let undone_at = self.default_date();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if super::reconciliation::standing_reconciliation_of(&tx, &record)?.is_some() {
                    return Ok(Err(
                        super::reconciliation::HELD_BY_RECONCILIATION.to_string()
                    ));
                }
                let undone = undo_unknown_origin_within(&tx, &record, &undone_at)?;
                if undone == UnknownOriginUndo::Undone {
                    tx.commit()?;
                }
                Ok(Ok(undone))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }
}

/// The history entries of stock held as of unknown origin, from an arrival
/// the readings saw or a reconciliation's surplus, undone ones included.
/// They belong to no activity, so only the Inventory's own history shows
/// them.
pub(super) fn push_unknown_origin_rows(
    conn: &rusqlite::Connection,
    profession: Profession,
    rows: &mut Vec<ActivityHistoryRow>,
) -> rusqlite::Result<()> {
    if profession != Profession::Inventory {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT id, item_name, quantity, tt_value, found_at, undone_at \
         FROM stock_unknown_origins",
    )?;
    let records = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, item, quantity, tt, found_at, undone_at) in records {
        rows.push(ActivityHistoryRow {
            conversion_lines: Vec::new(),
            ped_amount: None,
            itemised: false,
            derived_from: Vec::new(),
            from_account: None,
            reconciliation_id: None,
            id,
            kind: "unknownOrigin".to_string(),
            subject_kind: "loot".to_string(),
            channel: "unknownOrigin".to_string(),
            status: "held".to_string(),
            item_name: item,
            target_item: None,
            occurred_at: found_at,
            quantity,
            tt_value: tt,
            net_markup: None,
            activity_net_markup: None,
            can_revert_sale: false,
            can_delete: undone_at.is_none(),
            undo_blocked_reason: None,
            undone: undone_at.is_some(),
        });
    }
    Ok(())
}

/// Hold `quantity` of `item`, worth `tt_value`, as stock of unknown origin
/// inside the caller's transaction, under the record id `id`, dated `day`:
/// the record, and the one unattributed movement that is its stock.
pub(super) fn write_unknown_origin(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    item: &str,
    quantity: f64,
    tt_value: f64,
    day: &str,
    now: f64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stock_unknown_origins \
             (id, item_name, quantity, tt_value, found_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![id, item, quantity, tt_value, day, now],
    )?;
    super::insert_movement(
        tx,
        item,
        "opening_balance",
        Some(id),
        None,
        None,
        None,
        None,
        quantity,
        tt_value,
        day,
        now,
        crate::stock_allocation::DerivedFacts::default(),
    )
}

/// Take back stock of unknown origin inside the caller's transaction: its
/// movement goes and the record is marked undone.
pub(super) fn undo_unknown_origin_within(
    tx: &rusqlite::Transaction<'_>,
    record: &str,
    undone_at: &str,
) -> rusqlite::Result<UnknownOriginUndo> {
    let standing: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_unknown_origins \
                        WHERE id = ?1 AND undone_at IS NULL)",
        [record],
        |row| row.get(0),
    )?;
    if !standing {
        return Ok(UnknownOriginUndo::NotFound);
    }
    tx.execute(
        "DELETE FROM stock_movements \
         WHERE ref_id = ?1 AND movement_kind = 'opening_balance'",
        [record],
    )?;
    tx.execute(
        "UPDATE stock_unknown_origins SET undone_at = ?1 WHERE id = ?2",
        rusqlite::params![undone_at, record],
    )?;
    Ok(UnknownOriginUndo::Undone)
}

#[cfg(test)]
mod tests {
    use super::super::conversion::tests::{ledger, seed_hunted, stock};
    use super::super::tests::write_service;
    use super::*;
    use crate::analytics::{ConversionHolding, ConversionInput, ConversionOutput, ItemConversion};

    /// The test service's clock, as the app reads it: every step below is
    /// placed from it, so no figure depends on the machine's time zone.
    fn noon() -> f64 {
        naive_to_epoch(
            chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S")
                .unwrap(),
        )
    }

    /// A minute's step ending `ago` seconds before the clock, looking for
    /// hand entries from five minutes before it began, its stock settled
    /// ten minutes after it ended.
    fn step_ending(reading: i64, ago: f64) -> AccountStep {
        let seen_to = noon() - ago;
        AccountStep {
            reading_id: reading,
            prev_reading_id: reading - 1,
            seen_from: seen_to - 60.0,
            seen_to,
            hand_entries_since: seen_to - 60.0 - 300.0,
            stock_settles_by: seen_to + 600.0,
            card_change_pec: None,
        }
    }

    /// A step that ended an hour before the clock: settled.
    fn step(reading: i64) -> AccountStep {
        step_ending(reading, 3_600.0)
    }

    async fn record(
        service: &AnalyticsService,
        reading: i64,
        record: AccountRecord,
    ) -> FromAccount {
        service
            .record_from_account(step(reading), record)
            .await
            .unwrap()
    }

    fn recorded_id(outcome: &FromAccount) -> String {
        match outcome {
            FromAccount::Recorded { record_id, .. } => record_id.clone(),
            other => panic!("expected a record, got {other:?}"),
        }
    }

    async fn history(service: &AnalyticsService) -> Vec<ActivityHistoryRow> {
        service
            .activity_history(Profession::Inventory)
            .await
            .unwrap()
    }

    /// Bring `quantity` of `item` into stock bought for `markup` above its
    /// TT, through a conversion of something bought, so the stock carries
    /// paid markup.
    async fn seed_bought(
        service: &AnalyticsService,
        item: &str,
        quantity: f64,
        tt: f64,
        markup: f64,
    ) {
        service
            .convert_items(
                Profession::Inventory,
                ItemConversion {
                    inputs: vec![ConversionInput::Bought {
                        item_name: format!("{item} Voucher"),
                        quantity: 1.0,
                        tt_value: tt,
                        markup_paid: markup,
                    }],
                    outputs: vec![ConversionOutput {
                        item_name: item.to_string(),
                        quantity,
                        tt_value: tt,
                        holding: ConversionHolding::Loot,
                    }],
                    ped_amount: None,
                    converted_at: Some("2026-05-30".to_string()),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_shrapnel_conversion_is_recorded_as_the_player_s_own_and_dated_by_its_step() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 100_000, 10.0).await;
        let outcome = record(
            &service,
            2,
            AccountRecord::ShrapnelConversion { quantity: 40_000.0 },
        )
        .await;
        let id = recorded_id(&outcome);
        let left = stock(&service, SHRAPNEL).await.expect("still held");
        assert!((left.quantity - 60_000.0).abs() < 1e-6);
        // The 1% the Trade Terminal adds is realised, as by hand.
        assert!(ledger(&service)
            .await
            .iter()
            .any(|(kind, description, amount)| kind == "markup"
                && description == "Shrapnel Conversion"
                && (amount - 0.04).abs() < 1e-9));
        let entry = history(&service)
            .await
            .into_iter()
            .find(|row| row.id == id)
            .expect("on the history");
        // Dated by the day its step ended, not by the clock's.
        assert_eq!(entry.occurred_at, epoch_to_iso(step(2).seen_to));
        let source = entry.from_account.expect("from the account");
        assert_eq!(
            (source.reading_id, source.seen_from, source.seen_to),
            (2, step(2).seen_from, step(2).seen_to)
        );
        assert_eq!(source.recorded_at, noon(), "recorded when the clock says");
        assert_eq!(source.card_change, None, "the step named no Card move");
    }

    #[tokio::test]
    async fn a_record_keeps_how_far_the_card_moved_across_its_step() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 100_000, 10.0).await;
        let moved = AccountStep {
            card_change_pec: Some(-250),
            ..step(2)
        };
        let outcome = service
            .record_from_account(
                moved,
                AccountRecord::ShrapnelConversion { quantity: 40_000.0 },
            )
            .await
            .unwrap();
        let id = recorded_id(&outcome);
        let source = history(&service)
            .await
            .into_iter()
            .find(|row| row.id == id)
            .expect("on the history")
            .from_account
            .expect("from the account");
        // Kept on the record itself, in PEC, and shown in PED.
        assert_eq!(source.card_change, Some(-2.5));
    }

    #[tokio::test]
    async fn the_same_part_of_a_step_is_recorded_once_and_never_again_once_undone() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 100_000, 10.0).await;
        let first = record(
            &service,
            2,
            AccountRecord::ShrapnelConversion { quantity: 10_000.0 },
        )
        .await;
        let id = recorded_id(&first);
        assert_eq!(
            record(
                &service,
                2,
                AccountRecord::ShrapnelConversion { quantity: 10_000.0 }
            )
            .await,
            FromAccount::AlreadyRecorded
        );
        assert!(service.undo_stock_conversion(&id).await.unwrap());
        assert_eq!(
            record(
                &service,
                2,
                AccountRecord::ShrapnelConversion { quantity: 10_000.0 }
            )
            .await,
            FromAccount::AlreadyRecorded
        );
        let held = stock(&service, SHRAPNEL).await.expect("held");
        assert!((held.quantity - 100_000.0).abs() < 1e-6);
        // The undone record is what a reconciliation needs to know of.
        let taken_back = service.account_records_taken_back().await.unwrap();
        assert_eq!(taken_back.len(), 1);
        assert_eq!(taken_back[0].item_name, SHRAPNEL);
        assert_eq!(taken_back[0].record_id, id);
    }

    #[tokio::test]
    async fn only_the_held_part_is_recorded_once_settled_and_nothing_when_none_is_held() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, "Animal Oil Residue", 300, 3.0).await;
        let outcome = record(
            &service,
            2,
            AccountRecord::TerminalSale {
                item_name: "Animal Oil Residue".to_string(),
                quantity: 500.0,
            },
        )
        .await;
        let FromAccount::Recorded { taken, .. } = outcome else {
            panic!("a sale of what is held");
        };
        assert_eq!(taken, vec![("Animal Oil Residue".to_string(), 300.0)]);
        // No unattributed stock was invented for the rest.
        let left = stock(&service, "Animal Oil Residue").await;
        assert!(left.is_none_or(|row| row.quantity.abs() < 1e-9));
        assert_eq!(
            record(
                &service,
                3,
                AccountRecord::TerminalSale {
                    item_name: "Lysterium Stone".to_string(),
                    quantity: 10.0,
                },
            )
            .await,
            FromAccount::NotHeld
        );
    }

    #[tokio::test]
    async fn part_of_the_shrapnel_held_is_converted_once_the_step_has_settled() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 30_000, 3.0).await;
        let outcome = record(
            &service,
            2,
            AccountRecord::ShrapnelConversion { quantity: 40_000.0 },
        )
        .await;
        let FromAccount::Recorded { taken, .. } = outcome else {
            panic!("the held part converted");
        };
        assert_eq!(taken, vec![(SHRAPNEL.to_string(), 30_000.0)]);
        let left = stock(&service, SHRAPNEL).await;
        assert!(left.is_none_or(|row| row.quantity.abs() < 1e-9));
    }

    #[tokio::test]
    async fn a_young_step_waits_for_stock_still_landing_and_records_it_all_after() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 30_000, 3.0).await;
        // Ended a minute ago: the loot the chat log shows may not be in yet.
        let young = step_ending(2, 60.0);
        let conversion = || AccountRecord::ShrapnelConversion { quantity: 40_000.0 };
        assert_eq!(
            service
                .record_from_account(young, conversion())
                .await
                .unwrap(),
            FromAccount::Waiting
        );
        let held = stock(&service, SHRAPNEL).await.expect("untouched");
        assert!((held.quantity - 30_000.0).abs() < 1e-6);
        // The rest lands, and the step is tried again.
        seed_hunted(&service, 7, SHRAPNEL, 10_000, 1.0).await;
        let FromAccount::Recorded { taken, .. } = service
            .record_from_account(young, conversion())
            .await
            .unwrap()
        else {
            panic!("all of it, now held");
        };
        assert_eq!(taken, vec![(SHRAPNEL.to_string(), 40_000.0)]);
    }

    #[tokio::test]
    async fn a_manual_record_made_since_shortly_before_the_step_keeps_the_step_s_part_out() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 100_000, 10.0).await;
        // The player's entry is made at the clock.
        service
            .convert_shrapnel(Profession::Inventory, 10_000.0, None)
            .await
            .unwrap();
        // A step that began two minutes after it: within the look back.
        let mut later = step_ending(3, -180.0);
        assert!(later.hand_entries_since <= noon() && noon() < later.seen_from);
        assert_eq!(
            service
                .record_from_account(
                    later,
                    AccountRecord::ShrapnelConversion { quantity: 10_000.0 },
                )
                .await
                .unwrap(),
            FromAccount::RecordedByHand
        );
        // One that began well after it: the entry was another conversion.
        later = step_ending(9, -1_200.0);
        later.stock_settles_by = later.seen_from;
        assert!(noon() < later.hand_entries_since);
        let outcome = service
            .record_from_account(
                later,
                AccountRecord::ShrapnelConversion { quantity: 10_000.0 },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, FromAccount::Recorded { .. }));
        let held = stock(&service, SHRAPNEL).await.expect("held");
        assert!((held.quantity - 80_000.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn a_terminal_sale_of_bought_stock_realises_its_markup_as_the_manual_sale_does() {
        let (_dir, automatic) = write_service().await;
        let (_dir2, manual) = write_service().await;
        for service in [&automatic, &manual] {
            seed_bought(service, "Garcen Grease", 10.0, 2.0, 0.6).await;
        }
        record(
            &automatic,
            2,
            AccountRecord::TerminalSale {
                item_name: "Garcen Grease".to_string(),
                quantity: 10.0,
            },
        )
        .await;
        manual
            .create_private_sale(Profession::Inventory, "Garcen Grease", 10.0, 2.0, None)
            .await
            .unwrap();
        let sold = |rows: Vec<(String, String, f64)>| {
            rows.into_iter()
                .filter(|(_, description, _)| description.starts_with("Private Sale"))
                .collect::<Vec<_>>()
        };
        let automatic_ledger = sold(ledger(&automatic).await);
        assert_eq!(automatic_ledger, sold(ledger(&manual).await));
        assert_eq!(automatic_ledger.len(), 1);
        assert_eq!(automatic_ledger[0].0, "expense");
        assert!((automatic_ledger[0].2 - 0.6).abs() < 1e-9);
    }

    #[tokio::test]
    async fn recycling_carries_each_item_s_origin_forward_as_the_hand_entered_path_does() {
        let (_dir, automatic) = write_service().await;
        let (_dir2, manual) = write_service().await;
        for service in [&automatic, &manual] {
            seed_hunted(service, 5, "Animal Oil Residue", 60, 0.6).await;
            seed_hunted(service, 7, "Animal Oil Residue", 40, 0.4).await;
        }
        let outcome = record(
            &automatic,
            2,
            AccountRecord::Recycling {
                inputs: vec![("Animal Oil Residue".to_string(), 50.0)],
            },
        )
        .await;
        recorded_id(&outcome);
        manual
            .convert_stock(
                Profession::Inventory,
                "Animal Oil Residue",
                NANOCUBE,
                50.0,
                None,
            )
            .await
            .unwrap();
        for item in ["Animal Oil Residue", NANOCUBE] {
            let from_account = stock(&automatic, item).await.expect("held");
            let by_hand = stock(&manual, item).await.expect("held");
            assert!(
                (from_account.quantity - by_hand.quantity).abs() < 1e-9,
                "{item}"
            );
            assert_eq!(from_account.origins, by_hand.origins, "{item}");
            assert_eq!(from_account.derived_from, by_hand.derived_from, "{item}");
        }
        let nanocubes = stock(&automatic, NANOCUBE).await.expect("produced");
        assert!((nanocubes.quantity - 50.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn recycling_several_items_is_one_conversion() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, "Animal Oil Residue", 100, 1.0).await;
        seed_hunted(&service, 5, "Iron Stone", 10, 1.3).await;
        let outcome = record(
            &service,
            2,
            AccountRecord::Recycling {
                inputs: vec![
                    ("Animal Oil Residue".to_string(), 100.0),
                    ("Iron Stone".to_string(), 10.0),
                ],
            },
        )
        .await;
        let id = recorded_id(&outcome);
        let conversions: Vec<_> = history(&service)
            .await
            .into_iter()
            .filter(|row| row.kind == "conversion")
            .collect();
        assert_eq!(conversions.len(), 1);
        assert_eq!(conversions[0].id, id);
        let nanocubes = stock(&service, NANOCUBE).await.expect("produced");
        assert!((nanocubes.quantity - 230.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn the_forms_hear_of_a_conversion_or_sale_recorded_within_the_hour_only() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, 5, SHRAPNEL, 100_000, 10.0).await;
        seed_hunted(&service, 5, "Iron Stone", 10, 1.3).await;
        assert_eq!(
            service
                .recent_conversion_from_account(SHRAPNEL)
                .await
                .unwrap(),
            None
        );
        // Each ended under an hour ago.
        let id = recorded_id(
            &service
                .record_from_account(
                    step_ending(2, 1_800.0),
                    AccountRecord::ShrapnelConversion { quantity: 20_000.0 },
                )
                .await
                .unwrap(),
        );
        let sale = recorded_id(
            &service
                .record_from_account(
                    step_ending(4, 1_800.0),
                    AccountRecord::TerminalSale {
                        item_name: "Iron Stone".to_string(),
                        quantity: 4.0,
                    },
                )
                .await
                .unwrap(),
        );
        let recent = service
            .recent_conversion_from_account(SHRAPNEL)
            .await
            .unwrap()
            .expect("within the hour");
        assert_eq!(recent.record_id, id);
        assert!((recent.quantity - 20_000.0).abs() < 1e-6);
        assert!((recent.tt_value - 2.0).abs() < 1e-9);
        assert_eq!(recent.seen_to, step_ending(2, 1_800.0).seen_to);
        let sold = service
            .recent_sale_from_account("Iron Stone")
            .await
            .unwrap()
            .expect("within the hour");
        assert_eq!(sold.record_id, sale);
        assert!((sold.quantity - 4.0).abs() < 1e-9);
        assert!((sold.tt_value - 0.52).abs() < 1e-9);
        // A sale is not a conversion, nor the other way round.
        assert_eq!(
            service.recent_sale_from_account(SHRAPNEL).await.unwrap(),
            None
        );
        assert_eq!(
            service
                .recent_conversion_from_account("Iron Stone")
                .await
                .unwrap(),
            None
        );
        // One recorded from a step that ended over an hour ago is not offered.
        let (_dir2, other) = write_service().await;
        seed_hunted(&other, 5, SHRAPNEL, 100_000, 10.0).await;
        other
            .record_from_account(
                step_ending(20, 4_000.0),
                AccountRecord::ShrapnelConversion { quantity: 1.0 },
            )
            .await
            .unwrap();
        assert_eq!(
            other
                .recent_conversion_from_account(SHRAPNEL)
                .await
                .unwrap(),
            None
        );
        // Nor one since undone.
        service.undo_stock_conversion(&id).await.unwrap();
        service.undo_private_sale(&sale).await.unwrap();
        assert_eq!(
            service
                .recent_conversion_from_account(SHRAPNEL)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            service
                .recent_sale_from_account("Iron Stone")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_step_that_is_not_two_readings_in_order_is_refused() {
        let (_dir, service) = write_service().await;
        let mut backwards = step(2);
        backwards.seen_to = backwards.seen_from - 5.0;
        assert!(matches!(
            service
                .record_from_account(
                    backwards,
                    AccountRecord::ShrapnelConversion { quantity: 1.0 }
                )
                .await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        let mut looking_ahead = step(2);
        looking_ahead.hand_entries_since = looking_ahead.seen_from + 1.0;
        assert!(matches!(
            service
                .record_from_account(
                    looking_ahead,
                    AccountRecord::ShrapnelConversion { quantity: 1.0 }
                )
                .await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        assert!(matches!(
            service
                .record_from_account(
                    step(2),
                    AccountRecord::Recycling {
                        inputs: vec![(UNIVERSAL_AMMO.to_string(), 1.0)]
                    }
                )
                .await,
            Err(AnalyticsError::InvalidInput(_))
        ));
    }
}
