//! Account readings: the player's own inventory and PED Card, read from
//! the Entropia Universe account site, and what they measure.
//!
//! A reading is the site's full item list and the PED Card balance at one
//! instant. Consecutive readings differ by steps (an item wearing, a stack
//! burned or gained, an item moved, the Card moving), and steps form
//! patterns (a repair, a strongbox opening). Over a tracked session's
//! readings they add up to the session's measured account: ammo burned,
//! wear per item, repairs, other consumption, and what arrived.
//!
//! Where the readings measure a session's cost bucket, the measured amount
//! is the session's cost; the tracked figures share it out below the
//! session and stand in where nothing was measured. [`costs`] reads a
//! sealed session's measure as what the session cost, per bucket and per
//! item, and stores it in plain rows of its own beside the seal. The
//! accounting surfaces read those rows through one resolver
//! ([`crate::session_costs`]) and nothing else of this module: they may
//! not reference it, its readings or its measures (the
//! `measured-isolation` guard). Nothing here writes an accounting table
//! itself: once a session's costs are stored, the resolver brings the
//! figures derived from them up to date in the same write, and what a step
//! did to the stock ([`recognition`]: a Shrapnel conversion, a recycling, a
//! sale to the Trade Terminal) is recorded through the analytics service's
//! own commands ([`follow`]), as the player's entry would be. Role
//! resolution reads the equipment library, the protection sets and the
//! player's own placements of items the app cannot place ([`placement`],
//! which this module keeps);
//! [`proposals`] reads the strongbox openings, the auction listings and the
//! assets, to match what the readings saw to a listing and to tell whether
//! a proposal's record still stands. None is ever written here. What the
//! recogniser cannot place is left to a reconciliation ([`reconcile`]),
//! which hands the analytics service what the latest reading, or an
//! inventory pasted from the account site ([`paste`]), shows of the stock.
//!
//! The connection that takes the readings is [`recorder`]: one task that
//! holds the signed-in session and reads the site through [`site`] while a
//! tracked session or a strongbox run needs it. [`sessions`] serves the
//! measures it leads to; [`proposals`] what its steps propose (a strongbox
//! opening, a listing sold or returned, a purchase at auction) for the
//! player to record.
//!
//! Money here is integer throughout: whole PEC as the site reports it, and
//! micro-PED amounts that carry whether they are exact or rest on the
//! site's truncated values (see [`amount`]).

pub mod activity;
pub mod adoption;
pub mod amount;
pub mod armour;
pub mod between;
pub mod composite;
pub mod costs;
pub mod diff;
pub mod follow;
pub mod gap;
#[cfg(feature = "session-keyring")]
pub mod keyring_store;
pub mod live;
pub mod measure;
pub mod minutes;
pub mod paste;
pub mod placement;
pub mod proposals;
pub mod reading;
pub mod recognition;
pub mod reconcile;
pub mod recorder;
pub mod roles;
#[cfg(any(test, feature = "test-support"))]
pub mod scripted;
pub mod sessions;
pub mod settlement;
pub mod site;
pub mod store;

#[cfg(test)]
mod between_tests;
#[cfg(test)]
mod costs_tests;
#[cfg(test)]
mod follow_tests;
#[cfg(test)]
mod gap_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod minutes_tests;
#[cfg(test)]
mod proposals_tests;
#[cfg(test)]
mod recognition_tests;
#[cfg(test)]
mod reconcile_tests;
#[cfg(test)]
mod recorder_tests;
#[cfg(test)]
mod replay_tests;
#[cfg(test)]
mod sessions_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(crate) mod test_support;

pub use adoption::{confirm_stored_session, MemoryOnly, SessionStore};
pub use amount::{Amount, Pec, Precision};
pub use measure::{measure_session, SessionMeasure, SessionWindow};
pub use reading::{Container, Holding, HoldingKey, Reading, Row};
pub use recorder::{
    AccountRecorder, Confirming, ConnectEpoch, ConnectOutcome, ConnectSource, ConnectionFlag,
    ConnectionState, ConnectionStatus, ReadingMode, RecorderParts,
};
pub use roles::{KnownRoles, Role, RoleBook, RoleSource};
pub use site::{AccountSite, HttpAccountSite, SessionSecret, SiteFailure, SiteOutcome};
pub use store::{Purpose, ReadingId};

use crate::db::DbError;

/// Everything that can refuse a reading or fail the store.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// The site answered with a status other than success.
    #[error("the account site's {} answered with status {code}", response.describe())]
    SiteStatus {
        response: reading::SiteResponse,
        code: i64,
    },
    /// A response that is not the shape the site sends: a missing field, a
    /// field of the wrong type, or not JSON at all.
    #[error("the account site's {} is not in the expected shape: {detail}", response.describe())]
    SiteShape {
        response: reading::SiteResponse,
        detail: String,
    },
    /// One row of the item list refuses the whole reading.
    #[error("row {row} of the account site's item list {}", reason.describe())]
    RowRefused {
        row: i64,
        reason: reading::RowRefusal,
    },
    /// The PED Card balance refuses the whole reading.
    #[error("the account site's PED Card balance {}", .0.describe())]
    BalanceRefused(amount::ValueRefusal),
    /// The time a reading was taken is not a finite instant.
    #[error("a reading's time is not a finite instant")]
    TakenAtNotFinite,
    /// A figure beyond the range of the integers it is held in: a reading
    /// whose totals cannot be added up, or a step whose arithmetic would
    /// overflow. Never a wrapped figure.
    #[error("a figure is beyond the range the app counts in")]
    Overflow,
    /// A stored row that does not decode into its shape.
    #[error("a stored account reading does not decode: {0}")]
    Stored(&'static str),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl AccountError {
    /// A response not in the site's shape, named by where it first
    /// departs from it: a field path, never a value from the response, so
    /// the error can be logged without quoting what the site sent.
    fn shape(response: reading::SiteResponse, at: String) -> AccountError {
        AccountError::SiteShape {
            response,
            detail: at,
        }
    }
}

impl From<rusqlite::Error> for AccountError {
    fn from(error: rusqlite::Error) -> AccountError {
        AccountError::Db(DbError::from(error))
    }
}
