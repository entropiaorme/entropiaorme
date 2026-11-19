//! The inventory follows the account: every stored reading's step is read
//! once by the one recogniser ([`super::recognition`]), and what it closes
//! exactly is recorded on its own through the analytics service, as the
//! player's own entry would be.
//!
//! - A listing that came back from the auction is recorded expired, and
//!   one that sold is recorded sold when only one price fits it (see
//!   [`super::proposals::settled_in_step`]), dated by the local day its step
//!   ended and kept with the step (see
//!   [`crate::analytics::AnalyticsService::record_listing_from_account_blocking`]).
//!   From a step longer than [`LONGEST_STEP`] only a listing that came back,
//!   and the sales of a step that holds nothing else, each at the one price
//!   its listing can fetch: what else a longer step holds is proposed
//!   instead.
//! - A listing the player made (one item or stack into the auction, the
//!   PED Card falling by its fee and by nothing else) is created as the
//!   player's own entry would be, from a short step: its fee exact, its
//!   starting bid inferred from the fee and kept as a range (see
//!   [`super::listing_made`] and
//!   [`crate::analytics::AnalyticsService::record_listed_from_account_blocking`]).
//!   Only when the app holds all of what was listed as the step is looked
//!   at: it never waits on stock still landing, and one held in part is
//!   left for the player. Its buyout is not known, so its sale is never
//!   settled on its own.
//! - Shrapnel that left as Universal Ammo arrived, short of or past what it
//!   pays (converted in several goes, or part of the payout fired), is
//!   recorded as a conversion of what left, from a short step outside
//!   every tracked session.
//! - What a tracked session turned from Shrapnel into ammo is recorded
//!   once, as its last reading's step is looked at, from the session whole
//!   (see [`super::session_shrapnel`]).
//! - A Shrapnel conversion is recorded as a Shrapnel conversion, a recycling
//!   as one conversion into Nanocubes, and a sale to the Trade Terminal as a
//!   sale at TT, item by item. Each takes only what the app holds, keeps out
//!   of what the player recorded by hand since [`HAND_ENTRY_LOOK_BACK`]
//!   before the step began, and carries the step it came from (see
//!   [`crate::analytics::AnalyticsService::record_from_account`]).
//! - **Short steps, or whole ones, and never history.** A step longer than
//!   [`LONGEST_STEP`] may hold anything (a session the app did not track,
//!   trades, crafting) and is left to the reconciliation, unless every
//!   change in it closes exactly one way with nothing left over
//!   ([`Recognised::whole`]): untracked play shows as change no pattern
//!   closes, so such a step is safe at any length (a restart that kept the
//!   readings away while the player recycled, say). A step from a reading
//!   stored before the app recorded from the account at all
//!   (`account_recognition_boundary`), which may reach back days, is never
//!   looked at.
//! - **Stock still landing is waited for.** While a step is younger than
//!   [`STOCK_SETTLES`], a part the app's stock cannot yet cover in full is
//!   not recorded: the pass stops there with the step unmarked and tries
//!   it again after a later reading. After that the held part is recorded.
//! - A step is looked at once: the readings whose step has been looked at
//!   are marked, oldest first, and a part of a step already recorded (even
//!   one since undone) is never recorded again, so a step looked at twice
//!   records nothing twice. The marks are always the oldest readings, so the
//!   next step is the first reading after the latest mark.
//! - What a strongbox run read is the run's own flow's: the steps between
//!   its readings are marked and nothing in them is recorded. The step that
//!   ends at a run's first reading is not the run's (that reading is taken
//!   before any box is opened) and is read like any other. The connection
//!   never runs a pass while a run is open.
//! - A step between sessions that wore or used something up is kept with
//!   its measure, for what it bills as overhead on its day (see
//!   [`super::between`]).
//! - A step whose own content cannot be read is marked and passed. A
//!   database failure stops the pass with the step unmarked, so the next
//!   pass, after the next reading, tries it again; the connection passes a
//!   step over once it has failed several passes in a row
//!   ([`follow`]'s `pass_over`), so one bad step never holds the rest.
//!
//! Computed on a reader; each record is its own short write through the
//! analytics service; marking a step is a write of its own.

use std::sync::Arc;

use super::diff::Span;
use super::proposals::{
    is_the_step_s_own, local_day, outside_every_session, settled_in_step, step_between,
    ListingSettled,
};
use super::recognition::{recognise_step, Recognised};
use super::recorder::IDLE_READ_EVERY;
use super::roles::{KnownRoles, RoleSource};
use super::store::{self, Purpose, ReadingId, StoredReading};
use super::AccountError;
use crate::analytics::{
    AccountRecord, AccountStep, AnalyticsError, AnalyticsService, FromAccount, ListedFromAccount,
    ListingFromAccount,
};
use crate::db::Db;
use crate::game_data_store::GameDataStore;

/// The most steps one pass looks at; the rest wait for the next.
pub const STEPS_PER_PASS: usize = 20;

/// The longest step anything is recorded from, in seconds: the idle
/// reading's five minutes, and a minute and a half for a reading held back
/// by the floor or a slow site. A longer step spans time no reading saw.
pub const LONGEST_STEP: f64 = 6.5 * 60.0;

/// How long before a step began an outflow the player recorded by hand
/// still counts as the step's own, in seconds: one idle reading's interval,
/// since the site may show a change a reading after the player entered it.
pub const HAND_ENTRY_LOOK_BACK: f64 = IDLE_READ_EVERY.as_secs_f64();

/// How long after a step ended stock the app cannot yet cover in full is
/// waited for, in seconds: loot the chat log shows can be recorded a little
/// after the reading that shows it gone.
pub const STOCK_SETTLES: f64 = 10.0 * 60.0;

/// One step, read: what of it to record.
#[derive(Debug, Clone, PartialEq)]
pub struct Following {
    /// The later reading of the step.
    pub reading: ReadingId,
    /// The step, when anything in it is to be recorded.
    pub step: Option<AccountStep>,
    pub records: Vec<AccountRecord>,
    /// The listings whose outcome the step settles.
    pub listings: Vec<ListingFromAccount>,
    /// The listing the step shows the player made, to create.
    pub listed: Option<ListedFromAccount>,
    /// The local day the step ended: what a listing's outcome is dated.
    pub resolved_on: String,
    /// The step measured, when it lies between sessions and wore or used
    /// something up (see [`super::between`]).
    pub between: Option<super::between::BetweenStep>,
    /// The Shrapnel the session this reading closed turned into ammo, with
    /// the step that spans the session (see [`super::session_shrapnel`]).
    pub session_shrapnel: Option<super::session_shrapnel::SessionShrapnel>,
}

impl Following {
    /// Nothing in this reading's step is recorded.
    pub fn nothing(reading: ReadingId) -> Following {
        Following {
            reading,
            step: None,
            records: Vec::new(),
            listings: Vec::new(),
            listed: None,
            resolved_on: String::new(),
            between: None,
            session_shrapnel: None,
        }
    }
}

/// What one pass came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FollowPass {
    /// Steps looked at and marked.
    pub recognised: usize,
    /// Records made.
    pub recorded: usize,
    /// Steps may still be waiting.
    pub more: bool,
    /// The pass stopped at a step whose stock may still be landing.
    pub waiting: bool,
}

/// A pass that failed, and the step it failed on when it had one.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct FollowFailure {
    pub reading: Option<ReadingId>,
    pub error: AccountError,
}

impl FollowFailure {
    fn at(reading: Option<ReadingId>) -> impl FnOnce(AccountError) -> FollowFailure {
        move |error| FollowFailure { reading, error }
    }
}

/// The oldest stored reading whose step has not been looked at: the first
/// after the latest marked, since steps are marked oldest first.
pub fn next_unrecognised(conn: &rusqlite::Connection) -> Result<Option<ReadingId>, AccountError> {
    use rusqlite::OptionalExtension as _;
    Ok(conn
        .query_row(
            // id-order: insertion (the chain follows the order readings were stored)
            "SELECT r.id FROM account_readings r \
             WHERE r.id > COALESCE((SELECT MAX(s.reading_id) FROM account_steps_recognised s), 0) \
             ORDER BY r.id LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(ReadingId))
}

/// The latest reading stored before the app recorded from the account.
pub(super) fn history_ends_at(conn: &rusqlite::Connection) -> Result<i64, AccountError> {
    use rusqlite::OptionalExtension as _;
    Ok(conn
        .query_row(
            "SELECT last_reading_before FROM account_recognition_boundary WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// Mark a reading's step looked at. Marking it again changes nothing.
pub fn mark_recognised(
    conn: &rusqlite::Connection,
    reading: ReadingId,
    now: f64,
) -> Result<(), AccountError> {
    conn.execute(
        "INSERT INTO account_steps_recognised (reading_id, recognised_at) VALUES (?1, ?2) \
         ON CONFLICT (reading_id) DO NOTHING",
        rusqlite::params![reading.0, now],
    )?;
    Ok(())
}

/// The records what a step did to the stock asks for.
/// `presume` is whether Shrapnel presumed converted is recorded too.
fn records_of(recognised: &Recognised, presume: bool) -> Vec<AccountRecord> {
    let mut records = Vec::new();
    if let Some(converted) = recognised.shrapnel {
        records.push(AccountRecord::ShrapnelConversion {
            quantity: converted.shrapnel_units as f64,
        });
    }
    if let Some(units) = recognised.shrapnel_presumed.filter(|_| presume) {
        records.push(AccountRecord::ShrapnelPresumedConversion {
            quantity: units as f64,
        });
    }
    if !recognised.recycled.is_empty() {
        records.push(AccountRecord::Recycling {
            inputs: recognised
                .recycled
                .iter()
                .map(|item| (item.name.clone(), item.units as f64))
                .collect(),
        });
    }
    for item in &recognised.sold {
        records.push(AccountRecord::TerminalSale {
            item_name: item.name.clone(),
            quantity: item.units as f64,
        });
    }
    records
}

/// How far the readings settle, on their own, the listing outcomes a step
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListingReach {
    /// None of them: a step reaching back into the readings stored before
    /// the app recorded from the account, one between a strongbox run's own
    /// readings, or one whose readings are out of order.
    Nothing,
    /// A step no longer than [`LONGEST_STEP`]: every outcome one price fits.
    Short,
    /// A longer step: a listing that came back, and the sales of a step that
    /// holds nothing else.
    Long,
}

/// How far the step from `before` to `after` settles its listing outcomes
/// on their own. What it settles is recorded by the pass and never
/// proposed: both read it through [`settled_in_step`].
pub(super) fn listing_reach(
    conn: &rusqlite::Connection,
    before: &StoredReading,
    after: &StoredReading,
) -> Result<ListingReach, AccountError> {
    if before.header.id.0 <= history_ends_at(conn)? {
        return Ok(ListingReach::Nothing);
    }
    if after.header.purpose == Purpose::Run && before.header.purpose == Purpose::Run {
        return Ok(ListingReach::Nothing);
    }
    let length = after.reading.taken_at() - before.reading.taken_at();
    Ok(if (0.0..=LONGEST_STEP).contains(&length) {
        ListingReach::Short
    } else if length > LONGEST_STEP {
        ListingReach::Long
    } else {
        ListingReach::Nothing
    })
}

/// The listings whose outcome a step settles, as records to make.
fn settled_listings(
    conn: &rusqlite::Connection,
    set: &super::diff::StepSet,
    before: &super::reading::Reading,
    between: &Span,
    reach: ListingReach,
) -> Result<Vec<ListingFromAccount>, AccountError> {
    Ok(settled_in_step(conn, set, before, between, reach)?
        .into_iter()
        .map(|settled| match settled.outcome {
            ListingSettled::Returned => ListingFromAccount::Returned {
                listing_id: settled.listing_id,
            },
            ListingSettled::Sold {
                final_price,
                sale_fee,
            } => ListingFromAccount::Sold {
                listing_id: settled.listing_id,
                final_price: final_price.0 as f64 / 100.0,
                sale_fee: sale_fee.0 as f64 / 100.0,
            },
        })
        .collect())
}

/// Read one stored reading's step for what to record: a read, so run it
/// on a reader.
pub fn prepare(
    conn: &rusqlite::Connection,
    reading: ReadingId,
    roles: &dyn RoleSource,
) -> Result<Following, AccountError> {
    let Some(after) = store::load_reading(conn, reading)? else {
        return Ok(Following::nothing(reading));
    };
    let Some(prev) = after.header.prev else {
        return Ok(Following::nothing(reading));
    };
    // A step reaching back into the readings stored before the app
    // recorded from the account is history.
    if prev.0 <= history_ends_at(conn)? {
        return Ok(Following::nothing(reading));
    }
    let before = store::load_reading(conn, prev)?
        .ok_or(AccountError::Stored("a reading's predecessor is missing"))?;
    // A strongbox run's own flow records what happens between its
    // readings; the step up to its first is no part of the run.
    if after.header.purpose == Purpose::Run && before.header.purpose == Purpose::Run {
        return Ok(Following::nothing(reading));
    }
    let between = Span {
        from: before.reading.taken_at(),
        to: after.reading.taken_at(),
    };
    let length = between.to - between.from;
    if length < 0.0 {
        return Ok(Following::nothing(reading));
    }
    let set = step_between(&before.reading, &after.reading, roles)?;
    let recognised = recognise_step(&set, &before.reading, &after.reading)?;
    // A long step may hold play no reading saw, unless everything in it
    // closes exactly; a listing's outcome is read only from a short one.
    let records = if length > LONGEST_STEP && !recognised.whole {
        Vec::new()
    } else {
        // Shrapnel is presumed converted only outside every tracked
        // session: inside one it is looted as it is converted, so what a
        // step shows leaving is not what was converted.
        records_of(&recognised, outside_every_session(conn, &between)?)
    };
    let reach = listing_reach(conn, &before, &after)?;
    let listings = settled_listings(conn, &set, &before.reading, &between, reach)?;
    // A listing is created only from a short step: a longer one may hold
    // play no reading saw that moved the Card.
    let listed = if reach == ListingReach::Short {
        super::listing_made::listing_made(&set, &before.reading).map(|made| ListedFromAccount {
            item_name: made.name,
            quantity: made.units,
            fee_pec: made.fee.0,
            bid_low_pec: made.bids.0 .0,
            bid_high_pec: made.bids.1 .0,
        })
    } else {
        None
    };
    let between_sessions = super::between::measure(&before, &after, roles)?;
    Ok(Following {
        reading,
        between: between_sessions,
        resolved_on: local_day(between.to),
        step: (!records.is_empty() || !listings.is_empty() || listed.is_some()).then_some(
            AccountStep {
                reading_id: reading.0,
                prev_reading_id: prev.0,
                seen_from: between.from,
                seen_to: between.to,
                hand_entries_since: between.from - HAND_ENTRY_LOOK_BACK,
                stock_settles_by: between.to + STOCK_SETTLES,
                card_change_pec: Some(after.header.balance.0 - before.header.balance.0),
            },
        ),
        records,
        listings,
        listed,
        session_shrapnel: None,
    })
}

fn on_reader<T: Send + 'static>(
    db: &Db,
    job: impl FnOnce(&rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
) -> Result<T, AccountError> {
    db.with_reader_blocking(move |conn| Ok(job(conn)))
        .map_err(AccountError::from)
        .and_then(|done| done)
}

/// Look at the steps waiting, oldest first, at most [`STEPS_PER_PASS`],
/// and record what each closes exactly. A refused record is logged and
/// left; a step whose stock may still be landing stops the pass, unmarked;
/// a database failure stops it too, the step unmarked. The step ending at
/// `pass_over` is marked with nothing recorded: one that has failed pass
/// after pass.
///
/// Blocking: run it on a blocking thread, as the connection does with all
/// its database work.
pub fn follow(
    db: &Db,
    analytics: &AnalyticsService,
    game_data: Option<Arc<GameDataStore>>,
    now: f64,
    pass_over: Option<ReadingId>,
) -> Result<FollowPass, FollowFailure> {
    let mut pass = FollowPass::default();
    for _ in 0..STEPS_PER_PASS {
        let game_data_for_write = game_data.clone();
        let game_data = game_data.clone();
        let prepared = on_reader(db, move |conn| {
            let Some(reading) = next_unrecognised(conn)? else {
                return Ok(None);
            };
            if Some(reading) == pass_over {
                tracing::warn!(target: "eo::account", reading = reading.0, "a step failed to be recorded pass after pass; it is passed over and left to the reconciliation");
                return Ok(Some(Ok(Following::nothing(reading))));
            }
            let prepared = KnownRoles::read(conn, game_data.as_deref())
                .and_then(|roles| prepare(conn, reading, &roles));
            // What a session converted is read as its last reading's step
            // is: a reading of its own, never the step's to fail on.
            let session_shrapnel =
                match super::session_shrapnel::at_close(conn, reading, game_data.as_deref(), now) {
                    Ok(found) => found,
                    Err(error) => {
                        tracing::warn!(target: "eo::account", %error, reading = reading.0, "the Shrapnel a session converted could not be read; it is left to the stock check");
                        None
                    }
                };
            Ok(Some(match prepared {
                Ok(mut following) => {
                    following.session_shrapnel = session_shrapnel;
                    Ok(following)
                }
                Err(error) if is_the_step_s_own(&error) => {
                    tracing::warn!(target: "eo::account", %error, reading = reading.0, "a stored reading's step could not be read for the stock; it is passed");
                    Ok(Following::nothing(reading))
                }
                Err(error) => Err(FollowFailure {
                    reading: Some(reading),
                    error,
                }),
            }))
        })
        .map_err(FollowFailure::at(None))?;
        let following = match prepared {
            None => return Ok(pass),
            Some(prepared) => prepared?,
        };
        let reading = following.reading;
        if let Some(step) = following.step {
            // A listing's outcome first: it takes nothing from stock, so it
            // never waits on stock still landing.
            for listing in following.listings {
                match analytics.record_listing_from_account_blocking(
                    step,
                    listing,
                    following.resolved_on.clone(),
                ) {
                    Ok(FromAccount::Recorded { .. }) => pass.recorded += 1,
                    Ok(_) => {}
                    Err(AnalyticsError::Storage(error)) => {
                        return Err(FollowFailure {
                            reading: Some(reading),
                            error: AccountError::Db(error),
                        })
                    }
                    Err(error) => {
                        tracing::warn!(target: "eo::account", %error, reading = reading.0, "a listing outcome the readings showed was refused; it is left");
                    }
                }
            }
            // The listing the player made: created only of stock held in
            // full as it is looked at, so it never waits; the rest of the
            // step goes on either way.
            if let Some(listed) = following.listed {
                match analytics.record_listed_from_account_blocking(step, listed) {
                    Ok(FromAccount::Recorded { .. }) => pass.recorded += 1,
                    Ok(_) => {}
                    Err(AnalyticsError::Storage(error)) => {
                        return Err(FollowFailure {
                            reading: Some(reading),
                            error: AccountError::Db(error),
                        })
                    }
                    Err(error) => {
                        tracing::warn!(target: "eo::account", %error, reading = reading.0, "a listing the readings saw made was refused; it is left");
                    }
                }
            }
            for record in following.records {
                match analytics.record_from_account_blocking(step, record) {
                    Ok(FromAccount::Recorded { .. }) => pass.recorded += 1,
                    Ok(FromAccount::Waiting) => {
                        pass.waiting = true;
                        return Ok(pass);
                    }
                    Ok(_) => {}
                    Err(AnalyticsError::Storage(error)) => {
                        return Err(FollowFailure {
                            reading: Some(reading),
                            error: AccountError::Db(error),
                        })
                    }
                    Err(error) => {
                        tracing::warn!(target: "eo::account", %error, reading = reading.0, "a stock record the readings showed was refused; it is left");
                    }
                }
            }
        }
        if let Some(session) = following.session_shrapnel {
            let record = AccountRecord::ShrapnelSessionConversion {
                quantity: session.units as f64,
            };
            match analytics.record_from_account_blocking(session.step, record) {
                Ok(FromAccount::Recorded { .. }) => pass.recorded += 1,
                Ok(FromAccount::Waiting) => {
                    pass.waiting = true;
                    return Ok(pass);
                }
                Ok(_) => {}
                Err(AnalyticsError::Storage(error)) => {
                    return Err(FollowFailure {
                        reading: Some(reading),
                        error: AccountError::Db(error),
                    })
                }
                Err(error) => {
                    tracing::warn!(target: "eo::account", %error, reading = reading.0, "the Shrapnel a session converted was refused as a record; it is left");
                }
            }
        }
        // A step between sessions is kept with what it bills, priced by
        // the roles as they stand, and marked looked at in the same write.
        let between = following.between;
        let game_data = game_data_for_write.clone();
        db.with_writer_blocking(move |conn| {
            Ok((|| {
                if let Some(step) = &between {
                    let roles = KnownRoles::read(conn, game_data.as_deref())?;
                    super::between::store(conn, step, &roles, now)?;
                }
                mark_recognised(conn, reading, now)
            })())
        })
        .map_err(AccountError::from)
        .and_then(|marked| marked)
        .map_err(FollowFailure::at(Some(reading)))?;
        pass.recognised += 1;
    }
    pass.more = true;
    Ok(pass)
}
