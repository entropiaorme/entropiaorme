//! Skill-panel reading, ported from the parsing half of
//! `backend/services/skill_panel_parse.py` and the orchestration in
//! `backend/services/local_ocr.py`: the calibrated cell-slicing grid,
//! the integer level parse, the bar fill-ratio estimate, and the
//! fuzzy name resolution against the canonical vocabulary.
//!
//! `fuzzy_resolve` applies a minimum-score floor (`FUZZY_SCORE_FLOOR`):
//! below it the OCR text resembles no known skill, so it is left
//! unresolved rather than force-matched to its nearest entry (a
//! confident wrong label would silently corrupt skill tracking, whereas
//! a drop is recoverable by re-scanning). The floor is kept
//! byte-identical with the Python parser. It deliberately does not chase
//! same-band cross-skill matches of a skill ABSENT from the vocabulary
//! (a vocabulary-completeness matter, not a floor one).
//!
//! The text recogniser arrives as an injected reader, so this module
//! stays hermetically testable and the engine wiring lives with the
//! composition root. The original's low-confidence warning is a log
//! line and is omitted with the rest of the logging surface.

use serde_json::Value;

use crate::fuzzy_match::extract_top;
use crate::mob_lookup_service::python_whitespace;

/// One BGR HWC image buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct BgrImage {
    pub data: Vec<u8>,
    pub h: usize,
    pub w: usize,
}

impl BgrImage {
    /// The rectangle `[y0, y1) x [x0, x1)`, clamped into bounds the
    /// way the original's array slicing is for overruns. (A negative
    /// coordinate clamps to zero here where the original's slicing
    /// would wrap from the end; the calibrated geometry is all
    /// positive.)
    pub fn crop(&self, y0: i64, y1: i64, x0: i64, x1: i64) -> BgrImage {
        let y0 = y0.clamp(0, self.h as i64) as usize;
        let y1 = y1.clamp(y0 as i64, self.h as i64) as usize;
        let x0 = x0.clamp(0, self.w as i64) as usize;
        let x1 = x1.clamp(x0 as i64, self.w as i64) as usize;
        let (ch, cw) = (y1 - y0, x1 - x0);
        let mut data = Vec::with_capacity(ch * cw * 3);
        for y in y0..y1 {
            let start = (y * self.w + x0) * 3;
            data.extend_from_slice(&self.data[start..start + cw * 3]);
        }
        BgrImage { data, h: ch, w: cw }
    }
}

/// One sliced cell crop.
#[derive(Debug, Clone)]
pub struct CellCrop {
    pub row: usize,
    pub cell: String,
    pub image: BgrImage,
}

/// The decimal value of a digit character the recogniser can emit:
/// the original's digit class is Unicode-wide and its number parsing
/// converts such digits by value, and the decode alphabet carries
/// exactly the ASCII and fullwidth forms.
pub(crate) fn digit_value(ch: char) -> Option<u32> {
    match ch {
        '0'..='9' => Some(ch as u32 - '0' as u32),
        '\u{ff10}'..='\u{ff19}' => Some(ch as u32 - 0xff10),
        _ => None,
    }
}

/// Read the first integer run from a level cell's OCR text.
pub fn parse_level(text: &str) -> Option<i64> {
    let digits: String = text
        .chars()
        .skip_while(|c| digit_value(*c).is_none())
        .map_while(digit_value)
        .map(|value| char::from_digit(value, 10).expect("decimal digit"))
        .collect();
    digits.parse().ok()
}

/// Estimate the fractional fill in [0, 1) of a skill bar crop:
/// per-column mean luminance, threshold at the midpoint of the
/// column-mean range, rightmost bright column over width. A reading
/// of 1.0 is impossible mid-bar (the in-game bar would have just
/// levelled), so it always means the contrast-low fallback misread an
/// empty bar and flips to 0.0.
pub fn parse_bar_fill(crop: &BgrImage) -> f64 {
    if crop.data.is_empty() || crop.w == 0 || crop.h == 0 {
        return 0.0;
    }
    // The canonical fixed-point BGR -> grey conversion. Vendor
    // builds of the original's image library deviate from this by
    // one least-significant bit on a fraction of rounding-tie pixels;
    // sub-resolution for the fill estimate and accepted as the pinned
    // tolerance.
    let grey = |b: u8, g: u8, r: u8| -> u8 {
        ((b as u32 * 1868 + g as u32 * 9617 + r as u32 * 4899 + (1 << 13)) >> 14) as u8
    };
    let mut col_mean = vec![0.0f64; crop.w];
    for (x, mean) in col_mean.iter_mut().enumerate() {
        let mut sum = 0u64;
        for y in 0..crop.h {
            let i = (y * crop.w + x) * 3;
            sum += grey(crop.data[i], crop.data[i + 1], crop.data[i + 2]) as u64;
        }
        *mean = sum as f64 / crop.h as f64;
    }
    let lo = col_mean.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = col_mean.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if (hi - lo) < 15.0 {
        // Low contrast: no detectable fill edge (empty bars land
        // here), straight to zero.
        return 0.0;
    }
    let threshold = (lo + hi) / 2.0;
    let rightmost = col_mean
        .iter()
        .enumerate()
        .filter(|(_, mean)| **mean >= threshold)
        .map(|(index, _)| index)
        .next_back();
    let Some(rightmost) = rightmost else {
        return 0.0;
    };
    let fill = (rightmost + 1) as f64 / crop.w as f64;
    if fill >= 1.0 {
        return 0.0;
    }
    fill
}

/// Whitespace + case insensitive name key for tolerant matching.
fn norm_name(s: &str) -> String {
    s.chars()
        .filter(|c| !(c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(c)))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The minimum rapidfuzz WRatio a fuzzy candidate must score to be
/// accepted. 60 is the empirically-observed lower bound of a legitimate
/// match (a single-transposition typo of a PRESENT skill scores ~60);
/// below it the read resembles no known skill and is far likelier OCR
/// garbage than a real skill, so it is dropped rather than force-matched
/// to a confident wrong label. The floor is deliberately not raised to
/// chase same-band cross-skill force-matches of an ABSENT skill: a
/// global floor cannot separate those from legitimate typos, so they
/// belong to vocabulary completeness, not to discarding real reads.
/// MUST stay byte-identical with the Python parser's `_FUZZY_SCORE_FLOOR`.
const FUZZY_SCORE_FLOOR: f64 = 60.0;

/// Resolve an OCR name to its canonical vocabulary entry: exact match,
/// then the whitespace/case-collapsed match, then the fuzzy top-1 if it
/// scores at or above `FUZZY_SCORE_FLOOR` (a below-floor read is left
/// unresolved rather than force-matched to its nearest entry).
pub fn fuzzy_resolve(
    ocr_text: &str,
    vocab: &[String],
) -> (Option<String>, f64, Vec<(String, f64)>) {
    let cleaned = ocr_text.trim_matches(python_whitespace);
    if cleaned.is_empty() {
        return (None, 0.0, Vec::new());
    }
    if vocab.iter().any(|entry| entry == cleaned) {
        return (
            Some(cleaned.to_string()),
            100.0,
            vec![(cleaned.to_string(), 100.0)],
        );
    }
    let norm_query = norm_name(cleaned);
    for entry in vocab {
        if norm_name(entry) == norm_query {
            return (Some(entry.clone()), 100.0, vec![(entry.clone(), 100.0)]);
        }
    }
    let candidates: Vec<(String, f64)> = extract_top(cleaned, vocab, 3)
        .into_iter()
        .map(|(entry, score)| (entry.to_string(), score))
        .collect();
    match candidates.first() {
        Some((top, score)) if *score >= FUZZY_SCORE_FLOOR => {
            (Some(top.clone()), *score, candidates.clone())
        }
        // Below the floor: the read resembles no known skill, so leave it
        // unresolved (downstream drops None-name rows) rather than
        // force-match its nearest entry to a confident wrong label.
        Some((_, score)) => (None, *score, candidates.clone()),
        None => (None, 0.0, Vec::new()),
    }
}

/// Slice a captured panel into per-cell BGR crops via the calibrated
/// grid: rows top to bottom, then cells in geometry order, so callers
/// can group per row downstream. A missing geometry field panics the
/// way the original's lookup raises (the scan worker contains it and
/// surfaces the error); the committed geometry carries every field.
pub fn slice_panel_cells(panel: &BgrImage, geom: &Value) -> Vec<CellCrop> {
    let n_rows = geom
        .get("n_rows")
        .and_then(Value::as_i64)
        .expect("panel geometry: n_rows")
        .max(0) as usize;
    let cells = geom
        .get("cells")
        .and_then(Value::as_object)
        .expect("panel geometry: cells");
    let mut out = Vec::new();
    for r in 0..n_rows {
        for (cell_name, cell) in cells {
            let field = |key: &str| {
                cell.get(key)
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("panel geometry: {cell_name}.{key}"))
            };
            let first = field("first_y_top");
            let last = field("last_y_top");
            let y_top = if n_rows > 1 {
                // The original's round(): half to even.
                (first + r as f64 * (last - first) / (n_rows - 1) as f64).round_ties_even() as i64
            } else {
                first as i64
            };
            let y_bot = y_top + field("height") as i64;
            let crop = panel.crop(
                y_top,
                y_bot,
                field("x_left") as i64,
                field("x_right") as i64,
            );
            out.push(CellCrop {
                row: r,
                cell: cell_name.clone(),
                image: crop,
            });
        }
    }
    out
}

/// One read row of the skill panel.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRow {
    pub name: Option<String>,
    pub level: Option<f64>,
}

/// Read a skill panel: the integer from the level cell's OCR text,
/// the fractional part from the bar cell's fill ratio. Rows whose
/// name does not resolve still emit with `name: None`; the caller
/// decides their fate.
pub fn read_skill_panel(
    read_text: &dyn Fn(&BgrImage) -> (String, f64),
    panel: &BgrImage,
    geom: &Value,
    vocab: &[String],
) -> Vec<SkillRow> {
    struct RowState {
        name: Option<String>,
        int_level: Option<i64>,
        bar_fill: f64,
    }
    let crops = slice_panel_cells(panel, geom);
    let mut rows: std::collections::BTreeMap<usize, RowState> = std::collections::BTreeMap::new();
    for crop in crops {
        let row = rows.entry(crop.row).or_insert(RowState {
            name: None,
            int_level: None,
            bar_fill: 0.0,
        });
        if crop.cell == "bar" {
            row.bar_fill = parse_bar_fill(&crop.image);
            continue;
        }
        let (text, _conf) = read_text(&crop.image);
        if crop.cell == "name" {
            let (canonical, _score, _candidates) = fuzzy_resolve(&text, vocab);
            row.name = canonical;
        } else if crop.cell == "level" {
            row.int_level = parse_level(&text);
        }
    }
    rows.into_values()
        .map(|row| SkillRow {
            name: row.name,
            level: row.int_level.map(|int| int as f64 + row.bar_fill),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_parse_the_first_integer_run() {
        assert_eq!(parse_level("123"), Some(123));
        assert_eq!(parse_level(" lvl 42 / 100"), Some(42));
        assert_eq!(parse_level("no digits"), None);
        assert_eq!(parse_level(""), None);
        assert_eq!(parse_level("7a9"), Some(7));
        // The recogniser's fullwidth digits convert by value, mixed
        // runs included, exactly as the original's parsing does.
        assert_eq!(parse_level("\u{ff11}\u{ff12}"), Some(12));
        assert_eq!(parse_level("lvl \u{ff12}3"), Some(23));
    }

    fn bar(columns: &[u8], h: usize) -> BgrImage {
        let w = columns.len();
        let mut data = Vec::with_capacity(w * h * 3);
        for _ in 0..h {
            for &v in columns {
                data.extend_from_slice(&[v, v, v]);
            }
        }
        BgrImage { data, h, w }
    }

    #[test]
    fn bar_fill_thresholds_and_flips_full_to_empty() {
        // Half-filled: bright left half, dark right half.
        let mut columns = vec![200u8; 5];
        columns.extend(vec![20u8; 5]);
        let fill = parse_bar_fill(&bar(&columns, 4));
        assert!((fill - 0.5).abs() < 1e-9);

        // Uniform brightness: low contrast, no edge, zero.
        assert_eq!(parse_bar_fill(&bar(&[200u8; 10], 4)), 0.0);

        // Bright to the last column: the impossible 1.0 flips to 0.0.
        let mut columns = vec![20u8; 2];
        columns.extend(vec![200u8; 8]);
        // The rightmost bright column is the final one: fill 1.0
        // would be reported mid-bar, so it reads empty... but the
        // rightmost bright column here IS the last, so fill = 1.0
        // and the flip applies.
        assert_eq!(parse_bar_fill(&bar(&columns, 4)), 0.0);

        // Empty crop guards.
        assert_eq!(
            parse_bar_fill(&BgrImage {
                data: Vec::new(),
                h: 0,
                w: 0
            }),
            0.0
        );
    }

    fn colour_bar(columns: &[[u8; 3]], h: usize) -> BgrImage {
        let w = columns.len();
        let mut data = Vec::with_capacity(w * h * 3);
        for _ in 0..h {
            for c in columns {
                data.extend_from_slice(c);
            }
        }
        BgrImage { data, h, w }
    }

    #[test]
    fn bar_fill_reads_through_the_colour_conversion() {
        // Pure red columns convert to grey 76, pure blue to 29: the
        // fixed-point coefficients decide which side is bright.
        let mut columns = vec![[0u8, 0, 255]; 6];
        columns.extend(vec![[255u8, 0, 0]; 4]);
        let fill = parse_bar_fill(&colour_bar(&columns, 3));
        assert!((fill - 0.6).abs() < 1e-9, "red-bright fill: {fill}");

        // Green (150) against red (76): green is the bright side.
        let mut columns = vec![[0u8, 255, 0]; 3];
        columns.extend(vec![[0u8, 0, 255]; 7]);
        let fill = parse_bar_fill(&colour_bar(&columns, 3));
        assert!((fill - 0.3).abs() < 1e-9, "green-bright fill: {fill}");

        // A near-full bar reads its true ratio (only exactly-full
        // flips to empty).
        let mut columns = vec![[200u8, 200, 200]; 9];
        columns.push([10u8, 10, 10]);
        let fill = parse_bar_fill(&colour_bar(&columns, 2));
        assert!((fill - 0.9).abs() < 1e-9, "near-full fill: {fill}");
    }

    #[test]
    fn names_resolve_exact_collapsed_then_fuzzy_with_a_floor() {
        let vocab: Vec<String> = ["Whip", "Food Technology", "Combat Reflexes"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (name, score, _) = fuzzy_resolve("Whip", &vocab);
        assert_eq!(name.as_deref(), Some("Whip"));
        assert_eq!(score, 100.0);

        // Case + whitespace collapsed.
        let (name, score, _) = fuzzy_resolve("foodtechnology", &vocab);
        assert_eq!(name.as_deref(), Some("Food Technology"));
        assert_eq!(score, 100.0);
        let (name, _, _) = fuzzy_resolve(" combat  reflexes ", &vocab);
        assert_eq!(name.as_deref(), Some("Combat Reflexes"));

        // Fuzzy fallback.
        let (name, score, candidates) = fuzzy_resolve("Combat Reflexs", &vocab);
        assert_eq!(name.as_deref(), Some("Combat Reflexes"));
        assert!(score > 90.0);
        assert_eq!(candidates.len(), 3);

        // Above the floor, a high-scoring cross-skill match still
        // resolves: "Food Technology" shares the "Technology" token with
        // "Wood Technology" (WRatio 93.33). A global floor cannot
        // separate this from a legitimate typo; it is a
        // vocabulary-completeness residual (the real skill is absent from
        // this gappy vocab), addressed by refreshing the vocabulary, not
        // by the floor.
        let gappy: Vec<String> = ["Wood Technology", "Whip"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (name, score, _) = fuzzy_resolve("Food Technology", &gappy);
        assert_eq!(name.as_deref(), Some("Wood Technology"));
        assert!((score - 93.33333333333333).abs() < 1e-9);

        // Blank input resolves to nothing.
        let (name, score, candidates) = fuzzy_resolve("   ", &vocab);
        assert_eq!(name, None);
        assert_eq!(score, 0.0);
        assert!(candidates.is_empty());
        let (name, _, _) = fuzzy_resolve("x", &[]);
        assert_eq!(name, None);
    }

    #[test]
    fn fuzzy_resolve_floors_unknown_names_to_none() {
        // A read that resembles no known skill scores below the floor and
        // is left unresolved, rather than force-matched to its nearest
        // vocabulary entry. Downstream drops the None-name row.
        let vocab: Vec<String> = ["Wood Technology", "Whip", "Combat Reflexes"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (name, score, candidates) = fuzzy_resolve("qzxwv", &vocab);
        assert_eq!(name, None, "below-floor garbage is not force-matched");
        assert!(
            score < super::FUZZY_SCORE_FLOOR,
            "the rejected candidate scored {score}, expected below the floor"
        );
        // The candidate list is still surfaced (callers may inspect it),
        // even though the top scored below the floor.
        assert!(!candidates.is_empty());

        // A genuine typo of a PRESENT skill stays above the floor and
        // still resolves (the floor does not discard real reads).
        let (name, score, _) = fuzzy_resolve("Combat Reflexs", &vocab);
        assert_eq!(name.as_deref(), Some("Combat Reflexes"));
        assert!(score >= super::FUZZY_SCORE_FLOOR);
    }

    fn panel() -> BgrImage {
        // A 40x20 panel with distinct values per pixel row; the bar
        // band (x 15..19) carries a half-bright fill pattern instead.
        let (h, w) = (40usize, 20usize);
        let mut data = Vec::with_capacity(h * w * 3);
        for y in 0..h {
            for x in 0..w {
                let v = if x >= 15 {
                    if x < 17 {
                        220
                    } else {
                        10
                    }
                } else {
                    (y * 6) as u8
                };
                data.extend_from_slice(&[v, v, v]);
            }
        }
        BgrImage { data, h, w }
    }

    #[test]
    fn the_grid_slices_rows_with_banker_rounding() {
        let geom = json!({
            "n_rows": 3,
            "cells": {
                "name": {"x_left": 0, "x_right": 10, "first_y_top": 2,
                          "last_y_top": 31, "height": 4},
                "bar": {"x_left": 10, "x_right": 20, "first_y_top": 5,
                         "last_y_top": 34, "height": 2},
            },
        });
        let crops = slice_panel_cells(&panel(), &geom);
        assert_eq!(crops.len(), 6);
        // Row tops: 2, round(2 + 14.5) = round(16.5) = 16 (half to
        // even), 31.
        let names: Vec<&CellCrop> = crops.iter().filter(|c| c.cell == "name").collect();
        assert_eq!(names[0].image.data[0], 2 * 6);
        assert_eq!(names[1].image.data[0], 16 * 6);
        assert_eq!(names[2].image.data[0], 31 * 6);
        assert_eq!(names[0].image.h, 4);
        assert_eq!(names[0].image.w, 10);

        // A single-row grid sits at the first offset.
        let geom = json!({
            "n_rows": 1,
            "cells": {"name": {"x_left": 0, "x_right": 5, "first_y_top": 7,
                                "last_y_top": 30, "height": 3}},
        });
        let crops = slice_panel_cells(&panel(), &geom);
        assert_eq!(crops.len(), 1);
        assert_eq!(crops[0].image.data[0], 7 * 6);

        // Out-of-bounds rectangles clamp instead of panicking.
        let geom = json!({
            "n_rows": 1,
            "cells": {"name": {"x_left": 15, "x_right": 99, "first_y_top": 38,
                                "last_y_top": 38, "height": 10}},
        });
        let crops = slice_panel_cells(&panel(), &geom);
        assert_eq!(crops[0].image.w, 5);
        assert_eq!(crops[0].image.h, 2);
    }

    #[test]
    fn the_panel_reader_joins_names_levels_and_bars() {
        let geom = json!({
            "n_rows": 2,
            "cells": {
                "name": {"x_left": 0, "x_right": 10, "first_y_top": 0,
                          "last_y_top": 20, "height": 4},
                "level": {"x_left": 10, "x_right": 15, "first_y_top": 0,
                           "last_y_top": 20, "height": 4},
                "bar": {"x_left": 15, "x_right": 19, "first_y_top": 0,
                         "last_y_top": 20, "height": 4},
            },
        });
        let vocab: Vec<String> = ["Anatomy", "Rifle"].iter().map(|s| s.to_string()).collect();
        // Script the reader by crop geometry: names for the wide
        // cells, levels for the narrow ones.
        let reader = |crop: &BgrImage| -> (String, f64) {
            match (crop.w, crop.data[0] == 0) {
                (10, true) => ("Anatomy".to_string(), 0.99),
                (10, false) => ("Rifel".to_string(), 0.6),
                (5, true) => ("12".to_string(), 0.99),
                _ => ("no digits".to_string(), 0.2),
            }
        };
        let rows = read_skill_panel(&reader, &panel(), &geom, &vocab);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name.as_deref(), Some("Anatomy"));
        // The half-bright bar contributes its fill as the fractional
        // part on top of the OCR'd integer.
        assert_eq!(rows[0].level, Some(12.5));
        // The second row's name fuzzy-resolves; its level cell parses
        // nothing, so the level is None even with a bar present.
        assert_eq!(rows[1].name.as_deref(), Some("Rifle"));
        assert_eq!(rows[1].level, None);
    }
}
