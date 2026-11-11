//! The capture rectangle of the auction sale window: the pure geometry
//! core.
//!
//! The sale window is read from a fixed rectangle anchored to the game
//! window's bottom-right corner, with its fields at fixed offsets inside
//! it. Locating the live game window stays platform glue; the geometry
//! here takes the window rect as an argument so the maths is
//! host-independent.
//!
//! The rectangle and its fields load from the bundled
//! `panel_geometry.json`, the artefact the calibration tool writes. The
//! window has no shipped default: until it is calibrated its anchor is
//! empty and yields no region. An absent, unreadable or wrong-shape file
//! leaves it uncalibrated rather than failing.

use std::path::Path;

use serde_json::{json, Value};

/// One cell type's panel-relative rect, with per-row anchors for
/// interpolation: `y_top(r) = round(first_y_top + r * (last_y_top -
/// first_y_top) / (n_rows - 1))`, x extents and height uniform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellGeometry {
    pub x_left: i64,
    pub x_right: i64,
    pub first_y_top: i64,
    pub last_y_top: i64,
    pub height: i64,
}

/// A panel's dimensions and its offsets from the game window's
/// bottom-right corner, plus the panel-relative field geometry produced
/// by the calibration step (empty until calibration has run).
#[derive(Debug, Clone, PartialEq)]
pub struct PanelAnchor {
    pub width: i64,
    pub height: i64,
    pub right_offset: i64,
    pub bottom_offset: i64,
    pub n_rows: Option<i64>,
    pub cells: Vec<(String, CellGeometry)>,
}

impl PanelAnchor {
    /// An anchor with no rect and no fields: what an uncalibrated panel
    /// amounts to, and what a reader with no geometry to offer returns.
    pub fn empty() -> Self {
        Self::fallback(0, 0, 0, 0)
    }

    const fn fallback(width: i64, height: i64, right_offset: i64, bottom_offset: i64) -> Self {
        Self {
            width,
            height,
            right_offset,
            bottom_offset,
            n_rows: None,
            cells: Vec::new(),
        }
    }

    /// The field geometry as the geometry file carries it: `n_rows` plus
    /// a `cells` map of per-cell pixel extents.
    fn to_geom_value(&self) -> Value {
        let mut cells = serde_json::Map::new();
        for (name, cell) in &self.cells {
            cells.insert(
                name.clone(),
                json!({
                    "x_left": cell.x_left,
                    "x_right": cell.x_right,
                    "first_y_top": cell.first_y_top,
                    "last_y_top": cell.last_y_top,
                    "height": cell.height,
                }),
            );
        }
        json!({ "n_rows": self.n_rows, "cells": Value::Object(cells) })
    }

    /// Encode the whole anchor as one `panel_geometry.json` entry: the
    /// panel rect plus the grid geometry. The inverse of `build_anchor`
    /// over a complete entry, so what calibration writes is what the
    /// loader reads back.
    pub fn to_geometry_entry(&self) -> Value {
        let mut entry = self.to_geom_value();
        let object = entry.as_object_mut().expect("geometry entry object");
        object.insert("width".into(), json!(self.width));
        object.insert("height".into(), json!(self.height));
        object.insert("right_offset".into(), json!(self.right_offset));
        object.insert("bottom_offset".into(), json!(self.bottom_offset));
        if self.n_rows.is_none() {
            object.remove("n_rows");
        }
        entry
    }
}

impl CellGeometry {
    /// A single (non-repeating) cell as a panel-relative `x/y/w/h`
    /// rectangle: the sale window's fields are one rect each rather than
    /// a row band, which this shape expresses as a band of one row.
    pub fn single_rect(&self) -> (i64, i64, i64, i64) {
        (
            self.x_left,
            self.first_y_top,
            self.x_right - self.x_left,
            self.height,
        )
    }
}

/// The auction sale window carries no shipped constants: its rect is
/// whatever calibration recorded, and a degenerate fallback is the
/// honest stand-in until then. `compute_region` refuses a zero-sized
/// anchor, so an uncalibrated build yields no region and the read
/// refuses rather than capturing an arbitrary rectangle.
fn sale_window_fallback() -> PanelAnchor {
    PanelAnchor::fallback(0, 0, 0, 0)
}

/// Load `panel_geometry.json` if present and parseable; absence or an
/// unreadable file yields the empty mapping (the fallbacks then govern);
/// wrong-shape payloads also land here (see the module doc).
fn load_geometry(path: &Path) -> Value {
    if !path.exists() {
        return Value::Object(serde_json::Map::new());
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Object(serde_json::Map::new()))
}

fn parse_cell(entry: &Value) -> Option<CellGeometry> {
    let object = entry.as_object()?;
    if object.is_empty() {
        return None;
    }
    Some(CellGeometry {
        x_left: object.get("x_left")?.as_i64()?,
        x_right: object.get("x_right")?.as_i64()?,
        first_y_top: object.get("first_y_top")?.as_i64()?,
        last_y_top: object.get("last_y_top")?.as_i64()?,
        height: object.get("height")?.as_i64()?,
    })
}

/// Apply a JSON geometry entry on top of the panel-rect fallback: the
/// JSON carries `n_rows` and `cells`, and may override any of the four
/// panel-rect fields; each absent field keeps the fallback's value.
/// Absent or empty entries return the fallback unchanged.
fn build_anchor(entry: Option<&Value>, fallback: PanelAnchor) -> PanelAnchor {
    let Some(entry) = entry.filter(|e| e.as_object().is_some_and(|o| !o.is_empty())) else {
        return fallback;
    };
    let mut cells = Vec::new();
    if let Some(raw_cells) = entry.get("cells").and_then(Value::as_object) {
        for (cell_name, raw) in raw_cells {
            if let Some(parsed) = parse_cell(raw) {
                cells.push((cell_name.clone(), parsed));
            }
        }
    }
    let rect = |key: &str, default: i64| entry.get(key).and_then(Value::as_i64).unwrap_or(default);
    PanelAnchor {
        width: rect("width", fallback.width),
        height: rect("height", fallback.height),
        right_offset: rect("right_offset", fallback.right_offset),
        bottom_offset: rect("bottom_offset", fallback.bottom_offset),
        n_rows: entry.get("n_rows").and_then(Value::as_i64),
        cells,
    }
}

/// The panel anchors, built once from the geometry file beside the
/// snapshot data.
pub struct ScanPresets {
    pub sale_window: PanelAnchor,
}

impl ScanPresets {
    /// `geometry_path` points at `panel_geometry.json` (it need not
    /// exist; the fallbacks then govern).
    pub fn new(geometry_path: &Path) -> Self {
        let geometry = load_geometry(geometry_path);
        Self {
            sale_window: build_anchor(geometry.get(SALE_WINDOW_KEY), sale_window_fallback()),
        }
    }
}

/// The sale window's key in the geometry file, shared by the loader and
/// the calibration tool that writes the entry.
pub const SALE_WINDOW_KEY: &str = "sale_window";

/// The capture rect for a panel anchored to the window's bottom-right
/// corner, or None for a degenerate rect. `window` is the game
/// window's `(x, y, width, height)`.
pub fn compute_region(
    anchor: &PanelAnchor,
    window: (i64, i64, i64, i64),
) -> Option<([i64; 2], [i64; 2])> {
    let (win_x, win_y, win_w, win_h) = window;
    let br_x = win_x + win_w - anchor.right_offset;
    let br_y = win_y + win_h - anchor.bottom_offset;
    let tl_x = br_x - anchor.width;
    let tl_y = br_y - anchor.height;
    if br_x <= tl_x || br_y <= tl_y {
        return None;
    }
    Some(([tl_x, tl_y], [br_x, br_y]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A calibrated sale window, every field distinct.
    fn calibrated() -> Value {
        json!({
            "width": 400,
            "height": 220,
            "right_offset": 12,
            "bottom_offset": 40,
            "cells": {
                "quantity": {"x_left": 20, "x_right": 90, "first_y_top": 60, "last_y_top": 60, "height": 14},
            },
        })
    }

    fn presets_from(geometry: &Value) -> ScanPresets {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel_geometry.json");
        std::fs::write(&path, geometry.to_string()).unwrap();
        ScanPresets::new(&path)
    }

    #[test]
    fn without_a_geometry_file_the_sale_window_is_uncalibrated() {
        let presets = ScanPresets::new(Path::new("/nonexistent/panel_geometry.json"));
        // An uncalibrated sale window has no rect, so it yields no
        // region rather than an arbitrary one.
        assert_eq!(presets.sale_window, sale_window_fallback());
        assert!(compute_region(&presets.sale_window, (0, 0, 1920, 1080)).is_none());
    }

    #[test]
    fn a_geometry_entry_carries_the_whole_panel_rect() {
        let presets = presets_from(&json!({ "sale_window": calibrated() }));
        assert_eq!(presets.sale_window.width, 400);
        assert_eq!(presets.sale_window.height, 220);
        assert_eq!(presets.sale_window.right_offset, 12);
        assert_eq!(presets.sale_window.bottom_offset, 40);
        assert_eq!(presets.sale_window.n_rows, None);
        assert_eq!(presets.sale_window.cells.len(), 1);
        assert_eq!(
            presets.sale_window.cells[0].1.single_rect(),
            (20, 60, 70, 14)
        );
    }

    #[test]
    fn an_entry_written_from_an_anchor_reads_back_as_that_anchor() {
        let anchor = PanelAnchor {
            width: 400,
            height: 220,
            right_offset: 12,
            bottom_offset: 40,
            n_rows: None,
            cells: vec![(
                "item_name".to_string(),
                CellGeometry {
                    x_left: 20,
                    x_right: 300,
                    first_y_top: 18,
                    last_y_top: 18,
                    height: 16,
                },
            )],
        };
        let entry = anchor.to_geometry_entry();
        assert_eq!(entry.get("n_rows"), None, "a non-grid panel omits n_rows");
        // Round-trip over a fallback that shares none of its values, so
        // every field demonstrably comes from the entry.
        let read_back = build_anchor(Some(&entry), sale_window_fallback());
        assert_eq!(read_back, anchor);
    }

    #[test]
    fn unreadable_geometry_leaves_the_sale_window_uncalibrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel_geometry.json");
        std::fs::write(&path, "{not json").unwrap();
        let presets = ScanPresets::new(&path);
        assert_eq!(presets.sale_window, sale_window_fallback());
    }

    #[test]
    fn broken_and_empty_cells_are_skipped() {
        let mut entry = calibrated();
        entry["cells"]["broken"] = json!({"x_left": 1});
        entry["cells"]["empty"] = json!({});
        let presets = presets_from(&json!({ "sale_window": entry }));
        assert_eq!(presets.sale_window.cells.len(), 1);
        assert_eq!(presets.sale_window.cells[0].0, "quantity");
        // An empty entry returns the fallback unchanged.
        let presets = presets_from(&json!({ "sale_window": {} }));
        assert_eq!(presets.sale_window, sale_window_fallback());
    }

    #[test]
    fn regions_anchor_to_the_bottom_right_corner() {
        let presets = presets_from(&json!({ "sale_window": calibrated() }));
        // Window at (100, 50) sized 1920x1080: offsets 12/40 put the
        // bottom-right at (2008, 1090), and the 400x220 rect above it.
        let region = compute_region(&presets.sale_window, (100, 50, 1920, 1080)).unwrap();
        assert_eq!(region, ([1608, 870], [2008, 1090]));

        // A window smaller than the panel still yields a rect (negative
        // coordinates and all): the guard fires only for non-positive
        // anchor sizes, where the corners collapse.
        let region = compute_region(&presets.sale_window, (0, 0, 10, 10)).unwrap();
        assert_eq!(region, ([-402, -250], [-2, -30]));
        let zero_anchor = PanelAnchor::fallback(0, 17, 48, 86);
        assert!(compute_region(&zero_anchor, (0, 0, 800, 600)).is_none());
        let flat_anchor = PanelAnchor::fallback(50, 0, 48, 86);
        assert!(compute_region(&flat_anchor, (0, 0, 800, 600)).is_none());
    }
}
