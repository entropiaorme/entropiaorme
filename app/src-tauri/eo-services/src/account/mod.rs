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
//! Measured figures stand beside the figures the chat log tracks; they
//! never replace or adjust them. The accounting surfaces may not reference
//! this module (the `measured-isolation` guard), and nothing here reads or
//! writes an accounting table. Role resolution reads the equipment library
//! and the protection sets, never writing them.
//!
//! The connection that takes the readings is [`recorder`]: one task that
//! holds the signed-in session and reads the site through [`site`] while a
//! tracked session needs it. [`sessions`] serves the measures it leads to.
//!
//! Money here is integer throughout: whole PEC as the site reports it, and
//! micro-PED amounts that carry whether they are exact or rest on the
//! site's truncated values (see [`amount`]).

pub mod adoption;
pub mod amount;
pub mod composite;
pub mod diff;
pub mod measure;
pub mod reading;
pub mod recorder;
pub mod roles;
#[cfg(any(test, feature = "test-support"))]
pub mod scripted;
pub mod sessions;
pub mod site;
pub mod store;

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
    AccountRecorder, ConnectEpoch, ConnectOutcome, ConnectSource, ConnectionFlag, ConnectionState,
    ConnectionStatus, RecorderParts,
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
