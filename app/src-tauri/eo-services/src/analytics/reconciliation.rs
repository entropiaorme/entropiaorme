//! Reconciling held stock against what the account shows.
//!
//! The app's stock is what it recorded: loot, conversions, sales, removals,
//! and the assets the player holds by quantity. Trades, crafting, loot from
//! a session the player forgot to start and stock from before tracking leave
//! it apart from what the player really holds. A reconciliation compares the
//! two, item by item, and settles each difference the way the player says,
//! through the same writes as the player's own entries:
//!
//! - **A shortfall reduces open stock, never a session's returns.** It
//!   leaves the loose open positions in proportion to what each source
//!   holds, through the removal and its weighted allocation; Loot TT, Net
//!   and every rate stay as they were. Shrapnel defaults to converted into
//!   Universal Ammo (part of it may have been sold at TT instead). A
//!   shortfall larger than the loose stock would reach into an asset, which
//!   only the asset's own records move: the item is held back.
//! - **A surplus is unattributed stock**, of unknown origin: its TT is never
//!   income, and a later sale above TT books only the markup, credited to no
//!   activity. It is valued at the item's exact TT per unit where one is
//!   known, and at the account's own TT otherwise (at most a PEC a row low,
//!   which is never income either).
//! - **Several shortfalls and a surplus may be one conversion**, recycling
//!   into Nanocubes included, as long as the TT that went in is the TT that
//!   came out: a recycling makes no more Nanocubes than its inputs' TT pays
//!   for (the rest of the surplus is of unknown origin), and any other
//!   conversion balances to the PEC per row.
//! - **An item whose record from the account the player took back** is left
//!   as it is by default until a reconciliation settles a line of it.
//! - **A difference the player accepted is remembered.** A line left as it
//!   is in a standing reconciliation is the player's word on that item's
//!   difference: while a later plan finds the same difference, the line is
//!   filed apart as previously accepted rather than asked about again, and
//!   it can be brought back into review. A difference that has moved since
//!   is a live line, and undoing the reconciliation forgets what it
//!   accepted.
//! - **The TT tells two items of one name apart.** The account's TT for an
//!   item must fit its TT per unit: a constant of the game where the game
//!   (or the game data) gives one, or the app's own from recorded loot,
//!   allowing for the chat log rounding each loot line to the PEC. An item
//!   that does not fit is left out as another item of the name.
//!
//! The comparison is of quantities, never of values. What the account shows
//! arrives as plain observed stacks and the items held back with a reason;
//! nothing here knows what a reading is.
//!
//! The plan is a draft kept in the database, revised and validated here
//! until it is applied. Applying is refused while a session is tracked, and
//! is computed against the stock as it stands inside the applying
//! transaction: a line whose item had loot or a stock movement recorded
//! after the observation, or whose holding has changed since the plan, is
//! refused and left for the next plan. Each reconciliation applied is one
//! record, undone as a whole: undoing restores the quantities, and the
//! record stays as the history of the correction. A record it made cannot
//! be undone on its own while it stands.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::conversion::{self, ConversionTerms};
use super::from_account::{held, taken_back, undo_unknown_origin_within, write_unknown_origin};
use super::{
    all_item_positions, blocked_reason, epoch_to_iso, is_universal_ammo, item_positions,
    naive_to_epoch, produced_unit_tt, reversal_blocker, undo_conversion_within,
    undo_private_sale_within, undo_removal_within, write_private_sale, write_ratio_conversion,
    write_removal, ActivityHistoryRow, AnalyticsError, AnalyticsService, ConversionHolding,
    ConversionInput, ConversionOutput, ItemConversion, PrivateSale, Profession, RatioConversion,
    StockRemoval, UnknownOriginUndo, STOCK_EPSILON,
};
use crate::db::DbError;

const SHRAPNEL: &str = "Shrapnel";
const UNIVERSAL_AMMO: &str = "Universal Ammo";
const NANOCUBE: &str = "Nanocube";

/// Shrapnel's TT per unit, a constant of the game.
const SHRAPNEL_UNIT_TT: f64 = 0.0001;

/// A PEC, in PED: what the account site truncates each row's value to.
const PEC: f64 = 0.01;

/// How long after a session ends a pasted inventory may still predate it.
const PASTE_AFTER_SESSION_SECONDS: f64 = 60.0 * 60.0;

/// Why a reconciliation may not go ahead while a session is tracked.
pub const SESSION_TRACKED: &str =
    "stock is reconciled once no session is being tracked: what the session loots is still landing";

/// Why a record a standing reconciliation made is not undone on its own.
pub const HELD_BY_RECONCILIATION: &str =
    "this was recorded by a stock reconciliation: undo the reconciliation as a whole";

/// The name a reconciliation compares items under: trimmed, and without a
/// closing `(L)`, so the account's and the app's spellings of one item meet.
pub fn reconciliation_key(name: &str) -> String {
    let trimmed = name.trim();
    trimmed
        .strip_suffix("(L)")
        .map_or(trimmed, str::trim_end)
        .to_string()
}

/// What the observation was taken from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReconciliationSource {
    /// A stored account reading.
    #[serde(rename_all = "camelCase")]
    Reading { reading_id: i64 },
    /// An inventory the player pasted from the account site.
    Paste,
}

impl ReconciliationSource {
    fn as_str(&self) -> &'static str {
        match self {
            ReconciliationSource::Reading { .. } => "reading",
            ReconciliationSource::Paste => "paste",
        }
    }

    fn reading_id(&self) -> Option<i64> {
        match self {
            ReconciliationSource::Reading { reading_id } => Some(*reading_id),
            ReconciliationSource::Paste => None,
        }
    }
}

/// One item the account shows, merged across the places that count (the
/// avatar and storage), as the account site merges them.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedStack {
    /// As the account names it.
    pub item_name: String,
    pub quantity: i64,
    /// Its TT in PED: the sum of its rows' values, each truncated to the
    /// whole PEC, so up to a PEC a row below the true figure.
    pub tt_value: f64,
    pub rows: usize,
    /// Every row holds one item: not a stack, unless the app holds it as
    /// one.
    pub single: bool,
    /// Compared only when the app holds the item as stock; held back for
    /// this reason otherwise.
    pub unless_held: Option<Exclusion>,
    /// The game data's TT per unit for it, a constant of the game, when the
    /// game data lists it.
    pub catalogue_unit_tt: Option<f64>,
}

/// Why an item is left out of a reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Exclusion {
    /// Liquid PED, never stock.
    UniversalAmmo,
    /// Ammunition other than Universal Ammo.
    Ammo,
    /// A weapon, an amplifier, a tool or another piece of equipment.
    Equipment,
    Armour,
    Plate,
    /// Rows attached to another item: not loose stock.
    Attached,
    /// Rows listed at auction: the listing's own records follow them.
    Auction,
    /// More of it is listed at auction than the app has listings for.
    AuctionWithoutListing,
    /// A proposal the player has not decided waits on this item.
    PendingProposal,
    /// Used up as a cost when used, and not held as stock.
    Consumable,
    /// Strongboxes and their keys, not held as stock: the opening flow's.
    Strongbox,
    /// One item to a row that the app has never held as stock.
    SingleItem,
    /// Two of the app's items share the name, and both are held.
    SharedName,
    /// The account's TT for it does not fit the item's TT per unit: it may
    /// be another item of the same name, or one whose TT has changed.
    TtDiffers,
    /// More is missing than the loose stock holds: the rest is an asset,
    /// which only its own records move.
    HeldAsAsset,
}

impl Exclusion {
    /// Whether the reason holds the whole item back, rather than only some
    /// of its rows.
    fn holds_back_item(self) -> bool {
        !matches!(self, Exclusion::Attached | Exclusion::Auction)
    }
}

/// An item, or some of its rows, left out, with why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedItem {
    pub item_name: String,
    /// What the account shows of it under this reason.
    pub quantity: f64,
    pub reason: Exclusion,
}

/// What the account showed, as plain figures.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub source: ReconciliationSource,
    /// When it was observed (unix seconds).
    pub observed_at: f64,
    pub stacks: Vec<ObservedStack>,
    pub excluded: Vec<ExcludedItem>,
}

/// An item's TT per unit, and how far it can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UnitTt {
    /// The app's own (from recorded loot or what a conversion produced), or
    /// a constant of the game.
    Exact { ped: f64 },
    /// Read from the account's values, which are truncated to the PEC per
    /// row: at most a PEC a row below the true figure.
    Truncated { ped: f64 },
    /// Neither the app nor the account gives one.
    Unknown,
}

impl UnitTt {
    fn ped(self) -> f64 {
        match self {
            UnitTt::Exact { ped } | UnitTt::Truncated { ped } => ped,
            UnitTt::Unknown => 0.0,
        }
    }

    fn exact(self) -> Option<f64> {
        match self {
            UnitTt::Exact { ped } => Some(ped),
            UnitTt::Truncated { .. } | UnitTt::Unknown => None,
        }
    }
}

/// What to do about one item's difference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Disposition {
    /// The missing units leave the loose open positions: gone, where to
    /// unknown.
    Removed,
    /// The missing Shrapnel was converted into Universal Ammo, all of it or
    /// all but what was sold at TT.
    #[serde(rename_all = "camelCase")]
    ShrapnelConverted { sold_at_tt: Option<f64> },
    /// One of the items of a conversion: its shortfalls went in, its
    /// surplus came out.
    #[serde(rename_all = "camelCase")]
    ConversionGroup { group: u32 },
    /// The extra units are held as stock of unknown origin.
    UnknownOrigin,
    /// Nothing is recorded for it.
    LeftAsIs,
}

impl Disposition {
    fn as_str(&self) -> &'static str {
        match self {
            Disposition::Removed => "removed",
            Disposition::ShrapnelConverted { .. } => "shrapnel_converted",
            Disposition::ConversionGroup { .. } => "conversion_group",
            Disposition::UnknownOrigin => "unknown_origin",
            Disposition::LeftAsIs => "left_as_is",
        }
    }

    fn group(&self) -> Option<u32> {
        match self {
            Disposition::ConversionGroup { group } => Some(*group),
            _ => None,
        }
    }
}

/// One item whose quantity in the app differs from the account's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationLine {
    /// The app's name for it when it holds it, the account's otherwise.
    pub item_name: String,
    /// Everything the app holds of it: the loose stock and its assets.
    pub app_quantity: f64,
    /// The part of the app's quantity held as assets, which a
    /// reconciliation never moves.
    pub asset_quantity: f64,
    pub observed_quantity: f64,
    /// How many of the account's rows it was read from.
    pub observed_rows: usize,
    pub unit_tt: UnitTt,
    pub disposition: Disposition,
    /// The records the account readings made of this item that the player
    /// took back and no reconciliation has settled since: why it is left as
    /// it is by default.
    pub taken_back: Vec<String>,
    /// When a standing reconciliation accepted this same difference (left
    /// it as it is), if one did.
    #[serde(default)]
    pub accepted_at: Option<f64>,
}

impl ReconciliationLine {
    /// The account's quantity less the app's: below zero for a shortfall.
    pub fn difference(&self) -> f64 {
        self.observed_quantity - self.app_quantity
    }

    /// What the app holds of it outside its assets.
    pub fn loose_quantity(&self) -> f64 {
        self.app_quantity - self.asset_quantity
    }

    fn is_shortfall(&self) -> bool {
        self.difference() < -STOCK_EPSILON
    }

    fn is_surplus(&self) -> bool {
        self.difference() > STOCK_EPSILON
    }

    fn is_shrapnel(&self) -> bool {
        reconciliation_key(&self.item_name) == SHRAPNEL
    }

    /// The TT of the difference, by the unit TT known.
    fn difference_tt(&self) -> f64 {
        self.difference().abs() * self.unit_tt.ped()
    }

    /// What a line starts as: an item taken back is left as it is; missing
    /// Shrapnel was converted, anything else missing was removed; anything
    /// extra is of unknown origin when its TT per unit is known (exactly,
    /// or as the account shows it), and is left as it is otherwise.
    fn default_disposition(&self) -> Disposition {
        if !self.taken_back.is_empty() {
            Disposition::LeftAsIs
        } else if self.is_surplus() {
            if self.unit_tt == UnitTt::Unknown {
                Disposition::LeftAsIs
            } else {
                Disposition::UnknownOrigin
            }
        } else if self.is_shrapnel() {
            Disposition::ShrapnelConverted { sold_at_tt: None }
        } else {
            Disposition::Removed
        }
    }

    /// Whether this line, on its own, can take `disposition`; a group is
    /// judged as a whole.
    fn fits(&self, disposition: &Disposition) -> Result<(), String> {
        let name = &self.item_name;
        let missing = -self.difference();
        let within_loose = || {
            if missing > self.loose_quantity() + STOCK_EPSILON {
                Err(format!(
                    "more {name} is missing than the loose stock holds; the rest is an asset"
                ))
            } else {
                Ok(())
            }
        };
        match disposition {
            Disposition::Removed => {
                if !self.is_shortfall() {
                    return Err(format!("{name} is not missing, so nothing is removed"));
                }
                within_loose()
            }
            Disposition::ShrapnelConverted { sold_at_tt } => {
                if !self.is_shortfall() || !self.is_shrapnel() {
                    return Err(format!("only missing Shrapnel is converted, not {name}"));
                }
                if let Some(sold) = sold_at_tt {
                    if !sold.is_finite() || *sold <= 0.0 || *sold > missing + STOCK_EPSILON {
                        return Err(format!(
                            "the Shrapnel sold at TT is more than nothing and at most the \
                             {missing} missing"
                        ));
                    }
                }
                within_loose()
            }
            Disposition::UnknownOrigin => {
                if !self.is_surplus() {
                    return Err(format!(
                        "{name} has no extra units to hold as of unknown origin"
                    ));
                }
                if self.unit_tt == UnitTt::Unknown {
                    return Err(format!(
                        "{name}'s TT per unit is not known, so its extra units cannot be \
                         valued as stock"
                    ));
                }
                Ok(())
            }
            Disposition::ConversionGroup { .. } => {
                if self.is_shortfall() {
                    within_loose()
                } else {
                    Ok(())
                }
            }
            Disposition::LeftAsIs => Ok(()),
        }
    }
}

/// One conversion group's figures: the TT that went in and came out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupFigures {
    pub group: u32,
    /// The TT of the shortfalls that went in.
    pub tt_in: f64,
    /// The TT of what came out.
    pub tt_out: f64,
    /// The item that came out, how much of it the conversion produced, and
    /// how much more arrived (held as of unknown origin).
    pub output_item: String,
    pub output_quantity: f64,
    pub remainder_quantity: f64,
}

/// Something about a draft the player should know before applying it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Caution {
    /// A paste planned within the hour after a session ended may have been
    /// copied before it: copy the inventory afresh after a session.
    #[serde(rename_all = "camelCase")]
    PasteSoonAfterSession { session_ended_at: f64 },
}

/// A reconciliation being reviewed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationDraft {
    pub id: String,
    pub source: ReconciliationSource,
    /// When the account was observed (unix seconds).
    pub observed_at: f64,
    /// When the plan was made (unix seconds).
    pub planned_at: f64,
    /// The largest difference by TT first.
    pub lines: Vec<ReconciliationLine>,
    pub excluded: Vec<ExcludedItem>,
    /// Each conversion group's figures, by group.
    pub groups: Vec<GroupFigures>,
    pub caution: Option<Caution>,
    /// Lines whose difference a standing reconciliation accepted and which
    /// stand as they were: left as they are, and not applied, unless brought
    /// back into review.
    #[serde(default)]
    pub previously_accepted: Vec<ReconciliationLine>,
}

/// A disposition a line can be set to on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChoice {
    Removed,
    ShrapnelConverted,
    UnknownOrigin,
    LeftAsIs,
}

/// One change to a draft.
#[derive(Debug, Clone, PartialEq)]
pub enum Revision {
    /// Set the disposition of these items' lines, one or many (a filter
    /// the player typed arrives as the items it matched, so every search
    /// box matches the same way). The choice is set on every line it fits,
    /// and the rest are reported. A line taken out of a conversion breaks
    /// it up: its other lines go back to what they start as.
    Set {
        items: Vec<String>,
        choice: LineChoice,
    },
    /// How much of the missing Shrapnel was sold at TT rather than
    /// converted; `None` for none.
    ShrapnelSoldAtTt { quantity: Option<f64> },
    /// Make these lines one conversion: one surplus, and the shortfalls that
    /// went into it, balanced in TT.
    Group { items: Vec<String> },
    /// Bring these previously accepted lines back into review, each as it
    /// starts.
    Reconsider { items: Vec<String> },
}

/// A draft revised, and the lines a bulk choice did not fit, each with why.
#[derive(Debug, Clone, PartialEq)]
pub struct Revised {
    pub draft: ReconciliationDraft,
    pub unchanged: Vec<(String, String)>,
}

/// Why a line was not applied.
#[derive(Debug, Clone, PartialEq)]
pub enum LineRefusal {
    /// Loot or a stock movement of the item was recorded after the account
    /// was observed: the observation may not show it.
    StockMovedSince,
    /// What the app holds of it changed after the plan was made.
    ChangedSincePlanned,
    /// Another item of its conversion group was refused.
    GroupRefused,
}

/// What applying a draft came to.
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyOutcome {
    /// Applied: the reconciliation's id (none when nothing was recorded:
    /// every line refused or left as it is), the items settled or left as
    /// they are, and the items refused.
    Applied {
        reconciliation_id: Option<String>,
        settled: Vec<String>,
        refused: Vec<(String, LineRefusal)>,
    },
    /// This draft was applied before: nothing changed.
    AlreadyApplied { reconciliation_id: String },
    /// No draft of that id is being reviewed.
    NoDraft,
    /// A session is being tracked: nothing changed.
    SessionTracked,
}

/// What undoing a reconciliation came to.
#[derive(Debug, Clone, PartialEq)]
pub enum ReconciliationUndo {
    Undone,
    /// No such reconciliation stands.
    NotFound,
    /// Something it produced has since left; nothing changed.
    Blocked(String),
}

// ── Reading and writing the draft ────────────────────────────────────

fn read_draft(conn: &rusqlite::Connection) -> Result<Option<ReconciliationDraft>, DbError> {
    use rusqlite::OptionalExtension as _;
    let stored: Option<String> = conn
        .query_row(
            "SELECT draft_json FROM stock_reconciliation_draft WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    // A working document of another shape (an older version's) is no
    // draft: the next plan replaces it.
    Ok(stored.and_then(|json| serde_json::from_str(&json).ok()))
}

fn write_draft(
    conn: &rusqlite::Connection,
    draft: &ReconciliationDraft,
    now: f64,
) -> Result<(), DbError> {
    let json = serde_json::to_string(draft).map_err(|source| DbError::Decode {
        context: "a reconciliation draft encoded as JSON",
        source,
    })?;
    conn.execute(
        "INSERT INTO stock_reconciliation_draft (id, draft_id, draft_json, updated_at) \
         VALUES (1, ?1, ?2, ?3) \
         ON CONFLICT(id) DO UPDATE SET draft_id = excluded.draft_id, \
             draft_json = excluded.draft_json, updated_at = excluded.updated_at",
        rusqlite::params![draft.id, json, now],
    )?;
    Ok(())
}

fn session_tracked(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE is_active = 1)",
        [],
        |row| row.get(0),
    )
}

// ── What the app holds ───────────────────────────────────────────────

/// The assets held, by compared name.
fn assets_by_key(conn: &rusqlite::Connection) -> rusqlite::Result<HashMap<String, f64>> {
    let mut stmt = conn.prepare(
        "SELECT name, COALESCE(SUM(quantity), 0) FROM inventory_items \
         WHERE state = 'held' GROUP BY name",
    )?;
    let mut out: HashMap<String, f64> = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        *out.entry(reconciliation_key(&name)).or_insert(0.0) += row.get::<_, f64>(1)?;
    }
    Ok(out)
}

/// The quantities listed at auction in listings still open, by compared
/// name.
fn listed_by_key(conn: &rusqlite::Connection) -> rusqlite::Result<HashMap<String, f64>> {
    let mut stmt = conn.prepare(
        "SELECT item_name, COALESCE(SUM(quantity), 0) FROM auction_listings \
         WHERE status = 'pending' AND undone_at IS NULL GROUP BY item_name",
    )?;
    let mut out: HashMap<String, f64> = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        *out.entry(reconciliation_key(&name)).or_insert(0.0) += row.get::<_, f64>(1)?;
    }
    Ok(out)
}

/// The records taken back that a standing reconciliation has settled a
/// line of their item over.
fn acknowledged(conn: &rusqlite::Connection) -> rusqlite::Result<BTreeSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT a.record_id FROM stock_reconciliation_acknowledged a \
         JOIN stock_reconciliations r ON r.id = a.reconciliation_id \
         WHERE r.undone_at IS NULL",
    )?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(ids)
}

/// The difference each item's latest line in a standing reconciliation
/// accepted, and when, by compared name: only where that line was left as
/// it is (a later line settled another way ends the acceptance).
fn accepted_differences(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<HashMap<String, (f64, f64)>> {
    let mut stmt = conn.prepare(
        // id-order: insertion (a later line of the same check after an earlier)
        "SELECT l.item_name, l.observed_quantity - l.app_quantity, l.disposition, r.applied_at \
         FROM stock_reconciliation_lines l \
         JOIN stock_reconciliations r ON r.id = l.reconciliation_id \
         WHERE r.undone_at IS NULL ORDER BY r.applied_at, l.id",
    )?;
    let mut latest: HashMap<String, (f64, f64, bool)> = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let disposition: String = row.get(2)?;
        latest.insert(
            reconciliation_key(&name),
            (row.get(1)?, row.get(3)?, disposition == "left_as_is"),
        );
    }
    Ok(latest
        .into_iter()
        .filter(|(_, (_, _, left))| *left)
        .map(|(key, (difference, at, _))| (key, (difference, at)))
        .collect())
}

/// How far the app's TT per unit of an item may lie from the true one,
/// because each loot line it is averaged from was rounded to the PEC by the
/// chat log: half a PEC a line, over every unit looted.
fn loot_rounding(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<f64> {
    let (lines, units): (f64, f64) = conn.query_row(
        // every-kill: spend (every loot line the unit TT is averaged from)
        "SELECT \
             (SELECT COUNT(*) FROM kill_loot_items \
              WHERE item_name = ?1 AND deactivated_at IS NULL) \
           + (SELECT COUNT(*) FROM harvest_loot_items \
              WHERE item_name = ?1 AND deactivated_at IS NULL), \
             (SELECT COALESCE(SUM(quantity), 0) FROM kill_loot_items \
              WHERE item_name = ?1 AND deactivated_at IS NULL) \
           + (SELECT COALESCE(SUM(quantity), 0) FROM harvest_loot_items \
              WHERE item_name = ?1 AND deactivated_at IS NULL)",
        [name],
        |row| Ok((row.get::<_, i64>(0)? as f64, row.get::<_, f64>(1)?)),
    )?;
    Ok(if units > STOCK_EPSILON {
        lines * PEC / 2.0 / units
    } else {
        0.0
    })
}

/// The TT per unit of an item that is a constant of the game: what a
/// conversion produces (Nanocubes, Universal Ammo), Shrapnel, or what the
/// game data lists for it.
fn constant_unit_tt(name: &str, stack: Option<&ObservedStack>) -> Option<f64> {
    if reconciliation_key(name) == SHRAPNEL {
        return Some(SHRAPNEL_UNIT_TT);
    }
    produced_unit_tt(name).or_else(|| stack.and_then(|stack| stack.catalogue_unit_tt))
}

/// What the app holds loose of one name, and its TT per unit.
struct Held {
    name: String,
    quantity: f64,
    unit_tt: f64,
    /// How far `unit_tt` may lie from the true figure (see
    /// [`loot_rounding`]).
    rounding: f64,
}

/// The app's loose stock under one compared name.
enum AppStock {
    /// Nothing recorded under any spelling of it.
    Never,
    /// One spelling, held or not.
    One(Held),
    /// Two spellings both held: two items may share the name.
    Shared,
}

/// The app's loose stock under the spellings `names` of one compared name,
/// read item by item as the apply reads it, so the two cannot disagree.
fn app_stock(conn: &rusqlite::Connection, names: &[String]) -> rusqlite::Result<AppStock> {
    let mut found: Vec<Held> = Vec::new();
    for name in names {
        let (open, unit_tt) = item_positions(conn, name)?;
        found.push(Held {
            name: name.clone(),
            quantity: open
                .iter()
                .map(|position| position.quantity)
                .filter(|quantity| *quantity > STOCK_EPSILON)
                .sum(),
            unit_tt,
            rounding: loot_rounding(conn, name)?,
        });
    }
    let holding = found
        .iter()
        .filter(|held| held.quantity > STOCK_EPSILON)
        .count();
    if holding > 1 {
        return Ok(AppStock::Shared);
    }
    // The spelling held, or failing that the first recorded.
    let chosen = found
        .iter()
        .position(|held| held.quantity > STOCK_EPSILON)
        .unwrap_or(0);
    Ok(if found.is_empty() {
        AppStock::Never
    } else {
        AppStock::One(found.swap_remove(chosen))
    })
}

/// Whether the account's TT for its units fits a TT per unit known to
/// within `allowance` either way: the true figure lies between the
/// truncated sum and a PEC a row above it, and some unit TT within the
/// allowance reaches it.
fn tt_fits(stack: &ObservedStack, unit_tt: f64, allowance: f64) -> bool {
    let quantity = stack.quantity as f64;
    let lowest = quantity * (unit_tt - allowance);
    let highest = quantity * (unit_tt + allowance);
    let slack = 1e-6;
    highest >= stack.tt_value - slack && lowest <= stack.tt_value + stack.rows as f64 * PEC + slack
}

// ── Planning ─────────────────────────────────────────────────────────

/// Plan a reconciliation inside the caller's transaction.
fn plan_within(
    conn: &rusqlite::Connection,
    observation: Observation,
    now: f64,
) -> Result<ReconciliationDraft, DbError> {
    let Observation {
        source,
        observed_at,
        stacks,
        mut excluded,
    } = observation;

    // More at auction than the app's open listings hold: listed without a
    // listing in the app, so the auction's part of the item is unknown.
    let listed = listed_by_key(conn)?;
    let mut at_auction: BTreeMap<String, (String, f64)> = BTreeMap::new();
    for item in excluded
        .iter()
        .filter(|item| item.reason == Exclusion::Auction)
    {
        let entry = at_auction
            .entry(reconciliation_key(&item.item_name))
            .or_insert_with(|| (item.item_name.clone(), 0.0));
        entry.1 += item.quantity;
    }
    for (key, (name, quantity)) in at_auction {
        let listed = listed.get(&key).copied().unwrap_or(0.0);
        if quantity > listed + STOCK_EPSILON {
            excluded.push(ExcludedItem {
                item_name: name,
                quantity: quantity - listed,
                reason: Exclusion::AuctionWithoutListing,
            });
        }
    }

    let held_back: BTreeSet<String> = excluded
        .iter()
        .filter(|item| item.reason.holds_back_item())
        .map(|item| reconciliation_key(&item.item_name))
        .collect();

    let mut observed: BTreeMap<String, ObservedStack> = BTreeMap::new();
    for stack in stacks {
        if is_universal_ammo(&stack.item_name) {
            continue;
        }
        observed.insert(reconciliation_key(&stack.item_name), stack);
    }

    // The whole inventory read once names every item the app has recorded
    // and which of them it holds; each candidate is then read item by item.
    let hunting_loot = crate::session_rollup::effective_hunting_loot_cells(conn, None)?;
    let positions = all_item_positions(conn, &hunting_loot)?;
    let mut app_names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut held_somewhere: BTreeSet<String> = BTreeSet::new();
    for (name, (open, _)) in &positions {
        if is_universal_ammo(name) || name.trim().is_empty() {
            continue;
        }
        let key = reconciliation_key(name);
        if open
            .iter()
            .any(|position| position.quantity > STOCK_EPSILON)
        {
            held_somewhere.insert(key.clone());
        }
        app_names.entry(key).or_default().push(name.clone());
    }
    for names in app_names.values_mut() {
        names.sort();
    }
    let assets = assets_by_key(conn)?;

    // A record from the account the player took back keeps its item left as
    // it is until a standing reconciliation has settled a line of it.
    let settled_over = acknowledged(conn)?;
    let mut taken: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in taken_back(conn)? {
        if !settled_over.contains(&record.record_id) {
            taken
                .entry(reconciliation_key(&record.item_name))
                .or_default()
                .push(record.record_id);
        }
    }

    let accepted = accepted_differences(conn)?;
    let mut previously_accepted: Vec<ReconciliationLine> = Vec::new();
    let keys: BTreeSet<String> = observed
        .keys()
        .chain(held_somewhere.iter())
        .chain(assets.keys())
        .cloned()
        .collect();
    let mut lines: Vec<ReconciliationLine> = Vec::new();
    for key in keys {
        if held_back.contains(&key) {
            continue;
        }
        let stack = observed.get(&key);
        let app = match app_names.get(&key) {
            Some(names) => app_stock(conn, names)?,
            None => AppStock::Never,
        };
        let observed_quantity = stack.map_or(0.0, |stack| stack.quantity as f64);
        let shown_name = || stack.map_or_else(|| key.clone(), |s| s.item_name.trim().to_string());
        let (item_name, loose, app_unit) = match app {
            AppStock::Shared => {
                excluded.push(ExcludedItem {
                    item_name: shown_name(),
                    quantity: observed_quantity,
                    reason: Exclusion::SharedName,
                });
                continue;
            }
            AppStock::One(held) => (held.name, held.quantity, (held.unit_tt, held.rounding)),
            AppStock::Never => (shown_name(), 0.0, (0.0, 0.0)),
        };
        let (app_unit, app_rounding) = app_unit;
        let constant = constant_unit_tt(&item_name, stack);
        let asset_quantity = assets.get(&key).copied().unwrap_or(0.0);
        let app_quantity = loose + asset_quantity;
        if let Some(stack) = stack {
            if app_quantity <= STOCK_EPSILON {
                if let Some(reason) = stack.unless_held {
                    excluded.push(ExcludedItem {
                        item_name: stack.item_name.clone(),
                        quantity: observed_quantity,
                        reason,
                    });
                    continue;
                }
                if stack.single {
                    excluded.push(ExcludedItem {
                        item_name: stack.item_name.clone(),
                        quantity: observed_quantity,
                        reason: Exclusion::SingleItem,
                    });
                    continue;
                }
            }
            // The game's constant where there is one; the app's own TT per
            // unit, allowing for the loot lines' rounding, otherwise.
            let fits = match constant {
                Some(unit) => tt_fits(stack, unit, 0.0),
                None if app_unit > STOCK_EPSILON => tt_fits(stack, app_unit, app_rounding),
                None => true,
            };
            if stack.quantity > 0 && !fits {
                excluded.push(ExcludedItem {
                    item_name: stack.item_name.clone(),
                    quantity: observed_quantity,
                    reason: Exclusion::TtDiffers,
                });
                continue;
            }
        }
        let difference = observed_quantity - app_quantity;
        if difference.abs() <= STOCK_EPSILON {
            continue;
        }
        if -difference > loose + STOCK_EPSILON {
            excluded.push(ExcludedItem {
                item_name: shown_name(),
                quantity: observed_quantity,
                reason: Exclusion::HeldAsAsset,
            });
            continue;
        }
        let unit_tt = if let Some(unit) = constant {
            UnitTt::Exact { ped: unit }
        } else if app_unit > STOCK_EPSILON {
            UnitTt::Exact { ped: app_unit }
        } else {
            match stack {
                Some(stack) if stack.quantity > 0 => UnitTt::Truncated {
                    ped: stack.tt_value / stack.quantity as f64,
                },
                _ => UnitTt::Unknown,
            }
        };
        let mut line = ReconciliationLine {
            taken_back: taken.get(&key).cloned().unwrap_or_default(),
            item_name,
            app_quantity,
            asset_quantity,
            observed_quantity,
            observed_rows: stack.map_or(0, |stack| stack.rows),
            unit_tt,
            disposition: Disposition::LeftAsIs,
            accepted_at: None,
        };
        // The same difference a standing reconciliation accepted stays
        // accepted, filed apart.
        if let Some((difference, at)) = accepted.get(&key) {
            if (difference - line.difference()).abs() <= STOCK_EPSILON {
                line.accepted_at = Some(*at);
                previously_accepted.push(line);
                continue;
            }
        }
        line.disposition = line.default_disposition();
        lines.push(line);
    }
    previously_accepted.sort_by(|a, b| a.item_name.cmp(&b.item_name));
    sort_lines(&mut lines);
    excluded.sort_by(|a, b| {
        a.reason
            .cmp(&b.reason)
            .then_with(|| a.item_name.cmp(&b.item_name))
    });
    // A pasted inventory may have been copied before a session that ended
    // in the last hour.
    let caution = if source == ReconciliationSource::Paste {
        let ended: Option<f64> = conn.query_row(
            "SELECT MAX(ended_at) FROM tracking_sessions \
             WHERE ended_at IS NOT NULL AND ended_at <= ?1 AND ended_at > ?1 - ?2",
            rusqlite::params![now, PASTE_AFTER_SESSION_SECONDS],
            |row| row.get(0),
        )?;
        ended.map(|session_ended_at| Caution::PasteSoonAfterSession { session_ended_at })
    } else {
        None
    };
    let draft = ReconciliationDraft {
        id: Uuid::new_v4().to_string(),
        source,
        observed_at,
        planned_at: now,
        lines,
        excluded,
        groups: Vec::new(),
        caution,
        previously_accepted,
    };
    write_draft(conn, &draft, now)?;
    Ok(draft)
}

/// The largest difference by TT first, so a large shortfall is the first
/// thing read, never buried.
fn sort_lines(lines: &mut [ReconciliationLine]) {
    lines.sort_by(|a, b| {
        b.difference_tt()
            .total_cmp(&a.difference_tt())
            .then_with(|| a.item_name.cmp(&b.item_name))
    });
}

// ── Revising ─────────────────────────────────────────────────────────

/// One conversion group's figures, or why it does not balance.
fn group_figures(draft: &ReconciliationDraft, group: u32) -> Result<GroupFigures, String> {
    let members: Vec<&ReconciliationLine> = draft
        .lines
        .iter()
        .filter(|line| line.disposition.group() == Some(group))
        .collect();
    let unbalanced =
        || "a conversion is one item that arrived and the missing items that went into it";
    let outputs: Vec<&&ReconciliationLine> =
        members.iter().filter(|line| line.is_surplus()).collect();
    let inputs: Vec<&&ReconciliationLine> =
        members.iter().filter(|line| line.is_shortfall()).collect();
    let [output] = outputs.as_slice() else {
        return Err(unbalanced().to_string());
    };
    if inputs.is_empty() {
        return Err(unbalanced().to_string());
    }
    let unit_of = |line: &ReconciliationLine| {
        line.unit_tt.exact().ok_or_else(|| {
            format!(
                "{}'s TT per unit is not known exactly, so a conversion of it cannot be balanced",
                line.item_name
            )
        })
    };
    let mut tt_in = 0.0;
    for input in &inputs {
        tt_in += -input.difference() * unit_of(input)?;
    }
    let unit_out = unit_of(output)?;
    let arrived = output.difference();
    // A recycling makes no more Nanocubes than its inputs' TT pays for: the
    // rest arrived some other way.
    let output_quantity = if reconciliation_key(&output.item_name) == NANOCUBE {
        arrived.min((tt_in / unit_out + 1e-6).floor())
    } else {
        arrived
    };
    let tt_out = output_quantity * unit_out;
    let tolerance = PEC * (output.observed_rows.max(1) + inputs.len()) as f64;
    if (tt_out - tt_in).abs() > tolerance + 1e-9 {
        return Err(format!(
            "the conversion does not balance: {tt_in:.2} PED went in and {tt_out:.2} PED came out"
        ));
    }
    Ok(GroupFigures {
        group,
        tt_in,
        tt_out,
        output_item: output.item_name.clone(),
        output_quantity,
        remainder_quantity: (arrived - output_quantity).max(0.0),
    })
}

/// Whether every line's disposition fits its difference and every group
/// balances; the groups' figures when they do.
fn validate(draft: &ReconciliationDraft) -> Result<Vec<GroupFigures>, String> {
    let mut groups: BTreeSet<u32> = BTreeSet::new();
    for line in &draft.lines {
        line.fits(&line.disposition)?;
        if let Some(group) = line.disposition.group() {
            groups.insert(group);
        }
    }
    groups
        .into_iter()
        .map(|group| group_figures(draft, group))
        .collect()
}

fn find_line(draft: &ReconciliationDraft, name: &str) -> Result<usize, String> {
    let key = reconciliation_key(name);
    draft
        .lines
        .iter()
        .position(|line| reconciliation_key(&line.item_name) == key)
        .ok_or_else(|| format!("no line for {}", name.trim()))
}

/// The lines of these items, each once; every name must be a line's.
fn selected(draft: &ReconciliationDraft, items: &[String]) -> Result<Vec<usize>, String> {
    let chosen = items
        .iter()
        .map(|name| find_line(draft, name))
        .collect::<Result<BTreeSet<usize>, _>>()?;
    if chosen.is_empty() {
        return Err("no line is selected".to_string());
    }
    Ok(chosen.into_iter().collect())
}

/// Apply one revision to a draft, or say why it does not fit. Returns the
/// lines a bulk choice did not fit.
fn revise_draft(
    draft: &mut ReconciliationDraft,
    revision: Revision,
) -> Result<Vec<(String, String)>, String> {
    let mut unchanged: Vec<(String, String)> = Vec::new();
    match revision {
        Revision::Set { items, choice } => {
            let mut broken: BTreeSet<u32> = BTreeSet::new();
            let mut changed = 0usize;
            for index in selected(draft, &items)? {
                let line = &draft.lines[index];
                let disposition = match choice {
                    LineChoice::Removed => Disposition::Removed,
                    LineChoice::ShrapnelConverted => match &line.disposition {
                        Disposition::ShrapnelConverted { sold_at_tt } => {
                            Disposition::ShrapnelConverted {
                                sold_at_tt: *sold_at_tt,
                            }
                        }
                        _ => Disposition::ShrapnelConverted { sold_at_tt: None },
                    },
                    LineChoice::UnknownOrigin => Disposition::UnknownOrigin,
                    LineChoice::LeftAsIs => Disposition::LeftAsIs,
                };
                if let Err(reason) = line.fits(&disposition) {
                    unchanged.push((line.item_name.clone(), reason));
                    continue;
                }
                if let Some(group) = line.disposition.group() {
                    broken.insert(group);
                }
                draft.lines[index].disposition = disposition;
                changed += 1;
            }
            if changed == 0 {
                return Err(unchanged
                    .first()
                    .map(|(_, reason)| reason.clone())
                    .unwrap_or_else(|| "no line is selected".to_string()));
            }
            // A conversion a line left breaks up.
            for line in &mut draft.lines {
                if line
                    .disposition
                    .group()
                    .is_some_and(|group| broken.contains(&group))
                {
                    line.disposition = line.default_disposition();
                }
            }
        }
        Revision::ShrapnelSoldAtTt { quantity } => {
            let index = find_line(draft, SHRAPNEL)?;
            let line = &mut draft.lines[index];
            let Disposition::ShrapnelConverted { sold_at_tt } = &mut line.disposition else {
                return Err("the missing Shrapnel is not set as converted".to_string());
            };
            *sold_at_tt = quantity;
        }
        Revision::Group { items } => {
            let indices = selected(draft, &items)?;
            if indices.len() < 2 {
                return Err(
                    "a conversion groups the item that arrived with what went into it".to_string(),
                );
            }
            if let Some(index) = indices
                .iter()
                .find(|index| draft.lines[**index].disposition.group().is_some())
            {
                return Err(format!(
                    "{} is in a conversion already; set it apart first",
                    draft.lines[*index].item_name
                ));
            }
            let group = draft
                .lines
                .iter()
                .filter_map(|line| line.disposition.group())
                .max()
                .map_or(1, |highest| highest + 1);
            for index in indices {
                draft.lines[index].disposition = Disposition::ConversionGroup { group };
            }
        }
        Revision::Reconsider { items } => {
            let mut brought = 0usize;
            for name in &items {
                let key = reconciliation_key(name);
                let Some(at) = draft
                    .previously_accepted
                    .iter()
                    .position(|line| reconciliation_key(&line.item_name) == key)
                else {
                    return Err(format!("{} is not among the accepted lines", name.trim()));
                };
                let mut line = draft.previously_accepted.remove(at);
                line.disposition = line.default_disposition();
                draft.lines.push(line);
                brought += 1;
            }
            if brought == 0 {
                return Err("no line is selected".to_string());
            }
            sort_lines(&mut draft.lines);
        }
    }
    draft.groups = validate(draft)?;
    Ok(unchanged)
}

// ── Applying ─────────────────────────────────────────────────────────

/// Whether loot, a stock movement or a change to an asset of `item` was
/// recorded after `at`. An asset's `updated_at` is the app's own clock:
/// every write of an asset stamps it, so the database's fill-in for a row
/// written without one never stands in for it.
fn stock_moved_since(conn: &rusqlite::Connection, item: &str, at: f64) -> rusqlite::Result<bool> {
    conn.query_row(
        // every-kill: spend (any row's loot moves the stock, a reward's arrival included)
        "SELECT EXISTS (SELECT 1 FROM stock_movements \
                        WHERE item_name = ?1 AND created_at > ?2) \
             OR EXISTS (SELECT 1 FROM kill_loot_items li JOIN kills k ON k.id = li.kill_id \
                        WHERE li.item_name = ?1 AND li.deactivated_at IS NULL \
                          AND k.timestamp > ?2) \
             OR EXISTS (SELECT 1 FROM harvest_loot_items l \
                        JOIN harvest_events e ON e.id = l.harvest_id \
                        WHERE l.item_name = ?1 AND l.deactivated_at IS NULL \
                          AND e.timestamp > ?2) \
             OR EXISTS (SELECT 1 FROM session_quest_completion_reward_items ri \
                        JOIN session_quest_completions c ON c.id = ri.completion_id \
                        WHERE ri.item_name = ?1 AND c.completed_at > ?2) \
             OR EXISTS (SELECT 1 FROM asset_purchases p \
                        JOIN inventory_items i ON i.id = p.position_id \
                        WHERE i.name = ?1 AND p.created_at > ?2) \
             OR EXISTS (SELECT 1 FROM inventory_items \
                        WHERE name = ?1 AND updated_at > ?2)",
        rusqlite::params![item, at],
        |row| row.get(0),
    )
}

/// One stock record a reconciliation made.
struct Made {
    kind: &'static str,
    id: String,
}

/// Hold `quantity` of a line's item as stock of unknown origin, at its
/// TT per unit: exact where known, the account's own otherwise.
fn write_surplus(
    tx: &rusqlite::Transaction<'_>,
    line: &ReconciliationLine,
    quantity: f64,
    day: &str,
    now: f64,
    made: &mut Vec<Made>,
) -> Result<Result<(), String>, DbError> {
    if line.unit_tt == UnitTt::Unknown {
        return Ok(Err(format!(
            "{}'s TT per unit is not known",
            line.item_name
        )));
    }
    let unit = line.unit_tt.ped();
    let id = Uuid::new_v4().to_string();
    write_unknown_origin(
        tx,
        &id,
        &line.item_name,
        quantity,
        quantity * unit,
        day,
        now,
    )?;
    made.push(Made {
        kind: "unknown_origin",
        id,
    });
    Ok(Ok(()))
}

/// Write one line's records (a group's are written with the group), or
/// say why the write refused.
fn write_line(
    tx: &rusqlite::Transaction<'_>,
    line: &ReconciliationLine,
    day: &str,
    now: f64,
    made: &mut Vec<Made>,
) -> Result<Result<(), String>, DbError> {
    let amount = line.difference().abs();
    match &line.disposition {
        Disposition::Removed => {
            let id = Uuid::new_v4().to_string();
            let written = write_removal(
                tx,
                &StockRemoval {
                    id: &id,
                    profession: Profession::Inventory,
                    item: &line.item_name,
                    quantity: amount,
                    removed_at: day,
                },
                now,
            )?;
            if written.is_ok() {
                made.push(Made {
                    kind: "removal",
                    id,
                });
            }
            Ok(written)
        }
        Disposition::ShrapnelConverted { sold_at_tt } => {
            let sold = sold_at_tt.unwrap_or(0.0);
            let converted = amount - sold;
            if converted > STOCK_EPSILON {
                let id = Uuid::new_v4().to_string();
                let inputs = vec![(line.item_name.clone(), converted)];
                let written = write_ratio_conversion(
                    tx,
                    &id,
                    Profession::Inventory,
                    &RatioConversion {
                        inputs: &inputs,
                        target: UNIVERSAL_AMMO,
                        value_ratio: 1.01,
                    },
                    day,
                    now,
                )?;
                if written.is_err() {
                    return Ok(written);
                }
                made.push(Made {
                    kind: "conversion",
                    id,
                });
            }
            if sold > STOCK_EPSILON {
                let id = Uuid::new_v4().to_string();
                write_private_sale(
                    tx,
                    &PrivateSale {
                        id: &id,
                        profession: Profession::Inventory,
                        item: &line.item_name,
                        quantity: sold,
                        price: None,
                        sold_at: day,
                    },
                    now,
                )?;
                made.push(Made { kind: "trade", id });
            }
            Ok(Ok(()))
        }
        Disposition::UnknownOrigin => write_surplus(tx, line, amount, day, now, made),
        Disposition::ConversionGroup { .. } | Disposition::LeftAsIs => Ok(Ok(())),
    }
}

/// Write one conversion group: its shortfalls in, its surplus out, and any
/// surplus beyond what the conversion produced as of unknown origin.
fn write_group(
    tx: &rusqlite::Transaction<'_>,
    figures: &GroupFigures,
    members: &[&ReconciliationLine],
    day: &str,
    now: f64,
    made: &mut Vec<Made>,
) -> Result<Result<(), String>, DbError> {
    let Some(output) = members.iter().find(|line| line.is_surplus()) else {
        return Ok(Err("a conversion needs the item that arrived".to_string()));
    };
    let inputs: Vec<ConversionInput> = members
        .iter()
        .filter(|line| line.is_shortfall())
        .map(|line| ConversionInput::Held {
            item_name: line.item_name.clone(),
            quantity: -line.difference(),
            worth: None,
        })
        .collect();
    let first_input = members
        .iter()
        .find(|line| line.is_shortfall())
        .map_or_else(String::new, |line| line.item_name.clone());
    let request = ItemConversion {
        inputs,
        outputs: vec![ConversionOutput {
            item_name: output.item_name.clone(),
            quantity: figures.output_quantity,
            tt_value: figures.tt_out,
            holding: ConversionHolding::Loot,
        }],
        ped_amount: None,
        converted_at: Some(day.to_string()),
    };
    // Recycling names nothing its Nanocubes descend from: Nanocubes are
    // Nanocubes. Any other conversion is itemised, as the player's own.
    let recycling = reconciliation_key(&output.item_name) == NANOCUBE;
    let terms = ConversionTerms {
        itemised: !recycling,
        target_label: recycling.then(|| NANOCUBE.to_string()),
        gain_description: format!("Conversion: {first_input}"),
    };
    let id = Uuid::new_v4().to_string();
    let written =
        conversion::write_conversion(tx, &id, Profession::Inventory, &request, day, now, &terms)?;
    if written.is_err() {
        return Ok(written);
    }
    made.push(Made {
        kind: "conversion",
        id,
    });
    if figures.remainder_quantity > STOCK_EPSILON {
        return write_surplus(tx, output, figures.remainder_quantity, day, now, made);
    }
    Ok(Ok(()))
}

fn apply_within(
    conn: &mut rusqlite::Connection,
    draft_id: &str,
    now: f64,
) -> Result<Result<ApplyOutcome, String>, DbError> {
    let tx = conn.transaction()?;
    let applied: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_reconciliations WHERE id = ?1)",
        [draft_id],
        |row| row.get(0),
    )?;
    if applied {
        return Ok(Ok(ApplyOutcome::AlreadyApplied {
            reconciliation_id: draft_id.to_string(),
        }));
    }
    let Some(draft) = read_draft(&tx)?.filter(|draft| draft.id == draft_id) else {
        return Ok(Ok(ApplyOutcome::NoDraft));
    };
    if session_tracked(&tx)? {
        return Ok(Ok(ApplyOutcome::SessionTracked));
    }
    let figures = match validate(&draft) {
        Ok(figures) => figures,
        Err(reason) => return Ok(Err(reason)),
    };

    // Each line is checked against the stock as it stands now.
    let assets = assets_by_key(&tx)?;
    let mut refused: BTreeMap<usize, LineRefusal> = BTreeMap::new();
    for (index, line) in draft.lines.iter().enumerate() {
        if line.disposition == Disposition::LeftAsIs {
            continue;
        }
        let now_held = held(&tx, &line.item_name)?
            + assets
                .get(&reconciliation_key(&line.item_name))
                .copied()
                .unwrap_or(0.0);
        if stock_moved_since(&tx, &line.item_name, draft.observed_at)? {
            refused.insert(index, LineRefusal::StockMovedSince);
        } else if (now_held - line.app_quantity).abs() > STOCK_EPSILON {
            refused.insert(index, LineRefusal::ChangedSincePlanned);
        }
    }
    // A conversion is recorded whole or not at all.
    let refused_groups: BTreeSet<u32> = refused
        .keys()
        .filter_map(|index| draft.lines[*index].disposition.group())
        .collect();
    for (index, line) in draft.lines.iter().enumerate() {
        if line
            .disposition
            .group()
            .is_some_and(|group| refused_groups.contains(&group))
        {
            refused.entry(index).or_insert(LineRefusal::GroupRefused);
        }
    }

    let day = epoch_to_iso(draft.observed_at);
    let mut made: Vec<Made> = Vec::new();
    let mut settled: Vec<&ReconciliationLine> = Vec::new();
    let mut groups: BTreeMap<u32, Vec<&ReconciliationLine>> = BTreeMap::new();
    for (index, line) in draft.lines.iter().enumerate() {
        if refused.contains_key(&index) {
            continue;
        }
        settled.push(line);
        if let Some(group) = line.disposition.group() {
            groups.entry(group).or_default().push(line);
            continue;
        }
        if let Err(reason) = write_line(&tx, line, &day, now, &mut made)? {
            return Ok(Err(reason));
        }
    }
    for (group, members) in &groups {
        let Some(figures) = figures.iter().find(|figures| figures.group == *group) else {
            return Ok(Err("a conversion's figures are missing".to_string()));
        };
        if let Err(reason) = write_group(&tx, figures, members, &day, now, &mut made)? {
            return Ok(Err(reason));
        }
    }

    // A check that settles or accepts anything keeps its record: a line
    // left as it is is the player's word on that difference, remembered
    // until the check is undone. One with every line refused keeps none.
    let records_anything = !settled.is_empty();
    let reconciliation_id = if !records_anything {
        None
    } else {
        tx.execute(
            "INSERT INTO stock_reconciliations \
                 (id, source, reading_id, observed_at, applied_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                draft.id,
                draft.source.as_str(),
                draft.source.reading_id(),
                draft.observed_at,
                now,
            ],
        )?;
        for line in &settled {
            tx.execute(
                "INSERT INTO stock_reconciliation_lines \
                     (reconciliation_id, item_name, app_quantity, observed_quantity, disposition) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    draft.id,
                    line.item_name,
                    line.app_quantity,
                    line.observed_quantity,
                    line.disposition.as_str(),
                ],
            )?;
            // A line settled over an item taken back acknowledges what was
            // taken back; one left as it is keeps it so.
            if line.disposition != Disposition::LeftAsIs {
                for record in &line.taken_back {
                    tx.execute(
                        "INSERT OR IGNORE INTO stock_reconciliation_acknowledged \
                             (reconciliation_id, record_id) VALUES (?1, ?2)",
                        rusqlite::params![draft.id, record],
                    )?;
                }
            }
        }
        for record in &made {
            tx.execute(
                "INSERT INTO stock_reconciliation_records \
                     (reconciliation_id, record_kind, record_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![draft.id, record.kind, record.id],
            )?;
        }
        Some(draft.id.clone())
    };
    tx.execute("DELETE FROM stock_reconciliation_draft WHERE id = 1", [])?;
    tx.commit()?;
    Ok(Ok(ApplyOutcome::Applied {
        reconciliation_id,
        settled: settled.iter().map(|line| line.item_name.clone()).collect(),
        refused: refused
            .into_iter()
            .map(|(index, reason)| (draft.lines[index].item_name.clone(), reason))
            .collect(),
    }))
}

fn undo_within(
    conn: &mut rusqlite::Connection,
    reconciliation_id: &str,
    undone_on: &str,
    now: f64,
) -> Result<ReconciliationUndo, DbError> {
    let tx = conn.transaction()?;
    let standing: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_reconciliations \
                        WHERE id = ?1 AND undone_at IS NULL)",
        [reconciliation_id],
        |row| row.get(0),
    )?;
    if !standing {
        return Ok(ReconciliationUndo::NotFound);
    }
    let records: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            // id-order: insertion (taken back in the reverse of the order made)
            "SELECT record_kind, record_id FROM stock_reconciliation_records \
             WHERE reconciliation_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map([reconciliation_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    // A record already taken back is passed over.
    for (kind, id) in records {
        match kind.as_str() {
            "removal" => {
                undo_removal_within(&tx, &id, undone_on)?;
            }
            "trade" => {
                undo_private_sale_within(&tx, &id, undone_on)?;
            }
            "conversion" => {
                if let Err(reason) = undo_conversion_within(&tx, &id, undone_on, now)? {
                    return Ok(ReconciliationUndo::Blocked(reason));
                }
            }
            "unknown_origin" => {
                if let Some((item, short)) = reversal_blocker(&tx, &id)? {
                    return Ok(ReconciliationUndo::Blocked(blocked_reason(&item, short)));
                }
                if undo_unknown_origin_within(&tx, &id, undone_on)? == UnknownOriginUndo::NotFound {
                    continue;
                }
            }
            _ => {
                return Ok(ReconciliationUndo::Blocked(
                    "this reconciliation holds a record this version cannot take back".to_string(),
                ))
            }
        }
    }
    tx.execute(
        "UPDATE stock_reconciliations SET undone_at = ?1 WHERE id = ?2",
        rusqlite::params![now, reconciliation_id],
    )?;
    tx.commit()?;
    Ok(ReconciliationUndo::Undone)
}

// ── The records a reconciliation made ─────────────────────────────────

/// The standing reconciliation that made the record `record_id`, if one
/// did.
pub(super) fn standing_reconciliation_of(
    conn: &rusqlite::Connection,
    record_id: &str,
) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    conn.query_row(
        "SELECT r.id FROM stock_reconciliation_records s \
         JOIN stock_reconciliations r ON r.id = s.reconciliation_id \
         WHERE s.record_id = ?1 AND r.undone_at IS NULL",
        [record_id],
        |row| row.get(0),
    )
    .optional()
}

/// Mark the history entries a standing reconciliation made: each names it,
/// and is undone only with it.
pub(super) fn attach_reconciliations(
    conn: &rusqlite::Connection,
    rows: &mut [ActivityHistoryRow],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT s.record_id, r.id FROM stock_reconciliation_records s \
         JOIN stock_reconciliations r ON r.id = s.reconciliation_id \
         WHERE r.undone_at IS NULL",
    )?;
    let made: HashMap<String, String> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if made.is_empty() {
        return Ok(());
    }
    for row in rows.iter_mut() {
        if let Some(reconciliation) = made.get(&row.id) {
            row.reconciliation_id = Some(reconciliation.clone());
            if !row.undone {
                row.can_delete = false;
                row.undo_blocked_reason = Some(HELD_BY_RECONCILIATION.to_string());
            }
        }
    }
    Ok(())
}

impl AnalyticsService {
    /// Plan a reconciliation of the app's stock against `observation`, and
    /// keep it as the draft being reviewed (replacing any other). Refused
    /// while a session is tracked.
    pub async fn plan_reconciliation(
        &self,
        observation: Observation,
    ) -> Result<ReconciliationDraft, AnalyticsError> {
        if !observation.observed_at.is_finite() {
            return Err(AnalyticsError::InvalidInput(
                "a reconciliation needs when the account was observed",
            ));
        }
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if session_tracked(&tx)? {
                    return Ok(Err(SESSION_TRACKED.to_string()));
                }
                let draft = plan_within(&tx, observation, now)?;
                tx.commit()?;
                Ok(Ok(draft))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// The draft being reviewed, if there is one.
    pub async fn reconciliation_draft(
        &self,
    ) -> Result<Option<ReconciliationDraft>, AnalyticsError> {
        Ok(self.db.with_reader(|conn| read_draft(conn)).await?)
    }

    /// Revise the draft `draft_id`: validated whole, and kept only when it
    /// fits. A bulk choice is set on the lines it fits and reports the
    /// rest. Refused when another plan has replaced the draft.
    pub async fn revise_reconciliation(
        &self,
        draft_id: &str,
        revision: Revision,
    ) -> Result<Revised, AnalyticsError> {
        let draft_id = draft_id.to_string();
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(mut draft) = read_draft(&tx)?.filter(|draft| draft.id == draft_id) else {
                    return Ok(Err(
                        "that reconciliation is no longer being reviewed".to_string()
                    ));
                };
                let unchanged = match revise_draft(&mut draft, revision) {
                    Ok(unchanged) => unchanged,
                    Err(reason) => return Ok(Err(reason)),
                };
                write_draft(&tx, &draft, now)?;
                tx.commit()?;
                Ok(Ok(Revised { draft, unchanged }))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Apply the draft `draft_id` against the stock as it stands, in one
    /// transaction. Applying it again changes nothing.
    pub async fn apply_reconciliation(
        &self,
        draft_id: &str,
    ) -> Result<ApplyOutcome, AnalyticsError> {
        let draft_id = draft_id.trim().to_string();
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| apply_within(conn, &draft_id, now))
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Undo a reconciliation as a whole: every record it made is taken back
    /// and it is marked undone. Refused, with nothing changed, when what it
    /// produced has since left.
    pub async fn undo_reconciliation(
        &self,
        reconciliation_id: &str,
    ) -> Result<ReconciliationUndo, AnalyticsError> {
        let id = reconciliation_id.trim().to_string();
        let undone_on = self.default_date();
        let now = naive_to_epoch(self.clock.now());
        Ok(self
            .db
            .with_writer(move |conn| undo_within(conn, &id, &undone_on, now))
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::write_service;
    use super::*;

    fn line(name: &str, app: f64, observed: f64) -> ReconciliationLine {
        let mut line = ReconciliationLine {
            item_name: name.to_string(),
            app_quantity: app,
            asset_quantity: 0.0,
            observed_quantity: observed,
            observed_rows: 1,
            unit_tt: UnitTt::Exact { ped: 0.01 },
            disposition: Disposition::LeftAsIs,
            taken_back: Vec::new(),
            accepted_at: None,
        };
        line.disposition = line.default_disposition();
        line
    }

    fn priced(name: &str, app: f64, observed: f64, unit: f64) -> ReconciliationLine {
        let mut line = line(name, app, observed);
        line.unit_tt = UnitTt::Exact { ped: unit };
        line
    }

    fn draft(lines: Vec<ReconciliationLine>) -> ReconciliationDraft {
        ReconciliationDraft {
            id: "d".to_string(),
            source: ReconciliationSource::Paste,
            observed_at: 0.0,
            planned_at: 0.0,
            lines,
            excluded: Vec::new(),
            groups: Vec::new(),
            caution: None,
            previously_accepted: Vec::new(),
        }
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn names_meet_across_trailing_space_and_a_limited_tag() {
        assert_eq!(reconciliation_key(" Iron Stone\n"), "Iron Stone");
        assert_eq!(reconciliation_key("Iron Stone (L)"), "Iron Stone");
        assert_eq!(reconciliation_key("Iron Stone"), "Iron Stone");
    }

    #[test]
    fn a_line_starts_removed_converted_unknown_or_left_as_it_is() {
        assert_eq!(
            line("Iron Stone", 10.0, 6.0).disposition,
            Disposition::Removed
        );
        assert_eq!(
            line(SHRAPNEL, 10.0, 6.0).disposition,
            Disposition::ShrapnelConverted { sold_at_tt: None }
        );
        assert_eq!(
            line("Iron Stone", 6.0, 10.0).disposition,
            Disposition::UnknownOrigin
        );
        let mut taken = line("Iron Stone", 10.0, 6.0);
        taken.taken_back = vec!["r1".to_string()];
        assert_eq!(taken.default_disposition(), Disposition::LeftAsIs);
        // Extra units whose TT per unit is the account's own figure are
        // valued at it; with no TT known at all they are left as they are,
        // and cannot be valued as stock.
        let mut truncated = line("Garcen Grease", 0.0, 10.0);
        truncated.unit_tt = UnitTt::Truncated { ped: 0.2 };
        assert_eq!(truncated.default_disposition(), Disposition::UnknownOrigin);
        assert!(truncated.fits(&Disposition::UnknownOrigin).is_ok());
        let mut unknown = line("Garcen Grease", 0.0, 10.0);
        unknown.unit_tt = UnitTt::Unknown;
        assert_eq!(unknown.default_disposition(), Disposition::LeftAsIs);
        assert!(unknown.fits(&Disposition::UnknownOrigin).is_err());
    }

    #[test]
    fn a_shortfall_never_reaches_into_an_asset() {
        let mut held = line("Garcen Grease", 600.0, 450.0);
        held.asset_quantity = 500.0;
        // 150 missing, 100 loose.
        assert!(held.fits(&Disposition::Removed).is_err());
        let mut within = line("Garcen Grease", 600.0, 550.0);
        within.asset_quantity = 500.0;
        assert!(within.fits(&Disposition::Removed).is_ok());
    }

    #[test]
    fn a_bulk_choice_is_set_on_the_lines_it_fits_and_reports_the_rest() {
        let mut d = draft(vec![
            line("Iron Stone", 10.0, 6.0),
            line("Oil", 2.0, 5.0),
            line("Lysterium Stone", 10.0, 2.0),
        ]);
        let unchanged = revise_draft(
            &mut d,
            Revision::Set {
                items: names(&["Iron Stone", "Oil", "Lysterium Stone"]),
                choice: LineChoice::Removed,
            },
        )
        .unwrap();
        assert_eq!(unchanged.len(), 1);
        assert_eq!(unchanged[0].0, "Oil");
        assert_eq!(d.lines[1].disposition, Disposition::UnknownOrigin);
        // A choice that fits none is refused, and the caller keeps the draft.
        assert!(revise_draft(
            &mut d,
            Revision::Set {
                items: names(&["Iron Stone"]),
                choice: LineChoice::ShrapnelConverted,
            },
        )
        .is_err());
        assert!(revise_draft(
            &mut d,
            Revision::ShrapnelSoldAtTt {
                quantity: Some(1.0)
            }
        )
        .is_err());
    }

    #[test]
    fn a_selection_sets_every_line_it_names_and_no_other() {
        let mut d = draft(vec![
            line("Iron Stone", 10.0, 6.0),
            line("Lysterium Stone", 10.0, 2.0),
            line("Oil", 2.0, 1.0),
        ]);
        revise_draft(
            &mut d,
            Revision::Set {
                items: names(&["Iron Stone", "Lysterium Stone"]),
                choice: LineChoice::LeftAsIs,
            },
        )
        .unwrap();
        let dispositions: Vec<_> = d.lines.iter().map(|l| l.disposition.clone()).collect();
        assert_eq!(
            dispositions,
            vec![
                Disposition::LeftAsIs,
                Disposition::LeftAsIs,
                Disposition::Removed
            ]
        );
    }

    #[test]
    fn a_recycling_balances_and_breaks_up_when_a_line_leaves_it() {
        let lines = vec![
            priced("Animal Oil Residue", 100.0, 0.0, 0.01),
            priced("Iron Stone", 10.0, 0.0, 0.13),
            priced(NANOCUBE, 0.0, 250.0, 0.01),
        ];
        // Two shortfalls alone are no conversion.
        let mut d = draft(lines.clone());
        assert!(revise_draft(
            &mut d,
            Revision::Group {
                items: names(&["Animal Oil Residue", "Iron Stone"])
            }
        )
        .is_err());
        let mut d = draft(lines);
        revise_draft(
            &mut d,
            Revision::Group {
                items: names(&["Animal Oil Residue", "Iron Stone", NANOCUBE]),
            },
        )
        .unwrap();
        assert!(d
            .lines
            .iter()
            .all(|l| l.disposition == Disposition::ConversionGroup { group: 1 }));
        // 2.30 PED went in: 230 Nanocubes came of it, and 20 arrived some
        // other way.
        assert_eq!(d.groups.len(), 1);
        assert!((d.groups[0].tt_in - 2.3).abs() < 1e-9);
        assert!((d.groups[0].tt_out - 2.3).abs() < 1e-9);
        assert!((d.groups[0].output_quantity - 230.0).abs() < 1e-9);
        assert!((d.groups[0].remainder_quantity - 20.0).abs() < 1e-9);
        // The surplus taken out leaves no conversion: the rest start over.
        revise_draft(
            &mut d,
            Revision::Set {
                items: names(&[NANOCUBE]),
                choice: LineChoice::LeftAsIs,
            },
        )
        .unwrap();
        let dispositions: Vec<_> = d.lines.iter().map(|l| l.disposition.clone()).collect();
        assert_eq!(
            dispositions,
            vec![
                Disposition::Removed,
                Disposition::Removed,
                Disposition::LeftAsIs
            ]
        );
        assert!(d.groups.is_empty());
    }

    #[test]
    fn a_conversion_that_does_not_balance_or_has_no_exact_output_is_refused() {
        // 1.00 PED in, 3.00 PED out.
        let mut d = draft(vec![
            priced("Animal Oil Residue", 100.0, 0.0, 0.01),
            priced("Garcen Grease", 0.0, 15.0, 0.2),
        ]);
        assert!(revise_draft(
            &mut d,
            Revision::Group {
                items: names(&["Animal Oil Residue", "Garcen Grease"])
            }
        )
        .is_err());
        // Too few Nanocubes for what went in: TT would vanish.
        let mut d = draft(vec![
            priced("Animal Oil Residue", 100.0, 0.0, 0.01),
            priced(NANOCUBE, 0.0, 40.0, 0.01),
        ]);
        assert!(revise_draft(
            &mut d,
            Revision::Group {
                items: names(&["Animal Oil Residue", NANOCUBE])
            }
        )
        .is_err());
        // A balanced one whose output's TT is only the account's.
        let mut output = priced("Garcen Grease", 0.0, 5.0, 0.2);
        output.unit_tt = UnitTt::Truncated { ped: 0.2 };
        let mut d = draft(vec![priced("Animal Oil Residue", 100.0, 0.0, 0.01), output]);
        assert!(revise_draft(
            &mut d,
            Revision::Group {
                items: names(&["Animal Oil Residue", "Garcen Grease"])
            }
        )
        .is_err());
    }

    #[test]
    fn shrapnel_sold_at_tt_is_at_most_what_is_missing() {
        let mut d = draft(vec![line(SHRAPNEL, 1000.0, 400.0)]);
        assert!(revise_draft(
            &mut d,
            Revision::ShrapnelSoldAtTt {
                quantity: Some(700.0)
            }
        )
        .is_err());
        let mut d = draft(vec![line(SHRAPNEL, 1000.0, 400.0)]);
        revise_draft(
            &mut d,
            Revision::ShrapnelSoldAtTt {
                quantity: Some(200.0),
            },
        )
        .unwrap();
        assert_eq!(
            d.lines[0].disposition,
            Disposition::ShrapnelConverted {
                sold_at_tt: Some(200.0)
            }
        );
    }

    #[test]
    fn the_account_s_truncated_tt_fits_the_app_s_unit_within_a_pec_a_row() {
        let stack = |quantity, tt_value, rows| ObservedStack {
            item_name: "Iron Stone".to_string(),
            quantity,
            tt_value,
            rows,
            single: false,
            unless_held: None,
            catalogue_unit_tt: None,
        };
        // 30 at 0.13 is 3.90; truncated per row it may read 3.89 over two.
        assert!(tt_fits(&stack(30, 3.90, 1), 0.13, 0.0));
        assert!(tt_fits(&stack(30, 3.89, 2), 0.13, 0.0));
        // Another item of the same name at another TT does not fit.
        assert!(!tt_fits(&stack(30, 1.50, 1), 0.13, 0.0));
        assert!(!tt_fits(&stack(30, 3.95, 1), 0.13, 0.0));
    }

    #[test]
    fn shrapnel_fits_the_game_s_constant_where_the_loot_average_falls_short() {
        let shrapnel = ObservedStack {
            item_name: "Shrapnel".to_string(),
            quantity: 31_731_376,
            tt_value: 3173.13,
            rows: 1,
            single: false,
            unless_held: None,
            catalogue_unit_tt: None,
        };
        // The loot lines, each rounded to the PEC, average a little low.
        let averaged = 104_438.611_4 / 1_045_010_067.0;
        assert!(!tt_fits(&shrapnel, averaged, 0.0));
        assert_eq!(constant_unit_tt("Shrapnel", Some(&shrapnel)), Some(0.0001));
        assert!(tt_fits(&shrapnel, 0.0001, 0.0));
    }

    #[test]
    fn the_loot_lines_rounding_is_allowed_for_and_a_real_difference_is_not() {
        let stack = |quantity, tt_value: f64| ObservedStack {
            item_name: "Robot Filter".to_string(),
            quantity,
            tt_value,
            rows: 2,
            single: false,
            unless_held: None,
            catalogue_unit_tt: None,
        };
        // 32,727 looted over 8,470 lines at 1,177.366 PED: 0.035975 a
        // unit, where the account shows 0.036.
        let unit = 1177.366 / 32_727.0;
        let rounding = 8470.0 * PEC / 2.0 / 32_727.0;
        assert!(!tt_fits(&stack(378, 13.60), unit, 0.0));
        assert!(tt_fits(&stack(378, 13.60), unit, rounding));
        // One loot line at a PEC against ten PED held: not rounding.
        let trophy = ObservedStack {
            item_name: "Carabok Trophy Head".to_string(),
            quantity: 1,
            tt_value: 10.0,
            rows: 1,
            single: true,
            unless_held: None,
            catalogue_unit_tt: None,
        };
        assert!(!tt_fits(&trophy, 0.01, PEC / 2.0));
    }

    #[tokio::test]
    async fn loot_of_any_kind_or_an_asset_bought_after_the_observation_moves_the_stock() {
        let (_dir, service) = write_service().await;
        service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                     VALUES ('s', 1000.0, 9000.0, 0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                     timestamp, shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, \
                     is_hof) VALUES ('k', 's', 'Atrox', 'Atrox', '', 5000.0, 1, 1.0, 0.1, 1.0, 0, 0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
                     VALUES ('k', 'Hunted Hide', 1, 1.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO harvest_events (id, session_id, timestamp) VALUES ('h', 's', 6000.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO harvest_loot_items (harvest_id, item_name, quantity, value_ped) \
                     VALUES ('h', 'Mined Ore', 1, 1.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completions (id, session_id, quest_id, completed_at) \
                     VALUES (1, 's', 7, 7000.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO session_quest_completion_reward_items \
                         (completion_id, item_name, quantity, value_ped) \
                     VALUES (1, 'Quest Token', 1, 1.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO inventory_items \
                         (id, name, tt_value, markup_paid, acquired_at, quantity, updated_at) \
                     VALUES ('a', 'Bought Oil', 1.0, 0.5, '2026-05-01', 10, 8000.0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO asset_purchases (position_id, quantity, tt_value, markup_paid, \
                         acquired_at, created_at) VALUES ('a', 10, 1.0, 0.5, '2026-05-01', 8000.0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let moved = |item: &'static str, at: f64| {
            service
                .db
                .with_reader(move |conn| Ok(stock_moved_since(conn, item, at)?))
        };
        for (item, when) in [
            ("Hunted Hide", 5000.0),
            ("Mined Ore", 6000.0),
            ("Quest Token", 7000.0),
            ("Bought Oil", 8000.0),
        ] {
            assert!(moved(item, when - 1.0).await.unwrap(), "{item} after");
            assert!(!moved(item, when).await.unwrap(), "{item} at or before");
        }
    }
}
