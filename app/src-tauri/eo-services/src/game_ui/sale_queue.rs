//! Reading the sell queue of the auction's sell window.
//!
//! The `AUCTION / SELL` window holds the player's inventory as a list on
//! its left and, beside it, the queue of items about to be listed: a
//! ruled table with one tall row per item. Each row prints the item's
//! name with its TT value under it, then spinners for the quantity, the
//! opening bid, the buyout and the days (a box with the number in it,
//! arrows above and below, sometimes a small figure under it), the fee,
//! and the markup the prices make, over a few rows of market statistics.
//!
//! Nothing here assumes where a column or a row sits. The headings name
//! the columns (each must own a column of its own, or the frame is
//! refused), the rule lines divide the rows, and within a cell the
//! profile says which piece of lettering is the value: the upper or lower
//! line of the cell's two-line stack, the number inside the spinner's box,
//! the figure under that box, the switch beside it, the topmost line, or
//! the cell's only line. Each value must then pass its grammar and, for a
//! number, be as wide as the client's numerals make it.
//!
//! The game prints some figures twice or derives them from others, and
//! those are checked against each other: the figure under the opening
//! bid is the bid's markup over the row's TT value, and so is the one
//! under an editable buyout for the buyout; the markup headline repeats
//! the figure for the price that sets it; the fee is the auction's fee
//! for the bid's markup ([`crate::auction_fee`]). The quantity and the
//! days are only bounded, by the stack size and the range the window
//! prints under them. A row is returned only when every field reads and
//! every check holds; otherwise it is reported unread, with why. What
//! each field of a row read has been checked by is told by
//! [`QueuedItem::assurance`].
//!
//! An item's name is free text no grammar can check. It is confirmed when
//! the application's bundled item data names it, or failing that when the
//! window's own inventory list shows it (weak evidence: see
//! [`ItemName::Listed`]); otherwise it is returned as unconfirmed, for the
//! player to check before anything keeps it.
//!
//! The client hints its lettering's height to whole pixels while the
//! width follows the interface scale, so the measures stated in lettering
//! heights (the numeral metrics and the word gap) hold at one scale only.
//! They are listed per scale, and the scale is told from the height of the
//! table's header band, rule to rule, before anything is read: exactly one
//! entry must claim it, or the queue is refused as drawn at a scale the
//! profile has not been measured at. Several other values were measured at
//! one scale and are stated relative to a row's or a run's height so that
//! they should carry to others: the header's share of a row, the picture's
//! share of a row, the stack gap, the spinner boxes' alignment and size
//! shares, and the brightness thresholds (lettering, locked and editable
//! spinners, the switches' colour).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value as Json;

use crate::auction_fee::listing_fee_ped;
use crate::fuzzy_match::ratio;
use crate::game_data_store::GameDataStore;
use crate::game_ui::geometry::BgrImage;

use super::field::{
    crop_margin, find_label, first_label, label_key, read_isolated, read_run, Recogniser,
    Remembering,
};
use super::geometry::{brightened, chroma, near, Pixels, Rect};
use super::locate::{locate, Located, ReadError};
use super::profile::{claiming, ScaleError, UiProfile};
use super::table::{
    column_rules, enclosed_regions, row_rules, rule_extent, table_shape, Band, Panel,
};
use super::text::{drawn_groups, run_below, run_right_of, text_runs, TextProfile, TextRun};
use super::validate::{
    expected_width, last_place, markup, parse, range, stack_size, width_agrees, Grammar,
    MarkupKind, MarkupText, NumeralMetrics,
};
use super::window::Side;

/// The profile key of the auction's sell window.
const WINDOW: &str = "auction_sell";

/// What the sell queue looks like, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaleQueueProfile {
    /// The heading over the queue, which the client follows with the
    /// number of rows queued: `Sell Queue (3)`.
    pub heading: String,
    /// The table's columns that hold a value, each by its heading.
    pub columns: Vec<QueueColumn>,
    /// The queue's lettering measures, one entry per interface scale
    /// measured. Exactly one entry must claim the scale the queue is
    /// drawn at, told from the table's geometry, or the queue is refused.
    pub scales: Vec<SaleQueueScale>,
    /// A name ending within this many of its run's heights of its
    /// column's right edge may have been cut to fit.
    pub cut_reach: f32,
    /// A share of the row height: how far a row may fall short of the
    /// table's row height and still count as whole.
    pub row_slack: f32,
    /// The header's height as a share of a row's: the client sizes both
    /// from its font, so a lone row cut short by the view is told from a
    /// whole one by it.
    pub header_height_share: f32,
    /// Lettering whose middle lies within this many of a heading's
    /// heights of the heading's own edges is on the headings' line.
    pub line_slack: f32,
    /// The two lines of a cell's stack stand at most this many of the
    /// upper line's heights apart.
    pub stack_gap: f32,
    /// Anything drawn in a cell at least this share of the row height
    /// tall is a picture in its frame, not lettering.
    pub picture_height: f32,
    /// How a spinner's box and its lettering are told apart.
    pub spinner: SpinnerProfile,
    /// How a lit switch is told from an unlit one.
    pub switch: SwitchProfile,
    /// The window's toggle between stating prices in PED and as markup.
    pub mode: ModeProfile,
    /// A name in the inventory list is compared with a queued one only
    /// when its run's width over height is within this share of the
    /// queued name's: the same text in the list's smaller type.
    pub name_aspect_slack: f32,
}

/// The queue's lettering at one interface scale. The client hints a
/// glyph's height to whole pixels while its width follows the scale, so
/// measures stated in lettering heights differ from one scale to the
/// next and are listed per scale.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaleQueueScale {
    /// The scale this entry is for, by the height in pixels of the
    /// queue's header band, from the rule above the headings to the rule
    /// under them (both ends included): the rules are drawn at a pitch
    /// the scale sets, so the band identifies the scale whether or not a
    /// whole row is in view, and before anything is read.
    pub header_height: [i32; 2],
    /// How wide the queue's numerals are: its amounts, spinners, count
    /// and markup headline. They are set smaller than the lettering the
    /// read profile's metrics were measured on, which narrows `PED`
    /// against the digits; its markups also need `TT+` and the percent
    /// sign.
    pub numerals: NumeralMetrics,
    /// How wide the numerals of the small figures under the spinners are.
    /// They are set in a smaller size than the rest, and the client hints
    /// each size's height to whole pixels on its own, so at one scale the
    /// two sizes' widths over heights can differ by more than either
    /// metric's tolerance allows a single set.
    pub figure_numerals: NumeralMetrics,
    /// The small, dim lettering of the figures under the spinners, whose
    /// gaps and sizes are stated in pixels.
    pub figure_lettering: TextProfile,
    /// A gap in a name's ink at least this many of its run's heights wide
    /// is a space between words; the client's letters stand closer.
    pub word_gap: f32,
}

/// One column of the queue, by its heading, and the values it holds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueColumn {
    pub heading: String,
    pub fields: Vec<FieldPick>,
}

/// Which value a column holds, and which of the cell's lettering it is.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldPick {
    pub field: QueueField,
    pub pick: Pick,
}

/// The values a queued row holds, and the helpers printed beside them.
/// Their order is the order a row is read in: a helper after the value
/// it bounds, the buyout's switch before the buyout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueField {
    Name,
    /// The TT value printed under the name.
    Tt,
    Quantity,
    /// The stack size printed under an editable quantity: `/ 52`.
    QuantityStack,
    OpeningBid,
    /// The markup figure printed under the opening bid's spinner.
    OpeningBidFigure,
    /// The tag beside the buyout's spinner, lit while a buyout is set.
    BuyoutSwitch,
    Buyout,
    /// The markup figure printed under the buyout's spinner.
    BuyoutFigure,
    Days,
    /// The range printed under the days: `1-7`.
    DaysRange,
    Fee,
    /// The markup headline at the top of the markup column.
    Markup,
}

impl QueueField {
    const ALL: [QueueField; 13] = [
        QueueField::Name,
        QueueField::Tt,
        QueueField::Quantity,
        QueueField::QuantityStack,
        QueueField::OpeningBid,
        QueueField::OpeningBidFigure,
        QueueField::BuyoutSwitch,
        QueueField::Buyout,
        QueueField::BuyoutFigure,
        QueueField::Days,
        QueueField::DaysRange,
        QueueField::Fee,
        QueueField::Markup,
    ];

    fn is_spinner(self) -> bool {
        matches!(
            self,
            QueueField::Quantity | QueueField::OpeningBid | QueueField::Buyout | QueueField::Days
        )
    }

    fn is_under_a_spinner(self) -> bool {
        matches!(
            self,
            QueueField::QuantityStack
                | QueueField::OpeningBidFigure
                | QueueField::BuyoutFigure
                | QueueField::DaysRange
        )
    }
}

/// Which of a cell's lettering is a value: a small closed set, so the
/// profile states a rule of the client's layout, never a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    /// The upper of the cell's two lines that share a left edge, one
    /// under the other, right of any picture: a name over its TT value.
    StackUpper,
    /// The lower of those two lines.
    StackLower,
    /// The lettering inside the spinner's box: the tallest box in the
    /// cell, its arrows' boxes stacked above and below it being shorter.
    Boxed,
    /// The first line under the spinner, overlapping its box.
    UnderBox,
    /// The largest closed shape right of the spinner's box, level with
    /// it: a switch's icon.
    BesideBox,
    /// The cell's topmost line, alone on it.
    Topmost,
    /// The cell's only line.
    Only,
}

/// How a spinner's box and its lettering are told apart.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpinnerProfile {
    /// Lettering in a box whose brightest channel is below this is no
    /// lettering: the box is empty, and nothing in it is brightened.
    pub min_peak: u8,
    /// Lettering in a box whose brightest channel is at most this is a
    /// locked spinner's: the client greys a value the player cannot
    /// change.
    pub locked_peak: u8,
    /// Lettering at least this bright is an editable spinner's. Between
    /// the two, which it is cannot be told, and the value is refused.
    pub editable_peak: u8,
    /// The brightest channel of an editable spinner's lettering. Dim
    /// lettering is brightened to it before it is found, read and
    /// measured, so the thresholds stated for bright lettering serve.
    pub lettering_peak: u8,
    /// Boxes stacked into one spinner line up on both sides to within
    /// this share of the value box's height.
    pub stack_align: f32,
    /// A second box in a cell at least this share of the tallest box's
    /// height is a second spinner's value box: the cell spans two columns.
    pub second_box_share: f32,
}

/// How a lit switch is told from an unlit one: by the colour of its ink,
/// lit in the client's accent colour, unlit in grey.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchProfile {
    /// The ink's mean colour spreads its channels at least this far when
    /// lit,
    pub lit_chroma: u8,
    /// and at most this far when unlit. Between, the switch is unread.
    pub unlit_chroma: u8,
}

/// The toggle between stating prices in PED and as markup: the readers'
/// spinners hold PED only in the mode this names.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeProfile {
    /// The toggle's label that must be lit.
    pub active: String,
    /// Its other labels, which must not be.
    pub others: Vec<String>,
}

/// Whether the player can change a spinner's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Drawn bright: the player can change it.
    Editable,
    /// Drawn greyed: fixed for this item, such as a single item's
    /// quantity.
    Locked,
}

/// A spinner's value, as its box shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpinnerValue {
    /// The number in the client's notation.
    pub text: String,
    pub value: u64,
    pub setting: Setting,
}

/// A price the player has set.
#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    /// The number in the client's notation.
    pub text: String,
    pub value: u64,
}

/// The buyout: switched off, or a price. A switched-off buyout's box
/// still shows a grey number (a `0`, or the last price set); it is no
/// price, so it is not carried.
#[derive(Debug, Clone, PartialEq)]
pub enum Buyout {
    /// Switched off: its box greyed and its tag unlit.
    Off,
    /// Set: its box bright and its tag lit.
    Price(Price),
}

/// An amount of PED as a cell shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Amount {
    /// The canonical spelling: `11.52 PED`.
    pub text: String,
    pub ped: f64,
}

/// An item's name, and what bears it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemName {
    /// The application's bundled item data names exactly this item.
    Catalogued(String),
    /// The window's inventory list shows exactly this name. Weak
    /// evidence: the list is read by the same recogniser in a like font,
    /// so a misreading the recogniser makes systematically (a letter it
    /// always takes for another) is repeated there and confirms itself.
    Listed(String),
    /// Read, but nothing bears it out: the caller shows it for the player
    /// to check and never keeps it unchecked.
    Unconfirmed(String),
}

impl ItemName {
    pub fn text(&self) -> &str {
        match self {
            ItemName::Catalogued(text) | ItemName::Listed(text) | ItemName::Unconfirmed(text) => {
                text
            }
        }
    }
}

/// What a field of a row read has been checked by, beyond its grammar
/// and the width of its ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assurance {
    /// Another figure the game prints fixes it: a misread digit anywhere
    /// in it breaks the agreement.
    CrossChecked,
    /// Other figures fix it only to within the rounding the client's
    /// printing hides: its last digit (more, for a percentage over a
    /// large TT value) can be misread with every check still agreeing.
    WithinWindow,
    /// Only bounds hold it: a digit misread for another of the same width
    /// inside the bounds passes.
    Bounded,
    /// Nothing but its grammar and its width: a digit misread for another
    /// of the same width passes.
    ReadOnly,
}

/// One queued row, every field read and every check the game's own
/// figures allow passed. What each field has been checked by is
/// [`QueuedItem::assurance`].
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedItem {
    pub name: ItemName,
    /// The TT value of the quantity queued.
    pub tt: Amount,
    /// The quantity, at least one, and for an editable quantity at most
    /// the stack size; within that a misread digit is not caught.
    pub quantity: SpinnerValue,
    /// The stack size printed under an editable quantity.
    pub stack: Option<u32>,
    pub opening_bid: SpinnerValue,
    pub buyout: Buyout,
    /// The days, within the range printed under them; within that a
    /// misread digit is not caught.
    pub days: SpinnerValue,
    /// The range of days the window allows.
    pub days_allowed: (u32, u32),
    pub fee: Amount,
    /// Whether the fee model admits only the printed fee for this row's
    /// opening bid and every TT value the printed one could stand for. At
    /// a boundary of the fee's last place it admits two, and the fee is
    /// then only bounded.
    pub fee_fixed: bool,
    /// The markup headline.
    pub markup: MarkupText,
}

impl QueuedItem {
    /// What `field` has been checked by.
    pub fn assurance(&self, field: QueueField) -> Assurance {
        match field {
            // A bid one PED out moves its figure, and the fee, by far more
            // than either's last place.
            QueueField::OpeningBid => Assurance::CrossChecked,
            // Two witnesses agree on whether it is set, and a price is
            // fixed by its own figure as the bid is.
            QueueField::Buyout | QueueField::BuyoutSwitch => Assurance::CrossChecked,
            QueueField::Fee if !self.fee_fixed => Assurance::Bounded,
            QueueField::Tt
            | QueueField::Fee
            | QueueField::Markup
            | QueueField::OpeningBidFigure
            | QueueField::BuyoutFigure => Assurance::WithinWindow,
            QueueField::Quantity if self.quantity.setting == Setting::Editable => {
                Assurance::Bounded
            }
            QueueField::Days => Assurance::Bounded,
            QueueField::Quantity
            | QueueField::QuantityStack
            | QueueField::DaysRange
            | QueueField::Name => Assurance::ReadOnly,
        }
    }
}

/// A figure the game prints that disagrees with the others it is
/// computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Identity {
    #[error("the figure under the opening bid is not the bid's markup over the TT value")]
    OpeningBidFigure,
    #[error("the figure under the buyout is not the buyout's markup over the TT value")]
    BuyoutFigure,
    #[error("the markup headline is not the figure under the price that sets it")]
    Headline,
    #[error("the fee is not the auction's fee for the bid's markup over the TT value")]
    Fee,
    #[error("the buyout's box and its tag disagree on whether a buyout is set")]
    BuyoutSwitch,
    #[error("the inventory list's count and the quantity queued do not make the stack size")]
    StackCount,
}

/// Why one field of a row did not read, or why the row's fields do not
/// hold together.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RowRefusal {
    #[error("{0:?}: no lettering stands where the value is drawn")]
    NotFound(QueueField),
    #[error("{0:?}: more than one piece of lettering could be the value")]
    Ambiguous(QueueField),
    #[error("{field:?} read as {raw:?}, which is not a valid value for it")]
    Invalid { field: QueueField, raw: String },
    #[error("{field:?} read as {raw:?}, but the figure on screen is not that wide")]
    WrongWidth { field: QueueField, raw: String },
    #[error("{field:?} read as {raw:?}, but one of its words read with a space inside it")]
    WordCount { field: QueueField, raw: String },
    #[error("{field:?} read as {raw:?}, outside the bounds the window gives it")]
    OutOfBounds { field: QueueField, raw: String },
    #[error("{0:?}: the value's ink touches its box, so it may be cut")]
    Clipped(QueueField),
    #[error("{0:?}: the client cut the text to fit its column")]
    Cut(QueueField),
    #[error("{0:?}: the lettering is neither bright as when set nor dim as when not")]
    Shading(QueueField),
    #[error(transparent)]
    Disagrees(Identity),
}

/// A whole row in view that could not be read exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct UnreadQueueRow {
    /// The recogniser's text for the name, when it had one.
    pub raw_name: Option<String>,
    /// Every reason, field by field; the checks between fields are made
    /// only when every field read.
    pub refusals: Vec<RowRefusal>,
    /// For a row whose every field read and whose only failed check is the
    /// fee's, every figure as printed: what a study of the fee model needs
    /// of a fee the model does not predict. Nothing that lists a row takes
    /// a figure from here.
    pub fee_disagreement: Option<FeeDisagreement>,
}

/// The printed figures of a row whose fee is not the model's for its bid.
#[derive(Debug, Clone, PartialEq)]
pub struct FeeDisagreement {
    pub name: String,
    pub tt: String,
    pub quantity: String,
    pub opening_bid: String,
    /// `None` for a buyout switched off.
    pub buyout: Option<String>,
    pub days: String,
    pub fee: String,
}

/// The heading's count against what the reading saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueCount {
    /// Every row the heading counts is in this reading: as many rows read
    /// as it counts, none unread and none partly in view.
    Complete(u32),
    /// The heading counts rows this reading does not hold: unread, partly
    /// in view, or out of view.
    Incomplete {
        heading: u32,
        read: usize,
        unread: usize,
        partial: usize,
    },
    /// The heading counts fewer rows than are seen: one of them is
    /// misread, so the count is not given.
    Contradicted { heading: u32, seen: usize },
    /// The heading's count did not read, or its digits are not as wide
    /// as its reading.
    Unread,
}

/// The sell queue, as far as it could be read.
#[derive(Debug, Clone, PartialEq)]
pub struct SaleQueue {
    /// Rows read whole, top to bottom.
    pub rows: Vec<QueuedItem>,
    /// Whole rows in view that did not read.
    pub unread: Vec<UnreadQueueRow>,
    /// Rows only partly in view: cut by the view, or covered by a window
    /// in front.
    pub partial: usize,
    pub count: QueueCount,
}

/// Why no queue could be read. Every variant is a refusal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SaleQueueError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("the profile does not describe the sell queue: {0}")]
    NotInProfile(String),
    #[error("the sell window is not open, or its title is covered or off the screen")]
    WindowNotFound,
    #[error("the sell window's extent is not certain (a side is covered, or the desktop scaled the picture)")]
    ExtentUncertain,
    #[error("the sell window runs off the screen")]
    WindowClipped,
    #[error("the queue's {0:?} heading was not found")]
    HeadingNotFound(String),
    #[error("the queue's columns are not all told apart: a divider is hidden")]
    ColumnsMerged,
    #[error("the queue's rule lines could not be found")]
    TableNotFound,
    #[error("the window's price toggle could not be read")]
    ModeUnread,
    #[error("the window states prices in another mode than the one the queue is read in")]
    OtherMode,
    /// The queue is drawn at an interface scale the profile lists no
    /// lettering measures for (or lists twice), told from its header
    /// band's height in pixels.
    #[error("{0}, the height of the queue's header band")]
    Scale(ScaleError),
}

impl From<ReadError> for SaleQueueError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::NoRecogniser => SaleQueueError::NoRecogniser,
            ReadError::NotInProfile(what) => SaleQueueError::NotInProfile(what),
            ReadError::WindowNotFound => SaleQueueError::WindowNotFound,
            ReadError::Field(_) => SaleQueueError::TableNotFound,
        }
    }
}

/// The bundled game data's collections whose entities are items a player
/// can hold and list.
pub const ITEM_ENDPOINTS: [&str; 9] = [
    "absorbers",
    "enhancers",
    "harvesting_tools",
    "medical_tools",
    "mindforce_implants",
    "stimulants",
    "weapon_amplifiers",
    "weapon_vision_attachments",
    "weapons",
];

/// An entity's name, and its `economy.max_tt` where it has one.
fn catalogue_entry(entity: &Json) -> Option<(String, Option<f64>)> {
    let name = entity.get("name")?.as_str()?.to_string();
    let max_tt = entity
        .get("economy")
        .and_then(|e| e.get("max_tt"))
        .and_then(Json::as_f64);
    Some((name, max_tt))
}

/// The items the application knows, by name, with the most TT value one
/// of each can hold where its data gives one.
#[derive(Debug, Clone, Default)]
pub struct ItemCatalogue {
    items: BTreeMap<String, Option<f64>>,
}

impl ItemCatalogue {
    pub fn new(items: impl IntoIterator<Item = (String, Option<f64>)>) -> Self {
        Self {
            items: items
                .into_iter()
                .map(|(name, max_tt)| (normalised_name(&name), max_tt))
                .collect(),
        }
    }

    /// From the bundled game data's item entities: each one's `name`, and
    /// its `economy.max_tt` where it has one.
    pub fn from_entities(entities: &[Json]) -> Self {
        Self::new(entities.iter().filter_map(catalogue_entry))
    }

    /// From the bundled game data: every entity of the collections that
    /// hold items ([`ITEM_ENDPOINTS`]).
    pub fn from_store(store: &GameDataStore) -> Self {
        Self::new(
            ITEM_ENDPOINTS
                .iter()
                .flat_map(|endpoint| store.get_entities(endpoint))
                .filter_map(catalogue_entry),
        )
    }

    /// Whether the catalogue names `name` exactly, and the item's most TT
    /// value if it does.
    fn find(&self, name: &str) -> Option<Option<f64>> {
        self.items.get(name).copied()
    }
}

/// Read the count from the queue's heading, `Sell Queue (3)`: text
/// matching `heading` (loosely, it is only an anchor; an icon before it
/// may read as a stray character), then the count in brackets and
/// nothing after. The count must be a whole number without a leading
/// zero.
pub fn heading_count(text: &str, heading: &str, label_score: f64) -> Option<u32> {
    let text = text.trim();
    let open = text.rfind(['(', '\u{ff08}'])?;
    let (label, rest) = text.split_at(open);
    let inner = rest
        .trim_start_matches(['(', '\u{ff08}'])
        .trim_end()
        .strip_suffix([')', '\u{ff09}'])?;
    if ratio(&label_key(label), &label_key(heading)) < label_score {
        return None;
    }
    parse(Grammar::Integer, inner)?.text.parse().ok()
}

/// A name as the comparison and the caller see it: fullwidth forms the
/// recogniser can emit folded to their ASCII letters, runs of space
/// made one, and the ends trimmed. Nothing else is changed, so a name
/// misread by a single character never equals the right one.
pub fn normalised_name(raw: &str) -> String {
    let folded: String = raw
        .chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => {
                char::from_u32(c as u32 - ('\u{ff01}' as u32 - '!' as u32)).unwrap_or(c)
            }
            '\u{3000}' => ' ',
            other => other,
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How far apart two amounts computed in floating point may be and still
/// be the same amount.
const EPSILON: f64 = 1e-9; // not layout: floating-point tolerance of PED arithmetic

/// The scale a markup is stated in as a percentage.
const PERCENT: f64 = 100.0; // not layout: the percent scale

/// The game's currency: a PED is a hundred PEC.
const PEC_PER_PED: f64 = 100.0; // not layout: the currency's own subdivision

/// Whether a figure under a spinner agrees with the price in the spinner
/// and the row's TT value. The client prints the TT value and the figure
/// each to a last place, cutting or rounding a finer true value, so the
/// check allows exactly what that can hide: a markup in PED is the price
/// less the TT value, less at most one unit of its last place; a
/// percentage is the price over the TT value, for any true TT value the
/// printed one could stand for, to within one unit of its own last place.
/// Within that window a misread digit is not caught: a TT value read one
/// unit high in its last place agrees as well as the true one.
pub fn figure_agrees(figure: &MarkupText, price: f64, tt: &Amount) -> bool {
    match figure.kind {
        MarkupKind::AboveTt => {
            let short = price - tt.ped - figure.figure;
            short >= -EPSILON && short <= last_place(&figure.text) + EPSILON
        }
        MarkupKind::Percent => {
            let unit = last_place(&tt.text);
            let own = last_place(&figure.text);
            let least = PERCENT * price / (tt.ped + unit) - own;
            let most = if tt.ped - unit / 2.0 > 0.0 {
                PERCENT * price / (tt.ped - unit / 2.0) + own
            } else {
                f64::INFINITY
            };
            figure.figure >= least - EPSILON && figure.figure <= most + EPSILON
        }
    }
}

/// Whether a fee is the auction's listing fee for the opening bid over
/// the TT value, for some true TT value the printed one could stand for
/// (the client cuts or rounds it to its last place, then cuts the fee
/// again). The fee grows with the markup, so the fees possible are those
/// between the least and the most markup the printing allows. The fee
/// model ([`listing_fee_ped`]) was measured against the game: it depends
/// on the opening bid's markup alone, not on the buyout, the quantity or
/// the days. Every queue captured here lists for seven days, so these
/// captures cannot themselves exclude a dependence on days; the model's
/// own measurements do.
pub fn fee_agrees(fee: &Amount, bid: f64, tt: &Amount) -> bool {
    let (least, most) = fee_window(bid, tt);
    let fee = (fee.ped * PEC_PER_PED).round() as i64;
    fee >= least && fee <= most
}

/// The least and the most fee, in PEC, the auction charges for `bid` over
/// any true TT value the printed one could stand for. When they differ, a
/// fee read one PEC out at that boundary agrees as well as the true one.
pub fn fee_window(bid: f64, tt: &Amount) -> (i64, i64) {
    let unit = last_place(&tt.text);
    let pec = |ped: f64| (ped * PEC_PER_PED).round() as i64;
    (
        pec(listing_fee_ped(bid - tt.ped - unit)),
        pec(listing_fee_ped(bid - tt.ped + unit / 2.0)),
    )
}

/// Why no value box could be told in a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoxFault {
    /// No closed box at all.
    None,
    /// Two boxes of the same height, neither the value's.
    Tie,
    /// A second box nearly as tall: the cell holds two spinners.
    Two,
}

/// The value box among a cell's closed shapes: the tallest, alone, with
/// no other within `second_share` of its height.
fn value_box(regions: &[Rect], second_share: f32) -> Result<Rect, BoxFault> {
    let tallest = regions
        .iter()
        .max_by_key(|r| r.height())
        .ok_or(BoxFault::None)?;
    let others: Vec<&Rect> = regions.iter().filter(|r| *r != tallest).collect();
    if others.iter().any(|r| r.height() == tallest.height()) {
        return Err(BoxFault::Tie);
    }
    if others
        .iter()
        .any(|r| r.height() as f32 >= second_share * tallest.height() as f32)
    {
        return Err(BoxFault::Two);
    }
    Ok(*tallest)
}

/// Where a value's lettering is, and the image it is read in.
struct Ink {
    /// The lettering brightened into a crop of its own, or `None` when it
    /// is read where it stands in the frame.
    crop: Option<BgrImage>,
    /// The lettering's run, in the coordinates of the image it is read in.
    rect: Rect,
    /// A spinner's setting, from how bright its lettering is drawn.
    setting: Option<Setting>,
    /// A switch's state, for a switch.
    lit: Option<bool>,
    /// The cell, in the frame's coordinates.
    cell: Rect,
}

/// A value that passed its grammar and its width.
enum Value {
    Text(String),
    Amount(Amount),
    Spinner(SpinnerValue),
    Markup(MarkupText),
    Bound(u32),
    Range((u32, u32)),
    Switch(bool),
    Off,
}

/// A spinner found in a cell: its value's box and how far down the
/// boxes stacked with it reach.
struct Spinner {
    value: Rect,
    bottom: i32,
}

/// What went wrong in a cell: a row's refusal, or a sign that the cell
/// spans two columns, which refuses the frame.
enum Fault {
    Row(RowRefusal),
    Merged,
}

impl From<RowRefusal> for Fault {
    fn from(refusal: RowRefusal) -> Self {
        Fault::Row(refusal)
    }
}

/// Everything one read of a frame shares.
struct Reader<'a> {
    image: &'a BgrImage,
    profile: &'a UiProfile,
    queue: &'a SaleQueueProfile,
    recogniser: &'a dyn Recogniser,
    located: &'a Located,
    runs: &'a [TextRun],
    panel: Panel,
    /// The table's row height.
    pitch: i32,
}

/// The words of a name: the stretches of `rect`'s ink parted by gaps at
/// least `gap` pixels wide.
fn words<P: Pixels>(image: &P, rect: Rect, min_ink: u8, gap: i32) -> Vec<Rect> {
    let inked = |x: i32| {
        (rect.y0..=rect.y1).any(|y| {
            image.in_bounds(x, y) && {
                let [r, g, b] = image.rgb(x, y);
                r.max(g).max(b) >= min_ink
            }
        })
    };
    let mut out: Vec<Rect> = Vec::new();
    let mut last_ink: Option<i32> = None;
    for x in rect.x0..=rect.x1 {
        if !inked(x) {
            continue;
        }
        match (out.last_mut(), last_ink) {
            (Some(word), Some(last)) if x - last - 1 < gap => word.x1 = x,
            _ => out.push(Rect::new(x, rect.y0, x, rect.y1)),
        }
        last_ink = Some(x);
    }
    out
}

/// The ink of the bracketed count that ends the heading's run, `(3)`:
/// from the nearest glyph before the closing bracket that is as tall as
/// it (the opening bracket: the brackets reach above and below the
/// digits) to the run's end. The digits can touch the closing bracket,
/// so the count is measured with its brackets.
fn count_ink<P: Pixels>(image: &P, rect: Rect, min_ink: u8) -> Option<Rect> {
    let inked = |x: i32, y: i32| {
        image.in_bounds(x, y) && {
            let [r, g, b] = image.rgb(x, y);
            r.max(g).max(b) >= min_ink
        }
    };
    let glyphs: Vec<Rect> = words(image, rect, min_ink, 1)
        .iter()
        .filter_map(|g| {
            let rows: Vec<i32> = (g.y0..=g.y1)
                .filter(|&y| (g.x0..=g.x1).any(|x| inked(x, y)))
                .collect();
            Some(Rect::new(g.x0, *rows.first()?, g.x1, *rows.last()?))
        })
        .collect();
    let (close, rest) = glyphs.split_last()?;
    let open = rest.iter().rposition(|g| g.height() >= close.height())?;
    let count = &glyphs[open..];
    Some(Rect::new(
        count[0].x0,
        count.iter().map(|g| g.y0).min()?,
        close.x1,
        count.iter().map(|g| g.y1).max()?,
    ))
}

fn centre_y(rect: &Rect) -> i32 {
    (rect.y0 + rect.y1) / 2
}

fn within(inner: &Rect, outer: &Rect) -> bool {
    inner.x0 >= outer.x0 && inner.x1 <= outer.x1 && inner.y0 >= outer.y0 && inner.y1 <= outer.y1
}

/// Whether a window in front of the sell window lies over `rect`.
fn covered(located: &Located, rect: Rect) -> bool {
    located
        .in_front
        .iter()
        .any(|front| front.intersect(&rect).is_some())
}

/// Whether the ink in `rect` is lit: `Some(true)` in the accent colour,
/// `Some(false)` grey, `None` when it cannot be told.
fn lit(
    image: &BgrImage,
    rect: Rect,
    profile: &UiProfile,
    queue: &SaleQueueProfile,
) -> Option<bool> {
    let rect = rect.intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1))?;
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for y in rect.y0..=rect.y1 {
        for x in rect.x0..=rect.x1 {
            let rgb = image.rgb(x, y);
            if rgb.iter().copied().max().unwrap_or(0) >= profile.text.min_ink {
                for (s, c) in sum.iter_mut().zip(rgb) {
                    *s += c as u64;
                }
                n += 1;
            }
        }
    }
    if n == 0 {
        return None;
    }
    let mean = sum.map(|s| (s / n) as u8);
    let spread = chroma(mean);
    if spread >= queue.switch.lit_chroma {
        Some(true)
    } else if spread <= queue.switch.unlit_chroma {
        Some(false)
    } else {
        None
    }
}

impl Reader<'_> {
    fn covered(&self, rect: Rect) -> bool {
        covered(self.located, rect)
    }

    fn read(&self, image: &BgrImage, rect: Rect) -> Result<String, SaleQueueError> {
        read_run(image, rect, &self.profile.read, self.recogniser)
            .map(|(text, _)| text)
            .ok_or(SaleQueueError::NoRecogniser)
    }

    /// The runs wholly inside `cell`.
    fn cell_runs(&self, cell: Rect) -> Vec<&TextRun> {
        self.runs
            .iter()
            .filter(|r| within(&r.rect, &cell))
            .collect()
    }

    fn lit(&self, rect: Rect) -> Option<bool> {
        lit(self.image, rect, self.profile, self.queue)
    }

    /// The cell's two-line stack, right of any picture: `(upper, lower)`.
    fn stack(&self, cell: Rect, field: QueueField) -> Result<(TextRun, TextRun), RowRefusal> {
        // A picture's frame is drawn dim, so the frame is one group of
        // non-panel ink as tall as itself; the picture's own bright ink
        // can lie close enough to the lettering to join its run, so the
        // lettering is looked for right of the frame.
        let tall = (self.queue.picture_height * self.pitch as f32).round() as i32;
        let left = drawn_groups(
            self.image,
            cell,
            self.panel.rgb,
            self.panel.tolerance,
            &self.profile.text,
        )
        .iter()
        .filter(|g| g.height() >= tall)
        .map(|g| g.x1 + 1)
        .max()
        .unwrap_or(cell.x0);
        let area = Rect::new(left, cell.y0, cell.x1, cell.y1);
        let runs: Vec<TextRun> = text_runs(self.image, area, &self.profile.text)
            .into_iter()
            .filter(|r| !self.covered(r.rect))
            .collect();
        let reach = self.profile.read.below_reach;
        let pairs: Vec<(&TextRun, &TextRun)> = runs
            .iter()
            .filter_map(|upper| {
                let lower = run_below(upper, &runs, reach)?;
                let gap = lower.rect.y0 - upper.rect.y1 - 1;
                (gap as f32 <= self.queue.stack_gap * upper.rect.height() as f32)
                    .then_some((upper, lower))
            })
            .collect();
        match pairs.as_slice() {
            [(upper, lower)] => Ok(((*upper).clone(), (*lower).clone())),
            [] => Err(RowRefusal::NotFound(field)),
            _ => Err(RowRefusal::Ambiguous(field)),
        }
    }

    /// The spinner in a cell: its value's box is the tallest box there,
    /// and no other box comes near it.
    fn spinner(&self, cell: Rect, field: QueueField) -> Result<Spinner, Fault> {
        let regions = enclosed_regions(self.image, cell, self.panel);
        let tallest = match value_box(&regions, self.queue.spinner.second_box_share) {
            Ok(rect) => rect,
            Err(BoxFault::None) => return Err(RowRefusal::NotFound(field).into()),
            Err(BoxFault::Tie) => return Err(RowRefusal::Ambiguous(field).into()),
            Err(BoxFault::Two) => return Err(Fault::Merged),
        };
        let align = (self.queue.spinner.stack_align * tallest.height() as f32).round() as i32;
        let bottom = regions
            .iter()
            .filter(|r| (r.x0 - tallest.x0).abs() <= align && (r.x1 - tallest.x1).abs() <= align)
            .map(|r| r.y1)
            .max()
            .unwrap_or(tallest.y1);
        Ok(Spinner {
            value: tallest,
            bottom,
        })
    }

    /// The brightest channel of the lettering in `rect`: pixels not the
    /// panel's fill, or at least `min_ink` when it is given.
    fn peak(&self, rect: Rect, min_ink: Option<u8>) -> u8 {
        let mut peak = 0u8;
        let Some(rect) = rect.intersect(&Rect::new(
            0,
            0,
            self.image.width() - 1,
            self.image.height() - 1,
        )) else {
            return 0;
        };
        for y in rect.y0..=rect.y1 {
            for x in rect.x0..=rect.x1 {
                let rgb = self.image.rgb(x, y);
                let brightest = rgb[0].max(rgb[1]).max(rgb[2]);
                let ink = match min_ink {
                    Some(floor) => brightest >= floor,
                    None => !near(rgb, self.panel.rgb, self.panel.tolerance),
                };
                if ink {
                    peak = peak.max(brightest);
                }
            }
        }
        peak
    }

    fn boxed(&self, cell: Rect, field: QueueField) -> Result<Ink, Fault> {
        let spinner = self.spinner(cell, field)?;
        let peak = self.peak(spinner.value, None);
        let settings = &self.queue.spinner;
        // Too dim to be lettering: an empty box, not ink to brighten.
        let setting = if peak < settings.min_peak {
            return Err(RowRefusal::NotFound(field).into());
        } else if peak <= settings.locked_peak {
            Setting::Locked
        } else if peak >= settings.editable_peak {
            Setting::Editable
        } else {
            return Err(RowRefusal::Shading(field).into());
        };
        let crop = brightened(self.image, spinner.value, peak, settings.lettering_peak);
        let whole = Rect::new(0, 0, crop.w as i32 - 1, crop.h as i32 - 1);
        let runs = text_runs(&crop, whole, &self.profile.text);
        let run = match runs.as_slice() {
            [run] => run.rect,
            [] => return Err(RowRefusal::NotFound(field).into()),
            _ => return Err(RowRefusal::Ambiguous(field).into()),
        };
        // A number reaching the box's sides may run on past them: a large
        // stack scrolled or cut to fit.
        if run.x0 <= whole.x0 || run.x1 >= whole.x1 || run.y0 <= whole.y0 || run.y1 >= whole.y1 {
            return Err(RowRefusal::Clipped(field).into());
        }
        Ok(Ink {
            rect: run,
            crop: Some(crop),
            setting: Some(setting),
            lit: None,
            cell,
        })
    }

    fn under_box(
        &self,
        cell: Rect,
        field: QueueField,
        scale: &SaleQueueScale,
    ) -> Result<Ink, Fault> {
        let spinner = self.spinner(cell, field)?;
        let lettering = &scale.figure_lettering;
        // Only what lies under the spinner is looked at: at this dim a
        // threshold the boxes' own frames are ink, and would join it.
        let below = Rect::new(cell.x0, spinner.bottom + 1, cell.x1, cell.y1);
        let runs: Vec<TextRun> = text_runs(self.image, below, lettering)
            .into_iter()
            .filter(|r| {
                !self.covered(r.rect)
                    && r.rect.x1 >= spinner.value.x0
                    && r.rect.x0 <= spinner.value.x1
            })
            .collect();
        let top = runs
            .iter()
            .min_by_key(|r| r.rect.y0)
            .ok_or(RowRefusal::NotFound(field))?;
        if runs.iter().any(|r| {
            r.rect != top.rect
                && centre_y(&r.rect) >= top.rect.y0
                && centre_y(&r.rect) <= top.rect.y1
        }) {
            return Err(RowRefusal::Ambiguous(field).into());
        }
        let peak = self.peak(top.rect, Some(lettering.min_ink));
        let margin = crop_margin(top.rect.height(), &self.profile.read);
        let area = top
            .rect
            .padded(margin)
            .intersect(&cell)
            .ok_or(RowRefusal::NotFound(field))?;
        Ok(Ink {
            crop: Some(brightened(
                self.image,
                area,
                peak,
                self.queue.spinner.lettering_peak,
            )),
            rect: top.rect.offset(-area.x0, -area.y0),
            setting: None,
            lit: None,
            cell,
        })
    }

    fn beside_box(&self, cell: Rect, field: QueueField) -> Result<Ink, Fault> {
        let spinner = self.spinner(cell, field)?;
        let value = spinner.value;
        // The icon's own closed shape: the largest right of the box and
        // level with it (the digits beside the box close smaller ones).
        let icon = enclosed_regions(self.image, cell, self.panel)
            .into_iter()
            .filter(|r| r.x0 > value.x1 && centre_y(r) >= value.y0 && centre_y(r) <= value.y1)
            .max_by_key(|r| r.width() * r.height())
            .ok_or(RowRefusal::NotFound(field))?;
        let around = icon.padded(icon.height() / 2);
        let lit = self.lit(around).ok_or(RowRefusal::Shading(field))?;
        Ok(Ink {
            crop: None,
            rect: around,
            setting: None,
            lit: Some(lit),
            cell,
        })
    }

    fn topmost(&self, cell: Rect, field: QueueField) -> Result<Ink, Fault> {
        let runs = self.cell_runs(cell);
        let top = runs
            .iter()
            .min_by_key(|r| r.rect.y0)
            .ok_or(RowRefusal::NotFound(field))?;
        if runs.iter().any(|r| {
            r.rect != top.rect
                && centre_y(&r.rect) >= top.rect.y0
                && centre_y(&r.rect) <= top.rect.y1
        }) {
            return Err(RowRefusal::Ambiguous(field).into());
        }
        Ok(self.plain(top.rect, cell))
    }

    fn only(&self, cell: Rect, field: QueueField) -> Result<Ink, Fault> {
        match self.cell_runs(cell).as_slice() {
            [run] => Ok(self.plain(run.rect, cell)),
            [] => Err(RowRefusal::NotFound(field).into()),
            _ => Err(RowRefusal::Ambiguous(field).into()),
        }
    }

    fn plain(&self, rect: Rect, cell: Rect) -> Ink {
        Ink {
            crop: None,
            rect,
            setting: None,
            lit: None,
            cell,
        }
    }

    fn ink(
        &self,
        cell: Rect,
        pick: Pick,
        field: QueueField,
        scale: &SaleQueueScale,
    ) -> Result<Ink, Fault> {
        match pick {
            Pick::StackUpper => Ok(self.plain(self.stack(cell, field)?.0.rect, cell)),
            Pick::StackLower => Ok(self.plain(self.stack(cell, field)?.1.rect, cell)),
            Pick::Boxed => self.boxed(cell, field),
            Pick::UnderBox => self.under_box(cell, field, scale),
            Pick::BesideBox => self.beside_box(cell, field),
            Pick::Topmost => self.topmost(cell, field),
            Pick::Only => self.only(cell, field),
        }
    }

    /// Read a name word by word: the ink shows where its spaces are, and
    /// the recogniser can drop one between words it reads together.
    fn name(
        &self,
        ink: &Ink,
        field: QueueField,
        scale: &SaleQueueScale,
    ) -> Result<(String, Result<Value, RowRefusal>), SaleQueueError> {
        let height = ink.rect.height() as f32;
        let gap = (scale.word_gap * height).round() as i32;
        let mut read = Vec::new();
        for word in words(self.image, ink.rect, self.profile.text.min_ink, gap) {
            let text = read_isolated(
                self.image,
                word,
                (word.x0 - 1, word.x1 + 1),
                self.panel.rgb,
                &self.profile.read,
                self.recogniser,
            )
            .map(|(text, _)| normalised_name(&text))
            .ok_or(SaleQueueError::NoRecogniser)?;
            read.push(text);
        }
        let raw = read.join(" ");
        let value = if read.iter().any(|w| w.contains(' ')) {
            Err(RowRefusal::WordCount {
                field,
                raw: raw.clone(),
            })
        } else if read.iter().any(String::is_empty) || !raw.chars().any(char::is_alphabetic) {
            Err(RowRefusal::Invalid {
                field,
                raw: raw.clone(),
            })
        } else if raw.ends_with(['.', '\u{2026}'])
            || (ink.cell.x1 - ink.rect.x1) as f32 <= self.queue.cut_reach * height
        {
            Err(RowRefusal::Cut(field))
        } else {
            Ok(Value::Text(raw.clone()))
        };
        Ok((raw, value))
    }

    /// Read one field's ink and check it by its grammar and its width at
    /// the queue's scale: the recogniser's text, and the value or why it
    /// was refused.
    fn value(
        &self,
        field: QueueField,
        ink: &Ink,
        scale: &SaleQueueScale,
    ) -> Result<(String, Result<Value, RowRefusal>), SaleQueueError> {
        if field == QueueField::Name {
            return self.name(ink, field, scale);
        }
        if let Some(lit) = ink.lit {
            return Ok((String::new(), Ok(Value::Switch(lit))));
        }
        let numerals = if field.is_under_a_spinner() {
            &scale.figure_numerals
        } else {
            &scale.numerals
        };
        let image = ink.crop.as_ref().unwrap_or(self.image);
        let raw = self.read(image, ink.rect)?;
        let invalid = || RowRefusal::Invalid {
            field,
            raw: raw.clone(),
        };
        // Every number here is one the metrics cover: a text they did not
        // would pass unmeasured, so it is refused instead.
        let wide = |text: &str| {
            expected_width(text, numerals).is_some()
                && width_agrees(image, ink.rect, text, numerals)
        };
        let wrong_width = || RowRefusal::WrongWidth {
            field,
            raw: raw.clone(),
        };
        let value = match field {
            QueueField::Name | QueueField::BuyoutSwitch => Err(invalid()),
            QueueField::Tt | QueueField::Fee => match parse(Grammar::PedAmount, &raw) {
                None => Err(invalid()),
                Some(p) if !wide(&p.text) => Err(wrong_width()),
                Some(p) => Ok(Value::Amount(Amount {
                    ped: p.number.expect("an amount has a number"),
                    text: p.text,
                })),
            },
            QueueField::Quantity
            | QueueField::OpeningBid
            | QueueField::Buyout
            | QueueField::Days => match parse(Grammar::Integer, &raw) {
                None => Err(invalid()),
                Some(p) if !wide(&p.text) => Err(wrong_width()),
                Some(p) => match (p.text.parse::<u64>(), ink.setting) {
                    (Ok(value), Some(setting)) => Ok(Value::Spinner(SpinnerValue {
                        text: p.text,
                        value,
                        setting,
                    })),
                    _ => Err(invalid()),
                },
            },
            // The helpers' slash and dash are not in the metrics: they are
            // bounds, read by grammar alone.
            QueueField::QuantityStack => stack_size(&raw).map(Value::Bound).ok_or_else(invalid),
            QueueField::DaysRange => range(&raw).map(Value::Range).ok_or_else(invalid),
            QueueField::OpeningBidFigure | QueueField::BuyoutFigure | QueueField::Markup => {
                match markup(&raw) {
                    None => Err(invalid()),
                    Some(m) if !wide(&m.text) => Err(wrong_width()),
                    Some(m) => Ok(Value::Markup(m)),
                }
            }
        };
        Ok((raw, value))
    }
}

/// Every field's column and pick, from the profile, checked to name each
/// field exactly once and to pair each pick with a field it can serve.
fn plan(queue: &SaleQueueProfile) -> Result<BTreeMap<QueueField, (usize, FieldPick)>, String> {
    let mut plan = BTreeMap::new();
    for (index, column) in queue.columns.iter().enumerate() {
        for pick in &column.fields {
            if plan.insert(pick.field, (index, pick.clone())).is_some() {
                return Err(format!("{:?} is placed twice", pick.field));
            }
        }
    }
    for field in QueueField::ALL {
        let Some((index, pick)) = plan.get(&field) else {
            return Err(format!("{field:?} is not placed"));
        };
        let has_spinner = plan
            .values()
            .any(|(i, p)| i == index && p.pick == Pick::Boxed);
        let fits = match pick.pick {
            Pick::Boxed => field.is_spinner(),
            // A helper is found under its column's spinner, a switch
            // beside it.
            Pick::UnderBox => field.is_under_a_spinner() && has_spinner,
            Pick::BesideBox => field == QueueField::BuyoutSwitch && has_spinner,
            _ => {
                !field.is_spinner()
                    && !field.is_under_a_spinner()
                    && field != QueueField::BuyoutSwitch
            }
        };
        if !fits {
            return Err(format!("{field:?} cannot be read as {:?}", pick.pick));
        }
    }
    Ok(plan)
}

/// Which field is read from which column, and how.
type Plan = BTreeMap<QueueField, (usize, FieldPick)>;

/// The sell queue's profile entry, and its plan of which field is read
/// from which column.
fn queue_profile(profile: &UiProfile) -> Result<(&SaleQueueProfile, Plan), SaleQueueError> {
    let queue = profile
        .windows
        .get(WINDOW)
        .and_then(|w| w.sale_queue.as_ref())
        .ok_or_else(|| SaleQueueError::NotInProfile("no sale_queue entry".into()))?;
    let plan = plan(queue).map_err(SaleQueueError::NotInProfile)?;
    Ok((queue, plan))
}

/// The queue's table as a frame shows it, found from its headings and
/// rule lines before anything in its rows is read.
struct Table {
    located: Located,
    runs: Vec<TextRun>,
    panel: Panel,
    /// The header band, from the row under the rule above the headings to
    /// the row over the rule under them.
    header: (i32, i32),
    /// The table's left and right edges.
    extent: (i32, i32),
    /// Each column's inner edges, in the profile's order of columns.
    columns: Vec<(i32, i32)>,
    /// The window's lettering above the table: the heading and the price
    /// toggle.
    above: Vec<TextRun>,
    /// The bands under the header, each with whether it is a whole row.
    rows: Vec<(Band, bool)>,
    /// A row's height, from the header's.
    row_height: i32,
}

impl Table {
    /// The header band's height in pixels, rule to rule: what tells the
    /// interface scale the queue is drawn at.
    fn header_height(&self) -> i32 {
        self.header.1 - self.header.0 + 1
    }

    fn reader<'a>(
        &'a self,
        image: &'a BgrImage,
        profile: &'a UiProfile,
        queue: &'a SaleQueueProfile,
        recogniser: &'a dyn Recogniser,
    ) -> Reader<'a> {
        Reader {
            image,
            profile,
            queue,
            recogniser,
            located: &self.located,
            runs: &self.runs,
            panel: self.panel,
            pitch: self.row_height,
        }
    }

    /// The whole rows in view, each with its band: rows cut by the view
    /// or covered by a window in front are left out.
    fn whole_rows(&self, queue: &SaleQueueProfile) -> Vec<&Band> {
        let least = self.row_height as f32 * (1.0 - queue.row_slack);
        self.rows
            .iter()
            .filter(|(band, full)| {
                let row = Rect::new(self.extent.0, band.top, self.extent.1, band.bottom);
                *full && band.height() as f32 >= least && !covered(&self.located, row)
            })
            .map(|(band, _)| band)
            .collect()
    }
}

/// Find the queue's table: the window, its headings, rule lines and
/// columns, and the price toggle in the mode the reader is built on.
fn find_table(
    image: &BgrImage,
    profile: &UiProfile,
    queue: &SaleQueueProfile,
    recogniser: &dyn Recogniser,
) -> Result<Table, SaleQueueError> {
    let located = locate(image, profile, WINDOW, recogniser)?;
    let window = &located.window;
    if !window.extent_certain() {
        return Err(SaleQueueError::ExtentUncertain);
    }
    if [window.left, window.top, window.right].contains(&Side::BeyondFrame) {
        return Err(SaleQueueError::WindowClipped);
    }
    let panel = Panel {
        rgb: profile.chrome.panel_rgb,
        tolerance: profile.chrome.panel_tolerance,
    };
    let runs = located.runs(image, profile);
    let no_recogniser = |_| SaleQueueError::NoRecogniser;

    // The headings: the first that reads anchors their line, and every
    // other is looked for along it.
    let mut anchor = None;
    for column in &queue.columns {
        if let Some(run) = first_label(image, &runs, &column.heading, &profile.read, recogniser)
            .map_err(no_recogniser)?
        {
            anchor = Some(run.clone());
            break;
        }
    }
    let anchor = anchor.ok_or_else(|| {
        SaleQueueError::HeadingNotFound(
            queue
                .columns
                .first()
                .map(|c| c.heading.clone())
                .unwrap_or_default(),
        )
    })?;
    let slack = (queue.line_slack * anchor.rect.height() as f32).round() as i32;
    let line_runs: Vec<TextRun> = text_runs(
        image,
        Rect::new(
            window.rect.x0,
            anchor.rect.y0 - slack,
            window.rect.x1,
            anchor.rect.y1 + slack,
        ),
        &profile.text,
    )
    .into_iter()
    .filter(|r| !covered(&located, r.rect))
    .collect();
    let mut headings = Vec::with_capacity(queue.columns.len());
    for column in &queue.columns {
        let run = find_label(
            image,
            &line_runs,
            &column.heading,
            &profile.read,
            recogniser,
        )
        .map_err(no_recogniser)?
        .ok_or_else(|| SaleQueueError::HeadingNotFound(column.heading.clone()))?;
        headings.push(run.rect);
    }

    // Rows are the bands between the table's rule lines, found across
    // the headings' span, which lies inside the table.
    let span = (
        headings
            .iter()
            .map(|r| r.x0)
            .min()
            .unwrap_or(window.rect.x0),
        headings
            .iter()
            .map(|r| r.x1)
            .max()
            .unwrap_or(window.rect.x1),
    );
    let lines = row_rules(
        image,
        Rect::new(span.0, window.rect.y0, span.1, window.rect.y1),
        panel,
        &profile.rules,
    );
    let bands: Vec<(i32, i32)> = lines
        .windows(2)
        .map(|pair| (pair[0].1 + 1, pair[1].0 - 1))
        .collect();
    let anchor_mid = centre_y(&anchor.rect);
    let header_index = bands
        .iter()
        .position(|(top, bottom)| anchor_mid >= *top && anchor_mid <= *bottom)
        .ok_or(SaleQueueError::TableNotFound)?;
    let header = bands[header_index];
    // The table's rules stop at its edges: the left one is the name
    // column's, which keeps the inventory list out of the table.
    let extent = rule_extent(
        image,
        lines[header_index + 1].0,
        (window.rect.x0, window.rect.x1),
        panel,
        &profile.rules,
    )
    .ok_or(SaleQueueError::TableNotFound)?;
    if headings.iter().any(|h| h.x0 < extent.0 || h.x1 > extent.1) {
        return Err(SaleQueueError::TableNotFound);
    }
    let reach = profile.rules.divider_reach;
    let dividers = column_rules(
        image,
        Rect::new(extent.0 + reach, header.0, extent.1 - reach, header.1),
        &profile.rules,
    );
    let mut edges = vec![extent.0 - 1];
    edges.extend(dividers.iter().copied());
    edges.push(extent.1 + 1);
    // Each column runs between the dividers either side of its heading,
    // which must lie wholly inside it, and no two headings may share one:
    // a divider hidden under a cursor or a dragged item would otherwise
    // let one column's cells take in the next one's spinner.
    let mut columns = Vec::with_capacity(headings.len());
    let mut owners = Vec::with_capacity(headings.len());
    for (heading, column) in headings.iter().zip(&queue.columns) {
        let index = edges
            .iter()
            .filter(|d| **d < heading.x0)
            .count()
            .checked_sub(1);
        let bounds = index.and_then(|i| Some((i, *edges.get(i)?, *edges.get(i + 1)?)));
        match bounds {
            Some((i, left, right)) if heading.x1 < right => {
                if owners.contains(&i) {
                    return Err(SaleQueueError::ColumnsMerged);
                }
                owners.push(i);
                columns.push((left + reach, right - reach));
            }
            _ => return Err(SaleQueueError::HeadingNotFound(column.heading.clone())),
        }
    }

    // The toggle over the table says how prices are stated: the spinners
    // hold PED only in the mode the reader is built on.
    let above: Vec<TextRun> = runs
        .iter()
        .filter(|r| r.rect.y1 < header.0 && r.rect.x0 >= extent.0 && r.rect.x1 <= extent.1)
        .cloned()
        .collect();
    let toggle = |label: &str| -> Result<Option<bool>, SaleQueueError> {
        let run = find_label(image, &above, label, &profile.read, recogniser)
            .map_err(no_recogniser)?
            .ok_or(SaleQueueError::ModeUnread)?;
        Ok(lit(image, run.rect, profile, queue))
    };
    match toggle(&queue.mode.active)? {
        Some(true) => {}
        Some(false) => return Err(SaleQueueError::OtherMode),
        None => return Err(SaleQueueError::ModeUnread),
    }
    for other in &queue.mode.others {
        match toggle(other)? {
            Some(false) => {}
            Some(true) => return Err(SaleQueueError::OtherMode),
            None => return Err(SaleQueueError::ModeUnread),
        }
    }

    // A band is the table's while its dividers cross it.
    let crosses = |top: i32, bottom: i32| -> bool {
        let mut checked = 0;
        for &d in &dividers {
            let strip = Rect::new(d - reach, top, d + reach, bottom);
            if covered(&located, strip) {
                continue;
            }
            checked += 1;
            if column_rules(image, strip, &profile.rules).is_empty() {
                return false;
            }
        }
        checked > 0
    };
    let below: Vec<Band> = bands[header_index + 1..]
        .iter()
        .map(|&(top, bottom)| Band {
            top,
            bottom,
            ruled: crosses(top, bottom),
        })
        .collect();
    let shape = table_shape(&below, queue.row_slack);
    // The header is sized from the same font as the rows, so it says how
    // tall a row is even when only one is in view to measure.
    let header_height = header.1 - header.0 + 1;
    let row_height = (header_height as f32 / queue.header_height_share).round() as i32;
    Ok(Table {
        runs,
        panel,
        header,
        extent,
        columns,
        above,
        rows: shape.rows,
        row_height,
        located,
    })
}

/// Read the sell queue from a frame.
pub fn read_sale_queue(
    image: &BgrImage,
    profile: &UiProfile,
    catalogue: &ItemCatalogue,
    recogniser: &dyn Recogniser,
) -> Result<SaleQueue, SaleQueueError> {
    let (queue, plan) = queue_profile(profile)?;
    // The title and the headings are recognised while searching and again
    // while reading.
    let remembering = Remembering::new(recogniser);
    let recogniser: &dyn Recogniser = &remembering;
    let table = find_table(image, profile, queue, recogniser)?;
    // The scale is decided from the table's geometry alone, before any
    // value is read, so no reading can choose the measures it is held to.
    let (_, scale) = claiming(&queue.scales, table.header_height(), |s| s.header_height)
        .map_err(SaleQueueError::Scale)?;
    let reader = table.reader(image, profile, queue, recogniser);
    let whole = table.whole_rows(queue);

    let mut rows = Vec::new();
    let mut unread = Vec::new();
    let partial = table.rows.len() - whole.len();
    for band in whole {
        match read_row(&reader, scale, &plan, &table.columns, band, catalogue)? {
            Ok(item) => rows.push(item),
            Err(row) => unread.push(row),
        }
    }
    confirm_by_list(&reader, scale, &mut rows, &mut unread, table.extent.0)?;

    // The heading over the table carries the count, its digits measured
    // as any other number.
    let mut counts = Vec::new();
    for run in &table.above {
        let text = reader.read(image, run.rect)?;
        if let Some(count) = heading_count(&text, &queue.heading, profile.read.label_score) {
            let bracketed = format!("({count})");
            let measured = count_ink(image, run.rect, profile.text.min_ink)
                .is_some_and(|ink| width_agrees(image, ink.padded(1), &bracketed, &scale.numerals));
            counts.push(measured.then_some(count));
        }
    }
    let seen = rows.len() + unread.len() + partial;
    let count = match counts.as_slice() {
        [Some(heading)] if (*heading as usize) < seen => QueueCount::Contradicted {
            heading: *heading,
            seen,
        },
        [Some(heading)] if *heading as usize == rows.len() && seen == rows.len() => {
            QueueCount::Complete(*heading)
        }
        [Some(heading)] => QueueCount::Incomplete {
            heading: *heading,
            read: rows.len(),
            unread: unread.len(),
            partial,
        },
        _ => QueueCount::Unread,
    };

    Ok(SaleQueue {
        rows: rows.into_iter().map(|(item, _)| item).collect(),
        unread,
        partial,
        count,
    })
}

/// The lettering of one field of a whole row, as the reader finds it,
/// for measuring.
#[derive(Debug, Clone)]
pub struct MeasuredInk {
    /// The image the ink is measured in: a spinner's value or a figure
    /// under one brightened into a crop of its own (as the reader reads
    /// and measures it), or `None` for the frame itself.
    pub crop: Option<BgrImage>,
    /// The lettering's run, in the coordinates of that image.
    pub rect: Rect,
    /// For a name, the widths in pixels of the blank stretches between
    /// its inked columns, left to right: a word gap must be wider than
    /// every gap between letters and no wider than every gap between
    /// words.
    pub gaps: Vec<i32>,
}

/// The queue as the reader finds it before reading a value, for
/// measuring the lettering of a scale the profile may not list yet.
#[derive(Debug, Clone)]
pub struct QueueMeasure {
    /// The header band's height in pixels, rule to rule: what tells the
    /// interface scale.
    pub header_height: i32,
    /// The profile's scale entry that claims it, or why none does.
    pub scale: Result<usize, ScaleError>,
    /// The scale entry whose figure lettering found the figures under the
    /// spinners: the one claiming the queue, or the nearest listed.
    pub lettering: usize,
    /// Every whole row in view, top to bottom, with each field's
    /// lettering or why it was not found. Switches carry no lettering and
    /// are left out.
    pub rows: Vec<BTreeMap<QueueField, Result<MeasuredInk, RowRefusal>>>,
    /// The bracketed count closing the queue's heading, `(3)`, as the
    /// reader measures it, in the frame's coordinates, and the count it
    /// read as.
    pub count: Option<(Rect, u32)>,
}

/// Find the queue and every field's lettering without holding anything
/// to a scale's measures: what an operator measures a new scale entry
/// from. Nothing here is a reading; a value is read only by
/// [`read_sale_queue`].
pub fn measure_sale_queue(
    image: &BgrImage,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<QueueMeasure, SaleQueueError> {
    let (queue, plan) = queue_profile(profile)?;
    let remembering = Remembering::new(recogniser);
    let recogniser: &dyn Recogniser = &remembering;
    let table = find_table(image, profile, queue, recogniser)?;
    let header_height = table.header_height();
    let scale = claiming(&queue.scales, header_height, |s| s.header_height).map(|(i, _)| i);
    // The figures under the spinners are found with the lettering of the
    // scale that claims the queue, or failing one, the nearest listed.
    let nearest = scale.unwrap_or_else(|_| nearest_scale(&queue.scales, header_height));
    let lettering = queue
        .scales
        .get(nearest)
        .ok_or_else(|| SaleQueueError::NotInProfile("no sale_queue scale".into()))?;
    let reader = table.reader(image, profile, queue, recogniser);
    let mut rows = Vec::new();
    for band in table.whole_rows(queue) {
        let mut fields = BTreeMap::new();
        for (&field, (column, pick)) in &plan {
            if pick.pick == Pick::BesideBox {
                continue;
            }
            let (left, right) = table.columns[*column];
            let cell = Rect::new(left, band.top, right, band.bottom);
            let found = match reader.ink(cell, pick.pick, field, lettering) {
                Ok(ink) => {
                    let gaps = if field == QueueField::Name {
                        let words = words(image, ink.rect, profile.text.min_ink, 1);
                        words.windows(2).map(|w| w[1].x0 - w[0].x1 - 1).collect()
                    } else {
                        Vec::new()
                    };
                    Ok(MeasuredInk {
                        crop: ink.crop,
                        rect: ink.rect,
                        gaps,
                    })
                }
                Err(Fault::Merged) => return Err(SaleQueueError::ColumnsMerged),
                Err(Fault::Row(refusal)) => Err(refusal),
            };
            fields.insert(field, found);
        }
        rows.push(fields);
    }
    let mut count = None;
    for run in &table.above {
        let text = reader.read(image, run.rect)?;
        if let Some(n) = heading_count(&text, &queue.heading, profile.read.label_score) {
            count = count_ink(image, run.rect, profile.text.min_ink).map(|ink| (ink.padded(1), n));
        }
    }
    Ok(QueueMeasure {
        header_height,
        scale,
        lettering: nearest,
        rows,
        count,
    })
}

/// The listed scale whose claimed header heights lie nearest `measured`.
pub fn nearest_scale(scales: &[SaleQueueScale], measured: i32) -> usize {
    (0..scales.len())
        .min_by_key(|i| {
            let [least, most] = scales[*i].header_height;
            (least - measured).abs().min((most - measured).abs())
        })
        .unwrap_or(0)
}

/// One row: every field read, then the checks between them. A row read
/// comes with where its name is lettered.
fn read_row(
    reader: &Reader,
    scale: &SaleQueueScale,
    plan: &BTreeMap<QueueField, (usize, FieldPick)>,
    columns: &[(i32, i32)],
    band: &Band,
    catalogue: &ItemCatalogue,
) -> Result<Result<(QueuedItem, Rect), UnreadQueueRow>, SaleQueueError> {
    let mut values: BTreeMap<QueueField, Value> = BTreeMap::new();
    let mut refusals = Vec::new();
    let mut raw_name = None;
    let mut name_rect = None;
    for (&field, (column, pick)) in plan {
        let (left, right) = columns[*column];
        let cell = Rect::new(left, band.top, right, band.bottom);
        // Helpers that only an editable value has.
        let editable = |of: QueueField| matches!(values.get(&of), Some(Value::Spinner(v)) if v.setting == Setting::Editable);
        if (field == QueueField::QuantityStack && !editable(QueueField::Quantity))
            || (field == QueueField::BuyoutFigure
                && !matches!(values.get(&QueueField::Buyout), Some(Value::Spinner(_))))
        {
            continue;
        }
        let ink = match reader.ink(cell, pick.pick, field, scale) {
            Ok(ink) => ink,
            Err(Fault::Merged) => return Err(SaleQueueError::ColumnsMerged),
            Err(Fault::Row(refusal)) => {
                refusals.push(refusal);
                continue;
            }
        };
        // The buyout is a price only while both its box and its tag say
        // it is set; a switched-off buyout's grey number is no price, and
        // is not read.
        if field == QueueField::Buyout {
            let on = match values.get(&QueueField::BuyoutSwitch) {
                Some(Value::Switch(lit)) => *lit,
                _ => continue,
            };
            let set = ink.setting == Some(Setting::Editable);
            if on != set {
                refusals.push(RowRefusal::Disagrees(Identity::BuyoutSwitch));
                continue;
            }
            if !on {
                values.insert(field, Value::Off);
                continue;
            }
        }
        let (raw, value) = reader.value(field, &ink, scale)?;
        if field == QueueField::Name {
            raw_name = Some(raw);
            name_rect = Some(ink.rect);
        }
        match value {
            Ok(value) => {
                values.insert(field, value);
            }
            Err(refusal) => refusals.push(refusal),
        }
    }
    if !refusals.is_empty() {
        return Ok(Err(UnreadQueueRow {
            raw_name,
            refusals,
            fee_disagreement: None,
        }));
    }
    let mut take = |field| values.remove(&field);
    let name = match take(QueueField::Name) {
        Some(Value::Text(text)) => text,
        _ => unreachable!("a row whose fields read has its name"),
    };
    let mut amount = |field| match take(field) {
        Some(Value::Amount(amount)) => amount,
        _ => unreachable!("{field:?} is read as an amount"),
    };
    let (tt, fee) = (amount(QueueField::Tt), amount(QueueField::Fee));
    let mut spinner = |field| match take(field) {
        Some(Value::Spinner(value)) => value,
        _ => unreachable!("{field:?} is read as a spinner"),
    };
    let (quantity, opening_bid, days) = (
        spinner(QueueField::Quantity),
        spinner(QueueField::OpeningBid),
        spinner(QueueField::Days),
    );
    let buyout = match take(QueueField::Buyout) {
        Some(Value::Spinner(value)) => Buyout::Price(Price {
            text: value.text,
            value: value.value,
        }),
        _ => Buyout::Off,
    };
    let stack = match take(QueueField::QuantityStack) {
        Some(Value::Bound(n)) => Some(n),
        _ => None,
    };
    let days_allowed = match take(QueueField::DaysRange) {
        Some(Value::Range(range)) => range,
        _ => unreachable!("a row whose fields read has its days' range"),
    };
    let mut figure = |field| match take(field) {
        Some(Value::Markup(markup)) => Some(markup),
        _ => None,
    };
    let headline = figure(QueueField::Markup).expect("the headline is read");
    let bid_figure = figure(QueueField::OpeningBidFigure).expect("the bid's figure is read");
    let buyout_figure = figure(QueueField::BuyoutFigure);

    let mut broken = Vec::new();
    // Bounds the window itself prints.
    let out_of = |field, text: &str| RowRefusal::OutOfBounds {
        field,
        raw: text.to_string(),
    };
    if quantity.value < 1 || stack.is_some_and(|n| quantity.value > n as u64) {
        broken.push(out_of(QueueField::Quantity, &quantity.text));
    }
    if days.value < days_allowed.0 as u64 || days.value > days_allowed.1 as u64 {
        broken.push(out_of(QueueField::Days, &days.text));
    }
    // The catalogue's most TT value for one item bounds the row's.
    let catalogued = catalogue.find(&name);
    if let Some(Some(max_tt)) = catalogued {
        if tt.ped > max_tt * quantity.value as f64 + EPSILON {
            broken.push(out_of(QueueField::Tt, &tt.text));
        }
    }
    if !figure_agrees(&bid_figure, opening_bid.value as f64, &tt) {
        broken.push(RowRefusal::Disagrees(Identity::OpeningBidFigure));
    }
    if let (Some(buyout_figure), Buyout::Price(price)) = (&buyout_figure, &buyout) {
        if !figure_agrees(buyout_figure, price.value as f64, &tt) {
            broken.push(RowRefusal::Disagrees(Identity::BuyoutFigure));
        }
    }
    // The headline states the markup of the price a buyer pays outright
    // when there is one, and of the opening bid otherwise.
    let setting = buyout_figure.as_ref().unwrap_or(&bid_figure);
    if headline.text != setting.text {
        broken.push(RowRefusal::Disagrees(Identity::Headline));
    }
    if !fee_agrees(&fee, opening_bid.value as f64, &tt) {
        broken.push(RowRefusal::Disagrees(Identity::Fee));
    }
    if !broken.is_empty() {
        let fee_disagreement =
            (broken == [RowRefusal::Disagrees(Identity::Fee)]).then(|| FeeDisagreement {
                name: name.clone(),
                tt: tt.text.clone(),
                quantity: quantity.text.clone(),
                opening_bid: opening_bid.text.clone(),
                buyout: match &buyout {
                    Buyout::Off => None,
                    Buyout::Price(price) => Some(price.text.clone()),
                },
                days: days.text.clone(),
                fee: fee.text.clone(),
            });
        return Ok(Err(UnreadQueueRow {
            raw_name,
            refusals: broken,
            fee_disagreement,
        }));
    }
    let (least_fee, most_fee) = fee_window(opening_bid.value as f64, &tt);
    let item = QueuedItem {
        name: if catalogued.is_some() {
            ItemName::Catalogued(name)
        } else {
            ItemName::Unconfirmed(name)
        },
        tt,
        quantity,
        stack,
        opening_bid,
        buyout,
        days,
        days_allowed,
        fee,
        fee_fixed: least_fee == most_fee,
        markup: headline,
    };
    // Only the name's width over its height is used, which is the same
    // wherever its run was measured.
    let name_rect = name_rect.expect("a row whose name read has its run");
    Ok(Ok((item, name_rect)))
}

/// Look for each name the catalogue does not confirm in the inventory
/// list beside the queue: a name the list shows exactly is confirmed by
/// it. Only list lettering about as long as the name (its width over its
/// height near the name's own) is recognised. Where the list still holds
/// some of a stack, the count it shows and the quantity queued make the
/// stack size; a row where they do not is unread.
fn confirm_by_list(
    reader: &Reader,
    scale: &SaleQueueScale,
    rows: &mut Vec<(QueuedItem, Rect)>,
    unread: &mut Vec<UnreadQueueRow>,
    table_left: i32,
) -> Result<(), SaleQueueError> {
    let listed: Vec<TextRun> = reader
        .runs
        .iter()
        .filter(|r| r.rect.x1 < table_left)
        .cloned()
        .collect();
    let aspect = |r: &Rect| r.width() as f32 / r.height() as f32;
    let mut kept = Vec::with_capacity(rows.len());
    for (mut row, name_rect) in rows.drain(..) {
        let name = row.name.text().to_string();
        let name_aspect = aspect(&name_rect);
        let mut found = None;
        for run in &listed {
            if (aspect(&run.rect) / name_aspect - 1.0).abs() > reader.queue.name_aspect_slack {
                continue;
            }
            let text = reader.read(reader.image, run.rect)?;
            if normalised_name(&text) == name {
                found = Some(run.clone());
                break;
            }
        }
        let Some(run) = found else {
            kept.push((row, name_rect));
            continue;
        };
        if let ItemName::Unconfirmed(_) = row.name {
            row.name = ItemName::Listed(name.clone());
        }
        // The count beside the name, where it reads exactly.
        let count = match (run_right_of(&run, &listed), row.stack) {
            (Some(count_run), Some(stack)) if row.quantity.setting == Setting::Editable => {
                let raw = reader.read(reader.image, count_run.rect)?;
                parse(Grammar::Integer, &raw)
                    .filter(|p| {
                        width_agrees(reader.image, count_run.rect, &p.text, &scale.numerals)
                    })
                    .and_then(|p| p.text.parse::<u64>().ok())
                    .map(|count| (count, stack))
            }
            _ => None,
        };
        match count {
            Some((count, stack)) if count + row.quantity.value != stack as u64 => {
                unread.push(UnreadQueueRow {
                    raw_name: Some(name),
                    refusals: vec![RowRefusal::Disagrees(Identity::StackCount)],
                    fee_disagreement: None,
                });
            }
            _ => kept.push((row, name_rect)),
        }
    }
    *rows = kept;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{
        bundled_profile, draw_window, sale_scene, sale_window, standard_sale_layout, Lettering,
        SaleLayout, SALE_HEADER_HEIGHT,
    };

    fn catalogue() -> ItemCatalogue {
        ItemCatalogue::new([("Electric Attack Nanochip 3".to_string(), Some(39.0))])
    }

    fn read_with(
        layout: &SaleLayout,
        change: impl Fn(String) -> String + Send + Sync,
    ) -> Result<SaleQueue, SaleQueueError> {
        let d = sale_window(layout);
        let inner = d.recogniser();
        let recogniser = move |crop: &BgrImage| inner(crop).map(|(text, c)| (change(text), c));
        read_sale_queue(&d.image, &bundled_profile(), &catalogue(), &recogniser)
    }

    fn read(layout: &SaleLayout) -> Result<SaleQueue, SaleQueueError> {
        read_with(layout, |text| text)
    }

    fn buyout_text(buyout: &Buyout) -> String {
        match buyout {
            Buyout::Off => "off".into(),
            Buyout::Price(price) => price.text.clone(),
        }
    }

    fn texts(item: &QueuedItem) -> Vec<String> {
        vec![
            item.name.text().to_string(),
            item.tt.text.clone(),
            item.quantity.text.clone(),
            item.opening_bid.text.clone(),
            buyout_text(&item.buyout),
            item.days.text.clone(),
            item.fee.text.clone(),
            item.markup.text.clone(),
        ]
    }

    use Setting::{Editable, Locked};

    #[test]
    fn a_whole_queue_reads_every_row_with_its_settings() {
        let queue = read(&standard_sale_layout()).expect("the queue reads");
        assert!(queue.unread.is_empty(), "{:?}", queue.unread);
        assert_eq!(queue.partial, 0);
        assert_eq!(queue.count, QueueCount::Complete(3));
        let rows: Vec<Vec<String>> = queue.rows.iter().map(texts).collect();
        let want = [
            [
                "Electric Attack Nanochip 3",
                "11.52 PED",
                "1",
                "162",
                "172",
                "7",
                "7.33 PED",
                "TT+160.47 PED",
            ],
            [
                "Output Amplifier Component",
                "20.80 PED",
                "52",
                "42",
                "off",
                "7",
                "1.54 PED",
                "201.92%",
            ],
            [
                "Skyripper Loyalist (L)",
                "24.54 PED",
                "1",
                "30",
                "off",
                "7",
                "0.77 PED",
                "122.25%",
            ],
        ];
        assert_eq!(
            rows,
            want.map(|row| row.map(String::from).to_vec()).to_vec()
        );
        let first = &queue.rows[0];
        assert_eq!(
            (first.tt.ped, first.opening_bid.value, first.fee.ped),
            (11.52, 162, 7.33)
        );
        assert_eq!(
            (first.markup.kind, first.markup.figure),
            (MarkupKind::AboveTt, 160.47)
        );
        assert_eq!(queue.rows[1].markup.kind, MarkupKind::Percent);
        assert_eq!(
            queue
                .rows
                .iter()
                .map(|r| (r.quantity.setting, r.opening_bid.setting, r.days.setting))
                .collect::<Vec<_>>(),
            vec![
                (Locked, Editable, Editable),
                (Editable, Editable, Editable),
                (Locked, Editable, Editable),
            ]
        );
        assert_eq!(
            queue
                .rows
                .iter()
                .map(|r| (r.stack, r.days_allowed))
                .collect::<Vec<_>>(),
            vec![(None, (1, 7)), (Some(52), (1, 7)), (None, (1, 7))]
        );
        let names: Vec<&ItemName> = queue.rows.iter().map(|row| &row.name).collect();
        assert_eq!(
            names,
            vec![
                &ItemName::Catalogued("Electric Attack Nanochip 3".into()),
                &ItemName::Unconfirmed("Output Amplifier Component".into()),
                &ItemName::Unconfirmed("Skyripper Loyalist (L)".into()),
            ]
        );
    }

    #[test]
    fn each_field_says_what_it_was_checked_by() {
        let queue = read(&standard_sale_layout()).expect("the queue reads");
        let (single, stack) = (&queue.rows[0], &queue.rows[1]);
        use Assurance::*;
        for (field, want) in [
            (QueueField::OpeningBid, CrossChecked),
            (QueueField::Buyout, CrossChecked),
            (QueueField::Tt, WithinWindow),
            (QueueField::Fee, WithinWindow),
            (QueueField::Markup, WithinWindow),
            (QueueField::Days, Bounded),
            (QueueField::Quantity, ReadOnly),
            (QueueField::Name, ReadOnly),
        ] {
            assert_eq!(single.assurance(field), want, "{field:?}");
        }
        assert_eq!(stack.assurance(QueueField::Quantity), Bounded);
    }

    #[test]
    fn a_name_the_inventory_list_shows_is_listed_and_its_count_makes_the_stack() {
        let mut layout = standard_sale_layout();
        layout.rows[1].quantity = ("40", false);
        layout.inventory = vec![
            ("Animal Hide", "2"),
            ("Output Amplifier Component", "12"),
            ("Skyripper Loyalist (M)", "1"),
        ];
        let queue = read(&layout).expect("the queue reads");
        let names: Vec<&ItemName> = queue.rows.iter().map(|row| &row.name).collect();
        assert_eq!(
            names,
            vec![
                &ItemName::Catalogued("Electric Attack Nanochip 3".into()),
                &ItemName::Listed("Output Amplifier Component".into()),
                &ItemName::Unconfirmed("Skyripper Loyalist (L)".into()),
            ]
        );
        // Forty queued and thirteen left do not make a stack of 52.
        layout.inventory[1].1 = "13";
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(queue.rows.len(), 2);
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::Disagrees(Identity::StackCount)]
        );
    }

    #[test]
    fn a_catalogued_items_tt_value_is_bounded_by_its_most() {
        let d = sale_window(&standard_sale_layout());
        let tight = ItemCatalogue::new([("Electric Attack Nanochip 3".to_string(), Some(10.0))]);
        let queue = read_sale_queue(&d.image, &bundled_profile(), &tight, &d.recogniser())
            .expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::OutOfBounds {
                field: QueueField::Tt,
                raw: "11.52 PED".into()
            }]
        );
    }

    /// The row's refusals when the recogniser reads `drawn` as `reads`,
    /// every other row still reading.
    fn refused_when(drawn: &'static str, reads: &'static str) -> Vec<(String, Vec<RowRefusal>)> {
        let queue = read_with(&standard_sale_layout(), move |text| {
            if text == drawn {
                reads.to_string()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(queue.rows.len() + queue.unread.len(), 3, "{queue:?}");
        queue
            .unread
            .into_iter()
            .map(|row| (row.raw_name.unwrap_or_default(), row.refusals))
            .collect()
    }

    #[test]
    fn short_long_and_wrong_readings_are_refused() {
        let nanochip = "Electric Attack Nanochip 3".to_string();
        let amplifier = "Output Amplifier Component".to_string();
        let width = |field, raw: &str| RowRefusal::WrongWidth {
            field,
            raw: raw.into(),
        };
        let bounds = |field, raw: &str| RowRefusal::OutOfBounds {
            field,
            raw: raw.into(),
        };
        let disagrees = |identity| vec![RowRefusal::Disagrees(identity)];
        let cases: Vec<(&str, &str, String, Vec<RowRefusal>)> = vec![
            // A digit short or one too many: the ink is not that wide.
            (
                "162",
                "16",
                nanochip.clone(),
                vec![width(QueueField::OpeningBid, "16")],
            ),
            (
                "7.33 PED",
                "7.333 PED",
                nanochip.clone(),
                vec![width(QueueField::Fee, "7.333 PED")],
            ),
            (
                "52",
                "5",
                amplifier.clone(),
                vec![width(QueueField::Quantity, "5")],
            ),
            (
                "TT+150.47 PED",
                "TT+15.47 PED",
                nanochip.clone(),
                vec![width(QueueField::OpeningBidFigure, "TT+15.47 PED")],
            ),
            // A wrong digit of the same width: the game's own figures
            // disagree with it, or it leaves the window's bounds.
            (
                "172",
                "173",
                nanochip.clone(),
                disagrees(Identity::BuyoutFigure),
            ),
            (
                "42",
                "43",
                amplifier.clone(),
                vec![
                    RowRefusal::Disagrees(Identity::OpeningBidFigure),
                    RowRefusal::Disagrees(Identity::Fee),
                ],
            ),
            (
                "20.80 PED",
                "26.80 PED",
                amplifier.clone(),
                vec![
                    RowRefusal::Disagrees(Identity::OpeningBidFigure),
                    RowRefusal::Disagrees(Identity::Fee),
                ],
            ),
            (
                "TT+160.47 PED",
                "TT+160.41 PED",
                nanochip.clone(),
                disagrees(Identity::BuyoutFigure),
            ),
            (
                "7.33 PED",
                "7.38 PED",
                nanochip.clone(),
                disagrees(Identity::Fee),
            ),
            (
                "1.54 PED",
                "1.55 PED",
                amplifier.clone(),
                disagrees(Identity::Fee),
            ),
            (
                "52",
                "53",
                amplifier.clone(),
                vec![bounds(QueueField::Quantity, "53")],
            ),
            // Text that is no value at all.
            (
                "201.92%",
                "201.92",
                amplifier.clone(),
                vec![
                    RowRefusal::Invalid {
                        field: QueueField::OpeningBidFigure,
                        raw: "201.92".into(),
                    },
                    RowRefusal::Invalid {
                        field: QueueField::Markup,
                        raw: "201.92".into(),
                    },
                ],
            ),
            // A word read with a space inside it.
            (
                "Amplifier",
                "Ampli fier",
                "Output Ampli fier Component".to_string(),
                vec![RowRefusal::WordCount {
                    field: QueueField::Name,
                    raw: "Output Ampli fier Component".into(),
                }],
            ),
        ];
        for (drawn, reads, row, want) in cases {
            assert_eq!(
                refused_when(drawn, reads),
                vec![(row, want)],
                "{drawn} read as {reads}"
            );
        }
        // Days are bounded by the range under them: a day past it is
        // refused, and a day inside it is the limit of what is caught.
        let queue = read_with(&standard_sale_layout(), |text| {
            if text == "7" {
                "8".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert!(queue.rows.is_empty());
        assert!(queue.unread.iter().all(|row| row.refusals
            == vec![RowRefusal::OutOfBounds {
                field: QueueField::Days,
                raw: "8".into()
            }]));
        let limit = read_with(&standard_sale_layout(), |text| {
            if text == "7" {
                "1".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(
            limit.rows.len(),
            3,
            "a same-width day inside the range passes"
        );
        assert!(limit
            .rows
            .iter()
            .all(|row| row.assurance(QueueField::Days) == Assurance::Bounded));
    }

    #[test]
    fn a_name_the_recogniser_runs_together_reads_by_its_words() {
        // Read whole, the name loses a space; read word by word, as the
        // ink parts it, it does not.
        let queue = read_with(&standard_sale_layout(), |text| {
            if text == "Output Amplifier Component" {
                "OutputAmplifier Component".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(queue.rows[1].name.text(), "Output Amplifier Component");
    }

    #[test]
    fn a_name_cut_to_fit_is_refused() {
        let mut layout = standard_sale_layout();
        // A name reaching the column's edge, as the client sets one it
        // has cut to fit.
        layout.rows[0].name = "Electric Attack Nanochip Mk. III";
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::Cut(QueueField::Name)]
        );
        let queue = read_with(&standard_sale_layout(), |text| {
            // The name's end read with the client's ellipsis.
            if text == "3" {
                "3...".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::Cut(QueueField::Name)]
        );
    }

    #[test]
    fn a_switched_off_buyout_carries_no_price_and_its_witnesses_must_agree() {
        let queue = read(&standard_sale_layout()).expect("the queue reads");
        assert_eq!(queue.rows[2].buyout, Buyout::Off);
        assert_eq!(
            queue.rows[0].buyout,
            Buyout::Price(Price {
                text: "172".into(),
                value: 172
            })
        );
        // The tag lit over a greyed box, and a bright box with its tag
        // unlit: the two witnesses disagree, and the row is refused.
        for (lit, locked) in [(true, true), (false, false)] {
            let mut layout = standard_sale_layout();
            layout.rows[0].buyout_lit = lit;
            layout.rows[0].buyout.1 = locked;
            let queue = read(&layout).expect("the queue reads");
            assert_eq!(
                queue.unread[0].refusals,
                vec![RowRefusal::Disagrees(Identity::BuyoutSwitch)],
                "tag lit {lit}, box greyed {locked}"
            );
        }
    }

    #[test]
    fn a_headline_that_is_not_the_setting_figure_is_refused() {
        let mut layout = standard_sale_layout();
        // The headline repeats the bid's figure although the buyout is
        // set and states its own.
        layout.rows[0].markup = "TT+150.47 PED";
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::Disagrees(Identity::Headline)]
        );
        assert_eq!(queue.rows.len(), 2);
        assert_eq!(
            queue.count,
            QueueCount::Incomplete {
                heading: 3,
                read: 2,
                unread: 1,
                partial: 0
            }
        );
    }

    #[test]
    fn a_spinner_neither_bright_nor_greyed_is_refused_and_one_too_dim_is_empty() {
        let mut layout = standard_sale_layout();
        layout.locked_peak = 130;
        let queue = read(&layout).expect("the queue reads");
        assert!(queue.rows.is_empty());
        let refusals: Vec<Vec<RowRefusal>> = queue
            .unread
            .iter()
            .map(|row| row.refusals.clone())
            .collect();
        assert_eq!(
            refusals,
            vec![
                vec![RowRefusal::Shading(QueueField::Quantity)],
                vec![RowRefusal::Shading(QueueField::Buyout)],
                vec![
                    RowRefusal::Shading(QueueField::Quantity),
                    RowRefusal::Shading(QueueField::Buyout)
                ],
            ]
        );
        layout.locked_peak = 30;
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::NotFound(QueueField::Quantity)]
        );
    }

    #[test]
    fn a_number_reaching_its_boxs_sides_is_refused() {
        use crate::game_ui::test_support::{sale_column_edges, sale_row_top};
        let layout = standard_sale_layout();
        let mut d = sale_window(&layout);
        // The second row's quantity drawn from the side of its box, as a
        // number too long for it is.
        let edges = sale_column_edges(layout.window);
        let cx = (edges[1] + edges[2]) / 2;
        let top = sale_row_top(layout.window, 1);
        d.text_at(Rect::new(cx - 31, top + 44, cx + 10, top + 53), "52", 232);
        let queue = read_sale_queue(&d.image, &bundled_profile(), &catalogue(), &d.recogniser())
            .expect("the queue reads");
        assert_eq!(
            queue.unread[0].refusals,
            vec![RowRefusal::Clipped(QueueField::Quantity)]
        );
    }

    #[test]
    fn a_hidden_divider_refuses_the_frame() {
        // Between the quantity and the opening bid, and between the
        // opening bid and the buyout: either would leave one cell over
        // two spinners.
        for hidden in [2, 3] {
            let mut layout = standard_sale_layout();
            layout.hidden_dividers = vec![hidden];
            assert_eq!(
                read(&layout),
                Err(SaleQueueError::ColumnsMerged),
                "divider {hidden}"
            );
        }
    }

    #[test]
    fn the_value_box_is_alone_in_its_cell() {
        let value = Rect::new(10, 34, 70, 63);
        let up = Rect::new(10, 16, 70, 29);
        let down = Rect::new(10, 65, 70, 77);
        let counter = Rect::new(40, 45, 42, 51);
        assert_eq!(value_box(&[up, value, down, counter], 0.75), Ok(value));
        assert_eq!(value_box(&[], 0.75), Err(BoxFault::None));
        assert_eq!(
            value_box(&[up, value, Rect::new(90, 34, 150, 63)], 0.75),
            Err(BoxFault::Tie)
        );
        // A neighbour's greyed box is a pixel shorter: still a second
        // spinner, not an arrow's box.
        assert_eq!(
            value_box(&[value, down, Rect::new(90, 35, 150, 63)], 0.75),
            Err(BoxFault::Two)
        );
    }

    #[test]
    fn the_queue_is_refused_in_the_other_price_mode() {
        let mut layout = standard_sale_layout();
        layout.markup_mode = true;
        assert_eq!(read(&layout), Err(SaleQueueError::OtherMode));
    }

    #[test]
    fn the_count_claims_completeness_only_when_every_row_is_read() {
        let mut layout = standard_sale_layout();
        layout.cut_last = Some(30);
        let queue = read(&layout).expect("the queue reads");
        assert_eq!((queue.rows.len(), queue.partial), (2, 1));
        assert_eq!(
            queue.count,
            QueueCount::Incomplete {
                heading: 3,
                read: 2,
                unread: 0,
                partial: 1
            }
        );
        // A heading that counts fewer rows than are seen is misread.
        let mut layout = standard_sale_layout();
        layout.heading = ("Sell Queue", "2");
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(
            queue.count,
            QueueCount::Contradicted {
                heading: 2,
                seen: 3
            }
        );
        // A count read with a digit more than its ink holds is unread.
        let queue = read_with(&standard_sale_layout(), |text| {
            if text == "Sell Queue (3)" {
                "Sell Queue (13)".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(queue.count, QueueCount::Unread);
    }

    #[test]
    fn a_row_cut_by_the_view_is_partial_even_when_it_is_alone() {
        let mut layout = standard_sale_layout();
        // With one row, there is no other to measure it by: the header
        // says how tall it should be.
        layout.rows.truncate(1);
        layout.heading = ("Sell Queue", "1");
        layout.cut_last = Some(30);
        let queue = read(&layout).expect("the queue reads");
        assert_eq!((queue.rows.len(), queue.partial), (0, 1));
    }

    #[test]
    fn a_row_under_a_window_in_front_is_partial() {
        let layout = standard_sale_layout();
        let mut d = sale_window(&layout);
        // A window over the second row's markup, its rules left in view.
        draw_window(&mut d.image, Rect::new(1120, 284, 1330, 384));
        let queue = read_sale_queue(&d.image, &bundled_profile(), &catalogue(), &d.recogniser())
            .expect("reads");
        assert_eq!(queue.partial, 1, "{queue:?}");
        let names: Vec<&str> = queue.rows.iter().map(|row| row.name.text()).collect();
        assert_eq!(
            names,
            vec!["Electric Attack Nanochip 3", "Skyripper Loyalist (L)"]
        );
    }

    #[test]
    fn an_empty_queue_reads_complete() {
        let mut layout = standard_sale_layout();
        layout.rows.clear();
        layout.heading = ("Sell Queue", "0");
        let queue = read(&layout).expect("the queue reads");
        assert_eq!(queue.count, QueueCount::Complete(0));
        assert!(queue.rows.is_empty() && queue.unread.is_empty());
    }

    #[test]
    fn frames_that_cannot_be_read_are_refused_with_why() {
        let mut layout = standard_sale_layout();
        layout.window = Rect::new(120, 20, 1480, 800);
        assert_eq!(read(&layout), Err(SaleQueueError::WindowClipped));
        let d = sale_window(&standard_sale_layout());
        let none = |_: &BgrImage| None;
        assert_eq!(
            read_sale_queue(&d.image, &bundled_profile(), &catalogue(), &none),
            Err(SaleQueueError::NoRecogniser)
        );
        let mut other = Lettering::new(sale_scene());
        draw_window(&mut other.image, Rect::new(20, 20, 900, 600));
        other.text(Rect::new(100, 42, 300, 53), "INVENTORY");
        assert_eq!(
            read_sale_queue(
                &other.image,
                &bundled_profile(),
                &catalogue(),
                &other.recogniser()
            ),
            Err(SaleQueueError::WindowNotFound)
        );
        let blank_heading = read_with(&standard_sale_layout(), |text| {
            if text == "OPENING BID" {
                "OPENNG".into()
            } else {
                text
            }
        });
        assert_eq!(
            blank_heading,
            Err(SaleQueueError::HeadingNotFound("OPENING BID".into()))
        );
        let no_toggle = read_with(&standard_sale_layout(), |text| {
            if text == "PED" {
                String::new()
            } else {
                text
            }
        });
        assert_eq!(no_toggle, Err(SaleQueueError::ModeUnread));
    }

    #[test]
    fn the_heading_count_reads_only_after_its_heading() {
        let count = |text: &str| heading_count(text, "Sell Queue", 80.0);
        assert_eq!(count("Sell Queue (3)"), Some(3));
        assert_eq!(count("E Sell Queue (12)"), Some(12));
        assert_eq!(count("Sell Queue（0）"), Some(0));
        for text in [
            "Sell Queue (03)",
            "Sell Queue (3",
            "Sell Queue 3",
            "Sell Queue (3) x",
            "Sell Queue (a)",
            "Buy Queue (3)",
            "(3)",
            "",
        ] {
            assert_eq!(count(text), None, "{text:?}");
        }
    }

    fn amount(text: &str) -> Amount {
        let parsed = parse(Grammar::PedAmount, text).expect("an amount");
        Amount {
            ped: parsed.number.expect("a number"),
            text: parsed.text,
        }
    }

    fn figure(text: &str) -> MarkupText {
        markup(text).expect("a markup")
    }

    #[test]
    fn figures_agree_with_the_price_and_tt_value_to_their_last_place() {
        let tt = amount("11.52 PED");
        // The price less the TT value, less at most one unit.
        assert!(figure_agrees(&figure("TT+150.48 PED"), 162.0, &tt));
        assert!(figure_agrees(&figure("TT+150.47 PED"), 162.0, &tt));
        assert!(!figure_agrees(&figure("TT+150.46 PED"), 162.0, &tt));
        assert!(!figure_agrees(&figure("TT+150.49 PED"), 162.0, &tt));
        assert!(!figure_agrees(&figure("TT+150.47 PED"), 163.0, &tt));
        // The limit of the check: a TT value read one cent high agrees
        // as well as the true one, because the screen itself cuts the
        // true value; one cent low does not.
        assert!(figure_agrees(
            &figure("TT+150.47 PED"),
            162.0,
            &amount("11.53 PED")
        ));
        assert!(!figure_agrees(
            &figure("TT+150.47 PED"),
            162.0,
            &amount("11.51 PED")
        ));
        // A percentage, for any TT value the printed one could stand for.
        let tt = amount("24.54 PED");
        assert!(figure_agrees(&figure("122.25%"), 30.0, &tt));
        // Again the limit: a neighbouring percentage inside the window.
        assert!(figure_agrees(&figure("122.24%"), 30.0, &tt));
        assert!(!figure_agrees(&figure("122.30%"), 30.0, &tt));
        assert!(!figure_agrees(&figure("122.18%"), 30.0, &tt));
        assert!(!figure_agrees(&figure("122.25%"), 31.0, &tt));
        assert!(figure_agrees(&figure("126.32%"), 31.0, &tt));
        assert!(figure_agrees(
            &figure("100.00%"),
            72.0,
            &amount("72.00 PED")
        ));
        assert!(!figure_agrees(
            &figure("10.00%"),
            72.0,
            &amount("72.00 PED")
        ));
        // A TT value too small to bound the percentage from above.
        assert!(figure_agrees(&figure("500.00%"), 0.05, &amount("0.01 PED")));
        assert!(!figure_agrees(
            &figure("100.00%"),
            0.05,
            &amount("0.01 PED")
        ));
    }

    #[test]
    fn fees_agree_with_the_auctions_fee_for_the_bids_markup() {
        // Every fee the captures show, with its bid and TT value.
        for (fee, bid, tt) in [
            ("7.74 PED", 172.0, "11.52 PED"),
            ("7.33 PED", 162.0, "11.52 PED"),
            ("1.54 PED", 42.0, "20.80 PED"),
            ("0.77 PED", 30.0, "24.54 PED"),
            ("0.50 PED", 72.0, "72.00 PED"),
            ("0.50 PED", 28.0, "28.00 PED"),
        ] {
            assert!(fee_agrees(&amount(fee), bid, &amount(tt)), "{fee} at {bid}");
            for off in [-0.01, 0.01] {
                let wrong = Amount {
                    ped: amount(fee).ped + off,
                    text: format!("{:.2} PED", amount(fee).ped + off),
                };
                assert!(
                    !fee_agrees(&wrong, bid, &amount(tt)),
                    "{} at {bid}",
                    wrong.text
                );
            }
        }
        // A bid at the TT value pays exactly the base fee.
        assert!(fee_agrees(&amount("0.50 PED"), 72.0, &amount("72.00 PED")));
        assert!(!fee_agrees(&amount("0.51 PED"), 72.0, &amount("72.00 PED")));
        assert!(!fee_agrees(&amount("0.49 PED"), 72.0, &amount("72.00 PED")));
    }

    #[test]
    fn a_fee_at_a_boundary_of_its_last_place_is_only_bounded() {
        // A bid of 20 over a printed TT value of 1.98 could be over a true
        // one anywhere from 1.98 to just under 1.99: the fee model charges
        // 1.38 for some of those and 1.39 for others, and both agree.
        let tt = amount("1.98 PED");
        assert_eq!(fee_window(20.0, &tt), (138, 139));
        assert!(fee_agrees(&amount("1.38 PED"), 20.0, &tt));
        assert!(fee_agrees(&amount("1.39 PED"), 20.0, &tt));
        assert!(!fee_agrees(&amount("1.40 PED"), 20.0, &tt));
        // Away from a boundary the window holds one fee.
        assert_eq!(fee_window(162.0, &amount("11.52 PED")), (733, 733));

        let queue = read(&standard_sale_layout()).expect("the queue reads");
        assert!(queue.rows.iter().all(|row| row.fee_fixed));
        let mut row = queue.rows[0].clone();
        assert_eq!(row.assurance(QueueField::Fee), Assurance::WithinWindow);
        row.fee_fixed = false;
        assert_eq!(row.assurance(QueueField::Fee), Assurance::Bounded);
    }

    #[test]
    fn a_row_refused_only_for_its_fee_keeps_its_printed_figures_aside() {
        let queue = read_with(&standard_sale_layout(), |text| {
            if text == "7.33 PED" {
                "7.38 PED".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(queue.rows.len(), 2);
        assert_eq!(
            queue.unread[0].fee_disagreement,
            Some(FeeDisagreement {
                name: "Electric Attack Nanochip 3".into(),
                tt: "11.52 PED".into(),
                quantity: "1".into(),
                opening_bid: "162".into(),
                buyout: Some("172".into()),
                days: "7".into(),
                fee: "7.38 PED".into(),
            })
        );
        // A row refused for anything besides its fee keeps nothing aside.
        let queue = read_with(&standard_sale_layout(), |text| {
            if text == "42" {
                "43".into()
            } else {
                text
            }
        })
        .expect("the queue reads");
        assert_eq!(queue.unread[0].fee_disagreement, None);
    }

    #[test]
    fn names_are_compared_after_folding_and_spacing_only() {
        assert_eq!(
            normalised_name("  Eyeshadow  （Gold） "),
            "Eyeshadow (Gold)"
        );
        assert_eq!(
            normalised_name("Skyripper\u{3000}Loyalist"),
            "Skyripper Loyalist"
        );
        assert_ne!(normalised_name("Eyeshadow Gold)"), "Eyeshadow (Gold)");
        assert_ne!(normalised_name("eyeshadow (gold)"), "Eyeshadow (Gold)");
        let catalogue = ItemCatalogue::from_entities(&[
            serde_json::json!({"name": "Electric Attack Nanochip 3", "economy": {"max_tt": 39}}),
            serde_json::json!({"name": "Opalo"}),
        ]);
        assert_eq!(
            catalogue.find("Electric Attack Nanochip 3"),
            Some(Some(39.0))
        );
        assert_eq!(catalogue.find("Opalo"), Some(None));
        assert_eq!(catalogue.find("opalo"), None);
    }

    #[test]
    fn the_catalogue_takes_items_from_the_item_collections_only() {
        let dir = tempfile::tempdir().unwrap();
        let write = |endpoint: &str, entities: Json| {
            std::fs::write(
                dir.path().join(format!("{endpoint}.json")),
                entities.to_string(),
            )
            .unwrap();
        };
        write(
            "weapons",
            serde_json::json!([{"name": "Sollomate Opalo", "economy": {"max_tt": 4.5}}]),
        );
        write(
            "medical_tools",
            serde_json::json!([{"name": "Vivo T1", "economy": {}}]),
        );
        // A creature is not an item, whatever its name.
        write("mobs", serde_json::json!([{"name": "Atrox"}]));
        let store = GameDataStore::new(dir.path()).unwrap();
        let catalogue = ItemCatalogue::from_store(&store);
        assert_eq!(catalogue.find("Sollomate Opalo"), Some(Some(4.5)));
        assert_eq!(catalogue.find("Vivo T1"), Some(None));
        assert_eq!(catalogue.find("Atrox"), None);
    }

    /// `numerals` with every advance and the bias `factor` times as wide:
    /// how one lettering size measures at another interface scale, its
    /// width following the scale and its height hinted.
    fn widened(numerals: &NumeralMetrics, factor: f32) -> NumeralMetrics {
        let scale = |v: f32| v * factor;
        NumeralMetrics {
            digit: scale(numerals.digit),
            one: scale(numerals.one),
            comma: scale(numerals.comma),
            period: scale(numerals.period),
            ped_suffix: scale(numerals.ped_suffix),
            above_tt_prefix: numerals.above_tt_prefix.map(scale),
            percent: numerals.percent.map(scale),
            bracket: numerals.bracket.map(scale),
            bias: scale(numerals.bias),
            ..numerals.clone()
        }
    }

    /// The bundled profile's entry for the drawn queue's scale, and a
    /// second scale's: a taller header band, and numerals as wide as the
    /// drawn lettering measures at a larger scale.
    fn two_scales() -> (SaleQueueScale, SaleQueueScale) {
        let drawn = bundled_profile().windows[WINDOW]
            .sale_queue
            .clone()
            .expect("the bundled profile describes the sell queue")
            .scales[0]
            .clone();
        let larger = SaleQueueScale {
            header_height: [37, 39],
            numerals: widened(&drawn.numerals, 1.12),
            figure_numerals: widened(&drawn.figure_numerals, 1.12),
            ..drawn.clone()
        };
        (drawn, larger)
    }

    /// The bundled profile with its sell queue's scales replaced.
    fn with_scales(scales: Vec<SaleQueueScale>) -> UiProfile {
        let mut profile = bundled_profile();
        profile
            .windows
            .get_mut(WINDOW)
            .and_then(|w| w.sale_queue.as_mut())
            .expect("the bundled profile describes the sell queue")
            .scales = scales;
        profile
    }

    fn read_under(
        profile: &UiProfile,
        layout: &SaleLayout,
        change: impl Fn(String) -> String + Send + Sync,
    ) -> Result<SaleQueue, SaleQueueError> {
        let d = sale_window(layout);
        let inner = d.recogniser();
        let recogniser = move |crop: &BgrImage| inner(crop).map(|(text, c)| (change(text), c));
        read_sale_queue(&d.image, profile, &catalogue(), &recogniser)
    }

    #[test]
    fn the_scale_entry_is_the_one_the_header_band_claims_wherever_it_is_listed() {
        let (drawn, larger) = two_scales();
        let layout = standard_sale_layout();
        let alone = read(&layout).expect("the queue reads");
        for scales in [
            vec![larger.clone(), drawn.clone()],
            vec![drawn.clone(), larger.clone()],
        ] {
            let profile = with_scales(scales);
            let d = sale_window(&layout);
            let measured = measure_sale_queue(&d.image, &profile, &d.recogniser())
                .expect("the queue is found");
            assert_eq!(measured.header_height, SALE_HEADER_HEIGHT);
            let claimed = measured.scale.expect("one entry claims the drawn queue");
            assert_eq!(
                profile.windows[WINDOW].sale_queue.as_ref().unwrap().scales[claimed].header_height,
                drawn.header_height
            );
            assert_eq!(read_under(&profile, &layout, |t| t), Ok(alone.clone()));
        }
        assert_eq!(alone.rows.len(), 3);
    }

    #[test]
    fn a_queue_at_a_scale_the_profile_does_not_list_is_refused_before_anything_is_read() {
        let (drawn, larger) = two_scales();
        let layout = standard_sale_layout();
        assert_eq!(
            read_under(&with_scales(vec![larger.clone()]), &layout, |t| t),
            Err(SaleQueueError::Scale(ScaleError::NotListed {
                measured: SALE_HEADER_HEIGHT
            }))
        );
        // Two entries claiming one header are a fault of the profile, which
        // loading refuses; a profile built in code is refused here too.
        let both = SaleQueueScale {
            header_height: [25, 35],
            ..larger
        };
        assert_eq!(
            read_under(&with_scales(vec![drawn, both]), &layout, |t| t),
            Err(SaleQueueError::Scale(ScaleError::Ambiguous {
                measured: SALE_HEADER_HEIGHT
            }))
        );
        // Measuring still finds the queue, to measure the missing entry
        // from, with the nearest scale's lettering.
        let (_, larger) = two_scales();
        let d = sale_window(&layout);
        let measured = measure_sale_queue(&d.image, &with_scales(vec![larger]), &d.recogniser())
            .expect("the queue is found");
        assert_eq!(
            measured.scale,
            Err(ScaleError::NotListed {
                measured: SALE_HEADER_HEIGHT
            })
        );
        assert_eq!(measured.rows.len(), 3);
        // Every value's lettering is found; only the helpers a row does
        // not draw (the stack under a greyed quantity) are not.
        for row in &measured.rows {
            for field in [
                QueueField::Name,
                QueueField::Tt,
                QueueField::Quantity,
                QueueField::OpeningBid,
                QueueField::OpeningBidFigure,
                QueueField::Buyout,
                QueueField::Days,
                QueueField::Fee,
                QueueField::Markup,
            ] {
                assert!(matches!(row.get(&field), Some(Ok(_))), "{field:?}: {row:?}");
            }
            assert!(!row.contains_key(&QueueField::BuyoutSwitch));
        }
        // A three-word name parts at two gaps at least.
        let name = measured.rows[1][&QueueField::Name].as_ref().unwrap();
        assert!(name.gaps.len() >= 2, "{:?}", name.gaps);
    }

    #[test]
    fn a_figure_is_never_held_to_another_scales_numerals() {
        use crate::game_ui::test_support::{blank, fill};
        let (drawn, larger) = two_scales();
        // The hazard: a figure drawn at the queue's scale, read a digit
        // short, is as wide as the larger scale's numerals make the short
        // text. Held to those, the misreading would pass its width.
        let (text, short) = ("TT+160.47 PED", "TT+16.47 PED");
        for h in [8, 10] {
            let units = expected_width(text, &drawn.numerals).expect("covered");
            let mut image = blank(400, 60, [8, 11, 11]);
            let ink = Rect::new(20, 20, 20 + (units * h as f32).round() as i32 - 1, 19 + h);
            fill(&mut image, ink, [200, 200, 200]);
            assert!(
                width_agrees(&image, ink.padded(3), short, &larger.numerals),
                "at {h}"
            );
            assert!(
                !width_agrees(&image, ink.padded(3), short, &drawn.numerals),
                "at {h}"
            );
        }
        let profile = with_scales(vec![larger.clone(), drawn.clone()]);
        let layout = standard_sale_layout();
        // A short reading: refused by its width under the drawn scale's
        // numerals, wherever the larger scale is listed.
        let queue = read_under(&profile, &layout, move |t| {
            if t == text {
                short.to_string()
            } else {
                t
            }
        })
        .expect("the queue reads");
        let width = |field| RowRefusal::WrongWidth {
            field,
            raw: short.into(),
        };
        assert_eq!(queue.rows.len(), 2);
        assert_eq!(
            queue.unread.iter().map(|u| &u.refusals).collect::<Vec<_>>(),
            vec![&vec![
                width(QueueField::BuyoutFigure),
                width(QueueField::Markup)
            ]]
        );
        // The recogniser's known failure on every reading: one of each
        // repeated pair of digits dropped. No damaged value is returned,
        // and the rows with nothing to damage still read.
        let dropping = |text: String| {
            let chars: Vec<char> = text.chars().collect();
            let mut out = String::new();
            let mut i = 0;
            while i < chars.len() {
                out.push(chars[i]);
                let pair =
                    i + 1 < chars.len() && chars[i].is_ascii_digit() && chars[i] == chars[i + 1];
                i += if pair { 2 } else { 1 };
            }
            out
        };
        let queue = read_under(&profile, &layout, dropping).expect("the queue reads");
        assert_eq!(
            queue.rows.iter().map(texts).collect::<Vec<_>>(),
            vec![vec![
                "Output Amplifier Component".to_string(),
                "20.80 PED".into(),
                "52".into(),
                "42".into(),
                "off".into(),
                "7".into(),
                "1.54 PED".into(),
                "201.92%".into(),
            ]]
        );
        assert_eq!(queue.unread.len(), 2);
        for row in &queue.unread {
            assert!(
                row.refusals
                    .iter()
                    .all(|r| matches!(r, RowRefusal::WrongWidth { .. } | RowRefusal::Disagrees(_))),
                "{row:?}"
            );
        }
        // Held to the larger scale's numerals, the drawn queue's true
        // readings are the wrong width: the entry the header claims is
        // the one that measures it.
        let wrong = SaleQueueScale {
            header_height: drawn.header_height,
            ..larger.clone()
        };
        let swapped = with_scales(vec![wrong]);
        let queue = read_under(&swapped, &layout, |t| t).expect("the queue reads");
        assert!(queue.rows.is_empty(), "{queue:?}");
        assert!(queue.unread.iter().all(|u| u
            .refusals
            .iter()
            .any(|r| matches!(r, RowRefusal::WrongWidth { .. }))));
    }

    #[test]
    fn the_profile_must_place_every_field_once_with_a_pick_that_serves_it() {
        let queue = bundled_profile().windows[WINDOW]
            .sale_queue
            .clone()
            .expect("the bundled profile describes the sell queue");
        assert!(plan(&queue).is_ok());
        let column = |q: &SaleQueueProfile, heading: &str| {
            q.columns
                .iter()
                .position(|c| c.heading == heading)
                .expect("a column")
        };
        let mut missing = queue.clone();
        missing.columns.retain(|c| c.heading != "FEE");
        assert_eq!(plan(&missing).err(), Some("Fee is not placed".into()));
        let mut twice = queue.clone();
        let fee = column(&twice, "FEE");
        twice.columns[fee].fields.push(FieldPick {
            field: QueueField::Days,
            pick: Pick::Only,
        });
        assert_eq!(plan(&twice).err(), Some("Days is placed twice".into()));
        let mut unboxed = queue.clone();
        let quantity = column(&unboxed, "QUANTITY");
        unboxed.columns[quantity].fields[0].pick = Pick::Topmost;
        assert_eq!(
            plan(&unboxed).err(),
            Some("Quantity cannot be read as Topmost".into())
        );
        let mut orphan = queue.clone();
        let bid = column(&orphan, "OPENING BID");
        let figure = orphan.columns[bid].fields.remove(1);
        let fee = column(&orphan, "FEE");
        orphan.columns[fee].fields.push(figure);
        assert_eq!(
            plan(&orphan).err(),
            Some("OpeningBidFigure cannot be read as UnderBox".into())
        );
    }
}
