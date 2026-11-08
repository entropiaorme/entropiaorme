//! Item conversions: several things go in, something else comes out.
//!
//! A hand-in of vouchers and components for a harness, a hundred strongboxes
//! opened into whatever they held, boards recycled into Nanocubes: each is one
//! recorded fact about what was consumed and what was produced. The activity
//! composition of what was consumed rides forward onto what was produced, so
//! the sale that eventually realises a gain credits the gameplay that earned
//! the inputs without the player tracing anything.
//!
//! Three money rules follow from the accounting doctrine:
//!
//! - TT that appears or disappears is recognised on the day, because TT is
//!   liquid the moment it is held.
//! - Markup paid for bought inputs becomes cost basis on the output and is
//!   recovered at sale, the way an asset's markup paid always has been.
//! - Markup declared on the player's own inputs is a claim on the eventual
//!   proceeds, so unsold markup is never realised.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use uuid::Uuid;

use crate::daily_rollup;
use crate::db::DbError;
use crate::harvest_yield::HarvestYieldTier;
use crate::stock_allocation::{self, CreditLine, DerivedFacts, StockProvenance};

use super::{
    as_source_positions, blocked_reason, insert_movement, is_universal_ammo, item_positions,
    naive_to_epoch, record_opening_balance, reversal_blocker, ActivityHistoryRow, AnalyticsError,
    AnalyticsService, DerivedSourceRow, HoldingOriginRow, InventoryRow, OpenPosition, Profession,
    STOCK_EPSILON,
};

/// One thing that went into a conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversionInput {
    /// Drawn from tracked stock. `worth` is what the player declares this line
    /// to be worth above TT; left out, the line shares whatever remains.
    Held {
        item_name: String,
        quantity: f64,
        worth: Option<f64>,
    },
    /// Acquired for this conversion: never part of holdings and of no
    /// activity's making. `markup_paid` is what it cost above its TT.
    Bought {
        item_name: String,
        quantity: f64,
        tt_value: f64,
        markup_paid: f64,
    },
    /// A whole held asset, consumed with everything it carries.
    Asset { item_id: String, worth: Option<f64> },
}

/// Which side of Inventory a conversion's result joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionHolding {
    Loot,
    Asset,
}

impl ConversionHolding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Loot => "loot",
            Self::Asset => "asset",
        }
    }

    fn from_db(value: &str) -> Self {
        if value == "asset" {
            Self::Asset
        } else {
            Self::Loot
        }
    }
}

/// Which side of a conversion a recorded line sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionLineSide {
    Input,
    Output,
}

/// How an input line was acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionAcquisition {
    Held,
    Bought,
    Asset,
}

impl ConversionAcquisition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Bought => "bought",
            Self::Asset => "asset",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "bought" => Self::Bought,
            "asset" => Self::Asset,
            _ => Self::Held,
        }
    }
}

/// One thing a conversion produced.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversionOutput {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    pub holding: ConversionHolding,
}

/// A complete conversion as the player recorded it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemConversion {
    pub inputs: Vec<ConversionInput>,
    pub outputs: Vec<ConversionOutput>,
    /// PED received outright, for an outcome the player has already cashed.
    pub ped_amount: Option<f64>,
    pub converted_at: Option<String>,
}

/// One recorded line of a conversion.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionLineRow {
    pub side: ConversionLineSide,
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    pub acquisition: Option<ConversionAcquisition>,
    pub markup: Option<f64>,
    pub holding: Option<ConversionHolding>,
}

/// How a conversion presents itself in the books.
pub(super) struct ConversionTerms {
    /// An itemised conversion names what its output descends from and keeps
    /// its own terms apart. Recycling does neither: Nanocubes are Nanocubes.
    pub(super) itemised: bool,
    pub(super) target_label: Option<String>,
    pub(super) gain_description: String,
}

/// An owned [`StockProvenance`], for facts that outlive the positions they
/// were read from.
#[derive(Debug, Clone, PartialEq)]
enum OwnedProvenance {
    Harvest(HarvestYieldTier),
    Hunt(Option<String>),
    Quest {
        reward_item_id: i64,
        quest_run_id: i64,
        quest_id: i64,
        activity_context_id: Option<i64>,
    },
}

impl OwnedProvenance {
    fn from_borrowed(provenance: StockProvenance<'_>) -> Self {
        match provenance {
            StockProvenance::Harvest(tier) => Self::Harvest(tier),
            StockProvenance::Hunt(species) => Self::Hunt(species.map(str::to_string)),
            StockProvenance::Quest {
                reward_item_id,
                quest_run_id,
                quest_id,
                activity_context_id,
            } => Self::Quest {
                reward_item_id,
                quest_run_id,
                quest_id,
                activity_context_id,
            },
        }
    }

    fn borrowed(&self) -> StockProvenance<'_> {
        match self {
            Self::Harvest(tier) => StockProvenance::Harvest(*tier),
            Self::Hunt(species) => StockProvenance::Hunt(species.as_deref()),
            Self::Quest {
                reward_item_id,
                quest_run_id,
                quest_id,
                activity_context_id,
            } => StockProvenance::Quest {
                reward_item_id: *reward_item_id,
                quest_run_id: *quest_run_id,
                quest_id: *quest_id,
                activity_context_id: *activity_context_id,
            },
        }
    }
}

/// One recorded movement, owned, with the facts needed to write it back.
pub(super) struct MovementRow {
    provenance: Option<OwnedProvenance>,
    pub(super) session_definition_id: Option<i64>,
    pub(super) activity_context_id: Option<i64>,
    pub(super) tool_name: Option<String>,
    pub(super) quantity: f64,
    pub(super) tt_value: f64,
    source_line_id: Option<i64>,
    origin_item: Option<String>,
    paid_markup: f64,
    declared_markup: Option<f64>,
    origin_quantity: Option<f64>,
}

impl MovementRow {
    pub(super) fn provenance(&self) -> Option<StockProvenance<'_>> {
        self.provenance.as_ref().map(OwnedProvenance::borrowed)
    }

    pub(super) fn derived(&self) -> DerivedFacts<'_> {
        DerivedFacts {
            source_line_id: self.source_line_id,
            origin_item: self.origin_item.as_deref(),
            paid_markup: self.paid_markup,
            declared_markup: self.declared_markup,
            origin_quantity: self.origin_quantity,
            inventory_item_id: None,
        }
    }
}

const MOVEMENT_COLUMNS: &str =
    "source_kind, yield_tier, mob_species, session_definition_id, tool_name, \
     quest_reward_item_id, quest_run_id, quest_id, activity_context_id, quantity, tt_value, \
     source_line_id, origin_item, paid_markup, declared_markup, origin_quantity";

fn movement_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MovementRow> {
    let source_kind: String = row.get(0)?;
    let tier: Option<String> = row.get(1)?;
    let species: Option<String> = row.get(2)?;
    let reward_item_id: Option<i64> = row.get(5)?;
    let quest_run_id: Option<i64> = row.get(6)?;
    let quest_id: Option<i64> = row.get(7)?;
    let activity_context_id: Option<i64> = row.get(8)?;
    let provenance = match source_kind.as_str() {
        "harvest" => tier
            .as_deref()
            .map(|tier| OwnedProvenance::Harvest(HarvestYieldTier::from_db(tier))),
        "hunt" => Some(OwnedProvenance::Hunt(species)),
        "quest" => match (reward_item_id, quest_run_id, quest_id) {
            (Some(reward_item_id), Some(quest_run_id), Some(quest_id)) => {
                Some(OwnedProvenance::Quest {
                    reward_item_id,
                    quest_run_id,
                    quest_id,
                    activity_context_id,
                })
            }
            _ => None,
        },
        _ => None,
    };
    Ok(MovementRow {
        provenance,
        session_definition_id: row.get(3)?,
        activity_context_id,
        tool_name: row.get(4)?,
        quantity: row.get(9)?,
        tt_value: row.get(10)?,
        source_line_id: row.get(11)?,
        origin_item: row.get(12)?,
        paid_markup: row.get(13)?,
        declared_markup: row.get(14)?,
        origin_quantity: row.get(15)?,
    })
}

/// Every movement of one kind belonging to a listing, trade, or conversion.
pub(super) fn movement_rows(
    conn: &rusqlite::Connection,
    ref_id: &str,
    movement_kind: &str,
) -> rusqlite::Result<Vec<MovementRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MOVEMENT_COLUMNS} FROM stock_movements WHERE ref_id = ? AND movement_kind = ?"
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![ref_id, movement_kind], movement_row)?
        .collect();
    rows
}

fn asset_movement_rows(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> rusqlite::Result<Vec<MovementRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MOVEMENT_COLUMNS} FROM stock_movements \
         WHERE inventory_item_id = ? AND movement_kind = 'conversion_in'"
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![item_id], movement_row)?
        .collect();
    rows
}

/// One source consumed by a conversion, on its way to being stamped onto what
/// the conversion produced.
struct ConsumedRow {
    input_index: usize,
    line_id: i64,
    provenance: Option<OwnedProvenance>,
    session_definition_id: Option<i64>,
    activity_context_id: Option<i64>,
    tool_name: Option<String>,
    quantity: f64,
    inherited_source_line: Option<i64>,
    origin_item: Option<String>,
    origin_quantity: Option<f64>,
    inherited_claim: Option<f64>,
    /// Filled once every line is known: this row's share of the conversion.
    weight: f64,
    /// Filled likewise: the worth this row is carrying, if any.
    claim: Option<f64>,
}

/// One line of a conversion on its way to being recorded.
struct NewLine<'a> {
    side: ConversionLineSide,
    position: usize,
    item_name: &'a str,
    quantity: f64,
    tt_value: f64,
    acquisition: Option<ConversionAcquisition>,
    markup: Option<f64>,
    holding: Option<ConversionHolding>,
    inventory_item_id: Option<&'a str>,
}

fn insert_line(
    tx: &rusqlite::Transaction<'_>,
    conversion_id: &str,
    line: NewLine<'_>,
) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT INTO stock_conversion_lines \
             (conversion_id, side, position, item_name, quantity, tt_value, acquisition, \
              markup, holding, inventory_item_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            conversion_id,
            match line.side {
                ConversionLineSide::Input => "input",
                ConversionLineSide::Output => "output",
            },
            line.position as i64,
            line.item_name,
            line.quantity,
            line.tt_value,
            line.acquisition.map(ConversionAcquisition::as_str),
            line.markup,
            line.holding.map(ConversionHolding::as_str),
            line.inventory_item_id,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Record one conversion inside the caller's transaction. A refusal in terms
/// of the state found comes back as the inner `Err`, with nothing committed by
/// the caller.
pub(super) fn write_conversion(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    profession: Profession,
    request: &ItemConversion,
    converted_at: &str,
    now: f64,
    terms: &ConversionTerms,
) -> Result<Result<(), String>, DbError> {
    use rusqlite::OptionalExtension as _;

    // Assets are checked before anything is written.
    let mut assets: HashMap<&str, (String, f64, f64)> = HashMap::new();
    for input in &request.inputs {
        if let ConversionInput::Asset { item_id, .. } = input {
            let asset: Option<(String, f64, f64)> = tx
                .query_row(
                    "SELECT name, tt_value, markup_paid FROM inventory_items \
                     WHERE id = ? AND state = 'held'",
                    rusqlite::params![item_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some(asset) = asset else {
                return Ok(Err("That asset is no longer held".to_string()));
            };
            assets.insert(item_id.as_str(), asset);
        }
    }

    let (first_name, first_quantity) = match &request.inputs[0] {
        ConversionInput::Held {
            item_name,
            quantity,
            ..
        }
        | ConversionInput::Bought {
            item_name,
            quantity,
            ..
        } => (item_name.clone(), *quantity),
        ConversionInput::Asset { item_id, .. } => (assets[item_id.as_str()].0.clone(), 1.0),
    };
    let target = terms.target_label.clone().unwrap_or_else(|| {
        request
            .outputs
            .first()
            .map_or_else(|| "PED".to_string(), |output| output.item_name.clone())
    });
    // The lines reference this row, so it goes in first and its totals follow.
    tx.execute(
        "INSERT INTO stock_conversions \
             (id, source_item, target_item, profession, quantity, tt_value, \
              output_tt_value, attributed_tt, converted_at, created_at, recognised_markup, \
              ped_amount, itemised) \
         VALUES (?, ?, ?, ?, ?, 0, NULL, NULL, ?, ?, 0, ?, ?)",
        rusqlite::params![
            id,
            first_name,
            target,
            profession.as_str(),
            first_quantity,
            converted_at,
            now,
            request.ped_amount,
            terms.itemised,
        ],
    )?;

    let mut consumed: Vec<ConsumedRow> = Vec::new();
    let mut worths: Vec<Option<f64>> = Vec::with_capacity(request.inputs.len());
    let mut tt_in = 0.0_f64;
    let mut paid_total = 0.0_f64;

    for (position, input) in request.inputs.iter().enumerate() {
        match input {
            ConversionInput::Held {
                item_name,
                quantity,
                worth,
            } => {
                let (positions, unit_tt) = item_positions(tx, item_name)?;
                let plan = stock_allocation::allocate(
                    &as_source_positions(&positions),
                    *quantity,
                    unit_tt,
                );
                let line_tt = quantity * unit_tt;
                tt_in += line_tt;
                worths.push(*worth);
                let line_id = insert_line(
                    tx,
                    id,
                    NewLine {
                        side: ConversionLineSide::Input,
                        position,
                        item_name,
                        quantity: *quantity,
                        tt_value: line_tt,
                        acquisition: Some(ConversionAcquisition::Held),
                        markup: *worth,
                        holding: None,
                        inventory_item_id: None,
                    },
                )?;
                record_opening_balance(tx, item_name, id, &plan, converted_at, now)?;
                for allocation in &plan.allocations {
                    paid_total += allocation.derived.paid_markup;
                    insert_movement(
                        tx,
                        item_name,
                        "conversion_out",
                        Some(id),
                        allocation.provenance,
                        allocation.session_definition_id,
                        allocation.activity_context_id,
                        allocation.tool_name,
                        -allocation.quantity,
                        -allocation.tt_value,
                        converted_at,
                        now,
                        allocation.derived.outflow(),
                    )?;
                    let (origin_item, origin_quantity) = match allocation.derived.origin_item {
                        Some(root) => (Some(root.to_string()), allocation.derived.origin_quantity),
                        None if terms.itemised => {
                            (Some(item_name.clone()), Some(allocation.quantity))
                        }
                        None => (None, None),
                    };
                    consumed.push(ConsumedRow {
                        input_index: position,
                        line_id,
                        provenance: allocation.provenance.map(OwnedProvenance::from_borrowed),
                        session_definition_id: allocation.session_definition_id,
                        activity_context_id: allocation.activity_context_id,
                        tool_name: allocation.tool_name.map(str::to_string),
                        quantity: allocation.quantity,
                        inherited_source_line: allocation.derived.source_line_id,
                        origin_item,
                        origin_quantity,
                        inherited_claim: allocation.derived.declared_markup,
                        weight: 0.0,
                        claim: None,
                    });
                }
            }
            ConversionInput::Bought {
                item_name,
                quantity,
                tt_value,
                markup_paid,
            } => {
                tt_in += tt_value;
                paid_total += markup_paid;
                worths.push(None);
                insert_line(
                    tx,
                    id,
                    NewLine {
                        side: ConversionLineSide::Input,
                        position,
                        item_name,
                        quantity: *quantity,
                        tt_value: *tt_value,
                        acquisition: Some(ConversionAcquisition::Bought),
                        markup: (*markup_paid > 0.0).then_some(*markup_paid),
                        holding: None,
                        inventory_item_id: None,
                    },
                )?;
            }
            ConversionInput::Asset { item_id, worth } => {
                let (name, tt_value, markup_paid) = assets[item_id.as_str()].clone();
                tt_in += tt_value;
                paid_total += markup_paid;
                worths.push(*worth);
                let line_id = insert_line(
                    tx,
                    id,
                    NewLine {
                        side: ConversionLineSide::Input,
                        position,
                        item_name: &name,
                        quantity: 1.0,
                        tt_value,
                        acquisition: Some(ConversionAcquisition::Asset),
                        markup: *worth,
                        holding: None,
                        inventory_item_id: Some(item_id),
                    },
                )?;
                tx.execute(
                    "UPDATE inventory_items SET state = 'converted', disposed_at = ?, \
                         updated_at = ? WHERE id = ?",
                    rusqlite::params![converted_at, now, item_id],
                )?;
                for row in asset_movement_rows(tx, item_id)? {
                    consumed.push(ConsumedRow {
                        input_index: position,
                        line_id,
                        provenance: row.provenance,
                        session_definition_id: row.session_definition_id,
                        activity_context_id: row.activity_context_id,
                        tool_name: row.tool_name,
                        quantity: row.quantity,
                        inherited_source_line: row.source_line_id,
                        origin_item: row.origin_item,
                        origin_quantity: row.origin_quantity,
                        inherited_claim: row.declared_markup,
                        weight: 0.0,
                        claim: None,
                    });
                }
            }
        }
    }

    // Every line with a source counts equally; within a line, sources count
    // by what each supplied. Comparing raw quantities across lines would rank
    // items by how small their units happen to be.
    let mut line_totals: BTreeMap<usize, f64> = BTreeMap::new();
    for row in &consumed {
        *line_totals.entry(row.input_index).or_insert(0.0) += row.quantity;
    }
    let source_lines = line_totals.len() as f64;
    for row in &mut consumed {
        let line_total = line_totals[&row.input_index];
        let fraction = if line_total > STOCK_EPSILON {
            row.quantity / line_total
        } else {
            0.0
        };
        row.weight = fraction / source_lines;
        row.claim = match worths[row.input_index] {
            Some(worth) => Some(worth * fraction),
            None => row.inherited_claim,
        };
    }

    let tt_out: f64 = request.outputs.iter().map(|output| output.tt_value).sum();
    let ped = request.ped_amount.unwrap_or(0.0);
    let difference = tt_out + ped - tt_in;
    let (recognised, carried_paid) = if request.outputs.is_empty() {
        // Nothing is left to carry a cost forward, so it is settled now.
        (difference - paid_total, 0.0)
    } else if difference > 0.0 {
        // Paid markup bought that TT gain, so it nets off first and a gain is
        // never overstated while the cost waits for a sale.
        let netted = difference.min(paid_total);
        (difference - netted, paid_total - netted)
    } else {
        (difference, paid_total)
    };
    let recognised = eo_wire::normalizer::round_half_even(recognised, 4);

    // Worth and cost split across outputs by value. PED counts at face value.
    let total_value = tt_out + ped;
    let output_count = request.outputs.len().max(1) as f64;
    let value_share = |tt_value: f64| {
        if total_value > STOCK_EPSILON {
            tt_value / total_value
        } else {
            1.0 / output_count
        }
    };
    let ped_share = if total_value > STOCK_EPSILON {
        ped / total_value
    } else {
        0.0
    };

    for (position, output) in request.outputs.iter().enumerate() {
        let share = value_share(output.tt_value);
        let basis = if tt_out > STOCK_EPSILON {
            carried_paid * (output.tt_value / tt_out)
        } else {
            carried_paid / output_count
        };
        let asset_id = match output.holding {
            ConversionHolding::Asset => {
                let asset_id = Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO inventory_items (id, name, tt_value, markup_paid, acquired_at) \
                     VALUES (?, ?, ?, ?, ?)",
                    rusqlite::params![
                        asset_id,
                        output.item_name,
                        output.tt_value,
                        basis,
                        converted_at,
                    ],
                )?;
                Some(asset_id)
            }
            ConversionHolding::Loot => None,
        };
        insert_line(
            tx,
            id,
            NewLine {
                side: ConversionLineSide::Output,
                position,
                item_name: &output.item_name,
                quantity: output.quantity,
                tt_value: output.tt_value,
                acquisition: None,
                markup: None,
                holding: Some(output.holding),
                inventory_item_id: asset_id.as_deref(),
            },
        )?;

        if consumed.is_empty() {
            // Everything that went in was bought, so nothing here has an
            // activity behind it. The output still exists.
            insert_movement(
                tx,
                &output.item_name,
                "conversion_in",
                Some(id),
                None,
                None,
                None,
                None,
                output.quantity,
                output.tt_value,
                converted_at,
                now,
                DerivedFacts {
                    paid_markup: basis,
                    inventory_item_id: asset_id.as_deref(),
                    ..DerivedFacts::default()
                },
            )?;
            continue;
        }
        for row in &consumed {
            insert_movement(
                tx,
                &output.item_name,
                "conversion_in",
                Some(id),
                row.provenance.as_ref().map(OwnedProvenance::borrowed),
                row.session_definition_id,
                row.activity_context_id,
                row.tool_name.as_deref(),
                output.quantity * row.weight,
                output.tt_value * row.weight,
                converted_at,
                now,
                DerivedFacts {
                    source_line_id: if terms.itemised {
                        Some(row.line_id)
                    } else {
                        row.inherited_source_line
                    },
                    origin_item: row.origin_item.as_deref(),
                    paid_markup: basis * row.weight,
                    declared_markup: row.claim.map(|claim| claim * share),
                    origin_quantity: row.origin_quantity.map(|quantity| quantity * share),
                    inventory_item_id: asset_id.as_deref(),
                },
            )?;
        }
    }

    let mut gain_entry_id: Option<String> = None;
    if recognised.abs() > STOCK_EPSILON {
        let entry_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, 'convert')",
            rusqlite::params![
                entry_id,
                converted_at,
                if recognised > 0.0 {
                    "markup"
                } else {
                    "expense"
                },
                terms.gain_description,
                recognised.abs(),
            ],
        )?;
        gain_entry_id = Some(entry_id);
        daily_rollup::refresh_days(tx, [converted_at])?;
        write_credits(tx, id, recognised, ped_share, &consumed)?;
    }

    tx.execute(
        "UPDATE stock_conversions SET tt_value = ?, output_tt_value = ?, \
             recognised_markup = ?, gain_entry_id = ? WHERE id = ?",
        rusqlite::params![tt_in, tt_out + ped, recognised, gain_entry_id, id],
    )?;
    Ok(Ok(()))
}

/// Split what a conversion recognised on the day across its sources and write
/// the split down. Only the PED part of an outcome has been cashed, so only
/// that part settles declared worths; the rest waits for a sale.
fn write_credits(
    tx: &rusqlite::Transaction<'_>,
    conversion_id: &str,
    recognised: f64,
    ped_share: f64,
    consumed: &[ConsumedRow],
) -> rusqlite::Result<()> {
    let total_weight: f64 = consumed.iter().map(|row| row.weight).sum();
    let attributed: Vec<&ConsumedRow> = consumed
        .iter()
        .filter(|row| row.provenance.is_some())
        .collect();
    let attributed_weight: f64 = attributed.iter().map(|row| row.weight).sum();
    if total_weight <= STOCK_EPSILON || attributed_weight <= STOCK_EPSILON {
        return Ok(());
    }
    let claimable = recognised * (attributed_weight / total_weight);
    let lines: Vec<CreditLine> = attributed
        .iter()
        .map(|row| CreditLine {
            weight: row.weight,
            worth: row.claim.map(|claim| claim * ped_share),
        })
        .collect();
    let parts = stock_allocation::split_realised(claimable, &lines);
    for (row, amount) in attributed.iter().zip(parts) {
        let (source_kind, yield_tier, mob_species, quest_id, quest_context) =
            match row.provenance.as_ref().expect("filtered to known sources") {
                OwnedProvenance::Harvest(tier) => {
                    ("harvest", Some(tier.as_str()), None, None, None)
                }
                OwnedProvenance::Hunt(species) => ("hunt", None, species.as_deref(), None, None),
                OwnedProvenance::Quest {
                    quest_id,
                    activity_context_id,
                    ..
                } => ("quest", None, None, Some(*quest_id), *activity_context_id),
            };
        tx.execute(
            "INSERT INTO stock_conversion_credits \
                 (conversion_id, source_kind, yield_tier, mob_species, session_definition_id, \
                  quest_id, activity_context_id, origin_item, origin_quantity, amount) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                conversion_id,
                source_kind,
                yield_tier,
                mob_species,
                row.session_definition_id,
                quest_id,
                row.activity_context_id.or(quest_context),
                row.origin_item,
                row.origin_quantity.map(|quantity| quantity * ped_share),
                amount,
            ],
        )?;
    }
    Ok(())
}

/// Whether undoing a conversion would strand something: stock it produced that
/// has since moved on, or an asset it made that is no longer held.
pub(super) fn conversion_blocker(
    conn: &rusqlite::Connection,
    conversion_id: &str,
) -> rusqlite::Result<Option<(String, f64)>> {
    use rusqlite::OptionalExtension as _;
    if let Some(found) = reversal_blocker(conn, conversion_id)? {
        return Ok(Some(found));
    }
    let moved_on: Option<String> = conn
        .query_row(
            "SELECT i.name FROM stock_conversion_lines l \
             JOIN inventory_items i ON i.id = l.inventory_item_id \
             WHERE l.conversion_id = ? AND l.side = 'output' AND i.state <> 'held' \
             LIMIT 1",
            rusqlite::params![conversion_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(moved_on.map(|name| (name, 1.0)))
}

/// Take back what a conversion did to Assets and to the credits it wrote.
pub(super) fn unwind_assets_and_credits(
    tx: &rusqlite::Transaction<'_>,
    conversion_id: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM stock_conversion_credits WHERE conversion_id = ?",
        rusqlite::params![conversion_id],
    )?;
    tx.execute(
        "UPDATE inventory_items SET state = 'held', disposed_at = NULL, \
             updated_at = unixepoch('now') \
         WHERE id IN (SELECT inventory_item_id FROM stock_conversion_lines \
                      WHERE conversion_id = ? AND side = 'input' \
                        AND inventory_item_id IS NOT NULL)",
        rusqlite::params![conversion_id],
    )?;
    tx.execute(
        "DELETE FROM inventory_items \
         WHERE id IN (SELECT inventory_item_id FROM stock_conversion_lines \
                      WHERE conversion_id = ? AND side = 'output' \
                        AND inventory_item_id IS NOT NULL)",
        rusqlite::params![conversion_id],
    )?;
    Ok(())
}

/// Whether a held asset came out of a conversion.
pub(super) fn is_converted_asset(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_movements WHERE inventory_item_id = ?)",
        rusqlite::params![item_id],
        |row| row.get(0),
    )
}

/// The fraction of an asset with a known activity behind it. Zero for one the
/// player entered by hand.
pub(super) fn asset_attributed_share(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> rusqlite::Result<f64> {
    let (attributed, total): (f64, f64) = conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN source_kind IN ('hunt', 'harvest', 'quest') \
                                  THEN quantity ELSE 0 END), 0), \
                COALESCE(SUM(quantity), 0) \
         FROM stock_movements \
         WHERE inventory_item_id = ? AND movement_kind = 'conversion_in'",
        rusqlite::params![item_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(if total > STOCK_EPSILON {
        (attributed / total).clamp(0.0, 1.0)
    } else {
        0.0
    })
}

pub(super) fn definition_names(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT id, name FROM session_definitions")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect();
    rows
}

/// What to call the activity behind a source, in the player's terms.
fn origin_label(
    source_kind: &str,
    definition_id: Option<i64>,
    definitions: &HashMap<i64, String>,
) -> Option<String> {
    let named = definition_id.and_then(|id| definitions.get(&id).cloned());
    match source_kind {
        "harvest" => Some("Tree Cutting".to_string()),
        "hunt" => Some(named.unwrap_or_else(|| "Hunting".to_string())),
        "quest" => Some(named.unwrap_or_else(|| "Quests".to_string())),
        _ => None,
    }
}

fn origins_from(totals: BTreeMap<String, f64>) -> Vec<HoldingOriginRow> {
    let mut rows: Vec<HoldingOriginRow> = totals
        .into_iter()
        .map(|(label, quantity)| HoldingOriginRow { label, quantity })
        .collect();
    rows.sort_by(|a, b| {
        b.quantity
            .partial_cmp(&a.quantity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.label.cmp(&b.label))
    });
    rows
}

fn derived_from_totals(totals: BTreeMap<String, f64>) -> Vec<DerivedSourceRow> {
    totals
        .into_iter()
        .map(|(item_name, quantity)| DerivedSourceRow {
            item_name,
            quantity,
        })
        .collect()
}

/// The activities a sale of this open stock would credit.
pub(super) fn position_origins(
    open: &[OpenPosition],
    definitions: &HashMap<i64, String>,
) -> Vec<HoldingOriginRow> {
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for position in open {
        if let Some(label) = origin_label(
            &position.key.source_kind,
            position.key.definition_id,
            definitions,
        ) {
            *totals.entry(label).or_insert(0.0) += position.quantity;
        }
    }
    origins_from(totals)
}

/// The root items this open stock descends from.
pub(super) fn position_derived_from(open: &[OpenPosition]) -> Vec<DerivedSourceRow> {
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for position in open {
        if let (false, Some(per_unit)) = (
            position.key.origin_item.is_empty(),
            position.lot.origin_per_unit,
        ) {
            *totals
                .entry(position.key.origin_item.clone())
                .or_insert(0.0) += position.quantity * per_unit;
        }
    }
    derived_from_totals(totals)
}

/// Fill in where each held asset came from.
pub(super) fn attach_asset_origins(
    conn: &rusqlite::Connection,
    rows: &mut [InventoryRow],
) -> rusqlite::Result<()> {
    let definitions = definition_names(conn)?;
    let mut stmt = conn.prepare(
        "SELECT inventory_item_id, source_kind, session_definition_id, origin_item, \
                SUM(quantity), SUM(origin_quantity) \
         FROM stock_movements \
         WHERE inventory_item_id IS NOT NULL AND movement_kind = 'conversion_in' \
         GROUP BY inventory_item_id, source_kind, session_definition_id, origin_item",
    )?;
    let mut origins: HashMap<String, BTreeMap<String, f64>> = HashMap::new();
    let mut derived: HashMap<String, BTreeMap<String, f64>> = HashMap::new();
    let mut found = stmt.query([])?;
    while let Some(row) = found.next()? {
        let item_id: String = row.get(0)?;
        let source_kind: String = row.get(1)?;
        let definition_id: Option<i64> = row.get(2)?;
        let origin_item: Option<String> = row.get(3)?;
        let quantity: f64 = row.get(4)?;
        let origin_quantity: Option<f64> = row.get(5)?;
        if let Some(label) = origin_label(&source_kind, definition_id, &definitions) {
            *origins
                .entry(item_id.clone())
                .or_default()
                .entry(label)
                .or_insert(0.0) += quantity;
        }
        if let (Some(origin_item), Some(origin_quantity)) = (origin_item, origin_quantity) {
            *derived
                .entry(item_id)
                .or_default()
                .entry(origin_item)
                .or_insert(0.0) += origin_quantity;
        }
    }
    for row in rows {
        if let Some(totals) = origins.remove(&row.id) {
            row.origins = origins_from(totals);
        }
        if let Some(totals) = derived.remove(&row.id) {
            row.derived_from = derived_from_totals(totals);
        }
    }
    Ok(())
}

/// What the stock in one sale descends from.
pub(super) fn sale_derived_from(
    conn: &rusqlite::Connection,
    ref_id: &str,
    movement_kind: &str,
    asset_item_id: Option<&str>,
) -> rusqlite::Result<Vec<DerivedSourceRow>> {
    let (filter, first, second) = match asset_item_id {
        Some(item_id) => (
            "inventory_item_id = ? AND movement_kind = ?",
            item_id,
            "conversion_in",
        ),
        None => ("ref_id = ? AND movement_kind = ?", ref_id, movement_kind),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT origin_item, ABS(SUM(origin_quantity)) FROM stock_movements \
         WHERE {filter} AND origin_item IS NOT NULL \
         GROUP BY origin_item ORDER BY origin_item"
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![first, second], |row| {
            Ok(DerivedSourceRow {
                item_name: row.get(0)?,
                quantity: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            })
        })?
        .collect();
    rows
}

fn conversion_lines(
    conn: &rusqlite::Connection,
    conversion_id: &str,
) -> rusqlite::Result<Vec<ConversionLineRow>> {
    let mut stmt = conn.prepare(
        "SELECT side, item_name, quantity, tt_value, acquisition, markup, holding \
         FROM stock_conversion_lines WHERE conversion_id = ? \
         ORDER BY CASE side WHEN 'input' THEN 0 ELSE 1 END, position",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![conversion_id], |row| {
            Ok(ConversionLineRow {
                side: if row.get::<_, String>(0)? == "input" {
                    ConversionLineSide::Input
                } else {
                    ConversionLineSide::Output
                },
                item_name: row.get(1)?,
                quantity: row.get(2)?,
                tt_value: row.get(3)?,
                acquisition: row
                    .get::<_, Option<String>>(4)?
                    .map(|value| ConversionAcquisition::from_db(&value)),
                markup: row.get(5)?,
                holding: row
                    .get::<_, Option<String>>(6)?
                    .map(|value| ConversionHolding::from_db(&value)),
            })
        })?
        .collect();
    rows
}

/// One row of a conversion's header, as History reads it.
struct ConversionHeader {
    id: String,
    source: String,
    target: String,
    quantity: f64,
    tt_value: f64,
    converted_at: String,
    undone_at: Option<String>,
    output_tt: Option<f64>,
    attributed_tt: Option<f64>,
    recognised: Option<f64>,
    ped_amount: Option<f64>,
    itemised: bool,
}

/// This profession's conversions, for the History list.
pub(super) fn push_history_rows(
    conn: &rusqlite::Connection,
    profession: Profession,
    rows: &mut Vec<ActivityHistoryRow>,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, source_item, target_item, quantity, tt_value, converted_at, \
                undone_at, output_tt_value, attributed_tt, recognised_markup, ped_amount, \
                itemised \
         FROM stock_conversions WHERE profession = ?",
    )?;
    let headers = stmt
        .query_map(rusqlite::params![profession.as_str()], |row| {
            Ok(ConversionHeader {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                quantity: row.get(3)?,
                tt_value: row.get(4)?,
                converted_at: row.get(5)?,
                undone_at: row.get(6)?,
                output_tt: row.get(7)?,
                attributed_tt: row.get(8)?,
                recognised: row.get(9)?,
                ped_amount: row.get(10)?,
                itemised: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut credited =
        conn.prepare("SELECT SUM(amount) FROM stock_conversion_credits WHERE conversion_id = ?")?;
    for header in headers {
        let undone = header.undone_at.is_some();
        let blocker = if undone {
            None
        } else {
            conversion_blocker(conn, &header.id)?
        };
        let (net_markup, activity_net_markup) = match header.recognised {
            Some(recognised) => {
                let net = (recognised.abs() > STOCK_EPSILON).then_some(recognised);
                let activity: Option<f64> =
                    credited.query_row(rusqlite::params![header.id], |row| row.get(0))?;
                (net, net.map(|_| activity.unwrap_or(0.0)))
            }
            // Recorded before conversions wrote their own split.
            None => (
                header.output_tt.map(|output| output - header.tt_value),
                header
                    .output_tt
                    .zip(header.attributed_tt)
                    .map(|(output, attributed)| {
                        stock_allocation::resolve_sale(
                            header.quantity,
                            if header.tt_value > STOCK_EPSILON {
                                header.quantity * attributed / header.tt_value
                            } else {
                                0.0
                            },
                            header.tt_value,
                            0.0,
                            output,
                            0.0,
                            0.0,
                        )
                        .activity_net_markup
                    }),
            ),
        };
        let conversion_lines = conversion_lines(conn, &header.id)?;
        rows.push(ActivityHistoryRow {
            conversion_lines,
            ped_amount: header.ped_amount,
            itemised: header.itemised,
            derived_from: Vec::new(),
            kind: "conversion".to_string(),
            subject_kind: "loot".to_string(),
            channel: "conversion".to_string(),
            status: "converted".to_string(),
            item_name: header.source,
            target_item: Some(header.target),
            occurred_at: header.converted_at,
            quantity: header.quantity,
            tt_value: header.tt_value,
            net_markup,
            activity_net_markup,
            can_revert_sale: false,
            can_delete: !undone && blocker.is_none(),
            undo_blocked_reason: blocker.map(|(item, short)| blocked_reason(&item, short)),
            undone,
            id: header.id,
        });
    }
    Ok(())
}

fn validate(request: &mut ItemConversion) -> Result<(), AnalyticsError> {
    let invalid = AnalyticsError::InvalidInput;
    if request.inputs.is_empty() {
        return Err(invalid("a conversion needs something going in"));
    }
    let ped_amount = request.ped_amount.filter(|amount| *amount != 0.0);
    request.ped_amount = ped_amount;
    if request.outputs.is_empty() && ped_amount.is_none() {
        return Err(invalid("a conversion needs a result"));
    }
    if ped_amount.is_some_and(|amount| !amount.is_finite() || amount < 0.0) {
        return Err(invalid("a PED outcome cannot be negative"));
    }
    let positive = |value: f64| value.is_finite() && value > 0.0;
    let amount = |value: f64| value.is_finite() && value >= 0.0;
    for input in &request.inputs {
        let (name, quantity, money) = match input {
            ConversionInput::Held {
                item_name,
                quantity,
                worth,
            } => (Some(item_name), *quantity, worth.unwrap_or(0.0)),
            ConversionInput::Bought {
                item_name,
                quantity,
                tt_value,
                markup_paid,
            } => {
                if !amount(*tt_value) {
                    return Err(invalid("a TT value cannot be negative"));
                }
                (Some(item_name), *quantity, *markup_paid)
            }
            ConversionInput::Asset { worth, .. } => (None, 1.0, worth.unwrap_or(0.0)),
        };
        if name.is_some_and(|name| name.trim().is_empty()) {
            return Err(invalid("every line needs an item"));
        }
        if name.is_some_and(|name| is_universal_ammo(name)) {
            return Err(invalid(
                "Universal Ammo is liquid value, not inventory stock",
            ));
        }
        if !positive(quantity) {
            return Err(invalid("every line needs a positive quantity"));
        }
        if !amount(money) {
            return Err(invalid("a markup cannot be negative"));
        }
    }
    for output in &mut request.outputs {
        if output.item_name.trim().is_empty() {
            return Err(invalid("every line needs an item"));
        }
        if is_universal_ammo(&output.item_name) {
            return Err(invalid(
                "Universal Ammo is liquid value, not inventory stock",
            ));
        }
        if !positive(output.quantity) {
            return Err(invalid("every line needs a positive quantity"));
        }
        if !amount(output.tt_value) {
            return Err(invalid("a TT value cannot be negative"));
        }
        // An asset is one whole holding, however many it was recorded as.
        if output.holding == ConversionHolding::Asset {
            output.quantity = 1.0;
        }
    }
    Ok(())
}

impl AnalyticsService {
    /// Record an itemised conversion: what went in, what came out, and what
    /// any of it cost or was worth.
    pub async fn convert_items(
        &self,
        profession: Profession,
        mut request: ItemConversion,
    ) -> Result<(), AnalyticsError> {
        validate(&mut request)?;
        let id = Uuid::new_v4().to_string();
        let converted_at = request
            .converted_at
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let first_input =
                    request
                        .inputs
                        .first()
                        .map_or_else(String::new, |input| match input {
                            ConversionInput::Held { item_name, .. }
                            | ConversionInput::Bought { item_name, .. } => item_name.clone(),
                            ConversionInput::Asset { .. } => "asset".to_string(),
                        });
                let terms = ConversionTerms {
                    itemised: true,
                    target_label: None,
                    gain_description: format!("Conversion: {first_input}"),
                };
                let written =
                    write_conversion(&tx, &id, profession, &request, &converted_at, now, &terms)?;
                if written.is_ok() {
                    tx.commit()?;
                }
                Ok(written)
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::write_service;
    use super::*;

    const ARIS: i64 = 5;
    const MAYHEM: i64 = 17;
    const MINING: i64 = 9;

    fn held(item: &str, quantity: f64) -> ConversionInput {
        ConversionInput::Held {
            item_name: item.to_string(),
            quantity,
            worth: None,
        }
    }

    fn held_worth(item: &str, quantity: f64, worth: f64) -> ConversionInput {
        ConversionInput::Held {
            item_name: item.to_string(),
            quantity,
            worth: Some(worth),
        }
    }

    fn bought(item: &str, quantity: f64, tt_value: f64, markup_paid: f64) -> ConversionInput {
        ConversionInput::Bought {
            item_name: item.to_string(),
            quantity,
            tt_value,
            markup_paid,
        }
    }

    fn output(item: &str, tt_value: f64, holding: ConversionHolding) -> ConversionOutput {
        ConversionOutput {
            item_name: item.to_string(),
            quantity: 1.0,
            tt_value,
            holding,
        }
    }

    /// Put `quantity` of an item in stock as hunted loot under a definition.
    async fn seed_hunted(
        service: &AnalyticsService,
        definition_id: i64,
        item: &str,
        quantity: i64,
        tt: f64,
    ) {
        let item = item.to_string();
        service
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO session_definitions(id, name, ad_hoc_segments, \
                     is_active) VALUES(?1, 'Definition ' || ?1, 0, 1)",
                    rusqlite::params![definition_id],
                )?;
                let session = format!("s{definition_id}");
                conn.execute(
                    "INSERT OR IGNORE INTO tracking_sessions(id, started_at, ended_at, \
                     is_active, definition_id) VALUES(?1, 1000.0, 2000.0, 0, ?2)",
                    rusqlite::params![session, definition_id],
                )?;
                let kill = format!("{session}-{item}");
                conn.execute(
                    "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, \
                     timestamp, shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, \
                     is_hof) VALUES(?1, ?2, 'Atrox', 'Atrox', '', 1500.0, 1, 1.0, 0.1, ?3, 0, 0)",
                    rusqlite::params![kill, session, tt],
                )?;
                conn.execute(
                    "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
                     VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![kill, item, quantity, tt],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn request(service: &AnalyticsService, conversion: ItemConversion) {
        service
            .convert_items(Profession::Inventory, conversion)
            .await
            .unwrap();
    }

    async fn credited(service: &AnalyticsService, definition_id: i64) -> f64 {
        service
            .realised_markup_by_definition()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.definition_id == definition_id)
            .map_or(0.0, |row| row.net_markup)
    }

    async fn stock(
        service: &AnalyticsService,
        item: &str,
    ) -> Option<crate::analytics::StockPositionRow> {
        service
            .stock_positions(Profession::Inventory)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.item_name == item)
    }

    async fn ledger(service: &AnalyticsService) -> Vec<(String, String, f64)> {
        service
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT type, description, amount FROM ledger_entries ORDER BY rowid",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap()
    }

    fn hand_in() -> ItemConversion {
        ItemConversion {
            inputs: vec![
                held("Hyperion Daily Voucher", 80.0),
                bought("Component X", 120.0, 25.0, 5.0),
                bought("Component Y", 140.0, 25.0, 5.0),
            ],
            outputs: vec![output("Hyperion Harness", 50.0, ConversionHolding::Loot)],
            ped_amount: None,
            converted_at: Some("2026-06-02".to_string()),
        }
    }

    async fn list_and_sell(service: &AnalyticsService, item: &str, price: f64, fee: f64) {
        let listing = service
            .create_auction_listing(
                Profession::Inventory,
                item,
                1.0,
                price,
                None,
                0.0,
                Some("2026-06-03"),
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, price, fee, Some("2026-06-04"))
            .await
            .unwrap()
            .unwrap();
    }

    /// The hand-in as agreed: vouchers plus bought components into a harness,
    /// which sells and credits the vouchers' origin with what it cleared
    /// after the paid markup and the fee.
    #[tokio::test]
    async fn a_hand_in_credits_the_vouchers_origin_after_paid_markup_and_fees() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;

        request(&service, hand_in()).await;

        let harness = stock(&service, "Hyperion Harness")
            .await
            .expect("it is held");
        assert!((harness.quantity - 1.0).abs() < 1e-9);
        assert!((harness.tt_value - 50.0).abs() < 1e-9);
        assert!((harness.paid_markup - 10.0).abs() < 1e-9);
        assert_eq!(harness.derived_from[0].item_name, "Hyperion Daily Voucher");
        assert!((harness.derived_from[0].quantity - 80.0).abs() < 1e-9);
        assert!(
            ledger(&service).await.is_empty(),
            "no TT moved and nothing is sold"
        );
        assert!(credited(&service, ARIS).await.abs() < 1e-9);

        list_and_sell(&service, "Hyperion Harness", 220.0, 8.0).await;

        assert_eq!(
            ledger(&service).await,
            vec![
                (
                    "markup".to_string(),
                    "Auction Sale: Hyperion Harness".to_string(),
                    160.0
                ),
                (
                    "expense".to_string(),
                    "Auction Fee: Hyperion Harness".to_string(),
                    8.0
                ),
            ]
        );
        assert!((credited(&service, ARIS).await - 152.0).abs() < 1e-9);

        let derived = service.realised_derived_by_definition().await.unwrap();
        assert_eq!(derived[0].origin_item, "Hyperion Daily Voucher");
        assert!((derived[0].quantity - 80.0).abs() < 1e-9);
        assert!((derived[0].net_markup - 152.0).abs() < 1e-9);
    }

    /// Most vouchers arrive as quest rewards rather than loot, and those have
    /// to credit their definition and their quest through a hand-in too.
    #[tokio::test]
    async fn quest_reward_vouchers_credit_their_definition_and_quest() {
        let (_dir, service) = write_service().await;
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO session_definitions(id, name, ad_hoc_segments, is_active) \
                     VALUES(5, 'ARIS Dailies', 0, 1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active, \
                     definition_id) VALUES('aris', 100.0, 300.0, 0, 5)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_contexts(id, session_id, created_at) \
                     VALUES(31, 'aris', 100.0)",
                    [],
                )?;
                conn.execute("INSERT INTO quests(id, name) VALUES(41, 'ARIS Daily')", [])?;
                conn.execute(
                    "INSERT INTO quest_runs(id, quest_id, status, started_at, completed_at) \
                     VALUES(42, 41, 'completed', 100, 200)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completions \
                     (id, session_id, quest_id, completed_at, reward_source, reward_kind, \
                      reward_outcome, reward_policy_snapshot, quest_run_id) \
                     VALUES(43, 'aris', 41, 200, 'tracked_loot', 'item', \
                            'confirmed', 'completion_clump', 42)",
                    [],
                )?;
                conn.execute("UPDATE quest_runs SET completion_id = 43 WHERE id = 42", [])?;
                conn.execute(
                    "INSERT INTO session_quest_completion_reward_items \
                     (id, completion_id, item_name, quantity, value_ped, accounting_kind) \
                     VALUES(44, 43, 'Hyperion Daily Voucher', 80, 0, 'stock')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO quest_reward_attributions \
                     (completion_id, activity_context_id, session_definition_id, weight, basis) \
                     VALUES(43, 31, 5, 1, 'duration')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        request(&service, hand_in()).await;
        list_and_sell(&service, "Hyperion Harness", 220.0, 8.0).await;

        assert!((credited(&service, ARIS).await - 152.0).abs() < 1e-9);
        let by_quest = service
            .db
            .with_reader(|conn| Ok(crate::analytics::realised_markup_by_quest(conn)?))
            .await
            .unwrap();
        assert!((by_quest[&41] - 152.0).abs() < 1e-9);
    }

    /// Declared worth on the player's own inputs goes to the activities that
    /// farmed them, and only once the sale has cleared.
    #[tokio::test]
    async fn declared_worth_is_paid_to_the_inputs_activity_when_the_output_sells() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        seed_hunted(&service, MINING, "Component X", 120, 25.0).await;
        seed_hunted(&service, MINING, "Component Y", 140, 25.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![
                    held("Hyperion Daily Voucher", 80.0),
                    held_worth("Component X", 120.0, 5.0),
                    held_worth("Component Y", 140.0, 5.0),
                ],
                outputs: vec![output("Hyperion Harness", 50.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;
        assert!(
            credited(&service, MINING).await.abs() < 1e-9,
            "unsold markup is not realised"
        );

        list_and_sell(&service, "Hyperion Harness", 220.0, 8.0).await;

        assert_eq!(ledger(&service).await[0].2, 170.0);
        assert!((credited(&service, MINING).await - 10.0).abs() < 1e-9);
        assert!((credited(&service, ARIS).await - 152.0).abs() < 1e-9);
    }

    /// With nothing declared, every line with a source shares equally,
    /// whatever its unit count.
    #[tokio::test]
    async fn undeclared_lines_share_equally_regardless_of_unit_counts() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        seed_hunted(&service, MINING, "Component X", 8000, 25.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![
                    held("Hyperion Daily Voucher", 80.0),
                    held("Component X", 8000.0),
                ],
                outputs: vec![output("Trophy", 25.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;
        list_and_sell(&service, "Trophy", 125.0, 0.0).await;

        assert!((credited(&service, ARIS).await - 50.0).abs() < 1e-9);
        assert!((credited(&service, MINING).await - 50.0).abs() < 1e-9);
    }

    /// Boxes written straight to a PED result credit their origin on the day.
    #[tokio::test]
    async fn a_ped_outcome_is_credited_immediately() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 145, 0.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![held("Halloween Strongbox", 100.0)],
                outputs: vec![],
                ped_amount: Some(100.0),
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        assert_eq!(
            ledger(&service).await,
            vec![(
                "markup".to_string(),
                "Conversion: Halloween Strongbox".to_string(),
                100.0
            )]
        );
        assert!((credited(&service, MAYHEM).await - 100.0).abs() < 1e-9);
        let boxes = stock(&service, "Halloween Strongbox").await.unwrap();
        assert!((boxes.quantity - 45.0).abs() < 1e-9);
        let derived = service.realised_derived_by_definition().await.unwrap();
        assert!((derived[0].quantity - 100.0).abs() < 1e-9);
    }

    /// Boxes opened into items: their TT counts on the day and their markup
    /// counts as each one sells, all to the same origin.
    #[tokio::test]
    async fn opened_boxes_recognise_tt_now_and_markup_as_items_sell() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 100, 0.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![held("Halloween Strongbox", 100.0)],
                outputs: vec![
                    output("Pumpkin Blade", 20.0, ConversionHolding::Loot),
                    output("Candy Pile", 10.0, ConversionHolding::Loot),
                ],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        assert_eq!(ledger(&service).await[0].2, 30.0);
        assert!((credited(&service, MAYHEM).await - 30.0).abs() < 1e-9);

        list_and_sell(&service, "Pumpkin Blade", 70.0, 0.0).await;
        list_and_sell(&service, "Candy Pile", 30.0, 0.0).await;

        assert!((credited(&service, MAYHEM).await - 100.0).abs() < 1e-9);
    }

    /// A TT gain repays markup paid before anything is shown as gained.
    #[tokio::test]
    async fn paid_markup_nets_against_a_tt_gain_first() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 100, 0.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![
                    held("Halloween Strongbox", 100.0),
                    bought("Strongbox Key", 100.0, 0.0, 12.0),
                ],
                outputs: vec![output("Pumpkin Blade", 30.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        assert_eq!(ledger(&service).await[0].2, 18.0);
        assert!(
            stock(&service, "Pumpkin Blade")
                .await
                .unwrap()
                .paid_markup
                .abs()
                < 1e-9
        );
    }

    /// TT that vanishes in a conversion is a loss on the day.
    #[tokio::test]
    async fn lost_tt_is_an_expense_on_the_day() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MINING, "Component X", 100, 40.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![held("Component X", 100.0)],
                outputs: vec![output("Trophy", 10.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        let entries = ledger(&service).await;
        assert_eq!(entries[0].0, "expense");
        assert_eq!(entries[0].2, 30.0);
        assert!((credited(&service, MINING).await + 30.0).abs() < 1e-9);
    }

    /// An expired listing brings the item home with everything it carried,
    /// and the fee stays a ledger cost no activity bears.
    #[tokio::test]
    async fn an_expired_listing_returns_the_item_with_its_terms_intact() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        request(&service, hand_in()).await;

        let listing = service
            .create_auction_listing(
                Profession::Inventory,
                "Hyperion Harness",
                1.0,
                220.0,
                None,
                3.0,
                Some("2026-06-03"),
                None,
            )
            .await
            .unwrap();
        assert!(
            stock(&service, "Hyperion Harness")
                .await
                .unwrap()
                .quantity
                .abs()
                < 1e-9
        );

        service
            .expire_auction_listing(&listing.id, Some("2026-06-05"))
            .await
            .unwrap()
            .unwrap();

        let harness = stock(&service, "Hyperion Harness").await.unwrap();
        assert!((harness.quantity - 1.0).abs() < 1e-9);
        assert!((harness.paid_markup - 10.0).abs() < 1e-9);
        assert!(
            credited(&service, ARIS).await.abs() < 1e-9,
            "no activity bears the fee"
        );
        assert_eq!(
            ledger(&service).await,
            vec![(
                "expense".to_string(),
                "Auction Fee: Hyperion Harness".to_string(),
                3.0
            )]
        );

        // It can then sell normally, and the earlier fee is still not charged.
        list_and_sell(&service, "Hyperion Harness", 220.0, 8.0).await;
        assert!((credited(&service, ARIS).await - 152.0).abs() < 1e-9);
    }

    /// Held as an asset, the harness keeps its origin and pays it on sale.
    #[tokio::test]
    async fn an_output_held_as_an_asset_keeps_its_origin() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        let mut conversion = hand_in();
        conversion.outputs[0].holding = ConversionHolding::Asset;
        request(&service, conversion).await;

        assert!(
            stock(&service, "Hyperion Harness").await.is_none(),
            "an asset is not loot stock"
        );
        let assets = service.list_inventory().await.unwrap();
        assert_eq!(assets.len(), 1);
        assert!((assets[0].tt_value - 50.0).abs() < 1e-9);
        assert!((assets[0].markup_paid - 10.0).abs() < 1e-9);
        assert_eq!(
            assets[0].derived_from[0].item_name,
            "Hyperion Daily Voucher"
        );
        assert_eq!(assets[0].origins[0].label, "Definition 5");

        let listing = service
            .create_equipment_listing(&assets[0].id, 220.0, None, 0.0, Some("2026-06-03"), None)
            .await
            .unwrap()
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 220.0, 8.0, Some("2026-06-04"))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(ledger(&service).await[0].2, 160.0);
        assert!((credited(&service, ARIS).await - 152.0).abs() < 1e-9);
    }

    /// An asset the player typed in stays exactly as it was: its sale reaches
    /// the ledger and no activity.
    #[tokio::test]
    async fn a_hand_entered_asset_still_credits_no_activity() {
        let (_dir, service) = write_service().await;
        let item = service
            .create_inventory_item("Bought Armour", 50.0, 10.0, None, None)
            .await
            .unwrap();
        service
            .trade_equipment(&item.id, 220.0, Some("2026-06-04"))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(ledger(&service).await[0].2, 160.0);
        assert!(service
            .realised_markup_by_definition()
            .await
            .unwrap()
            .is_empty());
    }

    /// Chains flatten at each step: a converted item used in another
    /// conversion still credits the original activity.
    #[tokio::test]
    async fn a_chain_of_conversions_credits_the_root_activity() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        request(&service, hand_in()).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![
                    held("Hyperion Harness", 1.0),
                    bought("Catalyst", 1.0, 0.0, 15.0),
                ],
                outputs: vec![output("Improved Harness", 50.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-03".to_string()),
            },
        )
        .await;
        let improved = stock(&service, "Improved Harness").await.unwrap();
        assert!(
            (improved.paid_markup - 25.0).abs() < 1e-9,
            "both costs carry forward"
        );

        list_and_sell(&service, "Improved Harness", 300.0, 0.0).await;

        assert!((credited(&service, ARIS).await - 225.0).abs() < 1e-9);
        let derived = service.realised_derived_by_definition().await.unwrap();
        assert_eq!(derived[0].origin_item, "Hyperion Daily Voucher");
        assert!((derived[0].quantity - 80.0).abs() < 1e-9);
    }

    /// An asset can go in as an ingredient, and undoing that brings it back.
    #[tokio::test]
    async fn an_asset_used_as_an_input_is_consumed_and_restored_by_undo() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        let mut first = hand_in();
        first.outputs[0].holding = ConversionHolding::Asset;
        request(&service, first).await;
        let asset = service.list_inventory().await.unwrap().remove(0);

        request(
            &service,
            ItemConversion {
                inputs: vec![ConversionInput::Asset {
                    item_id: asset.id.clone(),
                    worth: None,
                }],
                outputs: vec![output("Improved Harness", 50.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-03".to_string()),
            },
        )
        .await;
        assert!(service.list_inventory().await.unwrap().is_empty());
        let improved = stock(&service, "Improved Harness").await.unwrap();
        assert!((improved.paid_markup - 10.0).abs() < 1e-9);

        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        let conversion = history
            .iter()
            .find(|row| row.target_item.as_deref() == Some("Improved Harness"))
            .unwrap();
        assert!(service.undo_stock_conversion(&conversion.id).await.unwrap());

        assert_eq!(service.list_inventory().await.unwrap()[0].id, asset.id);
        assert!(
            stock(&service, "Improved Harness").await.is_none(),
            "what it produced is unmade"
        );
    }

    /// Undo takes back everything a conversion did, including an asset it
    /// made, the ledger row, and the credit.
    #[tokio::test]
    async fn undo_reverses_a_conversion_completely() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 100, 0.0).await;
        request(
            &service,
            ItemConversion {
                inputs: vec![held("Halloween Strongbox", 100.0)],
                outputs: vec![output("Pumpkin Blade", 30.0, ConversionHolding::Asset)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        assert!(history[0].itemised);
        assert_eq!(history[0].conversion_lines.len(), 2);
        assert!(service.undo_stock_conversion(&history[0].id).await.unwrap());

        assert!(service.list_inventory().await.unwrap().is_empty());
        assert!(ledger(&service).await.is_empty());
        assert!(credited(&service, MAYHEM).await.abs() < 1e-9);
        let boxes = stock(&service, "Halloween Strongbox").await.unwrap();
        assert!((boxes.quantity - 100.0).abs() < 1e-9);
    }

    /// Undo is refused once the output has been sold or listed.
    #[tokio::test]
    async fn undo_is_blocked_once_an_output_has_moved_on() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        let mut conversion = hand_in();
        conversion.outputs[0].holding = ConversionHolding::Asset;
        request(&service, conversion).await;
        let asset = service.list_inventory().await.unwrap().remove(0);
        service
            .create_equipment_listing(&asset.id, 220.0, None, 0.0, Some("2026-06-03"), None)
            .await
            .unwrap()
            .unwrap();

        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        let entry = history.iter().find(|row| row.kind == "conversion").unwrap();
        assert!(!entry.can_delete);
        assert!(service.undo_stock_conversion(&entry.id).await.is_err());
    }

    /// Removing a converted asset leaves the list, not the books.
    #[tokio::test]
    async fn removing_a_converted_asset_keeps_its_record() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, ARIS, "Hyperion Daily Voucher", 80, 0.0).await;
        let mut conversion = hand_in();
        conversion.outputs[0].holding = ConversionHolding::Asset;
        request(&service, conversion).await;
        let asset = service.list_inventory().await.unwrap().remove(0);

        assert!(service.delete_inventory_item(&asset.id).await.unwrap());

        assert!(service.list_inventory().await.unwrap().is_empty());
        assert!(ledger(&service).await.is_empty(), "removal moves no money");
        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        assert!(
            !history[0].can_delete,
            "there is no longer an asset to unmake"
        );
    }

    /// Going over what is held still records the conversion. The excess has
    /// no activity behind it, so it takes its share of the credit with it.
    #[tokio::test]
    async fn quantity_beyond_tracked_stock_is_real_but_unclaimed() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 50, 0.0).await;

        request(
            &service,
            ItemConversion {
                inputs: vec![held("Halloween Strongbox", 100.0)],
                outputs: vec![],
                ped_amount: Some(100.0),
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        assert_eq!(ledger(&service).await[0].2, 100.0);
        assert!((credited(&service, MAYHEM).await - 50.0).abs() < 1e-9);
    }

    /// Bought-only conversions produce real stock that no activity can claim.
    #[tokio::test]
    async fn a_conversion_of_only_bought_inputs_credits_no_activity() {
        let (_dir, service) = write_service().await;
        request(
            &service,
            ItemConversion {
                inputs: vec![bought("Component X", 10.0, 25.0, 5.0)],
                outputs: vec![output("Trophy", 25.0, ConversionHolding::Loot)],
                ped_amount: None,
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        list_and_sell(&service, "Trophy", 100.0, 0.0).await;

        assert_eq!(ledger(&service).await[0].2, 70.0);
        assert!(service
            .realised_markup_by_definition()
            .await
            .unwrap()
            .is_empty());
    }

    /// A session excluded from the economy reads as never recorded: what it
    /// supplied keeps its place in the split but no activity is credited.
    #[tokio::test]
    async fn a_conversion_drawn_from_an_excluded_session_credits_no_activity() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 100, 0.0).await;
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO session_contexts(id, session_id, created_at) \
                     VALUES(77, 's17', 1000.0)",
                    [],
                )?;
                conn.execute("UPDATE kills SET context_id = 77", [])?;
                conn.execute(
                    "UPDATE tracking_sessions SET economy_excluded_at = 2000.0 WHERE id = 's17'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        request(
            &service,
            ItemConversion {
                inputs: vec![held("Halloween Strongbox", 100.0)],
                outputs: vec![],
                ped_amount: Some(100.0),
                converted_at: Some("2026-06-02".to_string()),
            },
        )
        .await;

        assert_eq!(
            ledger(&service).await[0].2,
            100.0,
            "the money is still real"
        );
        assert!(credited(&service, MAYHEM).await.abs() < 1e-9);
    }

    #[tokio::test]
    async fn a_conversion_needs_something_in_and_something_out() {
        let (_dir, service) = write_service().await;
        let empty = service
            .convert_items(Profession::Inventory, ItemConversion::default())
            .await;
        assert!(empty.is_err());
        let no_result = service
            .convert_items(
                Profession::Inventory,
                ItemConversion {
                    inputs: vec![held("Halloween Strongbox", 1.0)],
                    ..ItemConversion::default()
                },
            )
            .await;
        assert!(no_result.is_err());
    }
}
