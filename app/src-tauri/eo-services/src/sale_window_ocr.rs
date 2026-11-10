//! Sale-window OCR: a one-shot screen read of the in-game auction
//! window's fields, so a listing can be recorded from what the game
//! shows rather than retyped.
//!
//! The window lookup, the capture and the recogniser arrive as injected
//! providers, and the capture region derives at scan time from the live
//! game window (the user docks the sale window bottom-right at default
//! interface scale; the calibrated anchor encodes each field's rectangle
//! relative to that corner).
//!
//! Every field feeds a draft that becomes a ledger entry, so a field
//! that does not read must come back **empty**, never approximate, and
//! never zero. An empty field
//! costs the user one retype; a confidently wrong one corrupts a
//! transaction record silently, and nothing downstream can tell it from
//! a figure that was really on screen.
//!
//! So each field is gated twice: the recogniser's own confidence must
//! clear a floor, and the parsed value must be plausible for what that
//! field can hold. Whatever fails either gate is named in `unread` and
//! left out, and the panel is captured once with every field cropped
//! from that single frame, so the fields cannot disagree about which
//! moment they describe.

use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::fuzzy_match::wratio;
use crate::game_ui::seams::{FrameReader, RegionCapture, RegionLookup};
use crate::ocr_text::parse_amount;
use crate::scan_presets::PanelAnchor;
use crate::skill_panel::BgrImage;

/// The capture observer (the recording controller's seam): called as
/// `tap(panel, region, frame)` after a successful grab.
pub type SaleCaptureTap = Arc<dyn Fn(&str, &Value, &BgrImage) + Send + Sync>;

/// The calibrated field rectangles, panel-relative.
pub type AnchorLookup = Arc<dyn Fn() -> PanelAnchor + Send + Sync>;

/// Below this, a field is treated as unread rather than trusted. The
/// recogniser reports per-line confidence; a low score on a short
/// numeric field usually means it read furniture (a spinner arrow, a
/// label's edge) rather than the value.
pub const FIELD_CONFIDENCE_FLOOR: f64 = 0.50;

/// The rectangle that proves the window is where the calibration says.
///
/// Nothing else in the read checks that. Every field rectangle is taken on
/// faith from the panel anchor, so a window docked somewhere else, or an
/// interface at a different scale, would have the read cropping whatever
/// happens to sit at those offsets. The confidence floor catches most of
/// that, but not all: another panel's digits can read cleanly and mean
/// something entirely different, and a wrong figure that looks right is
/// the one failure this design exists to prevent.
///
/// So one rectangle covers a label the window always shows, and the whole
/// capture is refused unless it reads as that label. The text is fixed in
/// code rather than calibrated because the game owns it, not the user.
pub const LANDMARK_FIELD: &str = "landmark";
pub const LANDMARK_TEXT: &str = "Item Name";

/// How close the landmark must read. Below this the panel is not where it
/// is supposed to be, and no field of the capture can be trusted.
pub const LANDMARK_SCORE_FLOOR: f64 = 70.0;

/// The fields this read expects, in the order the window shows them.
/// A calibration carrying fewer is read for what it has; one carrying
/// more has the extras ignored, so recalibrating cannot silently add a
/// field nothing knows how to interpret.
pub const SALE_FIELDS: [&str; 7] = [
    "item_name",
    "quantity",
    "tt_value",
    "auction_fee",
    "auction_days",
    "starting_bid",
    "buyout",
];

/// The provider seams the composition root wires in.
pub struct SaleWindowProviders {
    /// The sale window's region from the live game window.
    pub sale_window_region: RegionLookup,
    /// The calibrated panel geometry.
    pub anchor: AnchorLookup,
    /// Capture an `x/y/w/h` screen rectangle as BGR pixels.
    pub capture_region: RegionCapture,
    /// Recognise one frame.
    pub read_text: FrameReader,
}

impl Default for SaleWindowProviders {
    fn default() -> Self {
        Self {
            sale_window_region: Arc::new(|| None),
            anchor: Arc::new(PanelAnchor::empty),
            capture_region: Arc::new(|_, _, _, _| None),
            read_text: Arc::new(|_| None),
        }
    }
}

/// One-shot OCR for the auction sale window.
pub struct SaleWindowOcrService {
    providers: SaleWindowProviders,
    capture_tap: Mutex<Option<SaleCaptureTap>>,
}

/// One sale-window observation: the parsed answer plus the exact panel
/// pixels and screen rectangle it came from. Production callers normally
/// need only [`SaleWindowOcrService::scan_sale_window`]; the richer form is
/// for development research that must preserve the evidence behind each
/// quoted fee.
#[derive(Clone)]
pub struct SaleWindowObservation {
    pub read: Value,
    pub region: Option<Value>,
    pub frame: Option<BgrImage>,
}

impl SaleWindowOcrService {
    pub fn new(providers: SaleWindowProviders) -> Self {
        Self {
            providers,
            capture_tap: Mutex::new(None),
        }
    }

    /// Install a capture observer (called after a successful grab).
    pub fn set_capture_tap(&self, tap: SaleCaptureTap) {
        *self.capture_tap.lock().expect("capture tap") = Some(tap);
    }

    /// Remove the capture observer.
    pub fn clear_capture_tap(&self) {
        *self.capture_tap.lock().expect("capture tap") = None;
    }

    /// Capture the sale window and read its fields. Answers the parsed
    /// fields it could resolve, the names of those it could not
    /// (`unread`), and the lowest confidence among those it did
    /// (`confidence`), or a single `error` when there was nothing to
    /// read at all.
    pub fn scan_sale_window(&self) -> Value {
        self.observe_sale_window().read
    }

    /// Capture and read the sale window while retaining the source frame.
    /// The OCR path is shared with [`scan_sale_window`](Self::scan_sale_window),
    /// so research capture cannot drift into a second parser.
    pub fn observe_sale_window(&self) -> SaleWindowObservation {
        let failure = |error: &str| json!({ "error": error, "unread": SALE_FIELDS });

        let anchor = (self.providers.anchor)();
        if anchor.cells.is_empty() {
            return SaleWindowObservation {
                read: failure("The sale-window calibration is missing from this installation"),
                region: None,
                frame: None,
            };
        }
        let Some((top_left, bottom_right)) = (self.providers.sale_window_region)() else {
            return SaleWindowObservation {
                read: failure("Entropia Universe window not found: start the game first"),
                region: None,
                frame: None,
            };
        };
        let (x, y) = (top_left[0], top_left[1]);
        let (w, h) = (bottom_right[0] - x, bottom_right[1] - y);
        let region = json!({"x": x, "y": y, "w": w, "h": h});
        if w <= 0 || h <= 0 {
            return SaleWindowObservation {
                read: failure("Invalid region"),
                region: Some(region),
                frame: None,
            };
        }
        let Some(panel) = (self.providers.capture_region)(x, y, w, h) else {
            return SaleWindowObservation {
                read: failure("Capture failed"),
                region: Some(region),
                frame: None,
            };
        };

        let tap = self.capture_tap.lock().expect("capture tap").clone();
        if let Some(tap) = tap {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tap("sale_window", &region, &panel)
            }));
        }

        let read = self.read_panel(&anchor, &panel, &failure);
        SaleWindowObservation {
            read,
            region: Some(region),
            frame: Some(panel),
        }
    }

    fn read_panel(
        &self,
        anchor: &PanelAnchor,
        panel: &BgrImage,
        failure: &dyn Fn(&str) -> Value,
    ) -> Value {
        // The landmark first: everything after it is only meaningful if the
        // panel really is the sale window. A calibration recorded before
        // there was a landmark to record carries none, and reads on without
        // one rather than refusing work that used to be fine.
        if let Some((_, cell)) = anchor.cells.iter().find(|(name, _)| name == LANDMARK_FIELD) {
            let (cx, cy, cw, ch) = cell.single_rect();
            let crop = panel.crop(cy, cy + ch, cx, cx + cw);
            let seen = (self.providers.read_text)(&crop).map(|(text, _)| text);
            let score = seen
                .as_deref()
                .map(|text| wratio(text.trim(), LANDMARK_TEXT))
                .unwrap_or(0.0);
            if score < LANDMARK_SCORE_FLOOR {
                return failure(
                    "That is not the sale window: dock it in the bottom-right corner at the \
                     default interface scale",
                );
            }
        }

        let mut fields = Map::new();
        let mut unread: Vec<String> = Vec::new();
        let mut lowest: Option<f64> = None;
        let mut engine_answered = false;

        for name in SALE_FIELDS {
            let Some((_, cell)) = anchor.cells.iter().find(|(cell, _)| cell == name) else {
                unread.push(name.to_string());
                continue;
            };
            let (cx, cy, cw, ch) = cell.single_rect();
            let crop = panel.crop(cy, cy + ch, cx, cx + cw);
            let Some((text, confidence)) = (self.providers.read_text)(&crop) else {
                unread.push(name.to_string());
                continue;
            };
            engine_answered = true;
            match resolve_field(name, &text, confidence) {
                Some(value) => {
                    fields.insert(name.to_string(), value);
                    lowest = Some(lowest.map_or(confidence, |low: f64| low.min(confidence)));
                }
                None => unread.push(name.to_string()),
            }
        }

        if !engine_answered {
            return failure("Local OCR engine unavailable");
        }

        let mut result = Value::Object(fields);
        let object = result.as_object_mut().expect("field object");
        object.insert("unread".into(), json!(unread));
        object.insert("confidence".into(), json!(lowest));
        result
    }
}

/// Parse one field's text, refusing anything the field cannot hold.
///
/// The plausibility bounds are deliberately loose: they exist to catch a
/// misread, not to police what the user may list. Anything they let
/// through still lands in a form the user reviews before committing.
fn resolve_field(name: &str, text: &str, confidence: f64) -> Option<Value> {
    if confidence < FIELD_CONFIDENCE_FLOOR {
        return None;
    }
    if name == "item_name" {
        let trimmed = text.trim();
        // A name is matched against holdings downstream, which does its
        // own refusing; the only thing to reject here is nothing at all.
        return (!trimmed.is_empty()).then(|| json!(trimmed));
    }
    let value = parse_amount(text)?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    match name {
        // A listing of nothing is not a listing.
        "quantity" => (value > 0.0).then(|| json!(value)),
        // The longest auction the window offers is a fortnight; a read
        // outside that is furniture, not a duration.
        "auction_days" => ((1.0..=14.0).contains(&value) && value.fract() == 0.0)
            .then(|| json!(value.round() as i64)),
        // The remaining money fields carry no upper bound worth
        // asserting: a genuinely large listing is a real thing.
        _ => Some(json!(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_presets::CellGeometry;

    fn cell(name: &str, x: i64, y: i64) -> (String, CellGeometry) {
        (
            name.to_string(),
            CellGeometry {
                x_left: x,
                x_right: x + 10,
                first_y_top: y,
                last_y_top: y,
                height: 4,
            },
        )
    }

    fn calibrated() -> PanelAnchor {
        PanelAnchor {
            width: 40,
            height: 40,
            right_offset: 0,
            bottom_offset: 0,
            n_rows: None,
            cells: SALE_FIELDS
                .iter()
                .enumerate()
                .map(|(index, name)| cell(name, 0, index as i64 * 5))
                .collect(),
        }
    }

    fn panel() -> BgrImage {
        BgrImage {
            data: vec![0; 40 * 40 * 3],
            h: 40,
            w: 40,
        }
    }

    /// Providers that answer every field with the same text.
    fn reading(text: &'static str, confidence: f64) -> SaleWindowProviders {
        SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(calibrated),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new(move |_| Some((text.to_string(), confidence))),
        }
    }

    #[test]
    fn each_failure_leg_names_itself_and_reads_nothing() {
        let service = SaleWindowOcrService::new(SaleWindowProviders::default());
        let result = service.scan_sale_window();
        assert_eq!(
            result["error"],
            "The sale-window calibration is missing from this installation"
        );
        assert_eq!(
            result["unread"].as_array().unwrap().len(),
            SALE_FIELDS.len()
        );

        let service = SaleWindowOcrService::new(SaleWindowProviders {
            anchor: Arc::new(calibrated),
            ..Default::default()
        });
        assert_eq!(
            service.scan_sale_window()["error"],
            "Entropia Universe window not found: start the game first"
        );

        let service = SaleWindowOcrService::new(SaleWindowProviders {
            anchor: Arc::new(calibrated),
            sale_window_region: Arc::new(|| Some(([10, 10], [10, 30]))),
            ..Default::default()
        });
        assert_eq!(service.scan_sale_window()["error"], "Invalid region");

        let service = SaleWindowOcrService::new(SaleWindowProviders {
            anchor: Arc::new(calibrated),
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            ..Default::default()
        });
        assert_eq!(service.scan_sale_window()["error"], "Capture failed");

        let service = SaleWindowOcrService::new(SaleWindowProviders {
            anchor: Arc::new(calibrated),
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            ..Default::default()
        });
        assert_eq!(
            service.scan_sale_window()["error"],
            "Local OCR engine unavailable"
        );
    }

    #[test]
    fn a_read_panel_answers_each_field_from_one_capture() {
        let texts: std::collections::HashMap<&str, &str> = [
            ("item_name", "Shrapnel"),
            ("quantity", "2754889"),
            ("tt_value", "275.48 PED"),
            ("auction_fee", "64.51 PED"),
            ("auction_days", "6"),
            ("starting_bid", "0 0 0 9 2 7 6"),
            ("buyout", "0009276"),
        ]
        .into_iter()
        .collect();

        // The crops are laid out one per five rows, so the y of the crop
        // identifies which field the reader was handed.
        let captures = Arc::new(Mutex::new(0usize));
        let counter = captures.clone();
        let service = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(calibrated),
            capture_region: Arc::new(move |_, _, _, _| {
                *counter.lock().unwrap() += 1;
                Some(panel())
            }),
            read_text: Arc::new(move |crop| {
                let index = (crop.h, crop.w);
                assert_eq!(index, (4, 10), "each field crops its own rectangle");
                None
            }),
        });
        service.scan_sale_window();
        assert_eq!(
            *captures.lock().unwrap(),
            1,
            "one grab, then every field cropped from it"
        );

        // Now the real reads, keyed by call order.
        let order = Arc::new(Mutex::new(0usize));
        let service = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(calibrated),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new(move |_| {
                let mut index = order.lock().unwrap();
                let name = SALE_FIELDS[*index];
                *index += 1;
                Some((texts[name].to_string(), 0.95))
            }),
        });
        let result = service.scan_sale_window();
        assert_eq!(result["item_name"], "Shrapnel");
        assert_eq!(result["quantity"], 2754889.0);
        assert_eq!(result["tt_value"], 275.48);
        assert_eq!(result["auction_fee"], 64.51);
        assert_eq!(result["auction_days"], 6);
        assert_eq!(result["starting_bid"], 9276.0);
        assert_eq!(result["buyout"], 9276.0);
        assert_eq!(result["unread"], json!([]));
        assert_eq!(result["confidence"], 0.95);
        assert_eq!(result.get("error"), None);
    }

    #[test]
    fn a_field_below_the_confidence_floor_is_left_empty() {
        let service = SaleWindowOcrService::new(reading("275.48 PED", 0.2));
        let result = service.scan_sale_window();
        assert_eq!(
            result["unread"].as_array().unwrap().len(),
            SALE_FIELDS.len(),
            "a low-confidence read fills nothing"
        );
        assert_eq!(result["confidence"], Value::Null);
        assert_eq!(result.get("tt_value"), None);
        assert_eq!(result.get("error"), None, "a refusal is not an error");
    }

    #[test]
    fn implausible_values_are_refused_per_field() {
        // Text that parses everywhere, but is only plausible in some
        // fields: zero is a real bid and a real fee, not a real quantity
        // or duration.
        let result = SaleWindowOcrService::new(reading("0", 0.9)).scan_sale_window();
        assert_eq!(result["starting_bid"], 0.0);
        assert_eq!(result["buyout"], 0.0);
        assert_eq!(result["auction_fee"], 0.0);
        assert_eq!(result.get("quantity"), None);
        assert_eq!(result.get("auction_days"), None);
        let unread = result["unread"].as_array().unwrap();
        assert!(unread.contains(&json!("quantity")));
        assert!(unread.contains(&json!("auction_days")));

        // A duration longer than the window offers is a misread.
        let result = SaleWindowOcrService::new(reading("60", 0.9)).scan_sale_window();
        assert_eq!(result.get("auction_days"), None);
        assert_eq!(result["quantity"], 60.0);

        // Text with no digits leaves every number empty, while the name
        // takes it, that being what a name is.
        let result = SaleWindowOcrService::new(reading("Shrapnel", 0.9)).scan_sale_window();
        assert_eq!(result["item_name"], "Shrapnel");
        assert_eq!(result["unread"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn the_lowest_confidence_of_the_fields_that_read_is_reported() {
        let order = Arc::new(Mutex::new(0usize));
        let service = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(calibrated),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new(move |_| {
                let mut index = order.lock().unwrap();
                // The third field reads worst but still clears the floor;
                // the fourth is below it and must not drag the figure down,
                // because it contributes no value to be doubted.
                let confidence = match *index {
                    2 => 0.61,
                    3 => 0.10,
                    _ => 0.99,
                };
                *index += 1;
                Some(("5".to_string(), confidence))
            }),
        });
        let result = service.scan_sale_window();
        assert_eq!(result["confidence"], 0.61);
        assert_eq!(result["unread"], json!(["auction_fee"]));
    }

    #[test]
    fn an_uncalibrated_field_is_unread_rather_than_guessed() {
        let mut anchor = calibrated();
        anchor.cells.retain(|(name, _)| name != "buyout");
        let service = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(move || anchor.clone()),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new(|_| Some(("12".to_string(), 0.9))),
        });
        let result = service.scan_sale_window();
        assert_eq!(result["unread"], json!(["buyout"]));
        assert_eq!(result["starting_bid"], 12.0);
    }

    #[test]
    fn a_capture_of_the_wrong_panel_is_refused_whole() {
        let mut anchor = calibrated();
        anchor.cells.push(cell(LANDMARK_FIELD, 0, 35));

        // The landmark reads as something else: the window is not where the
        // calibration says, so nothing cropped from it means anything.
        let elsewhere = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new({
                let anchor = anchor.clone();
                move || anchor.clone()
            }),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new(|_| Some(("Repair cost".to_string(), 0.99))),
        });
        let result = elsewhere.scan_sale_window();
        assert!(result["error"]
            .as_str()
            .unwrap()
            .starts_with("That is not the sale window"));
        assert_eq!(
            result.get("quantity"),
            None,
            "no field survives a failed landmark, however well it read"
        );

        // The landmark reads as itself: the fields are trusted as before.
        // Recognition is imperfect on a label too, so a near miss passes.
        let found = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(|| Some(([0, 0], [40, 40]))),
            anchor: Arc::new(move || anchor.clone()),
            capture_region: Arc::new(|_, _, _, _| Some(panel())),
            read_text: Arc::new({
                // The landmark is read before any field, so call order is
                // what tells them apart here.
                let call = Mutex::new(0usize);
                move |_: &BgrImage| {
                    let mut call = call.lock().unwrap();
                    *call += 1;
                    // A near miss on the label still passes: recognition is
                    // no more exact on a word than on a number.
                    let text = if *call == 1 { "ltem Name" } else { "12" };
                    Some((text.to_string(), 0.99))
                }
            }),
        });
        let result = found.scan_sale_window();
        assert_eq!(result.get("error"), None);
        assert_eq!(result["starting_bid"], 12.0, "the fields read as normal");
    }

    #[test]
    fn a_calibration_without_a_landmark_still_reads() {
        // The landmark arrived after the first calibrations did, so its
        // absence must not refuse work that was fine the day before.
        let result = SaleWindowOcrService::new(reading("12", 0.9)).scan_sale_window();
        assert_eq!(result.get("error"), None);
        assert_eq!(result["starting_bid"], 12.0);
    }

    #[test]
    fn a_successful_scan_taps_the_captured_panel() {
        let service = SaleWindowOcrService::new(reading("12", 0.9));
        let taps = Arc::new(Mutex::new(Vec::new()));
        let sink = taps.clone();
        service.set_capture_tap(Arc::new(move |panel, region, _frame| {
            sink.lock()
                .unwrap()
                .push((panel.to_string(), region.clone()));
        }));
        service.scan_sale_window();
        {
            let taps = taps.lock().unwrap();
            assert_eq!(taps.len(), 1);
            assert_eq!(taps[0].0, "sale_window");
            assert_eq!(taps[0].1, json!({"x": 0, "y": 0, "w": 40, "h": 40}));
        }
        service.clear_capture_tap();
        service.scan_sale_window();
        assert_eq!(taps.lock().unwrap().len(), 1, "a cleared tap stays silent");
    }
}
