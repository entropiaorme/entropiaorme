//! Finding the game's own windows in a frame.
//!
//! Every window the client draws shares one frame: the panel's dark
//! fill, a thin lighter ridge just inside the outer edge, then the fill
//! again. That profile is exact enough to find a window from its
//! border alone, wherever it sits and whatever size it has been dragged
//! to, without knowing which window it is. The ridge is drawn in device
//! pixels, so its thickness tracks the interface scale; the locator
//! tries the thicknesses the profile lists and keeps the one the frame
//! answers to.
//!
//! A frame can hold several windows, overlapping. The locator recovers
//! them front to back: a rectangle is accepted when every stretch of
//! its border that is not under an already accepted window shows the
//! ridge. A side that is wholly covered is reported as hidden, and a
//! side that runs off the screen as beyond it, so a reader can refuse a
//! window whose extent is a guess.

use serde::Deserialize;

use super::geometry::{chroma, near, Pixels, Rect};

/// What the client's window frame looks like. Every number is a
/// measurement of the client's rendering and lives in the profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChromeProfile {
    /// The panel fill, `[r, g, b]`.
    pub panel_rgb: [u8; 3],
    /// Per-channel tolerance when matching the fill.
    pub panel_tolerance: u8,
    /// Brightness range of the ridge.
    pub ridge_lum: [u8; 2],
    /// Largest channel spread a ridge pixel may have: it is a grey.
    pub ridge_max_chroma: u8,
    /// On a two-pixel ridge the inner pixel is brighter by at least this.
    pub ridge_step: u8,
    /// Panel pixels that follow the ridge inwards: a ridge pixel is a
    /// border's outer edge only where this many pixels of fill back it.
    pub ridge_backing: i32,
    /// Ridge thicknesses to try, in order of preference.
    pub ridge_widths: Vec<u8>,
    /// Of those, the ones that only a frame scaled by the desktop shows.
    pub scaled_ridge_widths: Vec<u8>,
    /// Shortest stretch of ridge that counts as part of a border.
    pub min_edge_run: i32,
    /// Breaks in a border no longer than this are bridged.
    pub edge_gap: i32,
    /// Smallest window worth reporting, `[width, height]`.
    pub min_window: [i32; 2],
    /// The same for a rectangle whose title marker is seen: what shows
    /// of a window mostly under another, or mostly off the frame.
    pub min_window_marked: [i32; 2],
    /// Share of a side's uncovered length that must show the ridge.
    pub side_support: f32,
    /// Rectangles are ranked by their weakest side's share of ridge in
    /// steps of one part in this many, so sides that differ by less tie
    /// and the next measure decides.
    pub support_steps: i32,
    /// Share of a window's border that must be in view for nothing to
    /// count as covering it.
    pub full_view_share: f32,
    /// How near the frame's edge a neighbouring side must reach for a
    /// side to be taken as running off the frame.
    pub boundary_reach: i32,
    /// Fewest ridge pixels, over all sides, that make a window.
    pub min_evidence: i32,
    /// The same floor for a rectangle whose title marker is seen.
    pub min_evidence_marked: i32,
    /// A side with fewer uncovered pixels than this is treated as hidden.
    pub min_visible_side: i32,
    /// How far from a corner a side's ridge may begin: the client cuts
    /// its corners.
    pub corner_reach: i32,
    /// The title marker: a warm bar just inside the top-left corner.
    pub marker: MarkerProfile,
}

/// The title marker's search box (relative to the window's corner) and
/// what counts as its colour.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerProfile {
    /// `[x0, y0, x1, y1]` offsets from the window's top-left corner.
    pub search: [i32; 4],
    /// A marker pixel has red at least this.
    pub min_red: u8,
    /// And red exceeding blue by at least this.
    pub min_red_over_blue: u8,
    /// Fewest such pixels that make a marker.
    pub min_pixels: i32,
    /// The bar is narrower than this,
    pub max_width: i32,
    /// and at least this tall.
    pub min_height: i32,
}

/// How much of one side of a window the frame shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The ridge was seen along the side.
    Seen,
    /// Another window covers the whole side: its position is inferred.
    Hidden,
    /// The window runs off the frame on this side.
    BeyondFrame,
}

/// One window recovered from a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundWindow {
    /// The window's outer rectangle, clipped to the frame.
    pub rect: Rect,
    pub left: Side,
    pub top: Side,
    pub right: Side,
    pub bottom: Side,
    /// Share of the border, within the frame, not under another window.
    pub visible_border: f32,
    /// Whether the title marker was seen at the top-left corner.
    pub marker: bool,
    /// The ridge thickness the frame answered to.
    pub ridge_px: u8,
    /// The frame answered only to the blurred ridge of a picture the
    /// desktop has scaled: edges found in it are not pixel-exact.
    pub scaled_frame: bool,
}

impl FoundWindow {
    /// Whether the rectangle is a measurement on every side. A window
    /// with a hidden side has a guessed extent and must not be read by
    /// position.
    pub fn extent_certain(&self) -> bool {
        !self.scaled_frame
            && ![self.left, self.top, self.right, self.bottom].contains(&Side::Hidden)
    }

    /// Whether nothing covers the window and none of it is off the frame.
    pub fn fully_visible(&self, chrome: &ChromeProfile) -> bool {
        [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|side| *side == Side::Seen)
            && self.visible_border >= chrome.full_view_share
    }
}

const LEFT: u8 = 0b0001;
const RIGHT: u8 = 0b0010;
const TOP: u8 = 0b0100;
const BOTTOM: u8 = 0b1000;

/// A rectangle's sides, indexed left, top, right, bottom.
const SIDES: usize = 4; // not layout: a rectangle has four sides

/// Per-pixel flags: which border edges have their outer pixel here.
struct EdgeMap {
    w: i32,
    h: i32,
    flags: Vec<u8>,
}

impl EdgeMap {
    fn at(&self, x: i32, y: i32, bit: u8) -> bool {
        self.flags[(y * self.w + x) as usize] & bit != 0
    }
}

fn edge_map<P: Pixels>(image: &P, chrome: &ChromeProfile, ridge_px: u8) -> EdgeMap {
    let (w, h) = (image.width(), image.height());
    let n = (w * h) as usize;
    // 1 = panel fill, 2 = ridge-coloured; brightness kept for the step test.
    let mut class = vec![0u8; n];
    let mut lum = vec![0u8; n];
    for y in 0..h {
        for x in 0..w {
            let rgb = image.rgb(x, y);
            let l = ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8;
            let i = (y * w + x) as usize;
            lum[i] = l;
            if near(rgb, chrome.panel_rgb, chrome.panel_tolerance) {
                class[i] = 1;
            } else if l >= chrome.ridge_lum[0]
                && l <= chrome.ridge_lum[1]
                && chroma(rgb) <= chrome.ridge_max_chroma
            {
                class[i] = 2;
            }
        }
    }
    let get = |x: i32, y: i32| -> (u8, u8) {
        if x < 0 || y < 0 || x >= w || y >= h {
            (0, 0)
        } else {
            let i = (y * w + x) as usize;
            (class[i], lum[i])
        }
    };
    let mut flags = vec![0u8; n];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if class[i] != 1 {
                continue;
            }
            for (bit, dx, dy) in [(LEFT, 1, 0), (RIGHT, -1, 0), (TOP, 0, 1), (BOTTOM, 0, -1)] {
                let p = |k: i32| get(x + dx * k, y + dy * k);
                let fill = |k: i32| p(k).0 == 1;
                let ridge = |k: i32| p(k).0 == 2;
                if !ridge(1) {
                    continue;
                }
                // A ridge `width` pixels thick (its first pixel is known
                // already), then the fill that backs it.
                let backed = |width: i32| {
                    (2..=width).all(ridge) && (1..=chrome.ridge_backing).all(|k| fill(width + k))
                };
                let hit = match ridge_px {
                    1 => backed(1),
                    2 => backed(2) && p(2).1 >= p(1).1.saturating_add(chrome.ridge_step),
                    // A scaled-up frame blurs the ridge across one to
                    // `ridge_px` pixels; accept any of those shapes.
                    _ => (1..=i32::from(ridge_px)).any(backed),
                };
                if hit {
                    flags[i] |= bit;
                }
            }
        }
    }
    EdgeMap { w, h, flags }
}

/// Columns (for a vertical edge) or rows (for a horizontal one) that
/// carry at least one long enough stretch of the edge.
fn edge_lines(map: &EdgeMap, bit: u8, vertical: bool, chrome: &ChromeProfile) -> Vec<i32> {
    let (outer, inner) = if vertical {
        (map.w, map.h)
    } else {
        (map.h, map.w)
    };
    let mut lines = Vec::new();
    for o in 0..outer {
        let mut run_start: Option<i32> = None;
        let mut last = -1i32;
        let mut found = false;
        for i in 0..inner {
            let set = if vertical {
                map.at(o, i, bit)
            } else {
                map.at(i, o, bit)
            };
            if !set {
                continue;
            }
            match run_start {
                Some(start) if i - last <= chrome.edge_gap => {
                    if i - start + 1 >= chrome.min_edge_run {
                        found = true;
                        break;
                    }
                }
                _ => run_start = Some(i),
            }
            last = i;
        }
        if found {
            lines.push(o);
        }
    }
    lines
}

/// The ridge seen along one side of a candidate, over the part of the
/// side no accepted window covers: `(ridge pixels, uncovered pixels)`.
fn side_support(
    map: &EdgeMap,
    covered: &[bool],
    bit: u8,
    fixed: i32,
    from: i32,
    to: i32,
    vertical: bool,
) -> (i32, i32) {
    let (mut hit, mut free) = (0, 0);
    for i in from..=to {
        let (x, y) = if vertical { (fixed, i) } else { (i, fixed) };
        if covered[(y * map.w + x) as usize] {
            continue;
        }
        free += 1;
        if map.at(x, y, bit) {
            hit += 1;
        }
    }
    (hit, free)
}

fn has_marker<P: Pixels>(image: &P, x0: i32, y0: i32, marker: &MarkerProfile) -> bool {
    // The marker is a thin upright bar. Counting warm pixels alone
    // would also accept an item icon or a coloured tab that happens to
    // sit in the same corner of a pane, so its shape is checked too.
    marker_extent(image, x0, y0, marker).is_some_and(|(count, bounds)| {
        count >= marker.min_pixels
            && bounds.x1 - bounds.x0 < marker.max_width
            && bounds.height() >= marker.min_height
    })
}

/// The pixels of the marker's colour in its search box at the corner
/// `(x0, y0)`: how many, and their bounds in the frame. `None` when the
/// box holds none.
fn marker_extent<P: Pixels>(
    image: &P,
    x0: i32,
    y0: i32,
    marker: &MarkerProfile,
) -> Option<(i32, Rect)> {
    let mut count = 0;
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for y in (y0 + marker.search[1])..=(y0 + marker.search[3]) {
        for x in (x0 + marker.search[0])..=(x0 + marker.search[2]) {
            if !image.in_bounds(x, y) {
                continue;
            }
            let [r, _, b] = image.rgb(x, y);
            if r >= marker.min_red && r.saturating_sub(b) >= marker.min_red_over_blue {
                count += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    (count > 0).then(|| (count, Rect::new(min_x, min_y, max_x, max_y)))
}

/// The ridge along one side of a window, as it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct RidgeMeasurement {
    /// How many pixels thick the ridge is along most of the side.
    pub thickness: u8,
    /// The share of the side that shows a ridge of that thickness; the
    /// rest is covered, or drawn otherwise.
    pub support: f32,
    /// The darkest and the brightest pixel of the ridge.
    pub lum: [u8; 2],
    /// The most colourful pixel of the ridge.
    pub max_chroma: u8,
    /// The least rise in brightness from the ridge's outer pixel to the
    /// next one in, for a ridge at least two pixels thick.
    pub step: Option<u8>,
    /// The share of the side whose ridge the profile's limits admit: its
    /// pixels within the ridge's brightness and chroma, and for a ridge of
    /// two pixels the step between them. The finder wants the profile's
    /// side support of a side, not all of it: where the ridge meets the
    /// title bar's furniture a few positions stand outside any limit.
    pub admitted: f32,
}

/// The title marker as it is drawn: its bounds relative to the window's
/// top-left corner, and how many pixels of it have the marker's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerMeasurement {
    pub rect: Rect,
    pub pixels: i32,
}

/// A window's frame as it is drawn in one frame: what the profile's
/// chrome entries are measured from.
#[derive(Debug, Clone, PartialEq)]
pub struct ChromeMeasurement {
    /// The panel's fill at the window's outer edge.
    pub panel_rgb: [u8; 3],
    /// How far any channel of the fill, where a ridge backs it, stands
    /// from that colour.
    pub panel_spread: u8,
    /// The ridge along the left, top, right and bottom sides; `None` for
    /// a side that shows no ridge (it is covered, or off the frame).
    pub sides: [Option<RidgeMeasurement>; SIDES],
    /// The title marker, when its colour is seen at the corner.
    pub marker: Option<MarkerMeasurement>,
}

/// The middle value of `values`, which it reorders.
fn middle(values: &mut [u8]) -> Option<u8> {
    values.sort_unstable();
    values.get(values.len() / 2).copied()
}

/// Measure the frame of the window at `rect`, looking `depth` pixels in
/// from each side.
///
/// The frame is measured against itself, not against the profile: the
/// fill is the colour most of the outer edge has, and the ridge is
/// whatever stands between that edge and the fill behind it. So a frame
/// the client has redrawn, which [`find_windows`] no longer finds, is
/// still measured once its rectangle is given. Two things are taken from
/// `chrome`: how far a pixel may stray from the fill and still be the
/// fill, and what counts as the marker's colour. The ends of each side,
/// where the ridge turns the corner, are left out. `None` when none of
/// the rectangle's edge lies in the frame. Beside what is drawn, each
/// side says how much of it the limits in `chrome` admit.
pub fn measure_chrome<P: Pixels>(
    image: &P,
    rect: Rect,
    chrome: &ChromeProfile,
    depth: i32,
) -> Option<ChromeMeasurement> {
    // Every position along each side: its pixel on the outer edge, and
    // the step that leads inward from it.
    let along = |side: usize| -> Vec<(i32, i32, i32, i32)> {
        let upright = matches!(side, 0 | 2);
        let (from, to) = if upright {
            (rect.y0 + depth, rect.y1 - depth)
        } else {
            (rect.x0 + depth, rect.x1 - depth)
        };
        (from..=to)
            .map(|i| match side {
                0 => (rect.x0, i, 1, 0),
                1 => (i, rect.y0, 0, 1),
                2 => (rect.x1, i, -1, 0),
                _ => (i, rect.y1, 0, -1),
            })
            .filter(|&(x, y, dx, dy)| {
                image.in_bounds(x, y) && image.in_bounds(x + dx * depth, y + dy * depth)
            })
            .collect()
    };
    let sides: Vec<Vec<(i32, i32, i32, i32)>> = (0..SIDES).map(along).collect();
    let mut channels: [Vec<u8>; 3] = Default::default();
    for &(x, y, _, _) in sides.iter().flatten() {
        let rgb = image.rgb(x, y);
        for (channel, value) in channels.iter_mut().zip(rgb) {
            channel.push(value);
        }
    }
    let [r, g, b] = &mut channels;
    let panel_rgb = [middle(r)?, middle(g)?, middle(b)?];
    let is_fill = |x: i32, y: i32| near(image.rgb(x, y), panel_rgb, chrome.panel_tolerance);
    let mut panel_spread = 0u8;
    let mut measured: [Option<RidgeMeasurement>; SIDES] = Default::default();
    for (side, positions) in sides.iter().enumerate() {
        // The ridge's thickness at each position: the pixels between the
        // fill on the edge and the fill behind.
        let thickness: Vec<i32> = positions
            .iter()
            .map(|&(x, y, dx, dy)| {
                if !is_fill(x, y) {
                    return 0;
                }
                (1..=depth)
                    .find(|k| is_fill(x + dx * k, y + dy * k))
                    .map_or(0, |k| k - 1)
            })
            .collect();
        let Some(usual) = (1..depth)
            .max_by_key(|t| {
                (
                    thickness.iter().filter(|seen| *seen == t).count(),
                    std::cmp::Reverse(*t),
                )
            })
            .filter(|t| thickness.contains(t))
        else {
            continue;
        };
        let (mut lo, mut hi, mut max_chroma) = (u8::MAX, 0u8, 0u8);
        let mut step: Option<u8> = None;
        let (mut shown, mut admitted) = (0usize, 0usize);
        for (&(x, y, dx, dy), _) in positions
            .iter()
            .zip(&thickness)
            .filter(|(_, seen)| **seen == usual)
        {
            shown += 1;
            for (mine, panel) in image.rgb(x, y).iter().zip(panel_rgb) {
                panel_spread = panel_spread.max(mine.abs_diff(panel));
            }
            let mut within = true;
            for k in 1..=usual {
                let (px, py) = (x + dx * k, y + dy * k);
                let (lum, colour) = (image.lum(px, py), chroma(image.rgb(px, py)));
                lo = lo.min(lum);
                hi = hi.max(lum);
                max_chroma = max_chroma.max(colour);
                within &= lum >= chrome.ridge_lum[0]
                    && lum <= chrome.ridge_lum[1]
                    && colour <= chrome.ridge_max_chroma;
            }
            if usual >= 2 {
                let rise = image
                    .lum(x + dx * 2, y + dy * 2)
                    .saturating_sub(image.lum(x + dx, y + dy));
                step = Some(step.map_or(rise, |least| least.min(rise)));
                within &= usual > 2 || rise >= chrome.ridge_step;
            }
            admitted += within as usize;
        }
        measured[side] = Some(RidgeMeasurement {
            thickness: u8::try_from(usual).unwrap_or(u8::MAX),
            support: shown as f32 / positions.len().max(1) as f32,
            lum: [lo, hi],
            max_chroma,
            step,
            admitted: admitted as f32 / positions.len().max(1) as f32,
        });
    }
    let marker = marker_extent(image, rect.x0, rect.y0, &chrome.marker).map(|(pixels, bounds)| {
        MarkerMeasurement {
            rect: bounds.offset(-rect.x0, -rect.y0),
            pixels,
        }
    });
    Some(ChromeMeasurement {
        panel_rgb,
        panel_spread,
        sides: measured,
        marker,
    })
}

#[derive(Clone, Copy)]
struct Candidate {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    /// Sides that are the frame's own boundary: left, top, right, bottom.
    beyond: [bool; SIDES],
}

struct Scored {
    candidate: Candidate,
    seen: usize,
    /// Lowest share of ridge seen on any uncovered side, in the
    /// profile's support steps.
    weakest_side: i32,
    evidence: i32,
    visible: i32,
    hidden: [bool; SIDES],
    marker: bool,
}

fn find_with_ridge<P: Pixels>(image: &P, chrome: &ChromeProfile, ridge_px: u8) -> Vec<FoundWindow> {
    let map = edge_map(image, chrome, ridge_px);
    let (w, h) = (map.w, map.h);
    let mut lefts: Vec<Option<i32>> = edge_lines(&map, LEFT, true, chrome)
        .into_iter()
        .map(Some)
        .collect();
    let mut rights: Vec<Option<i32>> = edge_lines(&map, RIGHT, true, chrome)
        .into_iter()
        .map(Some)
        .collect();
    let mut tops: Vec<Option<i32>> = edge_lines(&map, TOP, false, chrome)
        .into_iter()
        .map(Some)
        .collect();
    let mut bottoms: Vec<Option<i32>> = edge_lines(&map, BOTTOM, false, chrome)
        .into_iter()
        .map(Some)
        .collect();
    // A side may also be the frame's own boundary, for a window dragged
    // partly off the screen. A maximised window has its border ON the
    // boundary instead; both readings of such a side yield the same
    // rectangle, and the one that saw the ridge wins below because it
    // has more sides seen.
    for list in [&mut lefts, &mut rights, &mut tops, &mut bottoms] {
        list.push(None);
    }

    let mut candidates = Vec::new();
    for &left in &lefts {
        for &right in &rights {
            let (x0, x1) = (left.unwrap_or(0), right.unwrap_or(w - 1));
            if x1 - x0 + 1 < chrome.min_window_marked[0] {
                continue;
            }
            for &top in &tops {
                for &bottom in &bottoms {
                    let (y0, y1) = (top.unwrap_or(0), bottom.unwrap_or(h - 1));
                    if y1 - y0 + 1 < chrome.min_window_marked[1] {
                        continue;
                    }
                    let beyond = [
                        left.is_none(),
                        top.is_none(),
                        right.is_none(),
                        bottom.is_none(),
                    ];
                    if beyond.iter().filter(|b| **b).count() > 2 {
                        continue;
                    }
                    candidates.push(Candidate {
                        x0,
                        y0,
                        x1,
                        y1,
                        beyond,
                    });
                }
            }
        }
    }

    let mut covered = vec![false; (w * h) as usize];
    let mut accepted: Vec<FoundWindow> = Vec::new();
    loop {
        let mut qualifying: Vec<Scored> = Vec::new();
        for &c in &candidates {
            if accepted
                .iter()
                .any(|a| a.rect.x0 == c.x0 && a.rect.y0 == c.y0)
            {
                continue;
            }
            let sides = [
                (!c.beyond[0]).then(|| side_support(&map, &covered, LEFT, c.x0, c.y0, c.y1, true)),
                (!c.beyond[1]).then(|| side_support(&map, &covered, TOP, c.y0, c.x0, c.x1, false)),
                (!c.beyond[2]).then(|| side_support(&map, &covered, RIGHT, c.x1, c.y0, c.y1, true)),
                (!c.beyond[3])
                    .then(|| side_support(&map, &covered, BOTTOM, c.y1, c.x0, c.x1, false)),
            ];
            let mut seen = 0;
            let mut evidence = 0;
            let mut visible = 0;
            let mut hidden = [false; SIDES];
            let mut weak = false;
            let mut weakest_side = chrome.support_steps;
            let mut side_hits = [0i32; SIDES];
            for (i, side) in sides.iter().enumerate() {
                let Some((hit, free)) = side else { continue };
                if *free < chrome.min_visible_side {
                    hidden[i] = true;
                    continue;
                }
                if (*hit as f32) < chrome.side_support * (*free as f32) {
                    weak = true;
                    break;
                }
                seen += 1;
                side_hits[i] = *hit;
                weakest_side = weakest_side.min(hit * chrome.support_steps / free);
                evidence += hit;
                visible += free;
            }
            if weak || seen < 2 {
                continue;
            }
            // The sides seen must be seen to belong to one window: two
            // sides are known to only where the ridge is seen turning
            // the corner between them. A side that joins none of the
            // others may be another window's edge that happens to line
            // up, with the place where the two would part hidden under a
            // third; it is kept as a guess, not a measurement.
            {
                let reach = chrome.corner_reach;
                let free = |px: i32, py: i32| {
                    px >= 0 && py >= 0 && px < w && py < h && !covered[(py * w + px) as usize]
                };
                let meets =
                    |vertical_bit: u8, x: i32, horizontal_bit: u8, y: i32, dy: i32, dx: i32| {
                        (0..reach)
                            .any(|k| free(x, y + dy * k) && map.at(x, y + dy * k, vertical_bit))
                            && (0..reach).any(|k| {
                                free(x + dx * k, y) && map.at(x + dx * k, y, horizontal_bit)
                            })
                    };
                let is_seen = |i: usize, hidden: &[bool; SIDES]| !c.beyond[i] && !hidden[i];
                let corners = [
                    (0, 1, meets(LEFT, c.x0, TOP, c.y0, 1, 1)),
                    (1, 2, meets(RIGHT, c.x1, TOP, c.y0, 1, -1)),
                    (2, 3, meets(RIGHT, c.x1, BOTTOM, c.y1, -1, -1)),
                    (3, 0, meets(LEFT, c.x0, BOTTOM, c.y1, -1, 1)),
                ];
                let mut group = [0usize, 1, 2, 3];
                for (a, b, joined) in corners {
                    if joined && is_seen(a, &hidden) && is_seen(b, &hidden) {
                        let (keep, merge) = (group[a], group[b]);
                        for g in group.iter_mut() {
                            if *g == merge {
                                *g = keep;
                            }
                        }
                    }
                }
                let weight = |g: usize| -> i32 {
                    (0..SIDES)
                        .filter(|&i| group[i] == g && is_seen(i, &hidden))
                        .map(|i| side_hits[i])
                        .sum()
                };
                let main = (0..SIDES)
                    .filter(|&i| is_seen(i, &hidden))
                    .map(|i| group[i])
                    .max_by_key(|&g| weight(g))
                    .expect("at least two sides are seen");
                let joined_sides = (0..SIDES)
                    .filter(|&i| is_seen(i, &hidden) && group[i] == main)
                    .count();
                // Two opposite sides with both others off the frame have
                // no corner on the frame to be seen at: a window spanning
                // the frame.
                let spans_frame =
                    seen == 2 && ((c.beyond[0] && c.beyond[2]) || (c.beyond[1] && c.beyond[3]));
                if !spans_frame {
                    if joined_sides < 2 {
                        continue;
                    }
                    for i in 0..SIDES {
                        if is_seen(i, &hidden) && group[i] != main {
                            hidden[i] = true;
                            seen -= 1;
                            evidence -= side_hits[i];
                        }
                    }
                }
            }
            // A side taken as the frame's boundary must have a
            // neighbouring side actually reaching that boundary.
            let reaches = |bit: u8, fixed: i32, at: i32, vertical: bool| {
                (0..chrome.boundary_reach).any(|k| {
                    let i = if at == 0 { k } else { at - k };
                    let (x, y) = if vertical { (fixed, i) } else { (i, fixed) };
                    map.at(x, y, bit)
                })
            };
            let boundary_ok = [
                !c.beyond[0] || reaches(TOP, c.y0, 0, false) || reaches(BOTTOM, c.y1, 0, false),
                !c.beyond[1] || reaches(LEFT, c.x0, 0, true) || reaches(RIGHT, c.x1, 0, true),
                !c.beyond[2]
                    || reaches(TOP, c.y0, w - 1, false)
                    || reaches(BOTTOM, c.y1, w - 1, false),
                !c.beyond[3]
                    || reaches(LEFT, c.x0, h - 1, true)
                    || reaches(RIGHT, c.x1, h - 1, true),
            ];
            if boundary_ok.contains(&false) {
                continue;
            }
            // Where the top-left corner is on the frame and uncovered,
            // the title marker must be there: an inner rule line has
            // the border's profile but no marker beneath it.
            let corner_on_frame = !c.beyond[0] && !c.beyond[1];
            let corner_free = corner_on_frame && {
                let m = &chrome.marker.search;
                let mut free = true;
                'scan: for y in (c.y0 + m[1])..=(c.y0 + m[3]).min(h - 1) {
                    for x in (c.x0 + m[0])..=(c.x0 + m[2]).min(w - 1) {
                        if covered[(y * w + x) as usize] {
                            free = false;
                            break 'scan;
                        }
                    }
                }
                free
            };
            let marker = corner_on_frame && has_marker(image, c.x0, c.y0, &chrome.marker);
            if corner_free && !marker {
                continue;
            }
            // A window showing its own title marker has proved itself
            // with that, and needs less of its border in view than a
            // rectangle known only by its edges. Without this a window
            // mostly under another is passed over, and its visible edges
            // are left to be mistaken for parts of the windows behind it.
            let (floor, smallest) = if marker {
                (chrome.min_evidence_marked, chrome.min_window_marked)
            } else {
                (chrome.min_evidence, chrome.min_window)
            };
            if evidence < floor || c.x1 - c.x0 + 1 < smallest[0] || c.y1 - c.y0 + 1 < smallest[1] {
                continue;
            }
            qualifying.push(Scored {
                candidate: c,
                seen,
                weakest_side,
                evidence,
                visible,
                hidden,
                marker,
            });
        }
        // A rectangle stitched from two windows' edges also clears the
        // support floor when they happen to line up, but always with a
        // side part of which is bare; a real window shows its ridge
        // along all of every uncovered side. So the cleanest rectangle
        // goes first, and once it is accepted its edges are spent.
        // A window drawn wholly inside another's rectangle is in front
        // of it, so it must be taken first: accepting the outer one
        // would mark the inner one's whole border as covered.
        let encloses_another = |outer: &Scored| {
            let o = &outer.candidate;
            qualifying.iter().any(|inner| {
                let i = &inner.candidate;
                (i.x0, i.y0, i.x1, i.y1) != (o.x0, o.y0, o.x1, o.y1)
                    && i.x0 >= o.x0
                    && i.y0 >= o.y0
                    && i.x1 <= o.x1
                    && i.y1 <= o.y1
                    // Only a rectangle that has proved itself a window by
                    // its marker: a pane's rule under a clipped corner has
                    // not.
                    && inner.marker
            })
        };
        let Some(best) = qualifying.iter().max_by_key(|s| {
            // On a tie the smaller rectangle wins: growing past an
            // edge has to be vouched for, below.
            let c = &s.candidate;
            let area = (c.x1 - c.x0 + 1) as i64 * (c.y1 - c.y0 + 1) as i64;
            (
                !encloses_another(s),
                s.seen,
                s.weakest_side,
                s.evidence,
                -area,
            )
        }) else {
            break;
        };
        // A pane inside a window is ruled with the same profile as the
        // window's own border, so the first rectangle found can stop
        // short at a pane's edge. Wherever the window's sides visibly
        // carry on past that edge, grow to the next rectangle that
        // shares the other three sides (including one that runs off
        // the frame), until nothing vouches for going further.
        let carries_on = |from: &Candidate, to: &Candidate, downward: bool| {
            // One side showing its ridge along the added stretch is
            // enough: the other may be under another window, or broken
            // by the title bar's own furniture.
            let sides = if downward {
                [(LEFT, to.x0, to.beyond[0]), (RIGHT, to.x1, to.beyond[2])]
            } else {
                [(TOP, to.y0, to.beyond[1]), (BOTTOM, to.y1, to.beyond[3])]
            };
            let (start, end) = if downward {
                (from.y1 + 1, to.y1)
            } else {
                (from.x1 + 1, to.x1)
            };
            sides.iter().any(|&(bit, fixed, beyond)| {
                if beyond {
                    return false;
                }
                let (hit, free) = side_support(&map, &covered, bit, fixed, start, end, downward);
                free >= chrome.min_visible_side && hit as f32 >= chrome.side_support * free as f32
            })
        };
        let mut chosen = best;
        loop {
            let c = chosen.candidate;
            let deeper = qualifying
                .iter()
                .filter(|s| {
                    let k = &s.candidate;
                    k.x0 == c.x0
                        && k.y0 == c.y0
                        && k.x1 == c.x1
                        && k.y1 > c.y1
                        && carries_on(&c, k, true)
                })
                .max_by_key(|s| (s.candidate.y1, s.seen));
            let wider = qualifying
                .iter()
                .filter(|s| {
                    let k = &s.candidate;
                    k.x0 == c.x0
                        && k.y0 == c.y0
                        && k.y1 == c.y1
                        && k.x1 > c.x1
                        && carries_on(&c, k, false)
                })
                .max_by_key(|s| (s.candidate.x1, s.seen));
            match deeper.or(wider) {
                Some(next) => chosen = next,
                None => break,
            }
        }
        let c = chosen.candidate;
        let side = |i: usize| {
            if c.beyond[i] {
                Side::BeyondFrame
            } else if chosen.hidden[i] {
                Side::Hidden
            } else {
                Side::Seen
            }
        };
        let on_frame_border: i32 = [
            (!c.beyond[0]) as i32 * (c.y1 - c.y0 + 1),
            (!c.beyond[1]) as i32 * (c.x1 - c.x0 + 1),
            (!c.beyond[2]) as i32 * (c.y1 - c.y0 + 1),
            (!c.beyond[3]) as i32 * (c.x1 - c.x0 + 1),
        ]
        .iter()
        .sum();
        accepted.push(FoundWindow {
            rect: Rect::new(c.x0, c.y0, c.x1, c.y1),
            left: side(0),
            top: side(1),
            right: side(2),
            bottom: side(3),
            visible_border: chosen.visible as f32 / on_frame_border.max(1) as f32,
            marker: chosen.marker,
            ridge_px,
            scaled_frame: chrome.scaled_ridge_widths.contains(&ridge_px),
        });
        for y in c.y0..=c.y1 {
            let row = (y * w) as usize;
            covered[row + c.x0 as usize..=row + c.x1 as usize].fill(true);
        }
    }
    accepted
}

/// Every game window in `image`, front to back.
pub fn find_windows<P: Pixels>(image: &P, chrome: &ChromeProfile) -> Vec<FoundWindow> {
    let mut best: Vec<FoundWindow> = Vec::new();
    let mut best_evidence = 0;
    for (rank, &ridge_px) in chrome.ridge_widths.iter().enumerate() {
        let found = find_with_ridge(image, chrome, ridge_px);
        let evidence: i32 = found
            .iter()
            .filter(|f| f.marker || f.top == Side::BeyondFrame || f.left == Side::BeyondFrame)
            .map(|f| f.rect.width() + f.rect.height())
            .sum();
        if evidence > best_evidence {
            best_evidence = evidence;
            best = found;
        }
        // The preferred thickness answering at all settles it: a
        // thinner pattern also matches rule lines inside the windows.
        if rank == 0 && best_evidence > 0 {
            break;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::geometry::BgrImage;
    use crate::game_ui::test_support::{blank, bundled_profile, draw_window, fill, put, Lcg};

    fn scenery(w: usize, h: usize, seed: u64) -> BgrImage {
        let mut rng = Lcg(seed);
        let mut image = blank(w, h, [120, 110, 90]);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = rng.range(0, 255) as u8;
                put(&mut image, x, y, [v, v.wrapping_mul(3), v.wrapping_add(90)]);
            }
        }
        image
    }

    fn rects(found: &[FoundWindow]) -> Vec<Rect> {
        found.iter().map(|f| f.rect).collect()
    }

    #[test]
    fn a_frame_with_no_window_yields_none() {
        let chrome = bundled_profile().chrome;
        assert!(find_windows(&scenery(640, 400, 1), &chrome).is_empty());
        assert!(find_windows(&blank(640, 400, [8, 11, 11]), &chrome).is_empty());
    }

    #[test]
    fn a_lone_window_is_found_to_the_pixel() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 2);
        let rect = Rect::new(120, 80, 700, 480);
        draw_window(&mut image, rect);
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![rect]);
        assert!(found[0].fully_visible(&chrome));
        assert!(found[0].marker);
        assert_eq!(found[0].ridge_px, 2);
    }

    #[test]
    fn a_frame_is_measured_as_it_is_drawn() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 2);
        let rect = Rect::new(120, 80, 700, 480);
        draw_window(&mut image, rect);
        let got = measure_chrome(&image, rect, &chrome, 6).expect("the window is in the frame");
        assert_eq!((got.panel_rgb, got.panel_spread), ([8, 11, 11], 0));
        // The drawn ridge: an outer pixel of brightness 47 and an inner
        // one of 80, the more colourful of them by 8.
        let ridge = RidgeMeasurement {
            thickness: 2,
            support: 1.0,
            lum: [47, 80],
            max_chroma: 8,
            step: Some(33),
            admitted: 1.0,
        };
        assert_eq!(got.sides, [(); SIDES].map(|()| Some(ridge.clone())));
        assert_eq!(
            got.marker,
            Some(MarkerMeasurement {
                rect: Rect::new(25, 18, 29, 37),
                pixels: 100,
            })
        );
        // What it measures is what the finder's thresholds must admit.
        assert!(chrome.ridge_lum[0] <= ridge.lum[0] && ridge.lum[1] <= chrome.ridge_lum[1]);
        assert!(ridge.max_chroma <= chrome.ridge_max_chroma);
        assert!(ridge.step >= Some(chrome.ridge_step));
    }

    #[test]
    fn a_redrawn_frame_is_measured_though_it_is_no_longer_found() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 4);
        let rect = Rect::new(100, 60, 600, 400);
        // Another fill, a one-pixel ridge two pixels in, and no marker.
        let (panel, ridge) = ([30, 34, 52], [140, 150, 170]);
        fill(&mut image, rect, panel);
        let line = rect.padded(-2);
        for x in line.x0..=line.x1 {
            put(&mut image, x, line.y0, ridge);
            put(&mut image, x, line.y1, ridge);
        }
        for y in line.y0..=line.y1 {
            put(&mut image, line.x0, y, ridge);
            put(&mut image, line.x1, y, ridge);
        }
        assert!(find_windows(&image, &chrome).is_empty());
        let got = measure_chrome(&image, rect, &chrome, 6).expect("the window is in the frame");
        assert_eq!((got.panel_rgb, got.marker), (panel, None));
        // The edge is fill for two pixels, so the first is not a ridge's
        // outer pixel: nothing stands between it and the fill behind.
        assert_eq!(got.sides, [(); SIDES].map(|()| None));
        let got = measure_chrome(&image, rect.padded(-1), &chrome, 6).expect("in the frame");
        let want = RidgeMeasurement {
            thickness: 1,
            support: 1.0,
            lum: [153, 153],
            max_chroma: 30,
            step: None,
            // Brighter and more colourful than the profile's ridge.
            admitted: 0.0,
        };
        assert_eq!(got.sides, [(); SIDES].map(|()| Some(want.clone())));
    }

    #[test]
    fn a_covered_side_is_measured_only_where_it_shows() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(1200, 800, 3);
        let back = Rect::new(100, 100, 800, 600);
        draw_window(&mut image, back);
        // A window over the whole right side and half the bottom.
        draw_window(&mut image, Rect::new(450, 300, 1100, 700));
        let got = measure_chrome(&image, back, &chrome, 6).expect("the window is in the frame");
        let [left, top, right, bottom] = got.sides;
        assert_eq!(left.map(|s| (s.thickness, s.support)), Some((2, 1.0)));
        assert_eq!(top.map(|s| (s.thickness, s.support)), Some((2, 1.0)));
        assert_eq!(right.as_ref().map(|s| s.thickness), Some(2));
        assert!(right.is_some_and(|s| s.support < 0.5));
        assert!(bottom.is_some_and(|s| {
            s.thickness == 2 && s.support > 0.4 && s.support < 0.6 && s.admitted == s.support
        }));
        // A rectangle wholly off the frame measures nothing.
        assert_eq!(
            measure_chrome(&image, Rect::new(2000, 100, 2400, 500), &chrome, 6),
            None
        );
    }

    #[test]
    fn overlapping_windows_come_back_front_to_back() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(1200, 800, 3);
        let back = Rect::new(100, 100, 800, 600);
        let front = Rect::new(500, 300, 1100, 700);
        draw_window(&mut image, back);
        draw_window(&mut image, front);
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![front, back]);
        assert!(found[0].fully_visible(&chrome));
        assert!(!found[1].fully_visible(&chrome));
        assert!(found[1].extent_certain());
    }

    #[test]
    fn a_window_run_off_each_edge_is_found_and_says_so() {
        let chrome = bundled_profile().chrome;
        let cases = [
            (Rect::new(-200, 100, 400, 500), 0usize),
            (Rect::new(100, -150, 700, 400), 1),
            (Rect::new(500, 100, 1100, 500), 2),
            (Rect::new(100, 300, 700, 800), 3),
        ];
        for (rect, beyond) in cases {
            let mut image = scenery(900, 600, 4);
            draw_window(&mut image, rect);
            let found = find_windows(&image, &chrome);
            assert_eq!(found.len(), 1, "{rect:?}");
            let sides = [found[0].left, found[0].top, found[0].right, found[0].bottom];
            for (i, side) in sides.iter().enumerate() {
                let want = if i == beyond {
                    Side::BeyondFrame
                } else {
                    Side::Seen
                };
                assert_eq!(*side, want, "{rect:?} side {i}");
            }
            let clipped = Rect::new(
                rect.x0.max(0),
                rect.y0.max(0),
                rect.x1.min(899),
                rect.y1.min(599),
            );
            assert_eq!(found[0].rect, clipped);
        }
    }

    #[test]
    fn a_window_whose_side_is_wholly_covered_has_an_uncertain_extent() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(1200, 800, 5);
        let back = Rect::new(300, 200, 900, 600);
        // Covers the back window's whole left side.
        let front = Rect::new(100, 100, 500, 700);
        draw_window(&mut image, back);
        draw_window(&mut image, front);
        let found = find_windows(&image, &chrome);
        assert_eq!(found[0].rect, front);
        let behind = found
            .iter()
            .find(|f| f.rect.x1 == back.x1 && f.rect.y1 == back.y1);
        let behind = behind.expect("the covered window is still reported");
        assert_eq!(behind.left, Side::Hidden);
        assert!(!behind.extent_certain());
    }

    #[test]
    fn a_maximised_window_has_all_four_sides_seen() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 8);
        let rect = Rect::new(0, 0, 899, 599);
        draw_window(&mut image, rect);
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![rect]);
        assert!(found[0].fully_visible(&chrome), "{:?}", found[0]);
    }

    #[test]
    fn a_window_run_off_the_frame_is_still_found_over_a_maximised_one() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 9);
        let maximised = Rect::new(0, 0, 899, 599);
        let front = Rect::new(500, 350, 1100, 800);
        draw_window(&mut image, maximised);
        draw_window(&mut image, front);
        let found = find_windows(&image, &chrome);
        assert_eq!(found[0].rect, Rect::new(500, 350, 899, 599));
        assert_eq!(
            (found[0].right, found[0].bottom),
            (Side::BeyondFrame, Side::BeyondFrame)
        );
        let behind = found
            .iter()
            .find(|f| f.rect == maximised)
            .expect("the maximised window");
        assert_eq!((behind.left, behind.top), (Side::Seen, Side::Seen));
    }

    #[test]
    fn a_maximised_window_with_panes_still_has_its_sides_seen() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(900, 600, 10);
        let rect = Rect::new(0, 0, 899, 599);
        draw_window(&mut image, rect);
        for (inset, rgb) in [(0, [8u8, 11, 11]), (1, [45, 49, 49]), (2, [76, 82, 84])] {
            for x in rect.x0..=rect.x1 {
                put(&mut image, x, 450 - inset, rgb);
            }
        }
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![rect]);
        assert!(found[0].fully_visible(&chrome), "{:?}", found[0]);
    }

    /// Two windows whose tops line up, and a third partly showing: the
    /// first two's edges must not be stitched into one window.
    #[test]
    fn windows_with_their_tops_in_line_are_not_stitched_together() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(600, 420, 11);
        let (back, middle, front) = (
            Rect::new(5, 166, 361, 382),
            Rect::new(306, 338, 562, 556),
            Rect::new(354, 166, 567, 348),
        );
        for rect in [back, middle, front] {
            draw_window(&mut image, rect);
        }
        let found = find_windows(&image, &chrome);
        assert_eq!(found[0].rect, front);
        for f in found.iter().filter(|f| f.extent_certain()) {
            let clipped = Rect::new(middle.x0, middle.y0, middle.x1, 419);
            assert!(
                f.rect == front || f.rect == back || f.rect == clipped,
                "{:?} is no drawn window",
                f.rect
            );
        }
    }

    /// A pane inside a window is ruled like the window's own border;
    /// the window must still come back whole.
    #[test]
    fn inner_panes_do_not_cut_a_window_short() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(1000, 700, 6);
        let rect = Rect::new(100, 60, 900, 640);
        draw_window(&mut image, rect);
        // A pane ruled across the lower part and one down the right.
        for (inset, rgb) in [(0, [8u8, 11, 11]), (1, [45, 49, 49]), (2, [76, 82, 84])] {
            for x in rect.x0..=rect.x1 {
                put(&mut image, x, 500 - inset, rgb);
            }
            for y in rect.y0 + 80..=rect.y1 {
                put(&mut image, 700 - inset, y, rgb);
            }
        }
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![rect]);
    }

    #[test]
    fn a_pane_edge_near_the_frame_does_not_hide_that_the_window_runs_off_it() {
        let chrome = bundled_profile().chrome;
        let mut image = scenery(1000, 700, 7);
        let rect = Rect::new(100, 300, 800, 900);
        draw_window(&mut image, rect);
        for (inset, rgb) in [(0, [8u8, 11, 11]), (1, [45, 49, 49]), (2, [76, 82, 84])] {
            for x in rect.x0..=rect.x1 {
                put(&mut image, x, 650 - inset, rgb);
            }
        }
        let found = find_windows(&image, &chrome);
        assert_eq!(rects(&found), vec![Rect::new(100, 300, 800, 699)]);
        assert_eq!(found[0].bottom, Side::BeyondFrame);
    }

    /// Random layouts of one to three windows over noise. No rectangle
    /// may be reported that is not a drawn window, and a window nothing
    /// covers must come back exactly.
    #[test]
    fn random_layouts_never_produce_a_false_window() {
        let chrome = bundled_profile().chrome;
        let mut rng = Lcg(0x5eed);
        // A small frame keeps two hundred layouts quick in an unoptimised
        // build; the locator's cost is per pixel.
        let (w, h) = (600i32, 420i32);
        let mut layouts = 0;
        let mut certain = 0;
        for seed in 0..220u64 {
            let mut image = scenery(w as usize, h as usize, 100 + seed);
            let mut drawn: Vec<Rect> = Vec::new();
            for _ in 0..rng.range(1, 3) {
                let (ww, wh) = (rng.range(170, 480), rng.range(120, 340));
                let (x, y) = (rng.range(-120, w - 80), rng.range(-90, h - 60));
                let rect = Rect::new(x, y, x + ww, y + wh);
                draw_window(&mut image, rect);
                drawn.push(rect);
            }
            let clip =
                |r: &Rect| Rect::new(r.x0.max(0), r.y0.max(0), r.x1.min(w - 1), r.y1.min(h - 1));
            let found = find_windows(&image, &chrome);
            for f in found.iter().filter(|f| f.extent_certain()) {
                assert!(
                    drawn.iter().any(|d| clip(d) == f.rect),
                    "layout {seed}: {:?} is not a drawn window ({drawn:?})",
                    f.rect
                );
                certain += 1;
            }
            let top = drawn.last().expect("at least one window");
            let top_clipped = clip(top);
            // What of its border is on the frame: a window dragged so far
            // off that less ridge shows than the evidence floor asks for
            // is rightly not reported.
            let on_frame = (top.x0 >= 0) as i32 * top_clipped.height()
                + (top.x1 < w) as i32 * top_clipped.height()
                + (top.y0 >= 0) as i32 * top_clipped.width()
                + (top.y1 < h) as i32 * top_clipped.width();
            if top_clipped.width() >= chrome.min_window[0]
                && top_clipped.height() >= chrome.min_window[1]
                && on_frame >= chrome.min_evidence + 80
            {
                assert!(
                    found.iter().any(|f| f.rect == top_clipped),
                    "layout {seed}: the front window {top:?} was not found ({found:?})"
                );
            }
            layouts += 1;
        }
        println!("window layouts: {layouts} layouts, {certain} certain rectangles, 0 false");
    }
}
