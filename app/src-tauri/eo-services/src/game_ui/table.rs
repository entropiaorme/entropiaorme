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

/// Horizontal rule lines across `area`, as `(first, last)` pixel rows.
pub fn row_rules<P: Pixels>(
    image: &P,
    area: Rect,
    panel_rgb: [u8; 3],
    rules: &RuleProfile,
) -> Vec<(i32, i32)> {
    let (x0, x1) = (area.x0 + rules.row_inset[0], area.x1 - rules.row_inset[1]);
    if x1 <= x0 {
        return Vec::new();
    }
    let need = (rules.coverage * (x1 - x0 + 1) as f32) as i32;
    let mut lines = Vec::new();
    for y in area.y0.max(0)..=area.y1.min(image.height() - 1) {
        let mut hit = 0;
        for x in x0.max(0)..=x1.min(image.width() - 1) {
            let rgb = image.rgb(x, y);
            let l = image.lum(x, y);
            if l >= rules.row_rule_lum[0] && l <= rules.row_rule_lum[1] && !near(rgb, panel_rgb, 3)
            {
                hit += 1;
            }
        }
        if hit >= need {
            lines.push(y);
        }
    }
    merged(lines, rules.merge)
}

/// The rows of a table: the bands between consecutive rule lines that
/// are a plausible row height. Each is `(top, bottom)`, inclusive, with
/// the rules themselves excluded.
pub fn rows<P: Pixels>(
    image: &P,
    area: Rect,
    panel_rgb: [u8; 3],
    rules: &RuleProfile,
) -> Vec<(i32, i32)> {
    let lines = row_rules(image, area, panel_rgb, rules);
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
            let l = image.lum(x, y);
            if l >= rules.column_rule_lum[0]
                && l <= rules.column_rule_lum[1]
                && chroma(image.rgb(x, y)) <= 24
            {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{blank, bundled_profile, fill};

    const PANEL: [u8; 3] = [8, 11, 11];

    #[test]
    fn rows_fall_between_rules_whatever_their_height() {
        let rules = bundled_profile().rules;
        for pitch in [46, 58, 72] {
            let mut image = blank(800, 600, PANEL);
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
        let mut image = blank(800, 400, PANEL);
        fill(&mut image, Rect::new(0, 100, 799, 100), [142, 234, 72]);
        fill(&mut image, Rect::new(0, 158, 799, 159), [142, 234, 72]);
        fill(&mut image, Rect::new(0, 216, 799, 216), [40, 44, 46]);
        let found = rows(&image, Rect::new(0, 60, 799, 399), PANEL, &rules);
        assert_eq!(found, vec![(101, 157), (160, 215)]);
    }

    #[test]
    fn text_in_a_row_is_not_a_rule() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 300, PANEL);
        fill(&mut image, Rect::new(0, 100, 799, 100), [40, 44, 46]);
        fill(&mut image, Rect::new(30, 120, 400, 130), [200, 200, 200]);
        fill(&mut image, Rect::new(0, 158, 799, 158), [40, 44, 46]);
        assert_eq!(
            rows(&image, Rect::new(0, 60, 799, 299), PANEL, &rules),
            vec![(101, 157)]
        );
    }

    #[test]
    fn columns_are_found_from_their_dividers() {
        let rules = bundled_profile().rules;
        let mut image = blank(800, 300, PANEL);
        for x in [200, 520, 700] {
            fill(&mut image, Rect::new(x, 50, x, 250), [36, 40, 42]);
        }
        assert_eq!(
            column_rules(&image, Rect::new(10, 50, 790, 250), &rules),
            vec![200, 520, 700]
        );
    }
}
