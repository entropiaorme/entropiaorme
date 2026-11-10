//! Repair cost OCR: a
//! one-shot screen read of the in-game repair terminal's total cost.
//! The capture region derives at scan time from the live game window
//! (the user docks the terminal bottom-right at default interface
//! scale; the bundled anchor encodes the cost rectangle relative to
//! that corner), and the shared recogniser reads the number.
//!
//! The window lookup, the capture, and the recogniser arrive as
//! injected providers, mirroring the manual scan's seams.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::game_ui::seams::{FrameReader, RegionCapture, RegionLookup};
use crate::skill_panel::BgrImage;

/// The capture observer (the recording controller's seam): called as
/// `tap(panel, region, frame)` after a successful grab.
pub type RepairCaptureTap = Arc<dyn Fn(&str, &Value, &BgrImage) + Send + Sync>;

/// The provider seams the composition root wires in.
pub struct RepairProviders {
    /// The repair-terminal region from the live game window.
    pub repair_region: RegionLookup,
    /// The bottom-right-docked Trade Terminal total-value region.
    pub trade_terminal_region: RegionLookup,
    /// False while the provisional fallback rectangle is in use.
    pub trade_terminal_calibrated: bool,
    /// Capture an `x/y/w/h` screen rectangle as BGR pixels.
    pub capture_region: RegionCapture,
    /// Recognise one frame.
    pub read_text: FrameReader,
}

impl Default for RepairProviders {
    fn default() -> Self {
        Self {
            repair_region: Arc::new(|| None),
            trade_terminal_region: Arc::new(|| None),
            trade_terminal_calibrated: false,
            capture_region: Arc::new(|_, _, _, _| None),
            read_text: Arc::new(|_| None),
        }
    }
}

/// One-shot OCR for the repair terminal cost number.
pub struct RepairOcrService {
    providers: RepairProviders,
    capture_tap: Mutex<Option<RepairCaptureTap>>,
}

impl RepairOcrService {
    pub fn new(providers: RepairProviders) -> Self {
        Self {
            providers,
            capture_tap: Mutex::new(None),
        }
    }

    /// Install a capture observer (called after a successful grab).
    pub fn set_capture_tap(&self, tap: RepairCaptureTap) {
        *self.capture_tap.lock().expect("capture tap") = Some(tap);
    }

    /// Remove the capture observer.
    pub fn clear_capture_tap(&self) {
        *self.capture_tap.lock().expect("capture tap") = None;
    }

    /// Capture and recognise the repair cost region:
    /// `{cost_ped, raw_text, confidence}`, with the original's error
    /// surface on each failure leg.
    pub fn scan_repair_cost(&self) -> Value {
        self.scan_number("repair", &self.providers.repair_region, true)
    }

    /// Capture and recognise the Trade Terminal's total TT value. The
    /// result carries whether its region is calibrated so the UI cannot
    /// present a provisional rectangle as trusted evidence.
    pub fn scan_trade_terminal_value(&self) -> Value {
        self.scan_number(
            "trade_terminal",
            &self.providers.trade_terminal_region,
            self.providers.trade_terminal_calibrated,
        )
    }

    fn scan_number(&self, panel: &str, region_lookup: &RegionLookup, calibrated: bool) -> Value {
        let failure = |error: &str| {
            json!({
                "error": error,
                "cost_ped": 0.0,
                "raw_text": "",
                "confidence": 0.0,
                "calibrated": calibrated,
            })
        };
        let Some((tl, br)) = region_lookup() else {
            return failure("Entropia Universe window not found: start the game first");
        };
        let (x, y) = (tl[0], tl[1]);
        let (w, h) = (br[0] - tl[0], br[1] - tl[1]);
        if w <= 0 || h <= 0 {
            return failure("Invalid region");
        }
        let Some(frame) = (self.providers.capture_region)(x, y, w, h) else {
            return failure("Capture failed");
        };

        let tap = self.capture_tap.lock().expect("capture tap").clone();
        if let Some(tap) = tap {
            let region = json!({"x": x, "y": y, "w": w, "h": h});
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tap(panel, &region, &frame)
            }));
        }

        let Some((text, confidence)) = (self.providers.read_text)(&frame) else {
            return failure("Local OCR engine unavailable");
        };
        let cost = parse_cost(&text);
        json!({
            "cost_ped": cost,
            "raw_text": text,
            "confidence": confidence,
            "calibrated": calibrated,
        })
    }
}

pub use crate::ocr_text::parse_cost;

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> BgrImage {
        BgrImage {
            data: vec![0; 12],
            h: 2,
            w: 2,
        }
    }

    #[test]
    fn the_scan_surfaces_each_failure_leg_verbatim() {
        let service = RepairOcrService::new(RepairProviders::default());
        assert_eq!(
            service.scan_repair_cost()["error"],
            "Entropia Universe window not found: start the game first"
        );

        let service = RepairOcrService::new(RepairProviders {
            repair_region: Arc::new(|| Some(([10, 10], [10, 30]))),
            ..Default::default()
        });
        assert_eq!(service.scan_repair_cost()["error"], "Invalid region");

        let service = RepairOcrService::new(RepairProviders {
            repair_region: Arc::new(|| Some(([10, 10], [40, 30]))),
            ..Default::default()
        });
        assert_eq!(service.scan_repair_cost()["error"], "Capture failed");

        let service = RepairOcrService::new(RepairProviders {
            repair_region: Arc::new(|| Some(([10, 10], [40, 30]))),
            capture_region: Arc::new(|_, _, _, _| Some(frame())),
            ..Default::default()
        });
        assert_eq!(
            service.scan_repair_cost()["error"],
            "Local OCR engine unavailable"
        );
    }

    #[test]
    fn a_successful_scan_parses_and_taps() {
        let service = RepairOcrService::new(RepairProviders {
            repair_region: Arc::new(|| Some(([10, 20], [110, 60]))),
            capture_region: Arc::new(|x, y, w, h| {
                assert_eq!((x, y, w, h), (10, 20, 100, 40));
                Some(frame())
            }),
            read_text: Arc::new(|_| Some(("2,20 PED".to_string(), 0.97))),
            ..Default::default()
        });

        let taps = Arc::new(Mutex::new(Vec::new()));
        let sink = taps.clone();
        service.set_capture_tap(Arc::new(move |panel, region, _frame| {
            sink.lock()
                .unwrap()
                .push((panel.to_string(), region.clone()));
        }));

        let result = service.scan_repair_cost();
        assert_eq!(result["cost_ped"], 2.2);
        assert_eq!(result["raw_text"], "2,20 PED");
        assert_eq!(result["confidence"], 0.97);
        assert_eq!(result.get("error"), None);
        {
            let taps = taps.lock().unwrap();
            assert_eq!(taps.len(), 1);
            assert_eq!(taps[0].0, "repair");
            assert_eq!(taps[0].1, json!({"x": 10, "y": 20, "w": 100, "h": 40}));
        }

        service.clear_capture_tap();
        service.scan_repair_cost();
        assert_eq!(taps.lock().unwrap().len(), 1, "a cleared tap stays silent");
    }
}
