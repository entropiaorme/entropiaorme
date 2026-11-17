//! Who earned a realised figure.
//!
//! One read answers it for every surface: each confirmed stock outcome is split
//! across the sources that funded it, and the reports below only group those
//! parts. A confirmed sale reads its split from its movement rows; a conversion
//! that recognised value on the day wrote its split down when it happened. A
//! ledger entry the player pegged to a session is shared across that session.
//! A session's overhead (what it cost beyond performing its activity) is no
//! realised figure: it is a cost, shared across the session's own parts by the
//! same split and read beside the realised markup ([`overhead_parts`]).

use std::collections::{hash_map::Entry, BTreeMap, HashMap};

use crate::harvest_yield::HarvestYieldTier;
use crate::session_scope::{self, StatsScope};
use crate::stock_allocation::{self, CreditLine};

use super::{
    ActivityOverhead, AnalyticsError, AnalyticsService, RealisedDefinitionMarkup,
    RealisedDerivedMarkup, RealisedSpeciesMarkup, RealisedTierMarkup, STOCK_EPSILON,
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

/// One part of a session a spread can land on.
struct SessionShare {
    source_kind: &'static str,
    yield_tier: Option<String>,
    activity_context_id: Option<i64>,
    cycled: f64,
}

/// The recordings a spread is shared across: every session of a definition
/// (a pegged ledger entry), or every session with overhead (each its own).
#[derive(Debug, Clone, Copy)]
enum ShareScope {
    Definition(i64),
    WithOverhead,
}

impl ShareScope {
    /// The SQL condition on `tracking_sessions` aliased `s` that holds the
    /// scope's recordings, bound to `?1`.
    fn condition(self) -> String {
        match self {
            ShareScope::Definition(_) => "s.definition_id = ?1".to_string(),
            ShareScope::WithOverhead => format!(
                "?1 IS NULL AND {} > 0",
                crate::session_costs::overhead_total_sql("s.id")
            ),
        }
    }

    fn bind(self) -> rusqlite::types::Value {
        match self {
            ShareScope::Definition(id) => rusqlite::types::Value::Integer(id),
            ShareScope::WithOverhead => rusqlite::types::Value::Null,
        }
    }
}

/// What each of the scope's recordings recorded, part by part, with what
/// each part cycled, by session: the one rule that weighs a spread across a
/// session's parts, read in one pass however many sessions the scope holds.
/// Excluded recordings read as never recorded, so they take no share of the
/// spread.
fn shares_by_session(
    conn: &rusqlite::Connection,
    scope: ShareScope,
) -> rusqlite::Result<BTreeMap<String, Vec<SessionShare>>> {
    let economy = session_scope::counts_on(StatsScope::Economy, "s");
    let within = scope.condition();
    let mut hunted: BTreeMap<(String, Option<i64>), f64> = BTreeMap::new();
    // Settled recordings read their projection; only the rest touch raw kills.
    // every-kill: spend (sums of spend and loot; a claimed arrival's loot is zeroed).
    for sql in [
        format!(
            "SELECT s.id, r.context_id, COALESCE(SUM(r.cycled_ped), 0) \
             FROM session_kill_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?2 \
             JOIN tracking_sessions s ON s.id = r.session_id \
             WHERE {within} AND {economy} \
             GROUP BY s.id, r.context_id"
        ),
        format!(
            "SELECT s.id, k.context_id, COALESCE(SUM({}), 0) \
             FROM tracking_sessions s \
             JOIN kills k ON k.session_id = s.id \
             LEFT JOIN session_rollup_meta m ON m.session_id = s.id \
                  AND m.rollup_version >= ?2 \
             WHERE {within} AND {economy} AND m.session_id IS NULL \
             GROUP BY s.id, k.context_id",
            crate::session_costs::kill_cycled_sql("k")
        ),
    ] {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![scope.bind(), crate::session_rollup::ROLLUP_VERSION],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            },
        )?;
        for row in rows {
            let (session, context, cycled) = row?;
            *hunted.entry((session, context)).or_default() += cycled;
        }
    }
    let mut shares: BTreeMap<String, Vec<SessionShare>> = BTreeMap::new();
    for ((session, context), cycled) in hunted {
        shares.entry(session).or_default().push(SessionShare {
            source_kind: "hunt",
            yield_tier: None,
            activity_context_id: context,
            cycled,
        });
    }

    let mut swung = conn.prepare(&format!(
        "SELECT s.id, h.context_id, h.yield_tier, COALESCE(SUM({}), 0) \
         FROM tracking_sessions s \
         JOIN harvest_events h ON h.session_id = s.id \
         WHERE {within} AND {economy} \
         GROUP BY s.id, h.context_id, h.yield_tier",
        crate::session_costs::scaled(
            "h.cost_ped",
            crate::session_costs::Bucket::Harvesting,
            "h.session_id"
        )
    ))?;
    let rows = swung.query_map(rusqlite::params![scope.bind()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            SessionShare {
                source_kind: "harvest",
                yield_tier: row.get(2)?,
                activity_context_id: row.get(1)?,
                cycled: row.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (session, share) = row?;
        shares.entry(session).or_default().push(share);
    }
    for parts in shares.values_mut() {
        parts.retain(|share| share.cycled > STOCK_EPSILON);
    }
    Ok(shares)
}

/// `amount` shared across `shares` by what each part cycled, so every part's
/// realised rate moves by the same points: the one split both a pegged
/// ledger entry and a session's overhead take.
fn spread(shares: &[SessionShare], amount: f64) -> impl Iterator<Item = (&SessionShare, f64)> + '_ {
    let lines: Vec<CreditLine> = shares
        .iter()
        .map(|share| CreditLine {
            weight: share.cycled,
            worth: None,
        })
        .collect();
    let split = stock_allocation::split_realised(amount, &lines);
    shares.iter().zip(split)
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
            Entry::Vacant(slot) => slot.insert(
                shares_by_session(conn, ShareScope::Definition(definition_id))?
                    .into_values()
                    .flatten()
                    .collect(),
            ),
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
        for (share, amount) in spread(shares, signed) {
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

/// One part of a session's overhead: what the session cost beyond performing
/// its activity, landed on one of its parts.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OverheadPart {
    /// The session whose overhead it is.
    pub(super) session_id: String,
    /// `hunt` or `harvest`.
    pub(super) source_kind: &'static str,
    pub(super) yield_tier: Option<String>,
    /// The session's definition, for a hunting part.
    pub(super) session_definition_id: Option<i64>,
    pub(super) activity_context_id: Option<i64>,
    /// PED: a cost, never realised markup.
    pub(super) amount: f64,
}

/// Each session's overhead (what it cost beyond performing its activity, as
/// the readings measured it) shared across the session's own parts by what
/// each cycled, by the split a pegged ledger entry takes: so every part's
/// Realised Net and Realised Rate carry it as a cost and its Cycled never
/// does, and it cannot reorder the session's own activities. It is a session
/// cost, never realised markup and never a ledger entry: it is taken at the
/// session's grain, and a session excluded from the economy takes it out of
/// the statistics with it. A session with no part that cycled anything
/// keeps it whole: on its yield tiers' side when it swung at all (with no
/// tier), otherwise on its own definition. Read in three passes whatever
/// the number of sessions.
pub(super) fn overhead_parts(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<OverheadPart>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.id, s.definition_id, {overhead} AS overhead, \
                EXISTS (SELECT 1 FROM harvest_events h WHERE h.session_id = s.id) \
         FROM tracking_sessions s \
         WHERE {economy} AND {overhead} > 0 ORDER BY s.id",
        overhead = crate::session_costs::overhead_total_sql("s.id"),
        economy = session_scope::counts_on(StatsScope::Economy, "s"),
    ))?;
    let sessions = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let mut shares = shares_by_session(conn, ShareScope::WithOverhead)?;
    let mut parts = Vec::new();
    for (session_id, definition_id, overhead, swung) in sessions {
        let part = |source_kind: &'static str,
                    yield_tier: Option<String>,
                    activity_context_id: Option<i64>,
                    amount: f64| OverheadPart {
            session_id: session_id.clone(),
            source_kind,
            yield_tier,
            session_definition_id: definition_id.filter(|_| source_kind == "hunt"),
            activity_context_id,
            amount,
        };
        let session_shares = shares.remove(&session_id).unwrap_or_default();
        if session_shares.is_empty() {
            let side = if swung { "harvest" } else { "hunt" };
            parts.push(part(side, None, None, overhead));
            continue;
        }
        for (share, amount) in spread(&session_shares, overhead) {
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

    /// The sessions' overhead as Activity reads it: per yield tier and per
    /// Hunting session definition, shared over each session's parts by what
    /// each cycled ([`overhead_parts`]). A cost beside the realised markup:
    /// Realised Net takes it off, and the Realised Rate's denominator is
    /// Cycled and it. Excluded sessions take none.
    pub async fn activity_overhead(&self) -> Result<ActivityOverhead, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut overhead = ActivityOverhead::default();
                for part in overhead_parts(conn)? {
                    if let Some(tier) = &part.yield_tier {
                        *overhead.tiers.entry(tier.clone()).or_insert(0.0) += part.amount;
                    }
                    if let Some(definition_id) = part.session_definition_id {
                        *overhead.definitions.entry(definition_id).or_insert(0.0) += part.amount;
                    }
                }
                Ok(overhead)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An ended session under definition 7 with one kill in each of two
    /// contexts, its cells settled as a session end settles them.
    fn hunt(conn: &rusqlite::Connection, id: &str, weapon: [f64; 2]) -> [i64; 2] {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, definition_id) \
             VALUES (?1, 1000.0, 4600.0, 0, 7)",
            [id],
        )
        .unwrap();
        let mut contexts = [0; 2];
        for (index, cost) in weapon.iter().enumerate() {
            conn.execute(
                "INSERT INTO session_contexts (session_id, created_at) VALUES (?1, 1000.0)",
                [id],
            )
            .unwrap();
            contexts[index] = conn.last_insert_rowid();
            let kill = format!("{id}-k{index}");
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, \
                 enhancer_cost, loot_total_ped, context_id) \
                 VALUES (?1, ?2, 'Atrox', 1500.0, ?3, 0, 1.0, ?4)",
                rusqlite::params![kill, id, cost, contexts[index]],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
                 VALUES (?1, 'Rifle', 1, ?2)",
                rusqlite::params![kill, cost],
            )
            .unwrap();
        }
        contexts
    }

    fn settle(conn: &rusqlite::Connection, id: &str) {
        crate::session_costs::refresh_derived(conn, id).unwrap();
    }

    /// What a pegged entry of 8.00 lands on, by context.
    fn landed(conn: &rusqlite::Connection) -> HashMap<i64, f64> {
        conn.execute(
            "INSERT INTO ledger_entries \
             (id, date, type, description, amount, tag, session_definition_id) \
             VALUES ('fee', '1970-01-01', 'expense', 'Fee', 8.0, 'fee', 7)",
            [],
        )
        .unwrap();
        session_ledger_contributions(conn)
            .unwrap()
            .into_iter()
            .map(|part| (part.activity_context_id.unwrap(), part.amount))
            .collect()
    }

    /// A session's overhead is shared over its own hunting contexts and its
    /// yield tiers by what each cycled, the parts add back up to it, it is
    /// never realised markup, and a session excluded from the economy takes
    /// none.
    #[test]
    fn a_session_s_overhead_is_shared_by_what_each_part_cycled() {
        let conn = crate::db::migrated_in_memory().unwrap();
        let [a, b] = hunt(&conn, "s1", [1.0, 3.0]);
        conn.execute(
            "INSERT INTO session_contexts (session_id, created_at) VALUES ('s1', 1000.0)",
            [],
        )
        .unwrap();
        let swung = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO harvest_events (id, session_id, timestamp, success, tool_name, \
             yield_tier, cost_ped, loot_total_ped, context_id) \
             VALUES ('h1', 's1', 2000.0, 1, 'Cutter', 'short', 4.0, 0.1, ?1)",
            [swung],
        )
        .unwrap();
        // Another session's overhead is its own; a third has none.
        let [own, _] = hunt(&conn, "s2", [2.0, 0.0]);
        hunt(&conn, "s3", [5.0, 5.0]);
        crate::session_costs_tests::store_measured(
            &conn,
            "s1",
            &[("travel", 0.8), ("unclassified", 0.4)],
        );
        crate::session_costs_tests::store_measured(&conn, "s2", &[("fieldLogistics", 0.5)]);
        for id in ["s1", "s2", "s3"] {
            settle(&conn, id);
        }

        let parts = overhead_parts(&conn).unwrap();
        let of = |context: i64| -> &OverheadPart {
            parts
                .iter()
                .find(|part| part.activity_context_id == Some(context))
                .unwrap_or_else(|| panic!("a part for context {context}: {parts:?}"))
        };
        // 1.20 over 1.00, 3.00 and 4.00 cycled.
        assert!((of(a).amount - 0.15).abs() < 1e-9, "{parts:?}");
        assert!((of(b).amount - 0.45).abs() < 1e-9, "{parts:?}");
        let tier = of(swung);
        assert!((tier.amount - 0.6).abs() < 1e-9, "{parts:?}");
        assert_eq!(tier.source_kind, "harvest");
        assert_eq!(tier.yield_tier.as_deref(), Some("short"));
        assert_eq!(tier.session_definition_id, None);
        assert_eq!(of(a).session_definition_id, Some(7));
        assert!((of(own).amount - 0.5).abs() < 1e-9, "{parts:?}");
        let s1: f64 = [a, b, swung]
            .iter()
            .map(|context| of(*context).amount)
            .sum();
        assert!((s1 - 1.2).abs() < 1e-9, "the parts add up to the overhead");
        assert_eq!(parts.len(), 4, "a session without overhead has no part");
        // It is a cost, never realised markup.
        assert!(realised_contributions(&conn).unwrap().is_empty());

        // Out of the statistics, its overhead leaves with it.
        conn.execute(
            "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 's1'",
            [],
        )
        .unwrap();
        settle(&conn, "s1");
        let parts = overhead_parts(&conn).unwrap();
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert_eq!(parts[0].activity_context_id, Some(own));
    }

    /// Activity reads the overhead by yield tier and by definition, beside
    /// the realised markup and never in it.
    #[tokio::test]
    async fn activity_reads_overhead_beside_the_realised_markup() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("overhead.db"))
            .await
            .unwrap();
        db.with_writer(|conn| {
            let [_, _] = hunt(conn, "s1", [1.0, 3.0]);
            conn.execute(
                "INSERT INTO harvest_events (id, session_id, timestamp, success, tool_name, \
                 yield_tier, cost_ped, loot_total_ped) \
                 VALUES ('h1', 's1', 2000.0, 1, 'Cutter', 'short', 4.0, 0.1)",
                [],
            )?;
            crate::session_costs_tests::store_measured(conn, "s1", &[("travel", 0.8)]);
            settle(conn, "s1");
            Ok(())
        })
        .await
        .unwrap();
        let clock = std::sync::Arc::new(crate::clock::MockClock::new(None, 0.0));
        let service = AnalyticsService::new(db.clone(), clock);
        let overhead = service.activity_overhead().await.unwrap();
        assert!(
            (overhead.definitions[&7] - 0.4).abs() < 1e-9,
            "{overhead:?}"
        );
        assert!((overhead.tiers["short"] - 0.4).abs() < 1e-9, "{overhead:?}");
        // Nothing sold: no realised markup anywhere.
        assert!(service
            .realised_markup_by_definition()
            .await
            .unwrap()
            .is_empty());
        assert!(service.realised_markup_by_tier().await.unwrap().is_empty());
    }

    /// A session's parts keep their shares of a pegged entry when its weapon
    /// is measured (one scale weighs every kill of it), while across
    /// sessions a measured one weighs at its measured spend.
    #[test]
    fn a_pegged_entry_is_shared_by_what_each_part_cost_as_resolved() {
        let conn = crate::db::migrated_in_memory().unwrap();
        let [a, b] = hunt(&conn, "s1", [1.0, 3.0]);
        crate::session_costs_tests::store_measured(&conn, "s1", &[("weapon", 8.0)]);
        settle(&conn, "s1");
        let shares = landed(&conn);
        assert!((shares[&a] + 2.0).abs() < 1e-9, "{shares:?}");
        assert!((shares[&b] + 6.0).abs() < 1e-9, "{shares:?}");

        let conn = crate::db::migrated_in_memory().unwrap();
        let [measured, _] = hunt(&conn, "s1", [1.0, 0.0]);
        let [tracked, _] = hunt(&conn, "s2", [1.0, 0.0]);
        // s1's 1.00 tracked was 3.00 measured; s2's stays 1.00.
        crate::session_costs_tests::store_measured(&conn, "s1", &[("weapon", 3.0)]);
        settle(&conn, "s1");
        settle(&conn, "s2");
        let shares = landed(&conn);
        assert!((shares[&measured] + 6.0).abs() < 1e-9, "{shares:?}");
        assert!((shares[&tracked] + 2.0).abs() < 1e-9, "{shares:?}");
    }
}
