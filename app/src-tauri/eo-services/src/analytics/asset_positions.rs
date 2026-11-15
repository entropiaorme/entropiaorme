//! Assets held by quantity: positions built up from purchases at a running
//! average cost.
//!
//! An asset row is a position: how many units are held, with the TT and the
//! markup paid for those units as totals. Every purchase that built it is kept,
//! so buying more moves the average and the history of what was paid stays
//! readable. Selling never moves the average: units leave at whatever the
//! position's average cost is at that moment.
//!
//! Part of a position leaves holdings by being carved out as a row of its own
//! carrying its share of the cost, which then goes through the same
//! whole-asset lifecycle as any other asset (listed, sold, traded, converted).
//! Units that come back unsold rejoin the position, and the row that carried
//! them is retired as `merged` so the listing that took them still has a row
//! to point at.
//!
//! Conservation holds per position: what its purchases add up to equals what
//! is held plus what has left and not come back.

use serde::Serialize;
use uuid::Uuid;

use super::{
    conversion, naive_to_epoch, AnalyticsError, AnalyticsService, InventoryRow, STOCK_EPSILON,
};

/// One acquisition of a position: a purchase the player entered, or the output
/// of a conversion.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPurchaseRow {
    pub id: i64,
    pub quantity: f64,
    pub tt_value: f64,
    pub markup_paid: f64,
    pub acquired_at: String,
    /// Made by a conversion rather than bought.
    pub from_conversion: bool,
    /// Whether it can still be changed or removed: not once units it paid for
    /// have been sold, listed, or converted, because a recorded result was
    /// worked out from what it cost.
    pub editable: bool,
}

/// What a purchase records, as the player entered it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PurchaseFigures<'a> {
    pub quantity: f64,
    pub tt_value: f64,
    pub markup_paid: f64,
    /// A bare date; `None` or empty means today.
    pub acquired_at: Option<&'a str>,
}

/// Units taken out of a held position, ready to leave holdings. `id` is the
/// row the caller moves on: the position itself when every unit is taken, or a
/// row carved out of it for part.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TakenUnits {
    pub id: String,
    pub name: String,
    pub quantity: f64,
    pub tt_value: f64,
    pub markup_paid: f64,
}

/// The outcome of asking a position for units.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Take {
    Taken(TakenUnits),
    /// No held asset has that id.
    Missing,
    /// Refused, in terms of the position found.
    Refused(String),
}

const WHOLE_ONLY: &str = "This asset came out of a conversion, so it is sold or converted whole";
const FIXED_PURCHASE: &str = "Units were sold, listed, or converted after this purchase, \
     so what it cost is fixed. Undo that first";

/// The position a row belongs to: the row it was carved from, or itself.
fn position_of(conn: &rusqlite::Connection, item_id: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    conn.query_row(
        "SELECT COALESCE(position_id, id) FROM inventory_items WHERE id = ?",
        rusqlite::params![item_id],
        |row| row.get(0),
    )
    .optional()
}

/// The latest purchase of a position, which a disposal made now draws on.
fn latest_purchase(conn: &rusqlite::Connection, position: &str) -> rusqlite::Result<Option<i64>> {
    // id-order: insertion (a disposal fixes every purchase recorded before it,
    // whatever date each was bought on, so "latest" is the last one written).
    conn.query_row(
        "SELECT MAX(id) FROM asset_purchases WHERE position_id = ?",
        rusqlite::params![position],
        |row| row.get(0),
    )
}

/// Record an acquisition of a position.
pub(super) fn insert_purchase(
    conn: &rusqlite::Connection,
    position: &str,
    figures: (f64, f64, f64),
    acquired_at: &str,
    conversion_id: Option<&str>,
    now: f64,
) -> rusqlite::Result<()> {
    let (quantity, tt_value, markup_paid) = figures;
    conn.execute(
        "INSERT INTO asset_purchases \
         (position_id, quantity, tt_value, markup_paid, acquired_at, conversion_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            position,
            quantity,
            tt_value,
            markup_paid,
            acquired_at,
            conversion_id,
            now
        ],
    )?;
    Ok(())
}

/// Take `quantity` units (every unit when `None`) out of a held position.
///
/// Every unit taken leaves the position whole, exactly as an asset always has.
/// Part is carved out at the position's average cost, which the rest keeps.
/// Either way the disposal is marked with the latest purchase it drew on.
pub(super) fn take_units(
    conn: &rusqlite::Connection,
    item_id: &str,
    quantity: Option<f64>,
    now: f64,
) -> rusqlite::Result<Take> {
    use rusqlite::OptionalExtension as _;
    let held: Option<(String, f64, f64, f64, String, String)> = conn
        .query_row(
            "SELECT name, quantity, tt_value, markup_paid, acquired_at, COALESCE(position_id, id) \
             FROM inventory_items WHERE id = ? AND state = 'held'",
            rusqlite::params![item_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((name, held_quantity, tt_value, markup_paid, acquired_at, position)) = held else {
        return Ok(Take::Missing);
    };
    let wanted = quantity.unwrap_or(held_quantity);
    if !wanted.is_finite() || wanted <= STOCK_EPSILON {
        return Ok(Take::Refused("A quantity must be positive".to_string()));
    }
    if wanted > held_quantity + STOCK_EPSILON {
        return Ok(Take::Refused(format!(
            "Only {} of {name} are held",
            format_quantity(held_quantity)
        )));
    }
    let drawn_through = latest_purchase(conn, &position)?;

    if wanted >= held_quantity - STOCK_EPSILON {
        conn.execute(
            "UPDATE inventory_items SET drawn_through = ? WHERE id = ?",
            rusqlite::params![drawn_through, item_id],
        )?;
        return Ok(Take::Taken(TakenUnits {
            id: item_id.to_string(),
            name,
            quantity: held_quantity,
            tt_value,
            markup_paid,
        }));
    }

    // Part of a position cannot be told apart from the rest by where it came
    // from, so a position that carries activity origins leaves whole.
    if conversion::is_converted_asset(conn, item_id)? {
        return Ok(Take::Refused(WHOLE_ONLY.to_string()));
    }
    let share = wanted / held_quantity;
    let carved_tt = tt_value * share;
    let carved_markup = markup_paid * share;
    let carved_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO inventory_items \
         (id, name, tt_value, markup_paid, acquired_at, quantity, position_id, drawn_through) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            carved_id,
            name,
            carved_tt,
            carved_markup,
            acquired_at,
            wanted,
            position,
            drawn_through
        ],
    )?;
    conn.execute(
        "UPDATE inventory_items \
         SET quantity = quantity - ?, tt_value = tt_value - ?, markup_paid = markup_paid - ?, \
             updated_at = ? \
         WHERE id = ?",
        rusqlite::params![wanted, carved_tt, carved_markup, now, item_id],
    )?;
    Ok(Take::Taken(TakenUnits {
        id: carved_id,
        name,
        quantity: wanted,
        tt_value: carved_tt,
        markup_paid: carved_markup,
    }))
}

/// Bring an asset that left holdings back: into the units of its position
/// still held, or as the held position itself when none are.
///
/// A row already back is left alone, so a correction to something whose units
/// have already returned cannot count them twice.
pub(super) fn restore(
    conn: &rusqlite::Connection,
    item_id: &str,
    now: f64,
) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension as _;
    let row: Option<(String, f64, f64, f64, String)> = conn
        .query_row(
            "SELECT state, quantity, tt_value, markup_paid, COALESCE(position_id, id) \
             FROM inventory_items WHERE id = ?",
            rusqlite::params![item_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((state, quantity, tt_value, markup_paid, position)) = row else {
        return Ok(());
    };
    if state == "held" || state == "merged" {
        return Ok(());
    }
    let held: Option<String> = conn
        .query_row(
            "SELECT id FROM inventory_items \
             WHERE COALESCE(position_id, id) = ? AND state = 'held' AND id <> ?",
            rusqlite::params![position, item_id],
            |row| row.get(0),
        )
        .optional()?;
    match held {
        Some(held_id) => {
            conn.execute(
                "UPDATE inventory_items \
                 SET quantity = quantity + ?, tt_value = tt_value + ?, \
                     markup_paid = markup_paid + ?, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![quantity, tt_value, markup_paid, now, held_id],
            )?;
            conn.execute(
                "UPDATE inventory_items SET state = 'merged', disposed_at = NULL, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![now, item_id],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE inventory_items SET state = 'held', disposed_at = NULL, updated_at = ? \
                 WHERE id = ?",
                rusqlite::params![now, item_id],
            )?;
        }
    }
    Ok(())
}

/// Whether a position has any row besides `item_id`: units carved out of it,
/// or the row it was carved from.
pub(super) fn has_other_rows(conn: &rusqlite::Connection, item_id: &str) -> rusqlite::Result<bool> {
    let Some(position) = position_of(conn, item_id)? else {
        return Ok(false);
    };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM inventory_items \
                       WHERE COALESCE(position_id, id) = ? AND id <> ?)",
        rusqlite::params![position, item_id],
        |row| row.get(0),
    )
}

/// How many purchases a position has.
pub(super) fn purchase_count(conn: &rusqlite::Connection, position: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM asset_purchases WHERE position_id = ?",
        rusqlite::params![position],
        |row| row.get(0),
    )
}

/// Whether a held asset is exactly its one purchase, still open to change:
/// bought once, and no units it paid for are out of holdings. Units that went
/// out and came back changed nothing, so they do not count.
pub(super) fn is_sole_open_purchase(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> rusqlite::Result<bool> {
    let Some(position) = position_of(conn, item_id)? else {
        return Ok(false);
    };
    // id-order: insertion (with a single purchase, MAX(id) is simply its id).
    let only: Option<i64> = conn.query_row(
        "SELECT CASE WHEN COUNT(*) = 1 THEN MAX(id) END FROM asset_purchases \
         WHERE position_id = ?",
        rusqlite::params![position],
        |row| row.get(0),
    )?;
    match only {
        Some(purchase_id) => Ok(!purchase_fixed(conn, &position, purchase_id)?),
        None => Ok(false),
    }
}

/// Whether a purchase is fixed: units it paid for left holdings and have not
/// come back.
fn purchase_fixed(
    conn: &rusqlite::Connection,
    position: &str,
    purchase_id: i64,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM inventory_items \
                       WHERE COALESCE(position_id, id) = ? \
                         AND state NOT IN ('held', 'merged') \
                         AND COALESCE(drawn_through, ?) >= ?)",
        rusqlite::params![position, i64::MAX, purchase_id],
        |row| row.get(0),
    )
}

/// The held row of a position, if any of it is held.
fn held_row(conn: &rusqlite::Connection, position: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    conn.query_row(
        "SELECT id FROM inventory_items WHERE COALESCE(position_id, id) = ? AND state = 'held'",
        rusqlite::params![position],
        |row| row.get(0),
    )
    .optional()
}

/// Attach each held asset's purchases and whether part of it can be taken.
pub(super) fn attach_purchases(
    conn: &rusqlite::Connection,
    rows: &mut [InventoryRow],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, quantity, tt_value, markup_paid, acquired_at, conversion_id IS NOT NULL \
         FROM asset_purchases WHERE position_id = ? \
         ORDER BY acquired_at, id",
    )?;
    for row in rows.iter_mut() {
        let Some(position) = position_of(conn, &row.id)? else {
            continue;
        };
        let purchases = stmt
            .query_map(rusqlite::params![position], |purchase| {
                Ok(AssetPurchaseRow {
                    id: purchase.get(0)?,
                    quantity: purchase.get(1)?,
                    tt_value: purchase.get(2)?,
                    markup_paid: purchase.get(3)?,
                    acquired_at: purchase.get(4)?,
                    from_conversion: purchase.get(5)?,
                    editable: false,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        row.purchases = purchases;
        for purchase in &mut row.purchases {
            purchase.editable = !purchase_fixed(conn, &position, purchase.id)?;
        }
        row.divisible = !conversion::is_converted_asset(conn, &row.id)?;
    }
    Ok(())
}

/// A quantity as a person would write it: whole numbers without decimals.
fn format_quantity(quantity: f64) -> String {
    if (quantity - quantity.round()).abs() < STOCK_EPSILON {
        format!("{}", quantity.round() as i64)
    } else {
        format!("{quantity:.2}")
    }
}

fn check_figures(figures: &PurchaseFigures<'_>) -> Result<(), AnalyticsError> {
    check_amounts(
        Some(figures.quantity),
        Some(figures.tt_value),
        Some(figures.markup_paid),
    )
}

/// The figures every asset and every purchase of one must carry, each checked
/// only when given: units to hold, a TT that may be zero (an item valued by
/// its markup alone, such as a strongbox) but never negative, and a markup
/// that is a number.
pub(super) fn check_amounts(
    quantity: Option<f64>,
    tt_value: Option<f64>,
    markup_paid: Option<f64>,
) -> Result<(), AnalyticsError> {
    if quantity.is_some_and(|value| !value.is_finite() || value <= STOCK_EPSILON) {
        return Err(AnalyticsError::InvalidInput("a quantity must be positive"));
    }
    if tt_value.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(AnalyticsError::InvalidInput(
            "a TT value cannot be negative",
        ));
    }
    if markup_paid.is_some_and(|value| !value.is_finite()) {
        return Err(AnalyticsError::InvalidInput("a markup must be a number"));
    }
    Ok(())
}

/// What a purchase change found, before it is turned into the reply.
enum PurchaseChange {
    Done(String),
    Missing,
    Refused(String),
}

impl AnalyticsService {
    /// Record buying more of a held asset: its quantity and cost grow by what
    /// was paid, which moves the average. Returns the position, or `None` when
    /// no held asset has that id.
    pub async fn add_asset_purchase(
        &self,
        item_id: &str,
        figures: PurchaseFigures<'_>,
    ) -> Result<Option<InventoryRow>, AnalyticsError> {
        check_figures(&figures)?;
        let acquired_at = self.purchase_date(figures.acquired_at);
        let now = naive_to_epoch(self.clock.now());
        let item_id = item_id.to_string();
        let PurchaseFigures {
            quantity,
            tt_value,
            markup_paid,
            ..
        } = figures;
        let change = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let held: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM inventory_items WHERE id = ? AND state = 'held')",
                    rusqlite::params![item_id],
                    |row| row.get(0),
                )?;
                if !held {
                    return Ok(PurchaseChange::Missing);
                }
                if conversion::is_converted_asset(&tx, &item_id)? {
                    return Ok(PurchaseChange::Refused(
                        "This asset came out of a conversion, so more of it cannot be added"
                            .to_string(),
                    ));
                }
                let position = position_of(&tx, &item_id)?.unwrap_or_else(|| item_id.clone());
                insert_purchase(
                    &tx,
                    &position,
                    (quantity, tt_value, markup_paid),
                    &acquired_at,
                    None,
                    now,
                )?;
                tx.execute(
                    "UPDATE inventory_items \
                     SET quantity = quantity + ?, tt_value = tt_value + ?, \
                         markup_paid = markup_paid + ?, updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![quantity, tt_value, markup_paid, now, item_id],
                )?;
                tx.commit()?;
                Ok(PurchaseChange::Done(item_id))
            })
            .await?;
        self.purchase_reply(change).await
    }

    /// Correct a purchase. The held units take the difference, so the average
    /// moves as if it had been entered right. Refused once units it paid for
    /// have left holdings.
    pub async fn update_asset_purchase(
        &self,
        purchase_id: i64,
        figures: PurchaseFigures<'_>,
    ) -> Result<Option<InventoryRow>, AnalyticsError> {
        check_figures(&figures)?;
        let acquired_at = self.purchase_date(figures.acquired_at);
        let now = naive_to_epoch(self.clock.now());
        let PurchaseFigures {
            quantity,
            tt_value,
            markup_paid,
            ..
        } = figures;
        let change = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let found = editable_purchase(&tx, purchase_id)?;
                let (_, held_id, before) = match found {
                    Ok(found) => found,
                    Err(change) => return Ok(change),
                };
                let held_quantity: f64 = tx.query_row(
                    "SELECT quantity FROM inventory_items WHERE id = ?",
                    rusqlite::params![held_id],
                    |row| row.get(0),
                )?;
                if held_quantity + quantity - before.0 <= STOCK_EPSILON {
                    return Ok(PurchaseChange::Refused(
                        "That would leave none of this asset held; remove it instead".to_string(),
                    ));
                }
                tx.execute(
                    "UPDATE asset_purchases \
                     SET quantity = ?, tt_value = ?, markup_paid = ?, acquired_at = ? \
                     WHERE id = ?",
                    rusqlite::params![quantity, tt_value, markup_paid, acquired_at, purchase_id],
                )?;
                tx.execute(
                    "UPDATE inventory_items \
                     SET quantity = quantity + ?, tt_value = tt_value + ?, \
                         markup_paid = markup_paid + ?, updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![
                        quantity - before.0,
                        tt_value - before.1,
                        markup_paid - before.2,
                        now,
                        held_id
                    ],
                )?;
                tx.commit()?;
                Ok(PurchaseChange::Done(held_id))
            })
            .await?;
        self.purchase_reply(change).await
    }

    /// Take back a purchase entered by mistake. The held units lose what it
    /// added. Refused for the only purchase (remove the asset instead) and
    /// once units it paid for have left holdings.
    pub async fn delete_asset_purchase(
        &self,
        purchase_id: i64,
    ) -> Result<Option<InventoryRow>, AnalyticsError> {
        let now = naive_to_epoch(self.clock.now());
        let change = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let found = editable_purchase(&tx, purchase_id)?;
                let (position, held_id, before) = match found {
                    Ok(found) => found,
                    Err(change) => return Ok(change),
                };
                if purchase_count(&tx, &position)? <= 1 {
                    return Ok(PurchaseChange::Refused(
                        "This is the asset's only purchase; remove the asset instead".to_string(),
                    ));
                }
                let held_quantity: f64 = tx.query_row(
                    "SELECT quantity FROM inventory_items WHERE id = ?",
                    rusqlite::params![held_id],
                    |row| row.get(0),
                )?;
                if held_quantity - before.0 <= STOCK_EPSILON {
                    return Ok(PurchaseChange::Refused(
                        "That would leave none of this asset held; remove it instead".to_string(),
                    ));
                }
                tx.execute(
                    "DELETE FROM asset_purchases WHERE id = ?",
                    rusqlite::params![purchase_id],
                )?;
                tx.execute(
                    "UPDATE inventory_items \
                     SET quantity = quantity - ?, tt_value = tt_value - ?, \
                         markup_paid = markup_paid - ?, updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![before.0, before.1, before.2, now, held_id],
                )?;
                tx.commit()?;
                Ok(PurchaseChange::Done(held_id))
            })
            .await?;
        self.purchase_reply(change).await
    }

    fn purchase_date(&self, acquired_at: Option<&str>) -> String {
        acquired_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date())
    }

    async fn purchase_reply(
        &self,
        change: PurchaseChange,
    ) -> Result<Option<InventoryRow>, AnalyticsError> {
        match change {
            PurchaseChange::Done(held_id) => Ok(Some(self.inventory_row(&held_id).await?)),
            PurchaseChange::Missing => Ok(None),
            PurchaseChange::Refused(reason) => Err(AnalyticsError::Rejected(reason)),
        }
    }
}

/// A purchase that may be changed: its position, the position's held row, and
/// what it recorded (quantity, TT, markup). Otherwise the change it comes to.
#[allow(clippy::type_complexity)]
fn editable_purchase(
    conn: &rusqlite::Connection,
    purchase_id: i64,
) -> rusqlite::Result<Result<(String, String, (f64, f64, f64)), PurchaseChange>> {
    use rusqlite::OptionalExtension as _;
    let purchase: Option<(String, f64, f64, f64)> = conn
        .query_row(
            "SELECT position_id, quantity, tt_value, markup_paid \
             FROM asset_purchases WHERE id = ?",
            rusqlite::params![purchase_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((position, quantity, tt_value, markup_paid)) = purchase else {
        return Ok(Err(PurchaseChange::Missing));
    };
    let Some(held_id) = held_row(conn, &position)? else {
        return Ok(Err(PurchaseChange::Refused(
            "None of this asset is held any more".to_string(),
        )));
    };
    if purchase_fixed(conn, &position, purchase_id)? {
        return Ok(Err(PurchaseChange::Refused(FIXED_PURCHASE.to_string())));
    }
    Ok(Ok((position, held_id, (quantity, tt_value, markup_paid))))
}

#[cfg(test)]
mod tests {
    use super::super::tests::write_service;
    use super::super::{ConversionHolding, ConversionInput, ConversionOutput, ItemConversion};
    use super::*;
    use crate::analytics::{InventoryItemPatch, Profession};

    fn close(left: f64, right: f64) -> bool {
        (left - right).abs() < 1e-9
    }

    fn buy(quantity: f64, tt_value: f64, markup_paid: f64) -> PurchaseFigures<'static> {
        PurchaseFigures {
            quantity,
            tt_value,
            markup_paid,
            acquired_at: Some("2026-06-01"),
        }
    }

    /// A position of 100 strongboxes bought at 10 PED markup each.
    async fn strongboxes(service: &AnalyticsService) -> InventoryRow {
        service
            .create_inventory_item(
                "Strongbox",
                Some(100.0),
                10.0,
                1000.0,
                None,
                Some("2026-05-01"),
            )
            .await
            .unwrap()
    }

    async fn held(service: &AnalyticsService, name: &str) -> InventoryRow {
        service
            .list_inventory()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.name == name)
            .expect("the asset is held")
    }

    async fn markup_ledger(service: &AnalyticsService) -> Vec<(String, f64)> {
        service
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT type, amount FROM ledger_entries \
                     WHERE tag = 'inventory_sale' ORDER BY rowid",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap()
    }

    /// What a position's purchases add up to is what is held plus what has
    /// left and not come back, unit for unit and PED for PED.
    async fn assert_conserved(service: &AnalyticsService, position: &str) {
        let position = position.to_string();
        let (bought, accounted): ((f64, f64, f64), (f64, f64, f64)) = service
            .db
            .with_reader(move |conn| {
                let bought = conn.query_row(
                    "SELECT SUM(quantity), SUM(tt_value), SUM(markup_paid) \
                     FROM asset_purchases WHERE position_id = ?",
                    rusqlite::params![position],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                let accounted = conn.query_row(
                    "SELECT SUM(quantity), SUM(tt_value), SUM(markup_paid) \
                     FROM inventory_items \
                     WHERE COALESCE(position_id, id) = ? AND state NOT IN ('merged', 'removed')",
                    rusqlite::params![position],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                Ok((bought, accounted))
            })
            .await
            .unwrap();
        assert!(
            close(bought.0, accounted.0),
            "units: {bought:?} vs {accounted:?}"
        );
        assert!(
            close(bought.1, accounted.1),
            "TT: {bought:?} vs {accounted:?}"
        );
        assert!(
            close(bought.2, accounted.2),
            "markup: {bought:?} vs {accounted:?}"
        );
    }

    #[tokio::test]
    async fn buying_more_moves_the_average() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        assert!(close(position.quantity, 100.0));
        assert_eq!(position.purchases.len(), 1);

        let position = service
            .add_asset_purchase(&position.id, buy(100.0, 10.0, 800.0))
            .await
            .unwrap()
            .expect("the asset is held");

        assert!(close(position.quantity, 200.0));
        assert!(close(position.markup_paid, 1800.0));
        assert!(close(position.markup_paid / position.quantity, 9.0));
        assert!(close(position.tt_value, 20.0));
        assert_eq!(position.purchases.len(), 2);
        assert!(position.purchases.iter().all(|purchase| purchase.editable));
        assert!(position.divisible);
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn selling_part_realises_against_the_average_and_keeps_it() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        service
            .add_asset_purchase(&position.id, buy(100.0, 10.0, 800.0))
            .await
            .unwrap();

        // 50 at 9.50 markup each over 0.10 TT each, against an average of 9.
        let listing = service
            .trade_equipment(&position.id, Some(50.0), 480.0, Some("2026-06-02"))
            .await
            .unwrap()
            .expect("the asset is held");
        assert!(close(listing.quantity, 50.0));
        assert!(close(listing.tt_value, 5.0));

        let ledger = markup_ledger(&service).await;
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].0, "markup");
        assert!(close(ledger[0].1, 25.0));

        let after = held(&service, "Strongbox").await;
        assert_eq!(after.id, position.id, "the position keeps its identity");
        assert!(close(after.quantity, 150.0));
        assert!(close(after.markup_paid / after.quantity, 9.0));
        assert!(close(after.tt_value, 15.0));
        assert!(after.purchases.iter().all(|purchase| !purchase.editable));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn selling_every_unit_moves_the_position_itself() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        service
            .trade_equipment(&position.id, Some(100.0), 1100.0, Some("2026-06-02"))
            .await
            .unwrap()
            .unwrap();
        assert!(service.list_inventory().await.unwrap().is_empty());
        let ledger = markup_ledger(&service).await;
        assert!(close(ledger[0].1, 90.0));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn more_than_is_held_is_refused_and_nothing_moves() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let refused = service
            .trade_equipment(&position.id, Some(101.0), 1000.0, None)
            .await;
        assert!(matches!(refused, Err(AnalyticsError::Rejected(_))));
        let zero = service
            .trade_equipment(&position.id, Some(0.0), 1.0, None)
            .await;
        assert!(matches!(zero, Err(AnalyticsError::Rejected(_))));
        assert!(close(held(&service, "Strongbox").await.quantity, 100.0));
        assert!(markup_ledger(&service).await.is_empty());
    }

    #[tokio::test]
    async fn units_listed_and_unsold_rejoin_their_position_once() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let listing = service
            .create_equipment_listing_with_terms(
                &position.id,
                Some(30.0),
                super::super::ListingTerms {
                    starting_bid: 320.0,
                    buyout: None,
                    listing_fee: 0.5,
                    listed_at: Some("2026-06-02"),
                    auction_days: None,
                    sources: &[],
                },
            )
            .await
            .unwrap()
            .expect("the asset is held");
        assert!(close(listing.quantity, 30.0));
        assert!(close(held(&service, "Strongbox").await.quantity, 70.0));

        service
            .expire_auction_listing(&listing.id, Some("2026-06-09"))
            .await
            .unwrap()
            .expect("the listing was pending");
        let back = held(&service, "Strongbox").await;
        assert_eq!(back.id, position.id);
        assert!(close(back.quantity, 100.0));
        assert!(close(back.markup_paid, 1000.0));
        assert!(
            back.purchases[0].editable,
            "units that came back fix nothing about what they cost"
        );

        // Undoing the expired listing cannot bring the same units back twice.
        assert!(service.undo_auction_listing(&listing.id).await.unwrap());
        assert!(close(held(&service, "Strongbox").await.quantity, 100.0));
        assert_eq!(service.list_inventory().await.unwrap().len(), 1);
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn units_that_come_back_after_the_rest_left_are_the_position() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let listing = service
            .create_equipment_listing(&position.id, 0.0, None, 0.0, Some("2026-06-02"), None)
            .await
            .unwrap()
            .unwrap();
        // Listed whole: nothing is left in Assets.
        assert!(service.list_inventory().await.unwrap().is_empty());
        service
            .expire_auction_listing(&listing.id, Some("2026-06-09"))
            .await
            .unwrap()
            .unwrap();
        assert!(close(held(&service, "Strongbox").await.quantity, 100.0));

        let partial = service
            .create_equipment_listing_with_terms(
                &position.id,
                Some(40.0),
                super::super::ListingTerms {
                    starting_bid: 400.0,
                    buyout: None,
                    listing_fee: 0.0,
                    listed_at: Some("2026-06-10"),
                    auction_days: None,
                    sources: &[],
                },
            )
            .await
            .unwrap()
            .unwrap();
        service
            .trade_equipment(&position.id, None, 700.0, Some("2026-06-11"))
            .await
            .unwrap()
            .unwrap();
        assert!(service.list_inventory().await.unwrap().is_empty());

        service
            .expire_auction_listing(&partial.id, Some("2026-06-17"))
            .await
            .unwrap()
            .unwrap();
        let back = held(&service, "Strongbox").await;
        assert!(close(back.quantity, 40.0));
        assert!(close(back.markup_paid, 400.0));
        assert_eq!(back.purchases.len(), 1, "the history is the position's");
        assert!(!back.purchases[0].editable, "the rest's sale drew on it");
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn opening_part_of_a_position_converts_that_part_and_undoes_cleanly() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        service
            .convert_items(
                Profession::Inventory,
                ItemConversion {
                    inputs: vec![ConversionInput::Asset {
                        item_id: position.id.clone(),
                        quantity: Some(40.0),
                        worth: None,
                    }],
                    outputs: vec![ConversionOutput {
                        item_name: "Weapon Cells".to_string(),
                        quantity: 12.0,
                        tt_value: 3.0,
                        holding: ConversionHolding::Loot,
                    }],
                    ped_amount: None,
                    converted_at: Some("2026-06-03".to_string()),
                },
            )
            .await
            .unwrap();
        let after = held(&service, "Strongbox").await;
        assert!(close(after.quantity, 60.0));
        assert!(close(after.markup_paid, 600.0));

        let (conversion_id, line_quantity): (String, f64) = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT conversion_id, quantity FROM stock_conversion_lines \
                     WHERE side = 'input'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert!(close(line_quantity, 40.0));
        assert!(service.undo_stock_conversion(&conversion_id).await.unwrap());
        let back = held(&service, "Strongbox").await;
        assert!(close(back.quantity, 100.0));
        assert!(close(back.markup_paid, 1000.0));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn an_asset_a_conversion_made_holds_its_units_whole() {
        let (_dir, service) = write_service().await;
        service
            .convert_items(
                Profession::Inventory,
                ItemConversion {
                    inputs: vec![ConversionInput::Bought {
                        item_name: "Strongbox".to_string(),
                        quantity: 10.0,
                        tt_value: 1.0,
                        markup_paid: 100.0,
                    }],
                    outputs: vec![ConversionOutput {
                        item_name: "Gold Ring".to_string(),
                        quantity: 5.0,
                        tt_value: 25.0,
                        holding: ConversionHolding::Asset,
                    }],
                    ped_amount: None,
                    converted_at: Some("2026-06-03".to_string()),
                },
            )
            .await
            .unwrap();
        let ring = held(&service, "Gold Ring").await;
        assert!(close(ring.quantity, 5.0));
        assert_eq!(ring.purchases.len(), 1);
        assert!(ring.purchases[0].from_conversion);
        assert!(!ring.divisible);
        assert!(matches!(
            service
                .add_asset_purchase(&ring.id, buy(5.0, 25.0, 50.0))
                .await,
            Err(AnalyticsError::Rejected(_))
        ));
        assert!(matches!(
            service
                .update_inventory_item(
                    &ring.id,
                    InventoryItemPatch {
                        quantity: Some(4.0),
                        ..Default::default()
                    },
                )
                .await,
            Err(AnalyticsError::Rejected(_))
        ));
        assert_conserved(&service, &ring.id).await;

        // Undone, it leaves no purchase behind.
        let conversion_id: String = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row("SELECT id FROM stock_conversions", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert!(service.undo_stock_conversion(&conversion_id).await.unwrap());
        let purchases: i64 = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM asset_purchases", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(purchases, 0);
    }

    #[tokio::test]
    async fn a_purchase_changes_until_units_it_paid_for_leave() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let position = service
            .add_asset_purchase(&position.id, buy(100.0, 10.0, 900.0))
            .await
            .unwrap()
            .unwrap();
        let second = position.purchases[1].id;

        // A typo corrected: the average follows.
        let corrected = service
            .update_asset_purchase(second, buy(100.0, 10.0, 800.0))
            .await
            .unwrap()
            .unwrap();
        assert!(close(corrected.markup_paid, 1800.0));

        service
            .trade_equipment(&position.id, Some(20.0), 200.0, None)
            .await
            .unwrap()
            .unwrap();
        let fixed = service
            .update_asset_purchase(second, buy(100.0, 10.0, 700.0))
            .await;
        assert!(matches!(fixed, Err(AnalyticsError::Rejected(_))));
        assert!(matches!(
            service.delete_asset_purchase(second).await,
            Err(AnalyticsError::Rejected(_))
        ));

        // A purchase after that sale is still the player's to correct.
        let later = service
            .add_asset_purchase(&position.id, buy(10.0, 1.0, 70.0))
            .await
            .unwrap()
            .unwrap();
        let third = later.purchases.last().unwrap();
        assert!(third.editable);
        let after = service
            .delete_asset_purchase(third.id)
            .await
            .unwrap()
            .unwrap();
        assert!(close(after.quantity, 180.0));
        assert!(close(after.markup_paid, 1800.0 - 20.0 * 9.0));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn the_only_purchase_is_removed_with_the_asset() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let only = position.purchases[0].id;
        assert!(matches!(
            service.delete_asset_purchase(only).await,
            Err(AnalyticsError::Rejected(_))
        ));
        assert!(service.delete_inventory_item(&position.id).await.unwrap());
        let purchases: i64 = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM asset_purchases", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(purchases, 0, "an asset entered by mistake leaves nothing");
        assert!(service.delete_asset_purchase(only).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn removing_a_position_with_history_keeps_that_history() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        service
            .trade_equipment(&position.id, Some(10.0), 120.0, None)
            .await
            .unwrap()
            .unwrap();
        assert!(service.delete_inventory_item(&position.id).await.unwrap());
        assert!(service.list_inventory().await.unwrap().is_empty());
        let state: String = service
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT state FROM inventory_items WHERE id = ?",
                    rusqlite::params![position.id],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(state, "removed");
        assert_eq!(markup_ledger(&service).await.len(), 1);
    }

    #[tokio::test]
    async fn the_edit_form_changes_figures_only_while_one_purchase_is_the_asset() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        let edited = service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    quantity: Some(120.0),
                    markup_paid: Some(1200.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(close(edited.quantity, 120.0));
        assert!(close(edited.purchases[0].quantity, 120.0));
        assert!(close(edited.purchases[0].markup_paid, 1200.0));
        assert!(close(edited.purchases[0].tt_value, 10.0));

        // Units listed and returned unsold change nothing about what it cost.
        let listing = service
            .create_equipment_listing_with_terms(
                &position.id,
                Some(20.0),
                super::super::ListingTerms {
                    starting_bid: 300.0,
                    buyout: None,
                    listing_fee: 0.0,
                    listed_at: Some("2026-06-02"),
                    auction_days: None,
                    sources: &[],
                },
            )
            .await
            .unwrap()
            .unwrap();
        let listed = service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    markup_paid: Some(1100.0),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            matches!(listed, Err(AnalyticsError::Rejected(_))),
            "a pending listing drew on it"
        );
        service
            .expire_auction_listing(&listing.id, Some("2026-06-09"))
            .await
            .unwrap()
            .unwrap();
        service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    markup_paid: Some(1200.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_conserved(&service, &position.id).await;

        service
            .add_asset_purchase(&position.id, buy(10.0, 1.0, 80.0))
            .await
            .unwrap();
        let refused = service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    markup_paid: Some(1.0),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(refused, Err(AnalyticsError::Rejected(_))));
        // The name and notes stay the player's to change.
        let renamed = service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    name: Some("Strongboxes"),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renamed.name, "Strongboxes");
        assert_conserved(&service, &position.id).await;
    }

    /// An item with no TT (a strongbox, a token) is worth its markup alone:
    /// it is held, bought into, and sold by count, and everything it cost is
    /// markup the sale recovers.
    #[tokio::test]
    async fn a_markup_only_stack_is_held_bought_into_and_sold_by_count() {
        let (_dir, service) = write_service().await;
        let position = service
            .create_inventory_item("Strongbox", Some(12.0), 0.0, 24.0, None, Some("2026-05-01"))
            .await
            .unwrap();
        assert!(close(position.tt_value, 0.0));
        assert!(close(position.quantity, 12.0));

        let position = service
            .add_asset_purchase(&position.id, buy(8.0, 0.0, 20.0))
            .await
            .unwrap()
            .expect("the asset is held");
        assert!(close(position.quantity, 20.0));
        assert!(close(position.markup_paid / position.quantity, 2.2));

        // Five sold at 3 PED each, against an average of 2.20 each.
        let sold = service
            .trade_equipment(&position.id, Some(5.0), 15.0, Some("2026-06-02"))
            .await
            .unwrap()
            .expect("the asset is held");
        assert!(close(sold.quantity, 5.0));
        assert!(close(sold.tt_value, 0.0));
        let ledger = markup_ledger(&service).await;
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].0, "markup");
        assert!(close(ledger[0].1, 4.0));

        let after = held(&service, "Strongbox").await;
        assert!(close(after.quantity, 15.0));
        assert!(close(after.tt_value, 0.0));
        assert!(close(after.markup_paid / after.quantity, 2.2));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn an_asset_with_impossible_figures_is_refused_on_create_and_edit() {
        let (_dir, service) = write_service().await;
        for (tt_value, markup_paid) in [(-1.0, 1.0), (f64::NAN, 1.0), (1.0, f64::INFINITY)] {
            assert!(matches!(
                service
                    .create_inventory_item(
                        "Strongbox",
                        Some(1.0),
                        tt_value,
                        markup_paid,
                        None,
                        None
                    )
                    .await,
                Err(AnalyticsError::InvalidInput(_))
            ));
        }
        let position = strongboxes(&service).await;
        assert!(matches!(
            service
                .update_inventory_item(
                    &position.id,
                    InventoryItemPatch {
                        tt_value: Some(-1.0),
                        ..InventoryItemPatch::default()
                    },
                )
                .await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        // TT may fall to nothing: the item is then worth its markup alone.
        let edited = service
            .update_inventory_item(
                &position.id,
                InventoryItemPatch {
                    tt_value: Some(0.0),
                    ..InventoryItemPatch::default()
                },
            )
            .await
            .unwrap()
            .expect("the asset is held");
        assert!(close(edited.tt_value, 0.0));
        assert_conserved(&service, &position.id).await;
    }

    #[tokio::test]
    async fn a_purchase_with_impossible_figures_is_refused() {
        let (_dir, service) = write_service().await;
        let position = strongboxes(&service).await;
        for figures in [
            buy(0.0, 1.0, 1.0),
            buy(1.0, -1.0, 1.0),
            buy(f64::NAN, 1.0, 1.0),
        ] {
            assert!(matches!(
                service.add_asset_purchase(&position.id, figures).await,
                Err(AnalyticsError::InvalidInput(_))
            ));
        }
        assert!(service
            .add_asset_purchase("missing", buy(1.0, 1.0, 1.0))
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            service
                .create_inventory_item("Strongbox", Some(0.0), 1.0, 1.0, None, None)
                .await,
            Err(AnalyticsError::InvalidInput(_))
        ));
    }
}
