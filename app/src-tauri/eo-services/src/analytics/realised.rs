//! Who earned a realised figure.
//!
//! One read answers it for every surface: each confirmed stock outcome is split
//! across the sources that funded it, and the reports below only group those
//! parts. A confirmed sale reads its split from its movement rows; a conversion
//! that recognised value on the day wrote its split down when it happened. A
//! ledger entry the player pegged to a session is shared across that session.

use std::collections::{hash_map::Entry, BTreeMap, HashMap};

use crate::harvest_yield::HarvestYieldTier;
use crate::session_scope::{self, StatsScope};
use crate::stock_allocation::{self, CreditLine};

use super::{
    AnalyticsError, AnalyticsService, RealisedDefinitionMarkup, RealisedDerivedMarkup,
    RealisedSpeciesMarkup, RealisedTierMarkup, STOCK_EPSILON,
};

struct RealisedStockOutcome {
    id: String,
    movement_kind: String,
    /// Set when the sale was of a whole asset, whose origin rows are read by
    /// the asset rather than by the sale.
    asset_item_id: Option<String>,
    outcome: stock_allocation::SaleOutcome,
}

/// Every stock outcome that has crossed a recognition boundary. Auction sales,
/// private trades, and older Shrapnel conversions differ operationally, but
/// their activity attribution is the same calculation over the immutable
/// source movements that funded them. Conversions recorded since itemised
/// recording carry their split in their own credit rows instead.
fn realised_stock_outcomes(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<RealisedStockOutcome>> {
    let mut stmt = conn.prepare(
        "SELECT id, 'listing', quantity, attributed_qty, tt_value, COALESCE(final_price, 0), \
                listing_fee, COALESCE(sale_fee, 0), COALESCE(cost_basis, tt_value) - tt_value, \
                inventory_item_id \
         FROM auction_listings \
         WHERE status = 'sold' AND undone_at IS NULL \
           AND (subject_kind = 'loot' OR attributed_qty > 0) \
         UNION ALL \
         SELECT id, 'trade', quantity, attributed_qty, tt_value, final_price, 0, 0, paid_markup, \
                NULL \
         FROM private_sales WHERE undone_at IS NULL \
         UNION ALL \
         SELECT id, 'conversion_out', quantity, \
                CASE WHEN tt_value > 0 \
                     THEN quantity * COALESCE(attributed_tt, tt_value) / tt_value \
                     ELSE 0 END, \
                tt_value, COALESCE(output_tt_value, tt_value), 0, 0, 0, NULL \
         FROM stock_conversions \
         WHERE undone_at IS NULL AND gain_entry_id IS NOT NULL \
           AND recognised_markup IS NULL",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RealisedStockOutcome {
                id: row.get(0)?,
                movement_kind: row.get(1)?,
                asset_item_id: row.get(9)?,
                outcome: stock_allocation::resolve_sale(
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(8)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ),
            })
        })?
        .collect();
    rows
}

/// One source's part of a realised figure.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RealisedContribution {
    pub(super) source_kind: String,
    pub(super) yield_tier: Option<String>,
    pub(super) mob_species: Option<String>,
    pub(super) session_definition_id: Option<i64>,
    pub(super) activity_context_id: Option<i64>,
    pub(super) quest_id: Option<i64>,
    /// Drawn from a session excluded from the economy: it keeps its place in
    /// the split, but no activity report may claim it.
    pub(super) excluded: bool,
    pub(super) origin_item: Option<String>,
    pub(super) origin_quantity: f64,
    pub(super) amount: f64,
}

struct FundingRow {
    source_kind: String,
    yield_tier: Option<String>,
    mob_species: Option<String>,
    session_definition_id: Option<i64>,
    activity_context_id: Option<i64>,
    quest_id: Option<i64>,
    quantity: f64,
    declared_markup: Option<f64>,
    origin_item: Option<String>,
    origin_quantity: Option<f64>,
    excluded: bool,
}

const FUNDING_COLUMNS: &str =
    "source_kind, yield_tier, mob_species, session_definition_id, activity_context_id, quest_id, \
     quantity, declared_markup, origin_item, origin_quantity";

fn funding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FundingRow> {
    Ok(FundingRow {
        source_kind: row.get(0)?,
        yield_tier: row.get(1)?,
        mob_species: row.get(2)?,
        session_definition_id: row.get(3)?,
        activity_context_id: row.get(4)?,
        quest_id: row.get(5)?,
        quantity: row.get(6)?,
        declared_markup: row.get(7)?,
        origin_item: row.get(8)?,
        origin_quantity: row.get(9)?,
        excluded: row.get(10)?,
    })
}

/// Every source's part of every realised stock outcome.
pub(super) fn realised_contributions(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<RealisedContribution>> {
    let excluded =
        session_scope::excluded(StatsScope::Economy, session_scope::STOCK_MOVEMENT_SESSION);
    let mut by_ref = conn.prepare(&format!(
        "SELECT {FUNDING_COLUMNS}, {excluded} AS excluded FROM stock_movements \
         WHERE ref_id = ? AND movement_kind = ? \
           AND source_kind IN ('hunt', 'harvest', 'quest')"
    ))?;
    let mut by_asset = conn.prepare(&format!(
        "SELECT {FUNDING_COLUMNS}, {excluded} AS excluded FROM stock_movements \
         WHERE inventory_item_id = ? AND movement_kind = 'conversion_in' \
           AND source_kind IN ('hunt', 'harvest', 'quest')"
    ))?;

    let mut parts = Vec::new();
    for realised in realised_stock_outcomes(conn)? {
        if realised.outcome.activity_net_markup.abs() <= STOCK_EPSILON {
            continue;
        }
        // The share is taken across every provenance dimension, so a sale
        // drawing on boards and hides credits each side only what it supplied.
        let funding: Vec<FundingRow> = match &realised.asset_item_id {
            Some(item_id) => by_asset
                .query_map(rusqlite::params![item_id], funding_row)?
                .collect::<rusqlite::Result<_>>()?,
            None => by_ref
                .query_map(
                    rusqlite::params![realised.id, realised.movement_kind],
                    funding_row,
                )?
                .collect::<rusqlite::Result<_>>()?,
        };
        let lines: Vec<CreditLine> = funding
            .iter()
            .map(|row| CreditLine {
                weight: row.quantity.abs(),
                worth: row.declared_markup.map(f64::abs),
            })
            .collect();
        if lines.iter().map(|line| line.weight).sum::<f64>() <= STOCK_EPSILON {
            continue;
        }
        let split = stock_allocation::split_realised(realised.outcome.activity_net_markup, &lines);
        for (row, amount) in funding.into_iter().zip(split) {
            parts.push(RealisedContribution {
                source_kind: row.source_kind,
                yield_tier: row.yield_tier,
                mob_species: row.mob_species,
                session_definition_id: row.session_definition_id,
                activity_context_id: row.activity_context_id,
                quest_id: row.quest_id,
                excluded: row.excluded,
                origin_item: row.origin_item,
                origin_quantity: row.origin_quantity.map_or(0.0, f64::abs),
                amount,
            });
        }
    }

    let credit_excluded = session_scope::excluded(
        StatsScope::Economy,
        "(SELECT sc.session_id FROM session_contexts sc \
          WHERE sc.id = stock_conversion_credits.activity_context_id)",
    );
    let mut credits = conn.prepare(&format!(
        "SELECT source_kind, yield_tier, mob_species, session_definition_id, \
                activity_context_id, quest_id, origin_item, origin_quantity, amount, \
                {credit_excluded} AS excluded \
         FROM stock_conversion_credits"
    ))?;
    let rows = credits.query_map([], |row| {
        Ok(RealisedContribution {
            source_kind: row.get(0)?,
            yield_tier: row.get(1)?,
            mob_species: row.get(2)?,
            session_definition_id: row.get(3)?,
            activity_context_id: row.get(4)?,
            quest_id: row.get(5)?,
            origin_item: row.get(6)?,
            origin_quantity: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
            amount: row.get(8)?,
            excluded: row.get(9)?,
        })
    })?;
    for row in rows {
        parts.push(row?);
    }
    parts.extend(session_ledger_contributions(conn)?);
    Ok(parts)
}

/// One part of a session a pegged ledger entry can land on.
struct SessionShare {
    source_kind: &'static str,
    yield_tier: Option<String>,
    activity_context_id: Option<i64>,
    cycled: f64,
}

/// What a session recorded, part by part, with what each part cycled. Excluded
/// recordings read as never recorded, so they take no share of the spread.
fn session_shares(
    conn: &rusqlite::Connection,
    definition_id: i64,
) -> rusqlite::Result<Vec<SessionShare>> {
    let economy = session_scope::counts_on(StatsScope::Economy, "s");
    let mut hunted: BTreeMap<Option<i64>, f64> = BTreeMap::new();
    // Settled recordings read their projection; only the rest touch raw kills.
    for sql in [
        format!(
            "SELECT r.context_id, COALESCE(SUM(r.cycled_ped), 0) \
             FROM session_kill_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?2 \
             JOIN tracking_sessions s ON s.id = r.session_id \
             WHERE s.definition_id = ?1 AND {economy} \
             GROUP BY r.context_id"
        ),
        format!(
            "SELECT k.context_id, COALESCE(SUM(k.cost_ped + k.enhancer_cost), 0) \
             FROM tracking_sessions s \
             JOIN kills k ON k.session_id = s.id \
             LEFT JOIN session_rollup_meta m ON m.session_id = s.id \
                  AND m.rollup_version >= ?2 \
             WHERE s.definition_id = ?1 AND {economy} AND m.session_id IS NULL \
             GROUP BY k.context_id"
        ),
    ] {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![definition_id, crate::session_rollup::ROLLUP_VERSION],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, f64>(1)?)),
        )?;
        for row in rows {
            let (context, cycled) = row?;
            *hunted.entry(context).or_default() += cycled;
        }
    }
    let mut shares: Vec<SessionShare> = hunted
        .into_iter()
        .map(|(context, cycled)| SessionShare {
            source_kind: "hunt",
            yield_tier: None,
            activity_context_id: context,
            cycled,
        })
        .collect();

    let mut swung = conn.prepare(&format!(
        "SELECT h.context_id, h.yield_tier, COALESCE(SUM(h.cost_ped), 0) \
         FROM tracking_sessions s \
         JOIN harvest_events h ON h.session_id = s.id \
         WHERE s.definition_id = ?1 AND {economy} \
         GROUP BY h.context_id, h.yield_tier"
    ))?;
    let rows = swung.query_map(rusqlite::params![definition_id], |row| {
        Ok(SessionShare {
            source_kind: "harvest",
            yield_tier: row.get(1)?,
            activity_context_id: row.get(0)?,
            cycled: row.get(2)?,
        })
    })?;
    for row in rows {
        shares.push(row?);
    }
    shares.retain(|share| share.cycled > STOCK_EPSILON);
    Ok(shares)
}

/// The ledger entries the player pegged to a session, each shared across what
/// that session recorded by what each part cycled. Every part's realised rate
/// therefore moves by the same points, so a session-wide cost cannot reorder
/// the session's own activities. The shares are worked out here rather than
/// stored, so they follow a recording that is later re-filed or excluded.
fn session_ledger_contributions(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<RealisedContribution>> {
    let mut stmt = conn.prepare(
        "SELECT session_definition_id, type, amount FROM ledger_entries \
         WHERE session_definition_id IS NOT NULL",
    )?;
    let entries = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut shares_by_definition: HashMap<i64, Vec<SessionShare>> = HashMap::new();
    let mut parts = Vec::new();
    for (definition_id, kind, amount) in entries {
        let signed = if kind == "markup" { amount } else { -amount };
        let shares = match shares_by_definition.entry(definition_id) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(slot) => slot.insert(session_shares(conn, definition_id)?),
        };
        let part = |source_kind: &str,
                    yield_tier: Option<String>,
                    activity_context_id: Option<i64>,
                    amount: f64| RealisedContribution {
            source_kind: source_kind.to_string(),
            yield_tier,
            mob_species: None,
            // As with stock, the session axis is Hunting's: swung output is
            // read by its yield tier instead.
            session_definition_id: (source_kind == "hunt").then_some(definition_id),
            activity_context_id,
            quest_id: None,
            excluded: false,
            origin_item: None,
            origin_quantity: 0.0,
            amount,
        };
        if shares.is_empty() {
            // Nothing recorded to share it across: it stays on the session.
            parts.push(part("hunt", None, None, signed));
            continue;
        }
        let lines: Vec<CreditLine> = shares
            .iter()
            .map(|share| CreditLine {
                weight: share.cycled,
                worth: None,
            })
            .collect();
        let split = stock_allocation::split_realised(signed, &lines);
        for (share, amount) in shares.iter().zip(split) {
            parts.push(part(
                share.source_kind,
                share.yield_tier.clone(),
                share.activity_context_id,
                amount,
            ));
        }
    }
    Ok(parts)
}

/// Net realised markup per quest, for the per-quest analytics. Quest rewards
/// sit deliberately outside session exclusion, so nothing is filtered here.
pub(crate) fn realised_markup_by_quest(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<HashMap<i64, f64>> {
    let mut totals = HashMap::new();
    for credit in realised_contributions(conn)? {
        if credit.source_kind != "quest" {
            continue;
        }
        if let Some(quest_id) = credit.quest_id {
            *totals.entry(quest_id).or_insert(0.0) += credit.amount;
        }
    }
    Ok(totals)
}

impl AnalyticsService {
    /// Net realised markup per yield tier, from confirmed stock outcomes. A
    /// share drawn from an excluded session is credited to no tier.
    pub async fn realised_markup_by_tier(&self) -> Result<Vec<RealisedTierMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut totals: BTreeMap<String, f64> = BTreeMap::new();
                for credit in realised_contributions(conn)? {
                    if credit.excluded {
                        continue;
                    }
                    if let Some(tier) = credit.yield_tier {
                        *totals.entry(tier).or_insert(0.0) += credit.amount;
                    }
                }
                let mut rows: Vec<RealisedTierMarkup> = totals
                    .into_iter()
                    .map(|(tier, net_markup)| RealisedTierMarkup {
                        yield_tier: HarvestYieldTier::from_db(&tier),
                        net_markup,
                    })
                    .collect();
                rows.sort_by_key(|row| row.yield_tier.sort_rank());
                Ok(rows)
            })
            .await?)
    }

    /// Net realised markup per mob species: the Hunting sibling of
    /// [`Self::realised_markup_by_tier`], so a joint-provenance sale credits
    /// tiers and species each their own share without double counting.
    pub async fn realised_markup_by_species(
        &self,
    ) -> Result<Vec<RealisedSpeciesMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut totals: BTreeMap<String, f64> = BTreeMap::new();
                for credit in realised_contributions(conn)? {
                    if credit.excluded {
                        continue;
                    }
                    if credit.source_kind == "hunt" || credit.mob_species.is_some() {
                        *totals
                            .entry(credit.mob_species.unwrap_or_default())
                            .or_insert(0.0) += credit.amount;
                    }
                }
                Ok(totals
                    .into_iter()
                    .map(|(mob_species, net_markup)| RealisedSpeciesMarkup {
                        mob_species,
                        net_markup,
                    })
                    .collect())
            })
            .await?)
    }

    /// Net realised markup per Hunting session definition. Movements without
    /// a definition context remain unclaimed here while retaining their
    /// Overall economic truth. Optional observed-species evidence never gates
    /// this deliberate activity axis.
    pub async fn realised_markup_by_definition(
        &self,
    ) -> Result<Vec<RealisedDefinitionMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut totals: BTreeMap<i64, f64> = BTreeMap::new();
                for credit in realised_contributions(conn)? {
                    if credit.excluded {
                        continue;
                    }
                    if let Some(definition_id) = credit.session_definition_id {
                        *totals.entry(definition_id).or_insert(0.0) += credit.amount;
                    }
                }
                Ok(totals
                    .into_iter()
                    .map(|(definition_id, net_markup)| RealisedDefinitionMarkup {
                        definition_id,
                        net_markup,
                    })
                    .collect())
            })
            .await?)
    }

    /// The part of each definition's realised markup that came through a
    /// conversion, per root item.
    pub async fn realised_derived_by_definition(
        &self,
    ) -> Result<Vec<RealisedDerivedMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut totals: BTreeMap<(i64, String), (f64, f64)> = BTreeMap::new();
                for credit in realised_contributions(conn)? {
                    if credit.excluded {
                        continue;
                    }
                    let (Some(definition_id), Some(origin_item)) =
                        (credit.session_definition_id, credit.origin_item)
                    else {
                        continue;
                    };
                    let total = totals
                        .entry((definition_id, origin_item))
                        .or_insert((0.0, 0.0));
                    total.0 += credit.origin_quantity;
                    total.1 += credit.amount;
                }
                Ok(totals
                    .into_iter()
                    .map(|((definition_id, origin_item), (quantity, net_markup))| {
                        RealisedDerivedMarkup {
                            definition_id,
                            origin_item,
                            quantity,
                            net_markup,
                        }
                    })
                    .collect())
            })
            .await?)
    }
}
