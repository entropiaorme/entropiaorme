//! Finding runs of text inside a window.
//!
//! The client draws its windows opaque and dark, so anything bright
//! inside one is ink: lettering, icons, bars. This stage groups ink
//! into runs, one per phrase on a line, without recognising anything.
//! A reader then recognises the runs it cares about; in particular a
//! field is found by recognising its label and taking the run beside
//! it, never by where the field usually sits or what colour it is.

use serde::Deserialize;

use super::geometry::{chroma, near, Pixels, Rect};

/// What counts as ink and how glyphs join into runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextProfile {
    /// A pixel is ink when its brightest channel reaches this.
    pub min_ink: u8,
    /// Ink pixels on a row no further apart than this belong to one glyph.
    pub glyph_gap: i32,
    /// Glyphs join into a run across gaps up to this multiple of the
    /// run's height: wide enough for the client's letter-spaced
    /// capitals, narrow enough to keep a label apart from its value.
    pub join_factor: f32,
    /// Glyphs of visibly different colour never join, however close.
    pub max_colour_distance: u16,
    /// Fewest ink pixels a glyph or run needs for its colour to be
    /// judged: a dot or a comma carries too little to tell.
    pub min_colour_ink: i32,
    /// Two pieces of lettering are on one line when the middle of the
    /// shorter lies within the taller's span widened by this share of the
    /// taller's height (a comma hangs just below a digit's foot).
    pub line_slack: f32,
    /// A run shorter or taller than this is not lettering, `[min, max]`.
    pub run_height: [i32; 2],
    /// Fewest ink pixels in a run.
    pub min_ink_pixels: i32,
    /// Merge runs on one line that overlap or stand within joining
    /// distance. Small, dim lettering breaks into pieces (a percent
    /// sign's parts, a faint stroke) that joining in reading order can
    /// leave apart, one phrase as two runs that overlap.
    #[serde(default)]
    pub merge_pieces: bool,
}

/// One phrase-sized run of ink.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// Tight bounds of the ink, in the frame's coordinates.
    pub rect: Rect,
    /// Mean colour of the ink, `[r, g, b]`.
    pub rgb: [u8; 3],
    /// Ink pixel count.
    pub ink: i32,
}

struct Blob {
    rect: Rect,
    sum: [u64; 3],
    ink: i32,
}

impl Blob {
    fn rgb(&self) -> [u8; 3] {
        let n = self.ink.max(1) as u64;
        [
            (self.sum[0] / n) as u8,
            (self.sum[1] / n) as u8,
            (self.sum[2] / n) as u8,
        ]
    }

    fn absorb(&mut self, other: &Blob) {
        self.rect = Rect::new(
            self.rect.x0.min(other.rect.x0),
            self.rect.y0.min(other.rect.y0),
            self.rect.x1.max(other.rect.x1),
            self.rect.y1.max(other.rect.y1),
        );
        for (sum, add) in self.sum.iter_mut().zip(other.sum) {
            *sum += add;
        }
        self.ink += other.ink;
    }
}

fn find(parent: &mut [u32], mut a: u32) -> u32 {
    while parent[a as usize] != a {
        parent[a as usize] = parent[parent[a as usize] as usize];
        a = parent[a as usize];
    }
    a
}

/// What sets lettering the client draws straight onto the game scene
/// apart from the scene behind it. Such lettering has no panel, so
/// brightness alone does not find it: a sunlit wall is as bright. The
/// client draws it white with a dark shadow, so each stroke is a thin
/// pale line with darker pixels close on both of its sides, which a
/// broad bright surface does not have.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrokeProfile {
    /// Largest channel spread a stroke pixel may have: it is a grey.
    pub max_chroma: u8,
    /// How far either side of a pixel the shadow is looked for.
    pub reach: i32,
    /// How much brighter than the darkest pixel within reach, on each
    /// of two opposite sides, a stroke pixel is.
    pub contrast: u8,
}

/// Whether `(x, y)` is a stroke of lettering drawn on the scene: bright
/// enough to be ink, a grey, and with shadow on both sides of it either
/// across or along.
pub fn stroke_ink<P: Pixels>(
    image: &P,
    x: i32,
    y: i32,
    min_ink: u8,
    stroke: &StrokeProfile,
) -> bool {
    let rgb = image.rgb(x, y);
    if rgb[0].max(rgb[1]).max(rgb[2]) < min_ink || chroma(rgb) > stroke.max_chroma {
        return false;
    }
    let lum = image.lum(x, y);
    let shadowed = |dx: i32, dy: i32| {
        (1..=stroke.reach).any(|k| {
            let (px, py) = (x + dx * k, y + dy * k);
            image.in_bounds(px, py) && image.lum(px, py).saturating_add(stroke.contrast) <= lum
        })
    };
    (shadowed(-1, 0) && shadowed(1, 0)) || (shadowed(0, -1) && shadowed(0, 1))
}

/// Connected groups of ink within `roi`, joining pixels on a row that
/// sit within `glyph_gap` of each other. `ink` says whether a pixel, in
/// the frame's coordinates, is ink.
fn glyph_blobs<P: Pixels>(
    image: &P,
    roi: Rect,
    profile: &TextProfile,
    ink: &dyn Fn(i32, i32) -> bool,
) -> Vec<Blob> {
    let roi = match roi.intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1)) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let (w, h) = (roi.width() as usize, roi.height() as usize);
    // Each pixel is asked once: the test can look at its neighbours.
    let mask: Vec<bool> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| ink(roi.x0 + x as i32, roi.y0 + y as i32))
        .collect();
    let ink_at = |x: usize, y: usize| mask[y * w + x];
    // Each row becomes spans of ink bridged across small gaps; spans on
    // adjacent rows that overlap are one blob.
    let mut labels = vec![0u32; w * h];
    let mut parent: Vec<u32> = vec![0];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if !ink_at(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            let mut end = x;
            let mut probe = x + 1;
            while probe < w && probe - end <= profile.glyph_gap as usize + 1 {
                if ink_at(probe, y) {
                    end = probe;
                }
                probe += 1;
            }
            let label = parent.len() as u32;
            parent.push(label);
            for sx in start..=end {
                labels[y * w + sx] = label;
                if y > 0 {
                    let above = labels[(y - 1) * w + sx];
                    if above != 0 {
                        let (a, b) = (find(&mut parent, above), find(&mut parent, label));
                        if a != b {
                            parent[b as usize] = a;
                        }
                    }
                }
            }
            x = end + 1;
        }
    }
    let mut blobs: std::collections::HashMap<u32, Blob> = std::collections::HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let label = labels[y * w + x];
            if label == 0 || !ink_at(x, y) {
                continue;
            }
            let root = find(&mut parent, label);
            let (px, py) = (roi.x0 + x as i32, roi.y0 + y as i32);
            let rgb = image.rgb(px, py);
            let blob = blobs.entry(root).or_insert(Blob {
                rect: Rect::new(px, py, px, py),
                sum: [0; 3],
                ink: 0,
            });
            blob.rect = Rect::new(
                blob.rect.x0.min(px),
                blob.rect.y0.min(py),
                blob.rect.x1.max(px),
                blob.rect.y1.max(py),
            );
            for (sum, channel) in blob.sum.iter_mut().zip(rgb) {
                *sum += channel as u64;
            }
            blob.ink += 1;
        }
    }
    let mut out: Vec<Blob> = blobs.into_values().collect();
    out.sort_by_key(|b| (b.rect.x0, b.rect.y0));
    out
}

fn colour_distance(a: [u8; 3], b: [u8; 3]) -> u16 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x.abs_diff(*y) as u16)
        .sum()
}

/// Whether `(x, y)` is ink on one of the client's panels: anything
/// bright enough, since the panel itself is dark.
fn panel_ink<P: Pixels>(image: &P, x: i32, y: i32, profile: &TextProfile) -> bool {
    let [r, g, b] = image.rgb(x, y);
    r.max(g).max(b) >= profile.min_ink
}

/// Every run of text-like ink inside `roi`, in reading order.
pub fn text_runs<P: Pixels>(image: &P, roi: Rect, profile: &TextProfile) -> Vec<TextRun> {
    runs_of_ink(image, roi, profile, &|x, y| panel_ink(image, x, y, profile))
}

/// Every run of ink inside `roi`, in reading order, grouped as
/// [`text_runs`] groups it, where `ink` says which pixels (in the frame's
/// coordinates) are ink. For lettering on a panel that is itself bright,
/// where brightness alone does not tell the lettering from the panel and
/// the reader knows what does.
pub fn runs_of_ink<P: Pixels>(
    image: &P,
    roi: Rect,
    profile: &TextProfile,
    ink: &dyn Fn(i32, i32) -> bool,
) -> Vec<TextRun> {
    let blobs = glyph_blobs(image, roi, profile, ink);
    let runs = join_runs(blobs, profile);
    let runs = if profile.merge_pieces {
        merge_lines(runs, profile)
    } else {
        runs
    };
    finish_runs(runs, profile)
}

/// Every run of lettering drawn straight onto the scene inside `roi`,
/// in reading order: the same grouping as [`text_runs`], over strokes
/// told from the scene by their shadow (see [`StrokeProfile`]). Nothing
/// inside the rectangles in `in_front` is ink: the client's windows are
/// drawn over the scene, so their own lettering and furniture are never
/// part of lettering on it.
pub fn scenery_runs<P: Pixels>(
    image: &P,
    roi: Rect,
    profile: &TextProfile,
    stroke: &StrokeProfile,
    in_front: &[Rect],
) -> Vec<TextRun> {
    let blobs = glyph_blobs(image, roi, profile, &|x, y| {
        !in_front.iter().any(|front| front.contains(x, y))
            && stroke_ink(image, x, y, profile.min_ink, stroke)
    });
    finish_runs(merge_lines(join_runs(blobs, profile), profile), profile)
}

/// The same, keeping only ink that stands wholly within the rows `rows`
/// (`(top, bottom)`, in the frame's coordinates): a group of ink that
/// reaches above or below them is something drawn across the line the
/// rows hold, and is left out before the line's runs are put together.
pub fn scenery_runs_in_rows<P: Pixels>(
    image: &P,
    roi: Rect,
    rows: (i32, i32),
    profile: &TextProfile,
    stroke: &StrokeProfile,
    in_front: &[Rect],
) -> Vec<TextRun> {
    let mut blobs = glyph_blobs(image, roi, profile, &|x, y| {
        !in_front.iter().any(|front| front.contains(x, y))
            && stroke_ink(image, x, y, profile.min_ink, stroke)
    });
    blobs.retain(|blob| blob.rect.y0 >= rows.0 && blob.rect.y1 <= rows.1);
    finish_runs(merge_lines(join_runs(blobs, profile), profile), profile)
}

/// Whether two runs share a line: the middle of the shorter lies within
/// the taller's span, widened by the profile's line slack.
fn same_line(a: &Rect, b: &Rect, profile: &TextProfile) -> bool {
    let (tall, short) = if a.height() >= b.height() {
        (a, b)
    } else {
        (b, a)
    };
    let slack = (tall.height() as f32 * profile.line_slack) as i32;
    let mid = (short.y0 + short.y1) / 2;
    mid >= tall.y0 - slack && mid <= tall.y1 + slack
}

/// Runs on one line that overlap or stand within joining distance merged
/// into one. Strokes on the scene break into many small pieces where the
/// scene behind them is bright: a fragment of a serif can open a run that
/// the full glyph beside it is too tall to join, leaving one phrase split
/// across runs that overlap. Lettering on a panel does not break up so,
/// unless it is small and dim (see [`TextProfile::merge_pieces`]).
fn merge_lines(mut runs: Vec<Blob>, profile: &TextProfile) -> Vec<Blob> {
    loop {
        let mut merged = false;
        'search: for i in 0..runs.len() {
            for j in i + 1..runs.len() {
                let (a, b) = (runs[i].rect, runs[j].rect);
                let gap = a.x0.max(b.x0) - a.x1.min(b.x1);
                let height = a.height().max(b.height());
                if same_line(&a, &b, profile) && gap as f32 <= profile.join_factor * height as f32 {
                    let other = runs.remove(j);
                    runs[i].absorb(&other);
                    merged = true;
                    break 'search;
                }
            }
        }
        if !merged {
            return runs;
        }
    }
}

/// Runs that have the size and ink of lettering, in reading order.
fn finish_runs(runs: Vec<Blob>, profile: &TextProfile) -> Vec<TextRun> {
    let mut out: Vec<TextRun> = runs
        .into_iter()
        .filter(|r| {
            r.rect.height() >= profile.run_height[0]
                && r.rect.height() <= profile.run_height[1]
                && r.ink >= profile.min_ink_pixels
        })
        .map(|r| TextRun {
            rect: r.rect,
            rgb: r.rgb(),
            ink: r.ink,
        })
        .collect();
    out.sort_by_key(|r| (r.rect.y0, r.rect.x0));
    out
}

/// Glyphs joined into phrase-sized runs.
fn join_runs(blobs: Vec<Blob>, profile: &TextProfile) -> Vec<Blob> {
    let mut runs: Vec<Blob> = Vec::new();
    for blob in blobs {
        if blob.rect.height() > profile.run_height[1] {
            continue;
        }
        // Attach to the run on the same line whose right end is nearest.
        let mut target: Option<usize> = None;
        for (i, run) in runs.iter().enumerate() {
            let height = run.rect.height().max(blob.rect.height());
            let gap = blob.rect.x0 - run.rect.x1;
            if gap < -1 || gap as f32 > profile.join_factor * height as f32 {
                continue;
            }
            // Same line: the shorter of the two sits within the taller's
            // vertical span (a comma hangs just below a digit's foot).
            let slack = (height as f32 * profile.line_slack) as i32;
            let (top, bottom) = (run.rect.y0 - slack, run.rect.y1 + slack);
            let mid = (blob.rect.y0 + blob.rect.y1) / 2;
            if mid < top || mid > bottom || blob.rect.height() > run.rect.height() * 2 {
                continue;
            }
            // A dot or comma carries too little ink to judge colour by.
            if blob.ink >= profile.min_colour_ink
                && run.ink >= profile.min_colour_ink
                && colour_distance(blob.rgb(), run.rgb()) > profile.max_colour_distance
            {
                continue;
            }
            if target.is_none_or(|t| runs[t].rect.x1 < run.rect.x1) {
                target = Some(i);
            }
        }
        match target {
            Some(i) => runs[i].absorb(&blob),
            None => runs.push(blob),
        }
    }
    runs
}

/// The bounds of each connected group of ink inside `roi`, before any
/// joining into runs. A picture, such as a skill's icon, is one or a few
/// large groups where lettering is many small ones, so this is how a
/// reader tells a picture from the text beside it when the two are close
/// enough in colour to join into one run.
pub fn ink_groups<P: Pixels>(image: &P, roi: Rect, profile: &TextProfile) -> Vec<Rect> {
    glyph_blobs(image, roi, profile, &|x, y| panel_ink(image, x, y, profile))
        .into_iter()
        .map(|blob| blob.rect)
        .collect()
}

/// The bounds of each connected group of anything drawn on the panel
/// inside `roi`: every pixel that is not the panel's fill, however dim.
/// The dim box the client frames a picture in is one group as tall as
/// the box, where lettering beside it is many small ones; so this finds
/// the frame of a picture whose own bright ink would join the
/// lettering's run.
pub fn drawn_groups<P: Pixels>(
    image: &P,
    roi: Rect,
    panel_rgb: [u8; 3],
    panel_tolerance: u8,
    profile: &TextProfile,
) -> Vec<Rect> {
    glyph_blobs(image, roi, profile, &|x, y| {
        !near(image.rgb(x, y), panel_rgb, panel_tolerance)
    })
    .into_iter()
    .map(|blob| blob.rect)
    .collect()
}

/// The run immediately to the right of `anchor` on its line, if any.
pub fn run_right_of<'a>(anchor: &TextRun, runs: &'a [TextRun]) -> Option<&'a TextRun> {
    let mid = (anchor.rect.y0 + anchor.rect.y1) / 2;
    runs.iter()
        .filter(|r| r.rect.x0 > anchor.rect.x1)
        .filter(|r| {
            let slack = r.rect.height().max(anchor.rect.height()) / 2;
            mid >= r.rect.y0 - slack && mid <= r.rect.y1 + slack
        })
        .min_by_key(|r| r.rect.x0)
}

/// The nearest run below `anchor` that starts within `reach` of the
/// anchor's heights of its left edge, if any.
pub fn run_below<'a>(anchor: &TextRun, runs: &'a [TextRun], reach: f32) -> Option<&'a TextRun> {
    let tolerance = (anchor.rect.height() as f32 * reach).round() as i32;
    runs.iter()
        .filter(|r| r.rect.y0 > anchor.rect.y1)
        .filter(|r| (r.rect.x0 - anchor.rect.x0).abs() <= tolerance)
        .min_by_key(|r| r.rect.y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{blank, bundled_profile, fill};

    /// Draw a "word": `glyphs` blocks of `h` tall, `gw` wide, `gap` apart.
    fn word(
        image: &mut crate::game_ui::geometry::BgrImage,
        (x, y): (i32, i32),
        glyphs: i32,
        (gw, h): (i32, i32),
        gap: i32,
        rgb: [u8; 3],
    ) -> Rect {
        for g in 0..glyphs {
            let gx = x + g * (gw + gap);
            fill(image, Rect::new(gx, y, gx + gw - 1, y + h - 1), rgb);
        }
        Rect::new(x, y, x + glyphs * (gw + gap) - gap - 1, y + h - 1)
    }

    #[test]
    fn a_label_and_its_value_are_separate_runs_on_one_line() {
        let profile = bundled_profile().text;
        let mut image = blank(600, 80, [8, 11, 11]);
        let label = word(&mut image, (20, 30), 8, (6, 10), 3, [150, 150, 150]);
        let value = word(
            &mut image,
            (20 + 8 * 9 + 50, 28),
            6,
            (8, 14),
            1,
            [140, 230, 70],
        );
        let runs = text_runs(&image, Rect::new(0, 0, 599, 79), &profile);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().any(|r| r.rect == value));
        let label_run = runs.iter().find(|r| r.rect == label).expect("label");
        assert_eq!(run_right_of(label_run, &runs).map(|r| r.rect), Some(value));
    }

    #[test]
    fn letter_spaced_capitals_join_into_one_run() {
        let profile = bundled_profile().text;
        let mut image = blank(600, 80, [8, 11, 11]);
        // Two "words" a wide letter-space apart, as the client's titles are set.
        let a = word(&mut image, (20, 30), 6, (6, 10), 5, [200, 200, 200]);
        let b = word(&mut image, (a.x1 + 14, 30), 4, (6, 10), 5, [200, 200, 200]);
        let runs = text_runs(&image, Rect::new(0, 0, 599, 79), &profile);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].rect, Rect::new(a.x0, a.y0, b.x1, b.y1));
    }

    #[test]
    fn touching_text_of_another_colour_stays_apart() {
        let profile = bundled_profile().text;
        let mut image = blank(600, 80, [8, 11, 11]);
        let grey = word(&mut image, (20, 30), 5, (6, 10), 2, [150, 150, 150]);
        let green = word(&mut image, (grey.x1 + 6, 30), 5, (6, 10), 2, [140, 230, 70]);
        let runs = text_runs(&image, Rect::new(0, 0, 599, 79), &profile);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().any(|r| r.rect == green));
    }

    #[test]
    fn a_value_below_its_label_is_found_by_its_left_edge() {
        let profile = bundled_profile().text;
        let mut image = blank(600, 120, [8, 11, 11]);
        let label = word(&mut image, (20, 20), 10, (5, 8), 2, [150, 150, 150]);
        let value = word(&mut image, (21, 60), 8, (14, 28), 2, [140, 230, 70]);
        word(&mut image, (400, 60), 4, (6, 10), 2, [200, 200, 200]);
        let runs = text_runs(&image, Rect::new(0, 0, 599, 119), &profile);
        let label_run = runs.iter().find(|r| r.rect == label).expect("label");
        assert_eq!(
            run_below(label_run, &runs, 1.0).map(|r| r.rect),
            Some(value)
        );
        assert_eq!(run_below(label_run, &runs, 0.0).map(|r| r.rect), None);
    }

    #[test]
    fn a_picture_beside_lettering_is_its_own_ink_group() {
        let profile = bundled_profile().text;
        let mut image = blank(600, 80, [8, 11, 11]);
        // A picture and a word close enough in colour and position to
        // join into one run, as a skill's icon and name can.
        let picture = Rect::new(20, 12, 60, 60);
        fill(&mut image, picture, [170, 160, 150]);
        let letters = word(&mut image, (75, 30), 5, (6, 12), 4, [180, 180, 180]);
        let runs = text_runs(&image, Rect::new(0, 0, 599, 79), &profile);
        assert_eq!(runs.len(), 1, "the picture and the word join");
        let groups = ink_groups(&image, Rect::new(0, 0, 599, 79), &profile);
        assert_eq!(groups.len(), 6);
        assert!(groups.contains(&picture));
        assert!(groups
            .iter()
            .filter(|g| **g != picture)
            .all(|g| g.x0 >= letters.x0 && g.x1 <= letters.x1));
    }

    #[test]
    fn shadowed_lettering_on_a_bright_scene_is_a_run_and_the_scene_is_not() {
        use crate::game_ui::test_support::put;
        let coords = bundled_profile().radar.coordinates;
        let mut image = blank(400, 100, [120, 110, 90]);
        // A sunlit wall, as bright and as grey as the lettering, and a
        // pale stripe across it: broad bright surfaces with no shadow.
        fill(&mut image, Rect::new(200, 22, 399, 50), [225, 225, 222]);
        fill(&mut image, Rect::new(0, 80, 399, 87), [235, 235, 235]);
        // Lettering: white glyph outlines on their own dark shadow, the
        // last of them on the wall.
        let text = Rect::new(150, 30, 239, 40);
        fill(&mut image, text.padded(1), [24, 24, 24]);
        let mut x = text.x0;
        while x + 5 <= text.x1 {
            for k in x..x + 6 {
                put(&mut image, k, text.y0, [250, 250, 250]);
                put(&mut image, k, text.y1, [250, 250, 250]);
            }
            for y in text.y0..=text.y1 {
                put(&mut image, x, y, [250, 250, 250]);
                put(&mut image, x + 5, y, [250, 250, 250]);
            }
            x += 8;
        }
        let runs = scenery_runs(
            &image,
            Rect::new(0, 0, 399, 99),
            &coords.lettering,
            &coords.stroke,
            &[],
        );
        assert_eq!(
            runs.iter().map(|r| r.rect).collect::<Vec<_>>(),
            vec![Rect::new(150, 30, 235, 40)]
        );
        // On a panel's terms the wall and the stripe would be ink.
        assert!(text_runs(&image, Rect::new(0, 0, 399, 99), &coords.lettering).len() > 1);
    }

    #[test]
    fn pieces_of_one_phrase_that_overlap_merge_only_when_asked() {
        let mut profile = bundled_profile().text;
        let mut image = blank(300, 60, [8, 11, 11]);
        // Three pieces of one phrase: the second joins the first and
        // reaches past where the third, lying under it, begins; so in
        // reading order the third finds the run already beyond it.
        fill(&mut image, Rect::new(20, 18, 70, 29), [150, 150, 150]);
        fill(&mut image, Rect::new(74, 18, 100, 21), [150, 150, 150]);
        fill(&mut image, Rect::new(85, 25, 95, 29), [150, 150, 150]);
        let area = Rect::new(0, 0, 299, 59);
        let apart = text_runs(&image, area, &profile);
        assert_eq!(apart.len(), 2, "{apart:?}");
        profile.merge_pieces = true;
        let merged = text_runs(&image, area, &profile);
        assert_eq!(
            merged.iter().map(|r| r.rect).collect::<Vec<_>>(),
            vec![Rect::new(20, 18, 100, 29)]
        );
    }

    #[test]
    fn a_framed_picture_is_one_tall_drawn_group_and_letters_are_their_own() {
        let profile = bundled_profile();
        let mut image = blank(300, 80, [8, 11, 11]);
        // A dim frame with a bright picture inside, then a word.
        let frame = Rect::new(10, 10, 50, 50);
        fill(&mut image, frame, [33, 36, 36]);
        fill(&mut image, frame.padded(-1), [8, 11, 11]);
        fill(&mut image, Rect::new(20, 20, 40, 40), [140, 220, 230]);
        let word = Rect::new(60, 25, 90, 35);
        fill(&mut image, word, [230, 230, 230]);
        let groups = drawn_groups(
            &image,
            Rect::new(0, 0, 299, 79),
            profile.chrome.panel_rgb,
            profile.chrome.panel_tolerance,
            &profile.text,
        );
        // The frame is one group as tall as itself, and what lies within
        // it stays there; the word's letters are a group of their own.
        assert!(
            groups.contains(&frame) && groups.contains(&word),
            "{groups:?}"
        );
        assert!(groups
            .iter()
            .filter(|g| **g != word)
            .all(|g| frame.intersect(g) == Some(*g)));
    }

    #[test]
    fn dim_rule_lines_are_not_ink() {
        let profile = bundled_profile().text;
        let mut image = blank(300, 60, [8, 11, 11]);
        fill(&mut image, Rect::new(0, 30, 299, 30), [60, 64, 66]);
        assert!(text_runs(&image, Rect::new(0, 0, 299, 59), &profile).is_empty());
    }
}
