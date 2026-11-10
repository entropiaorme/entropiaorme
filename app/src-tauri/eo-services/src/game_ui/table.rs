//! Slicing a table inside a window into rows and columns.
//!
//! The client rules its tables: a thin line under every row and
//! between columns. Those lines, not a remembered row height, are what
//! divide a table here, so a window dragged taller or wider, or drawn
//! at another interface scale, slices the same way.

use serde::Deserialize;

use super::geometry::{chroma, near, Pixels, Rect};

/// What a rule line looks like.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleProfile {
    /// Brightness range of a horizontal rule (a selected row's outline
    /// is brighter than a plain rule, so the range is wide).
    pub row_rule_lum: [u8; 2],
    /// Brightness range of a vertical rule.
    pub column_rule_lum: [u8; 2],
    /// Share of the scanned span that must be rule-coloured.
    pub coverage: f32,
    /// Pixels ignored at each end of a row scan, `[left, right]`: the
    /// window's border and its scrollbar.
    pub row_inset: [i32; 2],
    /// Rule lines this close together are one line.
    pub merge: i32,
    /// A row is between these heights, `[min, max]`.
    pub row_height: [i32; 2],
    /// Largest channel spread of a vertical rule's pixel: it is a grey.
    pub column_rule_max_chroma: u8,
    /// Pixels either side of a divider's measured position searched for
    /// it lower down the table: dividers in a table's header and in its
    /// rows can stand a pixel or two apart.
    pub divider_reach: i32,
}

/// The panel a table is drawn on: its fill, and how far a pixel may
/// stray from it and still be the fill.
#[derive(Debug, Clone, Copy)]
pub struct Panel {
    pub rgb: [u8; 3],
    pub tolerance: u8,
}

fn merged(mut lines: Vec<i32>, merge: i32) -> Vec<(i32, i32)> {
    lines.sort_unstable();
    let mut out: Vec<(i32, i32)> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some((_, end)) if line - *end <= merge => *end = line,
            _ => out.push((line, line)),
        }
    }
    out
}

/// Whether the pixel at `(x, y)` has a horizontal rule's colour.
fn row_rule_pixel<P: Pixels>(image: &P, x: i32, y: i32, panel: Panel, rules: &RuleProfile) -> bool {
    let l = image.lum(x, y);
    l >= rules.row_rule_lum[0]
        && l <= rules.row_rule_lum[1]
        && !near(image.rgb(x, y), panel.rgb, panel.tolerance)
}

/// Whether the pixel at `(x, y)` has a vertical rule's colour.
fn column_rule_pixel<P: Pixels>(image: &P, x: i32, y: i32, rules: &RuleProfile) -> bool {
    let l = image.lum(x, y);
    l >= rules.column_rule_lum[0]
        && l <= rules.column_rule_lum[1]
        && chroma(image.rgb(x, y)) <= rules.column_rule_max_chroma
}

/// Horizontal rule lines across `area`, as `(first, last)` pixel rows.
pub fn row_rules<P: Pixels>(
    image: &P,
    area: Rect,
    panel: Panel,
    rules: &RuleProfile,
) -> Vec<(i32, i32)> {
    let (x0, x1) = (area.x0 + rules.row_inset[0], area.x1 - rules.row_inset[1]);
    if x1 <= x0 {
        return Vec::new();
    }
    let need = (rules.coverage * (x1 - x0 + 1) as f32) as i32;
    let mut lines = Vec::new();
    for y in area.y0.max(0)..=area.y1.min(image.height() - 1) {
        let hit = (x0.max(0)..=x1.min(image.width() - 1))
            .filter(|&x| row_rule_pixel(image, x, y, panel, rules))
            .count() as i32;
        if hit >= need {
            lines.push(y);
        }
    }
    merged(lines, rules.merge)
}

/// How far the horizontal rule on pixel row `y` runs between `x0` and
/// `x1`: the longest stretch of rule-coloured pixels, bridging breaks no
/// longer than the profile's merge distance. A table's rules span the
/// table and stop at its edges, so this is the table's width wherever
/// the window has been resized to.
pub fn rule_extent<P: Pixels>(
    image: &P,
    y: i32,
    (x0, x1): (i32, i32),
    panel: Panel,
    rules: &RuleProfile,
) -> Option<(i32, i32)> {
    if y < 0 || y >= image.height() {
        return None;
    }
    let mut best: Option<(i32, i32)> = None;
    let mut current: Option<(i32, i32)> = None;
    for x in x0.max(0)..=x1.min(image.width() - 1) {
        if !row_rule_pixel(image, x, y, panel, rules) {
            continue;
        }
        current = match current {
            Some((start, end)) if x - end <= rules.merge + 1 => Some((start, x)),
            _ => Some((x, x)),
        };
        let (start, end) = current.expect("just set");
        if best.is_none_or(|(s, e)| end - start > e - s) {
            best = Some((start, end));
        }
    }
    best
}

/// The rows of a table: the bands between consecutive rule lines that
/// are a plausible row height. Each is `(top, bottom)`, inclusive, with
/// the rules themselves excluded.
pub fn rows<P: Pixels>(
    image: &P,
    area: Rect,
    panel: Panel,
    rules: &RuleProfile,
) -> Vec<(i32, i32)> {
    let lines = row_rules(image, area, panel, rules);
    lines
        .windows(2)
        .filter_map(|pair| {
            let (top, bottom) = (pair[0].1 + 1, pair[1].0 - 1);
            let height = bottom - top + 1;
            (height >= rules.row_height[0] && height <= rules.row_height[1])
                .then_some((top, bottom))
        })
        .collect()
}

/// Vertical rule lines running the height of `body`, as pixel columns.
pub fn column_rules<P: Pixels>(image: &P, body: Rect, rules: &RuleProfile) -> Vec<i32> {
    let (y0, y1) = (body.y0.max(0), body.y1.min(image.height() - 1));
    if y1 <= y0 {
        return Vec::new();
    }
    let need = (rules.coverage * (y1 - y0 + 1) as f32) as i32;
    let mut lines = Vec::new();
    for x in body.x0.max(0)..=body.x1.min(image.width() - 1) {
        let mut hit = 0;
        for y in y0..=y1 {
            if column_rule_pixel(image, x, y, rules) {
                hit += 1;
            }
        }
        if hit >= need {
            lines.push(x);
        }
    }
    merged(lines, rules.merge)
        .into_iter()
        .map(|(a, b)| (a + b) / 2)
        .collect()
}

/// Whether an upright rule within `(x0, x1)` carries on across the
/// horizontal rule between two bands: present on each of the `reach`
/// pixel rows above `bottom` (the upper band's last row) and below `top`
/// (the lower band's first). A table's column dividers do; a picture's
/// frame inside a cell stops short of the cell's edge.
pub fn carries_across<P: Pixels>(
    image: &P,
    (x0, x1): (i32, i32),
    (bottom, top): (i32, i32),
    reach: i32,
    rules: &RuleProfile,
) -> bool {
    (bottom - reach + 1..=bottom)
        .chain(top..top + reach)
        .all(|y| {
            y >= 0
                && y < image.height()
                && (x0.max(0)..=x1.min(image.width() - 1))
                    .any(|x| column_rule_pixel(image, x, y, rules))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{blank, bundled_profile, fill};

    const PANEL: Panel = Panel {
        rgb: [8, 11, 11],
        tolerance: 3,
    };

    #[test]
    fn rows_fall_between_rules_whatever_their_height() {
        let rules = bundled_profile().rules;
        for pitch in [46, 58, 72] {
            let mut image = blank(800, 600, PANEL.rgb);
            let ys: Vec<i32> = (0..6).map(|i| 100 + i * pitch).collect();
            for y in &ys {
                fill(&mut image, Rect::new(0, *y, 799, *y), [40, 44, 46]);
            }
            let found = rows(&image, Rect::new(0, 60, 799, 599), PANEL, &rules);
            assert_eq!(found.len(), 5, "pitch {pitch}");
            assert_eq!(found[0], (ys[0] + 1, ys[1] - 1));
        }
    }

    #[test]
    fn a_selected_rows_bright_outline_still_counts_as_its_rules() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 400, PANEL.rgb);
        fill(&mut image, Rect::new(0, 100, 799, 100), [142, 234, 72]);
        fill(&mut image, Rect::new(0, 158, 799, 159), [142, 234, 72]);
        fill(&mut image, Rect::new(0, 216, 799, 216), [40, 44, 46]);
        let found = rows(&image, Rect::new(0, 60, 799, 399), PANEL, &rules);
        assert_eq!(found, vec![(101, 157), (160, 215)]);
    }

    #[test]
    fn text_in_a_row_is_not_a_rule() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 300, PANEL.rgb);
        fill(&mut image, Rect::new(0, 100, 799, 100), [40, 44, 46]);
        fill(&mut image, Rect::new(30, 120, 400, 130), [200, 200, 200]);
        fill(&mut image, Rect::new(0, 158, 799, 158), [40, 44, 46]);
        assert_eq!(
            rows(&image, Rect::new(0, 60, 799, 299), PANEL, &rules),
            vec![(101, 157)]
        );
    }

    #[test]
    fn a_rules_extent_is_the_tables_width_across_dividers() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 200, PANEL.rgb);
        fill(&mut image, Rect::new(37, 100, 712, 100), [40, 44, 46]);
        // A divider crossing the rule, and a stray rule-coloured fleck
        // beyond the table's edge.
        fill(&mut image, Rect::new(300, 60, 300, 140), [36, 40, 42]);
        fill(&mut image, Rect::new(740, 100, 745, 100), [40, 44, 46]);
        assert_eq!(
            rule_extent(&image, 100, (0, 799), PANEL, &rules),
            Some((37, 712))
        );
        assert_eq!(rule_extent(&image, 20, (0, 799), PANEL, &rules), None);
    }

    #[test]
    fn a_divider_carries_across_a_row_rule_where_a_frame_stops_short() {
        let rules = bundled_profile().rules;
        let mut image = blank(400, 300, PANEL.rgb);
        fill(&mut image, Rect::new(0, 150, 399, 150), [40, 44, 46]);
        // A divider through both rows, and a frame in each row that stops
        // a few pixels short of the rule.
        fill(&mut image, Rect::new(300, 90, 300, 210), [36, 40, 42]);
        fill(&mut image, Rect::new(100, 95, 100, 144), [50, 54, 56]);
        fill(&mut image, Rect::new(100, 156, 100, 205), [50, 54, 56]);
        assert!(carries_across(&image, (297, 303), (149, 151), 3, &rules));
        assert!(!carries_across(&image, (97, 103), (149, 151), 3, &rules));
    }

    #[test]
    fn columns_are_found_from_their_dividers() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 300, PANEL.rgb);
        for x in [200, 520, 700] {
            fill(&mut image, Rect::new(x, 50, x, 250), [36, 40, 42]);
        }
        assert_eq!(
            column_rules(&image, Rect::new(10, 50, 790, 250), &rules),
            vec![200, 520, 700]
        );
    }
}
