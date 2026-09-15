//! The market domain service: the manual markup-observation feed and
//! its reads.
//!
//! The user pastes the game's market-ledger export ([`crate::market_paste`]);
//! an accepted paste commits as one submission whose observations carry
//! the five aggregation horizons per item. Reads serve the overview
//! (each item's latest readings, with when they were observed) and a
//! per-item, per-horizon history.
//!
//! Everything here is the INFORMATIONAL market layer: estimated markup
//! never joins the ledger, the analytics aggregates, or any realised
//! P&L figure. Writes stay on the `market_*` tables; the one sanctioned
//! read of the accounting tables is `mob_ranking`'s loot-composition
//! join, the one-way direction of the market boundary, and nothing
//! flows back.

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::market_paste::{parse_market_paste, MarketHorizon, MarketPasteParse, MarketReading};
use crate::time::naive_to_epoch;

/// The market domain service over the shared database and injected
/// clock.
pub struct MarketService {
    db: Db,
    clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    /// The paste yielded no usable rows, so there is nothing to commit.
    #[error("the paste contained no readable market rows")]
    EmptyPaste,
    #[error("{0}")]
    InvalidInput(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// A committed paste: the submission it created and the timestamp its
/// observations carry.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitOutcome {
    pub submission_id: i64,
    pub item_count: usize,
    pub skipped_count: usize,
    /// Epoch seconds.
    pub observed_at: f64,
}

/// One overview row: an item's latest readings (all five horizons from
/// the most recent submission that carried the item) and when they were
/// observed.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewRow {
    pub item_name: String,
    pub tier: i64,
    /// Epoch seconds of the submission the readings came from.
    pub observed_at: f64,
    /// Indexed in [`MarketHorizon::ALL`] order.
    pub readings: [MarketReading; 5],
}

/// One history point of an item's horizon: the observation and when it
/// was submitted.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPoint {
    /// Epoch seconds.
    pub observed_at: f64,
    pub markup_pct: Option<f64>,
    pub sales_ped: f64,
}

/// One item of a contributable batch: the pasted readings verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchItem {
    pub item_name: String,
    pub tier: i64,
    /// Indexed in [`MarketHorizon::ALL`] order.
    pub readings: [MarketReading; 5],
}

/// The most recent accepted paste, verbatim: what a user who opts in
/// to contributing shares, nothing more. Sending it anywhere is the
/// caller's decision behind its own explicit consent gate.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmissionBatch {
    /// Epoch seconds of the submission.
    pub observed_at: f64,
    pub items: Vec<BatchItem>,
}

/// One species' estimated-markup row: the recorded loot composition
/// TT-weighted by the latest markup observations on one horizon.
/// `est_markup_pct` is None when no composing item has an observation;
/// the coverage fields keep a thin sample from masquerading as a
/// verdict (estimates are weighted over the covered TT only).
#[derive(Debug, Clone, PartialEq)]
pub struct MobRankingRow {
    pub mob_species: String,
    /// Total active recorded loot TT for the species (PED).
    pub loot_tt: f64,
    /// The share of that TT whose items have a markup observation (PED).
    pub covered_tt: f64,
    pub item_count: i64,
    pub covered_item_count: i64,
    /// TT-weighted average of the latest markup observations over the
    /// covered composition. Estimated markup: informational only.
    pub est_markup_pct: Option<f64>,
}

/// The estimated market signals for the harvest-looted items, plus the
/// nanocube recycling floor. Markup is item-intrinsic (independent of
/// which tool looted the item), so this is a flat per-item list; the
/// analytics side owns the per-tool composition, and the frontend merges
/// the two, derives holding-independent market opportunity, and computes
/// the current-market aggregates. Estimated markup is informational only,
/// never a realised figure.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestMarketData {
    /// The nanocube item's resolved markup (percent): the universal
    /// recycling floor for items that cannot be sold at their own
    /// markup (recycling is TT-neutral, so any item converts to
    /// nanocubes at full TT). None when no nanocube observation exists;
    /// the frontend then falls back to a constant.
    pub nanocube_markup_pct: Option<f64>,
    pub items: Vec<HarvestItemMarkup>,
}

/// One horizon's reading for an item: its markup (None where the game
/// reported N/A) and TT turnover (PED) for percentage-markup items.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestHorizonReading {
    /// "day" | "week" | "month" | "year".
    pub horizon: String,
    pub markup_pct: Option<f64>,
    pub sales_ped: f64,
}

/// One harvest-looted item's resolved market signals plus the per-horizon
/// breakdown. The resolved markup/horizon/sales are the display default
/// and market-opportunity input (week preferred, then month, then year;
/// all None when no observation covers the item). `readings` carries every
/// horizon (day, week, month, year) for the detail view, including a
/// zero-volume, no-markup week that a fallback would otherwise mask.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestItemMarkup {
    pub item_name: String,
    pub markup_pct: Option<f64>,
    /// Absolute informational quote for one item unit. Mutually independent
    /// from percentage-of-TT markup and never enters realised accounting.
    pub unit_price_ped: Option<f64>,
    /// The horizon that supplied the reading ("week" | "month" | "year"),
    /// None when uncovered.
    pub horizon: Option<String>,
    /// TT turnover (PED) at the resolved horizon: market-capacity evidence.
    pub sales_ped: Option<f64>,
    /// TT packet whose expected direct-market markup keeps the listing fee
    /// to at most ten per cent of gross markup. Independent of turnover.
    pub recommended_packet_tt: Option<f64>,
    /// Every horizon's reading, ordered day, week, month, year.
    pub readings: Vec<HarvestHorizonReading>,
}

impl MarketService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Parse a paste without touching the database (the preview leg of
    /// the review-before-accept flow).
    pub fn preview(&self, text: &str) -> MarketPasteParse {
        parse_market_paste(text)
    }

    pub async fn set_unit_price(
        &self,
        item_name: &str,
        ped_per_unit: f64,
    ) -> Result<f64, MarketError> {
        let item_name = item_name.trim();
        if item_name.is_empty() {
            return Err(MarketError::InvalidInput(
                "item_name must not be empty".to_string(),
            ));
        }
        if !ped_per_unit.is_finite() || ped_per_unit < 0.0 {
            return Err(MarketError::InvalidInput(
                "ped_per_unit must be a finite, non-negative value".to_string(),
            ));
        }
        let item_name = item_name.to_string();
        let observed_at = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |connection| {
                connection.execute(
                    "INSERT INTO market_unit_price_observations \
                     (item_name, ped_per_unit, observed_at, source) \
                     VALUES (?, ?, ?, 'manual')",
                    rusqlite::params![item_name, ped_per_unit, observed_at],
                )?;
                Ok(())
            })
            .await?;
        Ok(observed_at)
    }

    /// Parse and commit a paste as one submission. The text is
    /// re-parsed here rather than trusting a client echo of the
    /// preview, so the stored rows always come from this parser.
    pub async fn commit_paste(&self, text: &str) -> Result<CommitOutcome, MarketError> {
        let parse = parse_market_paste(text);
        if parse.rows.is_empty() {
            return Err(MarketError::EmptyPaste);
        }
        let observed_at = naive_to_epoch(self.clock.now());
        let item_count = parse.rows.len();
        let skipped_count = parse.skipped.len();
        let rows = parse.rows;
        let submission_id = self
            .db
            .with_writer(move |connection| {
                let tx = connection.transaction()?;
                tx.execute(
                    "INSERT INTO market_submissions (submitted_at, source, item_count) \
                     VALUES (?1, 'paste', ?2)",
                    rusqlite::params![observed_at, item_count as i64],
                )?;
                let submission_id = tx.last_insert_rowid();
                {
                    let mut insert = tx.prepare(
                        "INSERT INTO market_observations \
                         (submission_id, item_name, tier, horizon, markup_pct, sales_ped) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?;
                    for row in &rows {
                        for (horizon, reading) in MarketHorizon::ALL.iter().zip(&row.readings) {
                            insert.execute(rusqlite::params![
                                submission_id,
                                row.item_name,
                                row.tier,
                                horizon.as_str(),
                                reading.markup_pct,
                                reading.sales_ped,
                            ])?;
                        }
                    }
                }
                tx.commit()?;
                Ok(submission_id)
            })
            .await?;
        Ok(CommitOutcome {
            submission_id,
            item_count,
            skipped_count,
            observed_at,
        })
    }

    /// Every observed item's latest readings: the five horizons from
    /// the most recent submission that carried the item, sorted by item
    /// name.
    pub async fn overview(&self) -> Result<Vec<OverviewRow>, DbError> {
        self.db
            .with_reader(|connection| {
                let mut stmt = connection.prepare(
                    "SELECT o.item_name, o.tier, s.submitted_at, o.horizon, \
                            o.markup_pct, o.sales_ped \
                     FROM market_observations o \
                     JOIN market_submissions s ON s.id = o.submission_id \
                     WHERE o.submission_id = (SELECT o2.submission_id \
                                              FROM market_observations o2 \
                                              JOIN market_submissions s2 ON s2.id = o2.submission_id \
                                              WHERE o2.item_name = o.item_name \
                                              ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1) \
                     ORDER BY o.item_name, o.id",
                )?;
                let mut rows = stmt.query([])?;
                let mut overview: Vec<OverviewRow> = Vec::new();
                while let Some(row) = rows.next()? {
                    let item_name: String = row.get(0)?;
                    let horizon: String = row.get(3)?;
                    let Some(horizon) = MarketHorizon::from_stored(&horizon) else {
                        // A vocabulary value outside the enum never gets
                        // written; tolerate rather than fail the read.
                        continue;
                    };
                    if overview.last().map(|entry| entry.item_name.as_str())
                        != Some(item_name.as_str())
                    {
                        overview.push(OverviewRow {
                            item_name,
                            tier: row.get(1)?,
                            observed_at: row.get(2)?,
                            readings: [MarketReading {
                                markup_pct: None,
                                sales_ped: 0.0,
                            }; 5],
                        });
                    }
                    let entry = overview.last_mut().expect("pushed above");
                    entry.readings[horizon as usize] = MarketReading {
                        markup_pct: row.get(4)?,
                        sales_ped: row.get(5)?,
                    };
                }
                Ok(overview)
            })
            .await
    }

    /// One item's observations over time on one horizon, oldest first.
    pub async fn item_history(
        &self,
        item_name: &str,
        horizon: MarketHorizon,
    ) -> Result<Vec<HistoryPoint>, DbError> {
        let item_name = item_name.to_string();
        self.db
            .with_reader(move |connection| {
                let mut stmt = connection.prepare(
                    "SELECT s.submitted_at, o.markup_pct, o.sales_ped \
                     FROM market_observations o \
                     JOIN market_submissions s ON s.id = o.submission_id \
                     WHERE o.item_name = ?1 AND o.horizon = ?2 \
                     ORDER BY o.submission_id",
                )?;
                let points = stmt
                    .query_map(rusqlite::params![item_name, horizon.as_str()], |row| {
                        Ok(HistoryPoint {
                            observed_at: row.get(0)?,
                            markup_pct: row.get(1)?,
                            sales_ped: row.get(2)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(points)
            })
            .await
    }

    /// The most recent accepted paste as a contributable batch, or None
    /// before the first commit. Most recent by `submitted_at` (the id only
    /// breaks a tie), so a restored or replayed older paste written later
    /// never displaces the newest one. Rows come back exactly as stored;
    /// the batch never blends submissions, so what a contributor shares
    /// is precisely the export they pasted.
    pub async fn latest_submission(&self) -> Result<Option<SubmissionBatch>, DbError> {
        self.db
            .with_reader(|connection| {
                let mut stmt = connection.prepare(
                    "SELECT o.item_name, o.tier, s.submitted_at, o.horizon, \
                            o.markup_pct, o.sales_ped \
                     FROM market_observations o \
                     JOIN market_submissions s ON s.id = o.submission_id \
                     WHERE o.submission_id = (SELECT id FROM market_submissions \
                                              ORDER BY submitted_at DESC, id DESC LIMIT 1) \
                     ORDER BY o.item_name, o.id",
                )?;
                let mut rows = stmt.query([])?;
                let mut observed_at = 0.0;
                let mut items: Vec<BatchItem> = Vec::new();
                while let Some(row) = rows.next()? {
                    let item_name: String = row.get(0)?;
                    let horizon: String = row.get(3)?;
                    let Some(horizon) = MarketHorizon::from_stored(&horizon) else {
                        // A vocabulary value outside the enum never gets
                        // written; tolerate rather than fail the read.
                        continue;
                    };
                    observed_at = row.get(2)?;
                    if items.last().map(|entry| entry.item_name.as_str())
                        != Some(item_name.as_str())
                    {
                        items.push(BatchItem {
                            item_name,
                            tier: row.get(1)?,
                            readings: [MarketReading {
                                markup_pct: None,
                                sales_ped: 0.0,
                            }; 5],
                        });
                    }
                    let entry = items.last_mut().expect("pushed above");
                    entry.readings[horizon as usize] = MarketReading {
                        markup_pct: row.get(4)?,
                        sales_ped: row.get(5)?,
                    };
                }
                Ok(if items.is_empty() {
                    None
                } else {
                    Some(SubmissionBatch { observed_at, items })
                })
            })
            .await
    }

    /// Every hunted species' estimated loot markup on one horizon: the
    /// species' recorded loot composition (active items across all
    /// recorded kills) TT-weighted by each item's latest markup
    /// observation. Reading the accounting tables here is the sanctioned
    /// direction of the market boundary; nothing flows back.
    pub async fn mob_ranking(&self, horizon: MarketHorizon) -> Result<Vec<MobRankingRow>, DbError> {
        let started = std::time::Instant::now();
        self.db.with_writer(crate::session_rollup::heal).await?;
        let result = self
            .db
            .with_reader(move |connection| {
                // The latest markup observation per item on the horizon.
                let mut markup_stmt = connection.prepare(
                    "SELECT o.item_name, o.markup_pct \
                     FROM market_observations o \
                     WHERE o.horizon = ?1 \
                       AND o.submission_id = (SELECT o2.submission_id \
                                              FROM market_observations o2 \
                                              JOIN market_submissions s2 ON s2.id = o2.submission_id \
                                              WHERE o2.item_name = o.item_name \
                                              ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1)",
                )?;
                let mut markups: std::collections::HashMap<String, Option<f64>> =
                    std::collections::HashMap::new();
                let mut rows = markup_stmt.query(rusqlite::params![horizon.as_str()])?;
                while let Some(row) = rows.next()? {
                    markups.insert(row.get(0)?, row.get(1)?);
                }

                // The per-species, per-item composition over active loot
                // (enhancer-shrapnel returns are enhancer accounting, not
                // loot composition).
                let loot = crate::session_rollup::effective_hunting_loot_cells(connection, None)?;
                let mut ranking: std::collections::BTreeMap<String, MobRankingRow> =
                    std::collections::BTreeMap::new();
                let mut weighted: std::collections::HashMap<String, f64> =
                    std::collections::HashMap::new();
                let mut composition: std::collections::BTreeMap<(String, String), f64> =
                    std::collections::BTreeMap::new();
                for cell in loot
                    .into_iter()
                    .filter(|cell| !cell.is_enhancer_shrapnel && !cell.mob_species.is_empty())
                {
                    *composition
                        .entry((cell.mob_species, cell.item_name))
                        .or_insert(0.0) += cell.value_ped;
                }
                for ((species, item), tt) in composition {
                    let entry = ranking
                        .entry(species.clone())
                        .or_insert_with(|| MobRankingRow {
                            mob_species: species.clone(),
                            loot_tt: 0.0,
                            covered_tt: 0.0,
                            item_count: 0,
                            covered_item_count: 0,
                            est_markup_pct: None,
                        });
                    entry.loot_tt += tt;
                    entry.item_count += 1;
                    if let Some(Some(markup)) = markups.get(&item) {
                        entry.covered_tt += tt;
                        entry.covered_item_count += 1;
                        *weighted.entry(species).or_insert(0.0) += tt * markup;
                    }
                }
                let mut result: Vec<MobRankingRow> = ranking
                    .into_values()
                    .map(|mut row| {
                        if row.covered_tt > 0.0 {
                            row.est_markup_pct = Some(weighted[&row.mob_species] / row.covered_tt);
                        }
                        row
                    })
                    .collect();
                // Best estimated markup first; no-data species last, by TT.
                result.sort_by(|a, b| match (a.est_markup_pct, b.est_markup_pct) {
                    (Some(x), Some(y)) => y.total_cmp(&x),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => b.loot_tt.total_cmp(&a.loot_tt),
                });
                Ok(result)
            })
            .await?;
        tracing::debug!(
            target: "eo::performance",
            operation = "market.mob_ranking",
            elapsed_us = started.elapsed().as_micros() as u64,
            result_count = result.len(),
            "analytical read complete"
        );
        Ok(result)
    }

    /// The estimated market signals for every active harvest-looted
    /// item, plus the nanocube recycling floor. Each item's resolved
    /// markup and TT turnover follow a horizon fallback: the latest week
    /// reading, else the latest month, else the latest year. The per-item
    /// `readings` carry every horizon (day, week, month, year) for the
    /// detail view; decade is the only horizon skipped. Items are ordered
    /// by name.
    ///
    /// Reading `harvest_loot_items` here (to scope the item set) is the
    /// sanctioned direction of the market boundary; nothing flows back,
    /// and no realised figure is computed.
    pub async fn harvest_markups(&self) -> Result<HarvestMarketData, DbError> {
        let names = self
            .db
            .with_reader(|connection| {
                let mut stmt = connection.prepare(
                    "SELECT DISTINCT item_name FROM harvest_loot_items \
                     WHERE deactivated_at IS NULL ORDER BY item_name",
                )?;
                let names = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(names)
            })
            .await?;
        self.activity_markups(names).await
    }

    /// The estimated market signals for every active hunting-looted item,
    /// plus the same nanocube recycling floor: the Hunting sibling of
    /// [`Self::harvest_markups`] over the kill loot composition. Enhancer
    /// shrapnel returns are enhancer accounting, not mob loot, and are
    /// excluded from the item set.
    pub async fn hunt_markups(&self) -> Result<HarvestMarketData, DbError> {
        let started = std::time::Instant::now();
        self.db.with_writer(crate::session_rollup::heal).await?;
        let names = self
            .db
            .with_reader(|connection| {
                let mut names: std::collections::BTreeSet<String> =
                    crate::session_rollup::effective_hunting_loot_cells(connection, None)?
                        .into_iter()
                        .filter(|cell| !cell.is_enhancer_shrapnel)
                        .map(|cell| cell.item_name)
                        .collect();
                let mut stmt = connection.prepare(
                    "SELECT DISTINCT item_name FROM session_quest_completion_reward_items",
                )?;
                let reward_names = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                names.extend(reward_names);
                Ok(names.into_iter().collect())
            })
            .await?;
        let result = self.activity_markups(names).await?;
        tracing::debug!(
            target: "eo::performance",
            operation = "market.hunt_markups",
            elapsed_us = started.elapsed().as_micros() as u64,
            result_count = result.items.len(),
            "analytical read complete"
        );
        Ok(result)
    }

    /// The shared markup read over one activity's item universe. The
    /// supplied names are the activity's active loot items; everything
    /// else (latest observations, horizon fallback, nanocube floor) is
    /// identical between activities on purpose.
    async fn activity_markups(&self, names: Vec<String>) -> Result<HarvestMarketData, DbError> {
        self.db
            .with_reader(move |connection| {
                // Per item, the (markup, sales) at the latest submission for
                // each of day/week/month/year. Markup is kept as an Option:
                // a NULL markup is "no value on that horizon" and does not
                // enter the resolution fallback, but its (zero) volume still
                // shows in the breakdown.
                let mut obs_stmt = connection.prepare(
                    "SELECT o.item_name, o.horizon, o.markup_pct, o.sales_ped \
                     FROM market_observations o \
                     WHERE o.horizon IN ('day', 'week', 'month', 'year') \
                       AND o.submission_id = (SELECT o2.submission_id \
                                              FROM market_observations o2 \
                                              JOIN market_submissions s2 ON s2.id = o2.submission_id \
                                              WHERE o2.item_name = o.item_name \
                                                AND o2.horizon = o.horizon \
                                              ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1)",
                )?;
                // item -> horizon -> (markup, sales_ped).
                let mut per_item: std::collections::HashMap<
                    String,
                    std::collections::HashMap<String, (Option<f64>, f64)>,
                > = std::collections::HashMap::new();
                let mut rows = obs_stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let item: String = row.get(0)?;
                    let horizon: String = row.get(1)?;
                    let markup: Option<f64> = row.get(2)?;
                    let sales: f64 = row.get(3)?;
                    per_item
                        .entry(item)
                        .or_default()
                        .insert(horizon, (markup, sales));
                }
                // Resolve one item's reading by the week -> month -> year
                // preference (only horizons with a markup qualify): the
                // markup, its horizon, and that horizon's TT turnover.
                let resolve = |item: &str| -> Option<(f64, String, f64)> {
                    let by_horizon = per_item.get(item)?;
                    for horizon in ["week", "month", "year"] {
                        if let Some(&(Some(markup), sales)) = by_horizon.get(horizon) {
                            return Some((markup, horizon.to_string(), sales));
                        }
                    }
                    None
                };
                // The day/week/month/year breakdown for an item (missing
                // horizons read as no markup, zero volume).
                let readings_of = |item: &str| -> Vec<HarvestHorizonReading> {
                    let by_horizon = per_item.get(item);
                    ["day", "week", "month", "year"]
                        .into_iter()
                        .map(|horizon| {
                            let (markup_pct, sales_ped) = by_horizon
                                .and_then(|m| m.get(horizon))
                                .copied()
                                .unwrap_or((None, 0.0));
                            HarvestHorizonReading {
                                horizon: horizon.to_string(),
                                markup_pct,
                                sales_ped,
                            }
                        })
                        .collect()
                };

                let nanocube_markup_pct = resolve("Nanocube").map(|(markup, _, _)| markup);

                let mut unit_prices = std::collections::HashMap::new();
                let mut unit_stmt = connection.prepare(
                    "SELECT p.item_name, p.ped_per_unit \
                     FROM market_unit_price_observations p \
                     WHERE p.id = (SELECT p2.id FROM market_unit_price_observations p2 \
                                   WHERE p2.item_name = p.item_name \
                                   ORDER BY p2.observed_at DESC, p2.id DESC LIMIT 1)",
                )?;
                let mut unit_rows = unit_stmt.query([])?;
                while let Some(row) = unit_rows.next()? {
                    unit_prices.insert(row.get::<_, String>(0)?, row.get::<_, f64>(1)?);
                }

                let items = names
                    .into_iter()
                    .map(|name| {
                        let resolved = resolve(&name);
                        let readings = readings_of(&name);
                        let (markup_pct, horizon, sales_ped) = match resolved {
                            Some((m, h, s)) => (Some(m), Some(h), Some(s)),
                            None => (None, None, None),
                        };
                        let unit_price_ped = unit_prices.get(&name).copied();
                        HarvestItemMarkup {
                            item_name: name,
                            markup_pct,
                            unit_price_ped,
                            horizon,
                            sales_ped,
                            recommended_packet_tt: markup_pct.and_then(|markup| {
                                crate::auction_fee::recommended_packet_tt(
                                    markup,
                                    crate::auction_fee::DEFAULT_MAX_FEE_SHARE_PCT,
                                )
                            }),
                            readings,
                        }
                    })
                    .collect();

                Ok(HarvestMarketData {
                    nanocube_markup_pct,
                    items,
                })
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;

    const SAMPLE: &str = "Carabok Hide\t0\t106.880%\t451.900 PED\t107.160%\t531.900 PED\t\
106.020%\t979.040 PED\t108.280%\t13.500K PED\t158.920%\t35.300K PED\n\
Carabok Leg Fur\t0\tN/A\t0.000 PEC\tN/A\t0.000 PEC\tN/A\t0.000 PEC\t109.380%\t6.400 PED\t\
339.100%\t375.400 PED";

    fn rig(dir: &std::path::Path) -> (tokio::runtime::Runtime, MarketService, Arc<MockClock>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = runtime
            .block_on(Db::open(&dir.join("entropia_orme.db")))
            .unwrap();
        let clock = Arc::new(MockClock::new(
            Some(
                chrono::NaiveDateTime::parse_from_str("2026-07-11 12:00:00", "%Y-%m-%d %H:%M:%S")
                    .unwrap(),
            ),
            0.0,
        ));
        let service = MarketService::new(db, clock.clone());
        (runtime, service, clock)
    }

    #[test]
    fn commit_then_overview_round_trips_the_readings() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());

        let outcome = runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        assert_eq!(outcome.item_count, 2);
        assert_eq!(outcome.skipped_count, 0);

        let overview = runtime.block_on(service.overview()).unwrap();
        assert_eq!(overview.len(), 2);
        // Sorted by item name; readings land on their horizons.
        assert_eq!(overview[0].item_name, "Carabok Hide");
        assert_eq!(overview[0].readings[0].markup_pct, Some(106.880));
        assert_eq!(overview[0].readings[4].sales_ped, 35_300.0);
        assert_eq!(overview[1].item_name, "Carabok Leg Fur");
        assert_eq!(overview[1].readings[0].markup_pct, None);
        assert_eq!(overview[1].readings[3].markup_pct, Some(109.380));
        assert_eq!(overview[0].observed_at, outcome.observed_at);
    }

    #[test]
    fn preview_returns_the_real_parse_without_touching_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let (_runtime, service, _clock) = rig(dir.path());

        // preview delegates to the parser and returns its rows verbatim;
        // it never yields an empty default parse.
        let parse = service.preview(SAMPLE);
        assert_eq!(parse.rows.len(), 2);
        assert!(parse.skipped.is_empty());
        assert_eq!(parse.rows[0].item_name, "Carabok Hide");
        assert_eq!(parse.rows[0].readings[0].markup_pct, Some(106.880));
    }

    #[test]
    fn overview_serves_each_items_latest_submission() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());

        runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        clock.advance(7.0 * 24.0 * 3600.0).unwrap();
        // A week later only Carabok Hide is re-observed, cheaper.
        let later = "Carabok Hide\t0\t101.000%\t10.000 PED\t101.000%\t10.000 PED\t\
101.000%\t10.000 PED\t101.000%\t10.000 PED\t101.000%\t10.000 PED";
        let second = runtime.block_on(service.commit_paste(later)).unwrap();

        let overview = runtime.block_on(service.overview()).unwrap();
        assert_eq!(overview.len(), 2);
        // The re-observed item serves the new submission...
        assert_eq!(overview[0].item_name, "Carabok Hide");
        assert_eq!(overview[0].readings[0].markup_pct, Some(101.000));
        assert_eq!(overview[0].observed_at, second.observed_at);
        // ...while the unrefreshed one keeps its older readings and
        // (staleness-bearing) older timestamp.
        assert_eq!(overview[1].item_name, "Carabok Leg Fur");
        assert_eq!(overview[1].readings[4].markup_pct, Some(339.100));
        assert!(overview[1].observed_at < second.observed_at);
    }

    #[test]
    fn history_orders_one_horizon_over_time() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());

        runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        clock.advance(24.0 * 3600.0).unwrap();
        let later = "Carabok Hide\t0\t105.500%\t200.000 PED\t107.000%\t500.000 PED\t\
106.000%\t900.000 PED\t108.000%\t13.000K PED\t158.000%\t35.000K PED";
        runtime.block_on(service.commit_paste(later)).unwrap();

        let history = runtime
            .block_on(service.item_history("Carabok Hide", MarketHorizon::Day))
            .unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].observed_at < history[1].observed_at);
        assert_eq!(history[0].markup_pct, Some(106.880));
        assert_eq!(history[1].markup_pct, Some(105.500));
        assert_eq!(history[1].sales_ped, 200.0);

        // A horizon the item never traded on still has its rows (N/A
        // markup as NULL), and an unknown item has none.
        let fur = runtime
            .block_on(service.item_history("Carabok Leg Fur", MarketHorizon::Day))
            .unwrap();
        assert_eq!(fur.len(), 1);
        assert_eq!(fur[0].markup_pct, None);
        let none = runtime
            .block_on(service.item_history("Unseen Item", MarketHorizon::Day))
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn latest_submission_serves_the_newest_batch_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());

        // Before any commit there is nothing to contribute.
        assert_eq!(runtime.block_on(service.latest_submission()).unwrap(), None);

        runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        clock.advance(7.0 * 24.0 * 3600.0).unwrap();
        let later = "Carabok Hide\t0\t101.000%\t10.000 PED\t101.000%\t10.000 PED\t\
101.000%\t10.000 PED\t101.000%\t10.000 PED\t101.000%\t10.000 PED";
        let second = runtime.block_on(service.commit_paste(later)).unwrap();

        // The newest paste only, verbatim: never a blend across
        // submissions (the earlier batch's other item is absent).
        let batch = runtime
            .block_on(service.latest_submission())
            .unwrap()
            .unwrap();
        assert_eq!(batch.observed_at, second.observed_at);
        assert_eq!(batch.items.len(), 1);
        assert_eq!(batch.items[0].item_name, "Carabok Hide");
        assert_eq!(batch.items[0].readings[0].markup_pct, Some(101.000));
        assert_eq!(batch.items[0].readings[4].sales_ped, 10.0);
    }

    #[test]
    fn latest_submission_is_latest_in_time_not_the_highest_id() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());

        // The newer paste is committed first; an older export is then
        // committed with the clock wound back (a restored backup, a
        // replayed log), so it carries the higher id but the earlier
        // submitted_at. Both the contributable batch and the per-item
        // overview read the paste latest in time.
        let newest = runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        clock.freeze_at(
            chrono::NaiveDateTime::parse_from_str("2026-07-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        );
        let older = "Carabok Hide\t0\t101.000%\t10.000 PED\t101.000%\t10.000 PED\t\
101.000%\t10.000 PED\t101.000%\t10.000 PED\t101.000%\t10.000 PED";
        let stale = runtime.block_on(service.commit_paste(older)).unwrap();
        assert!(stale.observed_at < newest.observed_at);

        let batch = runtime
            .block_on(service.latest_submission())
            .unwrap()
            .unwrap();
        assert_eq!(batch.observed_at, newest.observed_at);
        assert_eq!(batch.items.len(), 2);
        assert_eq!(batch.items[0].readings[0].markup_pct, Some(106.880));

        let overview = runtime.block_on(service.overview()).unwrap();
        assert_eq!(overview.len(), 2);
        assert_eq!(overview[0].item_name, "Carabok Hide");
        assert_eq!(overview[0].observed_at, newest.observed_at);
        assert_eq!(overview[0].readings[0].markup_pct, Some(106.880));
    }

    #[test]
    fn mob_ranking_weights_composition_by_latest_markup_with_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        // Seed loot composition: Carabok drops 100 PED of Hide (observed)
        // and 100 PED of Leg Fur (unobserved); Atrox drops 50 PED of an
        // item with no observation at all. The effective loot boundary is
        // session-owned, matching production tracking and orphan recovery.
        runtime
            .block_on(service.db.with_writer(|connection| {
                connection.execute_batch(
                    "INSERT INTO tracking_sessions \
                         (id, started_at, ended_at, is_active) \
                     VALUES ('s1', 0, 1, 0); \
                     INSERT INTO kills (id, session_id, mob_species, timestamp) VALUES \
                       ('k1', 's1', 'Carabok', 0), ('k2', 's1', 'Atrox', 0), \
                       ('k3', 's1', '', 0); \
                     INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) VALUES \
                       ('k1', 'Carabok Hide', 1, 60.0, 0), \
                       ('k1', 'Carabok Hide', 1, 40.0, 0), \
                       ('k1', 'Carabok Leg Fur', 1, 100.0, 0), \
                       ('k1', 'Shrapnel', 1, 999.0, 1), \
                       ('k2', 'Animal Oil Residue', 1, 50.0, 0), \
                       ('k3', 'Orphan Loot', 1, 10.0, 0);",
                )?;
                Ok(())
            }))
            .unwrap();
        runtime.block_on(service.commit_paste(SAMPLE)).unwrap();

        let ranking = runtime
            .block_on(service.mob_ranking(MarketHorizon::Day))
            .unwrap();
        assert_eq!(ranking.len(), 2, "unattributed kills are excluded");

        // Carabok: half covered (Hide day 106.880; Leg Fur's day is N/A,
        // which is no observation, not zero). Atrox: no observed items at
        // all, so no estimate, and it sorts after the estimated species.
        assert_eq!(ranking[0].mob_species, "Carabok");
        assert_eq!(ranking[0].loot_tt, 200.0);
        assert_eq!(ranking[0].covered_tt, 100.0);
        assert_eq!(ranking[0].item_count, 2);
        assert_eq!(ranking[0].covered_item_count, 1);
        assert_eq!(ranking[0].est_markup_pct, Some(106.88));
        assert_eq!(ranking[1].mob_species, "Atrox");
        assert_eq!(ranking[1].loot_tt, 50.0);
        assert_eq!(ranking[1].covered_tt, 0.0);
        assert_eq!(ranking[1].est_markup_pct, None);

        // On the decade horizon both Carabok items carry observations,
        // so the estimate TT-weights across the full composition.
        let decade = runtime
            .block_on(service.mob_ranking(MarketHorizon::Decade))
            .unwrap();
        assert_eq!(decade[0].mob_species, "Carabok");
        assert_eq!(decade[0].covered_tt, 200.0);
        let est = decade[0].est_markup_pct.unwrap();
        assert!((est - 249.01).abs() < 1e-9, "got {est}");
    }

    #[test]
    fn harvest_markups_resolve_by_week_month_year_fallback_with_sales() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        // Harvest loot: four items exercising each fallback rung and the
        // uncovered case.
        //  - Wood Shavings         -> week present (120%, 10 PED)
        //  - Short Moonleaf Board  -> week N/A, month present (200%, 30 PED)
        //  - Moonleaf Board        -> week+month N/A, year present (300%, 40 PED)
        //  - Long Moonleaf Board   -> no observation at all (uncovered)
        runtime
            .block_on(service.db.with_writer(|connection| {
                connection.execute_batch(
                    "INSERT INTO harvest_events (id, session_id, timestamp, success, tool_name, cost_ped, loot_total_ped) VALUES \
                       ('h1', 's1', 0, 1, 'Terratech PH-4 (L)', 1.0, 4.0); \
                     INSERT INTO harvest_loot_items (harvest_id, item_name, quantity, value_ped) VALUES \
                       ('h1', 'Wood Shavings', 1, 100.0), \
                       ('h1', 'Short Moonleaf Board', 1, 100.0), \
                       ('h1', 'Moonleaf Board', 1, 100.0), \
                       ('h1', 'Long Moonleaf Board', 1, 100.0);",
                )?;
                Ok(())
            }))
            .unwrap();
        // day, week, month, year, decade columns per item. A Nanocube row
        // provides the recycling floor.
        let paste = "Wood Shavings\t0\t110.000%\t5.000 PED\t120.000%\t10.000 PED\t\
130.000%\t20.000 PED\t140.000%\t30.000 PED\t150.000%\t40.000 PED\n\
Short Moonleaf Board\t0\tN/A\t0.000 PEC\tN/A\t0.000 PEC\t200.000%\t30.000 PED\t\
210.000%\t40.000 PED\t220.000%\t50.000 PED\n\
Moonleaf Board\t0\tN/A\t0.000 PEC\tN/A\t0.000 PEC\tN/A\t0.000 PEC\t300.000%\t40.000 PED\t\
320.000%\t50.000 PED\n\
Nanocube\t0\t101.000%\t100.000 PED\t100.840%\t200.000 PED\t\
100.650%\t300.000 PED\t100.820%\t400.000 PED\t101.210%\t500.000 PED";
        runtime.block_on(service.commit_paste(paste)).unwrap();

        let data = runtime.block_on(service.harvest_markups()).unwrap();
        assert_eq!(data.nanocube_markup_pct, Some(100.84));
        // Only the harvest-looted items, name-ordered.
        assert_eq!(
            data.items
                .iter()
                .map(|i| i.item_name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Long Moonleaf Board",
                "Moonleaf Board",
                "Short Moonleaf Board",
                "Wood Shavings"
            ],
        );
        let by_name: std::collections::HashMap<&str, &HarvestItemMarkup> = data
            .items
            .iter()
            .map(|i| (i.item_name.as_str(), i))
            .collect();
        // Each item resolves to its finest available horizon, with that
        // horizon's TT turnover.
        // Every item's breakdown is ordered day, week, month, year.
        let markups = |name: &str| {
            by_name[name]
                .readings
                .iter()
                .map(|r| (r.horizon.as_str(), r.markup_pct, r.sales_ped))
                .collect::<Vec<_>>()
        };
        assert_eq!(by_name["Wood Shavings"].markup_pct, Some(120.0));
        assert_eq!(by_name["Wood Shavings"].horizon.as_deref(), Some("week"));
        assert_eq!(by_name["Wood Shavings"].sales_ped, Some(10.0));
        assert_eq!(by_name["Wood Shavings"].recommended_packet_tt, Some(49.0));
        // Full day/week/month/year breakdown, markup and volume per horizon.
        assert_eq!(
            markups("Wood Shavings"),
            vec![
                ("day", Some(110.0), 5.0),
                ("week", Some(120.0), 10.0),
                ("month", Some(130.0), 20.0),
                ("year", Some(140.0), 30.0),
            ],
        );
        assert_eq!(by_name["Short Moonleaf Board"].markup_pct, Some(200.0));
        assert_eq!(
            by_name["Short Moonleaf Board"].horizon.as_deref(),
            Some("month")
        );
        assert_eq!(by_name["Short Moonleaf Board"].sales_ped, Some(30.0));
        assert_eq!(
            by_name["Short Moonleaf Board"].recommended_packet_tt,
            Some(9.8)
        );
        // Fell back to month; the day/week rows carry no markup and zero
        // volume, still present in the breakdown.
        assert_eq!(
            markups("Short Moonleaf Board"),
            vec![
                ("day", None, 0.0),
                ("week", None, 0.0),
                ("month", Some(200.0), 30.0),
                ("year", Some(210.0), 40.0),
            ],
        );
        assert_eq!(by_name["Moonleaf Board"].markup_pct, Some(300.0));
        assert_eq!(by_name["Moonleaf Board"].horizon.as_deref(), Some("year"));
        assert_eq!(by_name["Moonleaf Board"].sales_ped, Some(40.0));
        assert_eq!(by_name["Long Moonleaf Board"].markup_pct, None);
        assert_eq!(by_name["Long Moonleaf Board"].horizon, None);
        assert_eq!(by_name["Long Moonleaf Board"].sales_ped, None);
        assert_eq!(by_name["Long Moonleaf Board"].recommended_packet_tt, None);
        // No observation at all: every horizon reads empty.
        assert_eq!(
            markups("Long Moonleaf Board"),
            vec![
                ("day", None, 0.0),
                ("week", None, 0.0),
                ("month", None, 0.0),
                ("year", None, 0.0),
            ],
        );
    }

    #[test]
    fn harvest_markups_are_empty_without_harvest_loot() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        let data = runtime.block_on(service.harvest_markups()).unwrap();
        assert!(data.items.is_empty());
        assert_eq!(data.nanocube_markup_pct, None);
    }

    #[test]
    fn an_unusable_paste_commits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());

        let err = runtime
            .block_on(service.commit_paste("not market data\nat all"))
            .unwrap_err();
        assert!(matches!(err, MarketError::EmptyPaste));
        assert!(runtime.block_on(service.overview()).unwrap().is_empty());
    }
}
