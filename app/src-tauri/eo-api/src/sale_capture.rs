//! The capture of the auction's sell queue as listing drafts.
//!
//! One capture reads every queued item in view and answers one draft per
//! item that read whole. A draft fills the Create listing form, where the
//! player reviews and may change every figure before anything is recorded:
//! nothing here commits. A figure the reader could only bound or only read,
//! with nothing the game prints to check it against, is marked for the
//! player to check; one the game's own figures fixed is not. A queued item
//! that did not read whole answers which figure failed and how, and no
//! figure of it. A capture that read no queue answers why, with no draft.

use eo_services::game_ui::sale_queue::Assurance;
use eo_services::game_ui::sale_queue::{
    Buyout, Identity, ItemName, QueueCount, QueueField, QueuedItem, RowRefusal, UnreadQueueRow,
};
use eo_services::game_ui::validate::without_unit;
use eo_services::sale_capture::{QueueCapture, SaleCaptureRefusal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Api, ApiError, Nullable};

/// One capture of the sell queue: the listing drafts it read, or why
/// there are none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SaleQueueCapture {
    #[serde(rename_all = "camelCase")]
    Read {
        /// The capture's identity, increasing in the order captures were
        /// started: a capture with a lower one is older, and its drafts
        /// never replace a newer capture's.
        capture_id: i64,
        /// When the screen was read, as a local timestamp: what a listing
        /// recorded from one of these drafts is dated to.
        read_at: String,
        /// One per queued item that read whole, top to bottom.
        drafts: Vec<QueuedListingDraft>,
        /// The queued items in view that did not read whole.
        unread: Vec<UnreadQueuedItem>,
        /// How many items the window says are queued.
        queued: QueuedCount,
    },
    #[serde(rename_all = "camelCase")]
    Refused {
        capture_id: i64,
        reason: SaleQueueRefusal,
    },
}

/// Why a capture produced no listing drafts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SaleQueueRefusal {
    /// No game window is showing.
    GameWindowNotFound,
    /// The game window lies wholly outside what can be captured.
    GameWindowOffScreen,
    /// The game window was found but could not be captured.
    CaptureFailed,
    /// Screen capture was not allowed.
    CaptureNotAllowed,
    /// The Auction sell window is not open, or its title is covered.
    SellWindowNotOpen,
    /// The sell window is partly covered or off the screen.
    SellWindowNotWhole,
    /// The sell queue's headings or dividers are covered.
    QueueNotVisible,
    /// The sell window's price toggle is covered or did not read.
    PriceModeUnread,
    /// The sell window states prices as markup rather than in PED.
    OtherPriceMode,
    /// Nothing is queued.
    QueueEmpty,
    /// No recogniser or interface profile is available.
    ReaderUnavailable,
}

/// How many items the window says are queued, against the drafts and
/// unread items a capture answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueuedCount {
    /// Every queued item read.
    All,
    /// The window says `of` items are queued; those not among the drafts
    /// or the unread items are out of view or covered.
    #[serde(rename_all = "camelCase")]
    Some { of: i64 },
    /// The window's count could not be read: how many are queued is not
    /// known.
    Unread,
    /// The window's count reads as fewer than the rows in view, so it is
    /// misread and not passed on: how many are queued is not known.
    Mismatched,
}

/// One queued item, read whole, as a draft listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedListingDraft {
    /// Names this draft within its capture alone.
    pub draft_id: String,
    pub item_name: DraftName,
    /// The TT value of the quantity queued.
    pub tt_value: ReadFigure,
    pub quantity: ReadFigure,
    pub starting_bid: ReadFigure,
    pub buyout: DraftBuyout,
    pub auction_days: ReadFigure,
    pub listing_fee: ReadFigure,
}

/// A queued item's name as read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftName {
    pub text: String,
    /// What bears the name out. Only the application's item data confirms
    /// it; otherwise the player should check it against the window.
    pub found: NameFound,
}

/// Where a queued item's name was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NameFound {
    /// The application's item data names exactly this item.
    ItemData,
    /// Only the window's own inventory list shows it, read in a like font:
    /// a letter the recogniser always takes for another confirms itself
    /// there.
    InventoryList,
    /// Nothing bears it out.
    Nowhere,
}

/// A figure as read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadFigure {
    pub value: f64,
    /// The figure exactly as the window prints it, without the unit:
    /// `11.52`, `162`.
    pub text: String,
    /// Nothing the window prints checks this figure beyond its own shape
    /// and bounds, so a misread digit could pass: the player should check
    /// it against the window.
    pub check: bool,
}

/// The buyout as read: switched off, or a price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DraftBuyout {
    /// No buyout is set.
    Off,
    #[serde(rename_all = "camelCase")]
    Price { figure: ReadFigure },
}

/// A queued item in view that did not read whole. Nothing of it that did
/// read is carried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnreadQueuedItem {
    /// Each figure that failed, and how, in the order the row is read.
    pub problems: Vec<UnreadProblem>,
}

/// One figure of a queued item that did not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnreadProblem {
    pub field: QueuedField,
    pub why: UnreadWhy,
}

/// A figure of a queued item, by what the player knows it as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum QueuedField {
    ItemName,
    TtValue,
    Quantity,
    StartingBid,
    Buyout,
    AuctionDays,
    ListingFee,
    Markup,
}

/// How a figure failed to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UnreadWhy {
    /// It did not read clearly.
    NotRead,
    /// It is cut short to fit its column or box.
    CutOff,
    /// It reads as outside what the window allows.
    OutOfBounds,
    /// It does not agree with the window's other figures.
    Disagrees,
}

impl From<SaleCaptureRefusal> for SaleQueueRefusal {
    fn from(refusal: SaleCaptureRefusal) -> Self {
        match refusal {
            SaleCaptureRefusal::GameWindowNotFound => Self::GameWindowNotFound,
            SaleCaptureRefusal::GameWindowOffScreen => Self::GameWindowOffScreen,
            SaleCaptureRefusal::CaptureFailed => Self::CaptureFailed,
            SaleCaptureRefusal::CaptureNotAllowed => Self::CaptureNotAllowed,
            SaleCaptureRefusal::SellWindowNotOpen => Self::SellWindowNotOpen,
            SaleCaptureRefusal::SellWindowNotWhole => Self::SellWindowNotWhole,
            SaleCaptureRefusal::QueueNotVisible => Self::QueueNotVisible,
            SaleCaptureRefusal::PriceModeUnread => Self::PriceModeUnread,
            SaleCaptureRefusal::OtherPriceMode => Self::OtherPriceMode,
            SaleCaptureRefusal::QueueEmpty => Self::QueueEmpty,
            SaleCaptureRefusal::ReaderUnavailable => Self::ReaderUnavailable,
        }
    }
}

/// Whether a figure the reader checked by `assurance` is one a misread
/// could hide in: one only bounds or nothing at all held.
fn needs_check(assurance: Assurance) -> bool {
    matches!(assurance, Assurance::Bounded | Assurance::ReadOnly)
}

fn figure(value: f64, printed: &str, check: bool) -> ReadFigure {
    ReadFigure {
        value,
        text: without_unit(printed).to_string(),
        check,
    }
}

fn draft(capture_id: u64, row: usize, item: &QueuedItem) -> QueuedListingDraft {
    let checked = |field| needs_check(item.assurance(field));
    QueuedListingDraft {
        draft_id: format!("{capture_id}-{row}"),
        item_name: DraftName {
            text: item.name.text().to_string(),
            found: match item.name {
                ItemName::Catalogued(_) => NameFound::ItemData,
                ItemName::Listed(_) => NameFound::InventoryList,
                ItemName::Unconfirmed(_) => NameFound::Nowhere,
            },
        },
        // The TT value is fixed by the figures only to within the rounding
        // the client's printing hides, and every markup is measured against
        // it, so it is always the player's to check.
        tt_value: figure(item.tt.ped, &item.tt.text, true),
        quantity: figure(
            item.quantity.value as f64,
            &item.quantity.text,
            checked(QueueField::Quantity),
        ),
        starting_bid: figure(
            item.opening_bid.value as f64,
            &item.opening_bid.text,
            checked(QueueField::OpeningBid),
        ),
        buyout: match &item.buyout {
            Buyout::Off => DraftBuyout::Off,
            Buyout::Price(price) => DraftBuyout::Price {
                figure: figure(price.value as f64, &price.text, checked(QueueField::Buyout)),
            },
        },
        auction_days: figure(
            item.days.value as f64,
            &item.days.text,
            checked(QueueField::Days),
        ),
        listing_fee: figure(item.fee.ped, &item.fee.text, checked(QueueField::Fee)),
    }
}

fn queued_field(field: QueueField) -> QueuedField {
    match field {
        QueueField::Name => QueuedField::ItemName,
        QueueField::Tt => QueuedField::TtValue,
        QueueField::Quantity | QueueField::QuantityStack => QueuedField::Quantity,
        QueueField::OpeningBid | QueueField::OpeningBidFigure => QueuedField::StartingBid,
        QueueField::BuyoutSwitch | QueueField::Buyout | QueueField::BuyoutFigure => {
            QueuedField::Buyout
        }
        QueueField::Days | QueueField::DaysRange => QueuedField::AuctionDays,
        QueueField::Fee => QueuedField::ListingFee,
        QueueField::Markup => QueuedField::Markup,
    }
}

fn problem(refusal: &RowRefusal) -> UnreadProblem {
    let (field, why) = match refusal {
        RowRefusal::NotFound(field)
        | RowRefusal::Ambiguous(field)
        | RowRefusal::Shading(field)
        | RowRefusal::Invalid { field, .. }
        | RowRefusal::WrongWidth { field, .. }
        | RowRefusal::WordCount { field, .. } => (queued_field(*field), UnreadWhy::NotRead),
        RowRefusal::Clipped(field) | RowRefusal::Cut(field) => {
            (queued_field(*field), UnreadWhy::CutOff)
        }
        RowRefusal::OutOfBounds { field, .. } => (queued_field(*field), UnreadWhy::OutOfBounds),
        RowRefusal::Disagrees(identity) => (
            match identity {
                Identity::OpeningBidFigure => QueuedField::StartingBid,
                Identity::BuyoutFigure | Identity::BuyoutSwitch => QueuedField::Buyout,
                Identity::Headline => QueuedField::Markup,
                Identity::Fee => QueuedField::ListingFee,
                Identity::StackCount => QueuedField::Quantity,
            },
            UnreadWhy::Disagrees,
        ),
    };
    UnreadProblem { field, why }
}

fn unread(row: &UnreadQueueRow) -> UnreadQueuedItem {
    UnreadQueuedItem {
        problems: row.refusals.iter().map(problem).collect(),
    }
}

impl From<QueueCapture> for SaleQueueCapture {
    fn from(capture: QueueCapture) -> Self {
        let capture_id = capture.id as i64;
        match capture.outcome {
            Err(refusal) => Self::Refused {
                capture_id,
                reason: refusal.into(),
            },
            Ok(reading) => Self::Read {
                capture_id,
                read_at: reading.read_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
                drafts: reading
                    .queue
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(row, item)| draft(capture.id, row, item))
                    .collect(),
                unread: reading.queue.unread.iter().map(unread).collect(),
                queued: match reading.queue.count {
                    QueueCount::Complete(_) => QueuedCount::All,
                    QueueCount::Incomplete { heading, .. } => {
                        QueuedCount::Some { of: heading as i64 }
                    }
                    // A count fewer than the rows in view is a misreading,
                    // and is not passed on as a figure.
                    QueueCount::Contradicted { .. } => QueuedCount::Mismatched,
                    QueueCount::Unread => QueuedCount::Unread,
                },
            },
        }
    }
}

impl SaleQueueCapture {
    fn capture_id(&self) -> i64 {
        match self {
            Self::Read { capture_id, .. } | Self::Refused { capture_id, .. } => *capture_id,
        }
    }
}

impl Api {
    /// Read the auction's sell queue once and answer a draft per queued
    /// item, or why there are none.
    ///
    /// This fills a form; it never commits. Capture and typing converge on
    /// the same form and meet the same checks, so a misread cannot reach
    /// the ledger by a path a typo could not.
    ///
    /// Synchronous: the capture blocks on the platform's capturer and the
    /// recogniser, so its caller must offload it rather than run it on a
    /// runtime worker.
    pub fn inventory_sale_window_capture(&self) -> Result<SaleQueueCapture, ApiError> {
        Ok(self.sale_capture.capture().into())
    }

    /// Capture the sell queue for the main window's form, from the
    /// floating capture button: the form may not be on screen when the
    /// button is pressed, so the capture waits to be collected. A capture
    /// waiting already is replaced only by a newer one.
    ///
    /// Shell-only (the overlay's own command); not an IPC operation.
    /// Synchronous, as [`Self::inventory_sale_window_capture`].
    pub fn inventory_sale_window_capture_for_shell(&self) -> SaleQueueCapture {
        let capture: SaleQueueCapture = self.sale_capture.capture().into();
        if let Ok(mut slot) = self.last_sale_capture.lock() {
            if slot
                .as_ref()
                .is_none_or(|held| held.capture_id() < capture.capture_id())
            {
                *slot = Some(capture.clone());
            }
        }
        capture
    }

    /// Collect the capture waiting for the form, clearing it.
    pub fn inventory_sale_window_take_capture(&self) -> Nullable<SaleQueueCapture> {
        self.last_sale_capture
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use eo_services::game_ui::sale_queue::SaleQueue;
    use eo_services::game_ui::sale_queue::{Amount, Price, Setting, SpinnerValue};
    use eo_services::game_ui::validate::markup;
    use eo_services::sale_capture::QueueReading;

    fn spinner(text: &str, setting: Setting) -> SpinnerValue {
        SpinnerValue {
            text: text.into(),
            value: text.parse().unwrap(),
            setting,
        }
    }

    fn amount(text: &str) -> Amount {
        Amount {
            text: text.into(),
            ped: without_unit(text).parse().unwrap(),
        }
    }

    fn item(name: ItemName, quantity: Setting, buyout: Buyout) -> QueuedItem {
        QueuedItem {
            name,
            tt: amount("11.52 PED"),
            quantity: spinner("1", quantity),
            stack: None,
            opening_bid: spinner("162", Setting::Editable),
            buyout,
            days: spinner("7", Setting::Editable),
            days_allowed: (1, 7),
            fee: amount("7.33 PED"),
            fee_fixed: true,
            markup: markup("TT+160.47 PED").unwrap(),
        }
    }

    fn moment() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 3)
            .unwrap()
            .and_hms_opt(19, 42, 7)
            .unwrap()
    }

    fn read(rows: Vec<QueuedItem>, unread: Vec<UnreadQueueRow>, count: QueueCount) -> QueueCapture {
        QueueCapture {
            id: 4,
            outcome: Ok(QueueReading {
                read_at: moment(),
                queue: SaleQueue {
                    rows,
                    unread,
                    partial: 0,
                    count,
                },
            }),
        }
    }

    #[test]
    fn a_draft_marks_for_checking_exactly_the_figures_nothing_else_fixes() {
        let capture = read(
            vec![
                item(
                    ItemName::Catalogued("Electric Attack Nanochip 3".into()),
                    Setting::Locked,
                    Buyout::Price(Price {
                        text: "172".into(),
                        value: 172,
                    }),
                ),
                item(
                    ItemName::Listed("Output Amplifier Component".into()),
                    Setting::Editable,
                    Buyout::Off,
                ),
            ],
            Vec::new(),
            QueueCount::Complete(2),
        );
        let wire = serde_json::to_value(SaleQueueCapture::from(capture)).unwrap();
        let figure = |value: f64, text: &str, check: bool| serde_json::json!({"value": value, "text": text, "check": check});
        assert_eq!(
            wire,
            serde_json::json!({
                "kind": "read",
                "captureId": 4,
                "readAt": "2026-11-03T19:42:07",
                "drafts": [
                    {
                        "draftId": "4-0",
                        "itemName": {"text": "Electric Attack Nanochip 3", "found": "itemData"},
                        "ttValue": figure(11.52, "11.52", true),
                        "quantity": figure(1.0, "1", true),
                        "startingBid": figure(162.0, "162", false),
                        "buyout": {"kind": "price", "figure": figure(172.0, "172", false)},
                        "auctionDays": figure(7.0, "7", true),
                        "listingFee": figure(7.33, "7.33", false),
                    },
                    {
                        "draftId": "4-1",
                        // Weak evidence: the list is read in a like font.
                        "itemName": {"text": "Output Amplifier Component", "found": "inventoryList"},
                        "ttValue": figure(11.52, "11.52", true),
                        "quantity": figure(1.0, "1", true),
                        "startingBid": figure(162.0, "162", false),
                        "buyout": {"kind": "off"},
                        "auctionDays": figure(7.0, "7", true),
                        "listingFee": figure(7.33, "7.33", false),
                    },
                ],
                "unread": [],
                "queued": {"kind": "all"},
            })
        );
    }

    #[test]
    fn an_unread_item_says_which_figure_failed_and_how_and_carries_none() {
        let capture = read(
            Vec::new(),
            vec![UnreadQueueRow {
                raw_name: Some("Output Amplifier Component".into()),
                refusals: vec![
                    RowRefusal::WrongWidth {
                        field: QueueField::OpeningBid,
                        raw: "16".into(),
                    },
                    RowRefusal::Cut(QueueField::Name),
                    RowRefusal::OutOfBounds {
                        field: QueueField::Days,
                        raw: "8".into(),
                    },
                    RowRefusal::Disagrees(Identity::Fee),
                    RowRefusal::Clipped(QueueField::QuantityStack),
                ],
                fee_disagreement: None,
            }],
            QueueCount::Incomplete {
                heading: 3,
                read: 0,
                unread: 1,
                partial: 0,
            },
        );
        let wire = serde_json::to_value(SaleQueueCapture::from(capture)).unwrap();
        assert_eq!(
            wire["unread"],
            serde_json::json!([{"problems": [
                {"field": "startingBid", "why": "notRead"},
                {"field": "itemName", "why": "cutOff"},
                {"field": "auctionDays", "why": "outOfBounds"},
                {"field": "listingFee", "why": "disagrees"},
                {"field": "quantity", "why": "cutOff"},
            ]}])
        );
        assert_eq!(wire["queued"], serde_json::json!({"kind": "some", "of": 3}));
        let text = wire.to_string();
        assert!(!text.contains("Output Amplifier"), "{text}");
        assert!(!text.contains("\"16\""), "{text}");
    }

    #[test]
    fn a_count_that_contradicts_the_rows_in_view_is_not_passed_on() {
        for (count, kind) in [
            (
                QueueCount::Contradicted {
                    heading: 1,
                    seen: 3,
                },
                "mismatched",
            ),
            (QueueCount::Unread, "unread"),
        ] {
            let wire =
                serde_json::to_value(SaleQueueCapture::from(read(Vec::new(), Vec::new(), count)))
                    .unwrap();
            assert_eq!(wire["queued"], serde_json::json!({"kind": kind}));
        }
    }

    #[test]
    fn a_fee_the_model_admits_two_values_for_is_marked_for_checking() {
        let mut ambiguous = item(
            ItemName::Unconfirmed("Animal Hide".into()),
            Setting::Locked,
            Buyout::Off,
        );
        ambiguous.fee_fixed = false;
        let wire = serde_json::to_value(SaleQueueCapture::from(read(
            vec![ambiguous],
            Vec::new(),
            QueueCount::Complete(1),
        )))
        .unwrap();
        assert_eq!(wire["drafts"][0]["listingFee"]["check"], true);
        assert_eq!(wire["drafts"][0]["itemName"]["found"], "nowhere");
    }

    #[test]
    fn every_refusal_reaches_the_wire_by_name_and_without_a_draft() {
        for (refusal, reason) in [
            (SaleCaptureRefusal::GameWindowNotFound, "gameWindowNotFound"),
            (
                SaleCaptureRefusal::GameWindowOffScreen,
                "gameWindowOffScreen",
            ),
            (SaleCaptureRefusal::CaptureFailed, "captureFailed"),
            (SaleCaptureRefusal::CaptureNotAllowed, "captureNotAllowed"),
            (SaleCaptureRefusal::SellWindowNotOpen, "sellWindowNotOpen"),
            (SaleCaptureRefusal::SellWindowNotWhole, "sellWindowNotWhole"),
            (SaleCaptureRefusal::QueueNotVisible, "queueNotVisible"),
            (SaleCaptureRefusal::PriceModeUnread, "priceModeUnread"),
            (SaleCaptureRefusal::OtherPriceMode, "otherPriceMode"),
            (SaleCaptureRefusal::QueueEmpty, "queueEmpty"),
            (SaleCaptureRefusal::ReaderUnavailable, "readerUnavailable"),
        ] {
            let capture = QueueCapture {
                id: 9,
                outcome: Err(refusal),
            };
            assert_eq!(
                serde_json::to_value(SaleQueueCapture::from(capture)).unwrap(),
                serde_json::json!({"kind": "refused", "captureId": 9, "reason": reason}),
                "{refusal:?}"
            );
        }
    }
}
