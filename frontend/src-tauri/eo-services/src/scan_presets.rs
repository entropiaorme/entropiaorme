//! Capture regions for skill / profession scans, ported from
//! `backend/services/scan_presets.py`: the pure geometry core.
//!
//! The user docks the relevant in-game panel in the bottom-right
//! corner at default UI scale; the panel pixel size is fixed by the
//! game client regardless of window resolution, so the only variable
//! is where the window's bottom-right corner sits. The capture rect
//! anchors to that corner through these constants. Locating the live
//! game window stays platform glue; the geometry here takes the
//! window rect as an argument so the maths is host-independent.
//!
//! Panel-relative grid geometry (row band + column splits) loads from
//! `backend/data/panel_geometry.json` when present, falling back to
//! panel-anchor-only constants otherwise; an unreadable file falls
//! back (the backend also logs a warning; this crate has no logging
//! surface yet). The file is an optional calibration artefact, unlike
//! the snapshot catalogue whose absence is a hard fault, and a
//! wrong-shape payload also falls back where the backend would crash
//! at import: the divergence register covers the strict typed reads.

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

/// Bottom-right docked panel dimensions at default UI scale, plus the
/// panel-relative grid geometry produced by the calibration step
/// (empty until calibration has run).
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

    /// Encode the grid geometry as the JSON `skill_panel::read_skill_panel`
    /// (and `slice_panel_cells`) consume: `n_rows` plus a `cells` map of
    /// per-cell pixel extents. Built from this merged anchor (the
    /// calibration file applied over the fallback), so an uncalibrated
    /// anchor yields `{"n_rows": null, "cells": {}}` and the reader returns
    /// no rows, exactly as the Python reference skips extraction without calibration.
    pub fn to_geom_value(&self) -> Value {
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
}

fn skill_fallback() -> PanelAnchor {
    PanelAnchor::fallback(635, 331, 30, 170)
}

fn profession_fallback() -> PanelAnchor {
    PanelAnchor::fallback(474, 293, 31, 161)
}

fn repair_fallback() -> PanelAnchor {
    PanelAnchor::fallback(50, 17, 48, 86)
}

/// Load `panel_geometry.json` if present and parseable; absence or an
/// unreadable file yields the empty mapping (fallback constants then
/// govern), matching the backend's fall-back for unreadable files;
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

/// Apply a JSON grid-geometry entry on top of the panel-rect fallback:
/// the panel rect always comes from the fallback, the JSON carries
/// `n_rows` and `cells`; absent or empty entries return the fallback
/// unchanged.
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
    PanelAnchor {
        width: fallback.width,
        height: fallback.height,
        right_offset: fallback.right_offset,
        bottom_offset: fallback.bottom_offset,
        n_rows: entry.get("n_rows").and_then(Value::as_i64),
        cells,
    }
}

/// The three panel anchors, built once from the geometry file beside
/// the snapshot data (the repair anchor takes no grid geometry).
pub struct ScanPresets {
    pub skill: PanelAnchor,
    pub profession: PanelAnchor,
    pub repair: PanelAnchor,
}

impl ScanPresets {
    /// `geometry_path` points at `panel_geometry.json` (it need not
    /// exist; the fallbacks then govern).
    pub fn new(geometry_path: &Path) -> Self {
        let geometry = load_geometry(geometry_path);
        Self {
            skill: build_anchor(geometry.get("skill"), skill_fallback()),
            profession: build_anchor(geometry.get("profession"), profession_fallback()),
            repair: repair_fallback(),
        }
    }
}

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

    #[test]
    fn to_geom_value_encodes_the_calibrated_grid_for_the_reader() {
        let entry = json!({
            "n_rows": 12,
            "cells": {
                "name": {"x_left": 5, "x_right": 100, "first_y_top": 10, "last_y_top": 300, "height": 18},
                "level": {"x_left": 105, "x_right": 140, "first_y_top": 10, "last_y_top": 300, "height": 18},
            }
        });
        let geom = build_anchor(Some(&entry), skill_fallback()).to_geom_value();
        assert_eq!(geom["n_rows"], 12);
        assert_eq!(geom["cells"]["name"]["x_left"], 5);
        assert_eq!(geom["cells"]["name"]["x_right"], 100);
        assert_eq!(geom["cells"]["level"]["height"], 18);
    }

    #[test]
    fn to_geom_value_of_an_uncalibrated_anchor_carries_no_cells() {
        let geom = skill_fallback().to_geom_value();
        assert_eq!(geom["n_rows"], Value::Null);
        assert_eq!(geom["cells"], json!({}));
    }

    #[test]
    fn fallback_constants_govern_without_a_geometry_file() {
        let presets = ScanPresets::new(Path::new("/nonexistent/panel_geometry.json"));
        assert_eq!(presets.skill, skill_fallback());
        assert_eq!(presets.profession, profession_fallback());
        assert_eq!(presets.repair, repair_fallback());
        assert_eq!(presets.skill.width, 635);
        assert_eq!(presets.profession.bottom_offset, 161);
        assert_eq!(presets.repair.height, 17);
    }

    #[test]
    fn unreadable_geometry_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel_geometry.json");
        std::fs::write(&path, "{not json").unwrap();
        let presets = ScanPresets::new(&path);
        assert_eq!(presets.skill, skill_fallback());
    }

    #[test]
    fn geometry_entries_overlay_rows_and_cells_on_the_fallback_rect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel_geometry.json");
        std::fs::write(
            &path,
            json!({
                "skill": {
                    "n_rows": 12,
                    "cells": {
                        "name": {"x_left": 10, "x_right": 200, "first_y_top": 5, "last_y_top": 300, "height": 20},
                        "broken": {"x_left": 1},
                        "empty": {},
                    },
                },
                "profession": {},
            })
            .to_string(),
        )
        .unwrap();
        let presets = ScanPresets::new(&path);
        // The panel rect stays the fallback's.
        assert_eq!(presets.skill.width, 635);
        assert_eq!(presets.skill.right_offset, 30);
        assert_eq!(presets.skill.n_rows, Some(12));
        assert_eq!(presets.skill.cells.len(), 1, "broken and empty cells skip");
        let (name, cell) = &presets.skill.cells[0];
        assert_eq!(name, "name");
        assert_eq!(cell.x_right, 200);
        assert_eq!(cell.height, 20);
        // An empty entry returns the fallback unchanged.
        assert_eq!(presets.profession, profession_fallback());
    }

    #[test]
    fn regions_anchor_to_the_bottom_right_corner() {
        let presets = ScanPresets::new(Path::new("/nonexistent/panel_geometry.json"));
        // Window at (100, 50) sized 1920x1080: skill anchor 30/170
        // offsets -> br (1990, 960), tl (1355, 629).
        let region = compute_region(&presets.skill, (100, 50, 1920, 1080)).unwrap();
        assert_eq!(region, ([1355, 629], [1990, 960]));

        let region = compute_region(&presets.repair, (0, 0, 800, 600)).unwrap();
        assert_eq!(region, ([702, 497], [752, 514]));

        // A window smaller than the panel still yields a rect (negative
        // coordinates and all): the guard fires only for non-positive
        // anchor sizes, where the corners collapse.
        let region = compute_region(&presets.skill, (0, 0, 10, 10)).unwrap();
        assert_eq!(region, ([-655, -491], [-20, -160]));
        let zero_anchor = PanelAnchor::fallback(0, 17, 48, 86);
        assert!(compute_region(&zero_anchor, (0, 0, 800, 600)).is_none());
        let flat_anchor = PanelAnchor::fallback(50, 0, 48, 86);
        assert!(compute_region(&flat_anchor, (0, 0, 800, 600)).is_none());
    }
}
