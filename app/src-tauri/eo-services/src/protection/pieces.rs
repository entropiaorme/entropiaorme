//! A limited set's pieces, named once.
//!
//! The account readings bill a limited set's armour at what its pieces
//! lose times the set's markup, so the app has to know which items in a
//! reading are the set's. It suggests them: the limited armour and plates
//! in the latest account reading that the set's name names (`Pegasus`
//! names `Pegasus Helmet (M,L)`), the longest name winning where two sets'
//! names both would. The player confirms the suggestion once, or edits it
//! from the reading's other limited armour and plates. An item is a piece
//! of at most one set at a time. Each confirmation is kept with when it
//! ended (the piece removed, or its set taken out of use), so the sessions
//! billed under a set keep its markup when a piece moves to another.
//!
//! Confirming pieces changes what every measured session's armour bills;
//! the account side rebuilds those sessions' costs, and every figure
//! derived from them, once the pieces are stored.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;

use super::{
    account_error, map_constraint, ConfirmedPiece, ProtectionError, ProtectionService, SetPieces,
};
use crate::account::reading::Container;
use crate::account::roles::{cost_category, family_names, name_is_limited, CostCategory, Use};
use crate::account::{KnownRoles, RoleBook};
use crate::game_data_store::GameDataStore;

/// The names confirmed as a set's pieces now.
pub(super) fn confirmed_names(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT item_name FROM protection_set_pieces WHERE set_id = ?1 AND removed_at IS NULL",
    )?;
    let names = stmt
        .query_map([set_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(names)
}

/// The pieces every other set holds now. A set taken out of use holds
/// none: its confirmations ended with it.
fn held_by_other_sets(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT item_name FROM protection_set_pieces WHERE set_id != ?1 AND removed_at IS NULL",
    )?;
    let names = stmt
        .query_map([set_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(names)
}

/// A limited set in use, by id: its name.
fn set_in_use(conn: &rusqlite::Connection, set_id: i64) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT name FROM protection_sets \
         WHERE id = ?1 AND archived_at IS NULL AND economy_kind = 'limited'",
        [set_id],
        |row| row.get(0),
    )
    .optional()
}

/// Whether the set named `family` is the one whose name names `item` among
/// the sets in use: no other such set's longer name names it too, and none
/// of the same length does.
fn names_best(family: &str, others: &[String], item: &str) -> bool {
    if !family_names(family, item) {
        return false;
    }
    let length = family.trim().len();
    !others
        .iter()
        .any(|other| family_names(other, item) && other.trim().len() >= length)
}

/// Read a set's pieces and what the latest account reading suggests.
fn read_pieces(
    conn: &rusqlite::Connection,
    set_id: i64,
    game_data: Option<&GameDataStore>,
) -> Result<SetPieces, ProtectionError> {
    let Some(family) = set_in_use(conn, set_id)? else {
        return Err(ProtectionError::NotFound("Protection set not found"));
    };
    let confirmed = {
        let mut stmt = conn.prepare(
            "SELECT item_name, confirmed_at FROM protection_set_pieces \
             WHERE set_id = ?1 AND removed_at IS NULL ORDER BY item_name",
        )?;
        let rows = stmt
            .query_map([set_id], |row| {
                Ok(ConfirmedPiece {
                    name: row.get(0)?,
                    confirmed_at: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let Some(latest) = crate::account::store::latest_reading(conn).map_err(account_error)? else {
        return Ok(SetPieces {
            set_id,
            confirmed,
            suggested: Vec::new(),
            others: Vec::new(),
            reading_taken_at: None,
        });
    };
    let roles = KnownRoles::read(conn, game_data).map_err(account_error)?;
    let reading = latest.reading;
    let book = RoleBook::resolve(&roles, std::slice::from_ref(&reading));
    let mine: BTreeSet<&str> = confirmed.iter().map(|piece| piece.name.as_str()).collect();
    let theirs = held_by_other_sets(conn, set_id)?;
    let other_families: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM protection_sets WHERE archived_at IS NULL AND id != ?1")?;
        let rows = stmt
            .query_map([set_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut suggested = BTreeSet::new();
    let mut others = BTreeSet::new();
    for key in reading.holdings().keys() {
        let name = key.name.as_str();
        if !name_is_limited(name) || mine.contains(name) || theirs.contains(name) {
            continue;
        }
        let parent_role = match &key.container {
            Container::Attached { parent } => Some(book.role(parent)),
            _ => None,
        };
        let category = cost_category(&roles, key, book.role(name), Use::Wear, parent_role);
        if !matches!(
            category,
            CostCategory::LimitedArmour | CostCategory::LimitedPlate
        ) {
            continue;
        }
        if names_best(&family, &other_families, name) {
            suggested.insert(name.to_string());
        } else {
            others.insert(name.to_string());
        }
    }
    Ok(SetPieces {
        set_id,
        confirmed,
        suggested: suggested.into_iter().collect(),
        others: others.into_iter().collect(),
        reading_taken_at: Some(reading.taken_at()),
    })
}

impl ProtectionService {
    /// A limited set's confirmed pieces, and what the latest account
    /// reading suggests for it.
    pub async fn set_pieces(&self, set_id: i64) -> Result<SetPieces, ProtectionError> {
        let game_data = self.game_data.clone();
        self.db
            .with_reader(move |conn| {
                read_pieces(conn, set_id, game_data.as_deref()).map_err(super::protection_decode)
            })
            .await
            .map_err(ProtectionError::from)
    }

    /// Name a limited set's pieces: exactly `names`, each an item some
    /// account reading has held (or already one of the set's pieces), and
    /// none a piece of another set. A piece kept keeps when it was first
    /// confirmed; one dropped stops being the set's piece from now, so the
    /// sessions billed with it before stay billed. Refused whole when any
    /// name is.
    pub async fn confirm_set_pieces(
        &self,
        set_id: i64,
        names: Vec<String>,
    ) -> Result<SetPieces, ProtectionError> {
        self.active_set(set_id).await?;
        let mut chosen = BTreeSet::new();
        for name in names {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(ProtectionError::Invalid("A piece's name is required"));
            }
            chosen.insert(name);
        }
        let now = self.now();
        let refused = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let current = confirmed_names(&tx, set_id)?;
                let theirs = held_by_other_sets(&tx, set_id)?;
                for name in &chosen {
                    if theirs.contains(name) {
                        return Ok(Some(ProtectionError::Conflict(
                            "An item can be a piece of only one set in use",
                        )));
                    }
                    let held = current.contains(name)
                        || crate::account::store::ever_held(&tx, name)
                            .map_err(account_error)
                            .map_err(super::protection_decode)?;
                    if !held {
                        return Ok(Some(ProtectionError::Invalid(
                            "Only an item an account reading has held can be a piece",
                        )));
                    }
                }
                for name in current.difference(&chosen) {
                    tx.execute(
                        "UPDATE protection_set_pieces SET removed_at = ?3 \
                         WHERE set_id = ?1 AND item_name = ?2 AND removed_at IS NULL",
                        rusqlite::params![set_id, name, now],
                    )?;
                }
                for name in chosen.difference(&current) {
                    tx.execute(
                        "INSERT INTO protection_set_pieces (set_id, item_name, confirmed_at) \
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![set_id, name, now],
                    )?;
                }
                tx.commit()?;
                Ok(None)
            })
            .await
            .map_err(map_constraint("The set's pieces could not be stored"))?;
        if let Some(refusal) = refused {
            return Err(refusal);
        }
        self.notify_changed();
        self.set_pieces(set_id).await
    }
}
