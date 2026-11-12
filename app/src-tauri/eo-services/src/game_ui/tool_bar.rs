//! Finding the bar that names the tool the player holds, and reading it.
//!
//! Above the hotbar the client draws two panels side by side, the same
//! size: the player's own health bar, its fill red from the left as far as
//! the player's health reaches and dark beyond, and to its right the tool
//! bar, which names the tool in hand at its left and gives a counter
//! (ammunition or uses) at its right, in white lettering over a fill that
//! is tan when the tool is ready and dark while it reloads. Each panel is
//! framed alike: a dark outline one pixel thick along its top and bottom
//! and a few thick at its ends, inside a lighter frame.
//!
//! The interface can be moved, so nothing is found by where it usually
//! is. A panel is found by its outline: a dark row exactly a panel's width
//! long, ended by the dark columns of its ends, with a dark row exactly a
//! panel's height below it and lighter rows outside both. The health bar
//! is the panel whose fill starts red, or with no health left the health
//! bar's dark; the tool bar is looked for at the profile's pitch to its
//! right, level with it. A creature's nameplate, a window's panel and a
//! picture have neither the outline nor the health bar beside them. The
//! panels' pixel sizes differ with the interface scale, so the profile
//! lists them per scale, and a panel is taken only where exactly one
//! scale claims its outline's width: at a scale the profile does not
//! list, no bar is found.
//!
//! A window over the bar, or the edge of the frame, cuts a panel's
//! outline short. A part of a panel with one end whole and the other
//! ending where a window starts (or at the frame's edge) still places the
//! panel, from its whole end: the health bar's left part places the tool
//! bar beside it, and the tool bar's right part places the tool bar. Such
//! a bar is covered and never read. An outline broken where no window is
//! found (a pointer over it, say) leaves the bar unclear, never clear.
//!
//! The name is told from the fill by brightness alone: the lettering is
//! near white in every channel, where the fill below its top highlight is
//! not, whether tan or dark. That lettering, as a mask of pixels relative
//! to the bar, is what a watcher compares from one look to the next to
//! decide whether to read again. A name too long for the bar is cut short
//! with three dots; whether it was is told from those dots on the pixels,
//! not from the read, which turns them into one, two or three stops, an
//! ellipsis, or nothing.
//!
//! A bar whose name area is plain fill, no mark of any colour in it, is
//! taken as holding nothing (`Empty`); one whose name area holds marks
//! that are not the client's lettering is `Unclear`. No capture of the
//! client with nothing held has been seen, so `Empty` is unverified
//! against a real capture: the client may draw the bar differently then,
//! or not at all, which the reader gives as a health bar with no panel
//! beside it (`NoPanel`).

use serde::Deserialize;

use crate::game_ui::geometry::BgrImage;

use super::field::{label_key, read_run_within, Recogniser};
use super::geometry::{chroma, hue, near, Pixels, Rect};
use super::profile::{claiming, UiProfile};
use super::text::{runs_of_ink, TextProfile};
use super::window::WindowsNear;

/// What the health bar and the tool bar look like, as the profile
/// describes them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBarProfile {
    /// The panels' pixel sizes, one entry per interface scale measured.
    pub scales: Vec<ToolBarScale>,
    /// A pixel of a panel's outline is at most this in every channel.
    pub outline_max: u8,
    /// Least share of a row or column of the outline that must show its
    /// colour, and of the rows just outside it that must not, for a panel
    /// to be found.
    pub outline_share: f32,
    /// Least share of each of the tool bar's outline rows and end columns
    /// that must show its colour for the bar to be whole: anything
    /// crossing its outline leaves it short of this.
    pub whole_share: f32,
    /// Least share of the panel's middle row, between the outline's ends,
    /// that is not the outline's colour: the panel holds a fill.
    pub fill_share: f32,
    /// A part of a panel cut short by a window or the frame's edge counts
    /// when it runs at least this share of the panel's width.
    pub min_part: f64,
    /// The left end of the health bar's fill.
    pub health: HealthProfile,
    /// Where the tool's name stands, `[left, top, right, bottom]` as
    /// shares of the panel's width and height from its top-left corner
    /// (right and bottom exclusive): below the fill's top highlight, and
    /// clear of the counter.
    pub name_area: [f64; 4],
    /// Where the counter stands, in the same terms.
    pub counter_area: [f64; 4],
    /// The lettering as pixels.
    pub lettering: LetteringProfile,
    /// What plain fill in the name area looks like.
    pub plain: PlainProfile,
    /// The dots the client ends a name it cut short with.
    pub ellipsis: EllipsisProfile,
    /// The crop handed to the recogniser reaches at most this many pixels
    /// past the lettering (less where the read's own margin is less), and
    /// never past the name area: held to the name's rows, clear of the
    /// fill's highlight and the panel's edge, the recogniser reads every
    /// letter right, though it may drop the spaces between words.
    pub crop_margin: i32,
}

/// The panels' pixel sizes at one interface scale.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBarScale {
    /// The scale this entry is for, by the length in pixels of a panel's
    /// outline along its top (both ends included).
    pub width: [i32; 2],
    /// The outline's size, `[width, height]`, its top and bottom rows
    /// included.
    pub size: [i32; 2],
    /// How far right of the health bar's left end the tool bar's starts.
    pub pitch: i32,
    /// How many pixels the tool bar may stand from where the pitch puts
    /// it, either way and up or down.
    pub tolerance: i32,
    /// How the name's glyphs group into runs.
    pub text: TextProfile,
}

/// The left end of the health bar's fill: red as far as the player's
/// health reaches, the empty part's colour beyond.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthProfile {
    /// Where it is looked at, in the terms of `name_area`.
    pub area: [f64; 4],
    /// A pixel of the red has a hue within this range (degrees, red at
    /// zero),
    pub hue: [f32; 2],
    /// and at least this chroma.
    pub min_chroma: u8,
    /// The empty part's colour,
    pub empty_rgb: [u8; 3],
    /// and how far each channel may stand from it.
    pub empty_tolerance: u8,
    /// Least share of the area's pixels that are the red or the empty
    /// part's colour.
    pub share: f32,
}

/// The lettering as pixels.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LetteringProfile {
    /// A pixel of lettering is at least this in every channel.
    pub min_channel: u8,
    /// Fewest pixels of lettering in the name area for the bar to hold
    /// a name.
    pub min_pixels: usize,
    /// Two looks at the lettering are worth reading again between when
    /// more pixels than this differ: just above what differs between two
    /// looks at one name over one fill.
    pub max_differing: usize,
}

/// What plain fill in the name area looks like: smooth, its shading
/// changing a little from one pixel to the next and never stepping as the
/// edge of a mark does.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlainProfile {
    /// Most any channel changes from a pixel to the one left of it,
    pub across: u8,
    /// and to the one above it.
    pub down: u8,
    /// Most pixels that may change by more, for the area to be plain.
    pub max_marks: usize,
}

/// The dots the client ends a name it cut short with.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EllipsisProfile {
    /// How many there are.
    pub dots: usize,
    /// None is wider or taller than this many pixels.
    pub max_dot: i32,
}

/// How much of the tool bar is seen where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The whole panel, its outline unbroken, nothing over it.
    Clear,
    /// A window lies over part or all of it, or the frame's edge cuts it:
    /// it is placed, but not read.
    Covered,
    /// Its outline is there but broken where no window was found.
    Broken,
    /// No panel stands where the health bar places it, and nothing covers
    /// that place.
    Missing,
}

/// The tool bar found in a frame, or where it would stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolBar {
    /// The panel's outline, in the frame's coordinates.
    pub panel: Rect,
    /// Where the tool's name stands.
    pub name: Rect,
    /// Where the counter stands.
    pub counter: Rect,
    /// Which of the profile's scales it was found at.
    pub scale: usize,
    pub view: View,
}

/// What the tool bar shows, decided from its geometry and its pixels
/// before anything is recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarState {
    /// No bar is placed: no health bar, nor a tool panel with its health
    /// bar's place covered, is in view.
    Absent,
    /// A health bar with no panel beside it and nothing covering where the
    /// panel would stand. What the client shows with nothing held has not
    /// been seen; this may be it.
    NoPanel(ToolBar),
    /// The bar is placed but a window lies over it or the frame cuts it:
    /// it is never read.
    Covered(ToolBar),
    /// The bar is placed but cannot be told: its outline is broken where
    /// no window is, or its name area holds marks that are not the
    /// client's lettering.
    Unclear(ToolBar),
    /// The bar is whole and its name area is plain fill: nothing is held.
    /// Unverified against a real capture (see the module's note).
    Empty(ToolBar),
    /// The bar is whole and its name area holds the client's lettering.
    Lettered(ToolBar),
}

/// How much of a panel's outline the scan saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// All of it.
    Whole,
    /// Its left end, cut short at column `cut` by a window or the edge.
    LeftEnd { cut: i32 },
    /// Its right end, cut short at column `cut`.
    RightEnd { cut: i32 },
}

/// A panel found by its outline: where all of it lies (from the end that
/// was seen, for a part), at which scale, and how much was seen.
#[derive(Debug, Clone, Copy)]
struct Outline {
    rect: Rect,
    scale: usize,
    seen: Seen,
}

/// The rectangle `area` (shares of `size` from the top-left of `rect`)
/// stands for.
fn area_of(rect: &Rect, size: [i32; 2], area: [f64; 4]) -> Rect {
    let [w, h] = size.map(f64::from);
    let at = |share: f64, of: f64| (share * of).round() as i32;
    Rect::new(
        rect.x0 + at(area[0], w),
        rect.y0 + at(area[1], h),
        rect.x0 + at(area[2], w) - 1,
        rect.y0 + at(area[3], h) - 1,
    )
}

/// The smallest rectangle holding both.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// The frame's pixels as the outline search reads them.
struct Frame<'a> {
    image: &'a BgrImage,
    w: i32,
    h: i32,
    max: u8,
}

impl Frame<'_> {
    /// Whether `(x, y)` has the outline's colour; nothing outside the
    /// frame has.
    fn dark(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return false;
        }
        let i = ((y * self.w + x) * 3) as usize;
        let d = &self.image.data;
        d[i] <= self.max && d[i + 1] <= self.max && d[i + 2] <= self.max
    }

    /// The share of the columns `x0..=x1` of row `y` that `dark` is
    /// `want` at.
    fn row_share(&self, y: i32, x0: i32, x1: i32, want: bool) -> f32 {
        let n = (x0..=x1).filter(|x| self.dark(*x, y) == want).count();
        n as f32 / (x1 - x0 + 1).max(1) as f32
    }

    /// The share of the rows `y0..=y1` of column `x` that are dark.
    fn column_share(&self, x: i32, y0: i32, y1: i32) -> f32 {
        let n = (y0..=y1).filter(|y| self.dark(x, *y)).count();
        n as f32 / (y1 - y0 + 1).max(1) as f32
    }

    /// The least share of the outline's colour along the four sides of
    /// `rect`.
    fn outline_at(&self, rect: &Rect) -> f32 {
        [
            self.row_share(rect.y0, rect.x0, rect.x1, true),
            self.row_share(rect.y1, rect.x0, rect.x1, true),
            self.column_share(rect.x0, rect.y0, rect.y1),
            self.column_share(rect.x1, rect.y0, rect.y1),
        ]
        .into_iter()
        .fold(f32::INFINITY, f32::min)
    }
}

/// Whether the dark run `x0..=x1` on row `y` is the top of an outline `h`
/// rows tall: a dark row under it at the outline's bottom, lighter rows
/// outside both, and a fill between.
fn outline_rows(f: &Frame, y: i32, x0: i32, x1: i32, h: i32, p: &ToolBarProfile) -> bool {
    let bottom = y + h - 1;
    if h < 3 || bottom >= f.h {
        return false;
    }
    let share = p.outline_share;
    f.row_share(bottom, x0, x1, true) >= share
        && (y == 0 || f.row_share(y - 1, x0, x1, false) >= share)
        && (bottom + 1 >= f.h || f.row_share(bottom + 1, x0, x1, false) >= share)
        && f.row_share(y + h / 2, x0, x1, false) >= p.fill_share
}

/// What the dark run `x0..=x1` on row `y` is the top of, at each scale:
/// a whole outline, a part of one, or nothing.
fn consider(f: &Frame, y: i32, x0: i32, x1: i32, p: &ToolBarProfile, found: &mut Vec<Outline>) {
    let len = x1 - x0 + 1;
    let claimed = claiming(&p.scales, len, |s| s.width).ok().map(|(i, _)| i);
    for (index, scale) in p.scales.iter().enumerate() {
        let [w, h] = scale.size;
        if !outline_rows(f, y, x0, x1, h, p) {
            continue;
        }
        let end = |x: i32| f.column_share(x, y, y + h - 1) >= p.outline_share;
        let (left, right) = (end(x0), end(x1));
        let whole = Rect::new(x0, y, x0 + w - 1, y + h - 1);
        if claimed == Some(index) && left && right {
            found.push(Outline {
                rect: whole,
                scale: index,
                seen: Seen::Whole,
            });
        } else if len < scale.width[0] && f64::from(len) >= p.min_part * f64::from(w) {
            if left && !right {
                found.push(Outline {
                    rect: whole,
                    scale: index,
                    seen: Seen::LeftEnd { cut: x1 + 1 },
                });
            } else if right && !left {
                found.push(Outline {
                    rect: whole.offset(x1 - (x0 + w - 1), 0),
                    scale: index,
                    seen: Seen::RightEnd { cut: x0 - 1 },
                });
            }
        }
    }
}

/// Every panel outline whose top row lies within `roi`: each row scanned
/// for runs of the outline's colour with integer comparisons, and only a
/// run long enough to be a panel or a part of one looked at further.
fn outlines(f: &Frame, roi: Rect, p: &ToolBarProfile) -> Vec<Outline> {
    let Some(roi) = roi.intersect(&Rect::new(0, 0, f.w - 1, f.h - 1)) else {
        return Vec::new();
    };
    let Some(shortest) = p
        .scales
        .iter()
        .map(|s| (p.min_part * f64::from(s.size[0])).ceil() as i32)
        .min()
    else {
        return Vec::new();
    };
    let shortest = shortest.max(1);
    let max = p.outline_max;
    let mut found = Vec::new();
    for y in roi.y0..=roi.y1 {
        let start = ((y * f.w + roi.x0) * 3) as usize;
        let row = &f.image.data[start..start + (roi.width() * 3) as usize];
        let mut run: Option<i32> = None;
        for (k, px) in row.chunks_exact(3).enumerate() {
            let x = roi.x0 + k as i32;
            let dark = px[0] <= max && px[1] <= max && px[2] <= max;
            match (dark, run) {
                (true, None) => run = Some(x),
                (false, Some(from)) => {
                    if x - from >= shortest {
                        consider(f, y, from, x - 1, p, &mut found);
                    }
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(from) = run {
            if roi.x1 + 1 - from >= shortest {
                consider(f, y, from, roi.x1, p, &mut found);
            }
        }
    }
    found
}

/// How the left end of a panel's fill looks, when it is the health bar's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthLook {
    /// Some of it red: the player has health left.
    Red,
    /// All of it the empty part's colour: none left, or a panel filled
    /// alike (the tool bar while it reloads).
    Empty,
}

/// How the left end of the fill of the panel whose outline is `rect` (of
/// `size`) looks, when it is the health bar's: red where health is left,
/// the empty part's colour beyond. `None` when it is anything else.
fn health_look(
    image: &BgrImage,
    rect: &Rect,
    size: [i32; 2],
    p: &ToolBarProfile,
) -> Option<HealthLook> {
    let h = &p.health;
    let area = area_of(rect, size, h.area);
    let [lo, hi] = h.hue;
    let (mut red, mut empty, mut total) = (0usize, 0usize, 0usize);
    for y in area.y0..=area.y1 {
        for x in area.x0..=area.x1 {
            total += 1;
            if !image.in_bounds(x, y) {
                continue;
            }
            let rgb = image.rgb(x, y);
            if chroma(rgb) >= h.min_chroma && hue(rgb).is_some_and(|v| v >= lo && v <= hi) {
                red += 1;
            } else if near(rgb, h.empty_rgb, h.empty_tolerance) {
                empty += 1;
            }
        }
    }
    if total == 0 || ((red + empty) as f32) < h.share * total as f32 {
        return None;
    }
    Some(if red > 0 {
        HealthLook::Red
    } else {
        HealthLook::Empty
    })
}

/// The tool bar whose panel's outline is `panel`.
fn bar(panel: Rect, scale: usize, view: View, p: &ToolBarProfile) -> ToolBar {
    let size = p.scales[scale].size;
    ToolBar {
        panel,
        name: area_of(&panel, size, p.name_area),
        counter: area_of(&panel, size, p.counter_area),
        scale,
        view,
    }
}

/// The outline nearest `place` (within the scale's tolerance either way)
/// and the least share of the outline's colour along its sides.
fn best_outline(f: &Frame, place: Rect, tolerance: i32) -> (Rect, f32) {
    let mut best = (place, f.outline_at(&place));
    for dy in -tolerance..=tolerance {
        for dx in -tolerance..=tolerance {
            let rect = place.offset(dx, dy);
            let share = f.outline_at(&rect);
            if share > best.1 {
                best = (rect, share);
            }
        }
    }
    best
}

/// The tool bar among the outlines found: the panel at the pitch right of
/// the health bar, as much of it as is seen there; or, where no health bar
/// shows, a panel whose health bar lies under a window or off the frame.
/// `None` when none stands so, or more than one does.
fn decide(f: &Frame, found: &[Outline], profile: &UiProfile) -> Option<ToolBar> {
    let p = &profile.tool_bar;
    let image = f.image;
    let frame = Rect::new(0, 0, f.w - 1, f.h - 1);
    // Windows are looked for round each pair's own place, and the frame is
    // searched for them at most once.
    let windows = WindowsNear::new(image, &profile.chrome);
    let off_frame = |rect: &Rect| frame.intersect(rect) != Some(*rect);
    let under_window = |rect: &Rect, pair: Rect| {
        windows
            .around(pair)
            .iter()
            .any(|w| w.intersect(rect).is_some())
    };
    let pitch_of = |o: &Outline| p.scales[o.scale].pitch;
    // A part of a panel counts only where a window or the frame's edge
    // cuts it.
    let kept: Vec<Outline> = found
        .iter()
        .copied()
        .filter(|o| {
            let cut = match o.seen {
                Seen::Whole => return true,
                Seen::LeftEnd { cut } | Seen::RightEnd { cut } => cut,
            };
            let pitch = pitch_of(o);
            let pair = Rect::new(o.rect.x0 - pitch, o.rect.y0, o.rect.x1 + pitch, o.rect.y1);
            !frame.contains(cut, o.rect.y0)
                || windows
                    .around(pair)
                    .iter()
                    .any(|w| w.contains(cut, o.rect.y0))
        })
        .collect();
    let near_place = |o: &Outline, place: &Rect| {
        let tolerance = p.scales[o.scale].tolerance;
        (o.rect.x0 - place.x0).abs() <= tolerance && (o.rect.y0 - place.y0).abs() <= tolerance
    };
    let looks: Vec<(Outline, HealthLook)> = kept
        .iter()
        .filter(|o| !matches!(o.seen, Seen::RightEnd { .. }))
        .filter_map(|o| health_look(image, &o.rect, p.scales[o.scale].size, p).map(|l| (*o, l)))
        .collect();
    // A panel at the pitch right of another health bar is that bar's tool
    // bar, filled dark while it reloads; and a panel filled dark whose own
    // health bar's place is covered may be the tool bar too.
    let healths: Vec<&Outline> = looks
        .iter()
        .filter(|(o, look)| {
            let beside_one = looks.iter().any(|(other, _)| {
                other.scale == o.scale && near_place(o, &other.rect.offset(pitch_of(other), 0))
            });
            let left = o.rect.offset(-pitch_of(o), 0);
            let maybe_tool = *look == HealthLook::Empty
                && (off_frame(&left) || under_window(&left, union(left, o.rect)));
            !beside_one && !maybe_tool
        })
        .map(|(o, _)| o)
        .collect();
    let mut bars = Vec::new();
    for health in &healths {
        let scale = &p.scales[health.scale];
        let place = health.rect.offset(scale.pitch, 0);
        let pair = union(health.rect, place);
        let view = if off_frame(&place) || under_window(&place, pair) {
            (place, View::Covered)
        } else {
            let (rect, share) = best_outline(f, place, scale.tolerance);
            if share >= p.whole_share {
                (rect, View::Clear)
            } else if share >= p.outline_share {
                (rect, View::Broken)
            } else {
                (place, View::Missing)
            }
        };
        bars.push(bar(view.0, health.scale, view.1, p));
    }
    if !healths.is_empty() {
        // A bar seen at all is taken before a health bar alone.
        let seen: Vec<ToolBar> = bars
            .iter()
            .copied()
            .filter(|b| b.view != View::Missing)
            .collect();
        let pick = if seen.is_empty() { bars } else { seen };
        return match pick[..] {
            [one] => Some(one),
            _ => None,
        };
    }
    // No health bar shows: a panel whose health bar would lie under a
    // window or off the frame is the tool bar, covered; a whole one is
    // taken before a part.
    let mut candidates: Vec<&Outline> = kept
        .iter()
        .filter(|o| !matches!(o.seen, Seen::LeftEnd { .. }))
        .filter(|o| {
            let place = o.rect.offset(-pitch_of(o), 0);
            off_frame(&place) || under_window(&place, union(place, o.rect))
        })
        .collect();
    if candidates.iter().any(|o| o.seen == Seen::Whole) {
        candidates.retain(|o| o.seen == Seen::Whole);
    }
    match candidates[..] {
        [one] => Some(bar(one.rect, one.scale, View::Covered, p)),
        _ => None,
    }
}

/// The tool bar in `image`, if one is placed (see the module's note).
pub fn find_tool_bar(image: &BgrImage, profile: &UiProfile) -> Option<ToolBar> {
    find_tool_bar_in(
        image,
        Rect::new(0, 0, image.width() - 1, image.height() - 1),
        profile,
    )
}

/// The tool bar placed by panels whose tops lie within `roi`: the health
/// bar's (the tool bar is then looked at where it places it, within or
/// outside `roi`), or a covered tool bar's own.
pub fn find_tool_bar_in(image: &BgrImage, roi: Rect, profile: &UiProfile) -> Option<ToolBar> {
    let f = Frame {
        image,
        w: image.width(),
        h: image.height(),
        max: profile.tool_bar.outline_max,
    };
    let found = outlines(&f, roi, &profile.tool_bar);
    decide(&f, &found, profile)
}

/// The runs of lettering in `bar`'s name area, looked for along the name's
/// rows as far either side of the area as glyphs join across, within the
/// panel: lettering that runs on past the area joins a run reaching out of
/// it. Only runs reaching into the area are kept.
fn name_runs(
    image: &BgrImage,
    bar: &ToolBar,
    scale: &ToolBarScale,
    p: &ToolBarProfile,
) -> Vec<Rect> {
    let area = bar.name;
    let reach = (scale.text.join_factor * scale.text.run_height[1] as f32).ceil() as i32;
    let along = Rect::new(
        (area.x0 - reach).max(bar.panel.x0),
        area.y0,
        (area.x1 + reach).min(bar.panel.x1),
        area.y1,
    );
    runs_of_ink(image, along, &scale.text, &|x, y| {
        lit(image, x, y, &p.lettering)
    })
    .into_iter()
    .map(|run| run.rect)
    .filter(|run| run.intersect(&area).is_some())
    .collect()
}

/// Whether `area` is plain fill: no more than the profile's few pixels
/// change, in any channel, by more than the fill's own shading does from
/// the pixel left of them or above them.
fn plain(image: &BgrImage, area: Rect, p: &PlainProfile) -> bool {
    let step =
        |a: [u8; 3], b: [u8; 3], most: u8| a.iter().zip(b).any(|(x, y)| x.abs_diff(y) > most);
    let mut marks = 0;
    for y in area.y0..=area.y1 {
        for x in area.x0..=area.x1 {
            if !image.in_bounds(x, y) {
                return false;
            }
            let here = image.rgb(x, y);
            let across = x > area.x0 && step(here, image.rgb(x - 1, y), p.across);
            let down = y > area.y0 && step(here, image.rgb(x, y - 1), p.down);
            marks += usize::from(across || down);
        }
    }
    marks <= p.max_marks
}

/// What the bar `found` in `image` shows.
pub fn bar_state(image: &BgrImage, found: Option<ToolBar>, profile: &UiProfile) -> BarState {
    let p = &profile.tool_bar;
    let Some(bar) = found else {
        return BarState::Absent;
    };
    match bar.view {
        View::Missing => return BarState::NoPanel(bar),
        View::Covered => return BarState::Covered(bar),
        View::Broken => return BarState::Unclear(bar),
        View::Clear => {}
    }
    let Some(scale) = p.scales.get(bar.scale) else {
        return BarState::Unclear(bar);
    };
    let lettered = name_mask(image, &bar, profile).pixels() >= p.lettering.min_pixels
        && !name_runs(image, &bar, scale, p).is_empty();
    if lettered {
        BarState::Lettered(bar)
    } else if plain(image, bar.name, &p.plain) {
        BarState::Empty(bar)
    } else {
        BarState::Unclear(bar)
    }
}

/// What the tool bar in `image` shows.
pub fn tool_bar_state(image: &BgrImage, profile: &UiProfile) -> BarState {
    bar_state(image, find_tool_bar(image, profile), profile)
}

/// Whether the pixel at `(x, y)` is lettering on the bar.
fn lit(image: &BgrImage, x: i32, y: i32, lettering: &LetteringProfile) -> bool {
    image.in_bounds(x, y) && image.rgb(x, y).iter().all(|c| *c >= lettering.min_channel)
}

/// The lettering in a bar's name area, as pixels relative to the area:
/// the same name over the same fill shows nearly the same mask wherever
/// the bar stands. It is a gate on reading again, never a judge of which
/// tool is held: a name differing from another by one character can
/// differ by as few pixels as one name does from itself over another
/// fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lettering {
    width: i32,
    height: i32,
    bits: Vec<u64>,
    pixels: usize,
}

impl Lettering {
    /// How many pixels are lettering.
    pub fn pixels(&self) -> usize {
        self.pixels
    }

    /// The mask's size, `(width, height)`: the name area's.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// How many pixels differ between the two masks; `None` when their
    /// sizes differ.
    pub fn differing(&self, other: &Lettering) -> Option<usize> {
        if self.size() != other.size() {
            return None;
        }
        Some(
            self.bits
                .iter()
                .zip(&other.bits)
                .map(|(a, b)| (a ^ b).count_ones() as usize)
                .sum(),
        )
    }

    /// Whether this look at the lettering has changed from `earlier` by
    /// enough to be worth reading again: more pixels differ than between
    /// two looks at one name over one fill, or the masks are of areas of
    /// different sizes. `false` means only that no change worth a read
    /// was seen, never that the same tool is held: one character changed
    /// can stay under the threshold, so a watcher reads again on a timer
    /// as well.
    pub fn worth_rereading(&self, earlier: &Lettering, profile: &UiProfile) -> bool {
        self.differing(earlier)
            .is_none_or(|n| n > profile.tool_bar.lettering.max_differing)
    }
}

/// The lettering in `bar`'s name area, as pixels, whatever the bar shows.
fn name_mask(image: &BgrImage, bar: &ToolBar, profile: &UiProfile) -> Lettering {
    let area = bar.name;
    let (width, height) = (area.width().max(0), area.height().max(0));
    let word = u64::BITS as usize;
    let count = (width * height) as usize;
    let mut bits = vec![0u64; count.div_ceil(word)];
    let mut pixels = 0;
    for dy in 0..height {
        for dx in 0..width {
            if lit(
                image,
                area.x0 + dx,
                area.y0 + dy,
                &profile.tool_bar.lettering,
            ) {
                let k = (dy * width + dx) as usize;
                bits[k / word] |= 1 << (k % word);
                pixels += 1;
            }
        }
    }
    Lettering {
        width,
        height,
        bits,
        pixels,
    }
}

/// The lettering on a bar that shows lettering, as pixels; `None` for a
/// bar in any other state, whose name area holds no name to compare.
pub fn lettering(image: &BgrImage, state: &BarState, profile: &UiProfile) -> Option<Lettering> {
    match state {
        BarState::Lettered(bar) => Some(name_mask(image, bar, profile)),
        _ => None,
    }
}

/// The name read off the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolName {
    /// The name as the recogniser gave it, without the dots of a name cut
    /// short. It may lack the spaces between words (the crop is held tight
    /// to the lettering's rows, where the recogniser reads letters best),
    /// so it is compared on its letters and digits only: see
    /// [`ToolName::key`].
    pub text: String,
    /// The client cut the name short: `text` is the start of a longer
    /// name.
    pub cut_short: bool,
}

impl ToolName {
    /// The name's letters and digits, case-folded: what it is compared
    /// on, whatever spacing and punctuation the recogniser gave it.
    pub fn key(&self) -> Vec<char> {
        label_key(&self.text)
    }
}

/// Why the name was not read. Every variant is a refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolNameError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("no tool bar is in view")]
    NoBar,
    #[error("a window lies over the bar, or the frame cuts it")]
    Covered,
    #[error("the bar cannot be told: its outline is broken, or its name area holds marks that are not lettering")]
    Unclear,
    #[error("the bar's name area holds no lettering")]
    NoLettering,
    #[error("the bar was found at a scale the profile does not list")]
    UnknownScale,
    #[error("the lettering reaches the edge of the name area, so it may be cut")]
    LeavesArea,
    #[error("the lettering read as nothing")]
    Unread,
    #[error("the name read as {raw:?}, ending in dots the lettering does not show")]
    UnseenEllipsis { raw: String },
}

/// Whether the lettering within `extent` ends in the dots of a name cut
/// short: its last groups of columns are that many, each a dot no wider
/// and no taller than the profile's, standing below the lettering's
/// middle.
fn ends_in_dots(image: &BgrImage, extent: Rect, p: &ToolBarProfile) -> bool {
    let rows = |x: i32| -> Option<(i32, i32)> {
        let lit_rows: Vec<i32> = (extent.y0..=extent.y1)
            .filter(|y| lit(image, x, *y, &p.lettering))
            .collect();
        Some((*lit_rows.first()?, *lit_rows.last()?))
    };
    let middle = (extent.y0 + extent.y1) / 2;
    let mut dots = 0;
    let mut x = extent.x1;
    while dots < p.ellipsis.dots {
        // The next group of columns holding lettering, right to left.
        while x >= extent.x0 && rows(x).is_none() {
            x -= 1;
        }
        if x < extent.x0 {
            return false;
        }
        let right = x;
        let (mut top, mut foot) = (extent.y1, extent.y0);
        while x >= extent.x0 {
            let Some((t, b)) = rows(x) else { break };
            top = top.min(t);
            foot = foot.max(b);
            x -= 1;
        }
        let wide = right - x;
        if wide > p.ellipsis.max_dot || foot - top + 1 > p.ellipsis.max_dot || top <= middle {
            return false;
        }
        dots += 1;
    }
    true
}

/// Read the name on a bar that shows lettering: the lettering in its name
/// area, cropped from its own extent (no further past it than the
/// profile's margin, and never past the name area) and handed to the
/// recogniser. Nothing is accepted on the recogniser's confidence; a name
/// cut short is told by its dots on the pixels, and a read ending in dots
/// the pixels do not show is refused.
pub fn read_tool_name(
    image: &BgrImage,
    state: &BarState,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<ToolName, ToolNameError> {
    let bar = match state {
        BarState::Lettered(bar) => bar,
        BarState::Absent | BarState::NoPanel(_) => return Err(ToolNameError::NoBar),
        BarState::Covered(_) => return Err(ToolNameError::Covered),
        BarState::Unclear(_) => return Err(ToolNameError::Unclear),
        BarState::Empty(_) => return Err(ToolNameError::NoLettering),
    };
    let p = &profile.tool_bar;
    let scale = p.scales.get(bar.scale).ok_or(ToolNameError::UnknownScale)?;
    let area = bar.name;
    let extent = name_runs(image, bar, scale, p)
        .into_iter()
        .reduce(union)
        .ok_or(ToolNameError::NoLettering)?;
    if extent.x0 <= area.x0 || extent.y0 <= area.y0 || extent.x1 >= area.x1 || extent.y1 >= area.y1
    {
        return Err(ToolNameError::LeavesArea);
    }
    let bounds = extent
        .padded(p.crop_margin)
        .intersect(&area)
        .unwrap_or(extent);
    let (raw, _) = read_run_within(image, extent, bounds, &profile.read, recogniser)
        .ok_or(ToolNameError::NoRecogniser)?;
    let cut_short = ends_in_dots(image, extent, p);
    let read = raw.trim();
    let text = read.trim_end_matches(|c: char| c == '.' || c == '\u{2026}' || c.is_whitespace());
    if text.is_empty() {
        return Err(ToolNameError::Unread);
    }
    if !cut_short && text.len() != read.len() {
        return Err(ToolNameError::UnseenEllipsis { raw });
    }
    Ok(ToolName {
        text: text.to_string(),
        cut_short,
    })
}

/// The health bar and the tool bar as they are drawn, measured without
/// assuming any of the profile's scales: what a scale's entry is measured
/// from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolBarMeasurement {
    /// The health bar's outline.
    pub health: Rect,
    /// The tool bar's outline.
    pub tool: Rect,
}

/// Every pair of panels in `image` drawn as the health bar and the tool
/// bar, measured from their own pixels: two outlines of one size, level
/// with each other, the left one's fill starting as the health bar's. The
/// outline's colour, the share of it a row must show and the health bar's
/// colours are the profile's, which do not change with the scale; the
/// sizes are measured, so a pair at a scale the profile does not list is
/// measured too.
pub fn measure_tool_bars(image: &BgrImage, profile: &UiProfile) -> Vec<ToolBarMeasurement> {
    let p = &profile.tool_bar;
    let f = Frame {
        image,
        w: image.width(),
        h: image.height(),
        max: p.outline_max,
    };
    // The shortest panel worth measuring: half the narrowest listed.
    let Some(shortest) = p.scales.iter().map(|s| s.size[0] / 2).min() else {
        return Vec::new();
    };
    let mut panels: Vec<Rect> = Vec::new();
    for y in 0..f.h {
        let mut x = 0;
        while x < f.w {
            if !f.dark(x, y) {
                x += 1;
                continue;
            }
            let from = x;
            while x < f.w && f.dark(x, y) {
                x += 1;
            }
            let (x0, x1) = (from, x - 1);
            // Only the top row of a dark area can be an outline's top.
            if x1 - x0 + 1 < shortest
                || (y > 0 && f.row_share(y - 1, x0, x1, false) < p.outline_share)
            {
                continue;
            }
            // The outline's height: the first row below, no further down
            // than the run is long, that closes it.
            let closes = |h: i32| {
                outline_rows(&f, y, x0, x1, h, p)
                    && f.column_share(x0, y, y + h - 1) >= p.outline_share
                    && f.column_share(x1, y, y + h - 1) >= p.outline_share
            };
            if let Some(h) = (3..=x1 - x0).find(|h| closes(*h)) {
                panels.push(Rect::new(x0, y, x1, y + h - 1));
            }
        }
    }
    let health = |rect: &Rect| {
        health_look(image, rect, [rect.width(), rect.height()], p) == Some(HealthLook::Red)
    };
    let mut pairs = Vec::new();
    for left in panels.iter().filter(|r| health(r)) {
        let partner = panels
            .iter()
            .filter(|t| {
                t.x0 > left.x1
                    && (t.y0 - left.y0).abs() <= 1
                    && (t.width() - left.width()).abs() <= 1
                    && t.height() == left.height()
            })
            .min_by_key(|t| t.x0);
        if let Some(tool) = partner {
            pairs.push(ToolBarMeasurement {
                health: *left,
                tool: *tool,
            });
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{
        bundled_profile, draw_plate_bar, draw_tool_bars, draw_window, fill, scenery, PlateDrawing,
        ToolBarDrawing, ToolBarScene, ToolFill, GLYPH_H, NAME_LEFT, NAME_TOP, TOOL_LETTERING,
        TOOL_PANEL, TOOL_PITCH,
    };

    const NAME: &str = "Portable Repair Unit (L)";
    const CUT: &str = "Exceptional Mayhem Electric Att...";

    fn scene(w: usize, h: usize, seed: u64) -> ToolBarScene {
        ToolBarScene::new(scenery(w, h, seed))
    }

    /// The bar the bundled profile places on the tool panel at `tool`.
    fn placed(tool: Rect, view: View) -> ToolBar {
        bar(tool, 0, view, &bundled_profile().tool_bar)
    }

    /// A scene with the panels drawn as `drawing` says, its state, and the
    /// tool panel's outline.
    fn drawn(seed: u64, drawing: &ToolBarDrawing) -> (ToolBarScene, BarState, Rect) {
        let mut s = scene(1400, 800, seed);
        let (_, tool) = s.draw(drawing);
        let state = tool_bar_state(&s.image, &bundled_profile());
        (s, state, tool)
    }

    #[test]
    fn the_bar_is_found_by_its_own_shape_wherever_the_interface_is_moved() {
        let profile = bundled_profile();
        for (k, at) in [(40, 600), (900, 1308), (1200, 40), (3, 3), (1100, 760)]
            .into_iter()
            .enumerate()
        {
            let mut s = scene(2000, 1400, k as u64);
            let (health, tool) = s.draw(&ToolBarDrawing::named(at, NAME));
            assert_eq!(health.width(), TOOL_PANEL.0);
            let found = find_tool_bar(&s.image, &profile);
            assert_eq!(found, Some(placed(tool, View::Clear)), "{at:?}");
            let bar = found.unwrap();
            // The areas are the profile's shares of the panel, in the
            // frame's coordinates.
            assert!(bar.name.x0 > tool.x0 && bar.name.x1 < bar.counter.x0);
            assert!(bar.counter.x1 < tool.x1 && bar.name.y0 > tool.y0 && bar.name.y1 < tool.y1);
            assert_eq!(
                bar_state(&s.image, found, &profile),
                BarState::Lettered(bar)
            );
            // Searching round the health bar alone places the tool bar
            // beside it; searching round neither finds nothing.
            let round_health = health.padded(5);
            assert_eq!(find_tool_bar_in(&s.image, round_health, &profile), found);
            let elsewhere = Rect::new(tool.x1 + 10, 0, tool.x1 + 20, 10);
            assert_eq!(find_tool_bar_in(&s.image, elsewhere, &profile), None);
        }
    }

    #[test]
    fn a_bar_at_a_scale_the_profile_does_not_list_is_not_found() {
        let profile = bundled_profile();
        // A third as large again: outline, pitch and lettering.
        let (w, h) = TOOL_PANEL;
        let mut s = scene(2000, 900, 7);
        s.draw(&ToolBarDrawing {
            size: (w * 4 / 3, h * 4 / 3),
            pitch: TOOL_PITCH * 4 / 3,
            ..ToolBarDrawing::named((200, 400), NAME)
        });
        assert_eq!(find_tool_bar(&s.image, &profile), None);
        assert_eq!(tool_bar_state(&s.image, &profile), BarState::Absent);
        // Measured all the same, at its own size.
        let measured = measure_tool_bars(&s.image, &profile);
        assert_eq!(measured.len(), 1, "{measured:?}");
        assert_eq!(
            (measured[0].tool.width(), measured[0].tool.height()),
            (w * 4 / 3, h * 4 / 3)
        );
        assert_eq!(
            measured[0].tool.x0 - measured[0].health.x0,
            TOOL_PITCH * 4 / 3
        );
    }

    #[test]
    fn a_bar_whose_name_area_is_plain_fill_is_empty_and_one_with_lettering_is_lettered() {
        for fill_state in [ToolFill::Ready, ToolFill::Reloading] {
            let (_, state, tool) = drawn(
                8,
                &ToolBarDrawing {
                    fill: fill_state,
                    name: None,
                    ..ToolBarDrawing::named((100, 300), NAME)
                },
            );
            let bar = placed(tool, View::Clear);
            assert_eq!(state, BarState::Empty(bar), "{fill_state:?}");
            let (s, state, _) = drawn(
                8,
                &ToolBarDrawing {
                    fill: fill_state,
                    ..ToolBarDrawing::named((100, 300), NAME)
                },
            );
            assert_eq!(state, BarState::Lettered(bar), "{fill_state:?}");
            assert!(lettering(&s.image, &state, &bundled_profile()).is_some());
        }
    }

    #[test]
    fn marks_that_are_not_the_clients_lettering_leave_the_bar_unclear_never_empty() {
        let profile = bundled_profile();
        let marks: [(&str, [u8; 3], (i32, i32)); 4] = [
            ("grey lettering", [150, 150, 150], (40, 7)),
            ("red lettering", [220, 40, 40], (40, 7)),
            ("dimmed white lettering", [168, 168, 168], (40, 7)),
            ("a blob", [60, 140, 220], (14, 14)),
        ];
        for fill_state in [ToolFill::Ready, ToolFill::Reloading] {
            for (what, rgb, (w, h)) in marks {
                let (mut s, _, tool) = drawn(
                    9,
                    &ToolBarDrawing {
                        fill: fill_state,
                        name: None,
                        ..ToolBarDrawing::named((100, 300), NAME)
                    },
                );
                let at = (tool.x0 + NAME_LEFT, tool.y0 + NAME_TOP);
                fill(
                    &mut s.image,
                    Rect::new(at.0, at.1, at.0 + w - 1, at.1 + h - 1),
                    rgb,
                );
                let state = tool_bar_state(&s.image, &profile);
                let bar = placed(tool, View::Clear);
                assert_eq!(state, BarState::Unclear(bar), "{what} {fill_state:?}");
                assert_eq!(lettering(&s.image, &state, &profile), None, "{what}");
                assert_eq!(
                    read_tool_name(&s.image, &state, &profile, &s.recogniser()),
                    Err(ToolNameError::Unclear),
                    "{what}"
                );
            }
        }
    }

    #[test]
    fn a_health_bar_with_no_panel_beside_it_is_told_apart_from_no_bar() {
        let profile = bundled_profile();
        let mut s = scene(1400, 800, 10);
        let (_, tool) = s.draw(&ToolBarDrawing::named((200, 400), NAME));
        // The tool panel and its frame painted over with the scene.
        fill(&mut s.image, tool.padded(3), [120, 105, 85]);
        let state = tool_bar_state(&s.image, &profile);
        assert_eq!(state, BarState::NoPanel(placed(tool, View::Missing)));
        assert_eq!(lettering(&s.image, &state, &profile), None);
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &s.recogniser()),
            Err(ToolNameError::NoBar)
        );
        // With neither panel there is no bar at all.
        let s = scene(1400, 800, 10);
        assert_eq!(tool_bar_state(&s.image, &profile), BarState::Absent);
    }

    #[test]
    fn an_outline_broken_where_no_window_is_leaves_the_bar_unclear() {
        let profile = bundled_profile();
        // A pointer's narrow tip across the outline: under a tenth of any
        // side, so the panel is still found, but no longer whole.
        let crossings = [
            ("its top", (100, -2, 103, 3)),
            ("its bottom", (200, 28, 203, 33)),
            ("its right end", (369, 12, 374, 13)),
        ];
        for (what, (x0, y0, x1, y1)) in crossings {
            let (mut s, _, tool) = drawn(11, &ToolBarDrawing::named((150, 400), NAME));
            fill(
                &mut s.image,
                Rect::new(tool.x0 + x0, tool.y0 + y0, tool.x0 + x1, tool.y0 + y1),
                [240, 240, 240],
            );
            assert_eq!(
                tool_bar_state(&s.image, &profile),
                BarState::Unclear(placed(tool, View::Broken)),
                "{what}"
            );
        }
    }

    #[test]
    fn a_bar_under_a_window_or_cut_by_the_frame_is_covered_and_never_read() {
        let profile = bundled_profile();
        let at = (300, 700);
        let (health, tool) = {
            let mut s = scene(1600, 1000, 9);
            s.draw(&ToolBarDrawing::named(at, NAME))
        };
        let cases: [(&str, Rect); 5] = [
            (
                "the tool bar's right part",
                Rect::new(tool.x0 + 80, 300, 1500, 900),
            ),
            (
                "the health bar's right part and all the tool bar",
                Rect::new(health.x0 + 280, 400, 1500, 950),
            ),
            (
                "the health bar and the tool bar's left part",
                Rect::new(20, 350, tool.x0 + 160, 990),
            ),
            (
                "all but the health bar's left end",
                Rect::new(health.x0 + 30, 200, 1590, 990),
            ),
            (
                "the middle of the tool bar",
                Rect::new(tool.x0 + 100, 600, tool.x0 + 300, 800),
            ),
        ];
        for (what, window) in cases {
            let mut s = scene(1600, 1000, 9);
            s.draw(&ToolBarDrawing::named(at, NAME));
            draw_window(&mut s.image, window);
            let found = find_tool_bar(&s.image, &profile);
            assert_eq!(found, Some(placed(tool, View::Covered)), "{what}");
            let state = bar_state(&s.image, found, &profile);
            assert_eq!(
                state,
                BarState::Covered(placed(tool, View::Covered)),
                "{what}"
            );
            assert_eq!(lettering(&s.image, &state, &profile), None, "{what}");
            assert_eq!(
                read_tool_name(&s.image, &state, &profile, &s.recogniser()),
                Err(ToolNameError::Covered),
                "{what}"
            );
        }
        // A window over the whole of both: nothing of the bar shows.
        let mut s = scene(1600, 1000, 9);
        s.draw(&ToolBarDrawing::named(at, NAME));
        draw_window(&mut s.image, Rect::new(100, 300, 1550, 990));
        assert_eq!(tool_bar_state(&s.image, &profile), BarState::Absent);
        // The frame's right edge through the tool bar, and its left edge
        // through the health bar.
        for at in [(1600 - TOOL_PITCH - 200, 500), (-250, 500)] {
            let mut s = scene(1600, 1000, 10);
            let (_, tool) = s.draw(&ToolBarDrawing::named(at, NAME));
            assert_eq!(
                find_tool_bar(&s.image, &profile),
                Some(placed(tool, View::Covered)),
                "{at:?}"
            );
        }
    }

    #[test]
    fn the_bar_is_found_with_little_or_no_health_left() {
        for health in [1.0, 0.3, 0.02, 0.0] {
            for fill_state in [ToolFill::Ready, ToolFill::Reloading] {
                let (_, state, tool) = drawn(
                    12,
                    &ToolBarDrawing {
                        health,
                        fill: fill_state,
                        ..ToolBarDrawing::named((500, 300), NAME)
                    },
                );
                assert_eq!(
                    state,
                    BarState::Lettered(placed(tool, View::Clear)),
                    "health {health}, {fill_state:?}"
                );
            }
        }
    }

    #[test]
    fn plates_lone_panels_and_a_pair_without_the_health_bar_are_not_taken_for_it() {
        let profile = bundled_profile();
        let mut s = scene(2000, 1200, 11);
        // Creatures' plates, locked and washed out.
        draw_plate_bar(&mut s.image, &PlateDrawing::locked((200, 200), 0.6));
        draw_plate_bar(&mut s.image, &PlateDrawing::washed((700, 260)));
        // A pair whose left panel holds the tool bar's tan fill, and a lone
        // tool panel with nothing beside it.
        let (left, _) = draw_tool_bars(
            &mut s.image,
            &ToolBarDrawing::named((600, 500), "Teleportation Chip 1"),
        );
        let tan = draw_tool_bars(&mut s.image, &ToolBarDrawing::named((0, 0), NAME)).1;
        for y in left.y0 + 1..left.y1 {
            for x in left.x0 + 3..left.x1 - 2 {
                let rgb = s
                    .image
                    .rgb(tan.x0 + 3 + (x - left.x0 - 3), tan.y0 + (y - left.y0));
                crate::game_ui::test_support::put(&mut s.image, x, y, rgb);
            }
        }
        fill(&mut s.image, Rect::new(0, 0, 900, 40), [120, 105, 85]);
        let (gone, _) = draw_tool_bars(&mut s.image, &ToolBarDrawing::named((500, 900), NAME));
        fill(&mut s.image, gone.padded(3), [120, 105, 85]);
        assert_eq!(find_tool_bar(&s.image, &profile), None);
        // A red bar of the health bar's size with no outline.
        fill(&mut s.image, Rect::new(1200, 700, 1571, 730), [222, 40, 62]);
        assert_eq!(find_tool_bar(&s.image, &profile), None);
        // The real bar among them is found.
        let (_, tool) = s.draw(&ToolBarDrawing::named((1100, 1000), NAME));
        assert_eq!(
            find_tool_bar(&s.image, &profile),
            Some(placed(tool, View::Clear))
        );
    }

    #[test]
    fn two_health_bars_with_their_tool_bars_leave_the_bar_untold() {
        let profile = bundled_profile();
        let mut s = scene(2000, 1200, 12);
        s.draw(&ToolBarDrawing::named((100, 200), NAME));
        s.draw(&ToolBarDrawing::named((900, 800), CUT));
        assert_eq!(find_tool_bar(&s.image, &profile), None);
    }

    /// The lettering of `name` on a bar drawn at `at` over `fill_state`.
    fn mask(at: (i32, i32), fill_state: ToolFill, name: &str, health: f64) -> Lettering {
        let profile = bundled_profile();
        let mut s = scene(2000, 1200, at.0 as u64);
        s.draw(&ToolBarDrawing {
            fill: fill_state,
            health,
            ..ToolBarDrawing::named(at, name)
        });
        let state = tool_bar_state(&s.image, &profile);
        lettering(&s.image, &state, &profile).expect("a lettered bar")
    }

    #[test]
    fn the_lettering_mask_is_stable_across_fill_states_and_places() {
        let profile = bundled_profile();
        let ready = mask((100, 300), ToolFill::Ready, CUT, 1.0);
        let reloading = mask((700, 900), ToolFill::Reloading, CUT, 0.3);
        assert!(ready.pixels() >= profile.tool_bar.lettering.min_pixels);
        assert_eq!(ready.differing(&reloading), Some(0));
        assert!(!ready.worth_rereading(&reloading, &profile));
        // A mask of another size is always worth reading again.
        let mut small = profile.clone();
        small.tool_bar.name_area[2] -= 0.1;
        let mut s = scene(2000, 1200, 100);
        s.draw(&ToolBarDrawing::named((100, 300), CUT));
        let state = tool_bar_state(&s.image, &small);
        let shrunk = lettering(&s.image, &state, &small).expect("a lettered bar");
        assert_eq!(ready.differing(&shrunk), None);
        assert!(ready.worth_rereading(&shrunk, &profile));
    }

    #[test]
    fn a_name_one_character_or_a_suffix_apart_is_worth_reading_again() {
        let profile = bundled_profile();
        for (was, now) in [
            ("Teleportation Chip 1", "Teleportation Chip 2"),
            ("Hyperion Aurora TC-1", "Hyperion Aurora OC-1"),
            ("Portable TT Unit", "Portable TT Unit (L)"),
            ("Portable TT Unit", "Portable TT Unib"),
        ] {
            let a = mask((100, 300), ToolFill::Ready, was, 1.0);
            let b = mask((700, 900), ToolFill::Reloading, now, 1.0);
            assert!(
                b.worth_rereading(&a, &profile),
                "{was} to {now}: {:?} pixels",
                a.differing(&b)
            );
        }
    }

    #[test]
    fn the_name_is_read_from_the_letterings_own_extent_through_the_recogniser() {
        let profile = bundled_profile();
        for name in [NAME, "Teleportation Chip 1", "TT"] {
            let (s, state, _) = drawn(13, &ToolBarDrawing::named((200, 500), name));
            assert_eq!(
                read_tool_name(&s.image, &state, &profile, &s.recogniser()),
                Ok(ToolName {
                    text: name.to_string(),
                    cut_short: false
                }),
                "{name}"
            );
        }
        // A crop as wide as the panel takes in the outline's ends, which
        // the stand-in (like the real recogniser) misreads.
        let (s, state, tool) = drawn(13, &ToolBarDrawing::named((200, 500), NAME));
        assert!(matches!(state, BarState::Lettered(_)));
        let read = s.recogniser();
        let whole_panel = read_run_within(&s.image, tool, tool, &profile.read, &read);
        assert_eq!(whole_panel.map(|r| r.0), Some(String::new()));
    }

    #[test]
    fn a_name_is_compared_on_its_letters_and_digits_alone() {
        let read = |text: &str| ToolName {
            text: text.to_string(),
            cut_short: false,
        };
        assert_eq!(
            read("PortableRepair Unit (L)").key(),
            read("Portable Repair Unit (L)").key()
        );
        assert_eq!(
            read("teleportation chip 1").key(),
            read("Teleportation Chip 1").key()
        );
        assert_ne!(
            read("Teleportation Chip 1").key(),
            read("Teleportation Chip 2").key()
        );
        assert_eq!(read("Hyperion Aurora TC-1,Improv").key().len(), 23);
    }

    #[test]
    fn a_name_cut_short_is_told_by_its_dots_whatever_the_recogniser_makes_of_them() {
        let profile = bundled_profile();
        let (s, state, _) = drawn(14, &ToolBarDrawing::named((200, 500), CUT));
        let inner = s.recogniser();
        for (reads, text) in [
            (
                "Exceptional Mayhem Electric Att...",
                "Exceptional Mayhem Electric Att",
            ),
            (
                "Exceptional Mayhem Electric Att..",
                "Exceptional Mayhem Electric Att",
            ),
            (
                "Exceptional Mayhem Electric At.",
                "Exceptional Mayhem Electric At",
            ),
            (
                "Exceptional Mayhem Electric Att\u{2026}",
                "Exceptional Mayhem Electric Att",
            ),
            (
                "Exceptional Mayhem Electric Att . . .",
                "Exceptional Mayhem Electric Att",
            ),
            (
                "Exceptional Mayhem Electric Al",
                "Exceptional Mayhem Electric Al",
            ),
        ] {
            let read = |crop: &BgrImage| {
                inner(crop).map(|(t, c)| (if t.is_empty() { t } else { reads.to_string() }, c))
            };
            assert_eq!(
                read_tool_name(&s.image, &state, &profile, &read),
                Ok(ToolName {
                    text: text.to_string(),
                    cut_short: true
                }),
                "{reads}"
            );
        }
        // Dots in the read where the lettering has none are a misreading.
        let (s, state, _) = drawn(15, &ToolBarDrawing::named((200, 500), NAME));
        let inner = s.recogniser();
        let dotted = |crop: &BgrImage| inner(crop).map(|(t, c)| (format!("{t}."), c));
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &dotted),
            Err(ToolNameError::UnseenEllipsis {
                raw: format!("{NAME}.")
            })
        );
    }

    #[test]
    fn endings_that_are_not_the_clients_dots_are_not_taken_for_a_name_cut_short() {
        let profile = bundled_profile();
        let name = "Portable TT Unit";
        // Marks after the name, from its end: each `(dx, dy, w, h)` from the
        // lettering's top-left, the lettering `GLYPH_H` tall.
        let foot = GLYPH_H - 2;
        type Marks = Vec<(i32, i32, i32, i32)>;
        let endings: [(&str, Marks, bool); 6] = [
            (
                "three dots",
                vec![(2, foot, 2, 2), (7, foot, 2, 2), (12, foot, 2, 2)],
                true,
            ),
            (
                "a colon",
                vec![
                    (2, foot, 2, 2),
                    (7, foot, 2, 2),
                    (12, 2, 2, 2),
                    (12, foot, 2, 2),
                ],
                false,
            ),
            (
                "an i and a stop",
                vec![(2, 0, 2, GLYPH_H), (7, foot, 2, 2)],
                false,
            ),
            (
                "a mark above the middle",
                vec![(2, foot, 2, 2), (7, foot, 2, 2), (12, 1, 2, 2)],
                false,
            ),
            (
                "a stroke too tall for a dot",
                vec![(2, foot, 2, 2), (7, foot, 2, 2), (12, foot - 3, 2, 5)],
                false,
            ),
            ("a single full stop", vec![(2, foot, 2, 2)], false),
        ];
        for (what, marks, dotted) in endings {
            let (mut s, _, tool) = drawn(16, &ToolBarDrawing::named((200, 500), name));
            let end = tool.x0 + NAME_LEFT + crate::game_ui::test_support::name_pixels(name).1;
            for (dx, dy, w, h) in marks {
                let x = end + dx;
                let y = tool.y0 + NAME_TOP + dy;
                fill(
                    &mut s.image,
                    Rect::new(x, y, x + w - 1, y + h - 1),
                    TOOL_LETTERING,
                );
            }
            let state = tool_bar_state(&s.image, &profile);
            let read = s.recogniser();
            assert_eq!(
                read_tool_name(&s.image, &state, &profile, &read),
                Ok(ToolName {
                    text: name.to_string(),
                    cut_short: dotted
                }),
                "{what}"
            );
            // A read ending in a stop the pixels do not show as dots is
            // refused.
            let stopped = |crop: &BgrImage| read(crop).map(|(t, c)| (format!("{t}."), c));
            if !dotted {
                assert_eq!(
                    read_tool_name(&s.image, &state, &profile, &stopped),
                    Err(ToolNameError::UnseenEllipsis {
                        raw: format!("{name}.")
                    }),
                    "{what}"
                );
            }
        }
    }

    #[test]
    fn an_empty_read_no_recogniser_and_lettering_past_the_name_area_are_refused() {
        let profile = bundled_profile();
        let (s, state, _) = drawn(16, &ToolBarDrawing::named((200, 500), NAME));
        let nothing = |_: &BgrImage| Some((" . ".to_string(), 0.99));
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &nothing),
            Err(ToolNameError::Unread)
        );
        let none = |_: &BgrImage| -> Option<(String, f64)> { None };
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &none),
            Err(ToolNameError::NoRecogniser)
        );
        // A bar at a scale the profile does not list is its own refusal.
        let BarState::Lettered(bar) = state else {
            panic!("{state:?}")
        };
        let unknown = BarState::Lettered(ToolBar { scale: 9, ..bar });
        assert_eq!(
            read_tool_name(&s.image, &unknown, &profile, &s.recogniser()),
            Err(ToolNameError::UnknownScale)
        );
        // Lettering running on to the end of the name area.
        let long = "Exceptional Mayhem Electric Attack Chip";
        let (s, state, tool) = drawn(17, &ToolBarDrawing::named((200, 500), long));
        let BarState::Lettered(bar) = state else {
            panic!("{state:?}")
        };
        assert!(tool.x0 + NAME_LEFT + 8 * long.len() as i32 > bar.name.x1);
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &s.recogniser()),
            Err(ToolNameError::LeavesArea)
        );
        // Ink reaching the top of the name area.
        let (mut s, state, _) = drawn(18, &ToolBarDrawing::named((200, 500), NAME));
        let BarState::Lettered(bar) = state else {
            panic!("{state:?}")
        };
        fill(
            &mut s.image,
            Rect::new(
                bar.name.x0 + 30,
                bar.name.y0,
                bar.name.x0 + 31,
                bar.panel.y0 + NAME_TOP + GLYPH_H / 2,
            ),
            TOOL_LETTERING,
        );
        let state = tool_bar_state(&s.image, &profile);
        assert_eq!(
            read_tool_name(&s.image, &state, &profile, &s.recogniser()),
            Err(ToolNameError::LeavesArea)
        );
    }
}
