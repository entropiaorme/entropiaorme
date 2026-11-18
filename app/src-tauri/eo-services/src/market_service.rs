//! The market domain service: the manual markup-observation feed and
//! its reads.
//!
//! The user pastes the game's market-ledger export ([`crate::market_paste`]);
//! an accepted paste commits as one submission whose observations carry
//! the five aggregation horizons per item. Reads serve the overview
//! (each item's latest readings, with when they were observed) and a
//! per-item, per-horizon history.
//!
//! Beside the pasted observations, the player can enter an item's markup by
//! hand (a percentage, or PED above TT per unit) for items the import does
//! not cover or covers poorly; whichever of the two was recorded more
//! recently is the one an item's valuation uses. The player can also mark an
//! item to be valued at the Nanocube markup in the live session figures.
//!
//! Everything here is the INFORMATIONAL market layer: estimated markup
//! never joins the ledger, the analytics aggregates, or any realised
//! P&L figure. Writes stay on the `market_*` tables; the sanctioned
//! reads of the accounting tables are the harvest and hunting
//! loot-item scopes, the one-way direction of the market boundary, and
//! nothing flows back.

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::market_paste::{parse_market_paste, MarketHorizon, MarketPasteParse, MarketReading};
use crate::time::naive_to_epoch;

/// Told after any market write commits (a paste, a quote, a manual markup,
/// a Nanocube mark), so surfaces valuing loot can re-read.
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

/// The market domain service over the shared database and injected
/// clock.
pub struct MarketService {
    db: Db,
    clock: Arc<dyn Clock>,
    changed: Option<ChangedSink>,
}

/// How a manual markup was entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualMarkupKind {
    /// A percentage of TT (`120` is 120%).
    Percent,
    /// PED above TT per unit, as the auction quotes an item that does not
    /// stack (`+2.5`).
    PedPerUnit,
}

impl ManualMarkupKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Percent => "percent",
            Self::PedPerUnit => "ped_per_unit",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        match value {
            "percent" => Some(Self::Percent),
            "ped_per_unit" => Some(Self::PedPerUnit),
            _ => None,
        }
    }
}

/// A markup the player entered by hand for one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualMarkup {
    pub kind: ManualMarkupKind,
    pub value: f64,
    /// The item's TT per unit where the entry was made: what a PED-per-unit
    /// entry is a markup over.
    pub unit_tt_ped: Option<f64>,
    /// Epoch seconds.
    pub recorded_at: f64,
}

impl ManualMarkup {
    /// The entry as a percentage of TT: a percentage as entered, a PED
    /// amount over the unit TT it was entered against. None for a PED
    /// amount on an item with no TT, which only has an absolute value.
    pub fn markup_pct(&self) -> Option<f64> {
        match self.kind {
            ManualMarkupKind::Percent => Some(self.value),
            ManualMarkupKind::PedPerUnit => self
                .unit_tt_ped
                .filter(|tt| *tt > 0.0)
                .map(|tt| (tt + self.value) / tt * 100.0),
        }
    }
}

/// A manual markup as the player enters it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ManualMarkupInput {
    Percent(f64),
    PedPerUnit { ped: f64, unit_tt_ped: Option<f64> },
}

/// Which reading an item's valuation uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkupSource {
    Market,
    Manual,
}

/// The most recent of an item's market reading and manual entry wins: a
/// newer import replaces a manual value, a newer manual value replaces an
/// import. A manual entry recorded in the same instant as the import wins,
/// being the deliberate act. `market_observed_at` is present only when the
/// market resolved a markup.
pub fn preferred_source(
    market_observed_at: Option<f64>,
    manual: Option<&ManualMarkup>,
) -> Option<MarkupSource> {
    match (market_observed_at, manual) {
        (None, None) => None,
        (Some(_), None) => Some(MarkupSource::Market),
        (None, Some(_)) => Some(MarkupSource::Manual),
        (Some(market_at), Some(manual)) => Some(if manual.recorded_at >= market_at {
            MarkupSource::Manual
        } else {
            MarkupSource::Market
        }),
    }
}

/// Shrapnel converts to Universal Ammo at a fixed 101%, whatever any market
/// says, so neither a manual markup nor a Nanocube mark applies to it.
fn is_shrapnel(item_name: &str) -> bool {
    item_name.trim().eq_ignore_ascii_case("shrapnel")
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
    /// The market's resolved markup (week, then month, then year): always
    /// the market reading, whichever source `preferred` names.
    pub markup_pct: Option<f64>,
    /// Epoch seconds of the submission the resolved market reading came
    /// from; None when the market resolved no markup.
    pub market_observed_at: Option<f64>,
    /// The player's standing manual entry, if any.
    pub manual: Option<ManualMarkup>,
    /// Which of the two the item's valuation uses: the more recent.
    pub preferred: Option<MarkupSource>,
    /// Marked to be valued at the Nanocube markup in the live session
    /// figures (only those figures read it).
    pub nanocube_marked: bool,
    /// Absolute informational quote for one item unit. Mutually independent
    /// from percentage-of-TT markup and never enters realised accounting.
    pub unit_price_ped: Option<f64>,
    /// The horizon that supplied the reading ("week" | "month" | "year"),
    /// None when uncovered.
    pub horizon: Option<String>,
    /// TT turnover (PED) at the resolved horizon: market-capacity evidence.
    pub sales_ped: Option<f64>,
    /// TT packet whose expected direct-market markup keeps the listing fee
    /// to at most ten per cent of gross markup, at the preferred markup.
    /// Independent of turnover.
    pub recommended_packet_tt: Option<f64>,
    /// Every horizon's reading, ordered day, week, month, year.
    pub readings: Vec<HarvestHorizonReading>,
}

impl HarvestItemMarkup {
    /// The markup (percent) the preferred source gives: the manual entry
    /// when it is the more recent and reads as a percentage, else the
    /// market's.
    pub fn preferred_markup_pct(&self) -> Option<f64> {
        match self.preferred {
            Some(MarkupSource::Manual) => self.manual.as_ref().and_then(ManualMarkup::markup_pct),
            Some(MarkupSource::Market) => self.markup_pct,
            None => None,
        }
    }
}

/// One item's latest reading per horizon: (markup, TT turnover, when the
/// submission carrying it was made).
type HorizonReadings = std::collections::HashMap<String, (Option<f64>, f64, f64)>;

/// The most items one lookup by name serves: a session's loot, an
/// inventory's holdings, never the whole catalogue.
pub const MAX_LOOKUP_NAMES: usize = 2000;

impl MarketService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self {
            db,
            clock,
            changed: None,
        }
    }

    /// Announce every committed write through `changed`.
    pub fn with_changed(mut self, changed: ChangedSink) -> Self {
        self.changed = Some(changed);
        self
    }

    fn notify_changed(&self) {
        if let Some(changed) = &self.changed {
            changed();
        }
    }

    /// Record a markup the player entered for `item_name`. It replaces any
    /// earlier manual entry, is used wherever it is more recent than the
    /// item's market import, and lifts a live Nanocube mark on the item
    /// (entering a value says the player trusts it).
    pub async fn set_manual_markup(
        &self,
        item_name: &str,
        input: ManualMarkupInput,
    ) -> Result<ManualMarkup, MarketError> {
        let item_name = valid_item_name(item_name)?;
        let (kind, value, unit_tt_ped) = match input {
            ManualMarkupInput::Percent(percent) => {
                if !percent.is_finite() || percent <= 0.0 || percent > 1_000_000.0 {
                    return Err(MarketError::InvalidInput(
                        "markup percent must be above 0".to_string(),
                    ));
                }
                (ManualMarkupKind::Percent, percent, None)
            }
            ManualMarkupInput::PedPerUnit { ped, unit_tt_ped } => {
                if !ped.is_finite() || !(0.0..=10_000_000.0).contains(&ped) {
                    return Err(MarketError::InvalidInput(
                        "markup PED must be zero or more".to_string(),
                    ));
                }
                if unit_tt_ped.is_some_and(|tt| !tt.is_finite() || tt < 0.0) {
                    return Err(MarketError::InvalidInput(
                        "unit TT must be zero or more".to_string(),
                    ));
                }
                (ManualMarkupKind::PedPerUnit, ped, unit_tt_ped)
            }
        };
        let recorded_at = naive_to_epoch(self.clock.now());
        let write_name = item_name.clone();
        self.db
            .with_writer(move |connection| {
                let tx = connection.transaction()?;
                tx.execute(
                    "INSERT INTO market_manual_markups \
                     (item_name, kind, value, unit_tt_ped, recorded_at) VALUES (?, ?, ?, ?, ?)",
                    rusqlite::params![write_name, kind.as_str(), value, unit_tt_ped, recorded_at],
                )?;
                tx.execute(
                    "DELETE FROM market_live_nanocube_marks WHERE item_name = ?",
                    [&write_name],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.notify_changed();
        Ok(ManualMarkup {
            kind,
            value,
            unit_tt_ped,
            recorded_at,
        })
    }

    /// Withdraw the player's manual markup for `item_name`: the item falls
    /// back to its market import. False when there was none standing.
    pub async fn clear_manual_markup(&self, item_name: &str) -> Result<bool, MarketError> {
        let item_name = valid_item_name(item_name)?;
        let recorded_at = naive_to_epoch(self.clock.now());
        let cleared = self
            .db
            .with_writer(move |connection| {
                let tx = connection.transaction()?;
                let standing = latest_manual_markups(&tx, Some(&item_name))?
                    .remove(&item_name)
                    .is_some();
                if standing {
                    tx.execute(
                        "INSERT INTO market_manual_markups (item_name, kind, recorded_at) \
                         VALUES (?, 'cleared', ?)",
                        rusqlite::params![item_name, recorded_at],
                    )?;
                }
                tx.commit()?;
                Ok(standing)
            })
            .await?;
        if cleared {
            self.notify_changed();
        }
        Ok(cleared)
    }

    /// Mark `item_name` to be valued at the Nanocube markup in the live
    /// session figures (`marked`), or lift the mark. The mark stands until
    /// the player lifts it, or enters a manual markup for the item; a newer
    /// market import leaves it in place. Returns whether the item is marked.
    pub async fn set_live_nanocube_mark(
        &self,
        item_name: &str,
        marked: bool,
    ) -> Result<bool, MarketError> {
        let item_name = valid_item_name(item_name)?;
        let marked_at = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |connection| {
                if marked {
                    connection.execute(
                        "INSERT INTO market_live_nanocube_marks (item_name, marked_at) \
                         VALUES (?, ?) ON CONFLICT(item_name) DO NOTHING",
                        rusqlite::params![item_name, marked_at],
                    )?;
                } else {
                    connection.execute(
                        "DELETE FROM market_live_nanocube_marks WHERE item_name = ?",
                        [&item_name],
                    )?;
                }
                Ok(())
            })
            .await?;
        self.notify_changed();
        Ok(marked)
    }

    /// The market signals for exactly the named items: what a surface
    /// valuing loot it already holds (the live session's, an inventory's)
    /// reads, rather than an activity's whole item universe. Names are
    /// trimmed and de-duplicated; blanks are dropped.
    pub async fn item_markups(&self, names: Vec<String>) -> Result<HarvestMarketData, MarketError> {
        if names.len() > MAX_LOOKUP_NAMES {
            return Err(MarketError::InvalidInput(format!(
                "at most {MAX_LOOKUP_NAMES} items per lookup"
            )));
        }
        let names: std::collections::BTreeSet<String> = names
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        Ok(self.activity_markups(names.into_iter().collect()).await?)
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
        self.notify_changed();
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
        self.notify_changed();
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
                    "SELECT o.item_name, o.horizon, o.markup_pct, o.sales_ped, s.submitted_at \
                     FROM market_observations o \
                     JOIN market_submissions s ON s.id = o.submission_id \
                     WHERE o.horizon IN ('day', 'week', 'month', 'year') \
                       AND o.submission_id = (SELECT o2.submission_id \
                                              FROM market_observations o2 \
                                              JOIN market_submissions s2 ON s2.id = o2.submission_id \
                                              WHERE o2.item_name = o.item_name \
                                                AND o2.horizon = o.horizon \
                                              ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1)",
                )?;
                // item -> horizon -> (markup, sales_ped, submitted_at).
                let mut per_item: std::collections::HashMap<String, HorizonReadings> =
                    std::collections::HashMap::new();
                let mut rows = obs_stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let item: String = row.get(0)?;
                    let horizon: String = row.get(1)?;
                    let markup: Option<f64> = row.get(2)?;
                    let sales: f64 = row.get(3)?;
                    let submitted_at: f64 = row.get(4)?;
                    per_item
                        .entry(item)
                        .or_default()
                        .insert(horizon, (markup, sales, submitted_at));
                }
                // Resolve one item's reading by the week -> month -> year
                // preference (only horizons with a markup qualify): the
                // markup, its horizon, that horizon's TT turnover, and when
                // it was observed.
                let resolve = |item: &str| -> Option<(f64, String, f64, f64)> {
                    let by_horizon = per_item.get(item)?;
                    for horizon in ["week", "month", "year"] {
                        if let Some(&(Some(markup), sales, at)) = by_horizon.get(horizon) {
                            return Some((markup, horizon.to_string(), sales, at));
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
                                .map(|&(markup, sales, _)| (markup, sales))
                                .unwrap_or((None, 0.0));
                            HarvestHorizonReading {
                                horizon: horizon.to_string(),
                                markup_pct,
                                sales_ped,
                            }
                        })
                        .collect()
                };

                let manual = latest_manual_markups(connection, None)?;
                let marked: std::collections::HashSet<String> = {
                    let mut stmt =
                        connection.prepare("SELECT item_name FROM market_live_nanocube_marks")?;
                    let names = stmt
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    names
                };

                // The floor follows the same most-recent rule as every item:
                // a manual Nanocube markup newer than its import is the floor.
                let nanocube_markup_pct = {
                    let market = resolve("Nanocube");
                    let manual = manual.get("Nanocube");
                    match preferred_source(market.as_ref().map(|r| r.3), manual) {
                        Some(MarkupSource::Manual) => manual
                            .and_then(ManualMarkup::markup_pct)
                            .or(market.map(|r| r.0)),
                        _ => market.map(|r| r.0),
                    }
                };

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
                        let (markup_pct, horizon, sales_ped, market_observed_at) = match resolved {
                            Some((m, h, s, at)) => (Some(m), Some(h), Some(s), Some(at)),
                            None => (None, None, None, None),
                        };
                        let unit_price_ped = unit_prices.get(&name).copied();
                        // Shrapnel's value is its fixed conversion, so a
                        // manual entry or mark never applies to it.
                        let shrapnel = is_shrapnel(&name);
                        let manual = (!shrapnel).then(|| manual.get(&name).cloned()).flatten();
                        let nanocube_marked = !shrapnel && marked.contains(&name);
                        let mut item = HarvestItemMarkup {
                            preferred: preferred_source(market_observed_at, manual.as_ref()),
                            item_name: name,
                            markup_pct,
                            market_observed_at,
                            manual,
                            nanocube_marked,
                            unit_price_ped,
                            horizon,
                            sales_ped,
                            recommended_packet_tt: None,
                            readings,
                        };
                        item.recommended_packet_tt =
                            item.preferred_markup_pct().and_then(|markup| {
                                crate::auction_fee::recommended_packet_tt(
                                    markup,
                                    crate::auction_fee::DEFAULT_MAX_FEE_SHARE_PCT,
                                )
                            });
                        item
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

fn valid_item_name(item_name: &str) -> Result<String, MarketError> {
    let item_name = item_name.trim();
    if item_name.is_empty() {
        return Err(MarketError::InvalidInput(
            "item_name must not be empty".to_string(),
        ));
    }
    if is_shrapnel(item_name) {
        return Err(MarketError::InvalidInput(
            "Shrapnel is valued at its fixed 101% conversion".to_string(),
        ));
    }
    Ok(item_name.to_string())
}

/// Each item's standing manual markup: its latest entry, unless that entry
/// is a clearing. `only` narrows the read to one item.
fn latest_manual_markups(
    connection: &rusqlite::Connection,
    only: Option<&str>,
) -> Result<std::collections::HashMap<String, ManualMarkup>, DbError> {
    // Latest in time per item, the id breaking a tie within one instant.
    let mut stmt = connection.prepare(
        "SELECT m.item_name, m.kind, m.value, m.unit_tt_ped, m.recorded_at \
         FROM market_manual_markups m \
         WHERE (?1 IS NULL OR m.item_name = ?1) \
           AND m.id = (SELECT m2.id FROM market_manual_markups m2 \
                       WHERE m2.item_name = m.item_name \
                       ORDER BY m2.recorded_at DESC, m2.id DESC LIMIT 1)",
    )?;
    let mut rows = stmt.query([only])?;
    let mut standing = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let kind: String = row.get(1)?;
        let Some(kind) = ManualMarkupKind::from_stored(&kind) else {
            continue;
        };
        let Some(value) = row.get::<_, Option<f64>>(2)? else {
            continue;
        };
        standing.insert(
            row.get::<_, String>(0)?,
            ManualMarkup {
                kind,
                value,
                unit_tt_ped: row.get(3)?,
                recorded_at: row.get(4)?,
            },
        );
    }
    Ok(standing)
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
    fn overview_is_latest_in_time_not_the_highest_id() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());

        // The newer paste is committed first; an older export is then
        // committed with the clock wound back (a restored backup, a
        // replayed log), so it carries the higher id but the earlier
        // submitted_at. The per-item overview reads the paste latest in
        // time.
        let newest = runtime.block_on(service.commit_paste(SAMPLE)).unwrap();
        clock.freeze_at(
            chrono::NaiveDateTime::parse_from_str("2026-07-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        );
        let older = "Carabok Hide\t0\t101.000%\t10.000 PED\t101.000%\t10.000 PED\t\
101.000%\t10.000 PED\t101.000%\t10.000 PED\t101.000%\t10.000 PED";
        let stale = runtime.block_on(service.commit_paste(older)).unwrap();
        assert!(stale.observed_at < newest.observed_at);

        let overview = runtime.block_on(service.overview()).unwrap();
        assert_eq!(overview.len(), 2);
        assert_eq!(overview[0].item_name, "Carabok Hide");
        assert_eq!(overview[0].observed_at, newest.observed_at);
        assert_eq!(overview[0].readings[0].markup_pct, Some(106.880));
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

    fn manual(kind: ManualMarkupKind, value: f64, recorded_at: f64) -> ManualMarkup {
        ManualMarkup {
            kind,
            value,
            unit_tt_ped: None,
            recorded_at,
        }
    }

    #[test]
    fn the_more_recent_of_import_and_manual_entry_wins() {
        let entry = manual(ManualMarkupKind::Percent, 150.0, 200.0);
        assert_eq!(preferred_source(None, None), None);
        assert_eq!(
            preferred_source(Some(100.0), None),
            Some(MarkupSource::Market)
        );
        assert_eq!(
            preferred_source(None, Some(&entry)),
            Some(MarkupSource::Manual)
        );
        assert_eq!(
            preferred_source(Some(100.0), Some(&entry)),
            Some(MarkupSource::Manual)
        );
        assert_eq!(
            preferred_source(Some(300.0), Some(&entry)),
            Some(MarkupSource::Market)
        );
        // Recorded in the same instant, the deliberate act wins.
        assert_eq!(
            preferred_source(Some(200.0), Some(&entry)),
            Some(MarkupSource::Manual)
        );
    }

    #[test]
    fn a_per_unit_entry_reads_as_a_markup_over_its_unit_tt() {
        let percent = manual(ManualMarkupKind::Percent, 135.0, 0.0);
        assert_eq!(percent.markup_pct(), Some(135.0));
        let mut per_unit = manual(ManualMarkupKind::PedPerUnit, 2.5, 0.0);
        per_unit.unit_tt_ped = Some(5.0);
        assert_eq!(per_unit.markup_pct(), Some(150.0));
        // A zero-TT item has only an absolute value: no percentage.
        per_unit.unit_tt_ped = Some(0.0);
        assert_eq!(per_unit.markup_pct(), None);
        per_unit.unit_tt_ped = None;
        assert_eq!(per_unit.markup_pct(), None);
    }

    const ITEM_PASTE: &str = "Animal Oil Residue\t0\t101.000%\t5.000 PED\t104.000%\t50.000 PED\t\
103.000%\t200.000 PED\t102.000%\t900.000 PED\t102.000%\t2.000K PED\n\
Nanocube\t0\t101.000%\t100.000 PED\t100.840%\t200.000 PED\t\
100.650%\t300.000 PED\t100.820%\t400.000 PED\t101.210%\t500.000 PED";

    fn lookup(
        runtime: &tokio::runtime::Runtime,
        service: &MarketService,
        names: &[&str],
    ) -> HarvestMarketData {
        runtime
            .block_on(service.item_markups(names.iter().map(|name| name.to_string()).collect()))
            .unwrap()
    }

    /// A manual entry replaces the import while it is the newer of the two
    /// and gives way to a later import; the market reading stays on the row
    /// either way, and clearing the entry falls back to the import.
    #[test]
    fn manual_markups_and_imports_replace_each_other_by_recency() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        let item = |data: &HarvestMarketData| data.items[0].clone();

        let data = lookup(&runtime, &service, &["Animal Oil Residue"]);
        assert_eq!(item(&data).preferred, Some(MarkupSource::Market));
        assert_eq!(item(&data).preferred_markup_pct(), Some(104.0));

        clock.advance(60.0).unwrap();
        runtime
            .block_on(
                service.set_manual_markup("Animal Oil Residue", ManualMarkupInput::Percent(130.0)),
            )
            .unwrap();
        let data = lookup(&runtime, &service, &["Animal Oil Residue"]);
        assert_eq!(item(&data).preferred, Some(MarkupSource::Manual));
        assert_eq!(item(&data).preferred_markup_pct(), Some(130.0));
        assert_eq!(
            item(&data).markup_pct,
            Some(104.0),
            "the market reading stays"
        );

        clock.advance(60.0).unwrap();
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        let data = lookup(&runtime, &service, &["Animal Oil Residue"]);
        assert_eq!(item(&data).preferred, Some(MarkupSource::Market));
        assert!(item(&data).manual.is_some(), "the entry stands, just older");

        clock.advance(60.0).unwrap();
        runtime
            .block_on(service.set_manual_markup(
                "Animal Oil Residue",
                ManualMarkupInput::PedPerUnit {
                    ped: 0.05,
                    unit_tt_ped: Some(0.1),
                },
            ))
            .unwrap();
        let data = lookup(&runtime, &service, &["Animal Oil Residue"]);
        assert_eq!(item(&data).preferred, Some(MarkupSource::Manual));
        assert!((item(&data).preferred_markup_pct().unwrap() - 150.0).abs() < 1e-9);

        assert!(runtime
            .block_on(service.clear_manual_markup("Animal Oil Residue"))
            .unwrap());
        let data = lookup(&runtime, &service, &["Animal Oil Residue"]);
        assert_eq!(item(&data).manual, None);
        assert_eq!(item(&data).preferred, Some(MarkupSource::Market));
        // Nothing standing: a second clear writes nothing.
        assert!(!runtime
            .block_on(service.clear_manual_markup("Animal Oil Residue"))
            .unwrap());
    }

    /// An item the market never covered is valued by its manual entry
    /// alone, and the Nanocube floor follows the same rule as any item.
    #[test]
    fn a_manual_entry_covers_an_unlisted_item_and_the_nanocube_floor() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        clock.advance(60.0).unwrap();
        runtime
            .block_on(
                service.set_manual_markup("Rare Trade Item", ManualMarkupInput::Percent(400.0)),
            )
            .unwrap();
        runtime
            .block_on(service.set_manual_markup("Nanocube", ManualMarkupInput::Percent(101.5)))
            .unwrap();
        let data = lookup(&runtime, &service, &["Rare Trade Item", "Unseen Item"]);
        assert_eq!(data.nanocube_markup_pct, Some(101.5));
        let rare = data
            .items
            .iter()
            .find(|i| i.item_name == "Rare Trade Item")
            .unwrap();
        assert_eq!(rare.markup_pct, None);
        assert_eq!(rare.preferred, Some(MarkupSource::Manual));
        assert_eq!(rare.preferred_markup_pct(), Some(400.0));
        let unseen = data
            .items
            .iter()
            .find(|i| i.item_name == "Unseen Item")
            .unwrap();
        assert_eq!(unseen.preferred, None);
    }

    /// The live Nanocube mark stands across imports, until it is lifted or
    /// the player enters a value for the item. Shrapnel takes neither.
    #[test]
    fn the_live_nanocube_mark_outlasts_imports_until_lifted_or_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, clock) = rig(dir.path());
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        let marked = |names: &[&str]| lookup(&runtime, &service, names).items[0].nanocube_marked;

        assert!(runtime
            .block_on(service.set_live_nanocube_mark("Animal Oil Residue", true))
            .unwrap());
        assert!(marked(&["Animal Oil Residue"]));
        // Marking again is harmless.
        runtime
            .block_on(service.set_live_nanocube_mark("Animal Oil Residue", true))
            .unwrap();
        clock.advance(60.0).unwrap();
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        assert!(
            marked(&["Animal Oil Residue"]),
            "a newer import leaves the mark"
        );

        runtime
            .block_on(service.set_live_nanocube_mark("Animal Oil Residue", false))
            .unwrap();
        assert!(!marked(&["Animal Oil Residue"]));

        runtime
            .block_on(service.set_live_nanocube_mark("Animal Oil Residue", true))
            .unwrap();
        runtime
            .block_on(
                service.set_manual_markup("Animal Oil Residue", ManualMarkupInput::Percent(110.0)),
            )
            .unwrap();
        assert!(
            !marked(&["Animal Oil Residue"]),
            "entering a value lifts the mark"
        );

        for refused in [
            runtime
                .block_on(service.set_live_nanocube_mark("Shrapnel", true))
                .map(|_| ()),
            runtime
                .block_on(service.set_manual_markup("shrapnel", ManualMarkupInput::Percent(150.0)))
                .map(|_| ()),
        ] {
            assert!(matches!(refused, Err(MarketError::InvalidInput(_))));
        }
    }

    #[test]
    fn manual_markups_refuse_values_that_are_not_markups() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        for input in [
            ManualMarkupInput::Percent(0.0),
            ManualMarkupInput::Percent(f64::NAN),
            ManualMarkupInput::PedPerUnit {
                ped: -1.0,
                unit_tt_ped: None,
            },
            ManualMarkupInput::PedPerUnit {
                ped: 1.0,
                unit_tt_ped: Some(-1.0),
            },
        ] {
            assert!(matches!(
                runtime.block_on(service.set_manual_markup("Item", input)),
                Err(MarketError::InvalidInput(_))
            ));
        }
        assert!(matches!(
            runtime.block_on(service.set_manual_markup("  ", ManualMarkupInput::Percent(120.0))),
            Err(MarketError::InvalidInput(_))
        ));
    }

    /// A lookup by name serves exactly the named items, trimmed and
    /// de-duplicated, and refuses a catalogue-sized request.
    #[test]
    fn a_lookup_by_name_serves_exactly_the_named_items() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        let data = lookup(
            &runtime,
            &service,
            &[" Animal Oil Residue ", "Animal Oil Residue", ""],
        );
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.items[0].item_name, "Animal Oil Residue");
        assert_eq!(data.nanocube_markup_pct, Some(100.84));
        let too_many = vec!["x".to_string(); MAX_LOOKUP_NAMES + 1];
        assert!(matches!(
            runtime.block_on(service.item_markups(too_many)),
            Err(MarketError::InvalidInput(_))
        ));
    }

    /// Every committed write announces itself; a write that changed
    /// nothing (clearing an absent entry) does not.
    #[test]
    fn market_writes_announce_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, service, _clock) = rig(dir.path());
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = count.clone();
        let service = service.with_changed(Arc::new(move || {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let announced = || count.load(std::sync::atomic::Ordering::SeqCst);
        runtime.block_on(service.commit_paste(ITEM_PASTE)).unwrap();
        assert_eq!(announced(), 1);
        runtime
            .block_on(service.set_unit_price("Voucher", 0.5))
            .unwrap();
        assert_eq!(announced(), 2);
        runtime
            .block_on(service.set_manual_markup("Item", ManualMarkupInput::Percent(120.0)))
            .unwrap();
        assert_eq!(announced(), 3);
        runtime
            .block_on(service.clear_manual_markup("Item"))
            .unwrap();
        assert_eq!(announced(), 4);
        runtime
            .block_on(service.clear_manual_markup("Item"))
            .unwrap();
        assert_eq!(announced(), 4);
        runtime
            .block_on(service.set_live_nanocube_mark("Item", true))
            .unwrap();
        assert_eq!(announced(), 5);
    }
}
