//! The analytics family: the Overview and Activity aggregates, the ledger
//! (keyset-paginated list + create/delete), the ledger presets, and the
//! inventory ledger (list / create / patch / delete / sell).
//!
//! The computation lives in [`eo_services::analytics::AnalyticsService`]
//! (shared with the guide-mode demo surface); this facade is the typed
//! boundary over it. The service returns typed aggregates and rows, and
//! the facade maps them field by field onto the declared DTOs, so the
//! wire shape is single-sourced here and the mapping is compiler-checked.
//!
//! One contract movement rides this migration, ratified under ADR-0019:
//! the Overview's numeric fields are typed `f64`, so the pydantic-era
//! `Any`-passthrough integers (the empty-window `cycledBreakdown` zeros
//! and any all-integer bucket) render as JSON floats (`0` -> `0.0`).
//! Numerically identical, and the values are floats over any non-empty
//! window (so the demo surface and every populated read are byte-stable);
//! only the all-empty case shifts.
//!
//! The ledger list folds the transport's `X-Next-Cursor` header into the
//! return DTO ([`LedgerPage`]): a typed command answers one structured
//! payload, so the cursor travels in the body, not a header. The delete
//! operations return no body (the transport's `{"status":"deleted"}`
//! acknowledgement retires with no consumer).

use std::collections::BTreeMap;

use eo_services::analytics::AnalyticsError;
use eo_wire::normalizer::round_half_even;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Nullable;
use crate::{Api, ApiError};

// ── Overview response DTOs ──────────────────────────────────────────

/// The liquid + progression returns breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReturnsBreakdown {
    pub loot_tt: f64,
    pub quest_item_tt: f64,
    pub pes: f64,
    pub codex_pes: f64,
    pub quest_pes: f64,
    pub ledger: BTreeMap<String, f64>,
}

/// The per-family cycled-cost split.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CycledBreakdown {
    pub weapon: f64,
    pub healing: f64,
    pub enhancer: f64,
    pub armour: f64,
    pub dangling: f64,
    /// Consumed doses of cost-tracked items.
    pub consumables: f64,
}

/// The losses breakdown: tracking cost, its cycled split, and the ledger
/// expenses.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LossesBreakdown {
    pub tracking_cost: f64,
    pub cycled_breakdown: CycledBreakdown,
    pub ledger: BTreeMap<String, f64>,
}

/// One day of the Overview timeline.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineDay {
    pub date: String,
    pub loot_tt: f64,
    pub quest_item_tt: f64,
    pub pes: f64,
    pub codex_pes: f64,
    pub quest_pes: f64,
    pub ledger_gains: BTreeMap<String, f64>,
    pub tracking_cost: f64,
    pub ledger_losses: BTreeMap<String, f64>,
}

/// One month of the Overview monthly breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MonthlyEntry {
    pub month: String,
    pub loot_tt: f64,
    pub quest_item_tt: f64,
    pub pes: f64,
    pub codex_pes: f64,
    pub quest_pes: f64,
    pub ledger_gains: BTreeMap<String, f64>,
    pub tracking_cost: f64,
    pub ledger_losses: BTreeMap<String, f64>,
}

/// The Overview headline trend, as the service computes it. The
/// serialised forms are byte-identical to the strings they replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    Improving,
    Declining,
    Stable,
}

/// A ledger entry's accounting class. Writes validate to exactly these
/// two values; the read side classifies anything else as an expense,
/// mirroring the binary styling check the frontend has always applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LedgerKind {
    Expense,
    Markup,
}

impl LedgerKind {
    fn classify(kind: &str) -> Self {
        if kind == "markup" {
            Self::Markup
        } else {
            Self::Expense
        }
    }
}

/// The Overview aggregate: the total return rate and trend, the returns /
/// losses breakdowns, the totals, and the day / month timelines.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsOverview {
    pub total_return_rate: f64,
    pub trend: Trend,
    pub returns_breakdown: ReturnsBreakdown,
    pub losses_breakdown: LossesBreakdown,
    pub total_gains: f64,
    pub total_losses: f64,
    pub timeline: Vec<TimelineDay>,
    pub monthly_breakdown: Vec<MonthlyEntry>,
}

// ── Activity response DTOs ──────────────────────────────────────────

/// One row of the per-mob activity comparison.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobComparison {
    pub mob_name: String,
    pub sessions: i64,
    pub kills: i64,
    pub hours: f64,
    pub cycled: f64,
    pub pes_per100_ped: f64,
    pub loot_rate: f64,
}

/// One row of the per-session-name activity comparison (the designated
/// axis; legacy tag-mode sessions appear under their migrated names).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NameComparison {
    pub session_name: String,
    pub sessions: i64,
    pub kills: i64,
    pub hours: f64,
    pub cycled: f64,
    pub pes_per100_ped: f64,
    pub loot_rate: f64,
}

/// The Hunting aggregate: the per-mob and per-session-name comparison
/// tables (the observed and designated axes).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsHunting {
    pub mob_comparisons: Vec<MobComparison>,
    pub name_comparisons: Vec<NameComparison>,
}

/// The activity family a stock action belongs to. Closed vocabulary: the
/// auction and conversion lifecycle is shared, and the profession stamp is
/// what scopes each activity's Market and History to its own records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Profession {
    Harvesting,
    Hunting,
    Inventory,
}

impl From<Profession> for eo_services::analytics::Profession {
    fn from(value: Profession) -> Self {
        match value {
            Profession::Harvesting => Self::Harvesting,
            Profession::Hunting => Self::Hunting,
            Profession::Inventory => Self::Inventory,
        }
    }
}

// ── Revamped Hunting DTOs ───────────────────────────────────────────

/// The revamped Hunting aggregate: direct headline figures, the
/// definition-keyed Sessions axis, and the observed Targets axis. All
/// figures are DIRECT (weapon + enhancer cost at kill grain, loot TT,
/// session-grain activity skill); heal and armour stay session-grain
/// residues reported on Dashboard and Overview, never allocated into only
/// some comparison rows.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsHuntingActivity {
    pub overall: HuntingActivityOverall,
    pub definitions: Vec<HuntingDefinitionComparison>,
    pub species: Vec<HuntingSpeciesComparison>,
}

/// The whole activity's direct headline figures for the period.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HuntingActivityOverall {
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub expected: Nullable<ExpectedHuntingEconomics>,
}

/// Offensive-only Community Model v1 result over captured component evidence.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedHuntingEconomics {
    pub model_version: String,
    pub looter_source: String,
    pub looter_level: f64,
    pub expected_loot_tt: f64,
    pub modelled_raw_tt: f64,
    pub eligible_offensive_cost: f64,
    pub offensive_tt_recovery: f64,
    pub expected_tt_rate: f64,
    pub effective_efficiency: crate::equipment::EquipmentEffectiveEfficiency,
    pub break_even_loot_markup: f64,
    pub coverage: f64,
    pub incomplete: bool,
    pub missing_basis_phases: i64,
}

/// One session definition's aggregate over its hunted instances; the
/// unassigned bucket carries a null `definitionId`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HuntingDefinitionComparison {
    pub definition_id: Nullable<i64>,
    pub name: String,
    pub is_archived: bool,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub expected: Nullable<ExpectedHuntingEconomics>,
    pub loot_items: Vec<HarvestLootItem>,
    pub activities: Vec<HuntingActivityComparison>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HuntingActivityKind {
    Quest,
    QuestFamily,
    Segment,
    Bundle,
    Ambient,
}

impl HuntingActivityKind {
    fn classify(kind: &str) -> Self {
        match kind {
            "quest" => Self::Quest,
            "quest_family" => Self::QuestFamily,
            "segment" => Self::Segment,
            "bundle" => Self::Bundle,
            _ => Self::Ambient,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HuntingRewardStatus {
    None,
    IncludedInLoot,
    FixedLiquid,
    Item,
    Skill,
    Mixed,
    Unverified,
}

impl HuntingRewardStatus {
    fn classify(status: &str) -> Self {
        match status {
            "included_in_loot" => Self::IncludedInLoot,
            "fixed_liquid" => Self::FixedLiquid,
            "item" => Self::Item,
            "skill" => Self::Skill,
            "mixed" => Self::Mixed,
            "unverified" => Self::Unverified,
            _ => Self::None,
        }
    }
}

/// One exact declared activity signature inside a session definition.
/// Costs and loot are partitioned by the context stamped at capture; a
/// separately confirmed liquid reward is additive exactly once.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HuntingActivityComparison {
    pub kind: HuntingActivityKind,
    pub label: String,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub expected: Nullable<ExpectedHuntingEconomics>,
    pub confirmed_reward_ped: f64,
    pub realised_reward_markup: f64,
    /// Actual reward items observed at completion. Their markup stays a
    /// current market projection and never enters realised accounting.
    pub reward_items: Vec<HarvestLootItem>,
    /// Historical wire name for total realised returns at this activity
    /// grain: ordinary loot TT, confirmed reward TT, and later confirmed
    /// markup from both ordinary loot and reward stock.
    pub rewarded_returns: f64,
    pub rewarded_rate: f64,
    pub reward_status: HuntingRewardStatus,
    pub loot_items: Vec<HarvestLootItem>,
    pub variants: Vec<HuntingActivityComparison>,
}

/// One observed species' aggregate; the unclassified bucket carries an
/// empty species.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HuntingSpeciesComparison {
    pub mob_species: String,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub expected: Nullable<ExpectedHuntingEconomics>,
    pub loot_items: Vec<HarvestLootItem>,
}

/// One mob species' net realised markup from confirmed stock outcomes: the Hunting
/// sibling of [`RealisedTierMarkup`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealisedSpeciesMarkup {
    pub mob_species: String,
    pub net_markup: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealisedDefinitionMarkup {
    pub definition_id: i64,
    pub net_markup: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HuntingRealisedMarkup {
    pub species: Vec<RealisedSpeciesMarkup>,
    pub definitions: Vec<RealisedDefinitionMarkup>,
}

/// One item in an activity's harvest loot composition: realised TT only.
/// The market markup column is merged in at the frontend from the
/// market layer, never joined into this accounting DTO.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestLootItem {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

/// Tree Cutting's durable source-activity vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarvestYieldTier {
    Short,
    Long,
    Huge,
    Unknown,
}

impl From<eo_services::harvest_yield::HarvestYieldTier> for HarvestYieldTier {
    fn from(value: eo_services::harvest_yield::HarvestYieldTier) -> Self {
        match value {
            eo_services::harvest_yield::HarvestYieldTier::Short => Self::Short,
            eo_services::harvest_yield::HarvestYieldTier::Long => Self::Long,
            eo_services::harvest_yield::HarvestYieldTier::Huge => Self::Huge,
            eo_services::harvest_yield::HarvestYieldTier::Unknown => Self::Unknown,
        }
    }
}

/// One effective yield activity and the loot it produced.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestTierComparison {
    pub yield_tier: HarvestYieldTier,
    pub swings: i64,
    pub cycled: f64,
    pub returns: f64,
    pub loot_rate: f64,
    pub loot_items: Vec<HarvestLootItem>,
}

/// The Tree Cutting aggregate: the tier-first comparison table.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsHarvest {
    pub tier_comparisons: Vec<HarvestTierComparison>,
}

// ── Ledger / preset / inventory DTOs ────────────────────────────────

/// One ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LedgerItem {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub kind: LedgerKind,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// The whole-ledger summary for a period: the per-tag markup (gain) and
/// expense (loss) totals over every entry in the window, independent of
/// the paginated list. The Ledger tab's net-impact and source cards read
/// this instead of folding the loaded page window.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerSummary {
    pub gains: BTreeMap<String, f64>,
    pub losses: BTreeMap<String, f64>,
}

/// A page of ledger entries plus the opaque cursor for the next page
/// (`null` on the last page) and the whole-ledger row count, so a pager
/// can report true bounds while loading windows on demand: the keyset
/// `X-Next-Cursor` header folded into the typed return, since a command
/// answers one structured payload.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerPage {
    pub entries: Vec<LedgerItem>,
    pub next_cursor: Nullable<String>,
    pub total: i64,
}

/// One ledger preset (a reusable ledger-entry template).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LedgerPreset {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: LedgerKind,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// One inventory item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub id: String,
    pub name: String,
    pub tt_value: f64,
    pub markup_paid: f64,
    pub notes: Nullable<String>,
    pub acquired_at: String,
}

/// The result of selling an inventory item: the emitted ledger entry
/// (`null` for a zero-delta sale) and the sold item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventorySellResult {
    pub ledger_entry: Nullable<LedgerItem>,
    pub sold_item: InventoryItem,
}

/// One canonical item the player currently holds: recorded loot still in
/// hand after everything that has left through a listing or a conversion,
/// and back through an expiry. Position context only: it never feeds market
/// opportunity or its confidence levels, which stay holding-independent.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockPosition {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Quantity sitting in an unresolved auction listing. Already out of
    /// `quantity`, since listed stock has left the player's inventory in
    /// game, but reported so it does not read as simply gone.
    pub listed_quantity: f64,
}

/// One auction listing across its lifecycle. Realised figures are `null`
/// until the listing is confirmed sold: an open auction has no price yet,
/// and an expired one never realised anything.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionListing {
    pub id: String,
    pub item_name: String,
    pub quantity: f64,
    pub attributed_qty: f64,
    pub unattributed_qty: f64,
    pub tt_value: f64,
    pub attributed_tt: f64,
    pub starting_bid: f64,
    pub buyout: Nullable<f64>,
    pub listing_fee: f64,
    pub listed_at: String,
    pub status: String,
    pub final_price: Nullable<f64>,
    pub sale_fee: Nullable<f64>,
    pub resolved_at: Nullable<String>,
    pub subject_kind: String,
    pub inventory_item_id: Nullable<String>,
    pub cost_basis: Nullable<f64>,
    pub channel: String,
    /// How many days the listing was posted for. Null for a listing recorded
    /// before durations were captured; no deadline is invented for it.
    pub auction_days: Nullable<i64>,
    /// The moment the listing runs out, as a UTC timestamp: the instant it
    /// was posted plus `auction_days`, so it carries a time of day and not
    /// only a date. Null whenever the duration or the starting instant is
    /// unknown. Compare it as an instant; treating it as a calendar date
    /// mixes it with whatever local date the reader is on.
    pub expires_at: Nullable<String>,
    /// Net markup the activity may claim, after both auction fees and after
    /// removing the share covered by untracked stock.
    pub activity_net_markup: Nullable<f64>,
    /// Sale proceeds above the listing's TT, before fees.
    pub gross_markup: Nullable<f64>,
}

/// One thing an activity did to its stock: a listing across its whole
/// lifecycle, a private trade, a conversion, or a stock-only removal.
///
/// A listing appears once however far it has got. Creating and selling are the
/// same listing at two moments, not two entries.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHistoryEntry {
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
    /// What a conversion produced; `null` for other outcomes.
    pub target_item: Nullable<String>,
    /// When a listing resolved, or when it was listed if it has not; when a
    /// conversion happened.
    pub occurred_at: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Realised outcomes only: the net gain, and the part an activity may claim.
    pub net_markup: Nullable<f64>,
    pub activity_net_markup: Nullable<f64>,
    /// Whether the sale can be taken back, leaving the listing open.
    pub can_revert_sale: bool,
    /// Whether the entry can be undone outright, returning any stock it took.
    pub can_delete: bool,
    /// Why not, when it cannot, in terms a reader can act on.
    pub undo_blocked_reason: Nullable<String>,
    /// Already undone. Every effect it had is reversed and the entry is kept
    /// as the read-only record of a correction.
    pub undone: bool,
}

/// An undo payload: the history entry to take back.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityUndoInput {
    pub id: String,
}

/// One yield tier's net realised markup from confirmed stock outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealisedTierMarkup {
    pub yield_tier: HarvestYieldTier,
    pub net_markup: f64,
}

// ── Request DTOs ────────────────────────────────────────────────────

/// An auction-listing creation payload. Dates are optional and default to
/// today; the fee is what the game quoted at listing time. The profession
/// stamps which activity's Market owns the listing.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionListingInput {
    pub profession: Profession,
    pub item_name: String,
    pub quantity: f64,
    pub starting_bid: f64,
    pub buyout: Option<f64>,
    pub listing_fee: f64,
    pub listed_at: Option<String>,
    /// How many days the listing runs for. Optional: a listing whose duration
    /// the player did not record simply never nudges for resolution.
    pub auction_days: Option<i64>,
}

/// A sale-confirmation payload: the price the auction actually fetched and
/// the additional fee charged at the point of sale.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionConfirmInput {
    pub listing_id: String,
    pub final_price: f64,
    pub sale_fee: f64,
    pub resolved_at: Option<String>,
}

/// An expiry payload: the listing came back unsold.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionExpireInput {
    pub listing_id: String,
    pub resolved_at: Option<String>,
}

/// A stock-conversion payload (recycling into Nanocubes at 1:1 TT). The
/// profession stamps which activity's History owns the conversion.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockConversionInput {
    pub profession: Profession,
    pub source_item: String,
    pub target_item: String,
    pub quantity: f64,
    pub converted_at: Option<String>,
}

/// A completed player-to-player trade. Unlike an auction listing, its price
/// and outcome are already known and no fee is involved.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrivateSaleInput {
    pub profession: Profession,
    pub item_name: String,
    pub quantity: f64,
    pub sold_for: f64,
    pub sold_at: Option<String>,
}

/// Stock whose later fate is unknown. This changes holdings only and never
/// rewrites the loot or ledger history that established its TT.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockRemovalInput {
    pub profession: Profession,
    pub item_name: String,
    pub quantity: f64,
    pub removed_at: Option<String>,
}

/// Deliberate Shrapnel conversion at the game's fixed 100:101 ratio.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShrapnelConversionInput {
    pub profession: Profession,
    pub quantity: f64,
    pub converted_at: Option<String>,
}

/// A ledger-entry create payload.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LedgerEntryInput {
    pub date: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// A ledger-preset create payload.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LedgerPresetInput {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub amount: f64,
    pub tag: String,
}

/// An inventory-item create payload (snake_case, matching the frontend
/// request shape). `notes` / `acquired_at` are optional; an empty / absent
/// `acquired_at` defaults to today's UTC date.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InventoryItemInput {
    pub name: String,
    pub tt_value: f64,
    pub markup_paid: f64,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub acquired_at: Option<String>,
}

/// An inventory-item patch: only present (`Some`) fields update.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InventoryPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tt_value: Option<f64>,
    #[serde(default)]
    pub markup_paid: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// An inventory-sale payload.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InventorySellInput {
    pub sale_price: f64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sold_at: Option<String>,
}

/// An auction draft for one whole capital-equipment position.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentListingInput {
    pub item_id: String,
    pub starting_bid: f64,
    pub buyout: Option<f64>,
    pub listing_fee: f64,
    pub listed_at: Option<String>,
    /// How many days the listing runs for, when recorded.
    pub auction_days: Option<i64>,
}

/// A completed fee-free player trade for one whole capital position.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquipmentTradeInput {
    pub item_id: String,
    pub sold_for: f64,
    pub sold_at: Option<String>,
}

/// An intake-neutral transaction draft. Manual forms create this shape now;
/// a future OCR adapter can populate the same fields with its observed values
/// and confidence without gaining a second commit path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventorySaleDraft {
    pub draft_id: String,
    /// `manual` or `ocr`; descriptive provenance, never authorisation.
    pub source: String,
    /// `auction` or `trade`.
    pub channel: String,
    pub observed_name: String,
    pub quantity: Nullable<f64>,
    pub starting_bid: Nullable<f64>,
    pub buyout: Nullable<f64>,
    pub listing_fee: Nullable<f64>,
    pub final_price: Nullable<f64>,
    /// How many days an auction listing runs for.
    pub auction_days: Nullable<i64>,
    /// OCR may provide a field-level confidence. Manual drafts use null.
    pub confidence: Nullable<f64>,
}

/// What one look at the game's sale window resolved.
/// Every field is nullable because every field can refuse: a value that
/// did not read comes back empty and is named in `unread`, so the review
/// surface shows a gap to fill rather than a figure to trust. `error` is
/// set only when there was nothing to read at all (no game window, no
/// calibration, no capture), and then no field is populated.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaleWindowCapture {
    pub observed_name: Nullable<String>,
    pub quantity: Nullable<f64>,
    pub tt_value: Nullable<f64>,
    pub listing_fee: Nullable<f64>,
    pub auction_days: Nullable<i64>,
    pub starting_bid: Nullable<f64>,
    pub buyout: Nullable<f64>,
    /// The lowest confidence among the fields that did read.
    pub confidence: Nullable<f64>,
    /// The fields that did not read, by the name the window gives them.
    pub unread: Vec<String>,
    pub error: Nullable<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryHoldingCandidate {
    pub kind: String,
    pub holding_id: String,
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryDraftResolution {
    pub draft: InventorySaleDraft,
    pub candidates: Vec<InventoryHoldingCandidate>,
    /// Set only for an exact normalised name with one eligible holding, or a
    /// high-confidence fuzzy winner separated clearly from the runner-up.
    pub resolved: Nullable<InventoryHoldingCandidate>,
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// The Overview aggregate for a named period (`30d` / `90d` / `1y`, or
    /// all-time for any other value).
    pub async fn analytics_overview(&self, period: &str) -> Result<AnalyticsOverview, ApiError> {
        let value = self
            .analytics
            .overview(period)
            .await
            .map_err(analytics_error("analytics overview"))?;
        Ok(overview_dto(value))
    }

    /// The Hunting aggregate: the per-mob / per-tag tables.
    pub async fn analytics_hunting(&self) -> Result<AnalyticsHunting, ApiError> {
        let value = self
            .analytics
            .hunting()
            .await
            .map_err(analytics_error("analytics hunting"))?;
        Ok(hunting_dto(value))
    }

    /// The Tree Cutting aggregate for a named period: effective yield
    /// tiers, each with its loot composition.
    pub async fn analytics_harvest(&self, period: &str) -> Result<AnalyticsHarvest, ApiError> {
        let value = self
            .analytics
            .harvest(period)
            .await
            .map_err(analytics_error("analytics harvest"))?;
        Ok(harvest_dto(value))
    }

    /// The revamped Hunting aggregate for a named period: the direct
    /// headline figures, the definition-keyed Sessions axis, and the
    /// observed Targets axis.
    pub async fn analytics_hunting_activity(
        &self,
        period: &str,
    ) -> Result<AnalyticsHuntingActivity, ApiError> {
        let value = self
            .analytics
            .hunting_activity(period)
            .await
            .map_err(analytics_error("analytics hunting activity"))?;
        Ok(hunting_activity_dto(value))
    }

    /// One activity's current holdings. Operational position context for
    /// sale and recycling actions; it does not influence
    /// holding-independent market opportunity.
    pub async fn activity_stock(
        &self,
        profession: Profession,
    ) -> Result<Vec<StockPosition>, ApiError> {
        let rows = self
            .analytics
            .stock_positions(profession.into())
            .await
            .map_err(analytics_error("activity stock"))?;
        Ok(rows
            .into_iter()
            .map(|row| StockPosition {
                item_name: row.item_name,
                quantity: row.quantity,
                tt_value: row.tt_value,
                listed_quantity: row.listed_quantity,
            })
            .collect())
    }

    /// One activity's auction listings, unresolved first.
    pub async fn auction_listings(
        &self,
        profession: Profession,
    ) -> Result<Vec<AuctionListing>, ApiError> {
        let rows = self
            .analytics
            .auction_listings(profession.into())
            .await
            .map_err(analytics_error("auction listings"))?;
        Ok(rows.into_iter().map(auction_listing_dto).collect())
    }

    /// List stock on the auction: the quantity leaves holdings now and the
    /// starting-bid fee is spent now; nothing is realised until it sells.
    pub async fn auction_listing_create(
        &self,
        input: AuctionListingInput,
    ) -> Result<AuctionListing, ApiError> {
        let row = self
            .analytics
            .create_auction_listing(
                input.profession.into(),
                &input.item_name,
                input.quantity,
                input.starting_bid,
                input.buyout,
                input.listing_fee,
                input.listed_at.as_deref(),
                input.auction_days,
            )
            .await
            .map_err(analytics_error("auction listing create"))?;
        Ok(auction_listing_dto(row))
    }

    /// Confirm a listing sold at the price it fetched. The recognition
    /// boundary for realised markup. A listing that is missing or already
    /// resolved is a not-found.
    pub async fn auction_listing_confirm(
        &self,
        input: AuctionConfirmInput,
    ) -> Result<AuctionListing, ApiError> {
        self.analytics
            .confirm_auction_listing(
                &input.listing_id,
                input.final_price,
                input.sale_fee,
                input.resolved_at.as_deref(),
            )
            .await
            .map_err(analytics_error("auction listing confirm"))?
            .map(auction_listing_dto)
            .ok_or_else(|| ApiError::not_found("no unresolved listing with that id"))
    }

    /// Mark a listing expired: the stock returns, the fee stays spent, and
    /// nothing reaches the activity.
    pub async fn auction_listing_expire(
        &self,
        input: AuctionExpireInput,
    ) -> Result<AuctionListing, ApiError> {
        self.analytics
            .expire_auction_listing(&input.listing_id, input.resolved_at.as_deref())
            .await
            .map_err(analytics_error("auction listing expire"))?
            .map(auction_listing_dto)
            .ok_or_else(|| ApiError::not_found("no unresolved listing with that id"))
    }

    /// Recycle stock into another item at 1:1 TT, carrying its activity
    /// composition forward.
    pub async fn stock_convert(&self, input: StockConversionInput) -> Result<(), ApiError> {
        self.analytics
            .convert_stock(
                input.profession.into(),
                &input.source_item,
                &input.target_item,
                input.quantity,
                input.converted_at.as_deref(),
            )
            .await
            .map_err(analytics_error("stock convert"))?;
        Ok(())
    }

    pub async fn stock_private_sale(&self, input: PrivateSaleInput) -> Result<(), ApiError> {
        self.analytics
            .create_private_sale(
                input.profession.into(),
                &input.item_name,
                input.quantity,
                input.sold_for,
                input.sold_at.as_deref(),
            )
            .await
            .map_err(analytics_error("private sale"))
    }

    pub async fn stock_remove(&self, input: StockRemovalInput) -> Result<(), ApiError> {
        self.analytics
            .remove_stock(
                input.profession.into(),
                &input.item_name,
                input.quantity,
                input.removed_at.as_deref(),
            )
            .await
            .map_err(analytics_error("stock remove"))
    }

    pub async fn stock_shrapnel_convert(
        &self,
        input: ShrapnelConversionInput,
    ) -> Result<(), ApiError> {
        self.analytics
            .convert_shrapnel(
                input.profession.into(),
                input.quantity,
                input.converted_at.as_deref(),
            )
            .await
            .map_err(analytics_error("Shrapnel convert"))
    }

    /// Everything one activity has done to its stock, newest first, each
    /// entry carrying whether it can be taken back.
    pub async fn activity_history(
        &self,
        profession: Profession,
    ) -> Result<Vec<ActivityHistoryEntry>, ApiError> {
        let rows = self
            .analytics
            .activity_history(profession.into())
            .await
            .map_err(analytics_error("activity history"))?;
        Ok(rows
            .into_iter()
            .map(|row| ActivityHistoryEntry {
                id: row.id,
                subject_kind: row.subject_kind,
                channel: row.channel,
                kind: row.kind,
                status: row.status,
                item_name: row.item_name,
                target_item: row.target_item.into(),
                occurred_at: row.occurred_at,
                quantity: row.quantity,
                tt_value: row.tt_value,
                net_markup: row.net_markup.into(),
                activity_net_markup: row.activity_net_markup.into(),
                can_revert_sale: row.can_revert_sale,
                can_delete: row.can_delete,
                undo_blocked_reason: row.undo_blocked_reason.into(),
                undone: row.undone,
            })
            .collect())
    }

    /// Take back a confirmed sale, leaving the listing open. The stock does
    /// not move: it is still out on the auction. A listing that is missing or
    /// was never sold is a not-found.
    pub async fn auction_sale_revert(
        &self,
        input: ActivityUndoInput,
    ) -> Result<AuctionListing, ApiError> {
        self.analytics
            .revert_auction_sale(&input.id)
            .await
            .map_err(analytics_error("auction sale revert"))?
            .map(auction_listing_dto)
            .ok_or_else(|| ApiError::not_found("no sold listing with that id"))
    }

    /// Undo a listing: its stock comes back and every ledger row it wrote goes
    /// with it. The entry stays on file, marked.
    pub async fn auction_listing_undo(&self, input: ActivityUndoInput) -> Result<(), ApiError> {
        let existed = self
            .analytics
            .undo_auction_listing(&input.id)
            .await
            .map_err(analytics_error("auction listing undo"))?;
        if existed {
            Ok(())
        } else {
            Err(ApiError::not_found("no listing with that id to undo"))
        }
    }

    /// Undo a conversion: what it consumed comes back and what it produced is
    /// unmade. Refused when those produced units have since left.
    pub async fn stock_conversion_undo(&self, input: ActivityUndoInput) -> Result<(), ApiError> {
        let existed = self
            .analytics
            .undo_stock_conversion(&input.id)
            .await
            .map_err(analytics_error("stock conversion undo"))?;
        if existed {
            Ok(())
        } else {
            Err(ApiError::not_found("no conversion with that id to undo"))
        }
    }

    pub async fn private_sale_undo(&self, input: ActivityUndoInput) -> Result<(), ApiError> {
        if self
            .analytics
            .undo_private_sale(&input.id)
            .await
            .map_err(analytics_error("private sale undo"))?
        {
            Ok(())
        } else {
            Err(ApiError::not_found("no private sale with that id to undo"))
        }
    }

    pub async fn stock_removal_undo(&self, input: ActivityUndoInput) -> Result<(), ApiError> {
        if self
            .analytics
            .undo_stock_removal(&input.id)
            .await
            .map_err(analytics_error("stock removal undo"))?
        {
            Ok(())
        } else {
            Err(ApiError::not_found("no stock removal with that id to undo"))
        }
    }

    /// Net realised markup per yield tier, from confirmed stock outcomes.
    pub async fn harvest_realised_markup(&self) -> Result<Vec<RealisedTierMarkup>, ApiError> {
        let rows = self
            .analytics
            .realised_markup_by_tier()
            .await
            .map_err(analytics_error("harvest realised markup"))?;
        Ok(rows
            .into_iter()
            .map(|row| RealisedTierMarkup {
                yield_tier: row.yield_tier.into(),
                net_markup: row.net_markup,
            })
            .collect())
    }

    /// Net realised Hunting markup through both honest comparison axes.
    pub async fn hunting_realised_markup(&self) -> Result<HuntingRealisedMarkup, ApiError> {
        let (species, definitions) = tokio::try_join!(
            self.analytics.realised_markup_by_species(),
            self.analytics.realised_markup_by_definition(),
        )
        .map_err(analytics_error("hunting realised markup"))?;
        Ok(HuntingRealisedMarkup {
            species: species
                .into_iter()
                .map(|row| RealisedSpeciesMarkup {
                    mob_species: row.mob_species,
                    net_markup: row.net_markup,
                })
                .collect(),
            definitions: definitions
                .into_iter()
                .map(|row| RealisedDefinitionMarkup {
                    definition_id: row.definition_id,
                    net_markup: row.net_markup,
                })
                .collect(),
        })
    }

    /// One keyset page of ledger entries (newest first) plus the cursor for
    /// the next page. A malformed cursor is a bad-request.
    pub async fn ledger_list(
        &self,
        cursor: Option<String>,
        limit: Option<i64>,
    ) -> Result<LedgerPage, ApiError> {
        let page = self
            .analytics
            .list_ledger(cursor.as_deref(), limit)
            .await
            .map_err(analytics_error("ledger list"))?;
        Ok(LedgerPage {
            entries: page.entries.into_iter().map(ledger_item_dto).collect(),
            next_cursor: page.next_cursor.into(),
            total: page.total,
        })
    }

    /// The whole-ledger per-tag summary for a named period (`30d` / `90d` /
    /// `1y`, or all-time for any other value).
    pub async fn ledger_summary(&self, period: &str) -> Result<LedgerSummary, ApiError> {
        let summary = self
            .analytics
            .ledger_summary(period)
            .await
            .map_err(analytics_error("ledger summary"))?;
        Ok(LedgerSummary {
            gains: summary.gains,
            losses: summary.losses,
        })
    }

    /// Create a ledger entry (relanding its day's rollup).
    pub async fn ledger_create(&self, entry: LedgerEntryInput) -> Result<LedgerItem, ApiError> {
        let value = self
            .analytics
            .create_ledger_entry(
                &entry.date,
                &entry.kind,
                &entry.description,
                entry.amount,
                &entry.tag,
            )
            .await
            .map_err(analytics_error("ledger create"))?;
        Ok(ledger_item_dto(value))
    }

    /// Delete a ledger entry; a missing entry is a not-found.
    pub async fn ledger_delete(&self, entry_id: String) -> Result<(), ApiError> {
        match self
            .analytics
            .delete_ledger_entry(&entry_id)
            .await
            .map_err(analytics_error("ledger delete"))?
        {
            true => Ok(()),
            false => Err(ApiError::not_found("Entry not found")),
        }
    }

    /// The ledger presets.
    pub async fn ledger_presets_list(&self) -> Result<Vec<LedgerPreset>, ApiError> {
        let rows = self
            .analytics
            .list_ledger_presets()
            .await
            .map_err(analytics_error("ledger presets list"))?;
        Ok(rows.into_iter().map(ledger_preset_dto).collect())
    }

    /// Create a ledger preset; an invalid type is a bad-request.
    pub async fn ledger_preset_create(
        &self,
        preset: LedgerPresetInput,
    ) -> Result<LedgerPreset, ApiError> {
        let value = self
            .analytics
            .create_ledger_preset(
                &preset.name,
                &preset.kind,
                &preset.description,
                preset.amount,
                &preset.tag,
            )
            .await
            .map_err(analytics_error("ledger preset create"))?;
        Ok(ledger_preset_dto(value))
    }

    /// Delete a ledger preset; a missing preset is a not-found.
    pub async fn ledger_preset_delete(&self, preset_id: String) -> Result<(), ApiError> {
        match self
            .analytics
            .delete_ledger_preset(&preset_id)
            .await
            .map_err(analytics_error("ledger preset delete"))?
        {
            true => Ok(()),
            false => Err(ApiError::not_found("Preset not found")),
        }
    }

    /// The inventory items, newest acquisition first.
    pub async fn inventory_list(&self) -> Result<Vec<InventoryItem>, ApiError> {
        let rows = self
            .analytics
            .list_inventory()
            .await
            .map_err(analytics_error("inventory list"))?;
        Ok(rows.into_iter().map(inventory_item_dto).collect())
    }

    /// Read the game's auction sale window once and answer what it said.
    ///
    /// This fills a form; it never commits. Capture and typing converge on
    /// the same draft and meet the same checks, so a misread cannot reach
    /// the ledger by a path a typo could not.
    ///
    /// Synchronous, like the repair read: the capture blocks on the portal,
    /// so its caller must offload it rather than run it on a runtime worker.
    pub fn inventory_sale_window_capture(&self) -> Result<SaleWindowCapture, ApiError> {
        let read = self.sale_window_ocr.scan_sale_window();
        let text = |key: &str| {
            read.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .into()
        };
        let number = |key: &str| read.get(key).and_then(serde_json::Value::as_f64).into();
        let capture = SaleWindowCapture {
            observed_name: text("item_name"),
            quantity: number("quantity"),
            tt_value: number("tt_value"),
            // The window calls it the auction fee; the ledger has always
            // called it the listing fee. Same money.
            listing_fee: number("auction_fee"),
            auction_days: read
                .get("auction_days")
                .and_then(serde_json::Value::as_i64)
                .into(),
            starting_bid: number("starting_bid"),
            buyout: number("buyout"),
            confidence: number("confidence"),
            unread: read
                .get("unread")
                .and_then(serde_json::Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            error: text("error"),
        };
        // Held for the form to collect, since the overlay's button can be
        // pressed while the form is not on screen.
        if let Ok(mut slot) = self.last_sale_capture.lock() {
            *slot = Some(capture.clone());
        }
        Ok(capture)
    }

    /// Collect the last sale-window read, clearing it.
    pub fn inventory_sale_window_take_capture(&self) -> Nullable<SaleWindowCapture> {
        self.last_sale_capture
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .into()
    }

    /// Resolve a manual or OCR-originated transaction draft against current
    /// holdings. Resolution is conservative: ambiguity stays visible for the
    /// review surface instead of silently choosing a cost basis.
    pub async fn inventory_draft_resolve(
        &self,
        draft: InventorySaleDraft,
    ) -> Result<InventoryDraftResolution, ApiError> {
        if !matches!(draft.source.as_str(), "manual" | "ocr") {
            return Err(ApiError::bad_request("source must be 'manual' or 'ocr'"));
        }
        if !matches!(draft.channel.as_str(), "auction" | "trade") {
            return Err(ApiError::bad_request(
                "channel must be 'auction' or 'trade'",
            ));
        }
        let candidates: Vec<InventoryHoldingCandidate> = self
            .analytics
            .resolve_inventory_name(&draft.observed_name)
            .await
            .map_err(analytics_error("inventory draft resolve"))?
            .into_iter()
            .map(|row| InventoryHoldingCandidate {
                kind: row.kind,
                holding_id: row.holding_id,
                name: row.name,
                score: row.score,
            })
            .collect();
        let resolved = candidates.first().cloned().filter(|winner| {
            let runner_up = candidates.get(1).map(|row| row.score).unwrap_or(0.0);
            (winner.score == 100.0 && runner_up < 100.0)
                || (winner.score >= 92.0 && winner.score - runner_up >= 5.0)
        });
        Ok(InventoryDraftResolution {
            draft,
            candidates,
            resolved: resolved.into(),
        })
    }

    /// List one whole recognised equipment holding on the auction. The
    /// holding moves to `listed` and the starting-bid fee is spent now; a
    /// missing, sold, or already-listed holding is a not-found.
    pub async fn inventory_equipment_listing_create(
        &self,
        input: EquipmentListingInput,
    ) -> Result<AuctionListing, ApiError> {
        self.analytics
            .create_equipment_listing(
                &input.item_id,
                input.starting_bid,
                input.buyout,
                input.listing_fee,
                input.listed_at.as_deref(),
                input.auction_days,
            )
            .await
            .map_err(analytics_error("equipment listing create"))?
            .map(auction_listing_dto)
            .ok_or_else(|| ApiError::not_found("no held equipment with that id"))
    }

    /// Record a completed fee-free player trade for one whole recognised
    /// equipment holding. The result is realised immediately; a missing,
    /// sold, or already-listed holding is a not-found.
    pub async fn inventory_equipment_trade(
        &self,
        input: EquipmentTradeInput,
    ) -> Result<AuctionListing, ApiError> {
        self.analytics
            .trade_equipment(&input.item_id, input.sold_for, input.sold_at.as_deref())
            .await
            .map_err(analytics_error("equipment trade"))?
            .map(auction_listing_dto)
            .ok_or_else(|| ApiError::not_found("no held equipment with that id"))
    }

    /// Create an inventory item.
    pub async fn inventory_create(
        &self,
        item: InventoryItemInput,
    ) -> Result<InventoryItem, ApiError> {
        let value = self
            .analytics
            .create_inventory_item(
                &item.name,
                item.tt_value,
                item.markup_paid,
                item.notes.as_deref(),
                item.acquired_at.as_deref(),
            )
            .await
            .map_err(analytics_error("inventory create"))?;
        Ok(inventory_item_dto(value))
    }

    /// Update an inventory item; a missing item is a not-found.
    pub async fn inventory_update(
        &self,
        item_id: String,
        patch: InventoryPatch,
    ) -> Result<InventoryItem, ApiError> {
        match self
            .analytics
            .update_inventory_item(
                &item_id,
                patch.name.as_deref(),
                patch.tt_value,
                patch.markup_paid,
                patch.notes.as_deref(),
            )
            .await
            .map_err(analytics_error("inventory update"))?
        {
            Some(value) => Ok(inventory_item_dto(value)),
            None => Err(ApiError::not_found("Inventory item not found")),
        }
    }

    /// Delete an inventory item; a missing item is a not-found.
    pub async fn inventory_delete(&self, item_id: String) -> Result<(), ApiError> {
        match self
            .analytics
            .delete_inventory_item(&item_id)
            .await
            .map_err(analytics_error("inventory delete"))?
        {
            true => Ok(()),
            false => Err(ApiError::not_found("Inventory item not found")),
        }
    }

    /// Sell an inventory item (emit the realised delta to the ledger and
    /// retain the row as sold); a missing item is a not-found.
    pub async fn inventory_sell(
        &self,
        item_id: String,
        sale: InventorySellInput,
    ) -> Result<InventorySellResult, ApiError> {
        match self
            .analytics
            .sell_inventory_item(
                &item_id,
                sale.sale_price,
                sale.description.as_deref(),
                sale.sold_at.as_deref(),
            )
            .await
            .map_err(analytics_error("inventory sell"))?
        {
            Some(sale) => Ok(InventorySellResult {
                ledger_entry: sale.ledger_entry.map(ledger_item_dto).into(),
                sold_item: inventory_item_dto(sale.sold_item),
            }),
            None => Err(ApiError::not_found("Inventory item not found")),
        }
    }
}

// ── Service-row to DTO mapping ──────────────────────────────────────

pub(crate) fn overview_dto(data: eo_services::analytics::OverviewData) -> AnalyticsOverview {
    AnalyticsOverview {
        total_return_rate: data.total_return_rate,
        // The service emits exactly these three; its own fallback is stable.
        trend: match data.trend {
            "improving" => Trend::Improving,
            "declining" => Trend::Declining,
            _ => Trend::Stable,
        },
        returns_breakdown: ReturnsBreakdown {
            loot_tt: data.returns_breakdown.loot_tt,
            quest_item_tt: data.returns_breakdown.quest_item_tt,
            pes: data.returns_breakdown.pes,
            codex_pes: data.returns_breakdown.codex_pes,
            quest_pes: data.returns_breakdown.quest_pes,
            ledger: data.returns_breakdown.ledger,
        },
        losses_breakdown: LossesBreakdown {
            tracking_cost: data.losses_breakdown.tracking_cost,
            cycled_breakdown: CycledBreakdown {
                weapon: data.losses_breakdown.cycled_breakdown.weapon.as_f64(),
                healing: data.losses_breakdown.cycled_breakdown.healing.as_f64(),
                enhancer: data.losses_breakdown.cycled_breakdown.enhancer.as_f64(),
                armour: data.losses_breakdown.cycled_breakdown.armour.as_f64(),
                dangling: data.losses_breakdown.cycled_breakdown.dangling.as_f64(),
                consumables: data.losses_breakdown.cycled_breakdown.consumables.as_f64(),
            },
            ledger: data.losses_breakdown.ledger,
        },
        total_gains: data.total_gains,
        total_losses: data.total_losses,
        timeline: data
            .timeline
            .into_iter()
            .map(|point| TimelineDay {
                date: point.bucket,
                loot_tt: point.loot_tt,
                quest_item_tt: point.quest_item_tt,
                pes: point.pes,
                codex_pes: point.codex_pes,
                quest_pes: point.quest_pes,
                ledger_gains: point.ledger_gains,
                tracking_cost: point.tracking_cost,
                ledger_losses: point.ledger_losses,
            })
            .collect(),
        monthly_breakdown: data
            .monthly_breakdown
            .into_iter()
            .map(|point| MonthlyEntry {
                month: point.bucket,
                loot_tt: point.loot_tt,
                quest_item_tt: point.quest_item_tt,
                pes: point.pes,
                codex_pes: point.codex_pes,
                quest_pes: point.quest_pes,
                ledger_gains: point.ledger_gains,
                tracking_cost: point.tracking_cost,
                ledger_losses: point.ledger_losses,
            })
            .collect(),
    }
}

pub(crate) fn hunting_dto(data: eo_services::analytics::HuntingData) -> AnalyticsHunting {
    AnalyticsHunting {
        mob_comparisons: data
            .mob_comparisons
            .into_iter()
            .map(|row| MobComparison {
                mob_name: row.name,
                sessions: row.sessions,
                kills: row.kills,
                hours: row.hours,
                cycled: row.cycled,
                pes_per100_ped: row.pes_per100_ped,
                loot_rate: row.loot_rate,
            })
            .collect(),
        name_comparisons: data
            .name_comparisons
            .into_iter()
            .map(|row| NameComparison {
                session_name: row.name,
                sessions: row.sessions,
                kills: row.kills,
                hours: row.hours,
                cycled: row.cycled,
                pes_per100_ped: row.pes_per100_ped,
                loot_rate: row.loot_rate,
            })
            .collect(),
    }
}

pub(crate) fn auction_listing_dto(
    row: eo_services::analytics::AuctionListingRow,
) -> AuctionListing {
    AuctionListing {
        id: row.id,
        item_name: row.item_name,
        quantity: row.quantity,
        attributed_qty: row.attributed_qty,
        unattributed_qty: row.unattributed_qty,
        tt_value: row.tt_value,
        attributed_tt: row.attributed_tt,
        starting_bid: row.starting_bid,
        buyout: row.buyout.into(),
        listing_fee: row.listing_fee,
        listed_at: row.listed_at,
        status: row.status,
        final_price: row.final_price.into(),
        sale_fee: row.sale_fee.into(),
        resolved_at: row.resolved_at.into(),
        subject_kind: row.subject_kind,
        inventory_item_id: row.inventory_item_id.into(),
        cost_basis: row.cost_basis.into(),
        channel: row.channel,
        auction_days: row.auction_days.into(),
        expires_at: row.expires_at.into(),
        activity_net_markup: row.activity_net_markup.into(),
        gross_markup: row.gross_markup.into(),
    }
}

pub(crate) fn hunting_activity_dto(
    data: eo_services::analytics::HuntingActivityData,
) -> AnalyticsHuntingActivity {
    fn loot_item(row: eo_services::analytics::HarvestLootItemRow) -> HarvestLootItem {
        HarvestLootItem {
            item_name: row.item_name,
            quantity: row.quantity,
            value_ped: row.value_ped,
        }
    }
    fn expected(row: eo_services::analytics::ExpectedHuntingAggregate) -> ExpectedHuntingEconomics {
        ExpectedHuntingEconomics {
            model_version: row.model_version,
            looter_source: row.looter_source,
            looter_level: row.looter_level,
            expected_loot_tt: row.expected_loot_tt,
            modelled_raw_tt: row.modelled_raw_tt,
            eligible_offensive_cost: row.eligible_offensive_cost,
            offensive_tt_recovery: row.offensive_tt_recovery,
            expected_tt_rate: row.expected_tt_rate,
            effective_efficiency: row.effective_efficiency.into(),
            break_even_loot_markup: row.break_even_loot_markup,
            coverage: row.coverage,
            incomplete: row.incomplete,
            missing_basis_phases: row.missing_basis_phases,
        }
    }
    fn activity(row: eo_services::analytics::HuntingSignatureRow) -> HuntingActivityComparison {
        let realised_returns = row.returns
            + row.confirmed_reward_ped
            + row.realised_loot_markup
            + row.realised_reward_markup;
        HuntingActivityComparison {
            kind: HuntingActivityKind::classify(&row.kind),
            label: row.label,
            cycled: row.cycled,
            returns: row.returns,
            loot_rate: if row.cycled > 0.0 {
                round_half_even(row.returns / row.cycled, 4)
            } else {
                0.0
            },
            expected: row.expected.map(expected).into(),
            confirmed_reward_ped: row.confirmed_reward_ped,
            realised_reward_markup: row.realised_reward_markup,
            reward_items: row.reward_items.into_iter().map(loot_item).collect(),
            rewarded_returns: realised_returns,
            rewarded_rate: if row.cycled > 0.0 {
                round_half_even(realised_returns / row.cycled, 4)
            } else {
                0.0
            },
            reward_status: HuntingRewardStatus::classify(&row.reward_status),
            loot_items: row.loot_items.into_iter().map(loot_item).collect(),
            variants: row.variants.into_iter().map(activity).collect(),
        }
    }
    AnalyticsHuntingActivity {
        overall: HuntingActivityOverall {
            cycled: data.overall.cycled,
            returns: data.overall.returns,
            loot_rate: data.overall.loot_rate,
            expected: data.overall.expected.map(expected).into(),
        },
        definitions: data
            .definitions
            .into_iter()
            .map(|row| HuntingDefinitionComparison {
                definition_id: row.definition_id.into(),
                name: row.name,
                is_archived: row.is_archived,
                cycled: row.cycled,
                returns: row.returns,
                loot_rate: row.loot_rate,
                expected: row.expected.map(expected).into(),
                loot_items: row.loot_items.into_iter().map(loot_item).collect(),
                activities: row.activities.into_iter().map(activity).collect(),
            })
            .collect(),
        species: data
            .species
            .into_iter()
            .map(|row| HuntingSpeciesComparison {
                mob_species: row.mob_species,
                cycled: row.cycled,
                returns: row.returns,
                loot_rate: row.loot_rate,
                expected: row.expected.map(expected).into(),
                loot_items: row.loot_items.into_iter().map(loot_item).collect(),
            })
            .collect(),
    }
}

pub(crate) fn harvest_dto(data: eo_services::analytics::HarvestData) -> AnalyticsHarvest {
    AnalyticsHarvest {
        tier_comparisons: data
            .tier_comparisons
            .into_iter()
            .map(|row| HarvestTierComparison {
                yield_tier: row.yield_tier.into(),
                swings: row.swings,
                cycled: row.cycled,
                returns: row.returns,
                loot_rate: row.loot_rate,
                loot_items: row
                    .loot_items
                    .into_iter()
                    .map(|item| HarvestLootItem {
                        item_name: item.item_name,
                        quantity: item.quantity,
                        value_ped: item.value_ped,
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub(crate) fn ledger_item_dto(row: eo_services::analytics::LedgerRow) -> LedgerItem {
    LedgerItem {
        id: row.id,
        date: row.date,
        kind: LedgerKind::classify(&row.kind),
        description: row.description,
        amount: row.amount,
        tag: row.tag,
    }
}

pub(crate) fn ledger_preset_dto(row: eo_services::analytics::PresetRow) -> LedgerPreset {
    LedgerPreset {
        id: row.id,
        name: row.name,
        kind: LedgerKind::classify(&row.kind),
        description: row.description,
        amount: row.amount,
        tag: row.tag,
    }
}

pub(crate) fn inventory_item_dto(row: eo_services::analytics::InventoryRow) -> InventoryItem {
    InventoryItem {
        id: row.id,
        name: row.name,
        tt_value: row.tt_value,
        markup_paid: row.markup_paid,
        notes: row.notes.into(),
        acquired_at: row.acquired_at,
    }
}

/// Map the analytics service's error surface onto the IPC error contract:
/// the two validation variants become bad-requests carrying their verbatim
/// message; a driver / rollup failure collapses to the internal error,
/// logged server-side under `context`.
pub(crate) fn analytics_error(context: &'static str) -> impl FnOnce(AnalyticsError) -> ApiError {
    move |err| match err {
        AnalyticsError::InvalidCursor => ApiError::bad_request("Invalid cursor"),
        AnalyticsError::InvalidPresetType => {
            ApiError::bad_request("type must be 'expense' or 'markup'")
        }
        AnalyticsError::InvalidInput(message) => ApiError::bad_request(message),
        // The domain refused on the state it found, and said why in terms of
        // that state. The reason is for the player, so it travels intact
        // rather than being flattened into a generic failure.
        AnalyticsError::Rejected(ref message) => ApiError::bad_request(message.clone()),
        source => ApiError::internal(context)(source),
    }
}
