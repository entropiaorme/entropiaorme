//! The market family: the manual markup-observation feed (paste
//! preview and commit) and its reads (overview, per-item history).
//!
//! Everything on this surface is the INFORMATIONAL market layer:
//! estimated markup is a distinct data class that never joins the
//! ledger, the analytics aggregates, or any realised P&L figure. The
//! DTOs stay in this module so the accounting surfaces cannot consume
//! them by accident.

use crate::equipment::{EquipmentExpectedReturn, EquipmentExpectedReturns, EquipmentKind};
use eo_services::expected_hunting::HuntingLooterLevels;
use eo_services::market_paste::{self, MarketPasteRow};
use eo_services::market_service::{self, MarketError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Nullable;
use crate::{Api, ApiError};

// ── Wire vocabulary ─────────────────────────────────────────────────

/// One of the export's five aggregation horizons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MarketHorizon {
    Day,
    Week,
    Month,
    Year,
    Decade,
}

impl From<MarketHorizon> for market_paste::MarketHorizon {
    fn from(value: MarketHorizon) -> Self {
        match value {
            MarketHorizon::Day => Self::Day,
            MarketHorizon::Week => Self::Week,
            MarketHorizon::Month => Self::Month,
            MarketHorizon::Year => Self::Year,
            MarketHorizon::Decade => Self::Decade,
        }
    }
}

// ── Response DTOs ───────────────────────────────────────────────────

/// One horizon's reading: the markup percentage (null where the game
/// reported N/A, meaning no sales in that horizon) and the sales
/// volume in PED.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketReading {
    pub markup_pct: Nullable<f64>,
    pub sales_ped: f64,
}

impl From<market_paste::MarketReading> for MarketReading {
    fn from(value: market_paste::MarketReading) -> Self {
        Self {
            markup_pct: value.markup_pct.into(),
            sales_ped: value.sales_ped,
        }
    }
}

/// One parsed item row: the five horizon readings by name.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketPastePreviewRow {
    pub item_name: String,
    pub tier: i64,
    pub day: MarketReading,
    pub week: MarketReading,
    pub month: MarketReading,
    pub year: MarketReading,
    pub decade: MarketReading,
}

impl MarketPastePreviewRow {
    fn from_row(row: &MarketPasteRow) -> Self {
        let (day, week, month, year, decade) = horizon_fields(row.readings);
        Self {
            item_name: row.item_name.clone(),
            tier: row.tier,
            day,
            week,
            month,
            year,
            decade,
        }
    }
}

/// The five converted readings in horizon order, ready to land on the
/// named DTO fields; the one place the array-to-fields mapping lives,
/// so the preview and overview rows stay in lock-step.
fn horizon_fields(
    readings: [market_paste::MarketReading; 5],
) -> (
    MarketReading,
    MarketReading,
    MarketReading,
    MarketReading,
    MarketReading,
) {
    let [day, week, month, year, decade] = readings;
    (
        day.into(),
        week.into(),
        month.into(),
        year.into(),
        decade.into(),
    )
}

/// One line the parser could not use, for the review flow.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketSkippedLine {
    /// 1-based line number within the paste.
    pub line_number: i64,
    pub content: String,
    pub reason: String,
}

/// The parse preview: what a commit of the same text would store, and
/// what it would ignore.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketPastePreview {
    pub rows: Vec<MarketPastePreviewRow>,
    pub skipped: Vec<MarketSkippedLine>,
}

/// A committed paste: the submission it created.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketCommitResult {
    pub submission_id: i64,
    pub item_count: i64,
    pub skipped_count: i64,
    /// Epoch seconds.
    pub observed_at: f64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketUnitPriceResult {
    pub item_name: String,
    pub ped_per_unit: f64,
    /// Epoch seconds.
    pub observed_at: f64,
}

/// One overview row: an item's latest readings (from the most recent
/// submission that carried it) and when they were observed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketOverviewRow {
    pub item_name: String,
    pub tier: i64,
    /// Epoch seconds of the submission the readings came from (the
    /// staleness signal).
    pub observed_at: f64,
    pub day: MarketReading,
    pub week: MarketReading,
    pub month: MarketReading,
    pub year: MarketReading,
    pub decade: MarketReading,
}

/// One point of an item's per-horizon history, oldest first.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketHistoryPoint {
    /// Epoch seconds.
    pub observed_at: f64,
    pub markup_pct: Nullable<f64>,
    pub sales_ped: f64,
}

/// One item of a contributable batch: the pasted readings verbatim,
/// by horizon name.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketContributionItem {
    pub item_name: String,
    pub tier: i64,
    pub day: MarketReading,
    pub week: MarketReading,
    pub month: MarketReading,
    pub year: MarketReading,
    pub decade: MarketReading,
}

/// The most recent accepted paste as a contributable batch: exactly
/// what an opted-in contributor shares, nothing more. The frontend
/// owns the send, strictly behind the contribution opt-in.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketContributionBatch {
    /// Epoch seconds of the submission.
    pub observed_at: f64,
    pub items: Vec<MarketContributionItem>,
}

/// One species' estimated-markup row: recorded loot composition
/// TT-weighted by the latest markup observations on one horizon.
/// Coverage keeps a thin sample honest: the estimate weights only the
/// covered TT, and the row says how much that is.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketMobRankingRow {
    pub mob_species: String,
    pub loot_tt: f64,
    pub covered_tt: f64,
    pub item_count: i64,
    pub covered_item_count: i64,
    pub est_markup_pct: Nullable<f64>,
}

/// One horizon's reading for a harvest item: its markup (null where the
/// game reported N/A) and TT turnover (PED).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketHarvestHorizon {
    /// "day" | "week" | "month" | "year".
    pub horizon: String,
    pub markup_pct: Nullable<f64>,
    pub sales_ped: f64,
}

/// One harvest-looted item's resolved market signals plus the per-horizon
/// breakdown. The resolved markup/horizon/sales are the display default
/// and market-opportunity input (week preferred, then month, then year;
/// all null when no observation covers the item). `readings` carries every
/// horizon (day, week, month, year) for the detail view.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketHarvestItem {
    pub item_name: String,
    pub markup_pct: Nullable<f64>,
    /// Absolute informational quote per item unit. This is intended for
    /// zero-TT and unit-priced items, and remains estimated market value.
    pub unit_price_ped: Nullable<f64>,
    pub horizon: Nullable<String>,
    pub sales_ped: Nullable<f64>,
    /// Fee-efficient TT packet at the resolved direct-market markup. This
    /// deliberately excludes turnover: capacity is a separate market signal.
    pub recommended_packet_tt: Nullable<f64>,
    /// Every horizon's reading, ordered day, week, month, year.
    pub readings: Vec<MarketHarvestHorizon>,
}

/// The estimated market signals for the harvest-looted items, plus the
/// nanocube recycling floor. Markup is item-intrinsic, so this is a flat
/// per-item list; the frontend merges it with the per-tool composition,
/// derives holding-independent market opportunity, and computes the
/// current-market aggregates. Estimated markup is informational only,
/// never a realised figure.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketHarvestData {
    /// The nanocube item's resolved markup (percent): the universal
    /// recycling floor. Null when no nanocube observation exists (the
    /// frontend then falls back to a constant).
    pub nanocube_markup_pct: Nullable<f64>,
    pub items: Vec<MarketHarvestItem>,
}

/// Gross markup needed for the auction listing fee to consume no more than
/// the requested share. Item packet TT is this amount divided by its premium.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketAuctionPacketThreshold {
    pub max_fee_share_pct: f64,
    pub gross_markup_ped: f64,
}

/// One looter profession and its believed-current level.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketLooterLevel {
    pub name: String,
    pub level: f64,
}

/// One configured loadout's offensive-only expected return and break-even
/// loot markup under Community Model v1.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketWeaponBreakEven {
    pub name: String,
    pub amplifier_name: Nullable<String>,
    pub weighted_efficiency_pct: Nullable<f64>,
    pub offensive_tt_recovery_pct: Nullable<f64>,
    /// The looter-dependent figures at each basis the player can choose;
    /// null when the loadout has no modelled offensive stream.
    pub by_looter: Nullable<MarketBreakEvenByLooter>,
    pub coverage: Nullable<f64>,
    pub incomplete: bool,
    pub model_version: Nullable<String>,
}

/// A loadout's expected return and break-even loot markup at one looter
/// basis.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketLooterBreakEven {
    pub looter_level: f64,
    pub expected_tt_return_pct: Nullable<f64>,
    pub break_even_loot_markup_pct: Nullable<f64>,
}

/// One loadout's break-even figures at every looter basis: the
/// three-looter mean, or the one hunting looter a known target class
/// applies.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketBreakEvenByLooter {
    pub three_looter_mean: MarketLooterBreakEven,
    pub animal: MarketLooterBreakEven,
    pub mutant: MarketLooterBreakEven,
    pub robot: MarketLooterBreakEven,
}

impl From<&EquipmentExpectedReturn> for MarketLooterBreakEven {
    fn from(expected: &EquipmentExpectedReturn) -> Self {
        Self {
            looter_level: expected.looter_level,
            expected_tt_return_pct: expected
                .expected_tt_rate
                .0
                .map(|value| value * 100.0)
                .into(),
            break_even_loot_markup_pct: expected
                .break_even_loot_markup
                .0
                .map(|value| value * 100.0)
                .into(),
        }
    }
}

impl From<&EquipmentExpectedReturns> for MarketBreakEvenByLooter {
    fn from(returns: &EquipmentExpectedReturns) -> Self {
        Self {
            three_looter_mean: (&returns.three_looter_mean).into(),
            animal: (&returns.animal).into(),
            mutant: (&returns.mutant).into(),
            robot: (&returns.robot).into(),
        }
    }
}

/// The break-even readout: the player's exact three hunting-looter
/// professions and every library weapon's modelled offensive economics.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarketBreakEven {
    pub looters: Vec<MarketLooterLevel>,
    pub weapons: Vec<MarketWeaponBreakEven>,
}

// ── Operations ──────────────────────────────────────────────────────

impl Api {
    /// Resolve a user-selected listing-fee share through the measured auction
    /// fee curve. Keeping this calculation behind the typed API prevents UI
    /// clients from acquiring their own approximation of the game's curve.
    pub fn market_auction_packet_threshold(
        &self,
        max_fee_share_pct: f64,
    ) -> Result<MarketAuctionPacketThreshold, ApiError> {
        let gross_markup_ped =
            eo_services::auction_fee::minimum_efficient_gross_markup_ped(max_fee_share_pct)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "max_fee_share_pct must be greater than 0 and at most 100",
                    )
                })?;
        Ok(MarketAuctionPacketThreshold {
            max_fee_share_pct,
            gross_markup_ped,
        })
    }

    /// Parse a market-ledger paste without storing anything: the
    /// preview leg of the review-before-accept flow.
    pub async fn market_paste_preview(&self, text: String) -> Result<MarketPastePreview, ApiError> {
        let parse = self.market.preview(&text);
        Ok(MarketPastePreview {
            rows: parse
                .rows
                .iter()
                .map(MarketPastePreviewRow::from_row)
                .collect(),
            skipped: parse
                .skipped
                .into_iter()
                .map(|line| MarketSkippedLine {
                    line_number: line.line_number as i64,
                    content: line.content,
                    reason: line.reason,
                })
                .collect(),
        })
    }

    /// Parse and commit a market-ledger paste as one submission. The
    /// text is re-parsed server-side; the preview is never trusted as
    /// an echo.
    pub async fn market_paste_commit(&self, text: String) -> Result<MarketCommitResult, ApiError> {
        let outcome = self
            .market
            .commit_paste(&text)
            .await
            .map_err(|err| match err {
                MarketError::EmptyPaste | MarketError::InvalidInput(_) => {
                    ApiError::bad_request(err.to_string())
                }
                MarketError::Db(source) => ApiError::internal("market paste commit")(source),
            })?;
        Ok(MarketCommitResult {
            submission_id: outcome.submission_id,
            item_count: outcome.item_count as i64,
            skipped_count: outcome.skipped_count as i64,
            observed_at: outcome.observed_at,
        })
    }

    /// Store an informational PED-per-unit quote for a zero-TT or
    /// unit-priced item. This quote never enters realised accounting.
    pub async fn market_unit_price_set(
        &self,
        item_name: String,
        ped_per_unit: f64,
    ) -> Result<MarketUnitPriceResult, ApiError> {
        let canonical_name = item_name.trim().to_string();
        let observed_at = self
            .market
            .set_unit_price(&canonical_name, ped_per_unit)
            .await
            .map_err(|err| match err {
                MarketError::EmptyPaste | MarketError::InvalidInput(_) => {
                    ApiError::bad_request(err.to_string())
                }
                MarketError::Db(source) => ApiError::internal("market unit price set")(source),
            })?;
        Ok(MarketUnitPriceResult {
            item_name: canonical_name,
            ped_per_unit,
            observed_at,
        })
    }

    /// Every observed item's latest readings, sorted by item name.
    pub async fn market_overview(&self) -> Result<Vec<MarketOverviewRow>, ApiError> {
        let rows = self
            .market
            .overview()
            .await
            .map_err(ApiError::internal("market overview"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let (day, week, month, year, decade) = horizon_fields(row.readings);
                MarketOverviewRow {
                    item_name: row.item_name,
                    tier: row.tier,
                    observed_at: row.observed_at,
                    day,
                    week,
                    month,
                    year,
                    decade,
                }
            })
            .collect())
    }

    /// One item's observations over time on one horizon, oldest first.
    pub async fn market_item_history(
        &self,
        item_name: String,
        horizon: MarketHorizon,
    ) -> Result<Vec<MarketHistoryPoint>, ApiError> {
        let points: Vec<market_service::HistoryPoint> = self
            .market
            .item_history(&item_name, horizon.into())
            .await
            .map_err(ApiError::internal("market item history"))?;
        Ok(points
            .into_iter()
            .map(|point| MarketHistoryPoint {
                observed_at: point.observed_at,
                markup_pct: point.markup_pct.into(),
                sales_ped: point.sales_ped,
            })
            .collect())
    }

    /// The most recent accepted paste as a contributable batch, or null
    /// before the first commit.
    pub async fn market_contribution_batch(
        &self,
    ) -> Result<Nullable<MarketContributionBatch>, ApiError> {
        let batch = self
            .market
            .latest_submission()
            .await
            .map_err(ApiError::internal("market contribution batch"))?;
        Ok(batch
            .map(|batch| MarketContributionBatch {
                observed_at: batch.observed_at,
                items: batch
                    .items
                    .into_iter()
                    .map(|item| {
                        let (day, week, month, year, decade) = horizon_fields(item.readings);
                        MarketContributionItem {
                            item_name: item.item_name,
                            tier: item.tier,
                            day,
                            week,
                            month,
                            year,
                            decade,
                        }
                    })
                    .collect(),
            })
            .into())
    }

    /// Configured loadout economics under the shared component-weighted
    /// expected-hunting core. The three exact hunting looters are returned
    /// beside each row's figures at every basis, so the reader chooses the
    /// mean or the one looter their target's class applies.
    pub async fn market_break_even(&self) -> Result<MarketBreakEven, ApiError> {
        let professions = self.character_professions().await?;
        let profession_level = |name: &str| {
            professions
                .iter()
                .find(|profession| profession.name == name)
                .map(|profession| profession.level)
                .unwrap_or(0.0)
        };
        let looter_basis = HuntingLooterLevels {
            animal: profession_level("Animal Looter"),
            mutant: profession_level("Mutant Looter"),
            robot: profession_level("Robot Looter"),
        };
        let looters = ["Animal Looter", "Mutant Looter", "Robot Looter"]
            .into_iter()
            .map(|name| MarketLooterLevel {
                name: name.to_string(),
                level: profession_level(name),
            })
            .collect();

        let mut weapons = Vec::new();
        for item in self
            .equipment_library()
            .await?
            .into_iter()
            .filter(|item| matches!(item.kind, EquipmentKind::Weapon))
        {
            let item_id = item
                .id
                .parse::<i64>()
                .map_err(ApiError::internal("equipment id parse for break-even"))?;
            let detail = self
                .equipment_detail_with_looters(item_id, Some(looter_basis))
                .await?;
            let returns = detail.expected_returns.0;
            // The basis-independent figures read the same at every basis.
            let expected = returns.as_ref().map(|returns| &returns.three_looter_mean);
            weapons.push(MarketWeaponBreakEven {
                name: item.name,
                amplifier_name: item.amplifier_name,
                weighted_efficiency_pct: expected
                    .and_then(|value| value.weighted_efficiency_pct.0)
                    .into(),
                offensive_tt_recovery_pct: expected
                    .and_then(|value| value.offensive_tt_recovery.0)
                    .map(|value| value * 100.0)
                    .into(),
                by_looter: returns.as_ref().map(Into::into).into(),
                coverage: expected.map(|value| value.coverage).into(),
                incomplete: expected.is_none_or(|value| value.incomplete),
                model_version: expected.map(|value| value.model_version.clone()).into(),
            });
        }

        Ok(MarketBreakEven { looters, weapons })
    }

    /// Every hunted species' estimated loot markup on one horizon,
    /// best estimate first (species with no observed items last).
    pub async fn market_mob_ranking(
        &self,
        horizon: MarketHorizon,
    ) -> Result<Vec<MarketMobRankingRow>, ApiError> {
        let rows = self
            .market
            .mob_ranking(horizon.into())
            .await
            .map_err(ApiError::internal("market mob ranking"))?;
        Ok(rows
            .into_iter()
            .map(|row| MarketMobRankingRow {
                mob_species: row.mob_species,
                loot_tt: row.loot_tt,
                covered_tt: row.covered_tt,
                item_count: row.item_count,
                covered_item_count: row.covered_item_count,
                est_markup_pct: row.est_markup_pct.into(),
            })
            .collect())
    }

    /// The estimated market signals for every active harvest-looted item
    /// plus the nanocube recycling floor. Each item's markup and sales
    /// volume resolve via the week -> month -> year horizon fallback.
    pub async fn market_harvest_markups(&self) -> Result<MarketHarvestData, ApiError> {
        let data = self
            .market
            .harvest_markups()
            .await
            .map_err(ApiError::internal("market harvest markups"))?;
        Ok(market_items_dto(data))
    }

    /// The estimated market signals for every active hunting-looted item
    /// plus the nanocube recycling floor: the Hunting sibling of
    /// [`Self::market_harvest_markups`], over the kill loot composition
    /// (enhancer-shrapnel returns excluded).
    pub async fn market_hunt_markups(&self) -> Result<MarketHarvestData, ApiError> {
        let data = self
            .market
            .hunt_markups()
            .await
            .map_err(ApiError::internal("market hunt markups"))?;
        Ok(market_items_dto(data))
    }
}

/// Shape one activity's per-item market signals onto the wire DTO.
fn market_items_dto(data: eo_services::market_service::HarvestMarketData) -> MarketHarvestData {
    MarketHarvestData {
        nanocube_markup_pct: data.nanocube_markup_pct.into(),
        items: data
            .items
            .into_iter()
            .map(|item| MarketHarvestItem {
                item_name: item.item_name,
                markup_pct: item.markup_pct.into(),
                unit_price_ped: item.unit_price_ped.into(),
                horizon: item.horizon.into(),
                sales_ped: item.sales_ped.into(),
                recommended_packet_tt: item.recommended_packet_tt.into(),
                readings: item
                    .readings
                    .into_iter()
                    .map(|r| MarketHarvestHorizon {
                        horizon: r.horizon,
                        markup_pct: r.markup_pct.into(),
                        sales_ped: r.sales_ped,
                    })
                    .collect(),
            })
            .collect(),
    }
}
