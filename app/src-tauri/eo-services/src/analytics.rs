//! The analytics domain service: the Overview and Activity aggregates, the
//! ledger (keyset-paginated list + create/delete), the ledger presets, and
//! the inventory ledger (list / create / patch / delete / sell).
//!
//! This is domain logic, extracted from the transport layer that used to
//! host it inline: the same SQL aggregation and camelCase-ready value
//! shaping, now behind [`AnalyticsService`] over the shared database and
//! injected clock, so both the typed IPC facade and the guide-mode demo
//! surface read through one implementation. Reads scale O(days), not
//! O(rows): the Overview brings the daily rollups current and aggregates
//! rollup rows plus bounded raw edges (see [`hybrid_window`]).
//!
//! The aggregates carry the engine's numeric typing internally as
//! [`SqlNumber`]: an empty `COALESCE(SUM(...), 0)` reads as the exact
//! integer `0`, integer sums stay exact, and rounding applies only to
//! floats. The response boundary declares `f64` fields, so every number
//! coerces to its float form exactly where the facade DTOs pin it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use crate::clock::Clock;
use crate::daily_rollup;
use crate::db::{Db, DbError};
use crate::harvest_yield::HarvestYieldTier;
use crate::stock_allocation;
use crate::time::{naive_to_epoch, to_iso_utc};

/// The analytics domain service over the shared database and injected
/// clock: the Overview / Activity aggregates and the ledger / preset /
/// inventory CRUD.
pub struct AnalyticsService {
    db: Db,
    clock: Arc<dyn Clock>,
}

/// A page of ledger entries (newest first) plus the opaque cursor for the
/// following page (`None` on the last page) and the whole-table row count
/// (so a pager can report true bounds while loading windows on demand);
/// the cursor is the base64url keyset token.
pub struct LedgerPage {
    pub entries: Vec<LedgerRow>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

/// One ledger entry, in wire shape (`type` is the stored kind).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LedgerRow {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// One ledger preset, in wire shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PresetRow {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// One inventory item, in wire shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryRow {
    pub id: String,
    pub name: String,
    pub tt_value: f64,
    pub markup_paid: f64,
    pub notes: Option<String>,
    pub acquired_at: String,
}

/// One holding candidate for a manually typed or OCR-observed item name.
/// A candidate is a proposal only: callers commit against its stable reference,
/// never against the observed string.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryMatchRow {
    pub kind: String,
    pub holding_id: String,
    pub name: String,
    pub score: f64,
}

/// One canonical item's current position: recorded loot still held, after
/// everything that has left through a listing or a conversion and back
/// through an expiry. Position context only: it never feeds market
/// opportunity or its confidence levels, which stay holding-independent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockPositionRow {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Quantity currently sitting in an unresolved auction listing. Already
    /// out of `quantity`: it has left the player's inventory and may still
    /// come back, so it is reported rather than silently absent.
    pub listed_quantity: f64,
}

/// One auction listing across its whole lifecycle.
///
/// Realised markup is derived on read from the listing's own numbers rather
/// than stored, so it cannot drift from the fees and price it was resolved
/// with. `None` until the listing is confirmed sold.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuctionListingRow {
    pub id: String,
    pub item_name: String,
    pub quantity: f64,
    pub attributed_qty: f64,
    pub unattributed_qty: f64,
    pub tt_value: f64,
    pub attributed_tt: f64,
    pub starting_bid: f64,
    pub buyout: Option<f64>,
    pub listing_fee: f64,
    pub listed_at: String,
    pub status: String,
    pub final_price: Option<f64>,
    pub sale_fee: Option<f64>,
    pub resolved_at: Option<String>,
    /// The position family this listing consumes. Loot uses the weighted
    /// provenance pool; equipment points at one stable capital holding.
    pub subject_kind: String,
    pub inventory_item_id: Option<String>,
    /// Acquisition basis for equipment. Loot has no capital basis here: its
    /// TT was recognised when acquired and only markup is realised on sale.
    pub cost_basis: Option<f64>,
    /// `auction` for a pending lifecycle, `trade` for an immediately resolved
    /// fee-free player-to-player sale.
    pub channel: String,
    /// How many days the listing was posted for, when the player recorded it.
    /// `None` for a listing made before durations were captured: no deadline
    /// is invented for it.
    pub auction_days: Option<i64>,
    /// The day the listing runs out, derived from `listed_at` plus
    /// `auction_days` rather than stored, so it cannot drift from either.
    /// `None` whenever the duration is unknown.
    pub expires_at: Option<String>,
    /// Net markup the activity may claim, after both auction fees and after
    /// removing the share covered by untracked stock.
    pub activity_net_markup: Option<f64>,
    /// Gross sale proceeds above the listing's TT, before fees.
    pub gross_markup: Option<f64>,
}

/// One thing this activity did to its stock: an auction listing across its
/// whole lifecycle, a private trade, a conversion, or a stock-only removal.
///
/// A listing is one entry whatever state it reaches. Creating it and selling
/// it are not two events to be listed separately; they are the same listing,
/// later on.
///
/// Whether an entry can be undone is decided here rather than in the UI,
/// because the answer depends on what the rest of the ledger has since done
/// with the stock. `undo_blocked_reason` is written for the person reading it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHistoryRow {
    pub id: String,
    /// The holding family this outcome acted on: `loot` or `equipment`.
    pub subject_kind: String,
    /// The operational path: `auction`, `trade`, `conversion`, or `removal`.
    pub channel: String,
    /// `listing`, `trade`, `conversion`, or `removal`.
    pub kind: String,
    /// `pending`, `sold`, `expired`, `converted`, or `removed`.
    pub status: String,
    pub item_name: String,
    /// What a conversion produced. `None` for other outcomes.
    pub target_item: Option<String>,
    /// The date the entry currently stands at: when a listing resolved, or
    /// when it was listed if it has not, and when a conversion happened.
    pub occurred_at: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Realised outcomes only: the whole gain, and the part an activity may claim.
    pub net_markup: Option<f64>,
    pub activity_net_markup: Option<f64>,
    /// Sold listings only: whether the sale can be taken back, leaving the
    /// listing open. Never blocked, since no stock moves.
    pub can_revert_sale: bool,
    /// Whether the entry can be undone outright, returning any stock it took.
    pub can_delete: bool,
    pub undo_blocked_reason: Option<String>,
    /// Already undone: kept on the list as the record of a correction, with
    /// every effect it had reversed and no action left on it.
    pub undone: bool,
}

/// One yield tier's realised markup from confirmed stock outcomes, for the Tree
/// Cutting Realised figures.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealisedTierMarkup {
    pub yield_tier: HarvestYieldTier,
    pub net_markup: f64,
}

/// One mob species' realised markup from confirmed stock outcomes, for the Hunting
/// Realised figures. The Hunting sibling of [`RealisedTierMarkup`]: the
/// species is Hunting's observed source axis the way the tier is Tree
/// Cutting's.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealisedSpeciesMarkup {
    pub mob_species: String,
    pub net_markup: f64,
}

/// One session definition's net realised markup from confirmed stock outcomes. This
/// is a second projection of the same immutable allocations used by species,
/// never a second gain.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealisedDefinitionMarkup {
    pub definition_id: i64,
    pub net_markup: f64,
}

/// The activity family a stock action belongs to. Listings have carried this
/// since the auction lifecycle landed; the vocabulary is closed so a typo'd
/// caller cannot mint a third activity by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profession {
    Harvesting,
    Hunting,
    /// The central Inventory command surface. This stamps where an action was
    /// initiated, never what activity may claim its result; provenance owns
    /// that attribution.
    Inventory,
}

impl Profession {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Harvesting => "harvesting",
            Self::Hunting => "hunting",
            Self::Inventory => "inventory",
        }
    }
}

/// A realised inventory sale: the ledger entry it wrote (`None` for a
/// zero-delta sale) and the sold item.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventorySale {
    pub ledger_entry: Option<LedgerRow>,
    pub sold_item: InventoryRow,
}

/// The Overview aggregate. Response numerics are in their float form
/// except the cycled split, which keeps its engine typing
/// ([`SqlNumber`]: an empty `COALESCE` sum is the exact integer zero).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewData {
    pub total_return_rate: f64,
    pub trend: &'static str,
    pub returns_breakdown: ReturnsData,
    pub losses_breakdown: LossesData,
    pub total_gains: f64,
    pub total_losses: f64,
    pub timeline: Vec<TimelinePoint>,
    pub monthly_breakdown: Vec<TimelinePoint>,
}

/// The liquid + progression returns breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnsData {
    pub loot_tt: f64,
    pub quest_item_tt: f64,
    pub pes: f64,
    pub codex_pes: f64,
    pub quest_pes: f64,
    pub ledger: std::collections::BTreeMap<String, f64>,
}

/// The losses breakdown: tracking cost, its cycled split, and the
/// ledger expenses.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LossesData {
    pub tracking_cost: f64,
    pub cycled_breakdown: CycledData,
    pub ledger: std::collections::BTreeMap<String, f64>,
}

/// The per-family cycled-cost split, engine-typed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycledData {
    pub weapon: SqlNumber,
    pub healing: SqlNumber,
    pub enhancer: SqlNumber,
    pub armour: SqlNumber,
    pub dangling: SqlNumber,
    /// Harvesting (tree cutting) swing decay.
    pub harvest: SqlNumber,
    /// Consumed doses of cost-tracked items.
    pub consumables: SqlNumber,
}

/// One day or month of the Overview timeline; the caller labels the
/// bucket (`date` for days, `month` for months).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePoint {
    pub bucket: String,
    pub loot_tt: f64,
    pub quest_item_tt: f64,
    pub pes: f64,
    pub codex_pes: f64,
    pub quest_pes: f64,
    pub ledger_gains: std::collections::BTreeMap<String, f64>,
    pub tracking_cost: f64,
    pub ledger_losses: std::collections::BTreeMap<String, f64>,
}

/// The Hunting aggregate: the per-mob and per-session-name comparison
/// tables (the observed and designated axes).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingData {
    pub mob_comparisons: Vec<ActivityRow>,
    pub name_comparisons: Vec<ActivityRow>,
}

/// The Tree Cutting aggregate: effective yield tiers and their loot
/// composition.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestData {
    pub tier_comparisons: Vec<HarvestTierRow>,
}

/// One effective yield activity and its item composition.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestTierRow {
    pub yield_tier: HarvestYieldTier,
    pub swings: i64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub loot_items: Vec<HarvestLootItemRow>,
}

/// One item in a yield tier's harvest loot composition: realised TT figures
/// only (markup is the market layer's, merged in at the frontend).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestLootItemRow {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

/// One row of a Hunting comparison table; the caller labels the name
/// (`mobName` / `sessionName`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub name: String,
    pub sessions: i64,
    pub kills: i64,
    pub hours: f64,
    pub cycled: f64,
    pub pes_per100_ped: f64,
    pub loot_rate: f64,
}

// ── The revamped Hunting aggregate ──
//
// Two honest axes over the session-foundation substrate, replacing the
// free-text name and dominant-mob tables. Sessions are keyed by session
// definition (the deliberate, user-authored axis); Targets are keyed by
// mob species (the observed axis, with maturity as a drilldown). Every
// figure here is DIRECT: weapon and enhancer cost at kill grain, loot TT
// at kill grain, and skill TT at session grain for sessions that hunted.
// Heal and armour stay session-grain residues and are deliberately not
// allocated into comparison rows; full sustainability lives on the
// Dashboard and Overview.

/// The Hunting activity aggregate for one period.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingActivityData {
    pub overall: HuntingOverall,
    /// One row per session definition with hunting evidence in the period,
    /// plus at most one unassigned bucket (`definition_id` None) carrying
    /// sessions recorded outside any definition.
    pub definitions: Vec<HuntingDefinitionRow>,
    /// One row per observed species, plus at most one unclassified bucket
    /// (empty species) for kills whose species the tracker never learned.
    pub species: Vec<HuntingSpeciesRow>,
}

/// The whole activity's direct headline figures for the period.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingOverall {
    pub sessions: i64,
    pub kills: i64,
    pub duration_hours: f64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub pes: f64,
    pub pes_per100_ped: f64,
    pub expected: Option<ExpectedHuntingAggregate>,
}

/// Offensive-only community-model aggregate over immutable tool phases.
/// Every value remains informational and separate from realised accounting.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedHuntingAggregate {
    pub model_version: String,
    pub looter_source: String,
    pub looter_level: f64,
    pub expected_loot_tt: f64,
    pub modelled_raw_tt: f64,
    pub eligible_offensive_cost: f64,
    pub offensive_tt_recovery: f64,
    pub expected_tt_rate: f64,
    pub effective_efficiency: crate::expected_hunting::EffectiveEfficiency,
    pub break_even_loot_markup: f64,
    pub coverage: f64,
    pub incomplete: bool,
    pub missing_basis_phases: i64,
}

/// One session definition's aggregate over its hunted instances.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingDefinitionRow {
    /// `None` for the unassigned bucket: sessions recorded before
    /// definitions existed, or deliberately started outside one.
    pub definition_id: Option<i64>,
    pub name: String,
    /// Archived definitions stay analytically visible; archive means "not
    /// currently offered for play", never "hide its history".
    pub is_archived: bool,
    pub instances: i64,
    pub kills: i64,
    pub duration_hours: f64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub pes: f64,
    pub pes_per100_ped: f64,
    pub expected: Option<ExpectedHuntingAggregate>,
    pub activities: Vec<HuntingSignatureRow>,
    pub mobs: Vec<HuntingMobShareRow>,
    pub instance_rows: Vec<HuntingInstanceRow>,
    /// Item composition of every qualifying instance of this definition.
    pub loot_items: Vec<HarvestLootItemRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingSignatureRow {
    pub kind: String,
    pub label: String,
    pub runs: i64,
    pub kills: i64,
    pub duration_hours: f64,
    pub cycled: f64,
    pub returns: f64,
    pub pes: f64,
    pub pes_per100_ped: f64,
    /// Offensive-only expected economics over the immutable phases stamped
    /// with this exact activity context. Family rows aggregate their variants;
    /// joint signatures remain indivisible.
    pub expected: Option<ExpectedHuntingAggregate>,
    /// Confirmed reward TT recorded separately from ordinary loot.
    pub confirmed_reward_ped: f64,
    /// Net markup realised when reward stock was later sold or converted.
    pub realised_reward_markup: f64,
    /// Net markup realised when ordinary loot from this exact context was
    /// later sold or converted. Internal accounting input for the API's
    /// existing realised-return total, not a separate public statistic.
    #[serde(skip)]
    pub realised_loot_markup: f64,
    /// Actual reward items observed at completion. Their current market
    /// projection is resolved outside the accounting service.
    pub reward_items: Vec<HarvestLootItemRow>,
    /// `none`, `included_in_loot`, `fixed_liquid`, `item`, `skill`, `mixed`,
    /// or `unverified` for completions predating immutable treatment.
    pub reward_status: String,
    pub loot_items: Vec<HarvestLootItemRow>,
    pub variants: Vec<HuntingSignatureRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingMobShareRow {
    pub mob_species: String,
    pub kills: i64,
    pub loot_tt: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingInstanceRow {
    pub session_id: String,
    pub started_at: f64,
    pub duration_hours: f64,
    pub kills: i64,
    pub cycled: f64,
    pub returns: f64,
    pub pes: f64,
}

/// One observed species' economic aggregate and loot composition.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingSpeciesRow {
    /// Empty for the unclassified bucket: kills whose species the tracker
    /// never learned (legacy tag-mode rows and unidentified nameplates).
    pub mob_species: String,
    pub kills: i64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub pes: Option<f64>,
    pub pes_per100_ped: Option<f64>,
    pub pes_sessions: i64,
    pub expected: Option<ExpectedHuntingAggregate>,
    pub maturities: Vec<HuntingMaturityRow>,
    /// Item composition of the species' loot, largest TT first. Enhancer
    /// shrapnel returns are enhancer accounting, not mob loot, and are
    /// excluded.
    pub loot_items: Vec<HarvestLootItemRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HuntingMaturityRow {
    pub maturity: String,
    pub kills: i64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
}

/// The analytics service's error surface. The two validation variants (a
/// malformed ledger cursor, an out-of-vocabulary preset type) carry their
/// pinned refusal messages verbatim and map to `ApiError::BadRequest` at
/// the facade; `Db` / `Storage` are the driver and rollup-refresh
/// failures, mapped to `ApiError::Internal`.
#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    #[error("Invalid cursor")]
    InvalidCursor,
    #[error("type must be 'expense' or 'markup'")]
    InvalidPresetType,
    /// A caller-supplied value the domain rejects before it reaches storage.
    #[error("{0}")]
    InvalidInput(&'static str),
    /// A request the domain refuses on the state it finds, carrying the reason
    /// in terms of that state. Distinct from [`Self::InvalidInput`], which
    /// rejects the argument itself and can say so in a fixed sentence.
    #[error("{0}")]
    Rejected(String),
    #[error(transparent)]
    Storage(#[from] DbError),
}

impl AnalyticsService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }
}

const ACTIVITY_DOMINANCE_THRESHOLD: f64 = 0.6;

// ── Engine-typed numeric primitives (the quest-analytics siblings in
//    eo-services::quests; kept local so this module stays self-contained,
//    matching the sibling families' per-file formatter convention) ──

/// A SQLite-engine-typed number: a REAL read stays a float and an INTEGER
/// read (including the `COALESCE(SUM(...), 0)` empty case) stays an exact
/// integer, so the engine's numeric typing is visible in the type rather
/// than carried through untyped JSON.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum SqlNumber {
    Int(i64),
    Float(f64),
}

impl SqlNumber {
    /// Read a numeric column preserving the engine type: the stored
    /// value's affinity (`ValueRef`) drives the branch, so a REAL sum
    /// stays a float and an integer sum (the NULL-sum zeros) stays an
    /// integer.
    fn read(row: &rusqlite::Row, index: usize) -> SqlNumber {
        match row.get_ref_unwrap(index) {
            rusqlite::types::ValueRef::Real(value) => SqlNumber::Float(value),
            value => SqlNumber::Int(
                value
                    .as_i64()
                    .expect("SqlNumber::read reads a numeric column"),
            ),
        }
    }

    /// A window's family sum: an absent (all-NULL) family is the exact
    /// integer zero.
    fn from_family(sum: Option<f64>) -> SqlNumber {
        sum.map_or(SqlNumber::Int(0), SqlNumber::Float)
    }

    /// The sum of two engine-typed numbers, integer (exact) when both are.
    fn sum(self, other: SqlNumber) -> SqlNumber {
        match (self, other) {
            (SqlNumber::Int(left), SqlNumber::Int(right)) => SqlNumber::Int(left + right),
            _ => SqlNumber::Float(self.as_f64() + other.as_f64()),
        }
    }

    /// Banker's rounding on a float; an integer is already exact.
    fn rounded(self, places: usize) -> SqlNumber {
        match self {
            SqlNumber::Float(value) => {
                SqlNumber::Float(eo_wire::normalizer::round_half_even(value, places))
            }
            int => int,
        }
    }

    /// The float form (the response boundary's declared type).
    pub fn as_f64(self) -> f64 {
        match self {
            SqlNumber::Int(value) => value as f64,
            SqlNumber::Float(value) => value,
        }
    }
}

/// `float(value)` over an engine-typed number (the activity path, where every
/// numeric is summed in float space).
fn as_float(row: &rusqlite::Row, index: usize) -> f64 {
    SqlNumber::read(row, index).as_f64()
}

// ── Period + WHERE helpers (mirroring `_period_epoch` / `_where` /
//    `_where_iso` / `_epoch_to_iso`) ──

/// Epoch start for a named period, or `None` for all-time (and for any
/// unrecognised value, exactly as the reference's `dict.get` miss).
fn period_epoch(period: &str, now: f64) -> Option<f64> {
    let days = match period {
        "30d" => 30.0,
        "90d" => 90.0,
        "1y" => 365.0,
        _ => return None,
    };
    Some(now - days * 86400.0)
}

/// `datetime.fromtimestamp(epoch, tz=UTC).strftime("%Y-%m-%d")`.
fn epoch_to_iso(epoch: f64) -> String {
    chrono::DateTime::from_timestamp(epoch.floor() as i64, 0)
        .expect("epoch within range")
        .format("%Y-%m-%d")
        .to_string()
}

/// WHERE clause + epoch params for a numeric (unix-timestamp) column.
fn where_epoch(col: &str, start: Option<f64>, end: Option<f64>) -> (String, Vec<f64>) {
    let mut parts = Vec::new();
    let mut params = Vec::new();
    if let Some(s) = start {
        parts.push(format!("{col} >= ?"));
        params.push(s);
    }
    if let Some(e) = end {
        parts.push(format!("{col} < ?"));
        params.push(e);
    }
    (
        if parts.is_empty() {
            "1=1".to_string()
        } else {
            parts.join(" AND ")
        },
        params,
    )
}

// ── The hybrid rollup/raw window split ──

/// The hybrid split of an epoch window against the daily-rollup
/// watermark: whole days at or below the watermark aggregate from
/// `daily_rollups` (O(days), not O(rows)); partial edge days and
/// everything past the watermark aggregate from the raw tables
/// (bounded: at most one head day, plus the un-rolled tail from the
/// watermark on). The parts partition `[start, end)` exactly, so the
/// hybrid reproduces the raw-only aggregates.
struct HybridWindow {
    /// Inclusive rollup day-key range; a `None` lower bound is
    /// unbounded (the all-time period).
    rollup_days: Option<(Option<String>, String)>,
    /// The raw epoch ranges complementing the rollup days.
    raw_ranges: Vec<(Option<f64>, Option<f64>)>,
}

fn hybrid_window(start: Option<f64>, end: Option<f64>, watermark: &str) -> HybridWindow {
    let day_range = |day: &str| daily_rollup::day_range(day).expect("canonical day");
    // Everything past the watermark day's end is raw territory; rollups
    // can serve full days up to this cut.
    let boundary = day_range(watermark).1;
    let cut = end.map_or(boundary, |e| e.min(boundary));

    // First full day at or after start (None = unbounded below).
    let lo_day = start.map(|s| {
        let day = daily_rollup::epoch_day(s);
        let (d0, d1) = day_range(&day);
        if s <= d0 {
            day
        } else {
            daily_rollup::epoch_day(d1)
        }
    });
    // Last full day ending at or before the cut.
    let hi_day = daily_rollup::epoch_day(day_range(&daily_rollup::epoch_day(cut)).0 - 1.0);

    let full_days_exist = lo_day
        .as_ref()
        .is_none_or(|lo| lo.as_str() <= hi_day.as_str());
    if !full_days_exist {
        return HybridWindow {
            rollup_days: None,
            raw_ranges: vec![(start, end)],
        };
    }

    let mut raw_ranges = Vec::new();
    if let (Some(s), Some(lo)) = (start, &lo_day) {
        let lo_start = day_range(lo).0;
        if s < lo_start {
            raw_ranges.push((Some(s), Some(lo_start)));
        }
    }
    let hi_end = day_range(&hi_day).1;
    if end.is_none_or(|e| e > hi_end) {
        raw_ranges.push((Some(hi_end), end));
    }
    HybridWindow {
        rollup_days: Some((lo_day, hi_day)),
        raw_ranges,
    }
}

/// The eleven aggregate-family sums of one window part, position-matched
/// to the `daily_rollups` family columns (loot, weapon, enhancer,
/// armour, heal, dangling, skill, codex, quest, harvest loot, harvest
/// cost). A sum stays None when the part had no contributing rows, so
/// the merged result reproduces the raw engine typing: an all-empty
/// window leaves the wire as an integer zero, exactly as
/// `COALESCE(SUM(...), 0)` does.
type FamilySums = [Option<f64>; 13];

fn merge_family_sums(into: &mut FamilySums, from: FamilySums) {
    for (slot, value) in into.iter_mut().zip(from) {
        if let Some(value) = value {
            *slot = Some(slot.unwrap_or(0.0) + value);
        }
    }
}

/// The `daily_rollups` family-sum columns, position-matched to
/// [`FamilySums`] (loot, weapon, enhancer, armour, heal, dangling, skill,
/// codex, quest, harvest loot, harvest cost, quest items, consumed doses).
const ROLLUP_FAMILY_COLS: [&str; 13] = [
    "loot_tt",
    "weapon_cost",
    "enhancer_cost",
    "armour_cost",
    "heal_cost",
    "dangling_cost",
    "skill_tt",
    "codex_pes",
    "quest_pes",
    "harvest_loot_tt",
    "harvest_cost",
    "quest_item_tt",
    "consumable_cost",
];

/// The rollup-side family sums for several windows in ONE conditional-
/// aggregation pass over `daily_rollups`, so the Overview reads the rollup
/// range once however many windows it reports (the period window plus the
/// two fixed trend windows), rather than re-scanning the rollups per
/// window. Each returned slot holds the same verbatim family sums a
/// single-range [`rollup_family_sums`] would (NULL preserved as `None`), so
/// the per-window merge with the raw edges is unchanged and the response is
/// byte-identical. A window with no full rollup days contributes an
/// all-`None` slot.
///
/// Day keys are canonical `YYYY-MM-DD` (chrono `%Y-%m-%d`, produced by
/// [`daily_rollup::epoch_day`]/[`daily_rollup::day_range`]): they sort
/// lexically and carry no quotes, so they inline into the CASE guards the
/// same way this file's other composed statements inline column and period
/// expressions (`AssertSqlSafe`, never caller data).
fn rollup_family_sums_multi(
    conn: &rusqlite::Connection,
    windows: &[HybridWindow],
) -> Result<Vec<FamilySums>, DbError> {
    let mut out: Vec<FamilySums> = vec![[None; 13]; windows.len()];

    // The windows that actually cover full rollup days, paired with their
    // index back into `out`.
    let active: Vec<(usize, Option<&str>, &str)> = windows
        .iter()
        .enumerate()
        .filter_map(|(index, window)| {
            window
                .rollup_days
                .as_ref()
                .map(|(lo, hi)| (index, lo.as_deref(), hi.as_str()))
        })
        .collect();
    if active.is_empty() {
        return Ok(out);
    }

    // One CASE-guarded SUM per (window, family): the conditional-aggregation
    // pass. Columns are emitted window-major, family-minor, matching the
    // read-back below.
    let mut cols: Vec<String> = Vec::with_capacity(active.len() * ROLLUP_FAMILY_COLS.len());
    for (_, lo, hi) in &active {
        for col in ROLLUP_FAMILY_COLS {
            let guard = match lo {
                Some(lo) => format!("day >= '{lo}' AND day <= '{hi}'"),
                None => format!("day <= '{hi}'"),
            };
            cols.push(format!("SUM(CASE WHEN {guard} THEN {col} END)"));
        }
    }

    // Bound the single scan to the union of the windows' day ranges: up to
    // the greatest `hi`, and down to the least `lo` only when every active
    // window has a lower bound (an all-time window leaves the scan unbounded
    // below, exactly as the per-window `rollup_family_sums` did).
    let max_hi = active.iter().map(|(_, _, hi)| *hi).max().expect("active");
    let min_lo = active
        .iter()
        .all(|(_, lo, _)| lo.is_some())
        .then(|| active.iter().filter_map(|(_, lo, _)| *lo).min())
        .flatten();
    let mut where_clause = format!("day <= '{max_hi}'");
    if let Some(min_lo) = min_lo {
        where_clause.push_str(&format!(" AND day >= '{min_lo}'"));
    }

    let sql = format!(
        "SELECT {} FROM daily_rollups WHERE {where_clause}",
        cols.join(", ")
    );
    let per_slot = conn.query_row(&sql, [], |row| {
        let mut per_slot: Vec<FamilySums> = vec![[None; 13]; active.len()];
        for (slot, sums) in per_slot.iter_mut().enumerate() {
            let base = slot * ROLLUP_FAMILY_COLS.len();
            for (family, value) in sums.iter_mut().enumerate() {
                *value = row.get::<_, Option<f64>>(base + family)?;
            }
        }
        Ok(per_slot)
    })?;

    for ((out_index, _, _), sums) in active.iter().zip(per_slot) {
        out[*out_index] = sums;
    }
    Ok(out)
}

/// One raw part's family sums over `[start, end)`, verbatim (NULL kept)
/// so the merge preserves engine typing.
fn raw_family_sums(
    conn: &rusqlite::Connection,
    range: (Option<f64>, Option<f64>),
) -> Result<FamilySums, DbError> {
    fn fetch(
        conn: &rusqlite::Connection,
        sql: String,
        params: &[f64],
        sums: usize,
    ) -> Result<Vec<Option<f64>>, DbError> {
        let values = conn.query_row(&sql, rusqlite::params_from_iter(params), |row| {
            (0..sums)
                .map(|index| row.get::<_, Option<f64>>(index))
                .collect::<rusqlite::Result<Vec<Option<f64>>>>()
        })?;
        Ok(values)
    }

    let (start, end) = range;
    let mut sums: FamilySums = [None; 13];
    let (w, p) = where_epoch("timestamp", start, end);
    let kills = fetch(
        conn,
        format!("SELECT SUM(loot_total_ped), SUM(enhancer_cost) FROM kills WHERE {w}"),
        &p,
        2,
    )?;
    sums[0] = kills[0];
    sums[2] = kills[1];

    let (w, p) = where_epoch("k.timestamp", start, end);
    let weapon = fetch(
        conn,
        format!(
            "SELECT SUM(ts.cost_per_shot * ts.shots_fired) \
             FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE {w}"
        ),
        &p,
        1,
    )?;
    sums[1] = weapon[0];

    let (w, p) = where_epoch("started_at", start, end);
    let sessions = fetch(
        conn,
        format!(
            "SELECT SUM(armour_cost), SUM(heal_cost), SUM(dangling_cost), \
             SUM(consumable_cost) FROM tracking_sessions WHERE {w}"
        ),
        &p,
        4,
    )?;
    sums[3] = sessions[0];
    sums[4] = sessions[1];
    sums[5] = sessions[2];
    sums[12] = sessions[3];

    let (w, p) = where_epoch("timestamp", start, end);
    sums[6] = fetch(
        conn,
        format!("SELECT SUM(ped_value) FROM skill_gains WHERE {w}"),
        &p,
        1,
    )?[0];
    let (w, p) = where_epoch("claimed_at", start, end);
    sums[7] = fetch(
        conn,
        format!("SELECT SUM(ped_value) FROM codex_claims WHERE {w}"),
        &p,
        1,
    )?[0];
    let (w, p) = where_epoch("claimed_at", start, end);
    sums[8] = fetch(
        conn,
        format!("SELECT SUM(ped_value) FROM quest_claims WHERE {w}"),
        &p,
        1,
    )?[0];
    let (w, p) = where_epoch("timestamp", start, end);
    let harvest = fetch(
        conn,
        format!("SELECT SUM(loot_total_ped), SUM(cost_ped) FROM harvest_events WHERE {w}"),
        &p,
        2,
    )?;
    sums[9] = harvest[0];
    sums[10] = harvest[1];
    let (w, p) = where_epoch("c.completed_at", start, end);
    sums[11] = fetch(
        conn,
        format!(
            "SELECT SUM(ri.value_ped) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             WHERE ri.accounting_kind = 'stock' AND {w} \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id) \
               AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = c.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            c.reward_outcome) = 'confirmed'"
        ),
        &p,
        1,
    )?[0];
    Ok(sums)
}

/// A day/month-keyed aggregate (`SELECT <bucket>, COALESCE(SUM(...), 0) ...
/// GROUP BY <bucket>`) collected as `bucket -> engine-typed number`.
/// Consumers merge and look up by bucket key; no ordering is carried.
fn bucketed_epoch(
    conn: &rusqlite::Connection,
    sql: String,
    params: &[f64],
) -> Result<std::collections::BTreeMap<String, SqlNumber>, DbError> {
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
    let mut out = std::collections::BTreeMap::new();
    while let Some(row) = rows.next()? {
        out.insert(row.get::<_, String>(0)?, SqlNumber::read(row, 1));
    }
    Ok(out)
}

// ── _compute_metrics ──

/// The gains/losses breakdown for one window (`_compute_metrics`).
struct Metrics {
    /// Liquid loot TT: kill loot plus harvest (wood) loot.
    loot_tt: SqlNumber,
    quest_item_tt: SqlNumber,
    skill_tt: SqlNumber,
    codex_pes: SqlNumber,
    quest_pes: SqlNumber,
    weapon: SqlNumber,
    healing: SqlNumber,
    enhancer: SqlNumber,
    armour: SqlNumber,
    dangling: SqlNumber,
    harvest: SqlNumber,
    consumables: SqlNumber,
    tracking_cost: SqlNumber,
    ledger_gains: std::collections::BTreeMap<String, f64>,
    ledger_losses: std::collections::BTreeMap<String, f64>,
}

/// Per-tag ledger totals for a window, rounded to two places and
/// collected in tag order (the order the raw `GROUP BY le.tag` sorter
/// emits). Ledger windows are day-granular (`le.date` is TEXT), so the
/// split is purely lexical: date keys at or below the watermark are all
/// rolled up (the heal sweeps stray spellings); later keys read raw.
/// Part totals stay unrounded until the final merge.
fn ledger_by_tag(
    conn: &rusqlite::Connection,
    entry_type: &str,
    epoch_start: Option<f64>,
    epoch_end: Option<f64>,
    watermark: &str,
) -> Result<std::collections::BTreeMap<String, f64>, DbError> {
    let mut totals: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let bounds = |column: &str| {
        let mut sql = String::new();
        let mut params = Vec::new();
        if let Some(start) = epoch_start {
            sql.push_str(&format!(" AND {column} >= ?"));
            params.push(epoch_to_iso(start));
        }
        if let Some(end) = epoch_end {
            sql.push_str(&format!(" AND {column} < ?"));
            params.push(epoch_to_iso(end));
        }
        (sql, params)
    };

    fn accumulate(
        conn: &rusqlite::Connection,
        totals: &mut std::collections::BTreeMap<String, f64>,
        sql: &str,
        params: &[String],
    ) -> Result<(), DbError> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            *totals.entry(row.get::<_, String>(0)?).or_insert(0.0) += row.get::<_, f64>(1)?;
        }
        Ok(())
    }

    let (extra, params) = bounds("day");
    let sql = format!(
        "SELECT tag, SUM(amount) FROM daily_ledger_rollups \
         WHERE entry_type = ? AND day <= ?{extra} GROUP BY tag"
    );
    let mut p: Vec<String> = vec![entry_type.to_string(), watermark.to_string()];
    p.extend(params);
    accumulate(conn, &mut totals, &sql, &p)?;

    let (extra, params) = bounds("le.date");
    let sql = format!(
        "SELECT le.tag, SUM(le.amount) FROM ledger_entries le \
         WHERE le.type = ? AND le.date > ?{extra} GROUP BY le.tag"
    );
    let mut p: Vec<String> = vec![entry_type.to_string(), watermark.to_string()];
    p.extend(params);
    accumulate(conn, &mut totals, &sql, &p)?;

    let mut out = std::collections::BTreeMap::new();
    for (tag, total) in totals {
        out.insert(tag, eo_wire::normalizer::round_half_even(total, 2));
    }
    Ok(out)
}

/// One window's metrics through the hybrid split: full days at or
/// below the rollup watermark aggregate O(days) from `daily_rollups`;
/// the partial edge days and the un-rolled tail aggregate from bounded
/// raw windows.
/// One window's metrics, given its pre-computed hybrid split and the
/// rollup-side family sums from the batched [`rollup_family_sums_multi`]
/// pass. Adds the window's bounded raw edges and its per-window ledger
/// totals, then shapes the gains/losses breakdown exactly as the
/// single-pass path did (the batched rollup sums are identical to the
/// per-window ones, so the merge and the response are byte-for-byte
/// unchanged).
fn assemble_metrics(
    conn: &rusqlite::Connection,
    window: &HybridWindow,
    rollup_sums: FamilySums,
    epoch_start: Option<f64>,
    epoch_end: Option<f64>,
    watermark: &str,
) -> Result<Metrics, DbError> {
    let mut sums = rollup_sums;
    for range in &window.raw_ranges {
        merge_family_sums(&mut sums, raw_family_sums(conn, *range)?);
    }

    let kill_loot = SqlNumber::from_family(sums[0]);
    let weapon = SqlNumber::from_family(sums[1]);
    let enhancer = SqlNumber::from_family(sums[2]);
    let armour = SqlNumber::from_family(sums[3]);
    let healing = SqlNumber::from_family(sums[4]);
    let dangling = SqlNumber::from_family(sums[5]);
    let skill_tt = SqlNumber::from_family(sums[6]);
    let codex_pes = SqlNumber::from_family(sums[7]);
    let quest_pes = SqlNumber::from_family(sums[8]);
    let harvest_loot = SqlNumber::from_family(sums[9]);
    let harvest = SqlNumber::from_family(sums[10]);
    let quest_item_tt = SqlNumber::from_family(sums[11]);
    let consumables = SqlNumber::from_family(sums[12]);

    // Wood TT is liquid loot; the headline Loot TT carries both
    // activities.
    let loot_tt = kill_loot.sum(harvest_loot);

    // weapon + heal + enhancer + armour + dangling (the pinned order),
    // then harvest swing decay and consumed doses.
    let tracking_cost = weapon
        .sum(healing)
        .sum(enhancer)
        .sum(armour)
        .sum(dangling)
        .sum(harvest)
        .sum(consumables);

    let ledger_gains = ledger_by_tag(conn, "markup", epoch_start, epoch_end, watermark)?;
    let ledger_losses = ledger_by_tag(conn, "expense", epoch_start, epoch_end, watermark)?;

    Ok(Metrics {
        loot_tt,
        quest_item_tt,
        skill_tt,
        codex_pes,
        quest_pes,
        weapon,
        healing,
        enhancer,
        armour,
        dangling,
        harvest,
        consumables,
        tracking_cost,
        ledger_gains,
        ledger_losses,
    })
}

/// Sum of a ledger map's values in float space.
fn sum_values(map: &std::collections::BTreeMap<String, f64>) -> f64 {
    map.values().sum()
}

/// `_rate_from_metrics`: liquid gains over liquid losses (progression
/// excluded), 0.0 when losses are non-positive.
fn rate_from_metrics(m: &Metrics) -> f64 {
    let total_gains = m.loot_tt.as_f64() + m.quest_item_tt.as_f64() + sum_values(&m.ledger_gains);
    let total_losses = m.tracking_cost.as_f64() + sum_values(&m.ledger_losses);
    if total_losses > 0.0 {
        total_gains / total_losses
    } else {
        0.0
    }
}

/// `bucket -> {tag -> rounded amount}` for the timeline / monthly
/// breakdowns, hybrid over the same lexical watermark split as
/// [`ledger_by_tag`]: rolled date keys read `daily_ledger_rollups`,
/// later keys read raw. Both maps emit in (bucket, tag) order, the
/// order the raw `GROUP BY` sorter produced; part sums merge unrounded
/// (month buckets can span the split) and round once.
fn ledger_buckets(
    conn: &rusqlite::Connection,
    kind: BucketKind,
    entry_type: &str,
    epoch_start: Option<f64>,
    watermark: &str,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeMap<String, f64>>, DbError> {
    let mut sums: std::collections::BTreeMap<String, std::collections::BTreeMap<String, f64>> =
        std::collections::BTreeMap::new();
    let start_iso = epoch_start.map(epoch_to_iso);

    fn accumulate(
        conn: &rusqlite::Connection,
        sums: &mut std::collections::BTreeMap<String, std::collections::BTreeMap<String, f64>>,
        sql: &str,
        params: &[String],
    ) -> Result<(), DbError> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            *sums
                .entry(row.get::<_, String>(0)?)
                .or_default()
                .entry(row.get::<_, String>(1)?)
                .or_insert(0.0) += row.get::<_, f64>(2)?;
        }
        Ok(())
    }

    let bucket_expr = match kind {
        BucketKind::Day => "day",
        BucketKind::Month => "strftime('%Y-%m', day)",
    };
    let extra = if start_iso.is_some() {
        " AND day >= ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {bucket_expr} AS bucket, tag, SUM(amount) FROM daily_ledger_rollups \
         WHERE entry_type = ? AND day <= ?{extra} GROUP BY bucket, tag"
    );
    let mut p: Vec<String> = vec![entry_type.to_string(), watermark.to_string()];
    if let Some(start) = &start_iso {
        p.push(start.clone());
    }
    accumulate(conn, &mut sums, &sql, &p)?;

    let bucket_expr = match kind {
        BucketKind::Day => "le.date",
        BucketKind::Month => "strftime('%Y-%m', le.date)",
    };
    let extra = if start_iso.is_some() {
        " AND le.date >= ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {bucket_expr} AS bucket, le.tag, SUM(le.amount) FROM ledger_entries le \
         WHERE le.type = ? AND le.date > ?{extra} GROUP BY bucket, le.tag"
    );
    let mut p: Vec<String> = vec![entry_type.to_string(), watermark.to_string()];
    if let Some(start) = &start_iso {
        p.push(start.clone());
    }
    accumulate(conn, &mut sums, &sql, &p)?;

    let mut out: std::collections::BTreeMap<String, std::collections::BTreeMap<String, f64>> =
        std::collections::BTreeMap::new();
    for (bucket, tags) in sums {
        let entry = out.entry(bucket).or_default();
        for (tag, amount) in tags {
            entry.insert(tag, eo_wire::normalizer::round_half_even(amount, 2));
        }
    }
    Ok(out)
}

// ── overview_impl ──

/// The Overview aggregate. Scaling is O(days), not O(rows): the heal
/// brings the daily rollups current (steady-state, a single metadata
/// read), and every window then aggregates rollup rows plus bounded raw
/// edges (see [`hybrid_window`]).
async fn overview_impl(db: &Db, now: f64, period: &str) -> Result<OverviewData, DbError> {
    // The lazy rollup heal is a write (route it to the writer, never a
    // reader-held connection); every subsequent aggregate is a plain read,
    // run as one synchronous unit on a reader-core connection.
    let watermark = db
        .with_writer(move |conn| daily_rollup::heal_rollups(conn, now))
        .await?;
    let epoch_start = period_epoch(period, now);
    db.with_reader(move |conn| overview_read(conn, now, epoch_start, &watermark))
        .await
}

/// The Overview aggregate proper: a single synchronous read pass over a
/// reader-core connection (the caller heals the rollups first, on the
/// writer). Every step is a plain read, so the whole aggregate runs
/// without an executor between its statements.
fn overview_read(
    conn: &rusqlite::Connection,
    now: f64,
    epoch_start: Option<f64>,
    watermark: &str,
) -> Result<OverviewData, DbError> {
    // The Overview reports three metric windows: the requested period
    // window, and the two fixed trend windows (recent-30d, prior-30d, the
    // pair independent of the period). Their rollup-side family sums come
    // from a single conditional-aggregation pass over `daily_rollups` (the
    // Overview reads the rollup range once); each window then adds its own
    // bounded raw edges and per-window ledger totals.
    let day_30 = now - 30.0 * 86400.0;
    let day_60 = now - 60.0 * 86400.0;
    let window_bounds = [
        (epoch_start, None),
        (Some(day_30), None),
        (Some(day_60), Some(day_30)),
    ];
    let windows: Vec<HybridWindow> = window_bounds
        .iter()
        .map(|&(start, end)| hybrid_window(start, end, watermark))
        .collect();
    let rollup_sums = rollup_family_sums_multi(conn, &windows)?;
    let mut metrics = Vec::with_capacity(window_bounds.len());
    for ((start, end), (window, sums)) in window_bounds.iter().zip(windows.iter().zip(rollup_sums))
    {
        metrics.push(assemble_metrics(
            conn, window, sums, *start, *end, watermark,
        )?);
    }
    let mut metrics = metrics.into_iter();
    let m = metrics.next().expect("period window metrics");
    let rate_30d = rate_from_metrics(&metrics.next().expect("recent-30d window metrics"));
    let rate_prior = rate_from_metrics(&metrics.next().expect("prior-30d window metrics"));

    let total_ledger_gains = sum_values(&m.ledger_gains);
    let total_ledger_losses = sum_values(&m.ledger_losses);
    let total_gains = m.loot_tt.as_f64() + m.quest_item_tt.as_f64() + total_ledger_gains;
    let total_losses = m.tracking_cost.as_f64() + total_ledger_losses;
    let return_rate = if total_losses > 0.0 {
        total_gains / total_losses
    } else {
        0.0
    };

    // Trend: always recent-30d vs prior-30d, independent of period.
    let trend = if rate_30d > 0.0 && rate_prior > 0.0 {
        if rate_30d > rate_prior * 1.02 {
            "improving"
        } else if rate_30d < rate_prior * 0.98 {
            "declining"
        } else {
            "stable"
        }
    } else {
        "stable"
    };

    // Daily breakdown (the facade labels the point key "date"; monthly
    // points label it "month").
    let timeline = breakdown_points(conn, watermark, epoch_start, BucketKind::Day)?;
    let monthly = breakdown_points(conn, watermark, epoch_start, BucketKind::Month)?;

    Ok(OverviewData {
        total_return_rate: eo_wire::normalizer::round_half_even(return_rate, 4),
        trend,
        returns_breakdown: ReturnsData {
            loot_tt: m.loot_tt.rounded(2).as_f64(),
            quest_item_tt: m.quest_item_tt.rounded(2).as_f64(),
            pes: m.skill_tt.rounded(2).as_f64(),
            codex_pes: m.codex_pes.rounded(2).as_f64(),
            quest_pes: m.quest_pes.rounded(2).as_f64(),
            ledger: m.ledger_gains,
        },
        losses_breakdown: LossesData {
            tracking_cost: m.tracking_cost.rounded(2).as_f64(),
            cycled_breakdown: CycledData {
                weapon: m.weapon.rounded(2),
                healing: m.healing.rounded(2),
                enhancer: m.enhancer.rounded(2),
                armour: m.armour.rounded(2),
                dangling: m.dangling.rounded(2),
                harvest: m.harvest.rounded(2),
                consumables: m.consumables.rounded(2),
            },
            ledger: m.ledger_losses,
        },
        total_gains: eo_wire::normalizer::round_half_even(total_gains, 2),
        total_losses: eo_wire::normalizer::round_half_even(total_losses, 2),
        timeline,
        monthly_breakdown: monthly,
    })
}

#[derive(Clone, Copy)]
enum BucketKind {
    Day,
    Month,
}

/// The seven per-bucket family maps plus the bucket-membership set the
/// point loop consumes. Buckets with rows whose sums are NULL matter
/// only for membership (their emitted values coincide with the absent
/// key's integer-zero default), so the rollup side contributes NULL
/// family sums as absent keys and membership through `has_rows`.
#[derive(Default)]
struct BreakdownMaps {
    loot: std::collections::BTreeMap<String, SqlNumber>,
    quest_item: std::collections::BTreeMap<String, SqlNumber>,
    weapon: std::collections::BTreeMap<String, SqlNumber>,
    enhancer: std::collections::BTreeMap<String, SqlNumber>,
    sess: std::collections::BTreeMap<String, SqlNumber>,
    skill: std::collections::BTreeMap<String, SqlNumber>,
    codex: std::collections::BTreeMap<String, SqlNumber>,
    quest: std::collections::BTreeMap<String, SqlNumber>,
    /// Harvest swing decay (wood loot merges into `loot`).
    harvest_cost: std::collections::BTreeMap<String, SqlNumber>,
    members: BTreeSet<String>,
}

impl BreakdownMaps {
    /// Merge one bucket's value into a family map. Day buckets never
    /// collide across parts (the hybrid ranges partition the timeline);
    /// month buckets can span the split and sum engine-typed.
    fn merge(
        map: &mut std::collections::BTreeMap<String, SqlNumber>,
        bucket: &str,
        value: SqlNumber,
    ) {
        match map.get(bucket) {
            Some(existing) => {
                let total = existing.sum(value);
                map.insert(bucket.to_string(), total);
            }
            None => {
                map.insert(bucket.to_string(), value);
            }
        }
    }
}

/// Collect the rollup side of the breakdown maps: one pass over the
/// rolled days (or their month groups).
fn rollup_breakdown(
    conn: &rusqlite::Connection,
    maps: &mut BreakdownMaps,
    kind: BucketKind,
    lo: Option<&str>,
    hi: &str,
) -> Result<(), DbError> {
    let extra = if lo.is_some() { " AND day >= ?" } else { "" };
    let sql = match kind {
        BucketKind::Day => format!(
            "SELECT day AS bucket, has_rows, loot_tt, weapon_cost, enhancer_cost, \
             armour_cost, heal_cost, dangling_cost, skill_tt, codex_pes, quest_pes, \
             harvest_loot_tt, harvest_cost, quest_item_tt, consumable_cost \
             FROM daily_rollups WHERE day <= ?{extra} ORDER BY bucket"
        ),
        BucketKind::Month => format!(
            "SELECT strftime('%Y-%m', day) AS bucket, MAX(has_rows), SUM(loot_tt), \
             SUM(weapon_cost), SUM(enhancer_cost), SUM(armour_cost), SUM(heal_cost), \
             SUM(dangling_cost), SUM(skill_tt), SUM(codex_pes), SUM(quest_pes), \
             SUM(harvest_loot_tt), SUM(harvest_cost), SUM(quest_item_tt), SUM(consumable_cost) \
             FROM daily_rollups WHERE day <= ?{extra} GROUP BY bucket ORDER BY bucket"
        ),
    };
    let mut params: Vec<&str> = vec![hi];
    if let Some(lo) = lo {
        params.push(lo);
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
    while let Some(row) = rows.next()? {
        let bucket = row.get::<_, String>(0)?;
        if row.get::<_, i64>(1)? != 0 {
            maps.members.insert(bucket.clone());
        }
        let family = |index: usize| row.get::<_, Option<f64>>(index);
        for (map, index) in [
            (&mut maps.loot, 2),
            (&mut maps.weapon, 3),
            (&mut maps.enhancer, 4),
            (&mut maps.skill, 8),
            (&mut maps.codex, 9),
            (&mut maps.quest, 10),
            (&mut maps.harvest_cost, 12),
            (&mut maps.quest_item, 13),
        ] {
            if let Some(value) = family(index)? {
                BreakdownMaps::merge(map, &bucket, SqlNumber::Float(value));
            }
        }
        // Wood loot joins the loot family (a second merge into the
        // same map, so it sits outside the disjoint-borrow loop).
        if let Some(value) = family(11)? {
            BreakdownMaps::merge(&mut maps.loot, &bucket, SqlNumber::Float(value));
        }
        // The session-cost leg mirrors the raw query's
        // COALESCE(SUM(armour),0) + COALESCE(SUM(heal),0) +
        // COALESCE(SUM(dangling),0) + COALESCE(SUM(consumable),0): integer
        // zeros for NULL legs, a bucket only when any session existed
        // (subsumed by has_rows membership; an absent key emits the same
        // integer zero).
        let armour = family(5)?;
        let heal = family(6)?;
        let dangling = family(7)?;
        let consumable = family(14)?;
        if armour.is_some() || heal.is_some() || dangling.is_some() || consumable.is_some() {
            let total = SqlNumber::from_family(armour)
                .sum(SqlNumber::from_family(heal))
                .sum(SqlNumber::from_family(dangling))
                .sum(SqlNumber::from_family(consumable));
            BreakdownMaps::merge(&mut maps.sess, &bucket, total);
        }
    }
    Ok(())
}

/// Collect one raw range's side of the breakdown maps: the original
/// per-source bucketed queries, windowed to the range.
fn raw_breakdown(
    conn: &rusqlite::Connection,
    maps: &mut BreakdownMaps,
    kind: BucketKind,
    range: (Option<f64>, Option<f64>),
) -> Result<(), DbError> {
    let (start, end) = range;
    let ts_bucket = |col: &str| match kind {
        BucketKind::Day => format!("date({col}, 'unixepoch')"),
        BucketKind::Month => format!("strftime('%Y-%m', {col}, 'unixepoch')"),
    };
    let (enc_w, enc_p) = where_epoch("k.timestamp", start, end);
    let (sg_w, sg_p) = where_epoch("sg.timestamp", start, end);
    let (cc_w, cc_p) = where_epoch("cc.claimed_at", start, end);
    let (qc_w, qc_p) = where_epoch("qc.claimed_at", start, end);
    let (sess_w, sess_p) = where_epoch("s.started_at", start, end);
    let (hv_w, hv_p) = where_epoch("h.timestamp", start, end);
    let (qri_w, qri_p) = where_epoch("c.completed_at", start, end);

    let sources: [(
        &mut std::collections::BTreeMap<String, SqlNumber>,
        String,
        &Vec<f64>,
    ); 9] = [
        (
            &mut maps.loot,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(k.loot_total_ped), 0) FROM kills k WHERE {enc_w} GROUP BY bucket",
                ts_bucket("k.timestamp")
            ),
            &enc_p,
        ),
        (
            &mut maps.weapon,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
                 FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE {enc_w} GROUP BY bucket",
                ts_bucket("k.timestamp")
            ),
            &enc_p,
        ),
        (
            &mut maps.enhancer,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE {enc_w} GROUP BY bucket",
                ts_bucket("k.timestamp")
            ),
            &enc_p,
        ),
        (
            &mut maps.sess,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(s.armour_cost), 0) + COALESCE(SUM(s.heal_cost), 0) \
                 + COALESCE(SUM(s.dangling_cost), 0) + COALESCE(SUM(s.consumable_cost), 0) \
                 FROM tracking_sessions s WHERE {sess_w} GROUP BY bucket",
                ts_bucket("s.started_at")
            ),
            &sess_p,
        ),
        (
            &mut maps.skill,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(sg.ped_value), 0) FROM skill_gains sg WHERE {sg_w} GROUP BY bucket",
                ts_bucket("sg.timestamp")
            ),
            &sg_p,
        ),
        (
            &mut maps.codex,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(cc.ped_value), 0) FROM codex_claims cc WHERE {cc_w} GROUP BY bucket",
                ts_bucket("cc.claimed_at")
            ),
            &cc_p,
        ),
        (
            &mut maps.quest,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(qc.ped_value), 0) FROM quest_claims qc WHERE {qc_w} GROUP BY bucket",
                ts_bucket("qc.claimed_at")
            ),
            &qc_p,
        ),
        (
            &mut maps.harvest_cost,
            format!(
                "SELECT {} as bucket, COALESCE(SUM(h.cost_ped), 0) FROM harvest_events h WHERE {hv_w} GROUP BY bucket",
                ts_bucket("h.timestamp")
            ),
            &hv_p,
        ),
        (
            &mut maps.quest_item,
            format!(
                "SELECT {} AS bucket, COALESCE(SUM(ri.value_ped), 0) \
                 FROM session_quest_completion_reward_items ri \
                 JOIN session_quest_completions c ON c.id = ri.completion_id \
                 WHERE ri.accounting_kind = 'stock' AND {qri_w} \
                   AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                                   WHERE rr.completion_id = c.id) \
                   AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                                 WHERE r.completion_id = c.id \
                                 ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                                c.reward_outcome) = 'confirmed' \
                 GROUP BY bucket",
                ts_bucket("c.completed_at")
            ),
            &qri_p,
        ),
    ];
    for (map, sql, params) in sources {
        let buckets = bucketed_epoch(conn, sql, params)?;
        for (bucket, value) in buckets {
            maps.members.insert(bucket.clone());
            BreakdownMaps::merge(map, &bucket, value);
        }
    }
    // Wood loot joins the loot family (the harvest table feeds two
    // families, so its second source sits outside the disjoint-borrow
    // array).
    let wood = bucketed_epoch(
        conn,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(h.loot_total_ped), 0) FROM harvest_events h WHERE {hv_w} GROUP BY bucket",
            ts_bucket("h.timestamp")
        ),
        &hv_p,
    )?;
    for (bucket, value) in wood {
        maps.members.insert(bucket.clone());
        BreakdownMaps::merge(&mut maps.loot, &bucket, value);
    }
    Ok(())
}

/// Build the timeline / monthly breakdown: per-source bucketed sums merged
/// over the union of all buckets, then one point per bucket in sorted order.
/// Hybrid over the rollup watermark, exactly as [`assemble_metrics`].
fn breakdown_points(
    conn: &rusqlite::Connection,
    watermark: &str,
    epoch_start: Option<f64>,
    kind: BucketKind,
) -> Result<Vec<TimelinePoint>, DbError> {
    let window = hybrid_window(epoch_start, None, watermark);
    let mut maps = BreakdownMaps::default();
    if let Some((lo, hi)) = &window.rollup_days {
        rollup_breakdown(conn, &mut maps, kind, lo.as_deref(), hi)?;
    }
    for range in &window.raw_ranges {
        raw_breakdown(conn, &mut maps, kind, *range)?;
    }

    // cost = weapon + enhancer + sess + harvest over the union of
    // their buckets.
    let mut cost: std::collections::BTreeMap<String, SqlNumber> = std::collections::BTreeMap::new();
    let mut cost_keys: BTreeSet<String> = BTreeSet::new();
    for k in maps
        .weapon
        .keys()
        .chain(maps.enhancer.keys())
        .chain(maps.sess.keys())
        .chain(maps.harvest_cost.keys())
    {
        cost_keys.insert(k.clone());
    }
    let zero = SqlNumber::Int(0);
    for key in &cost_keys {
        let total = maps
            .weapon
            .get(key)
            .copied()
            .unwrap_or(zero)
            .sum(maps.enhancer.get(key).copied().unwrap_or(zero))
            .sum(maps.sess.get(key).copied().unwrap_or(zero))
            .sum(maps.harvest_cost.get(key).copied().unwrap_or(zero));
        cost.insert(key.clone(), total);
    }

    let gains = ledger_buckets(conn, kind, "markup", epoch_start, watermark)?;
    let losses = ledger_buckets(conn, kind, "expense", epoch_start, watermark)?;

    // all buckets, sorted (lexicographic == chronological for these forms).
    let mut all: BTreeSet<String> = maps.members;
    for k in gains.keys().chain(losses.keys()) {
        all.insert(k.clone());
    }

    let family = |map: &std::collections::BTreeMap<String, SqlNumber>, bucket: &String| {
        map.get(bucket).copied().unwrap_or(zero).rounded(4).as_f64()
    };
    let mut points = Vec::new();
    for bucket in &all {
        points.push(TimelinePoint {
            bucket: bucket.clone(),
            loot_tt: family(&maps.loot, bucket),
            quest_item_tt: family(&maps.quest_item, bucket),
            pes: family(&maps.skill, bucket),
            codex_pes: family(&maps.codex, bucket),
            quest_pes: family(&maps.quest, bucket),
            ledger_gains: gains.get(bucket).cloned().unwrap_or_default(),
            tracking_cost: family(&cost, bucket),
            ledger_losses: losses.get(bucket).cloned().unwrap_or_default(),
        });
    }
    Ok(points)
}

// ── hunting_impl / harvest_impl ──

/// One completed session's aggregates (`_load_activity_sessions`).
#[derive(Default)]
struct SessionAgg {
    duration_hours: f64,
    armour_cost: f64,
    heal_cost: f64,
    consumable_cost: f64,
    dangling_cost: f64,
    weapon_cost: f64,
    enhancer_cost: f64,
    weapon_shots: f64,
    kills: i64,
    loot_tt: f64,
    skill_tt: f64,
    dominant_mob: Option<String>,
    dominant_mob_kills: i64,
    /// The session-name facet (the designated axis; a legacy tag-mode
    /// session reads its migrated name here).
    session_name: Option<String>,
    cycled_ped: f64,
}

async fn load_activity_sessions(db: &Db) -> Result<Vec<SessionAgg>, DbError> {
    // Read the per-session aggregates from the materialised summaries instead
    // of re-aggregating the raw tables on every request. Heal first (a write,
    // routed to the writer) so a read after a summary-version bump (or on a
    // fresh install) sees current rows; the read itself runs as one
    // synchronous unit on a reader-core connection.
    db.with_writer(|conn| crate::session_summary::heal_summaries(conn))
        .await?;
    db.with_reader(activity_sessions_read).await
}

/// The Activity per-session aggregates, read in one synchronous pass over a
/// reader-core connection (the caller heals the summaries first, on the
/// writer).
fn activity_sessions_read(conn: &mut rusqlite::Connection) -> Result<Vec<SessionAgg>, DbError> {
    let mut sessions = read_summary_activity_aggs(conn)?;

    // Reconcile the sessions Activity counts but a summary never holds: an
    // ended session with kills and cost but no skill gains qualifies for
    // Activity yet fails the summary's gains requirement, so it has no summary
    // row. Rare (usually none); computed raw only for those ids, so the cost
    // scales with the divergence, not the whole history. The divergent ids are
    // collected first, so the streaming read is done before the per-id raw
    // aggregates prepare their own statements on the same connection.
    let divergent: Vec<DivergentSession> = {
        let mut stmt = conn.prepare(
            "SELECT s.id, s.started_at, s.ended_at, COALESCE(s.armour_cost, 0), \
             COALESCE(s.heal_cost, 0), COALESCE(s.dangling_cost, 0), s.session_name, \
             COALESCE(s.consumable_cost, 0) \
             FROM tracking_sessions s \
             LEFT JOIN session_summaries ss ON ss.session_id = s.id \
             WHERE s.ended_at IS NOT NULL AND ss.session_id IS NULL",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1).unwrap_or(0.0),
                row.get::<_, f64>(2).unwrap_or(0.0),
                as_float(row, 3),
                as_float(row, 4),
                as_float(row, 5),
                row.get::<_, Option<String>>(6)?,
                as_float(row, 7),
            ));
        }
        out
    };
    for (id, started, ended, armour, heal, dangling, name, consumable) in divergent {
        let mut agg = raw_session_agg(conn, &id, started, ended, armour, heal, dangling, name)?;
        agg.consumable_cost = consumable;
        agg.cycled_ped = eo_wire::normalizer::round_half_even(agg.cycled_ped + consumable, 4);
        sessions.insert(id, agg);
    }

    // Activity's own qualifying filter. Order-independent: the slice builders
    // regroup and re-sort by (-kills, -cycled, name), and each group's name is
    // unique, so the map iteration order never reaches the response.
    Ok(sessions
        .into_values()
        .filter(|s| s.duration_hours > 0.0 && s.cycled_ped > 0.0 && s.kills > 0)
        .collect())
}

/// The per-session Activity aggregates read straight from `session_summaries`,
/// keyed by session id. Every field the slice builders read comes from a stored
/// column (the cost components that only fed `cycled_ped` are left at their
/// defaults, since the stored `cycled_ped` already carries their rounded sum).
fn read_summary_activity_aggs(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, SessionAgg>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT session_id, duration_hours, kills, loot_tt, cycled_ped, activity_skill_tt, \
         dominant_mob, dominant_mob_kills, session_name \
         FROM session_summaries",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let id = row.get::<_, String>(0)?;
        out.insert(
            id,
            SessionAgg {
                duration_hours: as_float(row, 1),
                kills: row.get::<_, i64>(2).unwrap_or(0),
                loot_tt: as_float(row, 3),
                cycled_ped: as_float(row, 4),
                skill_tt: as_float(row, 5),
                dominant_mob: row.get::<_, Option<String>>(6)?,
                dominant_mob_kills: row.get::<_, i64>(7).unwrap_or(0),
                session_name: row.get::<_, Option<String>>(8)?,
                ..SessionAgg::default()
            },
        );
    }
    Ok(out)
}

/// One ended session with no summary row, as the reconciliation read
/// returns it: id, start, end, armour, heal, dangling, the session name
/// facet, and consumed doses.
type DivergentSession = (String, f64, f64, f64, f64, f64, Option<String>, f64);

/// Compute one session's Activity aggregate directly from the raw tables, for
/// the reconciliation path (an ended session with no summary row). Mirrors the
/// summary's own per-session computation query for query, so an included
/// no-gains session carries the same numbers a summary would if it held one.
#[allow(clippy::too_many_arguments)]
fn raw_session_agg(
    conn: &rusqlite::Connection,
    session_id: &str,
    started_at: f64,
    ended_at: f64,
    armour_cost: f64,
    heal_cost: f64,
    dangling_cost: f64,
    session_name: Option<String>,
) -> Result<SessionAgg, DbError> {
    let mut agg = SessionAgg {
        duration_hours: (ended_at - started_at).max(0.0) / 3600.0,
        armour_cost,
        heal_cost,
        dangling_cost,
        // Carried from the caller's own read of the session row rather
        // than re-queried per session.
        session_name: session_name.filter(|name| !name.is_empty()),
        ..SessionAgg::default()
    };

    let (kills, loot_tt, enhancer_cost) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| Ok((row.get::<_, i64>(0)?, as_float(row, 1), as_float(row, 2))),
    )?;
    agg.kills = kills;
    agg.loot_tt = loot_tt;
    agg.enhancer_cost = enhancer_cost;

    let (weapon_cost, weapon_shots) = conn.query_row(
        "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0), \
         COALESCE(SUM(ts.shots_fired), 0) FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id WHERE k.session_id = ?",
        rusqlite::params![session_id],
        |row| Ok((as_float(row, 0), as_float(row, 1))),
    )?;
    agg.weapon_cost = weapon_cost;
    agg.weapon_shots = weapon_shots;

    agg.skill_tt = conn.query_row(
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains \
         WHERE session_id = ? AND ped_value IS NOT NULL",
        rusqlite::params![session_id],
        |row| Ok(as_float(row, 0)),
    )?;

    // Mob dominance over species-bearing stamps only, mirroring the summary
    // writer (a species-less stamp is a legacy tag, i.e. a session name).
    let mob_rows: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT mob_name, COUNT(*) \
             FROM kills WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
               AND COALESCE(mob_species, '') != '' \
             GROUP BY mob_name ORDER BY COUNT(*) DESC, mob_name ASC",
        )?;
        let mapped = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if !mob_rows.is_empty() {
        let total_known: i64 = mob_rows.iter().map(|r| r.1).sum();
        if total_known > 0 {
            let (top_name, top_count) = mob_rows[0].clone();
            if top_count as f64 / total_known as f64 >= ACTIVITY_DOMINANCE_THRESHOLD {
                agg.dominant_mob = Some(top_name);
                agg.dominant_mob_kills = top_count;
            }
        }
    }

    agg.cycled_ped = eo_wire::normalizer::round_half_even(
        agg.weapon_cost + agg.enhancer_cost + agg.armour_cost + agg.heal_cost + agg.dangling_cost,
        4,
    );
    Ok(agg)
}

/// `_build_activity_slice_rows`: group sessions by a dominant field, sum the
/// per-group stats, and sort by (-kills, -cycled, name).
fn build_activity_slice_rows(
    sessions: &[SessionAgg],
    select: impl Fn(&SessionAgg) -> Option<String>,
    kills_of: impl Fn(&SessionAgg) -> i64,
) -> Vec<ActivityRow> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<&SessionAgg>> =
        std::collections::HashMap::new();
    for session in sessions {
        if let Some(value) = select(session) {
            if value.is_empty() {
                continue;
            }
            grouped.entry(value.clone()).or_insert_with(|| {
                order.push(value.clone());
                Vec::new()
            });
            grouped.get_mut(&value).unwrap().push(session);
        }
    }

    let mut rows: Vec<(i64, f64, String, ActivityRow)> = Vec::new();
    for value in &order {
        let matched = &grouped[value];
        let sessions_count = matched.len() as i64;
        let kills: i64 = matched.iter().map(|s| kills_of(s)).sum();
        let hours: f64 = matched.iter().map(|s| s.duration_hours).sum();
        let cycled: f64 = matched.iter().map(|s| s.cycled_ped).sum();
        let loot_tt: f64 = matched.iter().map(|s| s.loot_tt).sum();
        let skill_tt: f64 = matched.iter().map(|s| s.skill_tt).sum();
        let hours_r = eo_wire::normalizer::round_half_even(hours, 2);
        let cycled_r = eo_wire::normalizer::round_half_even(cycled, 2);
        let pes_per_100 = if cycled > 0.0 {
            eo_wire::normalizer::round_half_even((skill_tt / cycled) * 100.0, 2)
        } else {
            0.0
        };
        let loot_rate = if cycled > 0.0 {
            eo_wire::normalizer::round_half_even(loot_tt / cycled, 4)
        } else {
            0.0
        };
        let row = ActivityRow {
            name: value.clone(),
            sessions: sessions_count,
            kills,
            hours: hours_r,
            cycled: cycled_r,
            pes_per100_ped: pes_per_100,
            loot_rate,
        };
        rows.push((kills, cycled, value.clone(), row));
    }
    // sort by (-kills, -cycled, name)
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.2.cmp(&b.2))
    });
    rows.into_iter().map(|(_, _, _, row)| row).collect()
}

async fn hunting_impl(db: &Db) -> Result<HuntingData, DbError> {
    let sessions = load_activity_sessions(db).await?;
    let mob = build_activity_slice_rows(
        &sessions,
        |s| s.dominant_mob.clone(),
        |s| s.dominant_mob_kills,
    );
    // The designated axis groups on the session-name facet: exact
    // session-scoped grouping, no dominance gate, and a session appears
    // in BOTH tables when it carries both a name and a dominant mob
    // (the co-recording model that replaced the tag-or-mob collapse).
    let name = build_activity_slice_rows(&sessions, |s| s.session_name.clone(), |s| s.kills);
    Ok(HuntingData {
        mob_comparisons: mob,
        name_comparisons: name,
    })
}

/// One tier group: tier, swing count, cost, loot TT.
type HarvestTierTotals = (String, i64, f64, f64);

/// One tier item group: tier, item, quantity, TT.
type HarvestTierItemTotals = (String, String, i64, f64);

/// The Tree Cutting tier-first aggregate, grouped straight off the
/// durable event attribution. A tab-open read, not a hot path: the scan
/// is O(total harvest events), acceptable at harvesting volumes.
///
/// The yield tier is the whole grouping. The tool is deliberately not a
/// dimension here: it is an input to the activity, not an outcome of it, and
/// its principal effect is which tier a swing reaches, which grouping inside
/// a tier holds constant. Comparing tools belongs with equipment, on cost.
async fn harvest_impl(db: &Db, epoch_start: Option<f64>) -> Result<HarvestData, DbError> {
    let (raw, composition): (Vec<HarvestTierTotals>, Vec<HarvestTierItemTotals>) = db
        .with_reader(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT yield_tier, COUNT(*), \
                 COALESCE(SUM(cost_ped), 0), \
                 COALESCE(SUM(loot_total_ped), 0) FROM harvest_events h \
                 WHERE (?1 IS NULL OR h.timestamp >= ?1) \
                 GROUP BY yield_tier",
            )?;
            let raw = stmt
                .query_map([epoch_start], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        as_float(row, 2),
                        as_float(row, 3),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Active item composition follows the durable tier. Deactivated
            // rows stay evidence for migration but never enter the accounting
            // totals served here.
            let mut comp_stmt = conn.prepare(
                "SELECT h.yield_tier, l.item_name, \
                 SUM(l.quantity), \
                 COALESCE(SUM(l.value_ped), 0) \
                 FROM harvest_loot_items l JOIN harvest_events h ON h.id = l.harvest_id \
                 WHERE (?1 IS NULL OR h.timestamp >= ?1) \
                   AND l.deactivated_at IS NULL \
                 GROUP BY h.yield_tier, l.item_name",
            )?;
            let composition = comp_stmt
                .query_map([epoch_start], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2).unwrap_or(0),
                        as_float(row, 3),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((raw, composition))
        })
        .await?;

    let mut items_by_tier: std::collections::HashMap<HarvestYieldTier, Vec<HarvestLootItemRow>> =
        std::collections::HashMap::new();
    for (tier, item_name, quantity, value_ped) in composition {
        let tier = HarvestYieldTier::from_db(&tier);
        items_by_tier
            .entry(tier)
            .or_default()
            .push(HarvestLootItemRow {
                item_name,
                quantity,
                value_ped: eo_wire::normalizer::round_half_even(value_ped, 2),
            });
    }
    for items in items_by_tier.values_mut() {
        items.sort_by(|a, b| {
            b.value_ped
                .partial_cmp(&a.value_ped)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.item_name.cmp(&b.item_name))
        });
    }

    let mut tier_comparisons: Vec<HarvestTierRow> = raw
        .into_iter()
        .map(|(tier, swings, cost, loot_tt)| {
            let tier = HarvestYieldTier::from_db(&tier);
            HarvestTierRow {
                yield_tier: tier,
                swings,
                cycled: eo_wire::normalizer::round_half_even(cost, 2),
                returns: eo_wire::normalizer::round_half_even(loot_tt, 2),
                loot_rate: if cost > 0.0 {
                    eo_wire::normalizer::round_half_even(loot_tt / cost, 4)
                } else {
                    0.0
                },
                loot_items: items_by_tier.remove(&tier).unwrap_or_default(),
            }
        })
        .collect();
    tier_comparisons.sort_by_key(|row| row.yield_tier.sort_rank());
    Ok(HarvestData { tier_comparisons })
}

// ── hunting_activity_impl ──
//
// The revamped Hunting aggregate. The unit of the period filter is the
// SESSION (started inside the window): every axis then aggregates the same
// session set, so Overall, the Sessions rows, and the Targets rows reconcile
// exactly instead of drifting apart where a session straddles the boundary.
//
// Direct figures only. Cycled here is weapon plus enhancer cost at kill
// grain; loot TT is the kills' loot; PES is the session-grain activity skill
// total of sessions that hunted. Heal and armour are session-grain residues
// the interval contract cannot yet attribute below the session, so they are
// deliberately absent rather than smeared across only some rows.

/// One qualifying session's per-axis facts, merged from the kill-grain sums
/// and the materialised summary.
#[derive(Default, Clone)]
struct HuntingSessionAgg {
    definition_id: Option<i64>,
    started_at: f64,
    ended_at: Option<f64>,
    duration_hours: f64,
    kills: i64,
    cycled: f64,
    loot_tt: f64,
    pes: f64,
}

/// One activity signature's accumulating totals, keyed by its member set.
#[derive(Default)]
struct SignatureAgg {
    kills: i64,
    cycled: f64,
    loot_tt: f64,
    pes: f64,
    duration_hours: f64,
    confirmed_reward_ped: f64,
    realised_reward_markup: f64,
    realised_loot_markup: f64,
    reward_kinds: std::collections::BTreeSet<String>,
    reward_items: std::collections::BTreeMap<String, (i64, f64)>,
    loot_items: std::collections::BTreeMap<String, (i64, f64)>,
    expected: ExpectedHuntingAccumulator,
    reward_unverified: bool,
    /// The distinct interval-id tuples seen, i.e. the focused stretches.
    runs: std::collections::BTreeSet<Vec<i64>>,
}

/// One signature member: a quest by id or a named segment. Ordered so a
/// bundle's identity is stable whatever order the intervals were opened in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SignatureMember {
    Quest(i64),
    Segment(String),
}

/// A quest's authored facts, for the economics columns.
#[derive(Debug, Clone)]
struct QuestFacts {
    name: String,
    family_id: Option<i64>,
}

async fn hunting_activity_impl(
    db: &Db,
    epoch_start: Option<f64>,
) -> Result<HuntingActivityData, DbError> {
    // Summaries feed duration and PES; converge them first, on the writer.
    db.with_writer(|conn| crate::session_summary::heal_summaries(conn))
        .await?;
    db.with_reader(move |conn| hunting_activity_read(conn, epoch_start))
        .await
}

/// Sessions qualify by having at least one kill and starting inside the
/// window. Summaries fill duration and PES where they exist (ended,
/// non-degenerate sessions); a session without one still reports its
/// kill-grain figures rather than vanishing.
/// One (session, context, species, maturity) cell of the kill-grain pass.
///
/// This is the finest grain any Hunting consumer aggregates at, so a single
/// pass over `kills` at this grain serves every downstream fold (per-session
/// totals, species and maturity rows, per-context signature sums, the
/// unstamped remainder) instead of each consumer re-scanning the table.
struct HuntingKillGrainRow {
    session_id: String,
    context_id: Option<i64>,
    mob_species: String,
    mob_maturity: String,
    kills: i64,
    cycled: f64,
    loot_tt: f64,
}

/// One (session, context) cell of the skill-gain pass; the PES sibling of
/// [`HuntingKillGrainRow`].
struct HuntingPesGrainRow {
    session_id: String,
    context_id: Option<i64>,
    pes: f64,
}

/// The qualifying sessions with their per-session totals, plus the two
/// grain passes every other Hunting fold derives from.
#[allow(clippy::type_complexity)]
fn hunting_sessions(
    conn: &rusqlite::Connection,
    epoch_start: Option<f64>,
) -> Result<
    (
        std::collections::HashMap<String, HuntingSessionAgg>,
        Vec<HuntingKillGrainRow>,
        Vec<HuntingPesGrainRow>,
    ),
    DbError,
> {
    // Session facts first, so the grain fold can stamp each session's
    // definition without re-joining per consumer.
    let mut meta: std::collections::HashMap<String, (Option<i64>, f64, Option<f64>)> =
        std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, definition_id, started_at, ended_at FROM tracking_sessions \
             WHERE (?1 IS NULL OR started_at >= ?1)",
        )?;
        let mut rows = stmt.query(rusqlite::params![epoch_start])?;
        while let Some(row) = rows.next()? {
            meta.insert(
                row.get::<_, String>(0)?,
                (
                    row.get(1)?,
                    row.get::<_, f64>(2).unwrap_or(0.0),
                    row.get(3)?,
                ),
            );
        }
    }

    // The kill grain, hybrid: settled sessions fold from their rollup
    // cells (O(cells), not O(kills)); every other session (the live one,
    // a freshly edited one, a stale-versioned one) aggregates raw, scoped
    // to its own id, so the read is correct whatever the heal has or has
    // not done yet. The session-metadata join keeps the standing
    // semantics either way: a kill whose session the tracker never
    // recorded stays out of Hunting.
    let unsettled: Vec<String> = crate::session_rollup::unsettled_sessions(conn)?
        .into_iter()
        .filter(|id| meta.contains_key(id))
        .collect();
    let mut grain: Vec<HuntingKillGrainRow> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT r.session_id, r.context_id, r.mob_species, r.mob_maturity, \
                    r.kills, r.cycled_ped, r.loot_tt \
             FROM session_kill_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?2 \
             JOIN tracking_sessions s ON s.id = r.session_id \
             WHERE (?1 IS NULL OR s.started_at >= ?1)",
        )?;
        let mut rows = stmt.query(rusqlite::params![
            epoch_start,
            crate::session_rollup::ROLLUP_VERSION
        ])?;
        while let Some(row) = rows.next()? {
            grain.push(HuntingKillGrainRow {
                session_id: row.get(0)?,
                context_id: row.get(1)?,
                mob_species: row.get(2)?,
                mob_maturity: row.get(3)?,
                kills: row.get::<_, i64>(4).unwrap_or(0),
                cycled: as_float(row, 5),
                loot_tt: as_float(row, 6),
            });
        }
    }
    if !unsettled.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT k.context_id, \
                    COALESCE(k.mob_species, ''), COALESCE(k.mob_maturity, ''), \
                    COUNT(*), \
                    COALESCE(SUM(k.cost_ped + k.enhancer_cost), 0), \
                    COALESCE(SUM(k.loot_total_ped), 0) \
             FROM kills k \
             WHERE k.session_id = ?1 \
             GROUP BY 1, 2, 3",
        )?;
        for session_id in &unsettled {
            let mut rows = stmt.query(rusqlite::params![session_id])?;
            while let Some(row) = rows.next()? {
                grain.push(HuntingKillGrainRow {
                    session_id: session_id.clone(),
                    context_id: row.get(0)?,
                    mob_species: row.get(1)?,
                    mob_maturity: row.get(2)?,
                    kills: row.get::<_, i64>(3).unwrap_or(0),
                    cycled: as_float(row, 4),
                    loot_tt: as_float(row, 5),
                });
            }
        }
    }

    let mut sessions: std::collections::HashMap<String, HuntingSessionAgg> =
        std::collections::HashMap::new();
    for cell in &grain {
        let Some((definition_id, started_at, ended_at)) = meta.get(&cell.session_id) else {
            continue;
        };
        let agg = sessions
            .entry(cell.session_id.clone())
            .or_insert_with(|| HuntingSessionAgg {
                definition_id: *definition_id,
                started_at: *started_at,
                ended_at: *ended_at,
                ..HuntingSessionAgg::default()
            });
        agg.kills += cell.kills;
        agg.cycled += cell.cycled;
        agg.loot_tt += cell.loot_tt;
    }

    {
        let mut stmt = conn.prepare("SELECT session_id, duration_hours FROM session_summaries")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id = row.get::<_, String>(0)?;
            if let Some(agg) = sessions.get_mut(&id) {
                agg.duration_hours = as_float(row, 1);
            }
        }
    }
    // A session the summaries never adopted (degenerate duration or cycled)
    // still reports the wall-clock span it actually ran.
    for agg in sessions.values_mut() {
        if agg.duration_hours == 0.0 {
            if let Some(ended) = agg.ended_at {
                agg.duration_hours = (ended - agg.started_at).max(0.0) / 3600.0;
            }
        }
    }

    // The skill-gain grain, (session, context), hybrid on the same split.
    // PES stays on the raw per-session basis, so the definition totals,
    // the signature rows, and the ambient remainder all sum the same fact.
    let mut pes_grain: Vec<HuntingPesGrainRow> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT r.session_id, r.context_id, r.pes \
             FROM session_pes_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![crate::session_rollup::ROLLUP_VERSION])?;
        while let Some(row) = rows.next()? {
            pes_grain.push(HuntingPesGrainRow {
                session_id: row.get(0)?,
                context_id: row.get(1)?,
                pes: as_float(row, 2),
            });
        }
    }
    if !unsettled.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT context_id, COALESCE(SUM(ped_value), 0) FROM skill_gains \
             WHERE session_id = ?1 AND ped_value IS NOT NULL GROUP BY 1",
        )?;
        for session_id in &unsettled {
            let mut rows = stmt.query(rusqlite::params![session_id])?;
            while let Some(row) = rows.next()? {
                pes_grain.push(HuntingPesGrainRow {
                    session_id: session_id.clone(),
                    context_id: row.get(0)?,
                    pes: as_float(row, 1),
                });
            }
        }
    }
    for cell in &pes_grain {
        if let Some(agg) = sessions.get_mut(&cell.session_id) {
            agg.pes += cell.pes;
        }
    }

    Ok((sessions, grain, pes_grain))
}

#[derive(Clone, Default)]
struct ExpectedHuntingAccumulator {
    expected_loot_tt: f64,
    modelled_raw_tt: f64,
    eligible_cost: f64,
    candidate_raw_tt: f64,
    looter_weight: f64,
    incomplete: bool,
    missing_basis_phases: i64,
}

impl ExpectedHuntingAccumulator {
    fn add(&mut self, evidence: &crate::expected_hunting::OffensiveLoadoutEvidence, shots: i64) {
        if shots <= 0 {
            return;
        }
        let uses = shots as f64;
        let candidate: f64 = evidence
            .components
            .iter()
            .map(|component| component.raw_tt_per_use.max(0.0))
            .sum::<f64>()
            * uses;
        let Ok(result) = crate::expected_hunting::evaluate(evidence) else {
            self.candidate_raw_tt += candidate;
            self.incomplete = true;
            return;
        };
        self.expected_loot_tt += result.expected_loot_tt * uses;
        self.modelled_raw_tt += result.modelled_raw_tt * uses;
        self.eligible_cost += result.eligible_offensive_cost * uses;
        self.candidate_raw_tt += candidate;
        self.looter_weight += result.looter_level * result.modelled_raw_tt * uses;
        self.incomplete |= result.incomplete;
    }

    fn mark_missing(&mut self, candidate_raw_tt: f64, missing_basis_phases: i64) {
        if candidate_raw_tt > 0.0 && missing_basis_phases > 0 {
            self.candidate_raw_tt += candidate_raw_tt;
            self.missing_basis_phases += missing_basis_phases;
            self.incomplete = true;
        }
    }

    fn merge(&mut self, other: &Self) {
        self.expected_loot_tt += other.expected_loot_tt;
        self.modelled_raw_tt += other.modelled_raw_tt;
        self.eligible_cost += other.eligible_cost;
        self.candidate_raw_tt += other.candidate_raw_tt;
        self.looter_weight += other.looter_weight;
        self.incomplete |= other.incomplete;
        self.missing_basis_phases += other.missing_basis_phases;
    }

    fn finish(&self) -> Option<ExpectedHuntingAggregate> {
        if self.modelled_raw_tt <= 0.0 || self.eligible_cost <= 0.0 {
            return None;
        }
        let round = |value: f64, places| eo_wire::normalizer::round_half_even(value, places);
        let tt_recovery = self.expected_loot_tt / self.modelled_raw_tt;
        let tt_rate = self.expected_loot_tt / self.eligible_cost;
        let looter_level = self.looter_weight / self.modelled_raw_tt;
        let effective_efficiency =
            crate::expected_hunting::effective_efficiency_v1(tt_rate, looter_level).ok()?;
        Some(ExpectedHuntingAggregate {
            model_version: crate::expected_hunting::COMMUNITY_MODEL_V1.to_string(),
            looter_source: "three_looter_mean".to_string(),
            looter_level: round(looter_level, 2),
            expected_loot_tt: round(self.expected_loot_tt, 4),
            modelled_raw_tt: round(self.modelled_raw_tt, 4),
            eligible_offensive_cost: round(self.eligible_cost, 4),
            offensive_tt_recovery: round(tt_recovery, 6),
            expected_tt_rate: round(tt_rate, 6),
            effective_efficiency,
            break_even_loot_markup: round(1.0 / tt_rate, 6),
            coverage: round(
                if self.candidate_raw_tt > 0.0 {
                    self.modelled_raw_tt / self.candidate_raw_tt
                } else {
                    0.0
                },
                4,
            ),
            incomplete: self.incomplete,
            missing_basis_phases: self.missing_basis_phases,
        })
    }
}

#[allow(clippy::type_complexity)]
fn hunting_expected_aggregates(
    conn: &rusqlite::Connection,
    epoch_start: Option<f64>,
    sessions: &std::collections::HashMap<String, HuntingSessionAgg>,
) -> Result<
    (
        Option<ExpectedHuntingAggregate>,
        std::collections::HashMap<Option<i64>, ExpectedHuntingAggregate>,
        std::collections::HashMap<String, ExpectedHuntingAggregate>,
        std::collections::HashMap<(String, Option<i64>), ExpectedHuntingAccumulator>,
    ),
    DbError,
> {
    use std::collections::HashMap;

    let mut overall = ExpectedHuntingAccumulator::default();
    let mut definitions: HashMap<Option<i64>, ExpectedHuntingAccumulator> = HashMap::new();
    let mut species: HashMap<String, ExpectedHuntingAccumulator> = HashMap::new();
    let mut contexts: HashMap<(String, Option<i64>), ExpectedHuntingAccumulator> = HashMap::new();
    let cells = crate::session_rollup::effective_offensive_evidence_cells(conn, epoch_start)?;
    for cell in cells {
        let Some(session) = sessions.get(&cell.session_id) else {
            continue;
        };
        let definition = definitions.entry(session.definition_id).or_default();
        let target = species.entry(cell.mob_species).or_default();
        let context = contexts
            .entry((cell.session_id.clone(), cell.context_id))
            .or_default();
        if let Some(encoded) = cell.expected_economics_json {
            let evidence =
                serde_json::from_str::<crate::expected_hunting::OffensiveLoadoutEvidence>(&encoded)
                    .map_err(|source| DbError::Decode {
                        context: "kill tool expected economics decode",
                        source,
                    })?;
            overall.add(&evidence, cell.shots_fired);
            definition.add(&evidence, cell.shots_fired);
            target.add(&evidence, cell.shots_fired);
            context.add(&evidence, cell.shots_fired);
        } else {
            overall.mark_missing(cell.missing_candidate_raw_tt, cell.missing_basis_phases);
            definition.mark_missing(cell.missing_candidate_raw_tt, cell.missing_basis_phases);
            target.mark_missing(cell.missing_candidate_raw_tt, cell.missing_basis_phases);
            context.mark_missing(cell.missing_candidate_raw_tt, cell.missing_basis_phases);
        }
    }

    Ok((
        overall.finish(),
        definitions
            .into_iter()
            .filter_map(|(key, value)| value.finish().map(|value| (key, value)))
            .collect(),
        species
            .into_iter()
            .filter_map(|(key, value)| value.finish().map(|value| (key, value)))
            .collect(),
        contexts,
    ))
}

#[allow(clippy::too_many_lines)]
fn hunting_activity_read(
    conn: &rusqlite::Connection,
    epoch_start: Option<f64>,
) -> Result<HuntingActivityData, DbError> {
    use std::collections::{BTreeMap, HashMap};

    let (sessions, kill_grain, pes_grain) = hunting_sessions(conn, epoch_start)?;
    let (overall_expected, mut definition_expected, mut species_expected, mut context_expected) =
        hunting_expected_aggregates(conn, epoch_start, &sessions)?;
    let round2 = |value: f64| eo_wire::normalizer::round_half_even(value, 2);
    let round4 = |value: f64| eo_wire::normalizer::round_half_even(value, 4);
    let rate = |returns: f64, cycled: f64| {
        if cycled > 0.0 {
            round4(returns / cycled)
        } else {
            0.0
        }
    };
    let pes_per100 = |pes: f64, cycled: f64| {
        if cycled > 0.0 {
            round2((pes / cycled) * 100.0)
        } else {
            0.0
        }
    };

    // ── Overall ──
    let overall = {
        let kills: i64 = sessions.values().map(|s| s.kills).sum();
        let cycled: f64 = sessions.values().map(|s| s.cycled).sum();
        let loot_tt: f64 = sessions.values().map(|s| s.loot_tt).sum();
        let pes: f64 = sessions.values().map(|s| s.pes).sum();
        let duration: f64 = sessions.values().map(|s| s.duration_hours).sum();
        HuntingOverall {
            sessions: sessions.len() as i64,
            kills,
            duration_hours: round2(duration),
            cycled: round2(cycled),
            returns: round2(loot_tt),
            loot_rate: rate(loot_tt, cycled),
            pes: round4(pes),
            pes_per100_ped: pes_per100(pes, cycled),
            expected: overall_expected,
        }
    };

    if sessions.is_empty() {
        return Ok(HuntingActivityData {
            overall,
            definitions: Vec::new(),
            species: Vec::new(),
        });
    }

    // The qualifying session ids, bound into the per-axis queries through a
    // temp table so the kill-grain reads stay scoped without an IN-list.
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS hunting_session_scope (id TEXT PRIMARY KEY); \
         DELETE FROM hunting_session_scope;",
    )?;
    {
        let mut stmt = conn.prepare("INSERT INTO hunting_session_scope (id) VALUES (?)")?;
        for id in sessions.keys() {
            stmt.execute(rusqlite::params![id])?;
        }
    }

    let effective_loot = crate::session_rollup::effective_hunting_loot_cells(conn, epoch_start)?;

    // ── Targets: species and maturity, kill grain ──
    #[derive(Default)]
    struct MaturityAgg {
        kills: i64,
        cycled: f64,
        loot_tt: f64,
    }
    let mut species_maturity: BTreeMap<String, BTreeMap<String, MaturityAgg>> = BTreeMap::new();
    for cell in &kill_grain {
        let entry = species_maturity
            .entry(cell.mob_species.clone())
            .or_default()
            .entry(cell.mob_maturity.clone())
            .or_default();
        entry.kills += cell.kills;
        entry.cycled += cell.cycled;
        entry.loot_tt += cell.loot_tt;
    }

    // Species loot composition (mob loot only: enhancer-shrapnel returns are
    // enhancer accounting, and deactivated rows are archived out of totals).
    // The unclassified bucket keeps its own composition: the loot is real
    // and only its attribution is missing, and dropping it would make the
    // Overall MU numerator exclude cost the denominator still carries.
    let mut species_item_cells: HashMap<String, BTreeMap<String, (i64, f64)>> = HashMap::new();
    let mut definition_item_cells: HashMap<Option<i64>, BTreeMap<String, (i64, f64)>> =
        HashMap::new();
    for cell in effective_loot
        .iter()
        .filter(|cell| !cell.is_enhancer_shrapnel && sessions.contains_key(&cell.session_id))
    {
        let species = species_item_cells
            .entry(cell.mob_species.clone())
            .or_default()
            .entry(cell.item_name.clone())
            .or_insert((0, 0.0));
        species.0 += cell.quantity;
        species.1 += cell.value_ped;
        let definition = definition_item_cells
            .entry(cell.definition_id)
            .or_default()
            .entry(cell.item_name.clone())
            .or_insert((0, 0.0));
        definition.0 += cell.quantity;
        definition.1 += cell.value_ped;
    }
    let into_items = |cells: BTreeMap<String, (i64, f64)>| {
        let mut items: Vec<HarvestLootItemRow> = cells
            .into_iter()
            .map(|(item_name, (quantity, value_ped))| HarvestLootItemRow {
                item_name,
                quantity,
                value_ped: round2(value_ped),
            })
            .collect();
        items.sort_by(|a, b| {
            b.value_ped
                .partial_cmp(&a.value_ped)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.item_name.cmp(&b.item_name))
        });
        items
    };
    let mut species_items: HashMap<String, Vec<HarvestLootItemRow>> = species_item_cells
        .into_iter()
        .map(|(species, cells)| (species, into_items(cells)))
        .collect();

    // The same loot evidence projected through the user-designated axis.
    // Definitions can grow to hundreds of items, so this is one set-based
    // pass for every row rather than a per-definition query.
    let mut definition_items: HashMap<Option<i64>, Vec<HarvestLootItemRow>> = definition_item_cells
        .into_iter()
        .map(|(definition, cells)| (definition, into_items(cells)))
        .collect();

    // Species PES through session dominance: skill gains carry no per-kill
    // attribution, so a species may claim a session's skill total only when
    // its kills dominated that session. Anything thinner stays unclaimed.
    let mut session_species_kills: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    {
        // The grain is finer than (session, species), so fold to a map
        // first; the dominance walk below wants one count per species.
        let mut per_session: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
        for cell in &kill_grain {
            *per_session
                .entry(cell.session_id.clone())
                .or_default()
                .entry(cell.mob_species.clone())
                .or_insert(0) += cell.kills;
        }
        for (session, counts) in per_session {
            session_species_kills.insert(session, counts.into_iter().collect());
        }
    }
    let mut species_pes: HashMap<String, (f64, f64, i64)> = HashMap::new();
    for (session_id, mut counts) in session_species_kills {
        let Some(agg) = sessions.get(&session_id) else {
            continue;
        };
        counts.retain(|(species, _)| !species.is_empty());
        let total: i64 = counts.iter().map(|(_, kills)| kills).sum();
        if total == 0 {
            continue;
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let (top_species, top_kills) = counts[0].clone();
        if top_kills as f64 / total as f64 >= ACTIVITY_DOMINANCE_THRESHOLD {
            let entry = species_pes.entry(top_species).or_insert((0.0, 0.0, 0));
            entry.0 += agg.pes;
            entry.1 += agg.cycled;
            entry.2 += 1;
        }
    }

    let mut species_rows: Vec<HuntingSpeciesRow> = species_maturity
        .into_iter()
        .map(|(species, maturities)| {
            let kills: i64 = maturities.values().map(|m| m.kills).sum();
            let cycled: f64 = maturities.values().map(|m| m.cycled).sum();
            let loot_tt: f64 = maturities.values().map(|m| m.loot_tt).sum();
            let mut maturity_rows: Vec<HuntingMaturityRow> = maturities
                .into_iter()
                .map(|(maturity, agg)| HuntingMaturityRow {
                    maturity,
                    kills: agg.kills,
                    cycled: round2(agg.cycled),
                    returns: round2(agg.loot_tt),
                    loot_rate: rate(agg.loot_tt, agg.cycled),
                })
                .collect();
            maturity_rows.sort_by(|a, b| b.kills.cmp(&a.kills).then(a.maturity.cmp(&b.maturity)));
            let dominated = species_pes.get(&species);
            HuntingSpeciesRow {
                loot_items: species_items.remove(&species).unwrap_or_default(),
                pes: dominated.map(|(pes, _, _)| round4(*pes)),
                pes_per100_ped: dominated.map(|(pes, cycled, _)| pes_per100(*pes, *cycled)),
                pes_sessions: dominated.map(|(_, _, count)| *count).unwrap_or(0),
                expected: species_expected.remove(&species),
                mob_species: species,
                kills,
                cycled: round2(cycled),
                returns: round2(loot_tt),
                loot_rate: rate(loot_tt, cycled),
                maturities: maturity_rows,
            }
        })
        .collect();
    // Busiest first by cycled, with the unclassified bucket pinned last.
    species_rows.sort_by(|a, b| {
        (a.mob_species.is_empty() as u8)
            .cmp(&(b.mob_species.is_empty() as u8))
            .then(
                b.cycled
                    .partial_cmp(&a.cycled)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| a.mob_species.cmp(&b.mob_species))
    });

    // ── Sessions: definitions, signatures, instances ──

    // Per-(session, species) loot for the definition mob composition.
    let mut definition_mobs: HashMap<Option<i64>, BTreeMap<String, (i64, f64)>> = HashMap::new();
    for cell in &kill_grain {
        if cell.mob_species.is_empty() {
            continue;
        }
        let Some(agg) = sessions.get(&cell.session_id) else {
            continue;
        };
        let entry = definition_mobs
            .entry(agg.definition_id)
            .or_default()
            .entry(cell.mob_species.clone())
            .or_insert((0, 0.0));
        entry.0 += cell.kills;
        entry.1 += cell.loot_tt;
    }

    // The signature substrate: contexts, their quest/segment interval sets,
    // and per-context event totals, attributed by stamp, never by timestamp.
    let quest_facts: HashMap<i64, QuestFacts> = {
        let mut stmt = conn.prepare("SELECT id, name, family_id FROM quests")?;
        let mut rows = stmt.query([])?;
        let mut out = HashMap::new();
        while let Some(row) = rows.next()? {
            out.insert(
                row.get::<_, i64>(0)?,
                QuestFacts {
                    name: row.get(1)?,
                    family_id: row.get(2)?,
                },
            );
        }
        out
    };
    let family_names: HashMap<i64, String> = {
        let mut stmt = conn.prepare("SELECT id, name FROM quest_families")?;
        let mut rows = stmt.query([])?;
        let mut out = HashMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get::<_, i64>(0)?, row.get::<_, String>(1)?);
        }
        out
    };

    // context id -> (session, created_at); ordered per session for spans.
    let mut contexts_by_session: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.session_id, c.created_at \
             FROM session_contexts c \
             JOIN hunting_session_scope scope ON scope.id = c.session_id \
             ORDER BY c.session_id, c.created_at, c.id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let session: String = row.get(1)?;
            let created: f64 = row.get(2)?;
            contexts_by_session
                .entry(session)
                .or_default()
                .push((id, created));
        }
    }
    // context id -> quest/segment interval members.
    let mut context_members: HashMap<i64, Vec<(i64, SignatureMember)>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT sci.context_id, i.id, i.kind, i.label, i.ref_id \
             FROM session_context_intervals sci \
             JOIN session_intervals i ON i.id = sci.interval_id \
             JOIN session_contexts c ON c.id = sci.context_id \
             JOIN hunting_session_scope scope ON scope.id = c.session_id \
             WHERE i.kind IN ('quest', 'segment')",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let context: i64 = row.get(0)?;
            let interval: i64 = row.get(1)?;
            let kind: String = row.get(2)?;
            let label: Option<String> = row.get(3)?;
            let ref_id: Option<i64> = row.get(4)?;
            let member = match kind.as_str() {
                "quest" => match ref_id {
                    Some(quest_id) => SignatureMember::Quest(quest_id),
                    None => SignatureMember::Segment(label.unwrap_or_default()),
                },
                _ => SignatureMember::Segment(label.unwrap_or_default()),
            };
            context_members
                .entry(context)
                .or_default()
                .push((interval, member));
        }
    }
    // Per-context event totals, by stamp.
    let mut context_kills: HashMap<i64, (i64, f64, f64)> = HashMap::new();
    // Events that never got a context stamp (legacy sessions and pre-model
    // rows), per session, so they can join their definition's ambient
    // remainder rather than silently dropping out of the breakdown.
    let mut legacy_kills_by_session: HashMap<String, (i64, f64, f64)> = HashMap::new();
    for cell in &kill_grain {
        match cell.context_id {
            Some(context) => {
                let entry = context_kills.entry(context).or_insert((0, 0.0, 0.0));
                entry.0 += cell.kills;
                entry.1 += cell.cycled;
                entry.2 += cell.loot_tt;
            }
            None => {
                let entry = legacy_kills_by_session
                    .entry(cell.session_id.clone())
                    .or_insert((0, 0.0, 0.0));
                entry.0 += cell.kills;
                entry.1 += cell.cycled;
                entry.2 += cell.loot_tt;
            }
        }
    }
    // The PES grain is session-unfiltered (its per-session totals serve
    // every session), so the context folds re-scope to qualifying sessions
    // exactly as the scope-joined queries did.
    let mut context_pes: HashMap<i64, f64> = HashMap::new();
    let mut legacy_pes_by_session: HashMap<String, f64> = HashMap::new();
    for cell in &pes_grain {
        if !sessions.contains_key(&cell.session_id) {
            continue;
        }
        match cell.context_id {
            Some(context) => {
                *context_pes.entry(context).or_insert(0.0) += cell.pes;
            }
            None => {
                *legacy_pes_by_session
                    .entry(cell.session_id.clone())
                    .or_insert(0.0) += cell.pes;
            }
        }
    }

    // Item composition at the same context grain as direct cost and loot.
    // Settled sessions read the maintained projection; only the live or
    // otherwise-unsettled sessions touch raw loot rows.
    let mut context_items: HashMap<i64, BTreeMap<String, (i64, f64)>> = HashMap::new();
    let unsettled = crate::session_rollup::unsettled_sessions(conn)?;
    {
        let mut stmt = conn.prepare(
            "SELECT r.context_id, r.item_name, r.quantity, r.value_ped \
             FROM session_context_loot_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?1 \
             JOIN hunting_session_scope scope ON scope.id = r.session_id \
             WHERE r.context_id IS NOT NULL",
        )?;
        let mut rows = stmt.query(rusqlite::params![crate::session_rollup::ROLLUP_VERSION])?;
        while let Some(row) = rows.next()? {
            let context: i64 = row.get(0)?;
            context_items.entry(context).or_default().insert(
                row.get(1)?,
                (row.get::<_, i64>(2).unwrap_or(0), as_float(row, 3)),
            );
        }
    }
    if !unsettled.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT k.context_id, li.item_name, SUM(li.quantity), \
                    COALESCE(SUM(li.value_ped), 0) \
             FROM kill_loot_items li \
             JOIN kills k ON k.id = li.kill_id \
             WHERE k.session_id = ?1 AND k.context_id IS NOT NULL \
               AND li.deactivated_at IS NULL AND li.is_enhancer_shrapnel = 0 \
             GROUP BY k.context_id, li.item_name",
        )?;
        for session_id in &unsettled {
            if !sessions.contains_key(session_id) {
                continue;
            }
            let mut rows = stmt.query(rusqlite::params![session_id])?;
            while let Some(row) = rows.next()? {
                let context: i64 = row.get(0)?;
                context_items.entry(context).or_default().insert(
                    row.get(1)?,
                    (row.get::<_, i64>(2).unwrap_or(0), as_float(row, 3)),
                );
            }
        }
    }

    // Completion-time reward facts. A NULL kind is deliberately not
    // valued: it names a legacy completion whose current quest definition
    // must never rewrite its history.
    let mut context_rewards: HashMap<i64, (f64, BTreeSet<String>)> = HashMap::new();
    let mut context_reward_items: HashMap<i64, BTreeMap<String, (i64, f64)>> = HashMap::new();
    let mut legacy_quests: HashMap<Option<i64>, BTreeSet<i64>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT sqc.session_id, sqc.quest_id, sqc.activity_context_id, \
                    CASE \
                      WHEN (SELECT r.outcome FROM quest_reward_reviews r \
                            WHERE r.completion_id = sqc.id \
                            ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1) = 'none' \
                        THEN 'none' \
                      WHEN (SELECT r.outcome FROM quest_reward_reviews r \
                            WHERE r.completion_id = sqc.id \
                            ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1) = 'confirmed' \
                        THEN 'item' \
                      ELSE sqc.reward_kind \
                    END, sqc.reward_ped, \
                    COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                              WHERE r.completion_id = sqc.id \
                              ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                             sqc.reward_outcome) \
             FROM session_quest_completions sqc \
             JOIN hunting_session_scope scope ON scope.id = sqc.session_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let quest_id: i64 = row.get(1)?;
            let context_id: Option<i64> = row.get(2)?;
            let kind: Option<String> = row.get(3)?;
            let outcome: Option<String> = row.get(5)?;
            if outcome.as_deref() == Some("unresolved") {
                if let Some(session) = sessions.get(&session_id) {
                    legacy_quests
                        .entry(session.definition_id)
                        .or_default()
                        .insert(quest_id);
                }
                continue;
            }
            let Some(kind) = kind else {
                if let Some(session) = sessions.get(&session_id) {
                    legacy_quests
                        .entry(session.definition_id)
                        .or_default()
                        .insert(quest_id);
                }
                continue;
            };
            let Some(context_id) = context_id else {
                continue;
            };
            let reward = context_rewards
                .entry(context_id)
                .or_insert_with(|| (0.0, BTreeSet::new()));
            if kind != "none" {
                reward.1.insert(kind);
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT a.activity_context_id, ri.item_name, \
                    SUM(ri.quantity * a.weight), COALESCE(SUM(ri.value_ped * a.weight), 0) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions sqc ON sqc.id = ri.completion_id \
             JOIN quest_reward_attributions a ON a.completion_id = sqc.id \
             JOIN hunting_session_scope scope ON scope.id = sqc.session_id \
             WHERE COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = sqc.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            sqc.reward_outcome) = 'confirmed' \
               AND NOT EXISTS(SELECT 1 FROM quest_reward_reversals rr \
                              WHERE rr.completion_id = sqc.id) \
             GROUP BY a.activity_context_id, ri.item_name",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let context_id = row.get(0)?;
            let value = as_float(row, 3);
            context_reward_items
                .entry(context_id)
                .or_default()
                .insert(row.get(1)?, (as_float(row, 2).round() as i64, value));
            let reward = context_rewards.entry(context_id).or_default();
            reward.0 += value;
            reward.1.insert("item".to_string());
        }
    }

    // A later stock outcome realises only markup beyond the item's TT
    // principal. Project that gain back through the immutable activity
    // context carried by each consumed movement. Quantity is the allocation
    // basis so a zero-TT reward voucher remains attributable.
    let mut context_realised_reward_markup: HashMap<i64, f64> = HashMap::new();
    let mut context_realised_loot_markup: HashMap<i64, f64> = HashMap::new();
    for realised in realised_stock_outcomes(conn)? {
        if realised.outcome.activity_net_markup.abs() <= STOCK_EPSILON {
            continue;
        }
        let contributions: Vec<(String, Option<i64>, f64)> = {
            let mut stmt = conn.prepare(
                "SELECT source_kind, activity_context_id, SUM(-quantity) \
                 FROM stock_movements WHERE ref_id = ? AND movement_kind = ? \
                   AND source_kind IN ('hunt', 'harvest', 'quest') \
                 GROUP BY source_kind, activity_context_id",
            )?;
            let mapped = stmt.query_map(
                rusqlite::params![realised.id, realised.movement_kind],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let attributed_quantity: f64 = contributions.iter().map(|(_, _, qty)| qty).sum();
        if attributed_quantity <= STOCK_EPSILON {
            continue;
        }
        for (source_kind, context_id, quantity) in contributions {
            let Some(context_id) = context_id else {
                continue;
            };
            let attributed_markup =
                realised.outcome.activity_net_markup * quantity / attributed_quantity;
            match source_kind.as_str() {
                "hunt" => {
                    *context_realised_loot_markup.entry(context_id).or_default() +=
                        attributed_markup;
                }
                "quest" => {
                    *context_realised_reward_markup
                        .entry(context_id)
                        .or_default() += attributed_markup;
                }
                _ => {}
            }
        }
    }

    // Fold contexts into per-definition signature aggregates. A context's
    // span is the stretch until the next context (or session end), which is
    // sound because a fresh context is minted on every change.
    let mut signatures: HashMap<Option<i64>, BTreeMap<Vec<SignatureMember>, SignatureAgg>> =
        HashMap::new();
    for (session_id, contexts) in &contexts_by_session {
        let Some(session) = sessions.get(session_id) else {
            continue;
        };
        let session_close = session.ended_at;
        for (index, (context_id, created_at)) in contexts.iter().enumerate() {
            let span_end = contexts
                .get(index + 1)
                .map(|(_, next_created)| *next_created)
                .or(session_close);
            let span_hours = span_end
                .map(|end| ((end - created_at).max(0.0)) / 3600.0)
                .unwrap_or(0.0);

            let mut members: Vec<(i64, SignatureMember)> =
                context_members.get(context_id).cloned().unwrap_or_default();
            members.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
            let key: Vec<SignatureMember> =
                members.iter().map(|(_, member)| member.clone()).collect();
            let interval_ids: Vec<i64> = members.iter().map(|(id, _)| *id).collect();

            let reward_unverified =
                legacy_quests
                    .get(&session.definition_id)
                    .is_some_and(|quests| {
                        key.iter().any(|member| {
                        matches!(member, SignatureMember::Quest(id) if quests.contains(id))
                    })
                    });

            let agg = signatures
                .entry(session.definition_id)
                .or_default()
                .entry(key)
                .or_default();
            if let Some(expected) =
                context_expected.remove(&(session_id.clone(), Some(*context_id)))
            {
                agg.expected.merge(&expected);
            }
            if let Some((kills, cycled, loot)) = context_kills.get(context_id) {
                agg.kills += kills;
                agg.cycled += cycled;
                agg.loot_tt += loot;
            }
            if let Some(pes) = context_pes.get(context_id) {
                agg.pes += pes;
            }
            if let Some((reward_ped, kinds)) = context_rewards.get(context_id) {
                agg.confirmed_reward_ped += reward_ped;
                agg.reward_kinds.extend(kinds.iter().cloned());
            }
            agg.realised_reward_markup += context_realised_reward_markup
                .get(context_id)
                .copied()
                .unwrap_or(0.0);
            agg.realised_loot_markup += context_realised_loot_markup
                .get(context_id)
                .copied()
                .unwrap_or(0.0);
            if let Some(items) = context_reward_items.get(context_id) {
                for (name, (quantity, value)) in items {
                    let item = agg.reward_items.entry(name.clone()).or_insert((0, 0.0));
                    item.0 += quantity;
                    item.1 += value;
                }
            }
            if let Some(items) = context_items.get(context_id) {
                for (name, (quantity, value)) in items {
                    let item = agg.loot_items.entry(name.clone()).or_insert((0, 0.0));
                    item.0 += quantity;
                    item.1 += value;
                }
            }
            agg.reward_unverified |= reward_unverified;
            agg.duration_hours += span_hours;
            if !interval_ids.is_empty() {
                agg.runs.insert(interval_ids);
            }
        }
    }
    // Unstamped events fold into their definition's ambient remainder (the
    // empty signature). Their sessions may predate contexts entirely, so no
    // duration is claimed for them: the stamps say what happened, not when
    // within the session it did.
    for (id, agg) in &sessions {
        let kills = legacy_kills_by_session.get(id);
        let pes = legacy_pes_by_session.get(id);
        if kills.is_none() && pes.is_none() {
            continue;
        }
        let ambient = signatures
            .entry(agg.definition_id)
            .or_default()
            .entry(Vec::new())
            .or_default();
        if let Some(expected) = context_expected.remove(&(id.clone(), None)) {
            ambient.expected.merge(&expected);
        }
        if let Some((kills, cycled, loot)) = kills {
            ambient.kills += kills;
            ambient.cycled += cycled;
            ambient.loot_tt += loot;
        }
        if let Some(pes) = pes {
            ambient.pes += pes;
        }
    }

    // ── Assemble the definition rows ──
    let definition_names: HashMap<i64, (String, bool)> = {
        let mut stmt = conn.prepare("SELECT id, name, is_active FROM session_definitions")?;
        let mut rows = stmt.query([])?;
        let mut out = HashMap::new();
        while let Some(row) = rows.next()? {
            out.insert(
                row.get::<_, i64>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2).unwrap_or(1) == 0,
                ),
            );
        }
        out
    };

    let mut by_definition: HashMap<Option<i64>, Vec<(&String, &HuntingSessionAgg)>> =
        HashMap::new();
    for (id, agg) in &sessions {
        by_definition
            .entry(agg.definition_id)
            .or_default()
            .push((id, agg));
    }

    let mut definition_rows: Vec<HuntingDefinitionRow> = by_definition
        .into_iter()
        .map(|(definition_id, group)| {
            let kills: i64 = group.iter().map(|(_, s)| s.kills).sum();
            let cycled: f64 = group.iter().map(|(_, s)| s.cycled).sum();
            let loot_tt: f64 = group.iter().map(|(_, s)| s.loot_tt).sum();
            let pes: f64 = group.iter().map(|(_, s)| s.pes).sum();
            let duration: f64 = group.iter().map(|(_, s)| s.duration_hours).sum();

            let mut instance_rows: Vec<HuntingInstanceRow> = group
                .iter()
                .map(|(id, s)| HuntingInstanceRow {
                    session_id: (*id).clone(),
                    started_at: s.started_at,
                    duration_hours: round2(s.duration_hours),
                    kills: s.kills,
                    cycled: round2(s.cycled),
                    returns: round2(s.loot_tt),
                    pes: round4(s.pes),
                })
                .collect();
            instance_rows.sort_by(|a, b| {
                b.started_at
                    .partial_cmp(&a.started_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            });
            // The rows exist for the trend read and recent review, not as an
            // exhaustive scrollback; `instances` still reports the true
            // count, so a capped list can say what it is showing.
            instance_rows.truncate(50);

            let mobs: Vec<HuntingMobShareRow> = definition_mobs
                .remove(&definition_id)
                .map(|composition| {
                    let mut rows: Vec<HuntingMobShareRow> = composition
                        .into_iter()
                        .map(|(species, (kills, loot))| HuntingMobShareRow {
                            mob_species: species,
                            kills,
                            loot_tt: round2(loot),
                        })
                        .collect();
                    rows.sort_by(|a, b| {
                        b.kills
                            .cmp(&a.kills)
                            .then_with(|| a.mob_species.cmp(&b.mob_species))
                    });
                    rows
                })
                .unwrap_or_default();

            let activities = assemble_signatures(
                signatures.remove(&definition_id).unwrap_or_default(),
                &quest_facts,
                &family_names,
            );

            let (name, is_archived) = match definition_id {
                Some(id) => definition_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| (format!("Definition {id}"), false)),
                None => ("Unassigned".to_string(), false),
            };

            HuntingDefinitionRow {
                definition_id,
                name,
                is_archived,
                instances: group.len() as i64,
                kills,
                duration_hours: round2(duration),
                cycled: round2(cycled),
                returns: round2(loot_tt),
                loot_rate: rate(loot_tt, cycled),
                pes: round4(pes),
                pes_per100_ped: pes_per100(pes, cycled),
                expected: definition_expected.remove(&definition_id),
                activities,
                mobs,
                instance_rows,
                loot_items: definition_items.remove(&definition_id).unwrap_or_default(),
            }
        })
        .collect();
    // Busiest first by cycled, the unassigned bucket pinned last.
    definition_rows.sort_by(|a, b| {
        (a.definition_id.is_none() as u8)
            .cmp(&(b.definition_id.is_none() as u8))
            .then(
                b.cycled
                    .partial_cmp(&a.cycled)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| a.name.cmp(&b.name))
    });

    conn.execute("DELETE FROM hunting_session_scope", [])?;

    Ok(HuntingActivityData {
        overall,
        definitions: definition_rows,
        species: species_rows,
    })
}

/// Fold one definition's signature aggregates into display rows: quest
/// variants grouped under their family, standalone quests and segments as
/// their own rows, co-activations as joint bundles, and the ambient
/// remainder last.
fn assemble_signatures(
    signatures: std::collections::BTreeMap<Vec<SignatureMember>, SignatureAgg>,
    quest_facts: &std::collections::HashMap<i64, QuestFacts>,
    family_names: &std::collections::HashMap<i64, String>,
) -> Vec<HuntingSignatureRow> {
    let round2 = |value: f64| eo_wire::normalizer::round_half_even(value, 2);
    let round4 = |value: f64| eo_wire::normalizer::round_half_even(value, 4);

    let member_label = |member: &SignatureMember| -> String {
        match member {
            SignatureMember::Quest(id) => quest_facts
                .get(id)
                .map(|facts| facts.name.clone())
                .unwrap_or_else(|| format!("Quest {id}")),
            SignatureMember::Segment(label) if label.is_empty() => "Unnamed segment".to_string(),
            SignatureMember::Segment(label) => label.clone(),
        }
    };

    let reward_status = |agg: &SignatureAgg| -> String {
        if agg.reward_unverified {
            return "unverified".to_string();
        }
        match agg.reward_kinds.len() {
            0 => "none".to_string(),
            1 => match agg.reward_kinds.iter().next().map(String::as_str) {
                Some("included_in_loot") => "included_in_loot".to_string(),
                Some("fixed_liquid") => "fixed_liquid".to_string(),
                Some("item") => "item".to_string(),
                Some("skill") => "skill".to_string(),
                Some("mixed") => "mixed".to_string(),
                _ => "none".to_string(),
            },
            _ => "mixed".to_string(),
        }
    };
    let loot_rows = |items: &BTreeMap<String, (i64, f64)>| {
        let mut rows: Vec<HarvestLootItemRow> = items
            .iter()
            .map(|(name, (quantity, value))| HarvestLootItemRow {
                item_name: name.clone(),
                quantity: *quantity,
                value_ped: round2(*value),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.value_ped
                .partial_cmp(&a.value_ped)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.item_name.cmp(&b.item_name))
        });
        rows
    };

    let base_row = |kind: &str, label: String, agg: &SignatureAgg| {
        let status = reward_status(agg);
        HuntingSignatureRow {
            kind: kind.to_string(),
            label,
            runs: agg.runs.len() as i64,
            kills: agg.kills,
            duration_hours: round2(agg.duration_hours),
            cycled: round2(agg.cycled),
            returns: round2(agg.loot_tt),
            pes: round4(agg.pes),
            pes_per100_ped: if agg.cycled > 0.0 {
                round2((agg.pes / agg.cycled) * 100.0)
            } else {
                0.0
            },
            expected: agg.expected.finish(),
            confirmed_reward_ped: round2(agg.confirmed_reward_ped),
            realised_reward_markup: round2(agg.realised_reward_markup),
            realised_loot_markup: round2(agg.realised_loot_markup),
            reward_items: loot_rows(&agg.reward_items),
            reward_status: status,
            loot_items: loot_rows(&agg.loot_items),
            variants: Vec::new(),
        }
    };

    let mut families: std::collections::BTreeMap<i64, Vec<HuntingSignatureRow>> =
        std::collections::BTreeMap::new();
    let mut family_expected: std::collections::BTreeMap<i64, ExpectedHuntingAccumulator> =
        std::collections::BTreeMap::new();
    let mut rows: Vec<HuntingSignatureRow> = Vec::new();
    let mut ambient: Option<HuntingSignatureRow> = None;

    for (key, agg) in &signatures {
        match key.as_slice() {
            [] => {
                if agg.kills > 0 || agg.pes.abs() > 0.0 || agg.duration_hours > 0.0 {
                    let mut row = base_row("ambient", "Unscoped".to_string(), agg);
                    // A remainder is not a run of anything.
                    row.runs = 0;
                    ambient = Some(match ambient.take() {
                        Some(mut merged) => {
                            merged.kills += row.kills;
                            merged.cycled = round2(merged.cycled + row.cycled);
                            merged.returns = round2(merged.returns + row.returns);
                            merged.pes = round4(merged.pes + row.pes);
                            merged.duration_hours =
                                round2(merged.duration_hours + row.duration_hours);
                            merged
                        }
                        None => row,
                    });
                }
            }
            [SignatureMember::Quest(quest_id)] => {
                let facts = quest_facts.get(quest_id);
                let row = base_row("quest", member_label(&key[0]), agg);
                match facts.and_then(|facts| facts.family_id) {
                    Some(family_id) => {
                        family_expected
                            .entry(family_id)
                            .or_default()
                            .merge(&agg.expected);
                        families.entry(family_id).or_default().push(row);
                    }
                    None => rows.push(row),
                }
            }
            [SignatureMember::Segment(_)] => {
                rows.push(base_row("segment", member_label(&key[0]), agg));
            }
            _ => {
                let label = key.iter().map(member_label).collect::<Vec<_>>().join(" + ");
                rows.push(base_row("bundle", label, agg));
            }
        }
    }

    for (family_id, mut variants) in families {
        variants.sort_by(|a, b| {
            b.cycled
                .partial_cmp(&a.cycled)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.label.cmp(&b.label))
        });
        let label = family_names
            .get(&family_id)
            .cloned()
            .unwrap_or_else(|| format!("Family {family_id}"));
        // A single recorded variant still reports at family grain, because
        // the family is the repeatable slot the player decides on.
        let family_cycled: f64 = variants.iter().map(|v| v.cycled).sum();
        let family_pes: f64 = variants.iter().map(|v| v.pes).sum();
        let mut family_items: BTreeMap<String, (i64, f64)> = BTreeMap::new();
        let mut family_reward_items: BTreeMap<String, (i64, f64)> = BTreeMap::new();
        let mut family_statuses: BTreeSet<String> = BTreeSet::new();
        let mut family_unverified = false;
        for variant in &variants {
            for item in &variant.loot_items {
                let total = family_items
                    .entry(item.item_name.clone())
                    .or_insert((0, 0.0));
                total.0 += item.quantity;
                total.1 += item.value_ped;
            }
            for item in &variant.reward_items {
                let total = family_reward_items
                    .entry(item.item_name.clone())
                    .or_insert((0, 0.0));
                total.0 += item.quantity;
                total.1 += item.value_ped;
            }
            if variant.reward_status == "unverified" {
                family_unverified = true;
            } else if variant.reward_status != "none" {
                family_statuses.insert(variant.reward_status.clone());
            }
        }
        let family_reward_status = if family_unverified {
            "unverified".to_string()
        } else if family_statuses.is_empty() {
            "none".to_string()
        } else if family_statuses.len() == 1 {
            family_statuses.into_iter().next().unwrap_or_default()
        } else {
            "mixed".to_string()
        };
        let mut family_row = HuntingSignatureRow {
            kind: "quest_family".to_string(),
            label,
            runs: variants.iter().map(|v| v.runs).sum(),
            kills: variants.iter().map(|v| v.kills).sum(),
            duration_hours: round2(variants.iter().map(|v| v.duration_hours).sum()),
            cycled: round2(family_cycled),
            returns: round2(variants.iter().map(|v| v.returns).sum()),
            pes: round4(family_pes),
            pes_per100_ped: if family_cycled > 0.0 {
                round2((family_pes / family_cycled) * 100.0)
            } else {
                0.0
            },
            expected: family_expected
                .remove(&family_id)
                .and_then(|expected| expected.finish()),
            confirmed_reward_ped: round2(variants.iter().map(|v| v.confirmed_reward_ped).sum()),
            realised_reward_markup: round2(variants.iter().map(|v| v.realised_reward_markup).sum()),
            realised_loot_markup: round2(variants.iter().map(|v| v.realised_loot_markup).sum()),
            reward_items: loot_rows(&family_reward_items),
            reward_status: family_reward_status,
            loot_items: loot_rows(&family_items),
            variants: Vec::new(),
        };
        family_row.variants = variants;
        rows.push(family_row);
    }

    rows.sort_by(|a, b| {
        b.cycled
            .partial_cmp(&a.cycled)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.label.cmp(&b.label))
    });
    if let Some(ambient) = ambient {
        rows.push(ambient);
    }
    rows
}

// ── The Overview and Activity aggregates ──

impl AnalyticsService {
    /// The Overview aggregate for a named period (`30d` / `90d` / `1y`, or
    /// all-time for any other value).
    ///
    /// Scales O(days), not O(kills): the aggregates read the daily rollup
    /// projection for completed days and touch the raw tables only for the
    /// partial edge days (see [`overview_impl`]).
    pub async fn overview(&self, period: &str) -> Result<OverviewData, AnalyticsError> {
        let now = naive_to_epoch(self.clock.now());
        Ok(overview_impl(&self.db, now, period).await?)
    }

    /// The Hunting aggregate: the per-mob / per-tag comparison tables
    /// over the completed sessions.
    pub async fn hunting(&self) -> Result<HuntingData, AnalyticsError> {
        Ok(hunting_impl(&self.db).await?)
    }

    /// The Tree Cutting aggregate for a named period (`30d` / `90d` /
    /// `1y`, or all-time for any other value): effective yield tiers,
    /// each with its loot composition.
    pub async fn harvest(&self, period: &str) -> Result<HarvestData, AnalyticsError> {
        let now = naive_to_epoch(self.clock.now());
        Ok(harvest_impl(&self.db, period_epoch(period, now)).await?)
    }

    /// The revamped Hunting aggregate for a named period (`30d` / `90d` /
    /// `1y`, or all-time for any other value): the direct headline
    /// figures, the definition-keyed Sessions axis with activity
    /// signatures and instances, and the observed Targets axis with
    /// maturity drilldown and loot composition.
    pub async fn hunting_activity(
        &self,
        period: &str,
    ) -> Result<HuntingActivityData, AnalyticsError> {
        let started = std::time::Instant::now();
        // Settle any ended sessions still served raw (a no-op in steady
        // state), the same heal-before-read the Overview runs on the
        // daily rollups; the read itself stays correct either way.
        self.db.with_writer(crate::session_rollup::heal).await?;
        let now = naive_to_epoch(self.clock.now());
        let result = hunting_activity_impl(&self.db, period_epoch(period, now)).await?;
        tracing::debug!(
            target: "eo::performance",
            operation = "analytics.hunting_activity",
            elapsed_us = started.elapsed().as_micros() as u64,
            result_count = result.definitions.len() + result.species.len(),
            "analytical read complete"
        );
        Ok(result)
    }

    /// The whole-ledger summary for a named period (`30d` / `90d` / `1y`,
    /// or all-time for any other value): the per-tag markup and expense
    /// totals over EVERY ledger entry in the window, independent of the
    /// paginated list. Serves the Ledger tab's net-impact and source
    /// cards, through the same hybrid rollup + raw-edge split as the
    /// Overview (O(days), not O(entries)).
    pub async fn ledger_summary(&self, period: &str) -> Result<LedgerSummaryData, AnalyticsError> {
        let now = naive_to_epoch(self.clock.now());
        let watermark = self
            .db
            .with_writer(move |conn| daily_rollup::heal_rollups(conn, now))
            .await?;
        let epoch_start = period_epoch(period, now);
        let (gains, losses) = self
            .db
            .with_reader(move |conn| {
                Ok((
                    ledger_by_tag(conn, "markup", epoch_start, None, &watermark)?,
                    ledger_by_tag(conn, "expense", epoch_start, None, &watermark)?,
                ))
            })
            .await?;
        Ok(LedgerSummaryData { gains, losses })
    }
}

/// The whole-ledger per-tag summary: markup (gain) and expense (loss)
/// totals by tag over the requested window.
pub struct LedgerSummaryData {
    pub gains: std::collections::BTreeMap<String, f64>,
    pub losses: std::collections::BTreeMap<String, f64>,
}

// ── Ledger / presets / inventory writes (the CRUD surface) ──

const INVENTORY_SALE_TAG: &str = "inventory_sale";

/// The ledger and preset rows both select
/// (id, name-or-date, type, description, amount, tag); the amount reads
/// with float coercion (an INTEGER-affinity amount leaves as its float).
fn ledger_item(row: &rusqlite::Row) -> LedgerRow {
    LedgerRow {
        id: row.get_unwrap::<_, String>(0),
        date: row.get_unwrap::<_, String>(1),
        kind: row.get_unwrap::<_, String>(2),
        description: row.get_unwrap::<_, String>(3),
        amount: row.get_unwrap::<_, f64>(4),
        tag: row.get_unwrap::<_, String>(5),
    }
}

fn preset_item(row: &rusqlite::Row) -> PresetRow {
    PresetRow {
        id: row.get_unwrap::<_, String>(0),
        name: row.get_unwrap::<_, String>(1),
        kind: row.get_unwrap::<_, String>(2),
        description: row.get_unwrap::<_, String>(3),
        amount: row.get_unwrap::<_, f64>(4),
        tag: row.get_unwrap::<_, String>(5),
    }
}

/// The default ledger page size when the client names no `limit`.
const LEDGER_PAGE_DEFAULT: i64 = 50;
/// The largest ledger page a client may request; larger `limit` values clamp
/// here, bounding the work a single request can ask for.
const LEDGER_PAGE_MAX: i64 = 200;

/// The opaque keyset cursor over `[date, id]` of the last row on a page
/// (the shared [`crate::keyset`] codec).
fn encode_ledger_cursor(date: &str, id: &str) -> String {
    crate::keyset::encode_cursor(&[date, id])
}

/// Decode a keyset cursor back to its `(date, id)` seek key, or `None` for a
/// malformed token (which the handler answers as a 400).
fn decode_ledger_cursor(token: &str) -> Option<(String, String)> {
    let [date, id]: [String; 2] = crate::keyset::decode_cursor(token)?;
    Some((date, id))
}

/// The inventory row: (id, name, tt_value, markup_paid, notes, acquired_at).
fn inventory_item(row: &rusqlite::Row) -> InventoryRow {
    InventoryRow {
        id: row.get_unwrap::<_, String>(0),
        name: row.get_unwrap::<_, String>(1),
        tt_value: row.get_unwrap::<_, f64>(2),
        markup_paid: row.get_unwrap::<_, f64>(3),
        notes: row.get_unwrap::<_, Option<String>>(4),
        acquired_at: row.get_unwrap::<_, String>(5),
    }
}

/// Sub-hundredth tolerance for treating a stock or money residual as
/// closed. Positions and PED both round to hundredths at the display edge.
const STOCK_EPSILON: f64 = 1e-9;

fn is_universal_ammo(item_name: &str) -> bool {
    item_name.trim().eq_ignore_ascii_case("Universal Ammo")
}

/// The auction-listing column list, in the order [`listing_from_row`] reads.
const LISTING_COLUMNS: &str = "id, item_name, quantity, attributed_qty, unattributed_qty, \
     tt_value, attributed_tt, starting_bid, buyout, listing_fee, listed_at, status, \
     final_price, sale_fee, resolved_at, subject_kind, inventory_item_id, cost_basis, channel, \
     auction_days, listed_instant";

/// The moment a listing runs out: its duration measured from the instant it
/// started, not from the day. Posted at 18:20 for seven days, it ends at
/// 18:20 seven days later, which is what the auction house itself does.
///
/// `None` unless both the duration and the starting instant are known. A
/// listing recorded as a bare date has no time of day, and assuming one would
/// put the deadline hours away from the truth in either direction.
fn listing_expiry(listed_instant: Option<f64>, auction_days: Option<i64>) -> Option<String> {
    let (instant, days) = (listed_instant?, auction_days?);
    Some(to_iso_utc(instant + (days as f64) * 86_400.0))
}

/// Split a caller's listing moment into the calendar date the ledger is keyed
/// by and the precise instant the auction clock runs from.
///
/// A full timestamp gives both. A bare date gives only the date, so the clock
/// stays unknown rather than being anchored to an invented midnight. Nothing
/// at all means now, which is the ordinary case: a listing is normally
/// recorded as it is made.
fn listing_moment(supplied: Option<&str>, now: chrono::NaiveDateTime) -> (String, Option<f64>) {
    let Some(text) = supplied.map(str::trim).filter(|value| !value.is_empty()) else {
        let epoch = naive_to_epoch(now);
        return (epoch_to_iso(epoch), Some(epoch));
    };
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(stamp) = chrono::NaiveDateTime::parse_from_str(text, format) {
            return (
                stamp.format("%Y-%m-%d").to_string(),
                Some(naive_to_epoch(stamp)),
            );
        }
    }
    (text.to_string(), None)
}

/// One auction listing from a row over [`LISTING_COLUMNS`], with its
/// realised figures derived rather than read: a stored copy could drift from
/// the price and fees it was resolved with.
fn listing_from_row(row: &rusqlite::Row) -> AuctionListingRow {
    let tt_value = row.get_unwrap::<_, f64>(5);
    let attributed_tt = row.get_unwrap::<_, f64>(6);
    let listing_fee = row.get_unwrap::<_, f64>(9);
    let status = row.get_unwrap::<_, String>(11);
    let final_price = row.get_unwrap::<_, Option<f64>>(12);
    let sale_fee = row.get_unwrap::<_, Option<f64>>(13);
    let listed_at = row.get_unwrap::<_, String>(10);
    let subject_kind = row.get_unwrap::<_, String>(15);
    let cost_basis = row.get_unwrap::<_, Option<f64>>(17);
    let auction_days = row.get_unwrap::<_, Option<i64>>(19);
    let expires_at = listing_expiry(row.get_unwrap::<_, Option<f64>>(20), auction_days);
    let sold = status == "sold";

    let loot_outcome = (sold && subject_kind == "loot").then(|| {
        stock_allocation::resolve_sale(
            row.get_unwrap::<_, f64>(2),
            row.get_unwrap::<_, f64>(3),
            tt_value,
            final_price.unwrap_or(0.0),
            listing_fee,
            sale_fee.unwrap_or(0.0),
        )
    });

    AuctionListingRow {
        id: row.get_unwrap::<_, String>(0),
        item_name: row.get_unwrap::<_, String>(1),
        quantity: row.get_unwrap::<_, f64>(2),
        attributed_qty: row.get_unwrap::<_, f64>(3),
        unattributed_qty: row.get_unwrap::<_, f64>(4),
        tt_value,
        attributed_tt,
        starting_bid: row.get_unwrap::<_, f64>(7),
        buyout: row.get_unwrap::<_, Option<f64>>(8),
        listing_fee,
        listed_at,
        status,
        final_price,
        sale_fee,
        resolved_at: row.get_unwrap::<_, Option<String>>(14),
        subject_kind,
        inventory_item_id: row.get_unwrap::<_, Option<String>>(16),
        cost_basis,
        channel: row.get_unwrap::<_, String>(18),
        auction_days,
        expires_at,
        activity_net_markup: loot_outcome.map(|outcome| outcome.activity_net_markup),
        gross_markup: if sold {
            if let Some(basis) = cost_basis {
                Some(final_price.unwrap_or(0.0) - basis)
            } else {
                loot_outcome.map(|outcome| outcome.gross_markup)
            }
        } else {
            None
        },
    }
}

/// Read one listing by id, or `None` when it does not exist.
fn read_listing(
    conn: &rusqlite::Connection,
    listing_id: &str,
) -> rusqlite::Result<Option<AuctionListingRow>> {
    use rusqlite::OptionalExtension as _;
    conn.query_row(
        &format!(
            "SELECT {LISTING_COLUMNS} FROM auction_listings \
             WHERE id = ? AND undone_at IS NULL"
        ),
        rusqlite::params![listing_id],
        |row| Ok(listing_from_row(row)),
    )
    .optional()
}

/// Append one signed stock movement. Rows are never updated or deleted: a
/// correction is a new fact, so the original allocation stays auditable.
#[allow(clippy::too_many_arguments)]
fn insert_movement(
    conn: &rusqlite::Connection,
    item_name: &str,
    movement_kind: &str,
    ref_id: Option<&str>,
    provenance: Option<stock_allocation::StockProvenance<'_>>,
    session_definition_id: Option<i64>,
    activity_context_id: Option<i64>,
    tool_name: Option<&str>,
    quantity: f64,
    tt_value: f64,
    occurred_at: &str,
    created_at: f64,
) -> rusqlite::Result<()> {
    use stock_allocation::StockProvenance;
    // A movement with no provenance is stock whose origin is genuinely
    // unknown; it is consumed like any other but funds no activity.
    let source_kind = match provenance {
        None => "unattributed",
        Some(StockProvenance::Hunt(_)) => "hunt",
        Some(StockProvenance::Harvest(_)) => "harvest",
        Some(StockProvenance::Quest { .. }) => "quest",
    };
    let yield_tier = match provenance {
        Some(StockProvenance::Harvest(tier)) => Some(tier.as_str()),
        _ => None,
    };
    let mob_species = match provenance {
        Some(StockProvenance::Hunt(species)) => species,
        _ => None,
    };
    let (quest_reward_item_id, quest_run_id, quest_id, quest_activity_context_id) = match provenance
    {
        Some(StockProvenance::Quest {
            reward_item_id,
            quest_run_id,
            quest_id,
            activity_context_id,
        }) => (
            Some(reward_item_id),
            Some(quest_run_id),
            Some(quest_id),
            activity_context_id,
        ),
        _ => (None, None, None, None),
    };
    let activity_context_id = activity_context_id.or(quest_activity_context_id);
    conn.execute(
        "INSERT INTO stock_movements ( \
             item_name, movement_kind, ref_id, source_kind, source_event_id, \
             yield_tier, mob_species, session_definition_id, quantity, tt_value, occurred_at, \
             created_at, tool_name, quest_reward_item_id, quest_run_id, quest_id, \
             activity_context_id) \
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            item_name,
            movement_kind,
            ref_id,
            source_kind,
            yield_tier,
            mob_species,
            session_definition_id,
            quantity,
            tt_value,
            occurred_at,
            created_at,
            tool_name,
            quest_reward_item_id,
            quest_run_id,
            quest_id,
            activity_context_id,
        ],
    )?;
    Ok(())
}

/// Book the stock an outflow proves was held but the app never recorded.
///
/// An outflow may exceed every open position: the player held stock from
/// before tracking began, or from a source the app never saw. Writing only the
/// outflow would make the derived position negative, which no inventory can
/// be. The disposal is itself the evidence the units existed, so the
/// acquisition it implies is recorded alongside it, with no tier and no tool
/// because neither is known. It nets the position to zero rather than below,
/// and being untiered it can never fund an activity.
///
/// A no-op when tracked stock covered the whole outflow, which is the ordinary
/// case.
///
/// The row belongs to the outflow that revealed it, so it carries the same
/// `ref_id`: undoing that listing or conversion has to take this row with it,
/// or the stock it accounts for outlives the only evidence there ever was of
/// it.
fn record_opening_balance(
    conn: &rusqlite::Connection,
    item_name: &str,
    ref_id: &str,
    plan: &stock_allocation::AllocationPlan<'_>,
    occurred_at: &str,
    created_at: f64,
) -> rusqlite::Result<()> {
    if plan.excess_qty <= STOCK_EPSILON {
        return Ok(());
    }
    insert_movement(
        conn,
        item_name,
        "opening_balance",
        Some(ref_id),
        None,
        None,
        None,
        None,
        plan.excess_qty,
        plan.excess_tt,
        occurred_at,
        created_at,
    )
}

/// TT per unit for an item the app can produce but never sees drop, so it has
/// no recorded loot to derive a unit value from.
///
/// Entropia fixes TT per item, so these are constants of the game rather than
/// estimates. Without one, a conversion has nothing to divide its preserved TT
/// by and can only count the produced stock in PED, which reads as a quantity
/// while being a value.
fn produced_unit_tt(item_name: &str) -> Option<f64> {
    match item_name {
        "Nanocube" => Some(0.01),
        "Universal Ammo" => Some(0.0001),
        _ => None,
    }
}

struct RealisedStockOutcome {
    id: String,
    movement_kind: String,
    outcome: stock_allocation::SaleOutcome,
}

/// Every stock outcome that has crossed a recognition boundary. Auction
/// sales, private trades, and Shrapnel conversion differ operationally, but
/// their activity attribution is the same calculation over the immutable
/// source movements that funded them.
fn realised_stock_outcomes(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<RealisedStockOutcome>> {
    let mut stmt = conn.prepare(
        "SELECT id, 'listing', quantity, attributed_qty, tt_value, COALESCE(final_price, 0), \
                listing_fee, COALESCE(sale_fee, 0) \
         FROM auction_listings \
         WHERE status = 'sold' AND undone_at IS NULL AND subject_kind = 'loot' \
         UNION ALL \
         SELECT id, 'trade', quantity, attributed_qty, tt_value, final_price, 0, 0 \
         FROM private_sales WHERE undone_at IS NULL \
         UNION ALL \
         SELECT id, 'conversion_out', quantity, \
                CASE WHEN tt_value > 0 \
                     THEN quantity * COALESCE(attributed_tt, tt_value) / tt_value \
                     ELSE 0 END, \
                tt_value, COALESCE(output_tt_value, tt_value), 0, 0 \
         FROM stock_conversions \
         WHERE undone_at IS NULL AND gain_entry_id IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RealisedStockOutcome {
                id: row.get(0)?,
                movement_kind: row.get(1)?,
                outcome: stock_allocation::resolve_sale(
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ),
            })
        })?
        .collect();
    rows
}

/// Why a reversal cannot go ahead, in terms of the stock rather than the
/// tables. The reader owns an inventory, not a movement ledger.
fn blocked_reason(item_name: &str, short: f64) -> String {
    format!(
        "{item_name} this produced has since been sold or converted. \
         Undoing it would leave you holding {short:.2} less than nothing of it; \
         undo whatever used it first."
    )
}

/// Delete the ledger row a listing owns through `column`, if it is still
/// there, and report its day so the rollup can reland.
///
/// The ledger stays the system of record for money and the player may have
/// already removed the row by hand, so a missing entry is an ordinary outcome
/// and not an error.
fn delete_owned_ledger_entry(
    conn: &rusqlite::Connection,
    listing_id: &str,
    column: &str,
) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    let entry_id: Option<String> = conn
        .query_row(
            &format!("SELECT {column} FROM auction_listings WHERE id = ?"),
            rusqlite::params![listing_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(entry_id) = entry_id else {
        return Ok(None);
    };
    let day: Option<String> = conn
        .query_row(
            "SELECT date FROM ledger_entries WHERE id = ?",
            rusqlite::params![entry_id],
            |row| row.get(0),
        )
        .optional()?;
    if day.is_some() {
        conn.execute(
            "DELETE FROM ledger_entries WHERE id = ?",
            rusqlite::params![entry_id],
        )?;
    }
    Ok(day)
}

/// An item's whole position as the ledger states it: recorded loot plus every
/// signed movement, with nothing filtered.
///
/// [`item_positions`] drops closed buckets because allocation may only draw on
/// what is open. A safety check has to see the raw arithmetic instead, or a
/// reversal that would take a holding below zero looks fine right up until it
/// is committed.
fn raw_position(conn: &rusqlite::Connection, item_name: &str) -> rusqlite::Result<f64> {
    conn.query_row(
        "SELECT COALESCE(( \
             SELECT SUM(l.quantity) FROM harvest_loot_items AS l \
             WHERE l.item_name = ?1 AND l.deactivated_at IS NULL), 0) \
         + COALESCE(( \
             SELECT SUM(li.quantity) FROM kill_loot_items AS li \
             WHERE li.item_name = ?1 AND li.deactivated_at IS NULL), 0) \
         + COALESCE(( \
             SELECT SUM(ri.quantity) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             WHERE ri.item_name = ?1 AND ri.accounting_kind = 'stock' \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id) \
               AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = c.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            c.reward_outcome) = 'confirmed'), 0) \
         + COALESCE(( \
             SELECT SUM(m.quantity) FROM stock_movements AS m \
             WHERE m.item_name = ?1), 0)",
        rusqlite::params![item_name],
        |row| row.get(0),
    )
}

/// Whether dropping every movement belonging to `ref_id` would leave some item
/// holding less than nothing, and which one.
///
/// One check covers every kind of entry, because it asks the only question
/// that matters: after these rows go, does the arithmetic still describe an
/// inventory a player could have? An outflow being undone always returns
/// stock, so it passes; a conversion being undone unmakes what it produced,
/// and fails if those units have since been sold or converted onward.
fn reversal_blocker(
    conn: &rusqlite::Connection,
    ref_id: &str,
) -> rusqlite::Result<Option<(String, f64)>> {
    let deltas: Vec<(String, f64)> = {
        let mut stmt = conn.prepare(
            "SELECT item_name, SUM(quantity) FROM stock_movements \
             WHERE ref_id = ? GROUP BY item_name",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![ref_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (item_name, delta) in deltas {
        let remaining = raw_position(conn, &item_name)? - delta;
        if remaining < -STOCK_EPSILON {
            return Ok(Some((item_name, -remaining)));
        }
    }
    Ok(None)
}

/// One item's open positions per (provenance, tool), plus its TT per unit.
///
/// Recorded loot is the acquisition base, so nothing here duplicates it:
/// harvest loot arrives keyed by its event's yield tier and hunting loot by
/// its activity source plus optional observed species (the two namespaces
/// barely overlap, and an item that genuinely has both simply holds a joint
/// pile). The movement ledger only adds what has since left, returned, or
/// been produced by a conversion.
/// Unit TT comes from recorded loot where there is any, and otherwise from
/// what a conversion produced, which is the only other place an item's value
/// is known.
///
/// Enhancer-shrapnel loot rows are physically held shrapnel, so they count
/// in the position, but they are enhancer accounting rather than hunted
/// output. Unclassified non-shrapnel loot remains Hunting provenance: missing
/// target evidence may limit a target drilldown, never economic attribution.
///
/// The tool is part of the key so the allocation records which one produced
/// harvested stock, even though no surface reports on tools today. Keys come
/// back owned because the caller borrows them to build the allocation plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PositionKey {
    /// The activity source independent of its optional analytical facets.
    /// Empty means genuinely unattributed stock.
    source_kind: String,
    /// The stored yield-tier spelling; empty when the row carries none.
    tier: String,
    /// The stored species; empty when the row carries none.
    species: String,
    /// Hunting's user-designated context; absent for harvesting and
    /// genuinely unassigned or pre-context hunted stock.
    definition_id: Option<i64>,
    /// Hunting's exact declared activity context. This remains separate from
    /// the definition so realised stock outcomes can reach the sub-activity
    /// breakdown without making that attribution a target-species guess.
    activity_context_id: Option<i64>,
    /// The producing tool; empty when unknown.
    tool: String,
    /// Present only for confirmed quest-reward stock.
    quest: Option<QuestPositionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct QuestPositionKey {
    reward_item_id: i64,
    quest_run_id: i64,
    quest_id: i64,
    activity_context_id: Option<i64>,
}

/// Recover the originating activity from a movement. Current rows carry it
/// directly in `source_kind`; older conversion rows retain it through their
/// tier or species facet, so the normalised position key stays compatible
/// with every previously applied schema generation.
fn movement_activity_kind(source_kind: &str, tier: Option<&str>, species: Option<&str>) -> String {
    if tier.is_some_and(|value| !value.is_empty()) {
        "harvest".to_string()
    } else if source_kind == "hunt" || species.is_some_and(|value| !value.is_empty()) {
        "hunt".to_string()
    } else if source_kind == "quest" {
        "quest".to_string()
    } else {
        String::new()
    }
}

/// Movements recorded before definition provenance carry species but no
/// definition. If such an outflow exceeds genuinely unassigned stock, spread
/// only that legacy residual across the still-open definition buckets. The
/// species total stays exact while no historical definition claim is invented
/// for the realised sale itself.
fn absorb_legacy_hunt_outflows(by_source: &mut std::collections::BTreeMap<PositionKey, f64>) {
    fn absorb(
        by_source: &mut std::collections::BTreeMap<PositionKey, f64>,
        broad_key: &PositionKey,
        candidates: Vec<PositionKey>,
    ) {
        let broad = by_source.get(broad_key).copied().unwrap_or(0.0);
        if broad >= -STOCK_EPSILON {
            return;
        }
        let available: f64 = candidates.iter().filter_map(|key| by_source.get(key)).sum();
        if available <= STOCK_EPSILON {
            return;
        }
        let remaining = (available + broad).max(0.0);
        for key in candidates {
            if let Some(quantity) = by_source.get_mut(&key) {
                *quantity *= remaining / available;
            }
        }
        by_source.insert(broad_key.clone(), 0.0);
    }

    // Rows written before exact activity provenance can already carry a
    // session definition. Consume any residual at that broader grain across
    // the context-specific stock in the same definition.
    let definition_keys: Vec<PositionKey> = by_source
        .iter()
        .filter(|(key, quantity)| {
            key.source_kind == "hunt"
                && !key.species.is_empty()
                && key.definition_id.is_some()
                && key.activity_context_id.is_none()
                && **quantity < -STOCK_EPSILON
        })
        .map(|(key, _)| key.clone())
        .collect();
    for broad_key in definition_keys {
        let candidates = by_source
            .iter()
            .filter(|(key, quantity)| {
                key.source_kind == "hunt"
                    && key.species == broad_key.species
                    && key.definition_id == broad_key.definition_id
                    && key.activity_context_id.is_some()
                    && **quantity > STOCK_EPSILON
            })
            .map(|(key, _)| key.clone())
            .collect();
        absorb(by_source, &broad_key, candidates);
    }

    // Older rows may have neither definition nor context. Spread only their
    // residual across the more specific stock of the same species, retaining
    // the species total without inventing a historical activity claim.
    let unattributed_keys: Vec<PositionKey> = by_source
        .iter()
        .filter(|(key, quantity)| {
            key.source_kind == "hunt"
                && !key.species.is_empty()
                && key.definition_id.is_none()
                && key.activity_context_id.is_none()
                && **quantity < -STOCK_EPSILON
        })
        .map(|(key, _)| key.clone())
        .collect();
    for broad_key in unattributed_keys {
        let candidates = by_source
            .iter()
            .filter(|(key, quantity)| {
                key.source_kind == "hunt"
                    && key.species == broad_key.species
                    && (key.definition_id.is_some() || key.activity_context_id.is_some())
                    && **quantity > STOCK_EPSILON
            })
            .map(|(key, _)| key.clone())
            .collect();
        absorb(by_source, &broad_key, candidates);
    }
}

fn item_positions(
    conn: &rusqlite::Connection,
    item_name: &str,
) -> rusqlite::Result<(Vec<(PositionKey, f64)>, f64)> {
    // Keyed on the durable database spellings, with the empty string standing
    // for "not known", so the merge is ordered and total.
    let mut by_source: std::collections::BTreeMap<PositionKey, f64> =
        std::collections::BTreeMap::new();
    let mut base_qty = 0.0_f64;
    let mut base_tt = 0.0_f64;

    {
        let mut stmt = conn.prepare(
            "SELECT e.yield_tier, e.tool_name, SUM(l.quantity), SUM(l.value_ped) \
             FROM harvest_loot_items AS l \
             JOIN harvest_events AS e ON e.id = l.harvest_id \
             WHERE l.item_name = ? AND l.deactivated_at IS NULL \
             GROUP BY e.yield_tier, e.tool_name",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![item_name], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (tier, tool, quantity, tt_value) in rows {
            base_qty += quantity;
            base_tt += tt_value;
            *by_source
                .entry(PositionKey {
                    source_kind: "harvest".to_string(),
                    tier: tier.unwrap_or_default(),
                    species: String::new(),
                    definition_id: None,
                    activity_context_id: None,
                    tool: tool.unwrap_or_default(),
                    quest: None,
                })
                .or_insert(0.0) += quantity;
        }
    }

    {
        // Hunted loot: activity-keyed with optional species and no producing
        // tool (a kill's loot is not one tool's produce). Enhancer shrapnel
        // joins the physical pile as explicitly unattributed stock.
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN li.is_enhancer_shrapnel = 0 THEN COALESCE(k.mob_species, '') \
                    ELSE '' END AS species, \
                    li.is_enhancer_shrapnel, s.definition_id, k.context_id, \
                    SUM(li.quantity), SUM(li.value_ped) \
             FROM kill_loot_items AS li \
             JOIN kills AS k ON k.id = li.kill_id \
             JOIN tracking_sessions AS s ON s.id = k.session_id \
             WHERE li.item_name = ? AND li.deactivated_at IS NULL \
             GROUP BY species, li.is_enhancer_shrapnel, s.definition_id, k.context_id",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![item_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (species, is_shrapnel, definition_id, context_id, quantity, tt_value) in rows {
            base_qty += quantity;
            base_tt += tt_value;
            *by_source
                .entry(PositionKey {
                    source_kind: if is_shrapnel { "" } else { "hunt" }.to_string(),
                    tier: String::new(),
                    species,
                    definition_id,
                    activity_context_id: (!is_shrapnel).then_some(context_id).flatten(),
                    tool: String::new(),
                    quest: None,
                })
                .or_insert(0.0) += quantity;
        }
    }

    {
        let mut stmt = conn.prepare(
            "SELECT ri.id, c.quest_run_id, c.quest_id, a.activity_context_id, \
                    a.session_definition_id, COALESCE(a.weight, 1.0), \
                    ri.quantity * COALESCE(a.weight, 1.0), \
                    ri.value_ped * COALESCE(a.weight, 1.0) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             LEFT JOIN quest_reward_attributions a ON a.completion_id = c.id \
             WHERE ri.item_name = ? AND ri.accounting_kind = 'stock' \
               AND c.quest_run_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id) \
               AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = c.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            c.reward_outcome) = 'confirmed'",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![item_name], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, f64>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (
            reward_item_id,
            quest_run_id,
            quest_id,
            context_id,
            definition_id,
            quantity,
            tt_value,
        ) in rows
        {
            base_qty += quantity;
            base_tt += tt_value;
            *by_source
                .entry(PositionKey {
                    source_kind: "quest".to_string(),
                    tier: String::new(),
                    species: String::new(),
                    definition_id,
                    activity_context_id: None,
                    tool: String::new(),
                    quest: Some(QuestPositionKey {
                        reward_item_id,
                        quest_run_id,
                        quest_id,
                        activity_context_id: context_id,
                    }),
                })
                .or_insert(0.0) += quantity;
        }
    }

    let mut produced_qty = 0.0_f64;
    let mut produced_tt = 0.0_f64;
    {
        let mut stmt = conn.prepare(
            "SELECT source_kind, yield_tier, mob_species, session_definition_id, tool_name, \
                    quest_reward_item_id, quest_run_id, quest_id, activity_context_id, \
                    SUM(quantity), SUM(tt_value) \
             FROM stock_movements WHERE item_name = ? \
             GROUP BY source_kind, yield_tier, mob_species, session_definition_id, tool_name, \
                      quest_reward_item_id, quest_run_id, quest_id, activity_context_id",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![item_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, f64>(9)?,
                    row.get::<_, f64>(10)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (
            source_kind,
            tier,
            species,
            definition_id,
            tool,
            reward_item_id,
            run_id,
            quest_id,
            context_id,
            quantity,
            tt_value,
        ) in rows
        {
            if quantity > 0.0 {
                produced_qty += quantity;
                produced_tt += tt_value;
            }
            *by_source
                .entry(PositionKey {
                    source_kind: movement_activity_kind(
                        &source_kind,
                        tier.as_deref(),
                        species.as_deref(),
                    ),
                    tier: tier.unwrap_or_default(),
                    species: species.unwrap_or_default(),
                    definition_id,
                    activity_context_id: if source_kind == "hunt" {
                        context_id
                    } else {
                        None
                    },
                    tool: tool.unwrap_or_default(),
                    quest: match (reward_item_id, run_id, quest_id) {
                        (Some(reward_item_id), Some(quest_run_id), Some(quest_id)) => {
                            Some(QuestPositionKey {
                                reward_item_id,
                                quest_run_id,
                                quest_id,
                                activity_context_id: context_id,
                            })
                        }
                        _ => None,
                    },
                })
                .or_insert(0.0) += quantity;
        }
    }

    let unit_tt = if base_qty > STOCK_EPSILON {
        base_tt / base_qty
    } else if produced_qty > STOCK_EPSILON {
        produced_tt / produced_qty
    } else {
        0.0
    };

    absorb_legacy_hunt_outflows(&mut by_source);
    let positions = by_source
        .into_iter()
        .filter(|(_, quantity)| *quantity > STOCK_EPSILON)
        .collect();
    Ok((positions, unit_tt))
}

/// One item's open positions per (provenance, tool) key, with its unit TT.
type ItemPosition = (Vec<(PositionKey, f64)>, f64);

/// Every item's open positions and unit TT in three whole-table passes:
/// the batch sibling of [`item_positions`], byte-for-byte the same
/// arithmetic, for readers that need the whole inventory at once. The
/// per-item shape stays for the write paths, which touch one item inside
/// a transaction; a list surface calling it in a loop would re-scan the
/// loot tables once per item, which is exactly the O(items x rows) read
/// this batch form exists to avoid.
fn all_item_positions(
    conn: &rusqlite::Connection,
    hunting_loot: &[crate::session_rollup::EffectiveHuntingLootCell],
) -> rusqlite::Result<std::collections::HashMap<String, ItemPosition>> {
    use std::collections::{BTreeMap, HashMap};
    #[derive(Default)]
    struct ItemAcc {
        by_source: BTreeMap<PositionKey, f64>,
        base_qty: f64,
        base_tt: f64,
        produced_qty: f64,
        produced_tt: f64,
    }
    let mut items: HashMap<String, ItemAcc> = HashMap::new();

    {
        let mut stmt = conn.prepare(
            "SELECT l.item_name, e.yield_tier, e.tool_name, SUM(l.quantity), SUM(l.value_ped) \
             FROM harvest_loot_items AS l \
             JOIN harvest_events AS e ON e.id = l.harvest_id \
             WHERE l.deactivated_at IS NULL \
             GROUP BY l.item_name, e.yield_tier, e.tool_name",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let item: String = row.get(0)?;
            let tier: Option<String> = row.get(1)?;
            let tool: Option<String> = row.get(2)?;
            let quantity: f64 = row.get(3)?;
            let tt_value: f64 = row.get(4)?;
            let acc = items.entry(item).or_default();
            acc.base_qty += quantity;
            acc.base_tt += tt_value;
            *acc.by_source
                .entry(PositionKey {
                    source_kind: "harvest".to_string(),
                    tier: tier.unwrap_or_default(),
                    species: String::new(),
                    definition_id: None,
                    activity_context_id: None,
                    tool: tool.unwrap_or_default(),
                    quest: None,
                })
                .or_insert(0.0) += quantity;
        }
    }

    {
        // Hunted loot arrives through the single effective-cell boundary:
        // settled sessions are projected and only explicitly unsettled
        // sessions can contribute scoped raw cells.
        let mut fold = |item: String,
                        species: String,
                        is_shrapnel: bool,
                        definition_id: Option<i64>,
                        quantity: f64,
                        tt_value: f64| {
            let acc = items.entry(item).or_default();
            acc.base_qty += quantity;
            acc.base_tt += tt_value;
            *acc.by_source
                .entry(PositionKey {
                    source_kind: if is_shrapnel { "" } else { "hunt" }.to_string(),
                    tier: String::new(),
                    species,
                    definition_id,
                    activity_context_id: None,
                    tool: String::new(),
                    quest: None,
                })
                .or_insert(0.0) += quantity;
        };
        for cell in hunting_loot {
            fold(
                cell.item_name.clone(),
                cell.mob_species.clone(),
                cell.is_enhancer_shrapnel,
                cell.definition_id,
                cell.quantity as f64,
                cell.value_ped,
            );
        }
    }

    {
        let mut stmt = conn.prepare(
            "SELECT ri.item_name, ri.id, c.quest_run_id, c.quest_id, \
                    a.activity_context_id, a.session_definition_id, \
                    ri.quantity * COALESCE(a.weight, 1.0), \
                    ri.value_ped * COALESCE(a.weight, 1.0) \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             LEFT JOIN quest_reward_attributions a ON a.completion_id = c.id \
             WHERE ri.accounting_kind = 'stock' AND c.quest_run_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id) \
               AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                             WHERE r.completion_id = c.id \
                             ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                            c.reward_outcome) = 'confirmed'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let item: String = row.get(0)?;
            let reward_item_id: i64 = row.get(1)?;
            let quest_run_id: i64 = row.get(2)?;
            let quest_id: i64 = row.get(3)?;
            let context_id: Option<i64> = row.get(4)?;
            let definition_id: Option<i64> = row.get(5)?;
            let quantity: f64 = row.get(6)?;
            let tt_value: f64 = row.get(7)?;
            let acc = items.entry(item).or_default();
            acc.base_qty += quantity;
            acc.base_tt += tt_value;
            *acc.by_source
                .entry(PositionKey {
                    source_kind: "quest".to_string(),
                    tier: String::new(),
                    species: String::new(),
                    definition_id,
                    activity_context_id: None,
                    tool: String::new(),
                    quest: Some(QuestPositionKey {
                        reward_item_id,
                        quest_run_id,
                        quest_id,
                        activity_context_id: context_id,
                    }),
                })
                .or_insert(0.0) += quantity;
        }
    }

    {
        let mut stmt = conn.prepare(
            "SELECT item_name, source_kind, yield_tier, mob_species, session_definition_id, tool_name, \
                    quest_reward_item_id, quest_run_id, quest_id, activity_context_id, \
                    SUM(quantity), SUM(tt_value) \
             FROM stock_movements \
             GROUP BY item_name, source_kind, yield_tier, mob_species, session_definition_id, tool_name, \
                      quest_reward_item_id, quest_run_id, quest_id, activity_context_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let item: String = row.get(0)?;
            let source_kind: String = row.get(1)?;
            let tier: Option<String> = row.get(2)?;
            let species: Option<String> = row.get(3)?;
            let definition_id: Option<i64> = row.get(4)?;
            let tool: Option<String> = row.get(5)?;
            let reward_item_id: Option<i64> = row.get(6)?;
            let run_id: Option<i64> = row.get(7)?;
            let quest_id: Option<i64> = row.get(8)?;
            let context_id: Option<i64> = row.get(9)?;
            let quantity: f64 = row.get(10)?;
            let tt_value: f64 = row.get(11)?;
            let acc = items.entry(item).or_default();
            if quantity > 0.0 {
                acc.produced_qty += quantity;
                acc.produced_tt += tt_value;
            }
            *acc.by_source
                .entry(PositionKey {
                    source_kind: movement_activity_kind(
                        &source_kind,
                        tier.as_deref(),
                        species.as_deref(),
                    ),
                    tier: tier.unwrap_or_default(),
                    species: species.unwrap_or_default(),
                    definition_id,
                    // The batch inventory read uses session-grain projected
                    // loot, so collapse hunted movement contexts back to that
                    // same grain. Write paths use `item_positions`, which
                    // retains the exact context for future attribution.
                    activity_context_id: None,
                    tool: tool.unwrap_or_default(),
                    quest: match (reward_item_id, run_id, quest_id) {
                        (Some(reward_item_id), Some(quest_run_id), Some(quest_id)) => {
                            Some(QuestPositionKey {
                                reward_item_id,
                                quest_run_id,
                                quest_id,
                                activity_context_id: context_id,
                            })
                        }
                        _ => None,
                    },
                })
                .or_insert(0.0) += quantity;
        }
    }

    Ok(items
        .into_iter()
        .map(|(item, acc)| {
            let unit_tt = if acc.base_qty > STOCK_EPSILON {
                acc.base_tt / acc.base_qty
            } else if acc.produced_qty > STOCK_EPSILON {
                acc.produced_tt / acc.produced_qty
            } else {
                0.0
            };
            let mut by_source = acc.by_source;
            absorb_legacy_hunt_outflows(&mut by_source);
            let positions: Vec<(PositionKey, f64)> = by_source
                .into_iter()
                .filter(|(_, quantity)| *quantity > STOCK_EPSILON)
                .collect();
            (item, (positions, unit_tt))
        })
        .collect())
}

/// Borrow a position list into the allocation module's shape. Activity
/// provenance is independent of the optional target facet: an unclassified
/// hunt remains Hunting, while a key with no source is an unattributed pile.
fn as_source_positions(
    positions: &[(PositionKey, f64)],
) -> Vec<stock_allocation::SourcePosition<'_>> {
    positions
        .iter()
        .map(|(key, quantity)| stock_allocation::SourcePosition {
            provenance: if key.source_kind == "harvest" {
                Some(stock_allocation::StockProvenance::Harvest(
                    HarvestYieldTier::from_db(&key.tier),
                ))
            } else if key.source_kind == "hunt" {
                Some(stock_allocation::StockProvenance::Hunt(
                    (!key.species.is_empty()).then_some(key.species.as_str()),
                ))
            } else {
                key.quest
                    .as_ref()
                    .map(|quest| stock_allocation::StockProvenance::Quest {
                        reward_item_id: quest.reward_item_id,
                        quest_run_id: quest.quest_run_id,
                        quest_id: quest.quest_id,
                        activity_context_id: quest.activity_context_id,
                    })
            },
            // Keep the session-definition key on every Hunting position;
            // optional target evidence never gates the deliberate axis.
            session_definition_id: key.definition_id,
            activity_context_id: key.activity_context_id,
            tool_name: (!key.tool.is_empty()).then_some(key.tool.as_str()),
            quantity: *quantity,
        })
        .collect()
}

impl AnalyticsService {
    /// The clock's instant as a UTC YYYY-MM-DD date.
    fn default_date(&self) -> String {
        epoch_to_iso(naive_to_epoch(self.clock.now()))
    }

    /// Keyset (seek) pagination over the ledger, newest first. Returns a
    /// [`LedgerPage`]: the page of entries (each an `id, date, type,
    /// description, amount, tag` object) plus the opaque cursor for the
    /// following page (`None` on the last page), so the list no longer
    /// reads the whole table on every request. Without a cursor the first
    /// page is served; `limit` bounds the page (default
    /// [`LEDGER_PAGE_DEFAULT`], capped at [`LEDGER_PAGE_MAX`]). A malformed
    /// cursor is [`AnalyticsError::InvalidCursor`].
    pub async fn list_ledger(
        &self,
        cursor: Option<&str>,
        limit: Option<i64>,
    ) -> Result<LedgerPage, AnalyticsError> {
        let page = limit
            .unwrap_or(LEDGER_PAGE_DEFAULT)
            .clamp(1, LEDGER_PAGE_MAX);
        let seek = match cursor {
            None => None,
            Some(token) => match decode_ledger_cursor(token) {
                Some(key) => Some(key),
                None => return Err(AnalyticsError::InvalidCursor),
            },
        };

        // The seek predicate reproduces the (date DESC, id DESC) order past
        // the cursor row; one extra row is fetched to detect a further page.
        // id-order: tiebreak (the seek is by date; id splits equal dates).
        let mut sql =
            String::from("SELECT id, date, type, description, amount, tag FROM ledger_entries");
        if seek.is_some() {
            sql.push_str(" WHERE date < ? OR (date = ? AND id < ?)");
        }
        sql.push_str(" ORDER BY date DESC, id DESC LIMIT ?");

        // Each fetched row as (date, id, wire shape) plus the whole-table
        // count; the cursor is cut from the last kept row's (date, id).
        let (rows, total): (Vec<(String, String, LedgerRow)>, i64) = self
            .db
            .with_reader(move |conn| {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM ledger_entries", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(&sql)?;
                let map_row =
                    |row: &rusqlite::Row| -> rusqlite::Result<(String, String, LedgerRow)> {
                        Ok((
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(0)?,
                            ledger_item(row),
                        ))
                    };
                let rows = match &seek {
                    Some((date, id)) => stmt
                        .query_map(rusqlite::params![date, date, id, page + 1], map_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                    None => stmt
                        .query_map(rusqlite::params![page + 1], map_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                };
                Ok((rows, total))
            })
            .await?;

        // A full extra row means another page follows: drop it and cut the
        // next cursor from the last row actually served.
        let has_more = rows.len() as i64 > page;
        let kept = if has_more {
            &rows[..page as usize]
        } else {
            &rows[..]
        };
        let entries: Vec<LedgerRow> = kept.iter().map(|(_, _, item)| item.clone()).collect();
        let next_cursor = has_more
            .then(|| kept.last())
            .flatten()
            .map(|(date, id, _)| encode_ledger_cursor(date, id));
        Ok(LedgerPage {
            entries,
            next_cursor,
            total,
        })
    }

    /// Create a ledger entry, relanding its (possibly backdated) day's
    /// rollup in the same transaction. Returns the created entry's wire
    /// shape (the input echoed with its generated id).
    pub async fn create_ledger_entry(
        &self,
        date: &str,
        kind: &str,
        description: &str,
        amount: f64,
        tag: &str,
    ) -> Result<LedgerRow, AnalyticsError> {
        let id = Uuid::new_v4().to_string();
        // One transaction over the insert and the rollup refresh: a
        // backdated entry relands its day's rollup with the write.
        let (id_c, date_c, kind_c, desc_c, tag_c) = (
            id.clone(),
            date.to_string(),
            kind.to_string(),
            description.to_string(),
            tag.to_string(),
        );
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![id_c, date_c, kind_c, desc_c, amount, tag_c],
                )?;
                daily_rollup::refresh_days(&tx, [date_c.as_str()])?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(LedgerRow {
            id,
            date: date.to_string(),
            kind: kind.to_string(),
            description: description.to_string(),
            amount,
            tag: tag.to_string(),
        })
    }

    /// Delete a ledger entry, relanding its day's rollup in the same
    /// transaction. Returns whether a row existed (`false` = not found).
    pub async fn delete_ledger_entry(&self, entry_id: &str) -> Result<bool, AnalyticsError> {
        // Capture the entry's day before deleting so its rollup relands
        // in the same transaction; a vanished entry reports not-found.
        let entry_id = entry_id.to_string();
        let existed = self
            .db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let date: Option<String> = tx
                    .query_row(
                        "SELECT date FROM ledger_entries WHERE id = ?",
                        rusqlite::params![entry_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(date) = date else {
                    return Ok(false);
                };
                tx.execute(
                    "DELETE FROM ledger_entries WHERE id = ?",
                    rusqlite::params![entry_id],
                )?;
                daily_rollup::refresh_days(&tx, [date])?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        Ok(existed)
    }

    /// The ledger presets, in creation order.
    pub async fn list_ledger_presets(&self) -> Result<Vec<PresetRow>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, type, description, amount, tag FROM ledger_presets \
                     ORDER BY created_at ASC, id ASC",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok(preset_item(row)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await?)
    }

    /// Create a ledger preset. The type is a closed vocabulary
    /// (`expense` / `markup`); anything else is
    /// [`AnalyticsError::InvalidPresetType`].
    pub async fn create_ledger_preset(
        &self,
        name: &str,
        kind: &str,
        description: &str,
        amount: f64,
        tag: &str,
    ) -> Result<PresetRow, AnalyticsError> {
        if kind != "expense" && kind != "markup" {
            return Err(AnalyticsError::InvalidPresetType);
        }
        let id = Uuid::new_v4().to_string();
        {
            let (id, name, kind, description, tag) = (
                id.clone(),
                name.to_string(),
                kind.to_string(),
                description.to_string(),
                tag.to_string(),
            );
            self.db
                .with_writer(move |conn| {
                    conn.execute(
                        "INSERT INTO ledger_presets (id, name, type, description, amount, tag) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        rusqlite::params![id, name, kind, description, amount, tag],
                    )?;
                    Ok(())
                })
                .await?;
        }
        Ok(PresetRow {
            id,
            name: name.to_string(),
            kind: kind.to_string(),
            description: description.to_string(),
            amount,
            tag: tag.to_string(),
        })
    }

    /// Delete a ledger preset. Returns whether a row existed.
    pub async fn delete_ledger_preset(&self, preset_id: &str) -> Result<bool, AnalyticsError> {
        let preset_id = preset_id.to_string();
        let affected = self
            .db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    "DELETE FROM ledger_presets WHERE id = ?",
                    rusqlite::params![preset_id],
                )?)
            })
            .await?;
        Ok(affected != 0)
    }

    /// The inventory items, newest acquisition first.
    pub async fn list_inventory(&self) -> Result<Vec<InventoryRow>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, tt_value, markup_paid, notes, acquired_at \
                     FROM inventory_items WHERE state = 'held' \
                     ORDER BY acquired_at DESC, id DESC",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok(inventory_item(row)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await?)
    }

    /// Resolve an observed item name against the holdings that can currently
    /// be transacted. This is the intake-neutral seam shared by manual entry
    /// and a future market-window OCR adapter. Exact normalised matches lead;
    /// fuzzy candidates remain proposals for the review surface.
    pub async fn resolve_inventory_name(
        &self,
        observed_name: &str,
    ) -> Result<Vec<InventoryMatchRow>, AnalyticsError> {
        let query = observed_name.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut candidates: Vec<InventoryMatchRow> = self
            .stock_positions(Profession::Inventory)
            .await?
            .into_iter()
            .filter(|row| row.quantity > STOCK_EPSILON)
            .map(|row| InventoryMatchRow {
                kind: "loot".to_string(),
                holding_id: row.item_name.clone(),
                score: crate::fuzzy_match::wratio(query, &row.item_name),
                name: row.item_name,
            })
            .collect();
        candidates.extend(
            self.list_inventory()
                .await?
                .into_iter()
                .map(|row| InventoryMatchRow {
                    kind: "equipment".to_string(),
                    holding_id: row.id,
                    score: crate::fuzzy_match::wratio(query, &row.name),
                    name: row.name,
                }),
        );
        let normalised = |value: &str| {
            value
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let query_key = normalised(query);
        for candidate in &mut candidates {
            if normalised(&candidate.name) == query_key {
                candidate.score = 100.0;
            }
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.holding_id.cmp(&right.holding_id))
        });
        candidates.truncate(5);
        Ok(candidates)
    }

    /// Every canonical item the player has held, most valuable first.
    ///
    /// Recorded loot is the acquisition base and the movement ledger says what
    /// has since left or returned. An item whose position has closed stays on
    /// the list at zero: it is stock the player produces and will hold again,
    /// and a line that vanishes on the last sale reads as an item that was
    /// never there rather than one that is currently empty.
    ///
    /// TT is the held quantity at the item's unit TT rather than a separate
    /// sum over loot and movements. Entropia fixes TT per item, so the two
    /// agree by definition, and deriving both figures from one place is what
    /// stops the pair drifting apart.
    pub async fn stock_positions(
        &self,
        profession: Profession,
    ) -> Result<Vec<StockPositionRow>, AnalyticsError> {
        let started = std::time::Instant::now();
        // The hunted arm of the position arithmetic folds settled
        // sessions' loot cells; settle any backlog first (steady-state
        // no-op, and the read is correct either way).
        self.db.with_writer(crate::session_rollup::heal).await?;
        let rows = self
            .db
            .with_reader(move |conn| {
                // The item universe each activity's stock panel lists: what
                // its own recorded loot produced, plus every item its own
                // listings and conversions have touched (which pulls in a
                // conversion target like Nanocube). The POSITION of an item
                // stays whole-inventory arithmetic either way; only which
                // tab lists it is scoped, so a jointly produced pile shows
                // the same, true figures on both tabs. Legacy movements with
                // no owning record are harvest-era by construction.
                let hunting_loot = crate::session_rollup::effective_hunting_loot_cells(conn, None)?;
                let mut items = std::collections::BTreeSet::new();
                if matches!(profession, Profession::Harvesting | Profession::Inventory) {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT item_name FROM harvest_loot_items \
                         WHERE deactivated_at IS NULL",
                    )?;
                    let names = stmt
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    items.extend(names);
                }
                if matches!(profession, Profession::Hunting | Profession::Inventory) {
                    items.extend(hunting_loot.iter().map(|cell| cell.item_name.clone()));
                }
                if matches!(profession, Profession::Inventory) {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT ri.item_name \
                         FROM session_quest_completion_reward_items ri \
                         JOIN session_quest_completions c ON c.id = ri.completion_id \
                         WHERE ri.accounting_kind = 'stock' \
                           AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                                           WHERE rr.completion_id = c.id) \
                           AND COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                                         WHERE r.completion_id = c.id \
                                         ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                                        c.reward_outcome) = 'confirmed'",
                    )?;
                    let names = stmt
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    items.extend(names);
                }
                let movement_scope = match profession {
                    Profession::Harvesting => {
                        "WHERE EXISTS (SELECT 1 FROM auction_listings al \
                                      WHERE al.id = m.ref_id AND al.profession = ?1) \
                            OR EXISTS (SELECT 1 FROM stock_conversions sc \
                                      WHERE sc.id = m.ref_id AND sc.profession = ?1) \
                            OR m.ref_id IS NULL"
                    }
                    Profession::Hunting => {
                        "WHERE EXISTS (SELECT 1 FROM auction_listings al \
                                      WHERE al.id = m.ref_id AND al.profession = ?1) \
                            OR EXISTS (SELECT 1 FROM stock_conversions sc \
                                      WHERE sc.id = m.ref_id AND sc.profession = ?1)"
                    }
                    Profession::Inventory => "WHERE ?1 = 'inventory'",
                };
                let sql =
                    format!("SELECT DISTINCT m.item_name FROM stock_movements m {movement_scope}");
                let mut stmt = conn.prepare(&sql)?;
                let movement_names = stmt
                    .query_map(rusqlite::params![profession.as_str()], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                items.extend(movement_names);

                // The whole inventory in three passes, then per-item lookups:
                // a per-item read here would re-scan the loot tables once per
                // item, which is the O(items x rows) shape this list used to
                // take its load time from.
                let all_positions = all_item_positions(conn, &hunting_loot)?;
                let listed_by_item: std::collections::HashMap<String, f64> = {
                    let mut stmt = conn.prepare(
                        "SELECT item_name, COALESCE(SUM(quantity), 0) FROM auction_listings \
                         WHERE status = 'pending' AND undone_at IS NULL \
                           AND subject_kind = 'loot' GROUP BY item_name",
                    )?;
                    let mut out = std::collections::HashMap::new();
                    let mut listed = stmt.query([])?;
                    while let Some(row) = listed.next()? {
                        out.insert(row.get::<_, String>(0)?, row.get::<_, f64>(1)?);
                    }
                    out
                };
                let mut rows = Vec::new();
                for item_name in items {
                    if is_universal_ammo(&item_name) {
                        continue;
                    }
                    // A universe item with no open position stays listed at
                    // zero, exactly as the per-item read reported it.
                    let (quantity, unit_tt) = all_positions
                        .get(&item_name)
                        .map(|(positions, unit_tt)| {
                            let quantity: f64 = positions
                                .iter()
                                .map(|(_, quantity)| quantity)
                                .sum::<f64>()
                                .max(0.0);
                            (quantity, *unit_tt)
                        })
                        .unwrap_or((0.0, 0.0));
                    let tt_value = quantity * unit_tt;
                    let listed_quantity = listed_by_item.get(&item_name).copied().unwrap_or(0.0);
                    rows.push(StockPositionRow {
                        item_name,
                        quantity,
                        tt_value,
                        listed_quantity,
                    });
                }
                rows.sort_by(|a, b| {
                    b.tt_value
                        .partial_cmp(&a.tt_value)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.item_name.cmp(&b.item_name))
                });
                Ok(rows)
            })
            .await?;
        tracing::debug!(
            target: "eo::performance",
            operation = "analytics.stock_positions",
            elapsed_us = started.elapsed().as_micros() as u64,
            result_count = rows.len(),
            "analytical read complete"
        );
        Ok(rows)
    }

    /// One activity's auction listings, unresolved first and newest within
    /// each group. A listing belongs to the activity it was created from;
    /// a joint-provenance sale still credits every contributing activity's
    /// realised figures, whichever tab it is listed on.
    pub async fn auction_listings(
        &self,
        profession: Profession,
    ) -> Result<Vec<AuctionListingRow>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {LISTING_COLUMNS} FROM auction_listings \
                     WHERE undone_at IS NULL AND profession = ? \
                     ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END, \
                              listed_at DESC, id DESC"
                ))?;
                let rows = stmt
                    .query_map(rusqlite::params![profession.as_str()], |row| {
                        Ok(listing_from_row(row))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await?)
    }

    /// List stock on the auction.
    ///
    /// The listed quantity leaves holdings immediately, because in game it
    /// has left the player's inventory the moment the listing exists. The
    /// starting-bid fee is spent immediately too, and is written to the
    /// ledger dated to the listing: it is gone whether or not the item ever
    /// sells. No markup is realised here. That waits for confirmation.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_auction_listing(
        &self,
        profession: Profession,
        item_name: &str,
        quantity: f64,
        starting_bid: f64,
        buyout: Option<f64>,
        listing_fee: f64,
        listed_at: Option<&str>,
        auction_days: Option<i64>,
    ) -> Result<AuctionListingRow, AnalyticsError> {
        if is_universal_ammo(item_name) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and cannot be listed",
            ));
        }
        if quantity <= 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a listing needs a positive quantity",
            ));
        }
        if starting_bid < 0.0 || listing_fee < 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "auction prices and fees cannot be negative",
            ));
        }
        if auction_days.is_some_and(|days| days <= 0) {
            return Err(AnalyticsError::InvalidInput(
                "a listing duration must be a positive number of days",
            ));
        }

        let id = Uuid::new_v4().to_string();
        let (listed_at, listed_instant) = listing_moment(listed_at, self.clock.now());
        let now = naive_to_epoch(self.clock.now());
        let (id_c, item_c, listed_c) = (id.clone(), item_name.to_string(), listed_at.clone());
        let instant_c = listed_instant;

        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let (positions, unit_tt) = item_positions(&tx, &item_c)?;
                let plan =
                    stock_allocation::allocate(&as_source_positions(&positions), quantity, unit_tt);

                tx.execute(
                    "INSERT INTO auction_listings ( \
                         id, item_name, profession, quantity, attributed_qty, unattributed_qty, \
                         tt_value, attributed_tt, starting_bid, buyout, listing_fee, listed_at, \
                         auction_days, listed_instant, status, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
                    rusqlite::params![
                        id_c,
                        item_c,
                        profession.as_str(),
                        quantity,
                        plan.attributed_qty,
                        plan.unattributed_qty,
                        quantity * unit_tt,
                        plan.attributed_tt,
                        starting_bid,
                        buyout,
                        listing_fee,
                        listed_c,
                        auction_days,
                        instant_c,
                        now,
                        now,
                    ],
                )?;

                record_opening_balance(&tx, &item_c, &id_c, &plan, &listed_c, now)?;

                for allocation in &plan.allocations {
                    insert_movement(
                        &tx,
                        &item_c,
                        "listing",
                        Some(&id_c),
                        allocation.provenance,
                        allocation.session_definition_id,
                        allocation.activity_context_id,
                        allocation.tool_name,
                        -allocation.quantity,
                        -allocation.tt_value,
                        &listed_c,
                        now,
                    )?;
                }

                if listing_fee > 0.0 {
                    let entry_id = Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, 'expense', ?, ?, 'market')",
                        rusqlite::params![
                            entry_id,
                            listed_c,
                            format!("Auction Fee: {item_c}"),
                            listing_fee,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE auction_listings SET fee_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, id_c],
                    )?;
                    daily_rollup::refresh_days(&tx, [listed_c.as_str()])?;
                }

                let row = read_listing(&tx, &id_c)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// List one whole capital-equipment holding on the auction.
    ///
    /// Equipment is indivisible in this first model: the stable inventory row
    /// is the position and its TT plus paid markup is the acquisition basis.
    /// The row moves to `listed` rather than being deleted, so expiry, sale
    /// reversal, history, and undo all retain the original fact.
    pub async fn create_equipment_listing(
        &self,
        item_id: &str,
        starting_bid: f64,
        buyout: Option<f64>,
        listing_fee: f64,
        listed_at: Option<&str>,
        auction_days: Option<i64>,
    ) -> Result<Option<AuctionListingRow>, AnalyticsError> {
        if starting_bid < 0.0 || listing_fee < 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "auction prices and fees cannot be negative",
            ));
        }
        if auction_days.is_some_and(|days| days <= 0) {
            return Err(AnalyticsError::InvalidInput(
                "a listing duration must be a positive number of days",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let item_id = item_id.to_string();
        let (listed_at, listed_instant) = listing_moment(listed_at, self.clock.now());
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let item: Option<(String, f64, f64)> = tx
                    .query_row(
                        "SELECT name, tt_value, markup_paid FROM inventory_items \
                         WHERE id = ? AND state = 'held'",
                        rusqlite::params![item_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let Some((name, tt_value, markup_paid)) = item else {
                    return Ok(None);
                };
                let cost_basis = tt_value + markup_paid;
                tx.execute(
                    "INSERT INTO auction_listings ( \
                         id, item_name, profession, quantity, attributed_qty, unattributed_qty, \
                         tt_value, attributed_tt, starting_bid, buyout, listing_fee, listed_at, \
                         auction_days, listed_instant, status, created_at, updated_at, \
                         subject_kind, inventory_item_id, cost_basis, channel) \
                     VALUES (?, ?, 'inventory', 1, 0, 1, ?, 0, ?, ?, ?, ?, ?, ?, \
                             'pending', ?, ?, 'equipment', ?, ?, 'auction')",
                    rusqlite::params![
                        id,
                        name,
                        tt_value,
                        starting_bid,
                        buyout,
                        listing_fee,
                        listed_at,
                        auction_days,
                        listed_instant,
                        now,
                        now,
                        item_id,
                        cost_basis,
                    ],
                )?;
                tx.execute(
                    "UPDATE inventory_items SET state = 'listed', updated_at = ? WHERE id = ?",
                    rusqlite::params![now, item_id],
                )?;
                if listing_fee > 0.0 {
                    let entry_id = Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, 'expense', ?, ?, 'market')",
                        rusqlite::params![
                            entry_id,
                            listed_at,
                            format!("Auction Fee: {name}"),
                            listing_fee,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE auction_listings SET fee_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, id],
                    )?;
                    daily_rollup::refresh_days(&tx, [listed_at.as_str()])?;
                }
                let row = read_listing(&tx, &id)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Record an immediate fee-free player trade for one whole equipment
    /// holding. It uses the listing record as the canonical market lifecycle,
    /// resolved in the same transaction, with `channel = trade` distinguishing
    /// it from an auction that happened to sell immediately.
    pub async fn trade_equipment(
        &self,
        item_id: &str,
        final_price: f64,
        sold_at: Option<&str>,
    ) -> Result<Option<AuctionListingRow>, AnalyticsError> {
        if final_price < 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a sale price cannot be negative",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let item_id = item_id.to_string();
        let sold_at = sold_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let item: Option<(String, f64, f64)> = tx
                    .query_row(
                        "SELECT name, tt_value, markup_paid FROM inventory_items \
                         WHERE id = ? AND state = 'held'",
                        rusqlite::params![item_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let Some((name, tt_value, markup_paid)) = item else {
                    return Ok(None);
                };
                let cost_basis = tt_value + markup_paid;
                tx.execute(
                    "INSERT INTO auction_listings ( \
                         id, item_name, profession, quantity, attributed_qty, unattributed_qty, \
                         tt_value, attributed_tt, starting_bid, buyout, listing_fee, listed_at, \
                         status, final_price, sale_fee, resolved_at, created_at, updated_at, \
                         subject_kind, inventory_item_id, cost_basis, channel) \
                     VALUES (?, ?, 'inventory', 1, 0, 1, ?, 0, ?, NULL, 0, ?, \
                             'sold', ?, 0, ?, ?, ?, 'equipment', ?, ?, 'trade')",
                    rusqlite::params![
                        id,
                        name,
                        tt_value,
                        final_price,
                        sold_at,
                        final_price,
                        sold_at,
                        now,
                        now,
                        item_id,
                        cost_basis,
                    ],
                )?;
                let delta = final_price - cost_basis;
                if delta.abs() > STOCK_EPSILON {
                    let entry_id = Uuid::new_v4().to_string();
                    let kind = if delta > 0.0 { "markup" } else { "expense" };
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        rusqlite::params![
                            entry_id,
                            sold_at,
                            kind,
                            format!("Inventory Sale: {name}"),
                            delta.abs(),
                            INVENTORY_SALE_TAG,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE auction_listings SET sale_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, id],
                    )?;
                }
                tx.execute(
                    "UPDATE inventory_items SET state = 'sold', disposed_at = ?, updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![sold_at, now, item_id],
                )?;
                daily_rollup::refresh_days(&tx, [sold_at.as_str()])?;
                let row = read_listing(&tx, &id)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Confirm a listing sold, at the price it actually fetched.
    ///
    /// This is the recognition boundary: markup becomes realised here and
    /// nowhere earlier, because until the auction closes neither the sale
    /// nor its price is a fact. The ledger gains the money that was not
    /// already booked as loot TT, plus the point-of-sale fee.
    pub async fn confirm_auction_listing(
        &self,
        listing_id: &str,
        final_price: f64,
        sale_fee: f64,
        resolved_at: Option<&str>,
    ) -> Result<Option<AuctionListingRow>, AnalyticsError> {
        if final_price < 0.0 || sale_fee < 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a sale price and fee cannot be negative",
            ));
        }
        let listing_id = listing_id.to_string();
        let resolved_at = resolved_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());

        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(listing) = read_listing(&tx, &listing_id)? else {
                    return Ok(None);
                };
                if listing.status != "pending" {
                    return Ok(None);
                }

                let gross_result = if listing.subject_kind == "equipment" {
                    final_price - listing.cost_basis.unwrap_or(listing.tt_value)
                } else {
                    stock_allocation::resolve_sale(
                        listing.quantity,
                        listing.attributed_qty,
                        listing.tt_value,
                        final_price,
                        listing.listing_fee,
                        sale_fee,
                    )
                    .gross_markup
                };

                tx.execute(
                    "UPDATE auction_listings SET status = 'sold', final_price = ?, \
                         sale_fee = ?, resolved_at = ? WHERE id = ?",
                    rusqlite::params![final_price, sale_fee, resolved_at, listing_id],
                )?;

                // Markup only. Selling at TT converts a position rather than
                // creating income, for untracked stock as much as for tracked:
                // the player held that value either way, and the app simply
                // has no record of where the untracked part came from.
                if gross_result.abs() > STOCK_EPSILON {
                    let entry_id = Uuid::new_v4().to_string();
                    let kind = if gross_result > 0.0 {
                        "markup"
                    } else {
                        "expense"
                    };
                    let tag = if listing.subject_kind == "equipment" {
                        INVENTORY_SALE_TAG
                    } else {
                        "market"
                    };
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        rusqlite::params![
                            entry_id,
                            resolved_at,
                            kind,
                            format!("Auction Sale: {}", listing.item_name),
                            gross_result.abs(),
                            tag,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE auction_listings SET sale_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, listing_id],
                    )?;
                }

                if sale_fee > 0.0 {
                    let entry_id = Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, 'expense', ?, ?, 'market')",
                        rusqlite::params![
                            entry_id,
                            resolved_at,
                            format!("Auction Fee: {}", listing.item_name),
                            sale_fee,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE auction_listings SET sale_fee_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, listing_id],
                    )?;
                }

                if listing.subject_kind == "equipment" {
                    if let Some(item_id) = &listing.inventory_item_id {
                        tx.execute(
                            "UPDATE inventory_items SET state = 'sold', disposed_at = ?, \
                                 updated_at = unixepoch('now') WHERE id = ?",
                            rusqlite::params![resolved_at, item_id],
                        )?;
                    }
                }

                daily_rollup::refresh_days(&tx, [resolved_at.as_str()])?;
                let row = read_listing(&tx, &listing_id)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Mark a listing expired: the stock comes back, the fee stays spent.
    ///
    /// The returning movements are written as new rows rather than by
    /// deleting the original allocation, so what was attributed at listing
    /// time stays auditable. Nothing reaches the activity: the loot returned
    /// intact, and a fee lost to an auction that did not clear describes
    /// market execution rather than the gameplay.
    pub async fn expire_auction_listing(
        &self,
        listing_id: &str,
        resolved_at: Option<&str>,
    ) -> Result<Option<AuctionListingRow>, AnalyticsError> {
        let listing_id = listing_id.to_string();
        let resolved_at = resolved_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());

        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(listing) = read_listing(&tx, &listing_id)? else {
                    return Ok(None);
                };
                if listing.status != "pending" {
                    return Ok(None);
                }

                if listing.subject_kind == "equipment" {
                    if let Some(item_id) = &listing.inventory_item_id {
                        tx.execute(
                            "UPDATE inventory_items SET state = 'held', disposed_at = NULL, \
                                 updated_at = ? WHERE id = ?",
                            rusqlite::params![now, item_id],
                        )?;
                    }
                } else {
                    // (source, tier, species, definition, tool, quantity, tt)
                    // of each original listing movement, to be written back
                    // in reverse without requiring an optional target label.
                    type ReturningRow = (
                        String,
                        Option<String>,
                        Option<String>,
                        Option<i64>,
                        Option<String>,
                        Option<i64>,
                        Option<i64>,
                        Option<i64>,
                        Option<i64>,
                        f64,
                        f64,
                    );
                    let returning: Vec<ReturningRow> = {
                        let mut stmt = tx.prepare(
                            "SELECT source_kind, yield_tier, mob_species, \
                                    session_definition_id, tool_name, \
                                    quest_reward_item_id, quest_run_id, quest_id, \
                                    activity_context_id, \
                                    quantity, tt_value \
                             FROM stock_movements \
                             WHERE ref_id = ? AND movement_kind = 'listing'",
                        )?;
                        let rows = stmt
                            .query_map(rusqlite::params![listing_id], |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                    row.get(5)?,
                                    row.get(6)?,
                                    row.get(7)?,
                                    row.get(8)?,
                                    row.get(9)?,
                                    row.get(10)?,
                                ))
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        rows
                    };
                    for (
                        source_kind,
                        tier,
                        species,
                        definition_id,
                        tool,
                        reward_item_id,
                        run_id,
                        quest_id,
                        context_id,
                        quantity,
                        tt_value,
                    ) in returning
                    {
                        let provenance = match source_kind.as_str() {
                            "harvest" => tier.as_deref().map(|tier| {
                                stock_allocation::StockProvenance::Harvest(
                                    HarvestYieldTier::from_db(tier),
                                )
                            }),
                            "hunt" => {
                                Some(stock_allocation::StockProvenance::Hunt(species.as_deref()))
                            }
                            "quest" => match (reward_item_id, run_id, quest_id) {
                                (Some(reward_item_id), Some(quest_run_id), Some(quest_id)) => {
                                    Some(stock_allocation::StockProvenance::Quest {
                                        reward_item_id,
                                        quest_run_id,
                                        quest_id,
                                        activity_context_id: context_id,
                                    })
                                }
                                _ => None,
                            },
                            _ => None,
                        };
                        insert_movement(
                            &tx,
                            &listing.item_name,
                            "listing_return",
                            Some(&listing_id),
                            provenance,
                            definition_id,
                            context_id,
                            tool.as_deref(),
                            -quantity,
                            -tt_value,
                            &resolved_at,
                            now,
                        )?;
                    }
                }

                tx.execute(
                    "UPDATE auction_listings SET status = 'expired', resolved_at = ? WHERE id = ?",
                    rusqlite::params![resolved_at, listing_id],
                )?;
                let row = read_listing(&tx, &listing_id)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Recycle stock into another item at 1:1 TT.
    ///
    /// A transformation, not a sale: no markup is realised and the ledger is
    /// untouched. The consumed stock's activity composition rides forward
    /// into the produced item, so selling the result still attributes back to
    /// the activities that grew it.
    pub async fn convert_stock(
        &self,
        profession: Profession,
        source_item: &str,
        target_item: &str,
        quantity: f64,
        converted_at: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        if is_universal_ammo(source_item) || is_universal_ammo(target_item) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid value, not inventory stock",
            ));
        }
        self.convert_stock_with_ratio(
            profession,
            source_item,
            target_item,
            quantity,
            converted_at,
            1.0,
        )
        .await
    }

    /// Convert held Shrapnel into Universal Ammo at the game's fixed 100:101
    /// ratio. The 1% increase becomes realised only here, when the player says
    /// the conversion happened.
    pub async fn convert_shrapnel(
        &self,
        profession: Profession,
        quantity: f64,
        converted_at: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        self.convert_stock_with_ratio(
            profession,
            "Shrapnel",
            "Universal Ammo",
            quantity,
            converted_at,
            1.01,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn convert_stock_with_ratio(
        &self,
        profession: Profession,
        source_item: &str,
        target_item: &str,
        quantity: f64,
        converted_at: Option<&str>,
        value_ratio: f64,
    ) -> Result<(), AnalyticsError> {
        if quantity <= 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a conversion needs a positive quantity",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let (source_c, target_c) = (source_item.to_string(), target_item.to_string());
        let converted_at = converted_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());

        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let (positions, unit_tt) = item_positions(&tx, &source_c)?;
                // What the produced item is worth per unit, so the conversion
                // records a count rather than a value wearing a count's label.
                // Falling back to the target's own recorded loot covers a
                // future target this table has not learned yet. No conversion
                // the app offers reaches the last fallback; if one ever does,
                // the count carries the source's PED magnitude rather than a
                // scale nothing supports.
                let (_, target_loot_unit_tt) = item_positions(&tx, &target_c)?;
                let target_unit_tt = produced_unit_tt(&target_c)
                    .or_else(|| {
                        (target_loot_unit_tt > STOCK_EPSILON).then_some(target_loot_unit_tt)
                    })
                    .unwrap_or(1.0);
                // The service tolerates converting past tracked stock for the
                // same reason it tolerates selling past it: the player may
                // hold stock from before tracking began, and the excess rides
                // forward explicitly unattributed rather than being invented.
                // The product rule is narrower and is enforced at the modal,
                // because what a conversion produces carries the source's
                // activity composition forward and crediting activities with
                // output they did not grow is the thing to avoid.
                let plan =
                    stock_allocation::allocate(&as_source_positions(&positions), quantity, unit_tt);
                let converted_tt = quantity * unit_tt;
                let gain =
                    eo_wire::normalizer::round_half_even(converted_tt * (value_ratio - 1.0), 4);
                let output_tt = converted_tt + gain;
                let realised_output_tt = (gain > STOCK_EPSILON).then_some(output_tt);
                let realised_attributed_tt = (gain > STOCK_EPSILON).then_some(plan.attributed_tt);

                tx.execute(
                    "INSERT INTO stock_conversions \
                         (id, source_item, target_item, profession, quantity, tt_value, \
                          output_tt_value, attributed_tt, converted_at, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        id,
                        source_c,
                        target_c,
                        profession.as_str(),
                        quantity,
                        converted_tt,
                        realised_output_tt,
                        realised_attributed_tt,
                        converted_at,
                        now
                    ],
                )?;

                record_opening_balance(&tx, &source_c, &id, &plan, &converted_at, now)?;

                for allocation in &plan.allocations {
                    insert_movement(
                        &tx,
                        &source_c,
                        "conversion_out",
                        Some(&id),
                        allocation.provenance,
                        allocation.session_definition_id,
                        allocation.activity_context_id,
                        allocation.tool_name,
                        -allocation.quantity,
                        -allocation.tt_value,
                        &converted_at,
                        now,
                    )?;
                    if !is_universal_ammo(&target_c) {
                        let allocation_output_tt = if converted_tt > STOCK_EPSILON {
                            allocation.tt_value + gain * (allocation.tt_value / converted_tt)
                        } else {
                            allocation.tt_value
                        };
                        insert_movement(
                            &tx,
                            &target_c,
                            "conversion_in",
                            Some(&id),
                            allocation.provenance,
                            allocation.session_definition_id,
                            allocation.activity_context_id,
                            allocation.tool_name,
                            allocation_output_tt / target_unit_tt,
                            allocation_output_tt,
                            &converted_at,
                            now,
                        )?;
                    }
                }

                if gain > STOCK_EPSILON {
                    let entry_id = Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, 'markup', ?, ?, 'convert')",
                        rusqlite::params![entry_id, converted_at, "Shrapnel Conversion", gain,],
                    )?;
                    tx.execute(
                        "UPDATE stock_conversions SET gain_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, id],
                    )?;
                    daily_rollup::refresh_days(&tx, [converted_at.as_str()])?;
                }

                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Record a private player-to-player sale. There is no listing lifecycle
    /// and no auction fee: the entered price is final, so recognition and the
    /// stock outflow happen atomically.
    pub async fn create_private_sale(
        &self,
        profession: Profession,
        item_name: &str,
        quantity: f64,
        final_price: f64,
        sold_at: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        if is_universal_ammo(item_name) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and cannot be traded",
            ));
        }
        if quantity <= 0.0 || final_price < 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a trade needs a positive quantity and a non-negative price",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let item = item_name.to_string();
        let sold_at = sold_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let (positions, unit_tt) = item_positions(&tx, &item)?;
                let plan =
                    stock_allocation::allocate(&as_source_positions(&positions), quantity, unit_tt);
                let tt_value = quantity * unit_tt;
                tx.execute(
                    "INSERT INTO private_sales (id, item_name, profession, quantity, \
                         attributed_qty, unattributed_qty, tt_value, attributed_tt, final_price, \
                         sold_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        id,
                        item,
                        profession.as_str(),
                        quantity,
                        plan.attributed_qty,
                        plan.unattributed_qty,
                        tt_value,
                        plan.attributed_tt,
                        final_price,
                        sold_at,
                        now,
                    ],
                )?;
                record_opening_balance(&tx, &item, &id, &plan, &sold_at, now)?;
                for allocation in &plan.allocations {
                    insert_movement(
                        &tx,
                        &item,
                        "trade",
                        Some(&id),
                        allocation.provenance,
                        allocation.session_definition_id,
                        allocation.activity_context_id,
                        allocation.tool_name,
                        -allocation.quantity,
                        -allocation.tt_value,
                        &sold_at,
                        now,
                    )?;
                }
                let markup = final_price - tt_value;
                if markup.abs() > STOCK_EPSILON {
                    let entry_id = Uuid::new_v4().to_string();
                    let kind = if markup > 0.0 { "markup" } else { "expense" };
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, ?, ?, ?, 'market')",
                        rusqlite::params![
                            entry_id,
                            sold_at,
                            kind,
                            format!("Private Sale: {item}"),
                            markup.abs(),
                        ],
                    )?;
                    tx.execute(
                        "UPDATE private_sales SET sale_entry_id = ? WHERE id = ?",
                        rusqlite::params![entry_id, id],
                    )?;
                }
                daily_rollup::refresh_days(&tx, [sold_at.as_str()])?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Remove held stock whose ultimate outcome is unknown. This changes the
    /// current position only: it writes no ledger row and cannot consume more
    /// than the app currently knows is held.
    pub async fn remove_stock(
        &self,
        profession: Profession,
        item_name: &str,
        quantity: f64,
        removed_at: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        if is_universal_ammo(item_name) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held as stock",
            ));
        }
        if quantity <= 0.0 {
            return Err(AnalyticsError::InvalidInput(
                "a removal needs a positive quantity",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let item = item_name.to_string();
        let removed_at = removed_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let (positions, unit_tt) = item_positions(&tx, &item)?;
                let plan =
                    stock_allocation::allocate(&as_source_positions(&positions), quantity, unit_tt);
                if plan.excess_qty > STOCK_EPSILON {
                    return Ok(Err(
                        "you cannot remove more than the current stock".to_string()
                    ));
                }
                tx.execute(
                    "INSERT INTO stock_removals \
                         (id, item_name, profession, quantity, tt_value, removed_at, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        id,
                        item,
                        profession.as_str(),
                        quantity,
                        quantity * unit_tt,
                        removed_at,
                        now,
                    ],
                )?;
                for allocation in &plan.allocations {
                    insert_movement(
                        &tx,
                        &item,
                        "removal",
                        Some(&id),
                        allocation.provenance,
                        allocation.session_definition_id,
                        allocation.activity_context_id,
                        allocation.tool_name,
                        -allocation.quantity,
                        -allocation.tt_value,
                        &removed_at,
                        now,
                    )?;
                }
                tx.commit()?;
                Ok(Ok(()))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Everything this activity has done to its stock, newest first.
    ///
    /// Listings, trades, conversions, and removals in one list, each carrying whether it can be
    /// taken back. The verdict is computed here because it depends on what the
    /// rest of the ledger has since done with the stock, which the caller
    /// cannot see.
    pub async fn activity_history(
        &self,
        profession: Profession,
    ) -> Result<Vec<ActivityHistoryRow>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut rows: Vec<ActivityHistoryRow> = Vec::new();

                {
                    // History is the one read that sees undone entries: they
                    // are the record of a correction, kept read-only.
                    let mut stmt = conn.prepare(&format!(
                        "SELECT {LISTING_COLUMNS}, undone_at FROM auction_listings \
                         WHERE profession = ?"
                    ))?;
                    let listings = stmt
                        .query_map(rusqlite::params![profession.as_str()], |row| {
                            // By name, not by position: this column sits past
                            // the end of LISTING_COLUMNS, so an index would
                            // silently start reading a different column the
                            // next time that list grows.
                            Ok((
                                listing_from_row(row),
                                row.get::<_, Option<String>>("undone_at")?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (listing, undone_at) in listings {
                        let net_markup = listing.gross_markup.map(|gross| {
                            gross - listing.listing_fee - listing.sale_fee.unwrap_or(0.0)
                        });
                        let undone = undone_at.is_some();
                        let blocker = if undone {
                            None
                        } else {
                            reversal_blocker(conn, &listing.id)?
                        };
                        rows.push(ActivityHistoryRow {
                            occurred_at: listing
                                .resolved_at
                                .clone()
                                .unwrap_or_else(|| listing.listed_at.clone()),
                            kind: if listing.channel == "trade" {
                                "trade".to_string()
                            } else {
                                "listing".to_string()
                            },
                            subject_kind: listing.subject_kind.clone(),
                            channel: listing.channel.clone(),
                            status: listing.status.clone(),
                            item_name: listing.item_name.clone(),
                            target_item: None,
                            quantity: listing.quantity,
                            tt_value: listing.tt_value,
                            net_markup,
                            activity_net_markup: listing.activity_net_markup,
                            can_revert_sale: !undone
                                && listing.status == "sold"
                                && listing.channel == "auction",
                            can_delete: !undone && blocker.is_none(),
                            undo_blocked_reason: blocker
                                .map(|(item, short)| blocked_reason(&item, short)),
                            undone,
                            id: listing.id,
                        });
                    }
                }

                {
                    let mut stmt = conn.prepare(
                        "SELECT id, source_item, target_item, quantity, tt_value, converted_at, \
                                undone_at, output_tt_value, attributed_tt \
                         FROM stock_conversions WHERE profession = ?",
                    )?;
                    let conversions = stmt
                        .query_map(rusqlite::params![profession.as_str()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, f64>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, Option<String>>(6)?,
                                row.get::<_, Option<f64>>(7)?,
                                row.get::<_, Option<f64>>(8)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (
                        id,
                        source,
                        target,
                        quantity,
                        tt_value,
                        converted_at,
                        undone_at,
                        output_tt,
                        attributed_tt,
                    ) in conversions
                    {
                        let undone = undone_at.is_some();
                        let blocker = if undone {
                            None
                        } else {
                            reversal_blocker(conn, &id)?
                        };
                        rows.push(ActivityHistoryRow {
                            id,
                            kind: "conversion".to_string(),
                            subject_kind: "loot".to_string(),
                            channel: "conversion".to_string(),
                            status: "converted".to_string(),
                            item_name: source,
                            target_item: Some(target),
                            occurred_at: converted_at,
                            quantity,
                            tt_value,
                            net_markup: output_tt.map(|output| output - tt_value),
                            activity_net_markup: output_tt.zip(attributed_tt).map(
                                |(output, attributed)| {
                                    stock_allocation::resolve_sale(
                                        quantity,
                                        if tt_value > STOCK_EPSILON {
                                            quantity * attributed / tt_value
                                        } else {
                                            0.0
                                        },
                                        tt_value,
                                        output,
                                        0.0,
                                        0.0,
                                    )
                                    .activity_net_markup
                                },
                            ),
                            can_revert_sale: false,
                            can_delete: !undone && blocker.is_none(),
                            undo_blocked_reason: blocker
                                .map(|(item, short)| blocked_reason(&item, short)),
                            undone,
                        });
                    }
                }

                {
                    let mut stmt = conn.prepare(
                        "SELECT id, item_name, quantity, attributed_qty, tt_value, final_price, \
                                sold_at, undone_at FROM private_sales WHERE profession = ?",
                    )?;
                    let sales = stmt
                        .query_map(rusqlite::params![profession.as_str()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, f64>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, f64>(4)?,
                                row.get::<_, f64>(5)?,
                                row.get::<_, String>(6)?,
                                row.get::<_, Option<String>>(7)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (id, item, quantity, attributed_qty, tt, price, sold_at, undone_at) in sales
                    {
                        let outcome = stock_allocation::resolve_sale(
                            quantity,
                            attributed_qty,
                            tt,
                            price,
                            0.0,
                            0.0,
                        );
                        rows.push(ActivityHistoryRow {
                            id,
                            kind: "trade".to_string(),
                            subject_kind: "loot".to_string(),
                            channel: "trade".to_string(),
                            status: "sold".to_string(),
                            item_name: item,
                            target_item: None,
                            occurred_at: sold_at,
                            quantity,
                            tt_value: tt,
                            net_markup: Some(outcome.net_markup),
                            activity_net_markup: Some(outcome.activity_net_markup),
                            can_revert_sale: false,
                            can_delete: undone_at.is_none(),
                            undo_blocked_reason: None,
                            undone: undone_at.is_some(),
                        });
                    }
                }

                {
                    let mut stmt = conn.prepare(
                        "SELECT id, item_name, quantity, tt_value, removed_at, undone_at \
                         FROM stock_removals WHERE profession = ?",
                    )?;
                    let removals = stmt
                        .query_map(rusqlite::params![profession.as_str()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, f64>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (id, item, quantity, tt, removed_at, undone_at) in removals {
                        rows.push(ActivityHistoryRow {
                            id,
                            kind: "removal".to_string(),
                            subject_kind: "loot".to_string(),
                            channel: "removal".to_string(),
                            status: "removed".to_string(),
                            item_name: item,
                            target_item: None,
                            occurred_at: removed_at,
                            quantity,
                            tt_value: tt,
                            net_markup: None,
                            activity_net_markup: None,
                            can_revert_sale: false,
                            can_delete: undone_at.is_none(),
                            undo_blocked_reason: None,
                            undone: undone_at.is_some(),
                        });
                    }
                }

                // Newest first, and stable on the id so a day with several
                // entries does not reshuffle between reads.
                rows.sort_by(|a, b| {
                    b.occurred_at
                        .cmp(&a.occurred_at)
                        .then_with(|| a.id.cmp(&b.id))
                });
                Ok(rows)
            })
            .await?)
    }

    /// Take back a confirmed sale, leaving the listing open again.
    ///
    /// The stock does not move: it left at listing time and, with the listing
    /// open once more, it is still out. Only the recognition is undone, so the
    /// money the sale wrote goes with it and the markup stops being realised.
    pub async fn revert_auction_sale(
        &self,
        listing_id: &str,
    ) -> Result<Option<AuctionListingRow>, AnalyticsError> {
        let listing_id = listing_id.to_string();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(listing) = read_listing(&tx, &listing_id)? else {
                    return Ok(None);
                };
                if listing.status != "sold" {
                    return Ok(None);
                }
                // A trade resolves in one step and has no open stage to
                // return to; only an auction sale can be taken back.
                if listing.channel != "auction" {
                    return Ok(None);
                }

                let mut days: Vec<String> = Vec::new();
                for column in ["sale_entry_id", "sale_fee_entry_id"] {
                    if let Some(day) = delete_owned_ledger_entry(&tx, &listing_id, column)? {
                        days.push(day);
                    }
                }

                tx.execute(
                    "UPDATE auction_listings \
                     SET status = 'pending', final_price = NULL, sale_fee = NULL, \
                         resolved_at = NULL, sale_entry_id = NULL, sale_fee_entry_id = NULL \
                     WHERE id = ?",
                    rusqlite::params![listing_id],
                )?;
                if listing.subject_kind == "equipment" {
                    if let Some(item_id) = &listing.inventory_item_id {
                        tx.execute(
                            "UPDATE inventory_items SET state = 'listed', disposed_at = NULL, \
                                 updated_at = unixepoch('now') WHERE id = ?",
                            rusqlite::params![item_id],
                        )?;
                    }
                }
                daily_rollup::refresh_days(&tx, days)?;
                let row = read_listing(&tx, &listing_id)?;
                tx.commit()?;
                Ok(row)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Undo a listing: the stock it took comes back, and every ledger row it
    /// wrote goes with it.
    ///
    /// The listing itself stays, marked. Its effects are what was wrong, not
    /// the fact that it was recorded, and a correction that erases its own
    /// evidence leaves the player unable to see what they changed.
    ///
    /// The movements and money go rather than being compensated: this is a
    /// mis-entry, not a market event. An expiry is the other case and keeps
    /// its returning rows, because the stock genuinely came back.
    pub async fn undo_auction_listing(&self, listing_id: &str) -> Result<bool, AnalyticsError> {
        let listing_id = listing_id.to_string();
        let undone_at = self.default_date();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                // `read_listing` sees only live listings, so undoing one twice
                // reports not-found rather than reversing it again.
                let Some(listing) = read_listing(&tx, &listing_id)? else {
                    return Ok(Ok(false));
                };
                if let Some((item, short)) = reversal_blocker(&tx, &listing_id)? {
                    return Ok(Err(blocked_reason(&item, short)));
                }

                let mut days = vec![listing.listed_at.clone()];
                if let Some(resolved) = listing.resolved_at.clone() {
                    days.push(resolved);
                }
                for column in ["fee_entry_id", "sale_entry_id", "sale_fee_entry_id"] {
                    if let Some(day) = delete_owned_ledger_entry(&tx, &listing_id, column)? {
                        days.push(day);
                    }
                }

                tx.execute(
                    "DELETE FROM stock_movements WHERE ref_id = ?",
                    rusqlite::params![listing_id],
                )?;
                if listing.subject_kind == "equipment" {
                    if let Some(item_id) = &listing.inventory_item_id {
                        tx.execute(
                            "UPDATE inventory_items SET state = 'held', disposed_at = NULL, \
                                 updated_at = unixepoch('now') WHERE id = ?",
                            rusqlite::params![item_id],
                        )?;
                    }
                }
                // The entry stands as a record; its pointers do not, so a
                // later read cannot follow them to a row somebody else owns.
                tx.execute(
                    "UPDATE auction_listings \
                     SET undone_at = ?, fee_entry_id = NULL, sale_entry_id = NULL, \
                         sale_fee_entry_id = NULL \
                     WHERE id = ?",
                    rusqlite::params![undone_at, listing_id],
                )?;
                daily_rollup::refresh_days(&tx, days)?;
                tx.commit()?;
                Ok(Ok(true))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Undo a conversion: what it consumed comes back and what it produced is
    /// unmade. Refused when those produced units have since left.
    ///
    /// Like a listing, the entry stays on file marked rather than removed.
    pub async fn undo_stock_conversion(&self, conversion_id: &str) -> Result<bool, AnalyticsError> {
        let conversion_id = conversion_id.to_string();
        let undone_at = self.default_date();
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let conversion: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT converted_at, gain_entry_id FROM stock_conversions \
                         WHERE id = ? AND undone_at IS NULL",
                        rusqlite::params![conversion_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let Some((converted_at, gain_entry_id)) = conversion else {
                    return Ok(Ok(false));
                };
                if let Some((item, short)) = reversal_blocker(&tx, &conversion_id)? {
                    return Ok(Err(blocked_reason(&item, short)));
                }

                tx.execute(
                    "DELETE FROM stock_movements WHERE ref_id = ?",
                    rusqlite::params![conversion_id],
                )?;
                if let Some(entry_id) = gain_entry_id {
                    tx.execute(
                        "DELETE FROM ledger_entries WHERE id = ?",
                        rusqlite::params![entry_id],
                    )?;
                }
                tx.execute(
                    "UPDATE stock_conversions SET undone_at = ?, gain_entry_id = NULL WHERE id = ?",
                    rusqlite::params![undone_at, conversion_id],
                )?;
                // A conversion writes no money, but its day is refreshed all
                // the same: cheap, and it keeps every undo path the same shape.
                daily_rollup::refresh_days(&tx, [converted_at])?;
                tx.commit()?;
                Ok(Ok(true))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Undo a private trade recorded in error: restore its stock and remove
    /// the markup row it owned. The history entry remains as the correction
    /// record.
    pub async fn undo_private_sale(&self, sale_id: &str) -> Result<bool, AnalyticsError> {
        let sale_id = sale_id.to_string();
        let undone_at = self.default_date();
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let sale: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT sold_at, sale_entry_id FROM private_sales \
                         WHERE id = ? AND undone_at IS NULL",
                        rusqlite::params![sale_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let Some((sold_at, entry_id)) = sale else {
                    return Ok(false);
                };
                tx.execute(
                    "DELETE FROM stock_movements WHERE ref_id = ?",
                    rusqlite::params![sale_id],
                )?;
                if let Some(entry_id) = entry_id {
                    tx.execute(
                        "DELETE FROM ledger_entries WHERE id = ?",
                        rusqlite::params![entry_id],
                    )?;
                }
                tx.execute(
                    "UPDATE private_sales SET undone_at = ?, sale_entry_id = NULL WHERE id = ?",
                    rusqlite::params![undone_at, sale_id],
                )?;
                daily_rollup::refresh_days(&tx, [sold_at])?;
                tx.commit()?;
                Ok(true)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Undo an uncertain removal recorded in error. No ledger row exists;
    /// deleting its movements is enough to restore the position.
    pub async fn undo_stock_removal(&self, removal_id: &str) -> Result<bool, AnalyticsError> {
        let removal_id = removal_id.to_string();
        let undone_at = self.default_date();
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM stock_removals WHERE id = ? AND undone_at IS NULL",
                        rusqlite::params![removal_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if exists.is_none() {
                    return Ok(false);
                }
                tx.execute(
                    "DELETE FROM stock_movements WHERE ref_id = ?",
                    rusqlite::params![removal_id],
                )?;
                tx.execute(
                    "UPDATE stock_removals SET undone_at = ? WHERE id = ?",
                    rusqlite::params![undone_at, removal_id],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await
            .map_err(AnalyticsError::from)
    }

    /// Net realised markup per yield tier, from confirmed stock outcomes.
    ///
    /// Each sold listing's activity-claimable markup is divided across the
    /// sources that supplied it, in proportion to the quantity each contributed.
    /// Pending listings realise nothing, and expired ones never realise.
    ///
    /// The tool that produced the stock is recorded on the movement rows but
    /// is deliberately not reported: it is an input to the activity, not an
    /// outcome, so it is compared on cost with equipment rather than on
    /// returns it does not cause.
    pub async fn realised_markup_by_tier(&self) -> Result<Vec<RealisedTierMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let sold = realised_stock_outcomes(conn)?;

                let mut totals: std::collections::BTreeMap<String, f64> =
                    std::collections::BTreeMap::new();
                for realised in sold {
                    let id = realised.id;
                    let outcome = realised.outcome;
                    if outcome.activity_net_markup.abs() <= STOCK_EPSILON {
                        continue;
                    }
                    // Attributed quantity spans every provenance dimension, so the
                    // share denominator does too: a sale drawing on boards
                    // AND hides credits each side only what it supplied.
                    let contributions: Vec<(Option<String>, f64)> = {
                        let mut stmt = conn.prepare(
                            "SELECT yield_tier, SUM(-quantity) FROM stock_movements \
                             WHERE ref_id = ? AND movement_kind = ? \
                               AND source_kind IN ('hunt', 'harvest', 'quest') \
                             GROUP BY yield_tier",
                        )?;
                        let rows = stmt
                            .query_map(rusqlite::params![id, realised.movement_kind], |row| {
                                Ok((row.get(0)?, row.get(1)?))
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        rows
                    };
                    let contributed_quantity: f64 = contributions.iter().map(|(_, qty)| qty).sum();
                    if contributed_quantity <= STOCK_EPSILON {
                        continue;
                    }
                    for (tier, quantity) in contributions {
                        let Some(tier) = tier else { continue };
                        let share = quantity / contributed_quantity;
                        *totals.entry(tier).or_insert(0.0) += outcome.activity_net_markup * share;
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

    /// Net realised markup per mob species, from confirmed stock outcomes: the
    /// Hunting sibling of [`Self::realised_markup_by_tier`], reading the
    /// species dimension of the same movement ledger. A sold listing's
    /// activity-claimable markup divides across every contributing source in
    /// proportion to the TT each supplied, so a joint-provenance sale credits
    /// tiers and species each their own share without double counting.
    pub async fn realised_markup_by_species(
        &self,
    ) -> Result<Vec<RealisedSpeciesMarkup>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let sold = realised_stock_outcomes(conn)?;

                let mut totals: std::collections::BTreeMap<String, f64> =
                    std::collections::BTreeMap::new();
                for realised in sold {
                    let id = realised.id;
                    let outcome = realised.outcome;
                    if outcome.activity_net_markup.abs() <= STOCK_EPSILON {
                        continue;
                    }
                    // Attributed quantity spans every provenance dimension; the
                    // share denominator has to as well, or a joint sale
                    // would credit the species side more than it supplied.
                    let contributions: Vec<(Option<String>, f64)> = {
                        let mut stmt = conn.prepare(
                            "SELECT CASE WHEN source_kind = 'hunt' OR mob_species IS NOT NULL \
                                         THEN COALESCE(mob_species, '') ELSE NULL END, \
                                    SUM(-quantity) FROM stock_movements \
                             WHERE ref_id = ? AND movement_kind = ? \
                               AND source_kind IN ('hunt', 'harvest', 'quest') \
                             GROUP BY 1",
                        )?;
                        let rows = stmt
                            .query_map(rusqlite::params![id, realised.movement_kind], |row| {
                                Ok((row.get(0)?, row.get(1)?))
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        rows
                    };
                    let contributed_quantity: f64 = contributions.iter().map(|(_, qty)| qty).sum();
                    if contributed_quantity <= STOCK_EPSILON {
                        continue;
                    }
                    for (species, quantity) in contributions {
                        let Some(species) = species else { continue };
                        let share = quantity / contributed_quantity;
                        *totals.entry(species).or_insert(0.0) +=
                            outcome.activity_net_markup * share;
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
                let sold = realised_stock_outcomes(conn)?;

                let mut totals: std::collections::BTreeMap<i64, f64> =
                    std::collections::BTreeMap::new();
                for realised in sold {
                    let id = realised.id;
                    let outcome = realised.outcome;
                    if outcome.activity_net_markup.abs() <= STOCK_EPSILON {
                        continue;
                    }
                    let contributions: Vec<(Option<i64>, f64)> = {
                        let mut stmt = conn.prepare(
                            "SELECT session_definition_id, SUM(-quantity) \
                             FROM stock_movements \
                             WHERE ref_id = ? AND movement_kind = ? \
                               AND source_kind IN ('hunt', 'harvest', 'quest') \
                             GROUP BY session_definition_id",
                        )?;
                        let rows = stmt
                            .query_map(rusqlite::params![id, realised.movement_kind], |row| {
                                Ok((row.get(0)?, row.get(1)?))
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        rows
                    };
                    let contributed_quantity: f64 = contributions.iter().map(|(_, qty)| qty).sum();
                    if contributed_quantity <= STOCK_EPSILON {
                        continue;
                    }
                    for (definition_id, quantity) in contributions {
                        let Some(definition_id) = definition_id else {
                            continue;
                        };
                        *totals.entry(definition_id).or_insert(0.0) +=
                            outcome.activity_net_markup * (quantity / contributed_quantity);
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

    /// The stored inventory row re-read and shaped (the create / patch
    /// reply). A row that has vanished since the write is a driver-level
    /// invariant break, surfaced as [`AnalyticsError::Storage`].
    async fn inventory_row(&self, item_id: &str) -> Result<InventoryRow, AnalyticsError> {
        let item_id = item_id.to_string();
        Ok(self
            .db
            .with_reader(move |conn| {
                conn.query_row(
                    "SELECT id, name, tt_value, markup_paid, notes, acquired_at \
                     FROM inventory_items WHERE id = ?",
                    rusqlite::params![item_id],
                    |row| Ok(inventory_item(row)),
                )
                .map_err(DbError::from)
            })
            .await?)
    }

    /// Create an inventory item; an empty / absent `acquired_at` defaults
    /// to the clock's UTC date. Returns the stored row's wire shape.
    pub async fn create_inventory_item(
        &self,
        name: &str,
        tt_value: f64,
        markup_paid: f64,
        notes: Option<&str>,
        acquired_at: Option<&str>,
    ) -> Result<InventoryRow, AnalyticsError> {
        if is_universal_ammo(name) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held in Inventory",
            ));
        }
        let id = Uuid::new_v4().to_string();
        // acquired_at falls back to today's UTC date: the original's `or`
        // treats an empty string as falsy, so "" defaults to the clock date.
        let date = acquired_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        {
            let (id, name, notes, date) = (
                id.clone(),
                name.to_string(),
                notes.map(str::to_string),
                date.clone(),
            );
            self.db
                .with_writer(move |conn| {
                    conn.execute(
                        "INSERT INTO inventory_items \
                         (id, name, tt_value, markup_paid, notes, acquired_at) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        rusqlite::params![id, name, tt_value, markup_paid, notes, date],
                    )?;
                    Ok(())
                })
                .await?;
        }
        self.inventory_row(&id).await
    }

    /// Update an inventory item: only provided (`Some`) fields change,
    /// bumping `updated_at`; an all-`None` patch still re-reads and returns
    /// the row. `None` (not `Some(Value::Null)`) reports a missing item.
    pub async fn update_inventory_item(
        &self,
        item_id: &str,
        name: Option<&str>,
        tt_value: Option<f64>,
        markup_paid: Option<f64>,
        notes: Option<&str>,
    ) -> Result<Option<InventoryRow>, AnalyticsError> {
        if name.is_some_and(is_universal_ammo) {
            return Err(AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held in Inventory",
            ));
        }
        // The existence check and the (possibly empty) update run together on
        // the writer connection, which reads as well as writes.
        let updated = {
            let item_id = item_id.to_string();
            let name = name.map(str::to_string);
            let notes = notes.map(str::to_string);
            self.db
                .with_writer(move |conn| {
                    use rusqlite::OptionalExtension as _;
                    let exists = conn
                        .query_row(
                            "SELECT 1 FROM inventory_items WHERE id = ? AND state = 'held'",
                            rusqlite::params![item_id],
                            |_| Ok(()),
                        )
                        .optional()?;
                    if exists.is_none() {
                        return Ok(false);
                    }

                    let mut sets: Vec<&str> = Vec::new();
                    if name.is_some() {
                        sets.push("name = ?");
                    }
                    if tt_value.is_some() {
                        sets.push("tt_value = ?");
                    }
                    if markup_paid.is_some() {
                        sets.push("markup_paid = ?");
                    }
                    if notes.is_some() {
                        sets.push("notes = ?");
                    }
                    if !sets.is_empty() {
                        sets.push("updated_at = unixepoch('now')");
                        let sql = format!(
                            "UPDATE inventory_items SET {} WHERE id = ?",
                            sets.join(", ")
                        );
                        let mut params: Vec<rusqlite::types::Value> = Vec::new();
                        if let Some(value) = &name {
                            params.push(value.clone().into());
                        }
                        if let Some(value) = tt_value {
                            params.push(value.into());
                        }
                        if let Some(value) = markup_paid {
                            params.push(value.into());
                        }
                        if let Some(value) = &notes {
                            params.push(value.clone().into());
                        }
                        params.push(item_id.clone().into());
                        conn.execute(&sql, rusqlite::params_from_iter(params))?;
                    }
                    Ok(true)
                })
                .await?
        };
        if !updated {
            return Ok(None);
        }
        Ok(Some(self.inventory_row(item_id).await?))
    }

    /// Delete an inventory item. Returns whether a row existed.
    pub async fn delete_inventory_item(&self, item_id: &str) -> Result<bool, AnalyticsError> {
        let item_id = item_id.to_string();
        let affected = self
            .db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    "DELETE FROM inventory_items WHERE id = ? AND state = 'held'",
                    rusqlite::params![item_id],
                )?)
            })
            .await?;
        Ok(affected != 0)
    }

    /// Sell an inventory item: emit the realised delta to the ledger and
    /// retain the row as sold, atomically; a zero-delta sale skips the ledger row
    /// (`ledgerEntry` null). Returns the `{ ledgerEntry, soldItem }` shape,
    /// or `None` for a missing item.
    pub async fn sell_inventory_item(
        &self,
        item_id: &str,
        sale_price: f64,
        description: Option<&str>,
        sold_at: Option<&str>,
    ) -> Result<Option<InventorySale>, AnalyticsError> {
        // The item is read on a reader-core connection; the realised sale then
        // writes its ledger row and marks the item sold in one writer transaction
        // (the rollup refresh must commit atomically with the ledger insert).
        let fetched = {
            let item_id = item_id.to_string();
            self.db
                .with_reader(move |conn| {
                    use rusqlite::OptionalExtension as _;
                    conn.query_row(
                        "SELECT id, name, tt_value, markup_paid, notes, acquired_at \
                         FROM inventory_items WHERE id = ? AND state = 'held'",
                        rusqlite::params![item_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(1)?,
                                row.get::<_, f64>(2)?,
                                row.get::<_, f64>(3)?,
                                inventory_item(row),
                            ))
                        },
                    )
                    .optional()
                    .map_err(DbError::from)
                })
                .await?
        };
        let Some((name, tt_value, markup_paid, sold_item)) = fetched else {
            return Ok(None);
        };

        let cost_basis = tt_value + markup_paid;
        let delta = sale_price - cost_basis;
        // sold_at falls back to today's UTC date; an empty string counts as absent.
        let sold_at = sold_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());

        // The realised sale writes its ledger row (when non-zero) and retains
        // the item as sold in one writer-core transaction; the rollup refresh commits
        // atomically with the ledger insert.
        let ledger_write: Option<(String, String, &'static str, String, f64)> = if delta != 0.0 {
            let entry_id = Uuid::new_v4().to_string();
            let entry_type = if delta > 0.0 { "markup" } else { "expense" };
            let amount = delta.abs();
            // `payload.description or "Inventory Sale: {name}"`: "" is falsy.
            let description = description
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Inventory Sale: {name}"));
            Some((entry_id, sold_at.clone(), entry_type, description, amount))
        } else {
            None
        };
        let item_id_owned = item_id.to_string();
        let ledger_for_closure = ledger_write.clone();
        let updated = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let affected = tx.execute(
                    "UPDATE inventory_items SET state = 'sold', disposed_at = ?, \
                         updated_at = unixepoch('now') WHERE id = ? AND state = 'held'",
                    rusqlite::params![sold_at, item_id_owned],
                )?;
                if affected == 0 {
                    return Ok(false);
                }
                if let Some((entry_id, sold_at, entry_type, description, amount)) =
                    &ledger_for_closure
                {
                    tx.execute(
                        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        rusqlite::params![
                            entry_id,
                            sold_at,
                            entry_type,
                            description,
                            amount,
                            INVENTORY_SALE_TAG
                        ],
                    )?;
                    daily_rollup::refresh_days(&tx, [sold_at.as_str()])?;
                }
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if !updated {
            return Ok(None);
        }
        let ledger_entry = ledger_write.map(
            |(entry_id, sold_at, entry_type, description, amount)| LedgerRow {
                id: entry_id,
                date: sold_at,
                kind: entry_type.to_string(),
                description,
                amount,
                tag: INVENTORY_SALE_TAG.to_string(),
            },
        );
        Ok(Some(InventorySale {
            ledger_entry,
            sold_item,
        }))
    }
}

#[cfg(test)]
impl AnalyticsService {
    /// The database handle, for tests that drive the synchronous core
    /// (the rollup heal) directly, or seed/read state through it.
    fn db(&self) -> &Db {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eo_wire::normalizer::to_wire_json;
    use serde_json::{json, Value};

    /// The wire shape of a typed value, for byte-shape assertions.
    fn to_json<T: Serialize>(value: T) -> Value {
        serde_json::to_value(value).expect("analytics value serialises")
    }

    /// A real database (the synchronous core) over a temp file. A temp file
    /// (not `:memory:`) is required: the synchronous core opens its own
    /// connections, which an in-memory database cannot share. The reads
    /// under test run on the core; the seeds commit through the same handle
    /// and are visible to the core's readers under WAL.
    async fn open_env() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// An [`AnalyticsService`] over a real temp-file database, its clock frozen
    /// so the UTC default date is deterministic
    /// (2026-06-01).
    async fn write_service() -> (tempfile::TempDir, AnalyticsService) {
        use crate::clock::MockClock;
        let (dir, db) = open_env().await;
        let naive =
            chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S")
                .unwrap();
        (
            dir,
            AnalyticsService::new(db, Arc::new(MockClock::new(Some(naive), 0.0))),
        )
    }

    /// 2026-06-05T00:00:00Z: heals the rollup watermark past the
    /// backdated days these tests write to, so the write hooks are
    /// observable.
    async fn heal_to_june_fifth(db: &Db) {
        db.with_writer(move |conn| daily_rollup::heal_rollups(conn, 1_780_617_600.0))
            .await
            .unwrap();
    }

    async fn ledger_rollup(db: &Db, day: &str, tag: &str) -> Option<(String, f64)> {
        use rusqlite::OptionalExtension;
        let day = day.to_string();
        let tag = tag.to_string();
        db.with_reader(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT entry_type, amount FROM daily_ledger_rollups WHERE day = ?1 AND tag = ?2",
                    rusqlite::params![day, tag],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
                )
                .optional()?)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn ledger_create_and_delete_reland_their_days_rollups() {
        let (_dir, service) = write_service().await;
        heal_to_june_fifth(service.db()).await;

        // A backdated create lands its day's rollup with the insert.
        let body = to_json(
            service
                .create_ledger_entry("2026-06-02", "expense", "ammo restock", 12.5, "manual")
                .await
                .unwrap(),
        );
        assert_eq!(
            ledger_rollup(service.db(), "2026-06-02", "manual").await,
            Some(("expense".into(), 12.5))
        );

        // The delete relands it empty; a missing id reports not-found.
        let id = body["id"].as_str().unwrap().to_string();
        assert!(service.delete_ledger_entry(&id).await.unwrap());
        assert_eq!(
            ledger_rollup(service.db(), "2026-06-02", "manual").await,
            None
        );
        assert!(!service.delete_ledger_entry("missing").await.unwrap());
    }

    #[tokio::test]
    async fn inventory_sale_relands_the_sold_days_rollup() {
        let (_dir, service) = write_service().await;
        heal_to_june_fifth(service.db()).await;
        service
            .db()
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO inventory_items (id, name, tt_value, markup_paid, notes, acquired_at) \
                     VALUES ('i1', 'Gun', 10.0, 2.0, NULL, '2026-05-01')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // Sold at a backdated date for an 8.0 markup delta.
        service
            .sell_inventory_item("i1", 20.0, None, Some("2026-06-02"))
            .await
            .unwrap()
            .expect("the item exists");
        assert_eq!(
            ledger_rollup(service.db(), "2026-06-02", INVENTORY_SALE_TAG).await,
            Some(("markup".into(), 8.0))
        );
    }

    #[tokio::test]
    async fn empty_overview_emits_the_engine_typed_zeros() {
        let (_dir, db) = open_env().await;
        let value = to_json(overview_impl(&db, 1_800_000_000.0, "all").await.unwrap());
        // cycledBreakdown is an `Any` field: empty COALESCE sums leave the
        // integer zero on the wire, while the float-declared aggregates coerce.
        assert_eq!(
            to_wire_json(&value),
            "{\"totalReturnRate\":0.0,\"trend\":\"stable\",\"returnsBreakdown\":{\"lootTt\":0.0,\
             \"questItemTt\":0.0,\"pes\":0.0,\"codexPes\":0.0,\"questPes\":0.0,\"ledger\":{}},\"lossesBreakdown\":\
             {\"trackingCost\":0.0,\"cycledBreakdown\":{\"weapon\":0,\"healing\":0,\"enhancer\":0,\
             \"armour\":0,\"dangling\":0,\"harvest\":0,\"consumables\":0},\"ledger\":{}},\"totalGains\":0.0,\"totalLosses\":0.0,\
             \"timeline\":[],\"monthlyBreakdown\":[]}"
        );
    }

    #[tokio::test]
    async fn empty_hunting_emits_two_empty_tables() {
        let (_dir, db) = open_env().await;
        let value = to_json(hunting_impl(&db).await.unwrap());
        assert_eq!(
            to_wire_json(&value),
            "{\"mobComparisons\":[],\"nameComparisons\":[]}"
        );
    }

    #[tokio::test]
    async fn empty_harvest_emits_an_empty_tier_table() {
        let (_dir, db) = open_env().await;
        let value = to_json(harvest_impl(&db, None).await.unwrap());
        assert_eq!(to_wire_json(&value), "{\"tierComparisons\":[]}");
    }

    /// Tier is the source activity; tool is a nested strategy. One tool may
    /// span tiers, several tools may share a tier, and tool gaps remain in
    /// the conserved totals.
    #[tokio::test]
    async fn harvest_groups_by_tier_without_dropping_toolless_swings() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at) VALUES('hs',1000.0,4600.0)",
                [],
            )?;
            type HarvestFixtureRow<'a> = (&'a str, &'a str, Option<&'a str>, i64, f64, f64);
            let rows: [HarvestFixtureRow<'_>; 6] = [
                ("h1", "long", Some("PH-3"), 1, 0.1, 0.3),
                ("h2", "long", Some("PH-3"), 0, 0.1, 0.0),
                ("h3", "huge", Some("PH-3"), 1, 0.1, 0.06),
                ("h4", "huge", Some("PH-4"), 1, 0.2, 0.1),
                ("h5", "huge", None, 1, 0.05, 0.04),
                ("h6", "short", Some(""), 1, 0.03, 0.02),
            ];
            for (id, tier, tool, success, cost, loot) in rows {
                conn.execute(
                    "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                     yield_tier,cost_ped,loot_total_ped) \
                     VALUES(?1,'hs',1000.0,?2,?3,?4,?5,?6)",
                    rusqlite::params![id, success, tool, tier, cost, loot],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let result = harvest_impl(&db, None).await.unwrap();
        assert_eq!(
            result
                .tier_comparisons
                .iter()
                .map(|tier| tier.swings)
                .sum::<i64>(),
            6
        );
        assert!(
            (result
                .tier_comparisons
                .iter()
                .map(|tier| tier.cycled)
                .sum::<f64>()
                - 0.58)
                .abs()
                < 1e-9
        );
        assert!(
            (result
                .tier_comparisons
                .iter()
                .map(|tier| tier.returns)
                .sum::<f64>()
                - 0.52)
                .abs()
                < 1e-9
        );
        let v = to_json(&result);
        let tiers = v["tierComparisons"].as_array().unwrap();
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0]["yieldTier"], json!("short"));
        assert_eq!(tiers[1]["yieldTier"], json!("long"));
        assert_eq!(tiers[1]["swings"], json!(2));
        assert_eq!(tiers[1]["cycled"], json!(0.2));
        assert_eq!(tiers[1]["returns"], json!(0.3));
        assert_eq!(tiers[2]["yieldTier"], json!("huge"));
        assert_eq!(tiers[2]["swings"], json!(3));
        assert_eq!(tiers[2]["cycled"], json!(0.35));
        assert_eq!(tiers[2]["returns"], json!(0.2));
        // The tool is not a reported dimension: several tools feed the huge
        // tier here, and the tier reports one set of figures for all of them.
        assert!(tiers[2].get("toolComparisons").is_none());
    }

    #[tokio::test]
    async fn harvest_period_scopes_totals_and_loot_composition_to_the_same_events() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at) VALUES('hs',100.0,2000.0)",
                [],
            )?;
            for (id, timestamp, tool, cost, loot) in [
                ("old", 100.0, "Old Tool", 1.0, 2.0),
                ("recent", 1000.0, "Recent Tool", 3.0, 6.0),
            ] {
                conn.execute(
                    "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                     yield_tier,cost_ped,loot_total_ped) \
                     VALUES(?1,'hs',?2,1,?3,'long',?4,?5)",
                    rusqlite::params![id, timestamp, tool, cost, loot],
                )?;
                conn.execute(
                    "INSERT INTO harvest_loot_items(harvest_id,item_name,quantity,value_ped) \
                     VALUES(?1,?2,1,?3)",
                    rusqlite::params![id, format!("{tool} Loot"), loot],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let period = harvest_impl(&db, Some(500.0)).await.unwrap();
        assert_eq!(period.tier_comparisons.len(), 1);
        let row = &period.tier_comparisons[0];
        assert_eq!(row.yield_tier, HarvestYieldTier::Long);
        assert_eq!(row.cycled, 3.0);
        assert_eq!(row.returns, 6.0);
        assert_eq!(row.loot_items.len(), 1);
        assert_eq!(row.loot_items[0].item_name, "Recent Tool Loot");
        assert_eq!(row.loot_items[0].value_ped, 6.0);
    }

    /// Tier loot composition: active items only, grouped by yield tier and
    /// ordered TT-descending, with deactivated loot excluded.
    #[tokio::test]
    async fn harvest_composition_groups_active_loot_by_tier() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at) VALUES('hs',1000.0,4600.0)",
                [],
            )?;
            // Two swings on Axe A, one on Axe B.
            for (id, tier, tool) in [
                ("h1", "huge", "Axe A"),
                ("h2", "huge", "Axe A"),
                ("h3", "short", "Axe B"),
            ] {
                conn.execute(
                    "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                     yield_tier,cost_ped,loot_total_ped) \
                     VALUES(?1,'hs',1000.0,1,?2,?3,0.1,1.0)",
                    rusqlite::params![id, tool, tier],
                )?;
            }
            // Loot: Axe A pulled Long Moonleaf Board (higher TT) and Wood
            // Shavings; one deactivated Wood Shavings row is excluded. Axe B
            // pulled Short Moonleaf Board only.
            let loot: [(&str, &str, i64, f64, Option<f64>); 4] = [
                ("h1", "Long Moonleaf Board", 2, 0.8, None),
                ("h1", "Wood Shavings", 5, 0.2, None),
                ("h2", "Wood Shavings", 3, 0.15, Some(2000.0)),
                ("h3", "Short Moonleaf Board", 4, 0.5, None),
            ];
            for (hid, item, qty, val, deact) in loot {
                conn.execute(
                    "INSERT INTO harvest_loot_items(harvest_id,item_name,quantity,value_ped,deactivated_at) \
                     VALUES(?1,?2,?3,?4,?5)",
                    rusqlite::params![hid, item, qty, val, deact],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let result = harvest_impl(&db, None).await.unwrap();
        let v = to_json(&result);
        let tiers = v["tierComparisons"].as_array().unwrap();
        // The huge tier: two active items, TT-desc (Long Moonleaf Board
        // first); the deactivated Wood Shavings row is excluded from its total.
        let short = &tiers[0];
        assert_eq!(short["yieldTier"], json!("short"));
        let huge = &tiers[1];
        assert_eq!(huge["yieldTier"], json!("huge"));
        let huge_items = huge["lootItems"].as_array().unwrap();
        assert_eq!(huge_items.len(), 2);
        assert_eq!(huge_items[0]["itemName"], json!("Long Moonleaf Board"));
        assert_eq!(huge_items[0]["quantity"], json!(2));
        assert_eq!(huge_items[0]["valuePed"], json!(0.8));
        assert_eq!(huge_items[1]["itemName"], json!("Wood Shavings"));
        assert_eq!(huge_items[1]["quantity"], json!(5));
        assert_eq!(huge_items[1]["valuePed"], json!(0.2));
        // The short tier, fed by the other tool, is its own row.
        let short_items = short["lootItems"].as_array().unwrap();
        assert_eq!(short_items.len(), 1);
        assert_eq!(short_items[0]["itemName"], json!("Short Moonleaf Board"));
    }

    /// Seed the representative scenario the live probe grounded, with the
    /// window relative to a fixed `now`, and assert the computed aggregates,
    /// the trend, dominance, and the filters.
    async fn seed_scenario(db: &Db, now: f64) {
        let day = 86400.0;
        let recent = now - 11.0 * day; // inside the 30d window
        let prior = now - 37.0 * day; // inside the 30-60d window
        let recent_iso = epoch_to_iso(recent);
        let prior_iso = epoch_to_iso(prior);
        db.with_writer(move |conn| {
            // sessions
            for (id, start, armour, heal, dangling) in [
                ("sess-a", recent, 1.0, 2.0, 0.5),
                ("sess-b", prior, 0.5, 1.0, 0.0),
                ("sess-z", recent, 0.0, 0.0, 0.0), // zero-kill, zero-cost: filtered from activity
            ] {
                conn.execute(
                    "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost,session_name) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    rusqlite::params![
                        id,
                        start,
                        start + 3600.0,
                        armour,
                        heal,
                        dangling,
                        // The designated axis is a session facet now; only
                        // sess-b carries a name, exactly as only it carried
                        // a tag before.
                        (id == "sess-b").then_some("Thing"),
                    ],
                )
                .expect("seed");
            }
            for i in 0..5 {
                let kid = format!("k-a-{i}");
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    rusqlite::params![kid, "sess-a", "Atrox", "Atrox", "Young", recent + i as f64, 0.1, 10.0],
                )
                .expect("seed");
                conn.execute(
                    "INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?1,?2,?3,?4)",
                    rusqlite::params![kid, "Opalo", 50_i64, 0.011],
                )
                .expect("seed");
            }
            for i in 0..3 {
                let kid = format!("k-b-{i}");
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,?2,?3,NULL,NULL,?4,?5,?6)",
                    rusqlite::params![kid, "sess-b", "Thing", prior + i as f64, 0.0, 5.0],
                )
                .expect("seed");
                conn.execute(
                    "INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?1,?2,?3,?4)",
                    rusqlite::params![kid, "Opalo", 30_i64, 0.01],
                )
                .expect("seed");
            }
            conn.execute(
                "INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value) \
                 VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params!["sess-a", recent, "Laser Weaponry Technology", 1.0, 3.0],
            )
            .expect("seed");
            conn.execute(
                "INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value) \
                 VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params!["sess-b", prior, "Laser Weaponry Technology", 1.0, 1.0],
            )
            .expect("seed");
            conn.execute(
                "INSERT INTO codex_claims(species_name,rank,skill_name,claimed_at,ped_value) \
                 VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params!["Atrox", 1_i64, "Rifle", recent, 7.0],
            )
            .expect("seed");
            conn.execute(
                "INSERT INTO quest_claims(quest_name,claimed_at,ped_value) VALUES(?1,?2,?3)",
                rusqlite::params!["A Quest", recent, 4.0],
            )
            .expect("seed");
            // ledger: a recent markup and a prior expense, dated by the ISO form.
            conn.execute(
                "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params!["led-1", recent_iso, "markup", "Sold hides", 12.5, "loot_sale"],
            )
            .expect("seed");
            conn.execute(
                "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params!["led-2", prior_iso, "expense", "Deposit", 8.0, "deposit"],
            )
            .expect("seed");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn seeded_overview_aggregates_match() {
        let now = 1_800_000_000.0;
        let (_dir, db) = open_env().await;
        seed_scenario(&db, now).await;
        let v = to_json(overview_impl(&db, now, "all").await.unwrap());
        assert_eq!(v["returnsBreakdown"]["lootTt"], json!(65.0));
        assert_eq!(v["returnsBreakdown"]["pes"], json!(4.0));
        assert_eq!(v["returnsBreakdown"]["codexPes"], json!(7.0));
        assert_eq!(v["returnsBreakdown"]["ledger"]["loot_sale"], json!(12.5));
        assert_eq!(v["lossesBreakdown"]["trackingCost"], json!(9.15));
        assert_eq!(
            v["lossesBreakdown"]["cycledBreakdown"]["weapon"],
            json!(3.65)
        );
        assert_eq!(
            v["lossesBreakdown"]["cycledBreakdown"]["armour"],
            json!(1.5)
        );
        assert_eq!(v["lossesBreakdown"]["ledger"]["deposit"], json!(8.0));
        // totalGains = loot 65 + markup 12.5; totalLosses = cost 9.15 + expense 8.0.
        assert_eq!(v["totalGains"], json!(77.5));
        assert_eq!(v["totalLosses"], json!(17.15));
        assert_eq!(v["totalReturnRate"], json!(4.519));
        // timeline points carry the day bucket; monthly points the month
        // (the facade labels them "date" / "month").
        assert!(v["timeline"][0]["bucket"]
            .as_str()
            .is_some_and(|b| b.len() == 10));
        assert!(v["monthlyBreakdown"][0]["bucket"]
            .as_str()
            .is_some_and(|b| b.len() == 7));
        // trend: recent-30d rate exceeds prior-30d rate beyond the 2% band.
        assert_eq!(v["trend"], json!("improving"));
        // period filter: 30d keeps only the recent window (markup in, expense out).
        let v30 = to_json(overview_impl(&db, now, "30d").await.unwrap());
        assert_eq!(v30["returnsBreakdown"]["lootTt"], json!(50.0));
        assert_eq!(v30["returnsBreakdown"]["ledger"]["loot_sale"], json!(12.5));
        assert_eq!(v30["lossesBreakdown"]["ledger"], json!({}));
        assert_eq!(v30["timeline"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn seeded_hunting_dominance_and_filters() {
        let now = 1_800_000_000.0;
        let (_dir, db) = open_env().await;
        seed_scenario(&db, now).await;
        let v = to_json(hunting_impl(&db).await.unwrap());
        // sess-z (zero kills) filtered out; sess-a carries a dominant mob,
        // sess-b a session name.
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["name"], json!("Atrox"));
        assert_eq!(mobs[0]["kills"], json!(5));
        assert_eq!(mobs[0]["hours"], json!(1.0)); // 3600s / 3600
        assert_eq!(mobs[0]["cycled"], json!(6.75));
        // pesPer100Ped = (skill 3.0 / cycled 6.75) * 100; lootRate = loot 50 / cycled.
        assert_eq!(mobs[0]["pesPer100Ped"], json!(44.44));
        assert_eq!(mobs[0]["lootRate"], json!(7.4074));
        let names = v["nameComparisons"].as_array().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0]["name"], json!("Thing"));
        // The designated axis counts the session's whole kill stream, not a
        // dominant slice of it: naming a session IS the declaration.
        assert_eq!(names[0]["kills"], json!(3));
        assert_eq!(names[0]["cycled"], json!(2.4));
        assert_eq!(names[0]["pesPer100Ped"], json!(41.67));
        assert_eq!(names[0]["lootRate"], json!(6.25));
    }

    /// The activity filter drops a session failing ANY of the three guards
    /// (duration > 0, cycled > 0, kills > 0); `||` not `&&`. Three sessions,
    /// each dominated by its own mob, each failing exactly one guard except
    /// the keeper: only the keeper's mob survives.
    #[tokio::test]
    async fn activity_filter_drops_a_session_failing_any_single_guard() {
        let (_dir, db) = open_env().await;
        // keeper: kills, duration, cost all positive.
        seed_filter_session(&db, "keep", "Keeper", 1000.0, 1000.0 + 3600.0, 5.0, 2).await;
        // zero cost -> cycled 0 -> dropped by the cycled guard alone.
        seed_filter_session(&db, "zcost", "Zerocost", 1000.0, 1000.0 + 3600.0, 0.0, 2).await;
        // zero duration (start == end) -> dropped by the duration guard alone.
        seed_filter_session(&db, "zdur", "Zerodur", 1000.0, 1000.0, 5.0, 2).await;
        let v = to_json(hunting_impl(&db).await.unwrap());
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1, "only the keeper survives the OR filter");
        assert_eq!(mobs[0]["name"], json!("Keeper"));
    }

    async fn seed_filter_session(
        db: &Db,
        id: &str,
        mob: &str,
        start: f64,
        end: f64,
        armour: f64,
        kills: i64,
    ) {
        let id = id.to_string();
        let mob = mob.to_string();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
                 VALUES(?1,?2,?3,?4,0,0)",
                rusqlite::params![id, start, end, armour],
            )
            .expect("seed");
            for i in 0..kills {
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    rusqlite::params![format!("{id}-k{i}"), id, mob, "Spec", "Young", start + i as f64, 0.0, 1.0],
                )
                .expect("seed");
            }
            Ok(())
        })
        .await
        .unwrap();
    }

    /// Seed one session (cost via armour) and `kills` loot rows at `ts`, so a
    /// window's rate is loot_total / armour_cost.
    async fn seed_rate(db: &Db, id: &str, ts: f64, cost: f64, kills: i64, loot: f64) {
        let id = id.to_string();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
                 VALUES(?1,?2,?3,?4,0,0)",
                rusqlite::params![id, ts, ts + 3600.0, cost],
            )
            .expect("seed");
            for i in 0..kills {
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,?2,?3,?4,0,?5)",
                    rusqlite::params![format!("{id}-k{i}"), id, "M", ts + i as f64, loot],
                )
                .expect("seed");
            }
            Ok(())
        })
        .await
        .unwrap();
    }

    /// The trend compares the recent-30d rate against the prior-30d rate with
    /// a +/-2% band, guarded by both rates being positive.
    #[tokio::test]
    async fn overview_trend_bands() {
        let now = 1_800_000_000.0;
        let day = 86400.0;
        let trend = |v: OverviewData| json!(v.trend);

        // declining: recent rate 1.0 (10/10) below prior 2.0 (20/10) * 0.98.
        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 10.0, 1, 10.0).await;
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&db, now, "all").await.unwrap()),
            json!("declining")
        );

        // improving: recent 2.0 above prior 1.0 * 1.02.
        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 10.0, 1, 20.0).await;
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 10.0).await;
        assert_eq!(
            trend(overview_impl(&db, now, "all").await.unwrap()),
            json!("improving")
        );

        // stable: recent equals prior, inside the band.
        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 10.0, 1, 10.0).await;
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 10.0).await;
        assert_eq!(
            trend(overview_impl(&db, now, "all").await.unwrap()),
            json!("stable")
        );

        // zero recent rate: the positivity guard short-circuits to stable
        // (a mutated guard would fall through into the banding and declare a
        // direction).
        let (_dir, db) = open_env().await;
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&db, now, "all").await.unwrap()),
            json!("stable")
        );

        // zero prior rate: the other half of the guard.
        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&db, now, "all").await.unwrap()),
            json!("stable")
        );
    }

    /// Dominance needs the top group at or above 60% of known kills, and the
    /// species/maturity presence decides mob vs tag.
    #[tokio::test]
    async fn activity_dominance_threshold_and_tag_split() {
        // Non-dominant: three distinct mobs, one kill each (33% each, below
        // the 0.6 floor) -> no dominant element, no comparison rows.
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
                 VALUES('nd',1000.0,4600.0,5.0,0,0)",
                [],
            )?;
            for (i, mob) in ["Alpha", "Bravo", "Charlie"].iter().enumerate() {
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,'nd',?2,'Spec','Young',?3,0,1.0)",
                    rusqlite::params![format!("nd-{i}"), mob, 1000.0 + i as f64],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let v = to_json(hunting_impl(&db).await.unwrap());
        assert_eq!(v["mobComparisons"].as_array().unwrap().len(), 0);
        // An unnamed session reaches the designated axis no more than the
        // mob axis: absent is absent, not an empty-string bucket.
        assert_eq!(v["nameComparisons"].as_array().unwrap().len(), 0);

        // A species with no maturity is still a mob: maturity is a finer
        // breakdown, not part of the identity test.
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
                 VALUES('as',1000.0,4600.0,5.0,0,0)",
                [],
            )?;
            for i in 0..2 {
                conn.execute(
                    "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                     VALUES(?1,'as','Foo','Bar','',?2,0,1.0)",
                    rusqlite::params![format!("as-{i}"), 1000.0 + i as f64],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let v = to_json(hunting_impl(&db).await.unwrap());
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["name"], json!("Foo"));
        assert_eq!(v["nameComparisons"].as_array().unwrap().len(), 0);
    }

    /// A kill referencing a session that does not exist (representable with
    /// foreign keys off, as the app runs) is not counted: it belongs to no
    /// session, so it never enters any session's aggregate.
    #[tokio::test]
    async fn activity_ignores_a_kill_for_a_missing_session() {
        let (_dir, db) = open_env().await;
        // A valid completed session with one dominant-mob kill.
        seed_filter_session(&db, "ok", "Real", 1000.0, 1000.0 + 3600.0, 5.0, 2).await;
        // An orphan kill whose session_id matches no tracking_sessions row.
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES('orphan','ghost-session','Ghost','Spec','Young',1.0,0,9.0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        // Only the real session's mob is compared; the orphan is ignored.
        let v = to_json(hunting_impl(&db).await.unwrap());
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["name"], json!("Real"));
    }

    #[test]
    fn period_epoch_maps_named_windows_only() {
        let now = 1_000_000.0;
        assert_eq!(period_epoch("all", now), None);
        assert_eq!(period_epoch("bogus", now), None);
        assert_eq!(period_epoch("30d", now), Some(now - 30.0 * 86400.0));
        assert_eq!(period_epoch("90d", now), Some(now - 90.0 * 86400.0));
        assert_eq!(period_epoch("1y", now), Some(now - 365.0 * 86400.0));
    }

    #[test]
    fn listing_expiry_runs_from_the_instant_not_the_day() {
        // 2026-08-11T18:20:00Z. Seven days later is the same clock time,
        // which is what the auction house does and what a date alone
        // cannot express.
        let listed = 1_786_472_400.0;
        assert_eq!(
            listing_expiry(Some(listed), Some(7)).as_deref(),
            Some("2026-08-18T18:20:00+00:00")
        );
        // An unrecorded duration never invents a deadline, so a listing made
        // before durations were captured simply never nudges.
        assert_eq!(listing_expiry(Some(listed), None), None);
        // Nor does a listing known only by its date: midnight would be a
        // guess, and hours out in either direction.
        assert_eq!(listing_expiry(None, Some(7)), None);
    }

    #[test]
    fn listing_moment_keeps_the_clock_only_when_it_was_given() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 8, 11)
            .unwrap()
            .and_hms_opt(18, 20, 0)
            .unwrap();

        // Nothing supplied means now, the ordinary case.
        let (date, instant) = listing_moment(None, now);
        assert_eq!(date, "2026-08-11");
        assert_eq!(instant, Some(naive_to_epoch(now)));

        // A full stamp gives both halves.
        let (date, instant) = listing_moment(Some("2026-07-02T09:05"), now);
        assert_eq!(date, "2026-07-02");
        assert!(instant.is_some());

        // A bare date gives the ledger its key and leaves the clock unknown.
        let (date, instant) = listing_moment(Some("2026-07-02"), now);
        assert_eq!(date, "2026-07-02");
        assert_eq!(instant, None);
    }

    #[test]
    fn sql_number_float_form_coerces_integers_only() {
        assert_eq!(SqlNumber::Int(0).as_f64(), 0.0);
        assert_eq!(SqlNumber::Int(3).as_f64(), 3.0);
        assert_eq!(SqlNumber::Float(1.5).as_f64(), 1.5);
    }

    #[test]
    fn sql_number_rounding_preserves_integers_and_banker_rounds_floats() {
        assert_eq!(SqlNumber::Int(0).rounded(2), SqlNumber::Int(0)); // int stays int
        assert_eq!(SqlNumber::Float(1.005).rounded(2), SqlNumber::Float(1.0)); // half-even
        assert_eq!(SqlNumber::Float(2.675).rounded(2), SqlNumber::Float(2.67));
    }

    #[test]
    fn sql_number_sum_is_integral_only_when_both_are() {
        assert_eq!(SqlNumber::Int(2).sum(SqlNumber::Int(3)), SqlNumber::Int(5));
        assert_eq!(
            SqlNumber::Int(2).sum(SqlNumber::Float(0.5)),
            SqlNumber::Float(2.5)
        );
        // The engine typing serialises untagged: ints as ints.
        assert_eq!(to_json(SqlNumber::Int(0)), json!(0));
        assert_eq!(to_json(SqlNumber::Float(0.0)), json!(0.0));
    }

    // ── Hermetic write-handler tests (the mutation campaign's kills) ──

    /// Create then list round-trips for the ledger: the create echoes the
    /// input plus a generated id, and the list reads it back.
    #[tokio::test]
    async fn ledger_create_and_list_round_trip() {
        let (_dir, service) = write_service().await;
        let body = to_json(
            service
                .create_ledger_entry("2026-05-01", "expense", "Ammo", 12.5, "ammo")
                .await
                .unwrap(),
        );
        assert_eq!(body["date"], json!("2026-05-01"));
        assert_eq!(body["type"], json!("expense"));
        assert_eq!(body["amount"], json!(12.5));
        assert_eq!(body["tag"], json!("ammo"));
        assert!(body["id"].as_str().is_some(), "create generates an id");

        let page = service.list_ledger(None, None).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].description, "Ammo");
        assert_eq!(json!(page.entries[0].id), body["id"]);
    }

    /// Keyset pagination walks the whole ledger newest-first, one bounded
    /// page at a time, following the `next_cursor` with no overlap and no
    /// gaps.
    #[tokio::test]
    async fn ledger_list_walks_every_entry_by_keyset_cursor() {
        let (_dir, service) = write_service().await;
        for day in ["01", "02", "03", "04", "05"] {
            service
                .create_ledger_entry(
                    &format!("2026-05-{day}"),
                    "expense",
                    &format!("e{day}"),
                    1.0,
                    "t",
                )
                .await
                .unwrap();
        }

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        // A generous cap: three pages of two cover five entries; more than
        // this means the cursor is not converging.
        for _ in 0..10 {
            let page = service
                .list_ledger(cursor.as_deref(), Some(2))
                .await
                .unwrap();
            assert!(page.entries.len() <= 2, "the page is bounded by the limit");
            for row in &page.entries {
                seen.push(row.description.clone());
            }
            match page.next_cursor {
                Some(token) => cursor = Some(token),
                None => break,
            }
        }
        assert_eq!(
            seen,
            ["e05", "e04", "e03", "e02", "e01"],
            "every entry appears once, newest first, across pages"
        );
    }

    #[tokio::test]
    async fn ledger_list_rejects_a_malformed_cursor() {
        let (_dir, service) = write_service().await;
        assert!(matches!(
            service.list_ledger(Some("not a cursor!"), None).await,
            Err(AnalyticsError::InvalidCursor)
        ));
    }

    /// The preset type guard: only 'expense'/'markup' pass; anything else is
    /// [`AnalyticsError::InvalidPresetType`] and writes nothing.
    #[tokio::test]
    async fn preset_create_validates_type() {
        let (_dir, service) = write_service().await;
        for kind in ["expense", "markup"] {
            service
                .create_ledger_preset("P", kind, "d", 1.0, "t")
                .await
                .unwrap_or_else(|_| panic!("{kind} accepted"));
        }
        assert!(matches!(
            service
                .create_ledger_preset("Bad", "income", "d", 1.0, "t")
                .await,
            Err(AnalyticsError::InvalidPresetType)
        ));
        // Only the two valid presets were written.
        assert_eq!(service.list_ledger_presets().await.unwrap().len(), 2);
    }

    /// Seed one harvest session whose swings yield boards across two tiers,
    /// giving `Moonleaf Board` a 60/40 open position to allocate against.
    async fn seed_board_stock(service: &AnalyticsService) {
        service
            .db
            .with_writer(|conn| {
                for (id, tier, tool, quantity, tt) in [
                    ("hs1", "long", "PH-3", 60, 1.80),
                    ("hs2", "huge", "PH-3", 40, 1.20),
                ] {
                    conn.execute(
                        "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                         yield_tier,cost_ped,loot_total_ped) \
                         VALUES(?1,'sale-s',1000.0,1,?2,?3,0.1,?4)",
                        rusqlite::params![id, tool, tier, tt],
                    )?;
                    conn.execute(
                        "INSERT INTO harvest_loot_items(harvest_id,item_name,quantity,value_ped) \
                         VALUES(?1,'Moonleaf Board',?2,?3)",
                        rusqlite::params![id, quantity, tt],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
    }

    /// Every ledger description currently on file, for asserting that an undo
    /// took the money with it.
    async fn ledger_descriptions(service: &AnalyticsService) -> Vec<String> {
        service
            .list_ledger(None, Some(100))
            .await
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.description)
            .collect()
    }

    fn position(rows: &[StockPositionRow], item: &str) -> Option<StockPositionRow> {
        rows.iter().find(|row| row.item_name == item).cloned()
    }

    /// A recorded duration survives the round trip and reports the day the
    /// listing runs out, so the Listings surface can ask what became of it.
    /// A listing made without one keeps no deadline at all.
    #[tokio::test]
    async fn listing_duration_round_trips_and_reports_its_expiry() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let dated = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                10.0,
                2.0,
                None,
                0.5,
                Some("2026-07-20T14:30:00"),
                Some(7),
            )
            .await
            .unwrap();
        assert_eq!(dated.auction_days, Some(7));
        // Listed at 14:30 on the machine's own clock, so the deadline is
        // that same wall-clock moment seven days on, reported in UTC. The
        // expectation is worked out the same way rather than written down,
        // because a written-down UTC string would only be right in the
        // timezone it was written in and would fail everywhere else.
        let listed_naive = chrono::NaiveDate::from_ymd_opt(2026, 7, 20)
            .unwrap()
            .and_hms_opt(14, 30, 0)
            .unwrap();
        let expected = to_iso_utc(naive_to_epoch(listed_naive) + 7.0 * 86_400.0);
        assert_eq!(dated.expires_at.as_deref(), Some(expected.as_str()));

        let undated = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                10.0,
                2.0,
                None,
                0.5,
                Some("2026-07-20"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(undated.auction_days, None);
        assert_eq!(undated.expires_at, None);

        // The duration reads back from the database, not just from the
        // creation call's own return value.
        let listings = service
            .auction_listings(Profession::Harvesting)
            .await
            .unwrap();
        let stored = listings.iter().find(|row| row.id == dated.id).unwrap();
        assert_eq!(stored.expires_at.as_deref(), Some(expected.as_str()));
    }

    /// A duration is either absent or a real number of days; zero and
    /// negatives are refused rather than stored as a deadline in the past.
    #[tokio::test]
    async fn listing_rejects_a_non_positive_duration() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let error = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                10.0,
                2.0,
                None,
                0.5,
                Some("2026-07-20"),
                Some(0),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AnalyticsError::InvalidInput(_)));
    }

    /// Listing removes the stock immediately, because in game it has left the
    /// player's inventory, and spends the starting-bid fee immediately too.
    /// Nothing is realised: the auction has not closed.
    #[tokio::test]
    async fn listing_removes_stock_and_spends_the_fee_without_realising_markup() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let before = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        assert_eq!(position(&before, "Moonleaf Board").unwrap().quantity, 100.0);

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                50.0,
                2.0,
                Some(4.0),
                0.5,
                Some("2026-07-20"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(listing.status, "pending");
        assert!((listing.attributed_qty - 50.0).abs() < 1e-9);
        assert!(listing.unattributed_qty.abs() < 1e-9);
        assert_eq!(
            listing.activity_net_markup, None,
            "an open auction has no realised figure"
        );

        let after = position(
            &service
                .stock_positions(Profession::Harvesting)
                .await
                .unwrap(),
            "Moonleaf Board",
        )
        .unwrap();
        assert!((after.quantity - 50.0).abs() < 1e-9);
        assert!(
            (after.listed_quantity - 50.0).abs() < 1e-9,
            "listed stock is reported, not just absent"
        );

        // The fee is a real, dated ledger expense the moment it is charged.
        let ledger = service.list_ledger(None, None).await.unwrap();
        let fee = ledger
            .entries
            .iter()
            .find(|entry| entry.description == "Auction Fee: Moonleaf Board")
            .expect("the listing fee reached the ledger");
        assert_eq!(fee.kind, "expense");
        assert!((fee.amount - 0.5).abs() < 1e-9);
        assert_eq!(fee.date, "2026-07-20");

        // Nothing is attributed to any activity yet.
        assert!(service.realised_markup_by_tier().await.unwrap().is_empty());
    }

    /// A listed quantity is drawn from every tier in proportion to what that
    /// tier still holds, never by picking source units or by FIFO.
    #[tokio::test]
    async fn listing_allocates_across_tiers_by_open_composition() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                50.0,
                2.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();

        let split: Vec<(String, f64)> = service
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT yield_tier, -quantity FROM stock_movements \
                     WHERE ref_id = ? AND movement_kind = 'listing' ORDER BY yield_tier",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![listing.id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap();

        // 60/40 open, so 50 listed splits 30/20.
        assert_eq!(split.len(), 2);
        let huge = split.iter().find(|(tier, _)| tier == "huge").unwrap().1;
        let long = split.iter().find(|(tier, _)| tier == "long").unwrap().1;
        assert!((long - 30.0).abs() < 1e-9, "long tier got {long}");
        assert!((huge - 20.0).abs() < 1e-9, "huge tier got {huge}");
    }

    /// Confirming a sale realises markup, writes only the money that was not
    /// already booked as loot TT, and divides the activity's share across the
    /// tiers that supplied the listing.
    #[tokio::test]
    async fn confirming_a_sale_realises_markup_and_attributes_it_by_tier() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                50.0,
                2.0,
                None,
                0.5,
                Some("2026-07-20"),
                None,
            )
            .await
            .unwrap();
        // 50 boards at 0.03 TT each.
        assert!((listing.tt_value - 1.5).abs() < 1e-9);

        let sold = service
            .confirm_auction_listing(&listing.id, 5.0, 0.2, Some("2026-07-22"))
            .await
            .unwrap()
            .expect("the pending listing resolved");
        assert_eq!(sold.status, "sold");
        // Gross 5.00 less 1.50 TT is 3.50, then less 0.70 of fees.
        assert!((sold.gross_markup.unwrap() - 3.5).abs() < 1e-9);
        assert!((sold.activity_net_markup.unwrap() - 2.8).abs() < 1e-9);

        // The ledger gains the uplift only: the TT counted as loot already.
        let ledger = service.list_ledger(None, None).await.unwrap();
        let sale = ledger
            .entries
            .iter()
            .find(|entry| entry.description == "Auction Sale: Moonleaf Board")
            .expect("the sale reached the ledger");
        assert_eq!(sale.kind, "markup");
        assert!((sale.amount - 3.5).abs() < 1e-9);
        assert_eq!(sale.date, "2026-07-22");

        // Both fees are ledger expenses, dated when each was charged.
        let fees = ledger
            .entries
            .iter()
            .filter(|entry| entry.description == "Auction Fee: Moonleaf Board")
            .count();
        assert_eq!(fees, 2, "listing fee and point-of-sale fee");

        // The activity's share divides 30/20 across the contributing tiers.
        let realised = service.realised_markup_by_tier().await.unwrap();
        let mut by_tier: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
        for row in &realised {
            *by_tier.entry(row.yield_tier.as_str()).or_insert(0.0) += row.net_markup;
        }
        assert!((by_tier["long"] - 2.8 * 0.6).abs() < 1e-9);
        assert!((by_tier["huge"] - 2.8 * 0.4).abs() < 1e-9);
        let total: f64 = realised.iter().map(|row| row.net_markup).sum();
        assert!(
            (total - 2.8).abs() < 1e-9,
            "the tier split sums to the whole"
        );
    }

    /// Stock produced by several tools inside one tier credits that tier
    /// once, in full. The tool is recorded on the movement rows but is not an
    /// axis anything reports on, so it must not split or duplicate the figure.
    #[tokio::test]
    async fn a_sale_credits_its_tier_once_across_every_tool_that_fed_it() {
        let (_dir, service) = write_service().await;
        service
            .db
            .with_writer(|conn| {
                // One tier, two tools: 75 units from PH-3 and 25 from PH-4.
                for (id, tool, quantity, tt) in [("t1", "PH-3", 75, 2.25), ("t2", "PH-4", 25, 0.75)]
                {
                    conn.execute(
                        "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                         yield_tier,cost_ped,loot_total_ped) \
                         VALUES(?1,'tool-s',1000.0,1,?2,'long',0.1,?3)",
                        rusqlite::params![id, tool, tt],
                    )?;
                    conn.execute(
                        "INSERT INTO harvest_loot_items(harvest_id,item_name,quantity,value_ped) \
                         VALUES(?1,'Moonleaf Board',?2,?3)",
                        rusqlite::params![id, quantity, tt],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                100.0,
                3.0,
                None,
                0.0,
                None,
                None,
            )
            .await
            .unwrap();
        let sold = service
            .confirm_auction_listing(&listing.id, 5.0, 0.0, None)
            .await
            .unwrap()
            .unwrap();
        // 5.00 on 3.00 of TT with no fees.
        assert!((sold.activity_net_markup.unwrap() - 2.0).abs() < 1e-9);

        let realised = service.realised_markup_by_tier().await.unwrap();
        // One row for the one tier, carrying the whole 2.00.
        assert_eq!(realised.len(), 1);
        assert_eq!(realised[0].yield_tier, HarvestYieldTier::Long);
        assert!((realised[0].net_markup - 2.0).abs() < 1e-9);
    }

    /// An expired listing returns the stock intact, keeps the fee spent, and
    /// attributes nothing: failing to sell describes market execution, not
    /// the gameplay that produced the loot.
    #[tokio::test]
    async fn expiring_a_listing_returns_stock_and_attributes_nothing() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                50.0,
                2.0,
                None,
                0.5,
                Some("2026-07-20"),
                None,
            )
            .await
            .unwrap();
        let expired = service
            .expire_auction_listing(&listing.id, Some("2026-07-25"))
            .await
            .unwrap()
            .expect("the pending listing resolved");
        assert_eq!(expired.status, "expired");
        assert_eq!(expired.activity_net_markup, None);

        let after = position(
            &service
                .stock_positions(Profession::Harvesting)
                .await
                .unwrap(),
            "Moonleaf Board",
        )
        .unwrap();
        assert!(
            (after.quantity - 100.0).abs() < 1e-9,
            "the stock came back whole"
        );
        assert!(after.listed_quantity.abs() < 1e-9);

        // The fee stays spent, and no activity may claim the loss.
        let ledger = service.list_ledger(None, None).await.unwrap();
        assert!(ledger
            .entries
            .iter()
            .any(|entry| entry.description == "Auction Fee: Moonleaf Board"));
        assert!(service.realised_markup_by_tier().await.unwrap().is_empty());

        // The original allocation survives for audit rather than being erased.
        let listing_rows: i64 = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM stock_movements WHERE movement_kind = 'listing'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            listing_rows, 2,
            "the listing's rows stay; the return is new rows"
        );
    }

    /// A resolved listing cannot resolve again, in either direction.
    #[tokio::test]
    async fn a_resolved_listing_cannot_be_resolved_twice() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                10.0,
                1.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 2.0, 0.0, None)
            .await
            .unwrap()
            .expect("first confirmation lands");

        assert!(service
            .confirm_auction_listing(&listing.id, 9.0, 0.0, None)
            .await
            .unwrap()
            .is_none());
        assert!(service
            .expire_auction_listing(&listing.id, None)
            .await
            .unwrap()
            .is_none());
    }

    /// Selling more than is tracked keeps the tracked part attributed and
    /// leaves the rest explicitly unattributed, so the activity is never
    /// credited with output it did not produce.
    #[tokio::test]
    async fn selling_beyond_tracked_stock_attributes_only_the_tracked_part() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                150.0,
                3.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        assert!((listing.attributed_qty - 100.0).abs() < 1e-9);
        assert!((listing.unattributed_qty - 50.0).abs() < 1e-9);

        let sold = service
            .confirm_auction_listing(&listing.id, 9.0, 0.0, None)
            .await
            .unwrap()
            .unwrap();
        // Two thirds of the listing was tracked, so two thirds of the net
        // markup may be claimed.
        let expected = (9.0 - 4.5 - 0.5) * (3.0 / 4.5);
        assert!((sold.activity_net_markup.unwrap() - expected).abs() < 1e-9);

        // The ledger records markup over the WHOLE listing's TT. The untracked
        // units' TT was value the player already held, so booking it as a gain
        // would invent profit out of a position conversion.
        let ledger = service.list_ledger(None, None).await.unwrap();
        let sale = ledger
            .entries
            .iter()
            .find(|entry| entry.description == "Auction Sale: Moonleaf Board")
            .unwrap();
        assert!((sale.amount - (9.0 - 4.5)).abs() < 1e-9);
    }

    /// Recycling preserves TT exactly and carries the source's activity
    /// composition into the produced item, so selling the result still
    /// attributes back to the tiers that grew it.
    #[tokio::test]
    async fn conversion_preserves_tt_and_carries_provenance_forward() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        service
            .convert_stock(
                Profession::Harvesting,
                "Moonleaf Board",
                "Nanocube",
                50.0,
                Some("2026-07-21"),
            )
            .await
            .unwrap();

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let source = position(&rows, "Moonleaf Board").unwrap();
        let produced = position(&rows, "Nanocube").expect("the conversion created stock");
        assert!((source.quantity - 50.0).abs() < 1e-9);
        // 50 boards at 0.03 TT is 1.50 PED, preserved 1:1.
        assert!((source.tt_value - 1.5).abs() < 1e-9);
        assert!((produced.tt_value - 1.5).abs() < 1e-9);

        service
            .db()
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO stock_movements (item_name, movement_kind, ref_id, source_kind, \
                         quantity, tt_value, occurred_at, created_at) \
                     VALUES ('Universal Ammo', 'legacy_adjustment', NULL, 'unattributed', \
                         100.0, 1.0, '2026-07-21', 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // The central Inventory universe includes outputs from every
        // profession-scoped conversion and legacy movements, not only records
        // labelled inventory.
        let inventory = service
            .stock_positions(Profession::Inventory)
            .await
            .unwrap();
        assert!(position(&inventory, "Nanocube").is_some());
        assert!(position(&inventory, "Universal Ammo").is_none());

        // Selling the produced stock attributes back to the original tiers.
        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Nanocube",
                produced.quantity,
                2.0,
                None,
                0.0,
                None,
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 2.5, 0.0, None)
            .await
            .unwrap()
            .unwrap();

        let realised = service.realised_markup_by_tier().await.unwrap();
        let total: f64 = realised.iter().map(|row| row.net_markup).sum();
        assert_eq!(realised.len(), 2, "both source tiers still carry the sale");
        assert!(
            (total - 1.0).abs() < 1e-9,
            "a 2.50 sale on 1.50 TT with no fees"
        );
    }

    /// A position that has fully closed stays on the list at zero. The item
    /// is still one the player produces; a line that vanished on the last
    /// sale would read as an item that never existed.
    #[tokio::test]
    async fn a_fully_sold_position_stays_on_the_stock_list_at_zero() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                100.0,
                3.0,
                None,
                0.0,
                None,
                None,
            )
            .await
            .unwrap();
        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the line stays");
        assert!(board.quantity.abs() < 1e-9);
        assert!(board.tt_value.abs() < 1e-9);
    }

    /// A listing is one history entry across its whole life. Selling it
    /// changes that entry rather than adding a second one beside it.
    #[tokio::test]
    async fn a_sale_replaces_its_listing_in_history_rather_than_joining_it() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                100.0,
                4.0,
                None,
                0.5,
                Some("2026-07-20"),
                None,
            )
            .await
            .unwrap();

        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "pending");
        assert_eq!(history[0].occurred_at, "2026-07-20");
        assert!(!history[0].can_revert_sale, "nothing has been realised yet");
        assert!(history[0].can_delete);

        service
            .confirm_auction_listing(&listing.id, 4.0, 0.0, Some("2026-07-22"))
            .await
            .unwrap()
            .unwrap();

        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        assert_eq!(history.len(), 1, "still one entry, now in its sold state");
        assert_eq!(history[0].status, "sold");
        assert_eq!(
            history[0].occurred_at, "2026-07-22",
            "dated by its resolution"
        );
        assert!(history[0].can_revert_sale);
        // 4.00 fetched on 3.00 TT, less the 0.50 listing fee.
        assert!((history[0].net_markup.unwrap() - 0.5).abs() < 1e-9);
    }

    /// Undoing a sale leaves the listing open: the stock stays out, because it
    /// left at listing time and the listing is live again, and the money the
    /// sale wrote goes away with the recognition.
    #[tokio::test]
    async fn reverting_a_sale_reopens_the_listing_and_unwrites_its_money() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                100.0,
                4.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 4.5, 0.2, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!service.realised_markup_by_tier().await.unwrap().is_empty());

        let reverted = service
            .revert_auction_sale(&listing.id)
            .await
            .unwrap()
            .expect("the listing is still there");
        assert_eq!(reverted.status, "pending");
        assert_eq!(reverted.final_price, None);
        assert_eq!(reverted.sale_fee, None);
        assert_eq!(reverted.resolved_at, None);

        // The sale and its point-of-sale fee are gone; the listing fee stays,
        // because that was spent at listing time and the listing still stands.
        let ledger = ledger_descriptions(&service).await;
        assert!(!ledger.iter().any(|d| d == "Auction Sale: Moonleaf Board"));
        assert_eq!(
            ledger
                .iter()
                .filter(|d| *d == "Auction Fee: Moonleaf Board")
                .count(),
            1,
        );

        // Nothing is realised by an open listing.
        assert!(service.realised_markup_by_tier().await.unwrap().is_empty());
        // The stock is still out on the auction.
        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the line stays");
        assert!(board.quantity.abs() < 1e-9);
        assert!((board.listed_quantity - 100.0).abs() < 1e-9);

        // Re-confirming lands on the same figures it first did.
        let resold = service
            .confirm_auction_listing(&listing.id, 4.5, 0.2, None)
            .await
            .unwrap()
            .unwrap();
        assert!((resold.gross_markup.unwrap() - 1.5).abs() < 1e-9);
    }

    /// A sale can only be taken back while it is a sale.
    #[tokio::test]
    async fn reverting_reports_not_found_unless_the_listing_is_sold() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                100.0,
                4.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(service
            .revert_auction_sale(&listing.id)
            .await
            .unwrap()
            .is_none());
        assert!(service
            .revert_auction_sale("no-such-id")
            .await
            .unwrap()
            .is_none());
    }

    /// Deleting a listing returns the stock it took and removes every ledger
    /// row it wrote, leaving no trace of an entry that should not have been.
    #[tokio::test]
    async fn deleting_a_sold_listing_returns_the_stock_and_its_money() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                60.0,
                3.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 3.0, 0.1, None)
            .await
            .unwrap()
            .unwrap();

        assert!(service.undo_auction_listing(&listing.id).await.unwrap());

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the stock is back");
        assert!((board.quantity - 100.0).abs() < 1e-9, "all 100 held again");
        assert!(board.listed_quantity.abs() < 1e-9);

        let ledger = ledger_descriptions(&service).await;
        assert!(!ledger.iter().any(|d| d.contains("Moonleaf Board")));
        assert!(service
            .auction_listings(Profession::Harvesting)
            .await
            .unwrap()
            .is_empty());
        assert!(service.realised_markup_by_tier().await.unwrap().is_empty());

        // The entry stays as the record of a correction, with nothing left to
        // do to it. Only history sees it.
        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].undone);
        assert!(!history[0].can_delete);
        assert!(!history[0].can_revert_sale);
        assert_eq!(history[0].status, "sold", "it still says what it was");

        // Undoing it again is a not-found, not a second reversal.
        assert!(!service.undo_auction_listing(&listing.id).await.unwrap());
    }

    /// Deleting a listing that outran tracked stock takes the opening balance
    /// with it. The units it accounted for were only ever evidenced by the
    /// listing, so they cannot outlive it as stock the player never had.
    #[tokio::test]
    async fn deleting_a_listing_takes_its_opening_balance_with_it() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                150.0,
                5.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(service.undo_auction_listing(&listing.id).await.unwrap());

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the stock is back");
        assert!(
            (board.quantity - 100.0).abs() < 1e-9,
            "back to the 100 that were ever recorded, not 150: {}",
            board.quantity
        );
    }

    /// Deleting an expired listing is a no-op on the stock: it went out and
    /// came back, and both rows go together.
    #[tokio::test]
    async fn deleting_an_expired_listing_leaves_the_stock_where_it_was() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                40.0,
                2.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();
        service
            .expire_auction_listing(&listing.id, None)
            .await
            .unwrap()
            .unwrap();
        assert!(service.undo_auction_listing(&listing.id).await.unwrap());

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the stock is there");
        assert!((board.quantity - 100.0).abs() < 1e-9);
        // The fee an expired listing kept spent goes with the listing.
        assert!(!ledger_descriptions(&service)
            .await
            .iter()
            .any(|d| d.contains("Moonleaf Board")));
    }

    /// Provenance survives the refiner. Shavings drawn 20/30/50 from the three
    /// tiers become Nanocubes holding that same composition, and selling those
    /// Nanocubes credits the tiers that grew the wood in those proportions.
    ///
    /// This is the two-hop case: the item sold is not the item any activity
    /// produced, and nothing about the sale itself knows where it came from.
    /// Only the composition carried through the conversion does.
    #[tokio::test]
    async fn a_conversion_carries_tier_provenance_into_what_it_produces() {
        let (_dir, service) = write_service().await;
        service
            .db
            .with_writer(|conn| {
                // 100 PED of shavings at 1.00 TT each: 20 short, 30 long, 50 huge.
                for (id, tier, quantity) in [
                    ("ws1", "short", 20),
                    ("ws2", "long", 30),
                    ("ws3", "huge", 50),
                ] {
                    conn.execute(
                        "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,\
                         yield_tier,cost_ped,loot_total_ped) \
                         VALUES(?1,'prov-s',1000.0,1,'PH-3',?2,0.1,?3)",
                        rusqlite::params![id, tier, quantity as f64],
                    )?;
                    conn.execute(
                        "INSERT INTO harvest_loot_items(harvest_id,item_name,quantity,value_ped) \
                         VALUES(?1,'Wood Shavings',?2,?3)",
                        rusqlite::params![id, quantity, quantity as f64],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        service
            .convert_stock(
                Profession::Harvesting,
                "Wood Shavings",
                "Nanocube",
                100.0,
                None,
            )
            .await
            .unwrap();

        // The produced stock is not one anonymous pile: it holds the same
        // 20/30/50 the shavings did.
        let composition: Vec<(String, f64)> = service
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT yield_tier, SUM(quantity) FROM stock_movements \
                     WHERE item_name = 'Nanocube' AND yield_tier IS NOT NULL \
                     GROUP BY yield_tier ORDER BY yield_tier",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap();
        assert_eq!(composition.len(), 3, "all three tiers ride forward");
        // 100 PED of shavings is 10,000 Nanocubes at 0.01 TT each, split in
        // the same 20/30/50 the shavings held.
        for (tier, expected) in [("huge", 5000.0), ("long", 3000.0), ("short", 2000.0)] {
            let (_, got) = composition
                .iter()
                .find(|(name, _)| name == tier)
                .unwrap_or_else(|| panic!("{tier} carried forward"));
            assert!((got - expected).abs() < 1e-9, "{tier}: {got} != {expected}");
        }

        // Sell the Nanocubes at 130 for 100 TT, no fees: 30 PED of markup.
        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Nanocube",
                10_000.0,
                130.0,
                Some(130.0),
                0.0,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            (listing.tt_value - 100.0).abs() < 1e-9,
            "the count and the value agree: {} ",
            listing.tt_value,
        );
        assert!(
            listing.unattributed_qty.abs() < 1e-9,
            "every Nanocube traces to a tier",
        );
        service
            .confirm_auction_listing(&listing.id, 130.0, 0.0, None)
            .await
            .unwrap()
            .unwrap();

        let realised = service.realised_markup_by_tier().await.unwrap();
        let credited = |tier: HarvestYieldTier| {
            realised
                .iter()
                .find(|row| row.yield_tier == tier)
                .map(|row| row.net_markup)
                .unwrap_or(0.0)
        };
        assert!((credited(HarvestYieldTier::Short) - 6.0).abs() < 1e-9);
        assert!((credited(HarvestYieldTier::Long) - 9.0).abs() < 1e-9);
        assert!((credited(HarvestYieldTier::Huge) - 15.0).abs() < 1e-9);
        let total: f64 = realised.iter().map(|row| row.net_markup).sum();
        assert!((total - 30.0).abs() < 1e-9, "and the whole gain is placed");
    }

    /// A conversion can be undone: the source comes back and what it produced
    /// is unmade.
    #[tokio::test]
    async fn deleting_a_conversion_unmakes_what_it_produced() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        service
            .convert_stock(
                Profession::Harvesting,
                "Moonleaf Board",
                "Nanocube",
                50.0,
                None,
            )
            .await
            .unwrap();
        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        let conversion = history
            .iter()
            .find(|row| row.kind == "conversion")
            .expect("the conversion is in history");
        assert_eq!(conversion.target_item.as_deref(), Some("Nanocube"));
        assert!(conversion.can_delete);

        assert!(service.undo_stock_conversion(&conversion.id).await.unwrap());

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the source is back");
        assert!((board.quantity - 100.0).abs() < 1e-9);
        assert!(
            position(&rows, "Nanocube").is_none_or(|row| row.quantity.abs() < 1e-9),
            "the produced stock is unmade"
        );

        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        assert_eq!(history.len(), 1, "the entry stays, marked");
        assert!(history[0].undone);
        assert!(!history[0].can_delete);
        assert!(!service.undo_stock_conversion(&conversion.id).await.unwrap());
    }

    /// A conversion whose output has since been sold cannot be undone: doing
    /// so would leave the player holding less than nothing of it. The refusal
    /// names what is in the way.
    #[tokio::test]
    async fn a_conversion_whose_output_was_sold_refuses_to_be_undone() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        service
            .convert_stock(
                Profession::Harvesting,
                "Moonleaf Board",
                "Nanocube",
                100.0,
                None,
            )
            .await
            .unwrap();
        let conversion_id = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "conversion" && !row.undone)
            .expect("the conversion")
            .id;

        // The Nanocubes go out on the auction, so they are no longer held.
        service
            .create_auction_listing(
                Profession::Harvesting,
                "Nanocube",
                3.0,
                4.0,
                None,
                0.5,
                None,
                None,
            )
            .await
            .unwrap();

        let history = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap();
        let conversion = history
            .iter()
            .find(|row| row.id == conversion_id)
            .expect("still in history");
        assert!(!conversion.can_delete, "history reports it as blocked");
        let reason = conversion
            .undo_blocked_reason
            .as_deref()
            .expect("with a reason");
        assert!(
            reason.contains("Nanocube"),
            "naming what is in the way: {reason}"
        );

        let refused = service.undo_stock_conversion(&conversion_id).await;
        assert!(
            matches!(refused, Err(AnalyticsError::Rejected(_))),
            "the command refuses it too, not only the UI",
        );

        // Undoing the listing that consumed them clears the way.
        let listing_id = service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "listing" && !row.undone)
            .expect("the listing")
            .id;
        assert!(service.undo_auction_listing(&listing_id).await.unwrap());
        assert!(service.undo_stock_conversion(&conversion_id).await.unwrap());
    }

    /// Undoing something that is not there is a not-found, not an error.
    #[tokio::test]
    async fn undoing_an_absent_entry_reports_not_found() {
        let (_dir, service) = write_service().await;
        assert!(!service.undo_auction_listing("no-such-id").await.unwrap());
        assert!(!service.undo_stock_conversion("no-such-id").await.unwrap());
    }

    /// Selling past tracked stock cannot drive holdings below zero. The units
    /// the app never recorded are booked as the opening balance the sale
    /// proves they were, so the position bottoms out at nothing held.
    #[tokio::test]
    async fn selling_beyond_tracked_stock_bottoms_out_at_zero() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        // 100 boards are tracked; the player sells 150 of them.
        let listing = service
            .create_auction_listing(
                Profession::Harvesting,
                "Moonleaf Board",
                150.0,
                5.0,
                None,
                0.0,
                None,
                None,
            )
            .await
            .unwrap();
        assert!((listing.unattributed_qty - 50.0).abs() < 1e-9);

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the line stays");
        assert!(
            board.quantity >= 0.0 && board.quantity.abs() < 1e-9,
            "holdings bottom out at zero, never negative: {}",
            board.quantity
        );
        assert!(
            board.tt_value >= 0.0 && board.tt_value.abs() < 1e-9,
            "stock TT follows the quantity: {}",
            board.tt_value
        );

        // Expiring returns every unit that left, including the ones the app
        // only learned about by their disposal: the player has them in hand.
        service
            .expire_auction_listing(&listing.id, None)
            .await
            .unwrap()
            .unwrap();
        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the line stays");
        assert!((board.quantity - 150.0).abs() < 1e-9);
    }

    /// Converting past tracked stock is the same story on the source side,
    /// and the produced item still receives the whole conversion.
    #[tokio::test]
    async fn converting_beyond_tracked_stock_bottoms_out_at_zero() {
        let (_dir, service) = write_service().await;
        seed_board_stock(&service).await;

        service
            .convert_stock(
                Profession::Harvesting,
                "Moonleaf Board",
                "Nanocube",
                150.0,
                None,
            )
            .await
            .unwrap();

        let rows = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        let board = position(&rows, "Moonleaf Board").expect("the line stays");
        assert!(
            board.quantity >= 0.0 && board.quantity.abs() < 1e-9,
            "holdings bottom out at zero, never negative: {}",
            board.quantity
        );
        // 150 boards at 0.03 TT is 4.50 PED, carried across 1:1 in value and
        // counted at the Nanocube's own 0.01 TT: 450 of them.
        let produced = position(&rows, "Nanocube").expect("the produced stock");
        assert!(
            (produced.quantity - 450.0).abs() < 1e-9,
            "{}",
            produced.quantity
        );
        assert!(
            (produced.tt_value - 4.5).abs() < 1e-9,
            "{}",
            produced.tt_value
        );
    }

    /// Create with the optional fields absent: notes is null and acquired_at
    /// defaults to the (frozen) clock's UTC date.
    #[tokio::test]
    async fn inventory_create_defaults_date_and_notes() {
        let (_dir, service) = write_service().await;
        let body = to_json(
            service
                .create_inventory_item("Imk2", 50.0, 5.0, None, None)
                .await
                .unwrap(),
        );
        // Response is camelCase even though the request is snake_case.
        assert_eq!(body["ttValue"], json!(50.0));
        assert_eq!(body["markupPaid"], json!(5.0));
        assert_eq!(body["notes"], Value::Null);
        assert_eq!(body["acquiredAt"], json!("2026-06-01"));

        // An explicit acquired_at / notes are honoured.
        let body = to_json(
            service
                .create_inventory_item("X", 1.0, 0.0, Some("spare"), Some("2026-01-02"))
                .await
                .unwrap(),
        );
        assert_eq!(body["notes"], json!("spare"));
        assert_eq!(body["acquiredAt"], json!("2026-01-02"));
    }

    /// PATCH field-selection: only PROVIDED (Some) fields update; a None
    /// field is left untouched, so the statement carries only the fields
    /// the patch actually names.
    #[tokio::test]
    async fn inventory_patch_updates_only_provided_fields() {
        let (_dir, service) = write_service().await;
        let created = to_json(
            service
                .create_inventory_item("Orig", 20.0, 3.0, Some("keep"), Some("2026-03-01"))
                .await
                .unwrap(),
        );
        let id = created["id"].as_str().unwrap().to_string();

        // Provide name + tt_value only: markup_paid and notes stay.
        let patched = to_json(
            service
                .update_inventory_item(&id, Some("Renamed"), Some(25.0), None, None)
                .await
                .unwrap()
                .expect("the item exists"),
        );
        assert_eq!(patched["name"], json!("Renamed"));
        assert_eq!(patched["ttValue"], json!(25.0));
        assert_eq!(patched["markupPaid"], json!(3.0), "untouched");
        assert_eq!(patched["notes"], json!("keep"), "untouched");

        let error = service
            .update_inventory_item(&id, Some("  universal AMMO "), Some(99.0), None, None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AnalyticsError::InvalidInput(
                "Universal Ammo is liquid PED and is never held in Inventory"
            )
        ));
        let unchanged = to_json(service.inventory_row(&id).await.unwrap());
        assert_eq!(unchanged, patched, "the rejected patch is atomic");

        // An all-None patch re-reads and returns the row unchanged.
        let same = to_json(
            service
                .update_inventory_item(&id, None, None, None, None)
                .await
                .unwrap()
                .expect("the item exists"),
        );
        assert_eq!(same, patched);

        // Patch a missing id -> not found.
        assert!(service
            .update_inventory_item("no-such", Some("Z"), None, None, None)
            .await
            .unwrap()
            .is_none());
    }

    /// Sell a created item, asserting the delta/type/description-default
    /// branch for profit / loss / zero-delta and the retained sold row.
    #[tokio::test]
    async fn sell_emits_the_right_delta_branch() {
        // PROFIT: sale 20 over cost 12 -> markup 8.0; default description.
        let (_dir, service) = write_service().await;
        let item = to_json(
            service
                .create_inventory_item("Sword", 10.0, 2.0, None, Some("2026-02-01"))
                .await
                .unwrap(),
        );
        let id = item["id"].as_str().unwrap().to_string();
        let body = to_json(
            service
                .sell_inventory_item(&id, 20.0, None, Some("2026-05-10"))
                .await
                .unwrap()
                .expect("the item exists"),
        );
        let entry = &body["ledgerEntry"];
        assert_eq!(entry["type"], json!("markup"));
        assert_eq!(entry["amount"], json!(8.0));
        assert_eq!(entry["tag"], json!("inventory_sale"));
        assert_eq!(entry["date"], json!("2026-05-10"));
        assert_eq!(
            entry["description"],
            json!("Inventory Sale: Sword"),
            "default description form"
        );
        assert_eq!(body["soldItem"]["name"], json!("Sword"));
        // The item leaves current holdings but remains as an auditable sold row.
        assert_eq!(service.list_inventory().await.unwrap().len(), 0);
        let sold_state = service
            .db()
            .with_reader({
                let id = id.clone();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT state, disposed_at FROM inventory_items WHERE id = ?",
                        rusqlite::params![id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(sold_state, ("sold".to_string(), "2026-05-10".to_string()));
        assert_eq!(
            service.list_ledger(None, None).await.unwrap().entries.len(),
            1
        );

        // LOSS: sale 5 under cost 12 -> expense 7.0; explicit description.
        let (_dir, service) = write_service().await;
        let item = to_json(
            service
                .create_inventory_item("Shield", 10.0, 2.0, None, Some("2026-02-01"))
                .await
                .unwrap(),
        );
        let id = item["id"].as_str().unwrap().to_string();
        let body = to_json(
            service
                .sell_inventory_item(&id, 5.0, Some("Dumped it"), None)
                .await
                .unwrap()
                .expect("the item exists"),
        );
        let entry = &body["ledgerEntry"];
        assert_eq!(entry["type"], json!("expense"));
        assert_eq!(entry["amount"], json!(7.0));
        assert_eq!(entry["description"], json!("Dumped it"));
        // Default sold_at is the frozen clock date.
        assert_eq!(entry["date"], json!("2026-06-01"));

        // ZERO-DELTA: sale == cost -> no ledger entry, item leaves holdings.
        let (_dir, service) = write_service().await;
        let item = to_json(
            service
                .create_inventory_item("Even", 8.0, 2.0, None, Some("2026-02-01"))
                .await
                .unwrap(),
        );
        let id = item["id"].as_str().unwrap().to_string();
        let body = to_json(
            service
                .sell_inventory_item(&id, 10.0, None, None)
                .await
                .unwrap()
                .expect("the item exists"),
        );
        assert_eq!(body["ledgerEntry"], Value::Null);
        assert_eq!(body["soldItem"]["name"], json!("Even"));
        assert_eq!(
            service.list_ledger(None, None).await.unwrap().entries.len(),
            0,
            "no noise row"
        );
        assert_eq!(
            service.list_inventory().await.unwrap().len(),
            0,
            "sold item is not a current holding"
        );

        // Sell a missing id -> not found.
        let (_dir, service) = write_service().await;
        assert!(service
            .sell_inventory_item("no-such", 1.0, None, None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn ledger_delete_removes_then_reports_missing() {
        let (_dir, service) = write_service().await;
        let created = to_json(
            service
                .create_ledger_entry("2026-05-01", "expense", "Ammo", 12.5, "ammo")
                .await
                .unwrap(),
        );
        let id = created["id"].as_str().unwrap().to_string();
        // A successful delete reports true (the row existed); a second delete
        // reports false (nothing to remove).
        assert!(service.delete_ledger_entry(&id).await.unwrap());
        assert!(!service.delete_ledger_entry(&id).await.unwrap());
    }

    #[tokio::test]
    async fn preset_list_shapes_rows_then_delete_removes() {
        let (_dir, service) = write_service().await;
        let created = to_json(
            service
                .create_ledger_preset("Decay", "expense", "d", 0.5, "decay")
                .await
                .unwrap(),
        );
        let id = created["id"].as_str().unwrap().to_string();
        // The list shapes the row via preset_item (not an empty default).
        let rows = to_json(service.list_ledger_presets().await.unwrap());
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["name"], json!("Decay"));
        assert_eq!(rows[0]["amount"], json!(0.5));
        assert_eq!(rows[0]["tag"], json!("decay"));
        assert!(service.delete_ledger_preset(&id).await.unwrap());
        assert!(!service.delete_ledger_preset(&id).await.unwrap());
    }

    #[tokio::test]
    async fn inventory_delete_removes_then_reports_missing() {
        let (_dir, service) = write_service().await;
        let created = to_json(
            service
                .create_inventory_item("Sword", 10.0, 2.0, None, None)
                .await
                .unwrap(),
        );
        let id = created["id"].as_str().unwrap().to_string();
        assert!(service.delete_inventory_item(&id).await.unwrap());
        assert!(!service.delete_inventory_item(&id).await.unwrap());
    }

    /// The inventory list reads created rows back, newest acquisition first.
    #[tokio::test]
    async fn list_inventory_returns_created_rows_newest_first() {
        let (_dir, service) = write_service().await;
        service
            .create_inventory_item("Old", 1.0, 0.0, None, Some("2026-01-01"))
            .await
            .unwrap();
        service
            .create_inventory_item("New", 2.0, 0.0, None, Some("2026-02-01"))
            .await
            .unwrap();
        let rows = service.list_inventory().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "New");
        assert_eq!(rows[1].name, "Old");
    }

    #[tokio::test]
    async fn equipment_auction_preserves_basis_across_expiry_and_undo() {
        let (_dir, service) = write_service().await;
        let item = service
            .create_inventory_item("Ares Ring, Improved", 25.0, 75.0, None, None)
            .await
            .unwrap();
        let listing = service
            .create_equipment_listing(&item.id, 105.0, Some(120.0), 0.5, Some("2026-06-02"), None)
            .await
            .unwrap()
            .expect("held equipment");

        assert_eq!(listing.subject_kind, "equipment");
        assert_eq!(listing.inventory_item_id.as_deref(), Some(item.id.as_str()));
        assert_eq!(listing.cost_basis, Some(100.0));
        assert!(service.list_inventory().await.unwrap().is_empty());

        service
            .expire_auction_listing(&listing.id, Some("2026-06-03"))
            .await
            .unwrap()
            .expect("pending listing");
        assert_eq!(service.list_inventory().await.unwrap().len(), 1);
        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        assert_eq!(history[0].subject_kind, "equipment");
        assert_eq!(history[0].status, "expired");
        assert!(service.undo_auction_listing(&listing.id).await.unwrap());
        assert_eq!(service.list_inventory().await.unwrap().len(), 1);
        assert!(service
            .list_ledger(None, None)
            .await
            .unwrap()
            .entries
            .is_empty());
    }

    #[tokio::test]
    async fn equipment_trade_realises_whole_position_result_and_can_be_undone() {
        let (_dir, service) = write_service().await;
        let item = service
            .create_inventory_item("Modified Restoration Chip", 50.0, 20.0, None, None)
            .await
            .unwrap();
        let sale = service
            .trade_equipment(&item.id, 85.0, Some("2026-06-04"))
            .await
            .unwrap()
            .expect("held equipment");

        assert_eq!(sale.channel, "trade");
        assert_eq!(sale.gross_markup, Some(15.0));
        assert!(service.list_inventory().await.unwrap().is_empty());
        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        assert_eq!(history[0].kind, "trade");
        assert_eq!(history[0].subject_kind, "equipment");
        assert_eq!(history[0].net_markup, Some(15.0));
        let ledger = service.list_ledger(None, None).await.unwrap();
        assert_eq!(ledger.entries[0].tag, INVENTORY_SALE_TAG);
        assert_eq!(ledger.entries[0].amount, 15.0);

        assert!(service.undo_auction_listing(&sale.id).await.unwrap());
        assert_eq!(service.list_inventory().await.unwrap().len(), 1);
        assert!(service
            .list_ledger(None, None)
            .await
            .unwrap()
            .entries
            .is_empty());
    }

    #[tokio::test]
    async fn equipment_auction_sale_reverts_but_an_immediate_trade_cannot() {
        let (_dir, service) = write_service().await;
        let item = service
            .create_inventory_item("Ares Ring", 25.0, 75.0, None, None)
            .await
            .unwrap();
        let listing = service
            .create_equipment_listing(&item.id, 105.0, Some(120.0), 0.5, Some("2026-06-02"), None)
            .await
            .unwrap()
            .expect("held equipment");
        let sale = service
            .confirm_auction_listing(&listing.id, 120.0, 1.0, Some("2026-06-04"))
            .await
            .unwrap()
            .expect("pending auction");
        assert_eq!(sale.gross_markup, Some(20.0));
        assert_eq!(sale.sale_fee, Some(1.0));
        let state = service
            .db()
            .with_reader({
                let id = item.id.clone();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT state, disposed_at FROM inventory_items WHERE id = ?",
                        rusqlite::params![id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(state, ("sold".to_string(), "2026-06-04".to_string()));

        let reopened = service
            .revert_auction_sale(&listing.id)
            .await
            .unwrap()
            .expect("sold auction");
        assert_eq!(reopened.status, "pending");
        assert_eq!(reopened.final_price, None);
        let reopened_state = service
            .db()
            .with_reader({
                let id = item.id.clone();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT state, disposed_at FROM inventory_items WHERE id = ?",
                        rusqlite::params![id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(reopened_state, ("listed".to_string(), None));
        let ledger = service.list_ledger(None, None).await.unwrap();
        assert_eq!(
            ledger.entries.len(),
            1,
            "the original listing fee remains spent"
        );
        assert_eq!(ledger.entries[0].amount, 0.5);

        let traded = service
            .create_inventory_item("Restoration Chip", 50.0, 20.0, None, None)
            .await
            .unwrap();
        let trade = service
            .trade_equipment(&traded.id, 85.0, Some("2026-06-05"))
            .await
            .unwrap()
            .expect("held equipment");
        assert!(service
            .revert_auction_sale(&trade.id)
            .await
            .unwrap()
            .is_none());
        let history = service
            .activity_history(Profession::Inventory)
            .await
            .unwrap();
        let trade_history = history.iter().find(|row| row.id == trade.id).unwrap();
        assert_eq!(trade_history.status, "sold");
        assert!(!trade_history.can_revert_sale);
    }

    #[tokio::test]
    async fn inventory_name_resolution_returns_stable_holding_identity() {
        let (_dir, service) = write_service().await;
        let item = service
            .create_inventory_item("Ares Ring, Improved", 25.0, 75.0, None, None)
            .await
            .unwrap();

        let candidates = service
            .resolve_inventory_name(" ares ring, improved ")
            .await
            .unwrap();
        assert_eq!(candidates[0].kind, "equipment");
        assert_eq!(candidates[0].holding_id, item.id);
        assert_eq!(candidates[0].score, 100.0);
    }

    /// A page whose row count exactly meets the limit ends the walk: the extra
    /// probe row finds nothing, so `has_more` is strictly greater-than, not
    /// greater-or-equal.
    #[tokio::test]
    async fn ledger_page_exactly_at_the_limit_has_no_next_cursor() {
        let (_dir, service) = write_service().await;
        service
            .create_ledger_entry("2026-05-01", "expense", "only", 1.0, "t")
            .await
            .unwrap();
        let page = service.list_ledger(None, Some(1)).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn hybrid_window_partitions_an_epoch_range_against_the_watermark() {
        let d = |day: &str| daily_rollup::day_range(day).unwrap();
        let (s11, _) = d("2026-03-11");
        let (s12, _) = d("2026-03-12");
        let (s15, _) = d("2026-03-15");
        let (_, e20) = d("2026-03-20");

        // Interior window with partial edge days at both ends: the full days
        // 12..=14 roll, the sub-day edges read raw.
        let a = hybrid_window(Some(s11 + 100.0), Some(s15 + 100.0), "2026-03-20");
        assert_eq!(
            a.rollup_days,
            Some((Some("2026-03-12".to_string()), "2026-03-14".to_string()))
        );
        assert_eq!(
            a.raw_ranges,
            vec![
                (Some(s11 + 100.0), Some(s12)),
                (Some(s15), Some(s15 + 100.0)),
            ]
        );

        // Bounds landing exactly on midnights: no raw edges at all.
        let b = hybrid_window(Some(s12), Some(s15), "2026-03-20");
        assert_eq!(
            b.rollup_days,
            Some((Some("2026-03-12".to_string()), "2026-03-14".to_string()))
        );
        assert_eq!(b.raw_ranges, Vec::<(Option<f64>, Option<f64>)>::new());

        // The all-time window: unbounded below, raw tail from the watermark on.
        let c = hybrid_window(None, None, "2026-03-20");
        assert_eq!(c.rollup_days, Some((None, "2026-03-20".to_string())));
        assert_eq!(c.raw_ranges, vec![(Some(e20), None)]);

        // A sub-day window spanning no full day is served entirely raw.
        let day = hybrid_window(Some(s12 + 100.0), Some(s12 + 200.0), "2026-03-20");
        assert_eq!(day.rollup_days, None);
        assert_eq!(day.raw_ranges, vec![(Some(s12 + 100.0), Some(s12 + 200.0))]);
    }

    #[test]
    fn merge_family_sums_adds_present_slots_and_leaves_the_rest() {
        let mut into: FamilySums = [
            Some(2.0),
            None,
            Some(1.0),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        merge_family_sums(
            &mut into,
            [
                Some(3.0),
                Some(4.0),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        );
        assert_eq!(into[0], Some(5.0)); // present + present
        assert_eq!(into[1], Some(4.0)); // an empty slot takes the incoming value
        assert_eq!(into[2], Some(1.0)); // an incoming None leaves the slot
        assert_eq!(into[3], None);
    }

    fn make_metrics(
        loot: f64,
        gains: &[(&str, f64)],
        cost: f64,
        losses: &[(&str, f64)],
    ) -> Metrics {
        let map = |pairs: &[(&str, f64)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<std::collections::BTreeMap<String, f64>>()
        };
        Metrics {
            loot_tt: SqlNumber::Float(loot),
            quest_item_tt: SqlNumber::Int(0),
            skill_tt: SqlNumber::Int(0),
            codex_pes: SqlNumber::Int(0),
            quest_pes: SqlNumber::Int(0),
            weapon: SqlNumber::Int(0),
            healing: SqlNumber::Int(0),
            enhancer: SqlNumber::Int(0),
            armour: SqlNumber::Int(0),
            dangling: SqlNumber::Int(0),
            harvest: SqlNumber::Int(0),
            consumables: SqlNumber::Int(0),
            tracking_cost: SqlNumber::Float(cost),
            ledger_gains: map(gains),
            ledger_losses: map(losses),
        }
    }

    #[test]
    fn rate_from_metrics_is_liquid_gains_over_liquid_losses() {
        // (loot 10 + markup 5) / (cost 4 + expense 1) = 3.0.
        let m = make_metrics(10.0, &[("markup", 5.0)], 4.0, &[("expense", 1.0)]);
        assert_eq!(rate_from_metrics(&m), 3.0);
        // Non-positive losses short-circuit to 0.0 (no division).
        let zero = make_metrics(10.0, &[("markup", 5.0)], 0.0, &[]);
        assert_eq!(rate_from_metrics(&zero), 0.0);
    }

    #[test]
    fn slice_rows_zero_cycled_group_yields_zero_rates() {
        // A group whose summed cycled PED is zero rates to 0.0, never a
        // divide-by-zero infinity.
        let agg = SessionAgg {
            duration_hours: 1.0,
            kills: 1,
            loot_tt: 5.0,
            skill_tt: 5.0,
            cycled_ped: 0.0,
            dominant_mob: Some("X".to_string()),
            dominant_mob_kills: 1,
            ..SessionAgg::default()
        };
        let rows =
            build_activity_slice_rows(&[agg], |s| s.dominant_mob.clone(), |s| s.dominant_mob_kills);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pes_per100_ped, 0.0);
        assert_eq!(rows[0].loot_rate, 0.0);
    }

    /// A summary row with zero kills is dropped by the Activity filter even
    /// when its duration and cycled PED are positive.
    #[tokio::test]
    async fn activity_filter_drops_a_zero_kill_summary_row() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO session_summaries \
                 (session_id, summary_version, started_at, ended_at, duration_hours, kills, \
                  loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, \
                  cycled_ped, regular_skill_ped_json, attribute_levels_json, regular_skill_tt, \
                  attribute_levels_total, dominant_mob, dominant_tag, dominant_weapon, \
                  dominant_mob_kills, dominant_tag_kills, activity_skill_tt) \
                 VALUES ('ghost', 2, 0, 3600, 1.0, 0, 2.0, 0, 0, 0, 0, 0, 5.0, '{}', '{}', 0, 0, \
                         'Ghost', NULL, NULL, 3, 0, 1.0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let sessions = db.with_reader(activity_sessions_read).await.unwrap();
        assert!(sessions.is_empty());
    }

    /// The Overview timeline carries per-bucket family totals from BOTH sides
    /// of the hybrid split (a rolled old day and a raw current day), including
    /// the session-cost leg of a rolled day whose heal/dangling sums are NULL.
    /// The period metrics fold the same raw edge in.
    #[tokio::test]
    async fn overview_timeline_carries_rolled_and_raw_family_totals() {
        let now = 1_800_000_000.0;
        let day = 86400.0;
        let (_dir, db) = open_env().await;
        let old = now - 40.0 * day;
        db.with_writer(move |conn| {
            // A rolled old day: a session with armour cost only (heal/dangling
            // NULL, so the rollup keeps them NULL) and a loot kill.
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, armour_cost, heal_cost, dangling_cost) \
                 VALUES ('old', ?1, ?2, 3.0, NULL, NULL)",
                rusqlite::params![old, old + 3600.0],
            )?;
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('ok', 'old', 'M', ?1, 0, 20.0)",
                rusqlite::params![old + 10.0],
            )?;
            // A raw kill dated at `now` (after the healed watermark).
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('rk', 'x', 'M', ?1, 0, 7.0)",
                rusqlite::params![now],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let data = overview_impl(&db, now, "all").await.unwrap();
        let loot: f64 = data.timeline.iter().map(|p| p.loot_tt).sum();
        let cost: f64 = data.timeline.iter().map(|p| p.tracking_cost).sum();
        // Rolled loot 20 (old day) + raw loot 7 (today).
        assert_eq!(loot, 27.0);
        // The armour-only rolled session's cost survives the session leg.
        assert_eq!(cost, 3.0);
        // The period metrics fold the raw edge into the rolled sums too.
        assert_eq!(data.returns_breakdown.loot_tt, 27.0);
    }

    /// A timeline point carries its per-tag ledger bucket totals.
    #[tokio::test]
    async fn overview_timeline_carries_ledger_bucket_totals() {
        let now = 1_800_000_000.0;
        let (_dir, db) = open_env().await;
        let today = epoch_to_iso(now);
        let today_c = today.clone();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                 VALUES ('g', ?1, 'markup', 'sold', 12.5, 'loot_sale')",
                rusqlite::params![today_c],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let data = overview_impl(&db, now, "all").await.unwrap();
        let point = data
            .timeline
            .iter()
            .find(|p| p.bucket == today)
            .expect("the ledgered day has a timeline point");
        assert_eq!(point.ledger_gains.get("loot_sale"), Some(&12.5));
    }

    /// The trend bands are exclusive at their edges: a recent rate exactly at
    /// prior * 1.02 is not "improving", and exactly at prior * 0.98 is not
    /// "declining".
    #[tokio::test]
    async fn overview_trend_bands_are_exclusive_at_the_edges() {
        let now = 1_800_000_000.0;
        let day = 86400.0;

        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 50.0, 1, 51.0).await; // 51/50 = 1.02
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 10.0).await; // 1.0
        assert_eq!(
            overview_impl(&db, now, "all").await.unwrap().trend,
            "stable"
        );

        let (_dir, db) = open_env().await;
        seed_rate(&db, "r", now - 10.0 * day, 50.0, 1, 49.0).await; // 49/50 = 0.98
        seed_rate(&db, "p", now - 45.0 * day, 10.0, 1, 10.0).await; // 1.0
        assert_eq!(
            overview_impl(&db, now, "all").await.unwrap().trend,
            "stable"
        );
    }

    /// Every field the raw reconciliation aggregate computes for one session,
    /// hand-derived from the seed.
    #[tokio::test]
    async fn raw_session_agg_computes_every_field() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            for (kill, mob, species, maturity, loot, enhancer) in [
                ("k1", "Young Atrox", "Atrox", "Young", 2.0, 0.1),
                ("k2", "Young Atrox", "Atrox", "Young", 3.0, 0.0),
                ("k3", "Young Atrox", "Atrox", "Young", 4.0, 0.0),
                ("k4", "Snable", "Snable", "", 1.0, 0.0),
                ("k5", "Unknown", "", "", 0.5, 0.0),
            ] {
                conn.execute(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                     timestamp, enhancer_cost, loot_total_ped) \
                     VALUES (?1, 'rs', ?2, ?3, ?4, 1500.0, ?5, ?6)",
                    rusqlite::params![kill, mob, species, maturity, enhancer, loot],
                )?;
            }
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
                 VALUES ('k1', 'Rifle', 30, 0.05), ('k2', 'Pistol', 10, 0.01)",
                [],
            )?;
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES ('rs', 1100.0, 'Rifle', 1.0, 0.5), ('rs', 1200.0, 'Rifle', 1.0, 0.25), \
                        ('rs', 1300.0, 'Agility', 0.25, NULL)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let agg = db
            .with_reader(|conn| raw_session_agg(conn, "rs", 1000.0, 8200.0, 0.07, 0.11, 0.13, None))
            .await
            .unwrap();
        assert_eq!(agg.duration_hours, 2.0); // (8200 - 1000) / 3600
        assert_eq!(agg.armour_cost, 0.07);
        assert_eq!(agg.heal_cost, 0.11);
        assert_eq!(agg.dangling_cost, 0.13);
        assert_eq!(agg.kills, 5);
        assert_eq!(agg.loot_tt, 10.5);
        assert_eq!(agg.enhancer_cost, 0.1);
        assert_eq!(agg.weapon_cost, 1.6); // 30 @ 0.05 + 10 @ 0.01
        assert_eq!(agg.weapon_shots, 40.0);
        assert_eq!(agg.skill_tt, 0.75); // 0.5 + 0.25 (NULL excluded)
                                        // Atrox 3 of 4 known kills = 0.75, species present -> dominant mob.
        assert_eq!(agg.dominant_mob, Some("Young Atrox".to_string()));
        assert_eq!(agg.dominant_mob_kills, 3);
        // weapon 1.6 + enhancer 0.1 + armour 0.07 + heal 0.11 + dangling 0.13.
        assert_eq!(agg.cycled_ped, 2.01);
    }

    /// Species-less stamps are legacy tag-mode kills, which migration 0018
    /// lifted onto the session row as its name. They must never re-enter the
    /// mob axis as if they were a hunted species.
    #[tokio::test]
    async fn raw_session_agg_excludes_species_less_stamps_from_the_mob_axis() {
        let (_dir, db) = open_env().await;
        db.with_writer(|conn| {
            for i in 0..3 {
                conn.execute(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                     timestamp, enhancer_cost, loot_total_ped) \
                     VALUES (?1, 'tg', 'Thing', NULL, NULL, ?2, 0, 1.0)",
                    rusqlite::params![format!("tg-{i}"), 1000.0 + i as f64],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let agg = db
            .with_reader(|conn| {
                raw_session_agg(
                    conn,
                    "tg",
                    1000.0,
                    4600.0,
                    0.0,
                    0.0,
                    0.0,
                    Some("Thing".to_string()),
                )
            })
            .await
            .unwrap();
        assert_eq!(agg.dominant_mob, None);
        assert_eq!(agg.dominant_mob_kills, 0);
        // The designated axis carries it instead, under the session name.
        assert_eq!(agg.session_name, Some("Thing".to_string()));
    }

    /// The trend compares the recent-30d window (lower bound `now - 30*86400`)
    /// against the prior-30d window (bounds `now - 60*86400` .. `now -
    /// 30*86400`). A high recent rate over a low prior rate is "improving".
    /// The three seeded sessions pin BOTH window bounds: an ancient session
    /// older than 60 days sits outside the prior window, so mutating either
    /// bound's `-` to a `/` (which collapses the bound to near-epoch) folds
    /// extra sessions in and flips the verdict. Mutating the 30-day bound
    /// empties the prior window ("stable"); mutating the 60-day bound drags
    /// the prior rate up with the ancient session ("declining").
    #[tokio::test]
    async fn overview_trend_reads_the_thirty_and_sixty_day_window_bounds() {
        let now = 1_800_000_000.0;
        let day = 86400.0;
        let (_dir, db) = open_env().await;
        seed_rate(&db, "recent", now - 10.0 * day, 10.0, 1, 20.0).await; // rate 2.0
        seed_rate(&db, "prior", now - 45.0 * day, 10.0, 1, 10.0).await; // rate 1.0
        seed_rate(&db, "ancient", now - 90.0 * day, 10.0, 1, 100.0).await; // rate 10.0
        assert_eq!(
            overview_impl(&db, now, "all").await.unwrap().trend,
            "improving"
        );
    }

    /// A rolled old day whose session carries heal cost only (armour/dangling
    /// NULL, so the rollup keeps them NULL). The session-cost leg must survive:
    /// the family guard is a disjunction, and mutating its second `||` to `&&`
    /// (which binds tighter, parsing as `armour || (heal && dangling)`) would
    /// drop a heal-only day's cost.
    #[tokio::test]
    async fn overview_timeline_carries_a_heal_only_rolled_session_cost() {
        let now = 1_800_000_000.0;
        let day = 86400.0;
        let (_dir, db) = open_env().await;
        let old = now - 40.0 * day;
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, armour_cost, heal_cost, dangling_cost) \
                 VALUES ('old', ?1, ?2, NULL, 4.0, NULL)",
                rusqlite::params![old, old + 3600.0],
            )?;
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
                 VALUES ('ok', 'old', 'M', ?1, 0, 20.0)",
                rusqlite::params![old + 10.0],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let data = overview_impl(&db, now, "all").await.unwrap();
        let cost: f64 = data.timeline.iter().map(|p| p.tracking_cost).sum();
        assert_eq!(cost, 4.0);
    }

    // ── The revamped Hunting aggregate and hunting stock lifecycle ──

    /// Seed two hunted sessions: one under a definition with a quest focus
    /// stamped through contexts, one legacy session with no definition and
    /// no stamps. Species and maturity ride the kills.
    async fn seed_hunting_scenario(service: &AnalyticsService) {
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO session_definitions(id, name, ad_hoc_segments, is_active) \
                     VALUES(7, 'ARIS Dailies', 0, 1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO quest_families(id, name) VALUES(3, 'Daily Hunting 1')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO quests(id, name, family_id, reward_ped, reward_is_skill, \
                     expected_reward_markup_percent) \
                     VALUES(11, 'Daily Hunting 1: Weak Mortirex', 3, 4.0, 0, 150.0)",
                    [],
                )?;

                // The definition-run session: one hour, ended.
                conn.execute(
                    "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active, \
                     definition_id) VALUES('hunt-a', 1780300000.0, 1780303600.0, 0, 7)",
                    [],
                )?;
                // The legacy session: no definition, no contexts.
                conn.execute(
                    "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active) \
                     VALUES('hunt-b', 1780200000.0, 1780203600.0, 0)",
                    [],
                )?;

                // The quest focus: an interval, then a context naming it, then
                // the empty context after unfocus.
                conn.execute(
                    "INSERT INTO session_intervals(id, session_id, kind, label, ref_id, \
                     started_at, ended_at) \
                     VALUES(21, 'hunt-a', 'quest', 'Daily Hunting 1: Weak Mortirex', 11, \
                     1780300000.0, 1780301800.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_contexts(id, session_id, created_at) \
                     VALUES(31, 'hunt-a', 1780300000.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_context_intervals(context_id, interval_id) \
                     VALUES(31, 21)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_contexts(id, session_id, created_at) \
                     VALUES(32, 'hunt-a', 1780301800.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completions \
                     (session_id, quest_id, completed_at, activity_context_id, \
                      activity_interval_id, reward_source, reward_kind, reward_ped, \
                      expected_reward_markup_percent, reward_outcome) \
                     VALUES('hunt-a', 11, 1780301800.0, 31, 21, 'none', 'item', NULL, NULL, 'confirmed')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completion_reward_items \
                     (completion_id, item_name, quantity, value_ped, accounting_kind) \
                     SELECT id, 'Universal Ammo', 1, 4.0, 'liquid' \
                     FROM session_quest_completions \
                     WHERE session_id = 'hunt-a' AND quest_id = 11",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO quest_reward_attributions \
                     (completion_id, activity_context_id, session_definition_id, weight, basis, \
                      cycled_ped, duration_seconds) \
                     SELECT id, 31, 7, 1.0, 'duration', 0.0, 1800.0 \
                     FROM session_quest_completions \
                     WHERE session_id = 'hunt-a' AND quest_id = 11",
                    [],
                )?;

                // Kills: two focused Atrox (distinct maturities), one
                // unfocused Atrox, and one species-less legacy stamp.
                for (id, session, ts, species, maturity, cost, enh, loot, context) in [
                    (
                        "k1",
                        "hunt-a",
                        1780300100.0,
                        "Atrox",
                        "Young",
                        3.0,
                        0.5,
                        3.2,
                        Some(31),
                    ),
                    (
                        "k2",
                        "hunt-a",
                        1780300200.0,
                        "Atrox",
                        "Mature",
                        4.0,
                        0.0,
                        3.4,
                        Some(31),
                    ),
                    (
                        "k3",
                        "hunt-a",
                        1780302000.0,
                        "Atrox",
                        "Young",
                        2.0,
                        0.0,
                        2.6,
                        Some(32),
                    ),
                    ("k4", "hunt-b", 1780200100.0, "", "", 5.0, 0.0, 4.1, None),
                ] {
                    conn.execute(
                        "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, \
                         timestamp, cost_ped, enhancer_cost, loot_total_ped, context_id) \
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        rusqlite::params![
                            id,
                            session,
                            if species.is_empty() {
                                "Old Tag"
                            } else {
                                species
                            },
                            species,
                            maturity,
                            ts,
                            cost,
                            enh,
                            loot,
                            context,
                        ],
                    )?;
                }

                // Loot items for the species composition and the stock base.
                for (kill, item, qty, tt, shrapnel) in [
                    ("k1", "Animal Muscle Oil", 40_i64, 12.0, 0_i64),
                    ("k2", "Animal Muscle Oil", 20, 6.0, 0),
                    ("k2", "Shrapnel", 5000, 5.0, 1),
                ] {
                    conn.execute(
                        "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped, \
                         is_enhancer_shrapnel) VALUES(?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![kill, item, qty, tt, shrapnel],
                    )?;
                }

                // A stamped and an unstamped skill gain.
                conn.execute(
                    "INSERT INTO skill_gains(session_id, skill_name, amount, ped_value, \
                     timestamp, context_id) \
                     VALUES('hunt-a', 'Rifle', 1.0, 0.8, 1780300150.0, 31)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO skill_gains(session_id, skill_name, amount, ped_value, \
                     timestamp) VALUES('hunt-b', 'Rifle', 1.0, 0.5, 1780200150.0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    /// The aggregate's three views reconcile: Overall equals the session
    /// set's kill-grain sums, the Sessions axis keys on the definition with
    /// the unassigned bucket pinned last, and the Targets axis groups
    /// species with maturity drilldown and an unclassified bucket.
    #[tokio::test]
    async fn hunting_activity_reconciles_across_overall_sessions_and_targets() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;
        service
            .db
            .with_writer(|conn| {
                conn.execute("UPDATE quests SET reward_ped = 99.0 WHERE id = 11", [])?;
                Ok(())
            })
            .await
            .unwrap();

        let data = service.hunting_activity("all").await.unwrap();

        assert_eq!(data.overall.sessions, 2);
        assert_eq!(data.overall.kills, 4);
        assert!((data.overall.cycled - 14.5).abs() < 1e-9, "3.5+4+2+5");
        assert!((data.overall.returns - 13.3).abs() < 1e-9);

        // Sessions: the definition row leads, the unassigned bucket is last.
        assert_eq!(data.definitions.len(), 2);
        let aris = &data.definitions[0];
        assert_eq!(aris.definition_id, Some(7));
        assert_eq!(aris.name, "ARIS Dailies");
        assert_eq!(aris.kills, 3);
        assert!((aris.cycled - 9.5).abs() < 1e-9);
        assert_eq!(aris.instances, 1);
        assert_eq!(aris.instance_rows.len(), 1);
        assert_eq!(aris.loot_items.len(), 1);
        assert_eq!(aris.loot_items[0].item_name, "Animal Muscle Oil");
        assert_eq!(aris.loot_items[0].quantity, 60);
        let unassigned = &data.definitions[1];
        assert_eq!(unassigned.definition_id, None);
        assert_eq!(unassigned.kills, 1);

        // The definition's signatures: the focused stretch reports at family
        // grain with the variant beneath it, and the unfocused remainder is
        // the ambient row.
        let family = aris
            .activities
            .iter()
            .find(|row| row.kind == "quest_family")
            .expect("family row");
        assert_eq!(family.label, "Daily Hunting 1");
        assert_eq!(family.kills, 2);
        assert!((family.cycled - 7.5).abs() < 1e-9);
        assert!((family.pes - 0.8).abs() < 1e-9);
        assert_eq!(family.runs, 1);
        assert_eq!(family.confirmed_reward_ped, 4.0);
        assert_eq!(family.reward_items.len(), 1);
        assert_eq!(family.reward_items[0].item_name, "Universal Ammo");
        assert_eq!(family.reward_items[0].value_ped, 4.0);
        assert_eq!(family.reward_status, "item");
        assert_eq!(family.loot_items.len(), 1);
        assert_eq!(family.loot_items[0].item_name, "Animal Muscle Oil");
        assert_eq!(family.loot_items[0].quantity, 60);
        assert_eq!(family.variants.len(), 1);
        assert_eq!(family.variants[0].label, "Daily Hunting 1: Weak Mortirex");
        let ambient = aris
            .activities
            .iter()
            .find(|row| row.kind == "ambient")
            .expect("ambient row");
        assert_eq!(ambient.kills, 1);
        // The focused stretch spans 1800s, the remainder the other 1800s.
        assert!((family.duration_hours - 0.5).abs() < 1e-9);
        assert!((ambient.duration_hours - 0.5).abs() < 1e-9);

        // The legacy session's unstamped kill lands in the unassigned
        // bucket's ambient remainder rather than vanishing.
        let legacy_ambient = unassigned
            .activities
            .iter()
            .find(|row| row.kind == "ambient")
            .expect("legacy ambient row");
        assert_eq!(legacy_ambient.kills, 1);
        assert!((legacy_ambient.pes - 0.5).abs() < 1e-9);

        // Targets: Atrox with two maturities, the unclassified bucket last,
        // enhancer shrapnel out of the composition.
        assert_eq!(data.species.len(), 2);
        let atrox = &data.species[0];
        assert_eq!(atrox.mob_species, "Atrox");
        assert_eq!(atrox.kills, 3);
        assert_eq!(atrox.maturities.len(), 2);
        assert_eq!(atrox.loot_items.len(), 1);
        assert_eq!(atrox.loot_items[0].item_name, "Animal Muscle Oil");
        assert_eq!(atrox.loot_items[0].quantity, 60);
        // Atrox dominated its session, so it claims that session's skill TT.
        assert_eq!(atrox.pes_sessions, 1);
        assert!(atrox.pes.is_some());
        let unclassified = &data.species[1];
        assert_eq!(unclassified.mob_species, "");
        assert_eq!(unclassified.kills, 1);
        assert!(unclassified.pes.is_none(), "a tag row claims no skill");
    }

    #[tokio::test]
    async fn hunting_expected_return_replays_captured_evidence_and_discloses_legacy_gaps() {
        use crate::expected_hunting::{
            HuntingLooterLevels, LooterSource, OffensiveComponentEvidence, OffensiveComponentKind,
            OffensiveLoadoutEvidence,
        };

        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;
        let evidence = OffensiveLoadoutEvidence {
            components: vec![
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Weapon,
                    catalog_id: Some("weapon-1".into()),
                    name: "Test Rifle".into(),
                    efficiency_pct: Some(90.0),
                    raw_tt_per_use: 0.03,
                    consumed_premium_per_use: 0.0,
                },
                OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Amplifier,
                    catalog_id: Some("amp-1".into()),
                    name: "Test Amplifier".into(),
                    efficiency_pct: Some(60.0),
                    raw_tt_per_use: 0.01,
                    consumed_premium_per_use: 0.002,
                },
            ],
            looters: HuntingLooterLevels {
                animal: 30.0,
                mutant: 60.0,
                robot: 90.0,
            },
            looter_source: LooterSource::ThreeLooterMean,
        };
        let encoded = serde_json::to_string(&evidence).unwrap();
        service
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO kill_tool_stats \
                     (kill_id, tool_name, shots_fired, cost_per_shot, \
                      expected_economics_json, evidence_fingerprint) \
                     VALUES ('k1', 'Test Rifle', 10, 0.042, ?1, ?1)",
                    rusqlite::params![encoded],
                )?;
                conn.execute(
                    "INSERT INTO kill_tool_stats \
                     (kill_id, tool_name, shots_fired, cost_per_shot) \
                     VALUES ('k2', 'Legacy Rifle', 4, 0.05)",
                    [],
                )?;
                crate::session_rollup::recompute_session(conn, "hunt-a")?;
                Ok(())
            })
            .await
            .unwrap();

        let data = service.hunting_activity("all").await.unwrap();
        let expected = data.overall.expected.expect("captured model basis");
        assert_eq!(expected.model_version, "community_v1");
        assert_eq!(expected.looter_source, "three_looter_mean");
        assert_eq!(expected.looter_level, 60.0);
        assert_eq!(expected.expected_loot_tt, 0.3839);
        assert_eq!(expected.modelled_raw_tt, 0.4);
        assert_eq!(expected.eligible_offensive_cost, 0.42);
        assert_eq!(expected.offensive_tt_recovery, 0.95975);
        assert_eq!(expected.expected_tt_rate, 0.914048);
        assert_eq!(
            expected.effective_efficiency,
            crate::expected_hunting::EffectiveEfficiency::WithinModelRange {
                efficiency_pct: 17.21,
            }
        );
        assert_eq!(expected.break_even_loot_markup, 1.094035);
        assert_eq!(expected.coverage, 0.6667);
        assert!(expected.incomplete);
        assert_eq!(expected.missing_basis_phases, 1);

        let aris = data.definitions[0]
            .expected
            .as_ref()
            .expect("definition model basis");
        let atrox = data.species[0]
            .expected
            .as_ref()
            .expect("species model basis");
        assert_eq!(aris, atrox);
        assert_eq!(aris, &expected);

        let family = data.definitions[0]
            .activities
            .iter()
            .find(|row| row.kind == "quest_family")
            .expect("quest family");
        assert_eq!(family.expected.as_ref(), Some(&expected));
        assert_eq!(family.variants[0].expected.as_ref(), Some(&expected));
        let ambient = data.definitions[0]
            .activities
            .iter()
            .find(|row| row.kind == "ambient")
            .expect("ambient remainder");
        assert!(ambient.expected.is_none());
    }

    #[test]
    fn aggregate_effective_efficiency_weights_mixed_loadouts_by_their_economics() {
        use crate::expected_hunting::{
            HuntingLooterLevels, LooterSource, OffensiveComponentEvidence, OffensiveComponentKind,
            OffensiveLoadoutEvidence,
        };

        let evidence =
            |name: &str, efficiency_pct: f64, raw_tt: f64, premium: f64| OffensiveLoadoutEvidence {
                components: vec![OffensiveComponentEvidence {
                    kind: OffensiveComponentKind::Weapon,
                    catalog_id: None,
                    name: name.to_string(),
                    efficiency_pct: Some(efficiency_pct),
                    raw_tt_per_use: raw_tt,
                    consumed_premium_per_use: premium,
                }],
                looters: HuntingLooterLevels {
                    animal: 50.0,
                    mutant: 50.0,
                    robot: 50.0,
                },
                looter_source: LooterSource::ThreeLooterMean,
            };
        let mut aggregate = ExpectedHuntingAccumulator::default();
        aggregate.add(&evidence("High-efficiency rifle", 90.0, 0.03, 0.0), 10);
        aggregate.add(&evidence("Limited finisher", 70.0, 0.01, 0.0002), 20);

        let result = aggregate.finish().expect("modelled mixed loadouts");
        assert_eq!(result.modelled_raw_tt, 0.5);
        assert_eq!(result.eligible_offensive_cost, 0.504);
        assert_eq!(result.expected_tt_rate, 0.944841);
        assert_eq!(
            result.effective_efficiency,
            crate::expected_hunting::EffectiveEfficiency::WithinModelRange {
                efficiency_pct: 71.2,
            }
        );
    }

    #[test]
    fn failed_expected_evaluation_still_narrows_coverage() {
        use crate::expected_hunting::{
            HuntingLooterLevels, LooterSource, OffensiveComponentEvidence, OffensiveComponentKind,
            OffensiveLoadoutEvidence,
        };

        let evidence = OffensiveLoadoutEvidence {
            components: vec![OffensiveComponentEvidence {
                kind: OffensiveComponentKind::Weapon,
                catalog_id: None,
                name: "Invalid evidence".to_string(),
                efficiency_pct: Some(f64::NAN),
                raw_tt_per_use: 0.02,
                consumed_premium_per_use: 0.0,
            }],
            looters: HuntingLooterLevels {
                animal: 50.0,
                mutant: 50.0,
                robot: 50.0,
            },
            looter_source: LooterSource::ThreeLooterMean,
        };
        let mut aggregate = ExpectedHuntingAccumulator::default();
        aggregate.add(&evidence, 5);

        assert_eq!(aggregate.candidate_raw_tt, 0.1);
        assert_eq!(aggregate.modelled_raw_tt, 0.0);
        assert!(aggregate.incomplete);
    }

    #[test]
    fn legacy_definition_outflows_absorb_context_specific_hunt_stock() {
        let key = |definition_id, activity_context_id| PositionKey {
            source_kind: "hunt".to_string(),
            tier: String::new(),
            species: "Atrox".to_string(),
            definition_id,
            activity_context_id,
            tool: String::new(),
            quest: None,
        };
        let legacy = key(Some(7), None);
        let context_a = key(Some(7), Some(70));
        let context_b = key(Some(7), Some(71));
        let mut positions = std::collections::BTreeMap::from([
            (legacy.clone(), -50.0),
            (context_a.clone(), 40.0),
            (context_b.clone(), 60.0),
        ]);

        absorb_legacy_hunt_outflows(&mut positions);

        assert_eq!(positions[&legacy], 0.0);
        assert!((positions[&context_a] - 20.0).abs() < 1e-9);
        assert!((positions[&context_b] - 30.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn hunting_activity_uses_the_latest_reward_review_outcome() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;
        service
            .db
            .with_writer(|conn| {
                let completion_id: i64 = conn.query_row(
                    "SELECT id FROM session_quest_completions \
                     WHERE session_id = 'hunt-a' AND quest_id = 11",
                    [],
                    |row| row.get(0),
                )?;
                conn.execute(
                    "UPDATE session_quest_completions \
                     SET reward_outcome = 'unresolved', reward_kind = NULL, reward_ped = NULL \
                     WHERE id = ?",
                    rusqlite::params![completion_id],
                )?;
                conn.execute(
                    "INSERT INTO quest_reward_reviews \
                     (completion_id, outcome, policy, reviewed_at) \
                     VALUES(?, 'confirmed', 'named_items', 1780301810.0)",
                    rusqlite::params![completion_id],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let data = service.hunting_activity("all").await.unwrap();
        let family = data.definitions[0]
            .activities
            .iter()
            .find(|row| row.kind == "quest_family")
            .expect("family row");
        assert_eq!(family.reward_status, "item");
        assert_eq!(family.confirmed_reward_ped, 4.0);
        assert_eq!(family.reward_items[0].item_name, "Universal Ammo");

        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "UPDATE quest_reward_reviews SET outcome = 'none', policy = 'none'",
                    [],
                )?;
                conn.execute("DELETE FROM session_quest_completion_reward_items", [])?;
                Ok(())
            })
            .await
            .unwrap();
        let data = service.hunting_activity("all").await.unwrap();
        let family = data.definitions[0]
            .activities
            .iter()
            .find(|row| row.kind == "quest_family")
            .expect("family row");
        assert_eq!(family.reward_status, "none");
        assert_eq!(family.confirmed_reward_ped, 0.0);
        assert!(family.reward_items.is_empty());
    }

    /// The period filter works at session grain: a window that excludes the
    /// legacy session drops it from every view at once.
    #[tokio::test]
    async fn hunting_activity_period_is_session_scoped() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;

        let data = hunting_activity_impl(service.db(), Some(1780250000.0))
            .await
            .unwrap();
        assert_eq!(data.overall.sessions, 1);
        assert_eq!(data.overall.kills, 3);
        assert_eq!(data.definitions.len(), 1);
        assert_eq!(data.definitions[0].definition_id, Some(7));
        assert!(
            data.species.iter().all(|row| row.mob_species == "Atrox"),
            "the legacy tag row left with its session"
        );
    }

    /// The hybrid read is exact: the same database answers identically
    /// with every session served raw (no settlement has run) and with the
    /// ended sessions settled into their rollup cells. This is the
    /// raw-versus-rollup equivalence the settlement marker promises.
    #[tokio::test]
    async fn hunting_activity_reads_identically_before_and_after_settlement() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;

        let raw = hunting_activity_impl(service.db(), None).await.unwrap();
        let raw_stock = all_positions_for_test(service.db()).await;
        service
            .db()
            .with_writer(crate::session_rollup::heal)
            .await
            .unwrap();
        let settled = hunting_activity_impl(service.db(), None).await.unwrap();
        let settled_stock = all_positions_for_test(service.db()).await;

        assert_eq!(raw, settled);
        assert_eq!(raw_stock, settled_stock);
    }

    /// A flattened `(item, [(tier, species, tool, quantity)], unit_tt)`
    /// row of the whole-inventory position map.
    type FlatPositionRow = (String, Vec<(String, String, String, Option<i64>, f64)>, f64);

    /// The whole-inventory position map through the batch read, for the
    /// settlement-equivalence assertion above.
    async fn all_positions_for_test(db: &crate::db::Db) -> Vec<FlatPositionRow> {
        db.with_reader(|conn| {
            let hunting_loot = crate::session_rollup::effective_hunting_loot_cells(conn, None)?;
            let map = all_item_positions(conn, &hunting_loot)?;
            let mut rows: Vec<FlatPositionRow> = map
                .into_iter()
                .map(|(item, (positions, unit_tt))| {
                    let keys = positions
                        .into_iter()
                        .map(|(key, quantity)| {
                            (key.tier, key.species, key.tool, key.definition_id, quantity)
                        })
                        .collect();
                    (item, keys, unit_tt)
                })
                .collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(rows)
        })
        .await
        .expect("positions")
    }

    /// The hunting stock lifecycle end to end: kill loot is the acquisition
    /// base keyed by species, a hunting listing consumes it in proportion,
    /// a confirmed sale realises markup back onto the species, and every
    /// read stays scoped to its own activity.
    #[tokio::test]
    async fn hunting_listing_attributes_realised_markup_to_species() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;

        // The hunting stock lists the oil (and the shrapnel pile); the
        // harvesting stock does not.
        let hunt_stock = service.stock_positions(Profession::Hunting).await.unwrap();
        let oil = position(&hunt_stock, "Animal Muscle Oil").expect("oil position");
        assert_eq!(oil.quantity, 60.0);
        assert!((oil.tt_value - 18.0).abs() < 1e-9);
        let harvest_stock = service
            .stock_positions(Profession::Harvesting)
            .await
            .unwrap();
        assert!(
            position(&harvest_stock, "Animal Muscle Oil").is_none(),
            "hunted loot stays off the harvesting stock list"
        );

        // List half the oil from the Hunting tab and confirm it sold above TT.
        let listing = service
            .create_auction_listing(
                Profession::Hunting,
                "Animal Muscle Oil",
                30.0,
                12.0,
                None,
                0.5,
                Some("2026-06-01"),
                None,
            )
            .await
            .unwrap();
        assert!((listing.attributed_qty - 30.0).abs() < 1e-9);
        assert!((listing.attributed_tt - 9.0).abs() < 1e-9);

        service
            .confirm_auction_listing(&listing.id, 12.0, 0.2, Some("2026-06-01"))
            .await
            .unwrap()
            .expect("confirmed");

        // 12.00 fetched over 9.00 TT, less 0.70 of fees: 2.30 net, all of it
        // Atrox's because the whole listing was tracked Atrox stock.
        let realised = service.realised_markup_by_species().await.unwrap();
        assert_eq!(realised.len(), 1);
        assert_eq!(realised[0].mob_species, "Atrox");
        assert!((realised[0].net_markup - 2.30).abs() < 1e-9);
        let definitions = service.realised_markup_by_definition().await.unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].definition_id, 7);
        assert!((definitions[0].net_markup - 2.30).abs() < 1e-9);
        assert!(
            service.realised_markup_by_tier().await.unwrap().is_empty(),
            "no yield tier claims a hunted sale"
        );

        // Each activity's Market and History see their own records only.
        assert_eq!(
            service
                .auction_listings(Profession::Hunting)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(service
            .auction_listings(Profession::Harvesting)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            service
                .activity_history(Profession::Hunting)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(service
            .activity_history(Profession::Harvesting)
            .await
            .unwrap()
            .is_empty());

        // The position dropped by the listed quantity and stays down.
        let after = service.stock_positions(Profession::Hunting).await.unwrap();
        assert_eq!(
            position(&after, "Animal Muscle Oil").unwrap().quantity,
            30.0
        );
    }

    /// A missing mob declaration limits only the optional Targets detail.
    /// Inventory still recognises non-shrapnel loot as Hunting output, and a
    /// confirmed sale reaches Overall, the unclassified target, and the
    /// session definition that deliberately owned the play.
    #[tokio::test]
    async fn unclassified_hunting_sale_attributes_without_a_mob_name() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, \
                         timestamp, cost_ped, enhancer_cost, loot_total_ped) \
                     VALUES('k-unclassified', 'hunt-a', 'Unknown', '', '', \
                            1780302100.0, 2.0, 0.0, 1.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped, \
                         is_enhancer_shrapnel) \
                     VALUES('k-unclassified', 'MarCorp Kallous-5, E.L.M Edition (L)', \
                            1, 1.0, 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let listing = service
            .create_auction_listing(
                Profession::Inventory,
                "MarCorp Kallous-5, E.L.M Edition (L)",
                1.0,
                12.0,
                Some(15.0),
                1.04,
                Some("2026-08-13"),
                Some(7),
            )
            .await
            .unwrap();
        assert_eq!(listing.attributed_qty, 1.0);
        assert_eq!(listing.unattributed_qty, 0.0);
        assert_eq!(listing.attributed_tt, 1.0);
        let movement_listing_id = listing.id.clone();
        service
            .db
            .with_reader(move |conn| {
                let movement: (String, Option<String>, Option<i64>) = conn.query_row(
                    "SELECT source_kind, mob_species, session_definition_id \
                     FROM stock_movements WHERE ref_id = ? AND movement_kind = 'listing'",
                    rusqlite::params![movement_listing_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                assert_eq!(movement, ("hunt".to_string(), None, Some(7)));
                Ok(())
            })
            .await
            .unwrap();

        service
            .expire_auction_listing(&listing.id, Some("2026-08-13"))
            .await
            .unwrap()
            .expect("expired");
        let listing = service
            .create_auction_listing(
                Profession::Inventory,
                "MarCorp Kallous-5, E.L.M Edition (L)",
                1.0,
                12.0,
                Some(15.0),
                1.04,
                Some("2026-08-13"),
                Some(7),
            )
            .await
            .unwrap();
        assert_eq!(listing.attributed_tt, 1.0, "expiry preserved provenance");

        service
            .confirm_auction_listing(&listing.id, 15.0, 0.15, Some("2026-08-13"))
            .await
            .unwrap()
            .expect("confirmed");

        let species = service.realised_markup_by_species().await.unwrap();
        assert_eq!(species.len(), 1);
        assert_eq!(species[0].mob_species, "");
        assert!((species[0].net_markup - 12.81).abs() < 1e-9);
        let definitions = service.realised_markup_by_definition().await.unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].definition_id, 7);
        assert!((definitions[0].net_markup - 12.81).abs() < 1e-9);
    }

    /// A hunting conversion carries species provenance into the produced
    /// Nanocubes, so selling the cubes still credits the species; the cube
    /// pile shows on the hunting stock list through its owning conversion.
    #[tokio::test]
    async fn hunting_conversion_carries_species_provenance_forward() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;

        service
            .convert_stock(
                Profession::Hunting,
                "Animal Muscle Oil",
                "Nanocube",
                30.0,
                Some("2026-06-01"),
            )
            .await
            .unwrap();

        let stock = service.stock_positions(Profession::Hunting).await.unwrap();
        let cubes = position(&stock, "Nanocube").expect("cube position");
        // 30 units at 0.30 TT each is 9.00 PED, which is 900 cubes.
        assert!((cubes.quantity - 900.0).abs() < 1e-6);

        let listing = service
            .create_auction_listing(
                Profession::Hunting,
                "Nanocube",
                900.0,
                10.0,
                None,
                0.5,
                Some("2026-06-01"),
                None,
            )
            .await
            .unwrap();
        service
            .confirm_auction_listing(&listing.id, 10.0, 0.0, Some("2026-06-01"))
            .await
            .unwrap()
            .expect("confirmed");

        let realised = service.realised_markup_by_species().await.unwrap();
        assert_eq!(realised.len(), 1);
        assert_eq!(realised[0].mob_species, "Atrox");
        // 10.00 fetched over 9.00 TT less the 0.50 fee.
        assert!((realised[0].net_markup - 0.50).abs() < 1e-9);
        let definitions = service.realised_markup_by_definition().await.unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].definition_id, 7);
        assert!((definitions[0].net_markup - 0.50).abs() < 1e-9);
    }

    #[tokio::test]
    async fn private_trade_and_removal_change_stock_without_rewriting_loot() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;

        service
            .create_private_sale(
                Profession::Hunting,
                "Animal Muscle Oil",
                10.0,
                4.0,
                Some("2026-06-02"),
            )
            .await
            .unwrap();
        service
            .remove_stock(
                Profession::Hunting,
                "Animal Muscle Oil",
                5.0,
                Some("2026-06-03"),
            )
            .await
            .unwrap();

        let stock = service.stock_positions(Profession::Hunting).await.unwrap();
        assert_eq!(
            position(&stock, "Animal Muscle Oil").unwrap().quantity,
            45.0
        );
        let realised = service.realised_markup_by_species().await.unwrap();
        assert!((realised[0].net_markup - 1.0).abs() < 1e-9);
        let history = service.activity_history(Profession::Hunting).await.unwrap();
        assert_eq!(history[0].kind, "removal");
        assert_eq!(history[1].kind, "trade");
        assert_eq!(history[1].net_markup, Some(1.0));

        assert!(service.undo_stock_removal(&history[0].id).await.unwrap());
        assert!(service.undo_private_sale(&history[1].id).await.unwrap());
        let restored = service.stock_positions(Profession::Hunting).await.unwrap();
        assert_eq!(
            position(&restored, "Animal Muscle Oil").unwrap().quantity,
            60.0
        );
        assert!(service
            .realised_markup_by_species()
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn deliberate_shrapnel_conversion_realises_the_fixed_margin() {
        let (_dir, service) = write_service().await;
        seed_hunting_scenario(&service).await;
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO kill_loot_items \
                     (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) \
                     VALUES ('k1', 'Shrapnel', 5000, 5.0, 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        service
            .convert_shrapnel(Profession::Hunting, 10_000.0, Some("2026-06-04"))
            .await
            .unwrap();

        let stock = service.stock_positions(Profession::Hunting).await.unwrap();
        let shrapnel = position(&stock, "Shrapnel").expect("depleted position remains visible");
        assert!((shrapnel.quantity - 0.0).abs() < 1e-9, "{shrapnel:?}");
        assert!((shrapnel.tt_value - 0.0).abs() < 1e-9, "{shrapnel:?}");
        assert!(position(&stock, "Universal Ammo").is_none());

        let ledger_gain: f64 = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT amount FROM ledger_entries WHERE tag = 'convert'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert!((ledger_gain - 0.10).abs() < 1e-9);
        // Half the converted pile was enhancer rebate stock and therefore
        // unattributed. Only the non-enhancer half reaches Hunting realised.
        let realised = service.realised_markup_by_species().await.unwrap();
        assert!((realised[0].net_markup - 0.05).abs() < 1e-9);
        let activity = service.hunting_activity("all").await.unwrap();
        let family = activity.definitions[0]
            .activities
            .iter()
            .find(|row| row.kind == "quest_family")
            .expect("activity carrying the converted loot");
        assert!((family.realised_loot_markup - 0.05).abs() < 1e-9);
        assert!(family.realised_reward_markup.abs() < 1e-9);

        let conversion = service
            .activity_history(Profession::Hunting)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "conversion")
            .expect("conversion history");
        assert!((conversion.net_markup.expect("conversion gain") - 0.10).abs() < 1e-9);
        assert!(service.undo_stock_conversion(&conversion.id).await.unwrap());
        assert!(service
            .realised_markup_by_species()
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn quest_reward_stock_enters_inventory_and_zero_tt_sale_keeps_provenance() {
        let (_dir, service) = write_service().await;
        service
            .db
            .with_writer(|conn| {
                conn.execute("INSERT INTO quests(id, name) VALUES(41, 'AI Daily')", [])?;
                conn.execute(
                    "INSERT INTO quest_runs(id, quest_id, status, started_at, completed_at) \
                     VALUES(42, 41, 'completed', 100, 200)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completions \
                     (id, session_id, quest_id, completed_at, reward_source, reward_kind, \
                      reward_outcome, reward_policy_snapshot, quest_run_id) \
                     VALUES(43, 'manual-quest', 41, 200, 'tracked_loot', 'item', \
                            'confirmed', 'completion_clump', 42)",
                    [],
                )?;
                conn.execute("UPDATE quest_runs SET completion_id = 43 WHERE id = 42", [])?;
                conn.execute(
                    "INSERT INTO session_quest_completion_reward_items \
                     (id, completion_id, item_name, quantity, value_ped, accounting_kind) \
                     VALUES(44, 43, 'AI Daily Voucher', 1, 0, 'stock')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let held = service
            .stock_positions(Profession::Inventory)
            .await
            .unwrap();
        let voucher = position(&held, "AI Daily Voucher").expect("reward enters Inventory");
        assert_eq!(voucher.quantity, 1.0);
        assert_eq!(voucher.tt_value, 0.0);

        service
            .create_private_sale(
                Profession::Inventory,
                "AI Daily Voucher",
                1.0,
                2.0,
                Some("2026-06-02"),
            )
            .await
            .unwrap();

        let proof = service
            .db
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT ps.attributed_qty, ps.tt_value, le.amount, m.source_kind, \
                            m.quest_reward_item_id, m.quest_run_id, m.quest_id, m.quantity \
                     FROM private_sales ps \
                     JOIN ledger_entries le ON le.id = ps.sale_entry_id \
                     JOIN stock_movements m ON m.ref_id = ps.id AND m.movement_kind = 'trade'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, f64>(0)?,
                            row.get::<_, f64>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, f64>(7)?,
                        ))
                    },
                )
                .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(
            proof,
            (1.0, 0.0, 2.0, "quest".to_string(), 44, 42, 41, -1.0)
        );
    }
}
