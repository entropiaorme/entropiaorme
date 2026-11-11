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
#[serde(deny_unknown_fields)]
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

/// One band of the table between two consecutive rule lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub top: i32,
    pub bottom: i32,
    /// Whether the table's column dividers cross it: a band of the
    /// table, not of a pane below it.
    pub ruled: bool,
}

impl Band {
    pub fn height(&self) -> i32 {
        self.bottom - self.top + 1
    }
}

/// The table's rows below the header, each with whether it is its full
/// height, and the row height they were measured against.
#[derive(Debug, Clone, PartialEq)]
pub struct TableShape {
    pub rows: Vec<(Band, bool)>,
    pub pitch: i32,
}

/// Sort the bands under the header into rows. The rows are the ruled
/// bands from the header down. Every row but the last lies between two
/// row rules, so those give the row's height; the last is bounded by
/// whatever ends the view (another row's rule, a pane, the window's
/// edge), so it is full when at least that tall, and short when the view
/// cuts it. A lone row is measured against itself: the caller's check
/// that its lettering sits on its middle is what catches one cut short.
/// Whether the list carries on beyond the view is not the rows' to say:
/// a view can end exactly on a row's rule with more rows below it, which
/// only the scrollbar shows.
pub fn table_shape(below_header: &[Band], row_slack: f32) -> TableShape {
    let rows: Vec<Band> = below_header
        .iter()
        .take_while(|band| band.ruled)
        .copied()
        .collect();
    let pitch = match rows.split_last() {
        Some((last, [])) => last.height(),
        Some((_, ruled_both_sides)) => ruled_both_sides.iter().map(Band::height).max().unwrap_or(0),
        None => 0,
    };
    let least = pitch as f32 * (1.0 - row_slack);
    let rows = rows
        .iter()
        .map(|band| (*band, band.height() as f32 >= least))
        .collect();
    TableShape { rows, pitch }
}

/// The stretches of panel inside `area` that something drawn on the
/// panel closes in on every side: the inside of a box, or of a glyph
/// such as `0`. Each is the bounds of one stretch of panel-coloured
/// pixels, joined edge to edge (never only at a corner, so a box whose
/// corner is cut still closes), that does not reach the edge of `area`.
/// The client draws a control that holds a value (a spinner's figure,
/// say) as a box on the panel, so this is how a reader finds the value
/// among the lettering beside it, whatever the box's size or colour.
pub fn enclosed_regions<P: Pixels>(image: &P, area: Rect, panel: Panel) -> Vec<Rect> {
    let Some(area) = area.intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1)) else {
        return Vec::new();
    };
    let (w, h) = (area.width() as usize, area.height() as usize);
    let is_panel: Vec<bool> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            near(
                image.rgb(area.x0 + x as i32, area.y0 + y as i32),
                panel.rgb,
                panel.tolerance,
            )
        })
        .collect();
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !is_panel[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let mut reaches_edge = false;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            reaches_edge |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let mut visit = |j: usize| {
                if is_panel[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if !reaches_edge {
            out.push(Rect::new(
                area.x0 + x0 as i32,
                area.y0 + y0 as i32,
                area.x0 + x1 as i32,
                area.y0 + y1 as i32,
            ));
        }
    }
    out
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

    fn band(top: i32, bottom: i32, ruled: bool) -> Band {
        Band { top, bottom, ruled }
    }

    fn fits(shape: &TableShape) -> Vec<bool> {
        shape.rows.iter().map(|(_, full)| *full).collect()
    }

    #[test]
    fn rows_ruled_on_both_sides_set_the_row_height() {
        let below = [
            band(100, 156, true),
            band(158, 214, true),
            band(216, 271, true),
            band(273, 500, false),
        ];
        let shape = table_shape(&below, 0.05);
        assert_eq!(shape.pitch, 57);
        assert_eq!(fits(&shape), vec![true, true, true]);
    }

    #[test]
    fn a_row_cut_by_the_bottom_of_the_view_is_not_full() {
        let below = [
            band(100, 156, true),
            band(158, 214, true),
            band(216, 240, true),
            band(242, 400, false),
        ];
        assert_eq!(fits(&table_shape(&below, 0.05)), vec![true, true, false]);
    }

    #[test]
    fn a_last_row_that_the_view_ends_below_is_still_full() {
        let below = [
            band(100, 155, true),
            band(159, 213, true),
            band(215, 271, true),
            band(273, 332, true),
            band(334, 500, false),
        ];
        assert!(fits(&table_shape(&below, 0.05)).iter().all(|f| *f));
    }

    #[test]
    fn panes_below_the_table_are_not_rows() {
        let below = [
            band(100, 156, true),
            band(158, 214, false),
            band(216, 272, true),
        ];
        assert_eq!(table_shape(&below, 0.05).rows.len(), 1);
    }

    #[test]
    fn the_insides_of_boxes_and_glyphs_are_enclosed_and_open_panel_is_not() {
        let mut image = blank(300, 200, PANEL.rgb);
        // A box with a cut corner, a dim one, and a box left open on
        // one side.
        let outline = |image: &mut crate::game_ui::geometry::BgrImage, r: Rect, rgb: [u8; 3]| {
            fill(image, Rect::new(r.x0, r.y0, r.x1, r.y0), rgb);
            fill(image, Rect::new(r.x0, r.y1, r.x1, r.y1), rgb);
            fill(image, Rect::new(r.x0, r.y0, r.x0, r.y1), rgb);
            fill(image, Rect::new(r.x1, r.y0, r.x1, r.y1), rgb);
        };
        outline(&mut image, Rect::new(20, 20, 80, 50), [74, 74, 74]);
        crate::game_ui::test_support::put(&mut image, 20, 20, PANEL.rgb);
        outline(&mut image, Rect::new(100, 20, 160, 50), [15, 18, 18]);
        outline(&mut image, Rect::new(180, 20, 240, 50), [74, 74, 74]);
        fill(&mut image, Rect::new(240, 30, 240, 35), PANEL.rgb);
        let found = enclosed_regions(&image, Rect::new(0, 0, 299, 199), PANEL);
        assert_eq!(
            found,
            vec![Rect::new(21, 21, 79, 49), Rect::new(101, 21, 159, 49)]
        );
        // Searched over less than the whole box, nothing is enclosed.
        assert!(enclosed_regions(&image, Rect::new(0, 0, 60, 199), PANEL).is_empty());
    }
}
