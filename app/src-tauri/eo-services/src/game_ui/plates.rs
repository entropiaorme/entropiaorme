//! Finding creatures' nameplates, and reading the one the player has
//! locked on to.
//!
//! The client floats a nameplate over every creature in view, drawn
//! straight onto the game scene: on one line the creature's level
//! (`L25`) and its name, and under that line a health bar. The bar is
//! the same size on screen for every creature, and its coloured part
//! runs from red at its left end through yellow to green at its right:
//! the colour at each point is fixed by the point's place along the bar,
//! and the creature's health is how far the colour reaches, the rest of
//! the bar standing empty.
//!
//! The plate of the creature the player has locked on to is drawn solid:
//! an opaque dark frame with a grey bevel inside it, the bar's empty part
//! black, its colours vivid, and a small red pointer under it. That plate
//! is found by its frame and bevel, which a creature at its last sliver
//! of health still has though hardly any colour is left; the pointer
//! confirms the lock. Every other plate is washed out, blended with the
//! scene behind it, and found by its colour run alone: a run of the
//! bar's width whose hue follows the bar's own gradient point by point.
//! A creature standing in front of a bar can cut the run, so short gaps
//! are bridged and a share of the run's points may differ.
//!
//! Nothing is found by where plates usually are or by a colour alone:
//! the player's own health bar, the tool bar beside it, the progress
//! bars inside windows and the pictures of items are red, orange or green
//! too, but none has the bar's width, its gradient, and (for the locked
//! plate) its frame.
//!
//! The plate's pixel sizes differ with the interface scale, so the profile
//! lists them per scale; a plate is taken only where exactly one scale's
//! frame, bevel and width all fit. Everything else is stated in bar
//! widths and serves every scale.
//!
//! The level and the name are one line of lettering above the bar. Where
//! the line starts is not assumed (the captures do not show whether the
//! client centres it or sets it from the bar's left): the line is the
//! lettering above the bar that overlaps the bar's columns, its first
//! piece must read as a level, and the name is the rest of the line,
//! aligned with the level. Lettering that runs out of the area searched,
//! meets a window, or meets another plate is refused: a name cut short or
//! joined to another plate's lettering can read as another creature's.

use serde::Deserialize;

use crate::game_ui::geometry::BgrImage;

use super::field::{read_run, Recogniser};
use super::geometry::{chroma, hue, near, Pixels, Rect};
use super::profile::UiProfile;
use super::text::{scenery_runs, StrokeProfile, TextProfile};
use super::validate::labelled_integer;
use super::window::{find_windows, ChromeProfile};

/// What creatures' nameplates look like, as the profile describes them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlateProfile {
    /// The plate's pixel sizes, one entry per interface scale measured.
    pub scales: Vec<PlateScale>,
    /// The colour run along every bar.
    pub run: ColourRunProfile,
    /// The locked plate's frame, bevel and black empty part.
    pub frame: FrameProfile,
    /// The pointer under a plate.
    pub pointer: PointerProfile,
    /// The line of lettering over the bar.
    pub line: LineProfile,
    /// The level, the line's first piece.
    pub level: LevelProfile,
}

/// A plate's pixel sizes at one interface scale.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlateScale {
    /// The health bar's coloured part (filled or not), `[width, height]`.
    pub bar: [i32; 2],
    /// How many pixels a bar's measured width may differ from it, and how
    /// far a run's ends may stand from where a bar of that width would put
    /// them.
    pub width_tolerance: i32,
    /// The locked plate's frame: its thickness,
    pub frame_width: i32,
    /// and the rows of bevel between it and the bar (as many columns at
    /// the bar's ends).
    pub bevel_rows: i32,
    /// A washed-out bar is seen on at least this many of its rows: a
    /// creature in front can hide the others.
    pub min_rows: usize,
    /// Fewest pixels of the red pointer under the locked plate.
    pub pointer_pixels: usize,
    /// How the lettering's strokes group into runs.
    pub lettering: TextProfile,
    /// How the lettering is told from the scene behind it.
    pub stroke: StrokeProfile,
}

/// The bar's colour run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColourRunProfile {
    /// The hue at points along the bar, `[place, degrees]`, the place
    /// from 0 at the bar's left end to 1 at its right, in order. Between
    /// two points the hue is taken to change evenly.
    pub gradient: Vec<[f32; 2]>,
    /// How far a pixel's hue may stand from the gradient's at its place.
    /// A washed-out bar's hues are pulled towards the scene's.
    pub hue_tolerance: f32,
    /// A pixel of a washed-out bar has at least this chroma.
    pub min_chroma: u8,
    /// Gaps in a run up to this share of the bar's width are bridged.
    pub max_gap: f32,
    /// A run, gaps bridged, spans at least this share of the bar's width.
    pub min_span: f32,
    /// At least this share of the bar's columns match the gradient.
    pub share: f32,
}

/// The locked plate's frame and the bar inside it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameProfile {
    /// The frame's colour,
    pub rgb: [u8; 3],
    /// and how far each channel may stand from it.
    pub tolerance: u8,
    /// The bevel's colour.
    pub bevel_rgb: [u8; 3],
    pub bevel_tolerance: u8,
    /// Least share of the frame's pixels that must show its colour.
    pub share: f32,
    /// The colour of the bar's empty part.
    pub unfilled_rgb: [u8; 3],
    pub unfilled_tolerance: u8,
    /// A pixel of the locked bar's colour has at least this chroma.
    pub vivid_chroma: u8,
    /// Most share of the bar's columns that may be neither colour nor
    /// empty: the column where the colour ends is drawn between the two.
    pub max_stray: f32,
}

/// The red pointer the client draws under the locked plate.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerProfile {
    /// Where it is looked for, `[left, top, right, bottom]`, in bar
    /// widths from the bar's top-left corner.
    pub area: [f64; 4],
    /// A pointer pixel is at least this red,
    pub min_red: u8,
    /// with green and blue at most this.
    pub max_other: u8,
}

/// The line of lettering over the bar.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineProfile {
    /// The area searched for it, `[left, top, right, bottom]`, in bar
    /// widths from the bar's top-left corner: wide enough for a long name
    /// set from the bar's left or centred on it.
    pub area: [f64; 4],
    /// Lettering further than this many bar widths from the line's last
    /// piece is not part of it: the client draws a status icon some way
    /// after the name.
    pub gap: f64,
    /// The name's top stands within this many of the level's heights of
    /// the level's top (both are capitals and digits at the top),
    pub alignment: f64,
    /// and its foot at most this many of the level's heights below the
    /// level's (letters such as `p` hang below the line).
    pub descent: f64,
    /// A row of window panel this many bar widths long, where the line or
    /// the stretch past its end lies, is a window in front of it.
    pub panel_run: f64,
}

/// The level, the line's first piece.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelProfile {
    /// The letter the client prints before the number.
    pub label: String,
    /// How many digits a level has, `[fewest, most]`.
    pub digits: [usize; 2],
    /// How far apart the level's glyphs stand, in bar widths: the ink of
    /// a level is about this many times its glyph count wide,
    pub pitch: f64,
    /// to within this many glyphs.
    pub pitch_tolerance: f64,
    /// How many glyphs a level has, `[fewest, most]` (its letter and its
    /// digits): a first piece of the line this many pitches wide is the
    /// level's place whatever it read as, so a misread level does not put
    /// the name in doubt.
    pub glyphs: [f64; 2],
}

/// Whether a plate is the locked one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlateKind {
    /// The plate of the creature the player has locked on to, with its
    /// health: the share of the bar its colour fills, 0 to 1.
    Locked { fill: f64 },
    /// Any other plate.
    Unlocked,
}

/// One plate found in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    /// The bar's coloured part, filled or not.
    pub bar: Rect,
    /// Which of the profile's scales it was found at.
    pub scale: usize,
    pub kind: PlateKind,
}

impl Plate {
    pub fn is_locked(&self) -> bool {
        matches!(self.kind, PlateKind::Locked { .. })
    }
}

/// Buffers the plate search reuses, so that a watcher reading the screen
/// once a second allocates nothing for the frame's size.
#[derive(Debug, Default)]
pub struct PlateScratch {
    top: Vec<bool>,
    mask: Vec<bool>,
}

/// The hue the gradient gives at `place` (0 to 1).
fn gradient_hue(gradient: &[[f32; 2]], place: f32) -> Option<f32> {
    let first = gradient.first()?;
    if place <= first[0] {
        return Some(first[1]);
    }
    for pair in gradient.windows(2) {
        let ([p0, h0], [p1, h1]) = (pair[0], pair[1]);
        if place <= p1 {
            let span = p1 - p0;
            return Some(if span > 0.0 {
                h0 + (h1 - h0) * (place - p0) / span
            } else {
                h1
            });
        }
    }
    gradient.last().map(|last| last[1])
}

/// The gradient's hue at the middle of each of the bar's columns.
fn column_hues(run: &ColourRunProfile, width: i32) -> Option<Vec<f32>> {
    (0..width)
        .map(|c| gradient_hue(&run.gradient, (c as f32 + 0.5) / width as f32))
        .collect()
}

/// Whether `rgb` is the bar's colour where the gradient's hue is `want`.
fn on_gradient(rgb: [u8; 3], want: f32, min_chroma: u8, tolerance: f32) -> bool {
    chroma(rgb) >= min_chroma && hue(rgb).is_some_and(|h| (h - want).abs() <= tolerance)
}

/// Whether the pixel at byte offset `i` of a BGR buffer is within
/// `tolerance` of `rgb` in every channel.
fn close(data: &[u8], i: usize, rgb: [u8; 3], tolerance: u8) -> bool {
    data[i + 2].abs_diff(rgb[0]) <= tolerance
        && data[i + 1].abs_diff(rgb[1]) <= tolerance
        && data[i].abs_diff(rgb[2]) <= tolerance
}

/// A solid plate's frame and bar found, before the pointer decides
/// whether it is the locked one.
#[derive(Debug, Clone, Copy)]
struct Framed {
    bar: Rect,
    scale: usize,
    fill: f64,
    pointer: bool,
}

/// The rectangle of a solid plate's outer edge at `scale`.
fn outer(bar: &Rect, scale: &PlateScale) -> Rect {
    bar.padded(scale.frame_width + scale.bevel_rows)
}

/// Starts and (exclusive) ends of the runs of `true` in `mask`, offset by
/// `x0`.
fn runs_of(mask: &[bool], x0: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let mut x = 0;
    while x < mask.len() {
        if !mask[x] {
            x += 1;
            continue;
        }
        let start = x;
        while x < mask.len() && mask[x] {
            x += 1;
        }
        out.push((x0 + start as i32, x0 + x as i32));
    }
    out
}

/// Keep only what exactly one scale found: a plate two scales both fit
/// cannot be told.
fn one_scale<T>(found: Vec<T>, place: impl Fn(&T) -> (Rect, usize)) -> Vec<T> {
    let placed: Vec<(Rect, usize)> = found.iter().map(&place).collect();
    found
        .into_iter()
        .enumerate()
        .filter(|(i, _)| {
            let (rect, scale) = placed[*i];
            !placed
                .iter()
                .any(|(other, s)| *s != scale && other.intersect(&rect).is_some())
        })
        .map(|(_, item)| item)
        .collect()
}

/// Every solid plate within `roi`, at every scale: a frame row with the
/// bevel under it along exactly the bar's width (the bevel turns into the
/// frame's end caps there), a frame all round, inside it a bar whose
/// colour (if any is left) reaches from the left end and gives way to the
/// black empty part, and nothing much else. Rows are scanned with integer
/// comparisons only; the gradient is checked on candidates alone.
fn framed_plates(
    image: &BgrImage,
    roi: Rect,
    p: &PlateProfile,
    scratch: &mut PlateScratch,
) -> Vec<Framed> {
    let f = &p.frame;
    let (iw, ih) = (image.width(), image.height());
    let Some(roi) = roi.intersect(&Rect::new(0, 0, iw - 1, ih - 1)) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (index, scale) in p.scales.iter().enumerate() {
        let [w, h] = scale.bar;
        let inset = scale.frame_width + scale.bevel_rows;
        if w < 2 || h < 1 || scale.frame_width < 1 || scale.bevel_rows < 1 {
            continue;
        }
        let Some(hues) = column_hues(&p.run, w) else {
            continue;
        };
        let frame_at = |x: i32, y: i32| {
            image.in_bounds(x, y)
                && close(&image.data, ((y * iw + x) * 3) as usize, f.rgb, f.tolerance)
        };
        let row_len = roi.width() as usize;
        scratch.top.clear();
        scratch.top.resize(row_len, false);
        for y in roi.y0..=roi.y1 - (h + 2 * inset) + 1 {
            for (k, on) in scratch.top.iter_mut().enumerate() {
                let x = roi.x0 + k as i32;
                let at = |row: i32| ((row * iw + x) * 3) as usize;
                *on = (y..y + scale.frame_width)
                    .all(|fy| close(&image.data, at(fy), f.rgb, f.tolerance))
                    && (y + scale.frame_width..y + inset)
                        .all(|by| close(&image.data, at(by), f.bevel_rgb, f.bevel_tolerance));
            }
            for (start, end) in runs_of(&scratch.top, roi.x0) {
                if (end - start - w).abs() > scale.width_tolerance {
                    continue;
                }
                let bar = Rect::new(start, y + inset, start + w - 1, y + inset + h - 1);
                let edge = outer(&bar, scale);
                if edge.x0 < 0 || edge.y0 < 0 || edge.x1 >= iw || edge.y1 >= ih {
                    continue;
                }
                if let Some(fill) = framed_fill(image, &bar, &edge, scale, p, &hues, &frame_at) {
                    found.push(Framed {
                        bar,
                        scale: index,
                        fill,
                        pointer: pointer_under(image, &bar, scale, &p.pointer),
                    });
                }
            }
        }
    }
    one_scale(found, |f| (f.bar, f.scale))
}

/// The fill of the solid bar at `bar`, when its frame and contents are
/// a plate's.
fn framed_fill(
    image: &BgrImage,
    bar: &Rect,
    edge: &Rect,
    scale: &PlateScale,
    p: &PlateProfile,
    hues: &[f32],
    frame_at: &dyn Fn(i32, i32) -> bool,
) -> Option<f64> {
    let f = &p.frame;
    let (mut shown, mut total) = (0usize, 0usize);
    for k in 0..scale.frame_width {
        let ring = edge.padded(-k);
        for x in ring.x0..=ring.x1 {
            for y in [ring.y0, ring.y1] {
                total += 1;
                shown += frame_at(x, y) as usize;
            }
        }
        for y in ring.y0 + 1..ring.y1 {
            for x in [ring.x0, ring.x1] {
                total += 1;
                shown += frame_at(x, y) as usize;
            }
        }
    }
    if total == 0 || (shown as f32) < f.share * total as f32 {
        return None;
    }
    // Each column is colour or empty by most of its rows.
    let rows = bar.height();
    let mut colour = Vec::with_capacity(hues.len());
    let mut empty = Vec::with_capacity(hues.len());
    for (c, want) in hues.iter().enumerate() {
        let x = bar.x0 + c as i32;
        let (mut lit, mut dark) = (0, 0);
        for y in bar.y0..=bar.y1 {
            let rgb = image.rgb(x, y);
            if on_gradient(rgb, *want, f.vivid_chroma, p.run.hue_tolerance) {
                lit += 1;
            } else if near(rgb, f.unfilled_rgb, f.unfilled_tolerance) {
                dark += 1;
            }
        }
        colour.push(2 * lit > rows);
        empty.push(2 * dark > rows);
    }
    // The colour reaches from the left end to its last column; past it
    // the bar is empty.
    let reach = colour.iter().rposition(|c| *c).map_or(0, |i| i + 1);
    let stray = (0..colour.len())
        .filter(|&c| if c < reach { !colour[c] } else { !empty[c] })
        .count();
    if stray as f32 > f.max_stray * colour.len() as f32 {
        return None;
    }
    Some(reach as f64 / colour.len() as f64)
}

/// The rectangle `area` (in bar widths from the bar's top-left) stands
/// for, for the bar at `bar`.
fn area_of(bar: &Rect, area: [f64; 4]) -> Rect {
    let w = bar.width() as f64;
    Rect::new(
        bar.x0 + (area[0] * w).floor() as i32,
        bar.y0 + (area[1] * w).floor() as i32,
        bar.x0 + (area[2] * w).ceil() as i32,
        bar.y0 + (area[3] * w).ceil() as i32,
    )
}

/// Whether the red pointer stands under the bar at `bar`.
fn pointer_under(
    image: &BgrImage,
    bar: &Rect,
    scale: &PlateScale,
    pointer: &PointerProfile,
) -> bool {
    let area = area_of(bar, pointer.area);
    let mut count = 0;
    for y in area.y0..=area.y1 {
        for x in area.x0..=area.x1 {
            if !image.in_bounds(x, y) {
                continue;
            }
            let [r, g, b] = image.rgb(x, y);
            if r >= pointer.min_red && g <= pointer.max_other && b <= pointer.max_other {
                count += 1;
            }
        }
    }
    count >= scale.pointer_pixels.max(1)
}

/// Every washed-out plate's bar within `roi`, at every scale: on each
/// row, runs of the bar's colours with short gaps bridged, whose span is
/// about the bar's width and whose hue follows the gradient over most of
/// it; such runs on enough rows of one bar.
fn washed_plates(
    image: &BgrImage,
    roi: Rect,
    p: &PlateProfile,
    scratch: &mut PlateScratch,
) -> Vec<(Rect, usize)> {
    let run = &p.run;
    let (iw, ih) = (image.width(), image.height());
    let Some(roi) = roi.intersect(&Rect::new(0, 0, iw - 1, ih - 1)) else {
        return Vec::new();
    };
    let scales: Vec<(usize, &PlateScale, Vec<f32>)> = p
        .scales
        .iter()
        .enumerate()
        .filter(|(_, s)| s.bar[0] >= 2 && s.bar[1] >= 1)
        .filter_map(|(i, s)| column_hues(run, s.bar[0]).map(|h| (i, s, h)))
        .collect();
    let Some((lowest, highest)) = scales
        .iter()
        .flat_map(|(_, _, h)| h.iter().copied())
        .fold(None, |acc: Option<(f32, f32)>, h| {
            Some(acc.map_or((h, h), |(lo, hi)| (lo.min(h), hi.max(h))))
        })
    else {
        return Vec::new();
    };
    let (lowest, highest) = (lowest - run.hue_tolerance, highest + run.hue_tolerance);
    let mut hits: Vec<Vec<(i32, i32)>> = vec![Vec::new(); scales.len()];
    for y in roi.y0..=roi.y1 {
        scratch.mask.clear();
        scratch.mask.extend((roi.x0..=roi.x1).map(|x| {
            let rgb = image.rgb(x, y);
            chroma(rgb) >= run.min_chroma
                && hue(rgb).is_some_and(|hh| hh >= lowest && hh <= highest)
        }));
        let pieces = runs_of(&scratch.mask, roi.x0);
        for (k, (_, scale, hues)) in scales.iter().enumerate() {
            let [w, _] = scale.bar;
            let tolerance = scale.width_tolerance;
            let gap = (run.max_gap * w as f32).round() as i32;
            let shortest = (run.min_span * w as f32).ceil() as i32;
            let mut spans: Vec<(i32, i32)> = Vec::new();
            for &(start, end) in &pieces {
                match spans.last_mut() {
                    Some(last) if start - last.1 <= gap => last.1 = end,
                    _ => spans.push((start, end)),
                }
            }
            for (start, end) in spans {
                let span = end - start;
                if span < shortest || span > w + tolerance {
                    continue;
                }
                // The bar starts at the run's left end or ends at its
                // right, whichever end a creature in front has not hidden.
                let matching = |x0: i32| {
                    (0..w)
                        .filter(|&c| {
                            on_gradient(
                                image.rgb(x0 + c, y),
                                hues[c as usize],
                                run.min_chroma,
                                run.hue_tolerance,
                            )
                        })
                        .count()
                };
                let best = (start - tolerance..=start + tolerance)
                    .chain(end - w - tolerance..=end - w + tolerance)
                    .filter(|x0| *x0 >= 0 && x0 + w <= iw)
                    .map(|x0| (x0, matching(x0)))
                    .max_by_key(|&(x0, n)| (n, std::cmp::Reverse(x0)));
                if let Some((x0, n)) = best {
                    if n as f32 >= run.share * w as f32 {
                        hits[k].push((x0, y));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    for (k, (index, scale, _)) in scales.iter().enumerate() {
        let [w, h] = scale.bar;
        // Rows of one bar: the same left end, within the bar's height.
        let mut bars: Vec<(i32, i32, usize)> = Vec::new();
        for &(x0, y) in &hits[k] {
            match bars
                .iter_mut()
                .find(|(bx, by, _)| (x0 - *bx).abs() <= scale.width_tolerance && y - *by < h)
            {
                Some(bar) => bar.2 += 1,
                None => bars.push((x0, y, 1)),
            }
        }
        found.extend(
            bars.into_iter()
                .filter(|(_, _, rows)| *rows >= scale.min_rows.max(1))
                .map(|(x0, y0, _)| (Rect::new(x0, y0, x0 + w - 1, y0 + h - 1), *index)),
        );
    }
    one_scale(found, |f| *f)
}

/// The solid plates with the red pointer among `framed`, as plates: the
/// locked one when there is exactly one. Plates are drawn behind the
/// client's windows, so when more than one has the pointer, those lying
/// inside a window (whatever draws them is the window's) are set aside
/// first; if more than one is still left the lock cannot be told and none
/// is locked.
fn lock(image: &BgrImage, framed: &[Framed], profile: &UiProfile) -> Vec<Plate> {
    let mut pointed: Vec<usize> = (0..framed.len()).filter(|i| framed[*i].pointer).collect();
    if pointed.len() > 1 {
        let windows: Vec<Rect> = find_windows(image, &profile.chrome)
            .into_iter()
            .map(|w| w.rect)
            .collect();
        pointed.retain(|i| {
            let bar = framed[*i].bar;
            !windows.iter().any(|w| w.intersect(&bar) == Some(bar))
        });
    }
    framed
        .iter()
        .enumerate()
        .map(|(i, f)| Plate {
            bar: f.bar,
            scale: f.scale,
            kind: if pointed.len() == 1 && pointed[0] == i {
                PlateKind::Locked { fill: f.fill }
            } else {
                PlateKind::Unlocked
            },
        })
        .collect()
}

/// Every nameplate's health bar within `roi`, top to bottom: the locked
/// plate with its fill, and the others. At most one plate is locked (see
/// [`find_locked_plate`]).
pub fn find_plates_in(image: &BgrImage, roi: Rect, profile: &UiProfile) -> Vec<Plate> {
    let p = &profile.plates;
    let mut scratch = PlateScratch::default();
    let framed = framed_plates(image, roi, p, &mut scratch);
    let mut plates = lock(image, &framed, profile);
    // A solid plate's full bar is also a colour run; it is the solid
    // plate already found.
    for (bar, scale) in washed_plates(image, roi, p, &mut scratch) {
        if !framed
            .iter()
            .any(|f| outer(&f.bar, &p.scales[f.scale]).intersect(&bar).is_some())
        {
            plates.push(Plate {
                bar,
                scale,
                kind: PlateKind::Unlocked,
            });
        }
    }
    plates.sort_by_key(|plate| (plate.bar.y0, plate.bar.x0));
    plates
}

/// Every nameplate's health bar in `image`, top to bottom.
pub fn find_plates(image: &BgrImage, profile: &UiProfile) -> Vec<Plate> {
    find_plates_in(
        image,
        Rect::new(0, 0, image.width() - 1, image.height() - 1),
        profile,
    )
}

/// The locked plate in `image`, if exactly one plate is drawn solid with
/// the red pointer under it (after setting aside any inside a window when
/// there are more). The search a watcher runs every second: it looks only
/// for solid plates, row by row with integer comparisons, and reuses
/// `scratch`.
pub fn find_locked_plate(
    image: &BgrImage,
    profile: &UiProfile,
    scratch: &mut PlateScratch,
) -> Option<Plate> {
    let whole = Rect::new(0, 0, image.width() - 1, image.height() - 1);
    let framed = framed_plates(image, whole, &profile.plates, scratch);
    lock(image, &framed, profile)
        .into_iter()
        .find(Plate::is_locked)
}

/// Why the level or the name of the locked plate was not read. Every
/// variant is a refusal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlateTextError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("no line of lettering stands over the bar")]
    NotFound,
    #[error("more than one line of lettering stands over the bar")]
    Ambiguous,
    #[error("the lettering reaches the edge of the area searched, so it may be cut")]
    Clipped,
    #[error("a window or another plate meets the lettering")]
    Covered,
    #[error("the level read as {raw:?}, which is not a level")]
    Invalid { raw: String },
    #[error("the level read as {raw:?}, but its lettering is not as wide as that")]
    WrongWidth { raw: String },
    #[error("the level did not read, so where the name starts is not certain")]
    LevelUnread,
    #[error("the lettering after the level is not on the level's line")]
    Misaligned,
}

/// The locked plate, read.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateRead {
    /// The bar's coloured part, filled or not.
    pub bar: Rect,
    /// The creature's health: the share of the bar its colour fills.
    pub fill: f64,
    /// The level, validated, or why it was refused.
    pub level: Result<u32, PlateTextError>,
    /// The name as the recogniser read it (it is resolved against the
    /// known creatures elsewhere), or why it was not read.
    pub name: Result<String, PlateTextError>,
}

/// The line of lettering over a bar: its pieces, left to right, and
/// their bounds.
struct Line {
    rect: Rect,
    pieces: Vec<Rect>,
}

/// The union of two rectangles.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// The pixels a piece of lettering may join its neighbour across, for a
/// bar of `width`.
fn line_gap(width: i32, line: &LineProfile) -> i32 {
    (line.gap * width as f64).ceil() as i32
}

/// The line of lettering over `bar`: the runs above it chained left to
/// right across gaps no wider than the profile's, the one chain that
/// overlaps the bar's columns. Refused when no chain or more than one
/// does, or when the chain reaches the edge of the area searched.
fn line_over(
    image: &BgrImage,
    bar: &Rect,
    scale: &PlateScale,
    windows: &[Rect],
    p: &PlateProfile,
) -> Result<Line, PlateTextError> {
    let wanted = area_of(bar, p.line.area);
    let area = wanted
        .intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1))
        .ok_or(PlateTextError::NotFound)?;
    let mut runs: Vec<Rect> = scenery_runs(image, area, &scale.lettering, &scale.stroke, windows)
        .into_iter()
        .map(|run| run.rect)
        .collect();
    runs.sort_by_key(|r| r.x0);
    let gap = line_gap(bar.width(), &p.line);
    let mut chains: Vec<Line> = Vec::new();
    for run in runs {
        match chains.last_mut() {
            Some(chain) if run.x0 - chain.rect.x1 <= gap => {
                chain.rect = union(chain.rect, run);
                chain.pieces.push(run);
            }
            _ => chains.push(Line {
                rect: run,
                pieces: vec![run],
            }),
        }
    }
    let mut over: Vec<Line> = chains
        .into_iter()
        .filter(|c| c.rect.x1 >= bar.x0 && c.rect.x0 <= bar.x1)
        .collect();
    let line = match over.len() {
        0 => {
            return Err(if windows.iter().any(|w| w.intersect(&wanted).is_some()) {
                PlateTextError::Covered
            } else {
                PlateTextError::NotFound
            })
        }
        1 => over.remove(0),
        _ => return Err(PlateTextError::Ambiguous),
    };
    // A further word could lie past the area's end (or the screen's) when
    // the line stops within joining distance of it.
    let r = line.rect;
    if r.x0 - gap <= area.x0 || r.y0 <= area.y0 || r.x1 + gap >= area.x1 || r.y1 >= area.y1 {
        return Err(PlateTextError::Clipped);
    }
    Ok(line)
}

/// Whether a row of `rect` holds `run` pixels of window panel in a row:
/// the client's windows are opaque, so a window in front of the plate's
/// line shows its panel there. Looking only where the line lies is all the
/// plate's read needs, where finding every window on the screen costs
/// many times the rest of the read.
fn panel_in(image: &BgrImage, rect: Rect, chrome: &ChromeProfile, run: i32) -> bool {
    let Some(rect) = rect.intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1)) else {
        return false;
    };
    (rect.y0..=rect.y1).any(|y| {
        let mut length = 0;
        (rect.x0..=rect.x1).any(|x| {
            let i = ((y * image.width() + x) * 3) as usize;
            if close(&image.data, i, chrome.panel_rgb, chrome.panel_tolerance) {
                length += 1;
            } else {
                length = 0;
            }
            length >= run.max(1)
        })
    })
}

/// Whether `raw`, read where a level stands, looks like one misread: it
/// holds a digit, or it starts with the level's label and no lower-case
/// letter follows the label (a word such as `Lava` does not).
fn reads_as_level(raw: &str, label: &str) -> bool {
    let raw = raw.trim();
    raw.chars().any(|c| c.is_ascii_digit())
        || raw
            .strip_prefix(label)
            .is_some_and(|rest| !rest.starts_with(char::is_lowercase))
}

/// The level read from the line's first piece.
fn read_level(
    image: &BgrImage,
    bar: &Rect,
    piece: Rect,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<u32, PlateTextError> {
    let p = &profile.plates;
    let (raw, _) =
        read_run(image, piece, &profile.read, recogniser).ok_or(PlateTextError::NoRecogniser)?;
    let Some(parsed) = labelled_integer(&raw, &p.level.label, p.level.digits) else {
        return Err(PlateTextError::Invalid { raw });
    };
    let Some(level) = parsed
        .number
        .and_then(|n| u32::try_from(n as u64).ok().filter(|v| *v as f64 == n))
    else {
        return Err(PlateTextError::Invalid { raw });
    };
    // A glyph dropped or doubled leaves a level that still reads; the
    // lettering's width shows it.
    let glyphs = parsed.text.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let pitch = p.level.pitch * bar.width() as f64;
    let measured = piece.width() as f64 / pitch;
    if (measured - glyphs).abs() > p.level.pitch_tolerance {
        return Err(PlateTextError::WrongWidth { raw });
    }
    Ok(level)
}

/// Read the line over the locked plate at `plate`: its level, and its
/// name. `others` are the other plates found near it.
fn read_line(
    image: &BgrImage,
    plate: &Plate,
    windows: &[Rect],
    others: &[Plate],
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> (Result<u32, PlateTextError>, Result<String, PlateTextError>) {
    let p = &profile.plates;
    let scale = &p.scales[plate.scale];
    let refused = |e: PlateTextError| (Err(e.clone()), Err(e));
    let line = match line_over(image, &plate.bar, scale, windows, p) {
        Ok(line) => line,
        Err(e) => return refused(e),
    };
    // A window or another plate meeting the line, or the stretch past
    // its end where it could go on, may hide or join part of it.
    let gap = line_gap(plate.bar.width(), &p.line);
    let reach = scale.stroke.reach;
    let guard = Rect::new(
        line.rect.x0 - gap,
        line.rect.y0 - reach,
        line.rect.x1 + gap,
        line.rect.y1 + reach,
    );
    let panel = (p.line.panel_run * plate.bar.width() as f64).ceil() as i32;
    if windows.iter().any(|w| w.intersect(&guard).is_some())
        || panel_in(image, guard.padded(reach), &profile.chrome, panel)
    {
        return refused(PlateTextError::Covered);
    }
    for other in others.iter().filter(|o| o.bar != plate.bar) {
        let other_scale = &p.scales[other.scale];
        let mut theirs = vec![outer(&other.bar, other_scale)];
        if let Ok(their_line) = line_over(image, &other.bar, other_scale, windows, p) {
            theirs.push(their_line.rect.padded(reach));
        }
        if theirs.iter().any(|r| r.intersect(&guard).is_some()) {
            return refused(PlateTextError::Covered);
        }
    }
    let level_piece = line.pieces[0];
    let level = read_level(image, &plate.bar, level_piece, profile, recogniser);
    // A misread level still marks where the name starts when its piece is
    // as wide as a level and reads like one (the label, or a digit);
    // otherwise the split is in doubt: with no level on the line, the
    // first piece is the name's first word, and the rest of the name may
    // be another creature's.
    let pitches = level_piece.width() as f64 / (p.level.pitch * plate.bar.width() as f64);
    let level_sized = pitches >= p.level.glyphs[0] - p.level.pitch_tolerance
        && pitches <= p.level.glyphs[1] + p.level.pitch_tolerance;
    match &level {
        Ok(_) => {}
        Err(PlateTextError::Invalid { raw } | PlateTextError::WrongWidth { raw })
            if level_sized && reads_as_level(raw, &p.level.label) => {}
        Err(_) => return (level, Err(PlateTextError::LevelUnread)),
    }
    let Some(name) = line.pieces[1..].iter().copied().reduce(union) else {
        return (level, Err(PlateTextError::NotFound));
    };
    let height = level_piece.height() as f64;
    let aligned = ((name.y0 - level_piece.y0) as f64).abs() <= p.line.alignment * height
        && name.y1 >= level_piece.y1
        && ((name.y1 - level_piece.y1) as f64) <= p.line.descent * height;
    if !aligned {
        return (level, Err(PlateTextError::Misaligned));
    }
    let name = match read_run(image, name, &profile.read, recogniser) {
        None => Err(PlateTextError::NoRecogniser),
        Some((raw, _)) if raw.trim().is_empty() => Err(PlateTextError::NotFound),
        Some((raw, _)) => Ok(raw.trim().to_string()),
    };
    (level, name)
}

/// Read `plate` when it is the locked one: its fill, and its level and
/// name. `windows` are any of the client's windows the caller has found
/// (lettering inside them is not the scene's); whether or not any are
/// given, a window's panel where the line lies refuses it. `others` are
/// every other plate found near it. `None` for a plate that is not locked.
pub fn read_plate(
    image: &BgrImage,
    plate: &Plate,
    windows: &[Rect],
    others: &[Plate],
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Option<PlateRead> {
    let PlateKind::Locked { fill } = plate.kind else {
        return None;
    };
    let (level, name) = read_line(image, plate, windows, others, profile, recogniser);
    Some(PlateRead {
        bar: plate.bar,
        fill,
        level,
        name,
    })
}

/// The part of the frame where another plate could meet the locked
/// plate's line: the area searched for the line, widened by how far
/// another plate's line reaches from its own bar, and down to where such
/// a bar can stand.
fn neighbourhood(plate: &Plate, p: &PlateProfile) -> Rect {
    let line = area_of(&plate.bar, p.line.area);
    let (w, h) = (plate.bar.width(), plate.bar.height());
    let [left, top, right, _] = p.line.area;
    let wide = |share: f64| (share * w as f64).ceil() as i32;
    // Another bar whose line reaches the area stands at most the line's
    // reach to either side and its height below; it is searched whole,
    // with its frame.
    Rect::new(
        line.x0 - wide(right),
        line.y0 - h,
        line.x1 + wide(-left) + w,
        line.y1 + wide(-top) + 2 * h,
    )
}

/// The plates other than `plate` that could meet its line: those found in
/// the part of the frame around it where another plate's bar or line can
/// reach the line's area.
pub fn plates_near(image: &BgrImage, plate: &Plate, profile: &UiProfile) -> Vec<Plate> {
    find_plates_in(image, neighbourhood(plate, &profile.plates), profile)
        .into_iter()
        .filter(|other| other.bar != plate.bar)
        .collect()
}

/// The locked plate in `image`, read: `None` when no plate is locked, or
/// the lock cannot be told (see [`find_locked_plate`]). Other plates are
/// looked for only around it, where they could meet its line, and windows
/// only where its line lies (see [`read_plate`]).
pub fn read_locked_plate(
    image: &BgrImage,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Option<PlateRead> {
    let mut scratch = PlateScratch::default();
    let locked = find_locked_plate(image, profile, &mut scratch)?;
    read_found_plate(image, &locked, profile, recogniser)
}

/// Read `locked`, a plate [`find_locked_plate`] found in `image`: the
/// second half of [`read_locked_plate`], for a watcher that keeps its own
/// search buffers and measures the search and the reading apart.
pub fn read_found_plate(
    image: &BgrImage,
    locked: &Plate,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Option<PlateRead> {
    let others = plates_near(image, locked, profile);
    read_plate(image, locked, &[], &others, profile, recogniser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{
        blank, bundled_profile, draw_plate_bar, draw_window, fill, scenery, LineAlign,
        PlateDrawing, PlateGeometry, PlateLook, PlatePointer, RadarScene, PLATE_DEFAULT,
    };

    fn plates_in(image: &BgrImage) -> Vec<Plate> {
        find_plates(image, &bundled_profile())
    }

    fn bar_at(at: (i32, i32)) -> Rect {
        let (w, h) = PLATE_DEFAULT.bar;
        Rect::new(at.0, at.1, at.0 + w - 1, at.1 + h - 1)
    }

    fn plate(at: (i32, i32), kind: PlateKind) -> Plate {
        Plate {
            bar: bar_at(at),
            scale: 0,
            kind,
        }
    }

    #[test]
    fn a_locked_plate_is_found_at_any_health_and_its_fill_measured() {
        let width = PLATE_DEFAULT.bar.0 as f64;
        for (k, (at, fill)) in [
            ((60, 60), 1.0),
            ((700, 400), 0.75),
            ((300, 600), 0.4),
            ((900, 120), 0.1),
            ((500, 300), 0.05),
            ((120, 450), 1.0 / width),
            ((800, 650), 0.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut image = scenery(1100, 720, 40 + k as u64);
            draw_plate_bar(&mut image, &PlateDrawing::locked(at, fill));
            let drawn = (fill * width).round() / width;
            let want = plate(at, PlateKind::Locked { fill: drawn });
            assert_eq!(plates_in(&image), vec![want], "fill {fill}");
            let mut scratch = PlateScratch::default();
            assert_eq!(
                find_locked_plate(&image, &bundled_profile(), &mut scratch),
                Some(want)
            );
        }
    }

    #[test]
    fn washed_plates_are_found_and_never_locked() {
        let mut image = scenery(1100, 720, 3);
        let at = [(80, 100), (400, 130), (700, 500)];
        for (k, at) in at.iter().enumerate() {
            let pointer = if k == 1 {
                PlatePointer::White
            } else {
                PlatePointer::Absent
            };
            draw_plate_bar(
                &mut image,
                &PlateDrawing {
                    pointer,
                    ..PlateDrawing::washed(*at)
                },
            );
        }
        let locked = (600, 300);
        draw_plate_bar(&mut image, &PlateDrawing::locked(locked, 0.5));
        let mut want: Vec<Plate> = at
            .iter()
            .map(|at| plate(*at, PlateKind::Unlocked))
            .collect();
        want.push(plate(locked, PlateKind::Locked { fill: 0.5 }));
        want.sort_by_key(|p| (p.bar.y0, p.bar.x0));
        assert_eq!(plates_in(&image), want);
        // The search for the lock alone finds only the lock.
        let mut scratch = PlateScratch::default();
        assert_eq!(
            find_locked_plate(&image, &bundled_profile(), &mut scratch),
            Some(plate(locked, PlateKind::Locked { fill: 0.5 }))
        );
    }

    #[test]
    fn a_creature_standing_in_front_of_a_bar_does_not_hide_it() {
        let mut image = scenery(900, 500, 5);
        let at = (300, 200);
        draw_plate_bar(&mut image, &PlateDrawing::washed(at));
        // A dark leg across two of the bar's rows and a stretch of it.
        fill(
            &mut image,
            Rect::new(at.0 + 40, at.1 - 20, at.0 + 49, at.1 + 1),
            [70, 60, 50],
        );
        fill(
            &mut image,
            Rect::new(at.0 + 120, at.1 + 2, at.0 + 126, at.1 + 40),
            [70, 60, 50],
        );
        let found = plates_in(&image);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, PlateKind::Unlocked);
        assert_eq!((found[0].bar.x0, found[0].bar.x1), (at.0, at.0 + 163));
    }

    #[test]
    fn a_solid_plate_without_the_red_pointer_is_not_the_lock() {
        for pointer in [PlatePointer::Absent, PlatePointer::White] {
            let mut image = scenery(800, 500, 6);
            let at = (200, 200);
            draw_plate_bar(
                &mut image,
                &PlateDrawing {
                    pointer,
                    ..PlateDrawing::locked(at, 0.6)
                },
            );
            assert_eq!(plates_in(&image), vec![plate(at, PlateKind::Unlocked)]);
        }
    }

    #[test]
    fn two_plates_that_both_look_locked_leave_the_lock_untold() {
        let mut image = scenery(900, 600, 7);
        draw_plate_bar(&mut image, &PlateDrawing::locked((100, 100), 0.8));
        draw_plate_bar(&mut image, &PlateDrawing::locked((500, 400), 0.2));
        let found = plates_in(&image);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.kind == PlateKind::Unlocked));
        let mut scratch = PlateScratch::default();
        assert_eq!(
            find_locked_plate(&image, &bundled_profile(), &mut scratch),
            None
        );
    }

    #[test]
    fn a_plate_like_drawing_inside_a_window_does_not_take_the_lock_away() {
        // A window that draws something exactly like a locked plate: the
        // client draws plates behind windows, so it is set aside.
        let mut image = scenery(1200, 700, 8);
        let window = Rect::new(600, 100, 1150, 600);
        draw_window(&mut image, window);
        draw_plate_bar(&mut image, &PlateDrawing::locked((800, 300), 0.7));
        draw_plate_bar(&mut image, &PlateDrawing::locked((100, 300), 0.4));
        let mut scratch = PlateScratch::default();
        assert_eq!(
            find_locked_plate(&image, &bundled_profile(), &mut scratch),
            Some(plate(
                (100, 300),
                PlateKind::Locked {
                    fill: (0.4f64 * 164.0).round() / 164.0
                }
            ))
        );
    }

    #[test]
    fn a_plate_cut_by_the_screen_edge_is_not_reported() {
        for (at, look) in [
            ((-20, 100), PlateLook::Solid),
            ((560, 100), PlateLook::Solid),
            ((-30, 300), PlateLook::Washed),
            ((570, 300), PlateLook::Washed),
        ] {
            let mut image = scenery(700, 400, 8);
            draw_plate_bar(
                &mut image,
                &PlateDrawing {
                    look,
                    ..PlateDrawing::locked(at, 1.0)
                },
            );
            assert_eq!(plates_in(&image), Vec::new(), "{at:?}");
        }
    }

    /// A run of `width` columns whose colour goes from `from` to `to` in
    /// hue (degrees), at full saturation.
    fn hue_run(image: &mut BgrImage, (x0, y0): (i32, i32), width: i32, (from, to): (f32, f32)) {
        for c in 0..width {
            let h = from + (to - from) * c as f32 / (width - 1) as f32;
            let rgb = if h <= 60.0 {
                [250, (250.0 * h.max(0.0) / 60.0) as u8, 0]
            } else {
                [(250.0 * (120.0 - h) / 60.0) as u8, 250, 0]
            };
            fill(image, Rect::new(x0 + c, y0, x0 + c, y0 + 5), rgb);
        }
    }

    #[test]
    fn bars_and_pictures_that_are_not_plates_are_never_reported() {
        let mut image = scenery(1400, 900, 9);
        // The player's own health bar, red, and the tool bar beside it.
        fill(&mut image, Rect::new(300, 820, 660, 845), [200, 30, 40]);
        fill(&mut image, Rect::new(680, 820, 980, 845), [235, 150, 40]);
        // Red-to-green runs of the wrong width, and one of the right width
        // whose colours run the other way.
        hue_run(&mut image, (100, 100), 100, (0.0, 105.0));
        hue_run(&mut image, (100, 200), 230, (0.0, 105.0));
        hue_run(&mut image, (100, 300), 164, (105.0, 0.0));
        // A window with green progress bars and a dark panel, and pictures
        // of items in it.
        let window = Rect::new(500, 80, 1300, 700);
        draw_window(&mut image, window);
        for k in 0..5 {
            fill(
                &mut image,
                Rect::new(560, 160 + 40 * k, 724, 166 + 40 * k),
                [70, 200, 60],
            );
        }
        for (k, rgb) in [[220, 60, 30], [240, 200, 40], [90, 200, 70]]
            .iter()
            .enumerate()
        {
            fill(
                &mut image,
                Rect::new(800 + 60 * k as i32, 400, 840 + 60 * k as i32, 440),
                *rgb,
            );
        }
        // A dark box of the plate's size with nothing of its bevel.
        let edge = Rect::new(100, 500, 269, 511);
        fill(&mut image, edge, [3, 7, 5]);
        fill(&mut image, edge.padded(-1), [6, 11, 8]);
        assert_eq!(plates_in(&image), Vec::new());
    }

    #[test]
    fn a_profile_with_no_gradient_or_no_scale_finds_nothing() {
        let mut image = scenery(600, 400, 10);
        draw_plate_bar(&mut image, &PlateDrawing::locked((100, 100), 0.5));
        let mut profile = bundled_profile();
        profile.plates.run.gradient.clear();
        assert_eq!(find_plates(&image, &profile), Vec::new());
        let mut profile = bundled_profile();
        profile.plates.scales.clear();
        assert_eq!(find_plates(&image, &profile), Vec::new());
        assert_eq!(
            find_plates(&blank(0, 0, [0, 0, 0]), &bundled_profile()),
            Vec::new()
        );
    }

    /// The bundled profile with a second scale: a bar 205 by 8 inside a
    /// one-pixel frame and three rows of bevel, the lettering scaled alike.
    fn two_scales() -> (UiProfile, PlateGeometry) {
        let mut profile = bundled_profile();
        let mut larger = profile.plates.scales[0].clone();
        larger.bar = [205, 8];
        larger.bevel_rows = 3;
        larger.min_rows = 3;
        larger.pointer_pixels = 8;
        larger.lettering.run_height = [8, 38];
        profile.plates.scales.push(larger);
        (
            profile,
            PlateGeometry {
                bar: (205, 8),
                frame: 1,
                bevel: 3,
            },
        )
    }

    #[test]
    fn each_scale_finds_its_own_plates_and_never_another_scales() {
        let (profile, larger) = two_scales();
        let mut image = scenery(1300, 800, 11);
        let small_locked = (100, 100);
        let large_washed = (500, 200);
        let large_locked = (300, 500);
        let small_washed = (900, 600);
        draw_plate_bar(&mut image, &PlateDrawing::locked(small_locked, 0.5));
        draw_plate_bar(
            &mut image,
            &PlateDrawing {
                geometry: larger,
                ..PlateDrawing::washed(large_washed)
            },
        );
        draw_plate_bar(
            &mut image,
            &PlateDrawing {
                geometry: larger,
                pointer: PlatePointer::Absent,
                ..PlateDrawing::locked(large_locked, 0.6)
            },
        );
        draw_plate_bar(&mut image, &PlateDrawing::washed(small_washed));
        let large = |at: (i32, i32), kind| Plate {
            bar: Rect::new(at.0, at.1, at.0 + 204, at.1 + 7),
            scale: 1,
            kind,
        };
        assert_eq!(
            find_plates(&image, &profile),
            vec![
                plate(small_locked, PlateKind::Locked { fill: 0.5 }),
                large(large_washed, PlateKind::Unlocked),
                large(large_locked, PlateKind::Unlocked),
                plate(small_washed, PlateKind::Unlocked),
            ]
        );
        // With only one scale in the profile, the other's plates are not
        // there at all.
        assert_eq!(
            find_plates(&image, &bundled_profile()),
            vec![
                plate(small_locked, PlateKind::Locked { fill: 0.5 }),
                plate(small_washed, PlateKind::Unlocked),
            ]
        );
    }

    #[test]
    fn a_plate_at_the_second_scale_is_read_with_its_own_sizes() {
        let (profile, larger) = two_scales();
        let mut scene = RadarScene::sized(1100, 600, 12);
        let bar = scene.plate(
            &PlateDrawing {
                geometry: larger,
                ..PlateDrawing::locked((400, 300), 0.5)
            },
            "L25",
            "Zombie Fleshripper",
        );
        let read = read_locked_plate(&scene.image, &profile, &scene.recogniser())
            .expect("a plate is locked");
        assert_eq!(read.bar, bar);
        assert_eq!(
            (read.level, read.name),
            (Ok(25), Ok("Zombie Fleshripper".to_string()))
        );
    }

    // Reading the locked plate.

    fn locked_read(scene: &RadarScene) -> Option<PlateRead> {
        read_locked_plate(&scene.image, &bundled_profile(), &scene.recogniser())
    }

    /// A scene with washed plates of other creatures and the locked one,
    /// its level and name as given, its line set as `line` says.
    fn hunting_scene(level: &str, name: &str, line: LineAlign) -> (RadarScene, Rect) {
        let mut scene = RadarScene::sized(1200, 700, 21);
        let other = |at| PlateDrawing {
            line,
            ..PlateDrawing::washed(at)
        };
        scene.plate(&other((80, 150)), "L25", "Zombie Fleshripper");
        scene.plate(&other((90, 500)), "L3", "Combibo Young");
        let bar = scene.plate(
            &PlateDrawing {
                line,
                ..PlateDrawing::locked((560, 320), 0.35)
            },
            level,
            name,
        );
        (scene, bar)
    }

    #[test]
    fn the_line_is_read_wherever_it_starts_for_levels_and_names_of_any_length() {
        for line in [LineAlign::Left, LineAlign::Centred] {
            for (level, number, name) in [
                ("L3", 3, "Foul"),
                ("L25", 25, "Zombie Fleshripper"),
                ("L104", 104, "Experimental Holo-Warlock Old Alpha"),
                ("L7", 7, "Combibo Young"),
                ("L188", 188, "Atrox"),
            ] {
                let (scene, bar) = hunting_scene(level, name, line);
                let read = locked_read(&scene).expect("a plate is locked");
                assert_eq!(
                    read,
                    PlateRead {
                        bar,
                        fill: (0.35f64 * 164.0).round() / 164.0,
                        level: Ok(number),
                        name: Ok(name.to_string()),
                    },
                    "{line:?} {level} {name}"
                );
            }
        }
    }

    #[test]
    fn a_level_that_is_not_its_letter_and_digits_or_not_its_width_is_refused() {
        // Lettering for `L25`, read as something else: wrong in grammar,
        // or a glyph short or long of the ink. Its piece is as wide as a
        // level, so the name after it is still read.
        for (reads, want) in [
            ("125", PlateTextError::Invalid { raw: "125".into() }),
            ("L2S", PlateTextError::Invalid { raw: "L2S".into() }),
            ("L025", PlateTextError::Invalid { raw: "L025".into() }),
            ("L2", PlateTextError::WrongWidth { raw: "L2".into() }),
            ("L255", PlateTextError::WrongWidth { raw: "L255".into() }),
        ] {
            let (scene, _) = hunting_scene("L25", "Zombie Fleshripper", LineAlign::Left);
            let inner = scene.recogniser();
            let misreading = move |crop: &BgrImage| {
                inner(crop).map(|(text, c)| {
                    if text == "L25" {
                        (reads.to_string(), c)
                    } else {
                        (text, c)
                    }
                })
            };
            let read = read_locked_plate(&scene.image, &bundled_profile(), &misreading)
                .expect("a plate is locked");
            assert_eq!(read.level, Err(want), "{reads}");
            assert_eq!(read.name, Ok("Zombie Fleshripper".to_string()), "{reads}");
        }
    }

    #[test]
    fn a_line_without_its_level_does_not_give_the_rest_of_the_name_as_the_name() {
        // `Old Alpha Daikiba` with no level on the line: the first word sits
        // where a level would, as wide as one, and does not read as one.
        // `Alpha Daikiba` is another creature's plate, so the name is
        // refused with the level.
        for line in [LineAlign::Left, LineAlign::Centred] {
            let (scene, _) = hunting_scene("Old", "Alpha Daikiba", line);
            let read = locked_read(&scene).expect("a plate is locked");
            assert_eq!(
                (read.level, read.name),
                (
                    Err(PlateTextError::Invalid { raw: "Old".into() }),
                    Err(PlateTextError::LevelUnread)
                ),
                "{line:?}"
            );
        }
        assert!(reads_as_level("L2S", "L"));
        assert!(reads_as_level("125", "L"));
        assert!(reads_as_level("LZS", "L"));
        assert!(!reads_as_level("Lava", "L"));
        assert!(!reads_as_level("Old", "L"));
    }

    #[test]
    fn a_first_piece_too_wide_for_a_level_that_does_not_read_as_one_refuses_the_name_too() {
        // Lettering five glyphs wide where the level stands, read as no
        // level: whether it is the level at all is in doubt.
        let (scene, _) = hunting_scene("L2555", "Zombie Fleshripper", LineAlign::Left);
        let inner = scene.recogniser();
        let misreading = move |crop: &BgrImage| {
            inner(crop).map(|(text, c)| {
                if text == "L2555" {
                    ("Lz5".to_string(), c)
                } else {
                    (text, c)
                }
            })
        };
        let read = read_locked_plate(&scene.image, &bundled_profile(), &misreading)
            .expect("a plate is locked");
        assert_eq!(
            (read.level, read.name),
            (
                Err(PlateTextError::Invalid { raw: "Lz5".into() }),
                Err(PlateTextError::LevelUnread)
            )
        );
    }

    #[test]
    fn lettering_under_a_window_or_off_the_screen_is_refused() {
        // A window over the end of the name, and one over the level.
        for front in [
            |bar: Rect| Rect::new(bar.x0 + 120, bar.y0 - 300, bar.x0 + 500, bar.y0 - 12),
            |bar: Rect| Rect::new(bar.x0 - 300, bar.y0 - 300, bar.x0 + 15, bar.y0 - 12),
        ] {
            let (mut scene, bar) = hunting_scene("L25", "Zombie Fleshripper", LineAlign::Left);
            draw_window(&mut scene.image, front(bar));
            let read = locked_read(&scene).expect("a plate is locked");
            assert_eq!(
                (read.level, read.name),
                (Err(PlateTextError::Covered), Err(PlateTextError::Covered))
            );
        }
        // A name running off the screen's right edge, and a centred line
        // running off its left.
        let mut scene = RadarScene::sized(800, 500, 22);
        scene.plate(
            &PlateDrawing::locked((560, 300), 0.9),
            "L25",
            "Zombie Fleshripper Old Alpha",
        );
        assert_eq!(
            locked_read(&scene).expect("a plate is locked").name,
            Err(PlateTextError::Clipped)
        );
        let mut scene = RadarScene::sized(800, 500, 23);
        scene.plate(
            &PlateDrawing {
                line: LineAlign::Centred,
                ..PlateDrawing::locked((40, 300), 0.9)
            },
            "L25",
            "Experimental Holo-Warlock Old Alpha",
        );
        assert_eq!(
            locked_read(&scene).expect("a plate is locked").name,
            Err(PlateTextError::Clipped)
        );
    }

    #[test]
    fn another_plate_meeting_the_line_refuses_it() {
        let profile = bundled_profile();
        // Another plate's bar across the end of the name (its last word
        // hidden), another plate's line running on from the locked one's,
        // and another plate above with its status icon over the start of
        // the name, its own line cut by the top of the area searched.
        let cases: [(&str, (i32, i32), bool, PlateTextError); 3] = [
            (
                "its bar over the name's end",
                (660, 290),
                false,
                PlateTextError::Covered,
            ),
            (
                "its line running on from the line",
                (727, 330),
                true,
                PlateTextError::Covered,
            ),
            (
                "its icon over the name",
                (387, 304),
                true,
                PlateTextError::Clipped,
            ),
        ];
        for (what, at, with_text, why) in cases {
            let mut scene = RadarScene::sized(1200, 700, 24);
            let bar = scene.plate(
                &PlateDrawing::locked((560, 320), 0.35),
                "L25",
                "Zombie Fleshripper",
            );
            if with_text {
                scene.plate(&PlateDrawing::washed(at), "L25", "Zombie Fleshripper");
            } else {
                draw_plate_bar(&mut scene.image, &PlateDrawing::washed(at));
            }
            let plates = find_plates(&scene.image, &profile);
            assert!(
                plates.iter().any(|p| p.bar.x0 == at.0),
                "{what}: {plates:?}"
            );
            let read = read_locked_plate(&scene.image, &profile, &scene.recogniser())
                .expect("a plate is locked");
            assert_eq!(read.bar, bar);
            assert_eq!(
                (read.level, read.name),
                (Err(why.clone()), Err(why)),
                "{what}"
            );
        }
    }

    #[test]
    fn a_plate_with_no_lettering_over_it_reads_nothing() {
        let mut scene = RadarScene::sized(900, 500, 25);
        let bar = draw_plate_bar(&mut scene.image, &PlateDrawing::locked((300, 300), 0.5));
        let read = locked_read(&scene).expect("a plate is locked");
        assert_eq!(
            read,
            PlateRead {
                bar,
                fill: 0.5,
                level: Err(PlateTextError::NotFound),
                name: Err(PlateTextError::NotFound),
            }
        );
    }

    #[test]
    fn without_a_lock_or_a_recogniser_nothing_is_read() {
        let mut scene = RadarScene::sized(900, 500, 26);
        scene.plate(
            &PlateDrawing::washed((300, 300)),
            "L25",
            "Zombie Fleshripper",
        );
        assert_eq!(locked_read(&scene), None);
        let (scene, _) = hunting_scene("L25", "Zombie Fleshripper", LineAlign::Left);
        let none = |_: &BgrImage| -> Option<(String, f64)> { None };
        let read =
            read_locked_plate(&scene.image, &bundled_profile(), &none).expect("a plate is locked");
        assert_eq!(
            (read.level, read.name),
            (
                Err(PlateTextError::NoRecogniser),
                Err(PlateTextError::LevelUnread)
            )
        );
    }
}
