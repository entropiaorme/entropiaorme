//! The player's own placement of an item: the cost category its use costs
//! as in every session, made once for an item the app cannot place.
//!
//! An item nothing the app knows names, and no structure of the readings
//! places, costs as unclassified overhead when it wears (see
//! [`super::roles`]); a stack of one whose units leave is listed and
//! charged nothing, since a stack leaving may be a trade. The
//! player places it once, by its exact name: in a Cycled category (a weapon,
//! a healing tool, a consumable, unlimited armour), as travel or field
//! logistics, or as no cost at all. The placement answers before the
//! equipment library and the game data, so it also corrects an item those
//! place wrongly. It is one of the inputs the roles' digest covers: placing
//! or clearing an item leaves every stored cost projection stale, and the
//! sweep that rebuilds them (see [`super::costs::heal_session_costs`])
//! re-places every measured session's items and brings the figures derived
//! from them up to date. Re-categorising measured facts this way is a
//! correction of where a measured amount belongs, never an estimate.

use std::collections::{BTreeMap, BTreeSet};

use super::amount::Amount;
use super::costs::{amount_of, CostBucket, LineReason, COSTS_VERSION};
use super::roles::CostCategory;
use super::AccountError;

/// A placement the player may make: an item's exact name, trimmed and not
/// empty, and a category an item may be placed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    item: String,
    category: CostCategory,
}

impl Placement {
    /// The placement of `item` in `category`; `None` when the name is
    /// blank or the category is not one an item may be placed in (see
    /// [`CostCategory::PLACEABLE`]).
    pub fn new(item: &str, category: CostCategory) -> Option<Placement> {
        let item = item.trim();
        (!item.is_empty() && category.is_placeable()).then(|| Placement {
            item: item.to_string(),
            category,
        })
    }

    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn category(&self) -> CostCategory {
        self.category
    }
}

/// A placement as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub item: String,
    pub category: CostCategory,
    pub placed_at: f64,
}

/// An item the app could not place, as the measured sessions saw it: an
/// item that wore (billed as unclassified overhead until it is placed), or
/// a stack whose units left (billed nothing until it is placed as used up).
#[derive(Debug, Clone, PartialEq)]
pub struct Awaiting {
    pub item: String,
    /// How many measured sessions it wore in or left in.
    pub sessions: usize,
    /// The TT it lost wearing across them; `None` when it never wore.
    pub lost: Option<Amount>,
    /// The units of it that left across them; `None` when none did.
    pub units: Option<i64>,
}

/// The items awaiting a placement, and the placements made.
#[derive(Debug, Clone, PartialEq)]
pub struct Placements {
    /// Most TT lost first, then by name.
    pub awaiting: Vec<Awaiting>,
    /// The latest placed first, then by name.
    pub placed: Vec<Placed>,
}

/// Every stored placement, the latest placed first. A row whose category
/// this build does not know places nothing, and is logged.
pub fn read_all(conn: &rusqlite::Connection) -> Result<Vec<Placed>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT item_name, category, placed_at FROM cost_item_placements \
         ORDER BY placed_at DESC, item_name",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut placed = Vec::with_capacity(rows.len());
    for (item, category, placed_at) in rows {
        match CostCategory::parse(&category).filter(|category| category.is_placeable()) {
            Some(category) => placed.push(Placed {
                item,
                category,
                placed_at,
            }),
            None => tracing::warn!(
                target: "eo::account",
                category,
                "a stored cost placement names a category this build does not know; \
                 it places nothing"
            ),
        }
    }
    Ok(placed)
}

/// The items the measured sessions' current costs list as unclassified
/// and the player has not placed: for each, how many sessions it wore or
/// left in, the TT it lost wearing and the units that left.
pub fn awaiting(conn: &rusqlite::Connection) -> Result<Vec<Awaiting>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT l.item_name, l.session_id, l.amount_micro_ped, l.precision, l.units, \
                l.reason \
         FROM account_session_cost_lines l \
         JOIN account_session_costs c ON c.session_id = l.session_id \
         WHERE l.bucket = ?1 AND c.costs_version = ?2 \
           AND c.status IN ('measured', 'acrossGap') \
           AND l.item_name NOT IN (SELECT item_name FROM cost_item_placements) \
         ORDER BY l.item_name, l.session_id, l.position",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![CostBucket::Unclassified.as_str(), COSTS_VERSION],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    type Seen = (BTreeSet<String>, Option<Amount>, Option<i64>);
    let mut items: BTreeMap<String, Seen> = BTreeMap::new();
    for (item, session_id, amount, precision, units, reason) in rows {
        let entry = items
            .entry(item)
            .or_insert_with(|| (BTreeSet::new(), None, None));
        entry.0.insert(session_id);
        match (amount, precision.as_deref(), units, reason.as_deref()) {
            // Worn: its TT lost.
            (Some(amount), Some(precision), _, None) => {
                let amount = amount_of(amount, precision)
                    .ok_or(AccountError::Stored("a stored cost line's precision"))?;
                entry.1 = Some(entry.1.unwrap_or(Amount::NONE).plus(amount)?);
            }
            // Units that left, charged nothing until it is placed.
            (None, None, Some(units), Some(reason)) if reason == LineReason::NotPlaced.as_str() => {
                let total = entry.2.unwrap_or(0).checked_add(units);
                entry.2 = Some(total.ok_or(AccountError::Overflow)?);
            }
            _ => return Err(AccountError::Stored("an unclassified cost line")),
        }
    }
    let mut awaiting: Vec<Awaiting> = items
        .into_iter()
        .map(|(item, (sessions, lost, units))| Awaiting {
            item,
            sessions: sessions.len(),
            lost,
            units,
        })
        .collect();
    // What wore costs the player now, so it leads, most TT first.
    let lost = |item: &Awaiting| item.lost.map_or(-1, Amount::micro_ped);
    awaiting.sort_by(|a, b| lost(b).cmp(&lost(a)).then_with(|| a.item.cmp(&b.item)));
    Ok(awaiting)
}

/// The items awaiting a placement, and the placements made.
pub fn placements(conn: &rusqlite::Connection) -> Result<Placements, AccountError> {
    Ok(Placements {
        awaiting: awaiting(conn)?,
        placed: read_all(conn)?,
    })
}

/// Place an item, replacing any placement it had.
pub fn place(
    conn: &rusqlite::Connection,
    placement: &Placement,
    now: f64,
) -> Result<(), AccountError> {
    conn.execute(
        "INSERT INTO cost_item_placements (item_name, category, placed_at) \
         VALUES (?1, ?2, ?3) \
         ON CONFLICT(item_name) DO UPDATE SET category = excluded.category, \
                                              placed_at = excluded.placed_at",
        rusqlite::params![placement.item(), placement.category().as_str(), now],
    )?;
    Ok(())
}

/// Clear an item's placement, so the library, the game data and the
/// readings place it again. `false` when it had none.
pub fn clear(conn: &rusqlite::Connection, item: &str) -> Result<bool, AccountError> {
    Ok(conn.execute(
        "DELETE FROM cost_item_placements WHERE item_name = ?1",
        [item.trim()],
    )? > 0)
}
