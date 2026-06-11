//! Repair cost OCR, ported from `backend/services/repair_ocr.py`: a
//! one-shot screen read of the in-game repair terminal's total cost.
//! The capture region derives at scan time from the live game window
//! (the user docks the terminal bottom-right at default interface
//! scale; the bundled anchor encodes the cost rectangle relative to
//! that corner), and the shared recogniser reads the number.
//!
//! The window lookup, the capture, and the recogniser arrive as
//! injected providers, mirroring the manual scan's seams; the
//! original's logging is omitted.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::skill_panel::{digit_value, BgrImage};

/// The capture observer (the recording controller's seam): called as
/// `tap(panel, region, frame)` after a successful grab.
pub type RepairCaptureTap = Arc<dyn Fn(&str, &Value, &BgrImage) + Send + Sync>;

/// The window-region lookup seam: the terminal's corners when found.
pub type RegionLookup = Arc<dyn Fn() -> Option<([i64; 2], [i64; 2])> + Send + Sync>;

/// The screen-capture seam: an `x/y/w/h` rectangle as BGR pixels.
pub type RegionCapture = Arc<dyn Fn(i64, i64, i64, i64) -> Option<BgrImage> + Send + Sync>;

/// The recognition seam: one frame to `(text, confidence)`, or the
/// engine's unavailability.
pub type FrameReader = Arc<dyn Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync>;

/// The provider seams the composition root wires in.
pub struct RepairProviders {
    /// The repair-terminal region from the live game window.
    pub repair_region: RegionLookup,
    /// Capture an `x/y/w/h` screen rectangle as BGR pixels.
    pub capture_region: RegionCapture,
    /// Recognise one frame.
    pub read_text: FrameReader,
}

impl Default for RepairProviders {
    fn default() -> Self {
        Self {
            repair_region: Arc::new(|| None),
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
        let failure = |error: &str| {
            json!({
                "error": error,
                "cost_ped": 0.0,
                "raw_text": "",
                "confidence": 0.0,
            })
        };
        let Some((tl, br)) = (self.providers.repair_region)() else {
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
                tap("repair", &region, &frame)
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
        })
    }
}

/// Extract a PED cost number from OCR text: commas read as decimal
/// points, spaces drop, and the first digit run (with an optional
/// fraction) parses. The original's digit class and float conversion
/// are Unicode-wide, converting fullwidth digits by value; the
/// recogniser's alphabet carries exactly the ASCII and fullwidth
/// forms, which `digit_value` covers.
pub fn parse_cost(text: &str) -> f64 {
    let cleaned: String = text.replace(',', ".").replace(' ', "");
    let chars: Vec<char> = cleaned.chars().collect();
    let Some(start) = chars.iter().position(|ch| digit_value(*ch).is_some()) else {
        return 0.0;
    };
    let mut number = String::new();
    let mut seen_dot = false;
    for &ch in &chars[start..] {
        if let Some(value) = digit_value(ch) {
            number.push(char::from_digit(value, 10).expect("decimal digit"));
        } else if ch == '.' && !seen_dot {
            // The optional fraction: one dot, then digits only.
            seen_dot = true;
            number.push(ch);
        } else {
            break;
        }
    }
    number.parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_parse_through_comma_space_and_noise() {
        assert_eq!(parse_cost("12.50"), 12.5);
        assert_eq!(parse_cost("12,50"), 12.5);
        assert_eq!(parse_cost("1 234,5"), 1234.5);
        assert_eq!(parse_cost("PED 0.05"), 0.05);
        assert_eq!(parse_cost("cost: 2.20 PED"), 2.2);
        assert_eq!(parse_cost("12."), 12.0);
        assert_eq!(parse_cost("1.2.3"), 1.2);
        assert_eq!(parse_cost("no digits"), 0.0);
        assert_eq!(parse_cost(""), 0.0);
        assert_eq!(parse_cost(".5"), 5.0);
        assert_eq!(parse_cost("0,0,7"), 0.0);
        // Fullwidth digits convert by value (the original's float()
        // accepts them); a fullwidth decimal point is not a fraction
        // dot, so the run ends there, both exactly as the original.
        assert_eq!(parse_cost("\u{ff11}\u{ff12}.50"), 12.5);
        assert_eq!(parse_cost("1\u{ff12}3"), 123.0);
        assert_eq!(parse_cost("\u{ff11}\u{ff12}\u{ff0e}50"), 12.0);
    }

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

        let mut providers = RepairProviders::default();
        providers.repair_region = Arc::new(|| Some(([10, 10], [10, 30])));
        let service = RepairOcrService::new(providers);
        assert_eq!(service.scan_repair_cost()["error"], "Invalid region");

        let mut providers = RepairProviders::default();
        providers.repair_region = Arc::new(|| Some(([10, 10], [40, 30])));
        let service = RepairOcrService::new(providers);
        assert_eq!(service.scan_repair_cost()["error"], "Capture failed");

        let mut providers = RepairProviders::default();
        providers.repair_region = Arc::new(|| Some(([10, 10], [40, 30])));
        providers.capture_region = Arc::new(|_, _, _, _| Some(frame()));
        let service = RepairOcrService::new(providers);
        assert_eq!(
            service.scan_repair_cost()["error"],
            "Local OCR engine unavailable"
        );
    }

    #[test]
    fn a_successful_scan_parses_and_taps() {
        let mut providers = RepairProviders::default();
        providers.repair_region = Arc::new(|| Some(([10, 20], [110, 60])));
        providers.capture_region = Arc::new(|x, y, w, h| {
            assert_eq!((x, y, w, h), (10, 20, 100, 40));
            Some(frame())
        });
        providers.read_text = Arc::new(|_| Some(("2,20 PED".to_string(), 0.97)));
        let service = RepairOcrService::new(providers);

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
