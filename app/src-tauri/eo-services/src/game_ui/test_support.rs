//! Shared helpers for the reading pipeline's tests: the bundled
//! profile, and frames drawn the way the client draws them. Compiled
//! for this crate's tests, and for the facade's through the
//! `test-support` feature.

use std::path::{Path, PathBuf};

use crate::game_ui::geometry::BgrImage;
use crate::game_ui::seams::{FrameArea, FrameUnavailable, GameFrame};

use super::geometry::Rect;
use super::profile::UiProfile;
use super::radar::CoordinateProfile;
use super::skills::SkillVocabulary;
use super::validate::{expected_width, NumeralMetrics};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The profile the application ships.
pub fn bundled_profile() -> UiProfile {
    UiProfile::load(&repo_root().join("app/src-tauri/entropia-orme/resources/ui_profile.json"))
        .expect("the bundled interface profile loads")
}

/// The creatures of the bundled game data, as its `mobs` collection
/// lists them.
pub fn bundled_mobs() -> Vec<serde_json::Value> {
    let path = repo_root().join("app/src-tauri/entropia-orme/resources/snapshot/mobs.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// What a game window showing `image`, its client area at `origin` on the
/// screen, gives a capture of `area`: the part asked for as far as it lies
/// in the window, with where it came from.
pub fn window_frame(
    image: &BgrImage,
    origin: (i64, i64),
    area: FrameArea,
) -> Result<GameFrame, FrameUnavailable> {
    let whole = Rect::new(0, 0, image.w as i32 - 1, image.h as i32 - 1);
    let part = match area {
        FrameArea::Whole => whole,
        FrameArea::Part(part) => part
            .intersect(&whole)
            .ok_or(FrameUnavailable::OutsideCapturableArea)?,
    };
    Ok(GameFrame {
        image: super::geometry::crop(image, part),
        origin: (origin.0 + part.x0 as i64, origin.1 + part.y0 as i64),
        offset: (part.x0 as i64, part.y0 as i64),
    })
}

/// A frame filled with one colour.
pub fn blank(w: usize, h: usize, rgb: [u8; 3]) -> BgrImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for _ in 0..w * h {
        data.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
    }
    BgrImage { data, h, w }
}

pub fn put(image: &mut BgrImage, x: i32, y: i32, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= image.w as i32 || y >= image.h as i32 {
        return;
    }
    let i = (y as usize * image.w + x as usize) * 3;
    image.data[i] = rgb[2];
    image.data[i + 1] = rgb[1];
    image.data[i + 2] = rgb[0];
}

pub fn fill(image: &mut BgrImage, rect: Rect, rgb: [u8; 3]) {
    for y in rect.y0..=rect.y1 {
        for x in rect.x0..=rect.x1 {
            put(image, x, y, rgb);
        }
    }
}

/// Draw a client window the way the client does at the default
/// interface scale: the panel fill, a two-pixel ridge one pixel inside
/// the outer edge, and the title marker at the top-left. Parts outside
/// the frame are clipped, as a window dragged off the screen is.
pub fn draw_window(image: &mut BgrImage, rect: Rect) {
    const PANEL: [u8; 3] = [8, 11, 11];
    const RIDGE_OUTER: [u8; 3] = [45, 49, 49];
    const RIDGE_INNER: [u8; 3] = [76, 82, 84];
    const MARKER: [u8; 3] = [255, 200, 90];
    fill(image, rect, PANEL);
    for (inset, rgb) in [(1, RIDGE_OUTER), (2, RIDGE_INNER)] {
        let r = rect.padded(-inset);
        for x in r.x0..=r.x1 {
            put(image, x, r.y0, rgb);
            put(image, x, r.y1, rgb);
        }
        for y in r.y0..=r.y1 {
            put(image, r.x0, y, rgb);
            put(image, r.x1, y, rgb);
        }
    }
    fill(
        image,
        Rect::new(rect.x0 + 25, rect.y0 + 18, rect.x0 + 29, rect.y0 + 37),
        MARKER,
    );
}

/// Lettering drawn as blocks of ink, each piece of text in a colour no
/// other piece has, with a stand-in recogniser that reads a crop as the
/// text whose colour covers most of it. A reader's geometry is exercised
/// this way without a real recogniser, and a test decides exactly what
/// each piece of ink reads as. A piece drawn dim, or a crop a reader has
/// brightened, still reads as its text: the recogniser knows a colour by
/// the proportions of its channels, which scaling keeps. The palette's
/// colours are saturated (no channel more than a fifth of the brightest),
/// so their proportions are never those of any colour the drawers use for
/// frames, rules, boxes or pictures, nor of one another (see
/// [`DRAWN_CHROME`] and the tests below); a few near-grey colours stand
/// for lettering the client draws unlit.
pub struct Lettering {
    pub image: BgrImage,
    palette: Vec<([u8; 3], String)>,
    unlit: usize,
}

/// The `i`th saturated colour: the brightest channel 250, the next one of
/// eight levels, the dimmest one of three, in every arrangement.
fn palette_colour(i: usize) -> [u8; 3] {
    const MIDDLE: [u8; 8] = [60, 85, 110, 135, 160, 185, 210, 235];
    const DIMMEST: [u8; 3] = [0, 25, 50];
    const ARRANGEMENTS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    assert!(i < 144, "the drawing has used every distinct colour");
    let [top, middle, dimmest] = ARRANGEMENTS[i / 24];
    let mut rgb = [0u8; 3];
    rgb[top] = 250;
    rgb[middle] = MIDDLE[(i / 3) % 8];
    rgb[dimmest] = DIMMEST[i % 3];
    rgb
}

/// Near-grey colours for lettering the client draws unlit: a little
/// blue, too little to count as lit, and in proportions no frame, rule or
/// box colour has.
const UNLIT: [[u8; 3]; 3] = [[165, 180, 200], [180, 165, 200], [190, 175, 200]];

/// Every colour the drawers use for anything but lettering.
pub const DRAWN_CHROME: [[u8; 3]; 21] = [
    [8, 11, 11],
    [45, 49, 49],
    [76, 82, 84],
    [255, 200, 90],
    SKILLS_SCENE,
    RULE,
    DIVIDER,
    ICON,
    THUMB,
    BOX_LIVE,
    BOX_LOCKED,
    ARROW_LIVE,
    ARROW_LOCKED,
    PICTURE,
    PICTURE_FRAME,
    TAG_LIT,
    TAG_UNLIT,
    SIDE_ICON,
    [24, 24, 24],
    [250, 250, 250],
    [200, 200, 200],
];

impl Lettering {
    pub fn new(image: BgrImage) -> Self {
        Self {
            image,
            palette: Vec::new(),
            unlit: 0,
        }
    }

    /// Draw a block of ink filling `rect` that reads as `text`.
    pub fn text(&mut self, rect: Rect, text: &str) {
        let rgb = self.colour(text);
        fill(&mut self.image, rect, rgb);
    }

    /// The next colour of the palette, kept for `text`.
    fn colour(&mut self, text: &str) -> [u8; 3] {
        let rgb = palette_colour(self.palette.len() - self.unlit);
        self.palette.push((rgb, text.to_string()));
        rgb
    }

    /// The same, drawn no brighter than `peak` in its brightest channel:
    /// lettering the client greys or sets dim.
    pub fn text_at(&mut self, rect: Rect, text: &str, peak: u8) {
        self.blocks(&[rect], text, peak);
    }

    /// Draw lettering the client leaves unlit (grey) filling `rect`.
    pub fn text_unlit(&mut self, rect: Rect, text: &str) {
        let rgb = UNLIT[self.unlit];
        self.unlit += 1;
        self.palette.push((rgb, text.to_string()));
        fill(&mut self.image, rect, rgb);
    }

    /// Draw several blocks in one colour at `peak`, reading together as
    /// `text`: a phrase whose glyphs stand apart.
    pub fn blocks(&mut self, rects: &[Rect], text: &str, peak: u8) {
        let rgb = dimmed(self.colour(text), peak);
        for rect in rects {
            fill(&mut self.image, *rect, rgb);
        }
    }

    /// Draw `text` filling `rect` word by word, one block per word as
    /// wide as its share of the letters, the words `gap` pixels apart, at
    /// `peak`. Each word is its own colour and reads as itself, as the
    /// recogniser reads a word cropped on its own.
    pub fn words(&mut self, rect: Rect, text: &str, gap: i32, peak: u8) {
        let words: Vec<&str> = text.split(' ').collect();
        let letters: usize = words.iter().map(|w| w.chars().count()).sum();
        let room = rect.width() - gap * (words.len() as i32 - 1);
        let mut x = rect.x0;
        for (k, word) in words.iter().enumerate() {
            let w = if k + 1 == words.len() {
                rect.x1 - x + 1
            } else {
                (room * word.chars().count() as i32 / letters.max(1) as i32).max(1)
            };
            self.text_at(Rect::new(x, rect.y0, x + w - 1, rect.y1), word, peak);
            x += w + gap;
        }
    }

    /// Draw a figure `h` tall with its top-left at `at`, its ink as wide
    /// as the client's numerals make `drawn`, that reads as `reads`.
    /// Returns where its ink is.
    pub fn figure(
        &mut self,
        at: (i32, i32),
        h: i32,
        drawn: &str,
        reads: &str,
        numerals: &NumeralMetrics,
    ) -> Rect {
        let units = expected_width(drawn, numerals).expect("a figure the metrics cover");
        let w = (units * h as f32).round() as i32;
        let rect = Rect::new(at.0, at.1, at.0 + w - 1, at.1 + h - 1);
        self.text(rect, reads);
        rect
    }

    /// Draw a figure as [`Lettering::figure`] does, no brighter than
    /// `peak`.
    pub fn figure_at(
        &mut self,
        at: (i32, i32),
        h: i32,
        drawn: &str,
        reads: &str,
        numerals: &NumeralMetrics,
        peak: u8,
    ) -> Rect {
        let units = expected_width(drawn, numerals).expect("a figure the metrics cover");
        let w = (units * h as f32).round() as i32;
        let rect = Rect::new(at.0, at.1, at.0 + w - 1, at.1 + h - 1);
        self.text_at(rect, reads, peak);
        rect
    }

    pub fn recogniser(&self) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
        let palette = self.palette.clone();
        move |crop: &BgrImage| {
            let mut counts: Vec<usize> = vec![0; palette.len()];
            for px in crop.data.chunks(3) {
                let rgb = [px[2], px[1], px[0]];
                if let Some(i) = palette.iter().position(|(c, _)| scaled_from(rgb, *c)) {
                    counts[i] += 1;
                }
            }
            let best = (0..palette.len()).max_by_key(|i| counts[*i]);
            let text = best
                .filter(|i| counts[*i] > 0)
                .map_or(String::new(), |i| palette[i].1.clone());
            Some((text, 0.99))
        }
    }
}

/// `rgb` scaled so its brightest channel is `peak`.
fn dimmed(rgb: [u8; 3], peak: u8) -> [u8; 3] {
    let top = rgb.iter().copied().max().unwrap_or(1).max(1) as f32;
    rgb.map(|c| (c as f32 * peak as f32 / top).round() as u8)
}

/// Whether `pixel` has `colour`'s proportions, each channel over the
/// brightest, to within what rounding a scaled colour leaves; dark pixels
/// are never matched, their proportions being too coarse to tell colours
/// apart.
fn scaled_from(pixel: [u8; 3], colour: [u8; 3]) -> bool {
    let top = colour.iter().copied().max().unwrap_or(0) as f32;
    let seen = pixel.iter().copied().max().unwrap_or(0) as f32;
    if top == 0.0 || seen < 40.0 {
        return false;
    }
    pixel
        .iter()
        .zip(colour)
        .all(|(p, c)| (*p as f32 / seen - c as f32 / top).abs() <= 0.02)
}

// A drawn skills window, its SKILLS tab showing. Each piece of text is
// a block of ink of its own colour (see [`Lettering`]), so the table's
// geometry is exercised without a real recogniser.

const SKILLS_SCENE: [u8; 3] = [120, 110, 90];
const RULE: [u8; 3] = [40, 44, 46];
const DIVIDER: [u8; 3] = [36, 40, 42];
const ICON: [u8; 3] = [90, 160, 230];

/// The grey of the table's scrollbar thumb.
pub const THUMB: [u8; 3] = [146, 146, 146];

/// The drawn table's row height.
pub const SKILLS_ROW_PITCH: i32 = 58;

/// Where the drawn skills window's pieces go, as offsets from its corner.
pub struct SkillsLayout {
    pub window: Rect,
    /// The name column's heading, and where its ink ends.
    pub name_header: (&'static str, i32),
    pub points_header: &'static str,
    pub pager: &'static str,
    /// The total, with its label.
    pub total: &'static str,
    /// Each row's name (`None` for the client's blank row) and points.
    pub rows: Vec<(Option<&'static str>, &'static str)>,
    /// Rows scrolled: the first row sits this far up under the header.
    pub scrolled: i32,
    /// The points' left edge, from the window's.
    pub points_x: i32,
    /// An extra upright line across the table, from the window's edge.
    pub stray_divider: Option<i32>,
    /// The scrollbar thumb's span, from the header's top.
    pub thumb: Option<(i32, i32)>,
    /// A pane drawn directly under the last row, the view ending on
    /// its rule.
    pub pane: bool,
}

/// The scene a drawn skills window sits on.
pub fn skills_scene() -> BgrImage {
    blank(1100, 900, SKILLS_SCENE)
}

/// A skills window drawn on a fresh scene, with its own lettering.
pub fn skills_window(layout: &SkillsLayout) -> Lettering {
    let mut d = Lettering::new(skills_scene());
    draw_skills_window(&mut d, layout);
    d
}

/// Draw a skills window into `d`'s image. Lettering already in `d`
/// keeps its colours, so one recogniser reads every window drawn
/// through the same `d`.
pub fn draw_skills_window(d: &mut Lettering, layout: &SkillsLayout) {
    let w = layout.window;
    draw_window(&mut d.image, w);
    d.text(
        Rect::new(w.x0 + 80, w.y0 + 22, w.x0 + 330, w.y0 + 33),
        "SKILLS & PROFESSIONS",
    );
    d.text(
        Rect::new(w.x0 + 20, w.y0 + 112, w.x0 + 150, w.y0 + 124),
        "ALL CATEGORIES",
    );
    d.text(
        Rect::new(w.x0 + 380, w.y0 + 112, w.x0 + 520, w.y0 + 124),
        layout.total,
    );
    d.text(
        Rect::new(w.x0 + 760, w.y0 + 112, w.x0 + 800, w.y0 + 124),
        layout.pager,
    );
    // The rules stop short of the window's right edge, leaving the
    // scrollbar's strip.
    let (left, right) = (w.x0 + 7, w.x1 - 30);
    let rule = |d: &mut Lettering, y: i32| fill(&mut d.image, Rect::new(left, y, right, y), RULE);
    let header_top = w.y0 + 150;
    rule(d, header_top);
    let header_labels = [
        (
            Rect::new(
                w.x0 + 30,
                header_top + 14,
                w.x0 + layout.name_header.1,
                header_top + 23,
            ),
            layout.name_header.0,
        ),
        (
            Rect::new(w.x0 + 330, header_top + 14, w.x0 + 360, header_top + 23),
            "Rank",
        ),
        (
            Rect::new(w.x0 + 820, header_top + 14, w.x0 + 860, header_top + 23),
            layout.points_header,
        ),
    ];
    for (rect, text) in header_labels {
        d.text(rect, text);
    }
    // The header is 32 pixels tall; the first row's top rule is its
    // bottom until the table is scrolled, when rows slide up under it.
    let first_top = header_top + 33 - layout.scrolled;
    let mut last_rule = header_top;
    let numerals = bundled_profile().read.numerals;
    for (k, (name, points)) in layout.rows.iter().enumerate() {
        let top = first_top + k as i32 * SKILLS_ROW_PITCH;
        for y in [top - 1, top + SKILLS_ROW_PITCH - 1] {
            if y > header_top {
                rule(d, y);
                last_rule = last_rule.max(y);
            }
        }
        // What of a row lies under the header shows through it, but
        // only below the header's top and clear of its labels.
        let visible = |r: Rect| {
            (r.y0 > header_top
                && header_labels
                    .iter()
                    .all(|(label, _)| label.padded(4).intersect(&r).is_none()))
            .then_some(r)
        };
        if let Some(icon) = visible(Rect::new(w.x0 + 20, top + 8, w.x0 + 60, top + 48)) {
            fill(&mut d.image, icon, ICON);
        }
        if let Some(name) = name {
            if let Some(r) = visible(Rect::new(w.x0 + 80, top + 22, w.x0 + 230, top + 35)) {
                d.text(r, name);
            }
        }
        if let Some(r) = visible(Rect::new(w.x0 + 420, top + 22, w.x0 + 500, top + 35)) {
            d.text(r, "Newbie");
        }
        let points_x = w.x0 + layout.points_x;
        // As wide as the client's numerals would make this figure.
        let width =
            expected_width(points, &numerals).map_or(50, |units| (units * 14.0).round() as i32);
        if let Some(r) = visible(Rect::new(
            points_x,
            top + 22,
            points_x + width - 1,
            top + 35,
        )) {
            d.text(r, points);
        }
    }
    let mut dividers = vec![w.x0 + 300, w.x0 + 600, w.x0 + 760];
    dividers.extend(layout.stray_divider.map(|x| w.x0 + x));
    for x in dividers {
        fill(
            &mut d.image,
            Rect::new(x, header_top + 1, x, last_rule - 1),
            DIVIDER,
        );
    }
    if layout.pane {
        // The pane's own frame, its top two pixels below the last
        // row's rule, as the description pane meets the table.
        for (inset, rgb) in [(2, [45u8, 49, 49]), (3, [76, 82, 84])] {
            fill(
                &mut d.image,
                Rect::new(left, last_rule + inset, right + 20, last_rule + inset),
                rgb,
            );
        }
    }
    if let Some((top, bottom)) = layout.thumb {
        fill(
            &mut d.image,
            Rect::new(
                w.x1 - 24,
                header_top + 1 + top,
                w.x1 - 13,
                header_top + 1 + bottom,
            ),
            THUMB,
        );
    }
}

/// The skills the drawn windows name.
pub fn skills_drawing_vocabulary() -> SkillVocabulary {
    SkillVocabulary::new(
        ["Agility", "Aim", "Alertness", "Analysis"]
            .map(String::from)
            .to_vec(),
        Vec::new(),
    )
}

/// A whole page of four rows, the blank row first, the list fitting the
/// window.
pub fn standard_skills_layout() -> SkillsLayout {
    SkillsLayout {
        window: Rect::new(40, 30, 1000, 860),
        name_header: ("Skill Name", 100),
        points_header: "Points",
        pager: "2 / 13",
        total: "Total: 198259",
        rows: vec![
            (None, "1"),
            (Some("Agility"), "73.71"),
            (Some("Aim"), "1,268.53"),
            (Some("Alertness"), "3,068.94"),
        ],
        scrolled: 0,
        points_x: 830,
        stray_divider: None,
        thumb: None,
        pane: false,
    }
}

/// A small deterministic generator, so randomised layouts replay.
pub struct Lcg(pub u64);

impl Lcg {
    fn step(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    /// A value in `[lo, hi]`.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.step() % (hi - lo + 1) as u64) as i32
    }

    /// A value in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        self.step() as f64 / (1u64 << 31) as f64
    }

    /// A value in `[lo, hi)`.
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

// A drawn radar: the disc, its rings and rim, the player's dot, and the
// lettering the client prints on and under it. Each piece of lettering
// is drawn as white glyph outlines on its own dark shadow, the way the
// client sets lettering over the scene and its ring distances over the
// map, each in a faint tint of its own. The stand-in recogniser reads a
// crop as the texts whose tints it holds, left to right, so lettering
// that touches reads as the client's does: `50 m100 m`.

pub const MAP: [u8; 3] = [59, 146, 55];
pub const RING: [u8; 3] = [111, 171, 131];
pub const RIM: [u8; 3] = [28, 30, 29];
pub const BEVEL: [u8; 3] = [92, 92, 92];
pub const DOT: [u8; 3] = [250, 255, 251];
pub const WHITE: [u8; 3] = [255, 255, 250];
pub const PALE_GREEN: [u8; 3] = [196, 240, 200];
pub const SHADOW: [u8; 3] = [22, 30, 22];

/// The rings the client draws at the default zoom, in pixels from the
/// centre of a radar of radius 113.
pub const RINGS: [f64; 3] = [26.0, 52.0, 103.0];

/// The distances the client prints on those rings at the default zoom.
pub const RING_LABELS: [&str; 3] = ["50 m", "100 m", "200 m"];

/// The digit pitch of the drawn coordinates, and the lettering's height.
pub const PITCH: i32 = 8;
pub const HEIGHT: i32 = 11;

/// Scenery: a dull, uneven ground with nothing green or white in it.
pub fn scenery(w: usize, h: usize, seed: u64) -> BgrImage {
    let mut rng = Lcg(seed);
    let mut image = blank(w, h, [0, 0, 0]);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let v = rng.range(-25, 25);
            let base = [130 + v, 112 + v, 90 + v / 2];
            put(&mut image, x, y, base.map(|c| c as u8));
        }
    }
    image
}

/// Whether pixel `(x, y)` lies inside the circle about `(cx, cy)`.
pub fn within(x: i32, y: i32, (cx, cy): (f64, f64), r: f64) -> bool {
    let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
    dx * dx + dy * dy < r * r
}

/// Draw a radar the way the client does: the green map, its range
/// rings, a dark line and some mobs across the map, the player's dot,
/// and a dark rim with round buttons set into it. `centre` is the pixel
/// the dot is drawn on; `rings` are radii in pixels.
pub fn draw_radar(image: &mut BgrImage, centre: (i32, i32), radius: i32, rings: &[f64], dot: bool) {
    let (cx, cy) = (centre.0 as f64 + 0.5, centre.1 as f64 + 0.5);
    let r = radius as f64;
    let rim = (r * 0.12).max(8.0);
    let reach = (r + rim).ceil() as i32 + 1;
    for y in centre.1 - reach..=centre.1 + reach {
        for x in centre.0 - reach..=centre.0 + reach {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            if d < r {
                let ring = rings.iter().any(|ring| (d - ring).abs() < 0.5);
                put(image, x, y, if ring { RING } else { MAP });
            } else if d < r + rim {
                let bevel = (d - r - rim / 2.0).abs() < 1.0;
                put(image, x, y, if bevel { BEVEL } else { RIM });
            }
        }
    }
    // A dark contour line across the map, and a cluster of mobs.
    for k in -(radius * 3 / 4)..(radius * 3 / 4) {
        for t in 0..2 {
            put(
                image,
                centre.0 + k,
                centre.1 + radius / 3 + k / 4 + t,
                [30, 46, 28],
            );
        }
    }
    for (mx, my) in [
        (radius / 4, -radius / 3),
        (radius / 5, -radius / 4),
        (-radius / 2, radius / 6),
    ] {
        fill(
            image,
            Rect::new(
                centre.0 + mx,
                centre.1 + my,
                centre.0 + mx + 4,
                centre.1 + my + 4,
            ),
            [220, 30, 30],
        );
    }
    // Buttons straddling the rim on the upper left and lower right.
    for degrees in [200.0f64, 215.0, 230.0, 245.0, 30.0, 45.0] {
        let a = degrees.to_radians();
        let (bx, by) = (
            cx + (r + rim / 2.0) * a.cos(),
            cy + (r + rim / 2.0) * a.sin(),
        );
        let br = rim * 0.9;
        for y in (by - br) as i32..=(by + br) as i32 {
            for x in (bx - br) as i32..=(bx + br) as i32 {
                if within(x, y, (bx, by), br) {
                    put(
                        image,
                        x,
                        y,
                        if within(x, y, (bx, by), br * 0.5) {
                            [235, 235, 235]
                        } else {
                            [12, 12, 12]
                        },
                    );
                }
            }
        }
    }
    if dot {
        let s = (radius / 40).max(1);
        fill(
            image,
            Rect::new(
                centre.0 - s / 2,
                centre.1 - s / 2,
                centre.0 - s / 2 + s,
                centre.1 - s / 2 + s,
            ),
            DOT,
        );
    }
}

/// How a coordinate's lettering is drawn: the label's ink, the space,
/// and `digits` digit glyphs at `pitch`, the last ending two columns
/// short of a whole pitch as the client's do.
#[derive(Clone, Copy)]
pub struct Printed {
    pub pitch: i32,
    pub digits: i32,
    pub space: bool,
}

impl Printed {
    pub fn of(digits: i32) -> Self {
        Self {
            pitch: PITCH,
            digits,
            space: true,
        }
    }

    fn widths(&self, value: &CoordinateProfile) -> (i32, i32, i32) {
        let p = self.pitch as f64;
        let label = (value.label_pitches * p).round() as i32;
        let space = if self.space {
            (value.space_pitches * p).round() as i32
        } else {
            0
        };
        let digits = if self.digits > 0 {
            self.digits * self.pitch - 2
        } else {
            0
        };
        (label, space, digits)
    }
}

/// A scene of the game world with what the client draws straight onto it
/// (a radar, the coordinates under it, creatures' nameplates), and the
/// stand-in recogniser for the lettering drawn there.
pub struct RadarScene {
    pub image: BgrImage,
    palette: Vec<([u8; 3], String)>,
}

impl RadarScene {
    pub fn new(seed: u64) -> Self {
        Self::sized(1280, 760, seed)
    }

    pub fn sized(w: usize, h: usize, seed: u64) -> Self {
        Self {
            image: scenery(w, h, seed),
            palette: Vec::new(),
        }
    }

    /// A scene of `image`, with no lettering drawn on it yet.
    pub fn over(image: BgrImage) -> Self {
        Self {
            image,
            palette: Vec::new(),
        }
    }

    /// Lettering the way the client prints it on the scene: `glyphs`
    /// glyph outlines in a tint of `base` filling `rect`, on their own
    /// dark shadow, read as `text`.
    pub fn lettering(&mut self, rect: Rect, glyphs: i32, base: [u8; 3], text: &str) {
        fill(&mut self.image, rect.padded(1), SHADOW);
        let tint = self.tint(base, text);
        self.glyphs((rect.x0, rect.x1), rect.y0, rect.height(), glyphs, tint);
    }

    /// A creature's nameplate the way the client draws it (see
    /// [`draw_plate_bar`]), with `level` and `name` printed over the bar
    /// on one line (see [`PlateDrawing::line`]) and the status icon some
    /// way after the name. Returns the bar.
    pub fn plate(&mut self, plate: &PlateDrawing, level: &str, name: &str) -> Rect {
        let bar = draw_plate_bar(&mut self.image, plate);
        let line = self.plate_line(plate, bar, level, name);
        let s = plate.geometry.factor();
        draw_status_icon(
            &mut self.image,
            (
                line.x1 + (51.0 * s).round() as i32,
                bar.y0 - (18.0 * s).round() as i32,
            ),
        );
        bar
    }

    /// The level and the name over the bar at `bar`, as the client sets
    /// them at the default scale (scaled for another): the level's
    /// capitals 9 pixels tall from 21 above the bar at a pitch of 19/3, a
    /// gap of 12, then the name 13 tall (its descenders included) from 22
    /// above the bar at a pitch of 7. Returns the line's rectangle.
    pub fn plate_line(&mut self, plate: &PlateDrawing, bar: Rect, level: &str, name: &str) -> Rect {
        let s = plate.geometry.factor();
        let px = |v: f64| (v * s).round() as i32;
        let level_chars = level.chars().count() as i32;
        let name_chars = name.chars().count() as i32;
        let level_width = px(level_chars as f64 * LEVEL_PITCH).max(1);
        let name_width = if name_chars == 0 {
            0
        } else {
            name_chars * px(NAME_PITCH) - 2
        };
        let gap = px(12.0);
        let width = level_width + if name_chars == 0 { 0 } else { gap + name_width };
        let x0 = match plate.line {
            LineAlign::Left => bar.x0 + px(7.0),
            LineAlign::Centred => bar.x0 + (bar.width() - width) / 2,
        };
        let level_rect = Rect::new(
            x0,
            bar.y0 - px(21.0),
            x0 + level_width - 1,
            bar.y0 - px(21.0) + px(9.0) - 1,
        );
        self.lettering(level_rect, level_chars.max(1), LEVEL_TINT, level);
        if name_chars == 0 {
            return level_rect;
        }
        let name_rect = Rect::new(
            level_rect.x1 + gap + 1,
            bar.y0 - px(22.0),
            level_rect.x1 + gap + name_width,
            bar.y0 - px(22.0) + px(13.0) - 1,
        );
        self.lettering(name_rect, name_chars, WHITE, name);
        Rect::new(level_rect.x0, name_rect.y0, name_rect.x1, name_rect.y1)
    }

    /// A radar with its coordinates printed under it: `lon` and `lat` are
    /// what the recogniser reads, each drawn as `Printed` says, or not
    /// drawn when `None`.
    pub fn with_coordinates(
        centre: (i32, i32),
        radius: i32,
        lon: Option<(&str, Printed)>,
        lat: Option<(&str, Printed)>,
    ) -> Self {
        let mut scene = Self::new(radius as u64 + 99);
        scene.radar(centre, radius, &RINGS);
        scene.coordinates(centre, radius, lon, lat);
        scene
    }

    /// Draw a radar with its player's dot.
    pub fn radar(&mut self, centre: (i32, i32), radius: i32, rings: &[f64]) {
        draw_radar(&mut self.image, centre, radius, rings, true);
    }

    /// Print coordinates under the radar at `centre` the way the client
    /// lays them out: `LON` from the disc's left edge, `LAT` ending a
    /// little short of its right edge.
    pub fn coordinates(
        &mut self,
        centre: (i32, i32),
        radius: i32,
        lon: Option<(&str, Printed)>,
        lat: Option<(&str, Printed)>,
    ) {
        let coords = bundled_profile().radar.coordinates;
        let top = centre.1 + radius + (radius as f64 * 0.27) as i32;
        if let Some((text, drawn)) = lon {
            self.value(centre.0 - radius, top, &coords.lon, drawn, text);
        }
        if let Some((text, drawn)) = lat {
            let right = centre.0 + radius - (0.6 * drawn.pitch as f64).round() as i32;
            let width = Self::width_of(&coords.lat, drawn);
            self.value(right - width + 1, top, &coords.lat, drawn, text);
        }
    }

    /// Print each ring's distance over it, the way the client labels its
    /// rings.
    pub fn ring_labels(&mut self, centre: (i32, i32), rings: &[f64], labels: &[&str]) {
        for (ring, label) in rings.iter().zip(labels) {
            self.ring_label(centre, *ring, label);
        }
    }

    fn tint(&mut self, base: [u8; 3], text: &str) -> [u8; 3] {
        let i = self.palette.len() as u8;
        let tint = [base[0] - i * 2, base[1], base[2] - i];
        self.palette.push((tint, text.to_string()));
        tint
    }

    /// Glyph outlines filling columns `x0..=x1`, from `y0`, `height`
    /// tall, the first at `x0` and the last ending at `x1`; nothing is
    /// drawn when `x1 < x0`.
    fn glyphs(&mut self, (x0, x1): (i32, i32), y0: i32, height: i32, count: i32, tint: [u8; 3]) {
        if x1 < x0 || count < 1 {
            return;
        }
        let glyph = if count == 1 {
            x1 - x0 + 1
        } else {
            ((x1 - x0 + 1) - 2 * (count - 1)) / count
        };
        for k in 0..count {
            let gx = if k == count - 1 {
                x1 - glyph + 1
            } else {
                x0 + k * ((x1 - x0 + 1 - glyph) / (count - 1).max(1))
            };
            let y1 = y0 + height - 1;
            for x in gx..gx + glyph {
                put(&mut self.image, x, y0, tint);
                put(&mut self.image, x, y1, tint);
            }
            for y in y0..=y1 {
                put(&mut self.image, gx, y, tint);
                put(&mut self.image, gx + glyph - 1, y, tint);
            }
        }
    }

    pub fn recogniser(&self) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
        let palette = self.palette.clone();
        move |crop: &BgrImage| {
            let mut seen: Vec<(usize, f64, usize)> = Vec::new();
            for (i, px) in crop.data.chunks(3).enumerate() {
                let rgb = [px[2], px[1], px[0]];
                if let Some(k) = palette.iter().position(|(c, _)| *c == rgb) {
                    let x = (i % crop.w) as f64;
                    match seen.iter_mut().find(|s| s.0 == k) {
                        Some(s) => {
                            s.1 += x;
                            s.2 += 1;
                        }
                        None => seen.push((k, x, 1)),
                    }
                }
            }
            seen.sort_by(|a, b| (a.1 / a.2 as f64).total_cmp(&(b.1 / b.2 as f64)));
            let text: String = seen.iter().map(|s| palette[s.0].1.clone()).collect();
            Some((text, 0.99))
        }
    }

    /// Draw a value's lettering with its left end at `x0`, read as
    /// `text`; returns its rectangle.
    pub fn value(
        &mut self,
        x0: i32,
        y0: i32,
        value: &CoordinateProfile,
        drawn: Printed,
        text: &str,
    ) -> Rect {
        let (label, space, digits) = drawn.widths(value);
        let x1 = x0 + label + space + digits - 1;
        let rect = Rect::new(x0, y0, x1, y0 + HEIGHT - 1);
        fill(&mut self.image, rect.padded(1), SHADOW);
        let tint = self.tint(WHITE, text);
        self.glyphs((x0, x0 + label - 1), y0, HEIGHT, 3, tint);
        let d0 = x0 + label + space;
        for k in 0..drawn.digits {
            let gx = d0 + k * drawn.pitch;
            self.glyphs((gx, gx + drawn.pitch - 3), y0, HEIGHT, 1, tint);
        }
        rect
    }

    /// The width of a value's lettering as drawn.
    pub fn width_of(value: &CoordinateProfile, drawn: Printed) -> i32 {
        let (label, space, digits) = drawn.widths(value);
        label + space + digits
    }

    /// The planet's name, centred above the disc of the radar at `centre`
    /// the way the client prints it; returns its rectangle.
    pub fn planet_label(&mut self, centre: (i32, i32), radius: i32, text: &str) -> Rect {
        let chars = text.chars().count() as i32;
        let width = chars * 7 - 2;
        let top = centre.1 - radius - (radius as f64 * 0.25) as i32 - HEIGHT / 2;
        let rect = Rect::new(
            centre.0 - width / 2,
            top,
            centre.0 - width / 2 + width - 1,
            top + HEIGHT - 1,
        );
        fill(&mut self.image, rect.padded(1), SHADOW);
        let tint = self.tint(WHITE, text);
        self.glyphs((rect.x0, rect.x1), rect.y0, HEIGHT, chars, tint);
        rect
    }

    /// A ring's distance label, centred over the ring where it crosses
    /// the line to the disc's right edge.
    pub fn ring_label(&mut self, centre: (i32, i32), radius: f64, text: &str) {
        let chars = text.chars().count() as i32;
        let width = chars * 6 - 2;
        let x0 = centre.0 + radius.round() as i32 - width / 2;
        let rect = Rect::new(x0, centre.1 - 3, x0 + width - 1, centre.1 + 3);
        fill(&mut self.image, rect.padded(1), SHADOW);
        let tint = self.tint(PALE_GREEN, text);
        self.glyphs((rect.x0, rect.x1), rect.y0, rect.height(), chars, tint);
    }
}

// A drawn auction sell window: the inventory list on the left, and the
// sell queue's ruled table beside it, each value drawn as a block of ink
// in its own colour (see [`Lettering`]) at the brightness the client
// draws it: spinners' values bright or greyed, the figures under them
// and their helpers dim, the buyout's tag lit while a buyout is set.

/// One queued row as drawn: each spinner's value and whether it is
/// greyed.
#[derive(Debug, Clone)]
pub struct SaleRowDrawing {
    pub name: &'static str,
    pub tt: &'static str,
    pub quantity: (&'static str, bool),
    /// The stack size printed under an editable quantity.
    pub stack: &'static str,
    pub opening_bid: (&'static str, bool),
    /// The figure under the opening bid's spinner.
    pub bid_figure: &'static str,
    pub buyout: (&'static str, bool),
    /// Whether the buyout's tag is lit.
    pub buyout_lit: bool,
    /// The figure under the buyout's spinner, where the client prints
    /// one.
    pub buyout_figure: Option<&'static str>,
    pub days: &'static str,
    /// The range printed under the days.
    pub days_range: &'static str,
    pub fee: &'static str,
    /// The markup headline.
    pub markup: &'static str,
}

/// Where the drawn sell window's pieces go.
#[derive(Debug, Clone)]
pub struct SaleLayout {
    pub window: Rect,
    /// The heading's text over the queue, and the count it ends with,
    /// drawn as the count's own glyphs between brackets.
    pub heading: (&'static str, &'static str),
    pub rows: Vec<SaleRowDrawing>,
    /// The last row cut short by the bottom of the view by this many
    /// pixels.
    pub cut_last: Option<i32>,
    /// The names the inventory list shows, with their counts.
    pub inventory: Vec<(&'static str, &'static str)>,
    /// How bright a greyed spinner's value is drawn.
    pub locked_peak: u8,
    /// How bright an editable spinner's value is drawn.
    pub editable_peak: u8,
    /// The column dividers left undrawn, by their place among the
    /// columns' edges (1 is the one after the name column).
    pub hidden_dividers: Vec<usize>,
    /// The price toggle showing markup, not PED.
    pub markup_mode: bool,
    /// How wide the figures are drawn, in digit heights: the bundled
    /// profile's numerals for the drawn scale unless a test draws them
    /// as another scale's would be.
    pub numerals: NumeralMetrics,
}

/// The drawn queue's row height, and its header's.
pub const SALE_ROW_PITCH: i32 = 112;
pub const SALE_HEADER_HEIGHT: i32 = 30;

/// The drawn queue's columns: heading and width, left to right.
pub const SALE_COLUMNS: [(&str, i32); 8] = [
    ("NAME", 300),
    ("QUANTITY", 80),
    ("OPENING BID", 110),
    ("BUYOUT", 140),
    ("DAYS", 56),
    ("FEE", 70),
    ("MARKUP", 156),
    ("", 102),
];

const BOX_LIVE: [u8; 3] = [74, 74, 74];
const BOX_LOCKED: [u8; 3] = [24, 27, 27];
const ARROW_LIVE: [u8; 3] = [115, 115, 115];
const ARROW_LOCKED: [u8; 3] = [32, 32, 32];
const PICTURE: [u8; 3] = [140, 220, 230];
const PICTURE_FRAME: [u8; 3] = [33, 36, 36];
const TAG_LIT: [u8; 3] = [112, 185, 59];
const TAG_UNLIT: [u8; 3] = [131, 142, 143];
const SIDE_ICON: [u8; 3] = [150, 150, 150];

/// The scene a drawn sell window sits on.
pub fn sale_scene() -> BgrImage {
    blank(1400, 840, SKILLS_SCENE)
}

/// A sell window drawn on a fresh scene, with its own lettering.
pub fn sale_window(layout: &SaleLayout) -> Lettering {
    let mut d = Lettering::new(sale_scene());
    draw_sale_window(&mut d, layout);
    d
}

/// The sell queue's numerals at the scale the drawn queue is drawn at,
/// as the bundled profile measures them: its entry claiming the drawn
/// header's height.
pub fn sale_numerals() -> NumeralMetrics {
    let queue = bundled_profile().windows["auction_sell"]
        .sale_queue
        .clone()
        .expect("the bundled profile describes the sell queue");
    queue
        .scales
        .into_iter()
        .find(|s| (s.header_height[0]..=s.header_height[1]).contains(&SALE_HEADER_HEIGHT))
        .expect("the bundled profile lists the drawn queue's scale")
        .numerals
}

/// The left edges of the drawn queue's columns, and the table's right
/// edge.
pub fn sale_column_edges(window: Rect) -> Vec<i32> {
    let mut x = window.x0 + 330;
    let mut edges = vec![x];
    for (_, width) in SALE_COLUMNS {
        x += width;
        edges.push(x);
    }
    edges
}

/// The top row of the drawn queue's row `k`.
pub fn sale_row_top(window: Rect, k: usize) -> i32 {
    window.y0 + 110 + SALE_HEADER_HEIGHT + 2 + k as i32 * (SALE_ROW_PITCH + 1)
}

/// Draw a box's outline.
fn outline(image: &mut BgrImage, r: Rect, rgb: [u8; 3]) {
    fill(image, Rect::new(r.x0, r.y0, r.x1, r.y0), rgb);
    fill(image, Rect::new(r.x0, r.y1, r.x1, r.y1), rgb);
    fill(image, Rect::new(r.x0, r.y0, r.x0, r.y1), rgb);
    fill(image, Rect::new(r.x1, r.y0, r.x1, r.y1), rgb);
}

/// Draw a spinner `2 * half + 1` wide centred on `cx` in the row whose
/// top is `top`: its arrows' boxes above and below the value's, the
/// value bright or greyed; returns the value box's outline.
fn draw_spinner(
    d: &mut Lettering,
    (cx, half): (i32, i32),
    top: i32,
    (value, locked): (&str, bool),
    layout: &SaleLayout,
) -> Rect {
    let frame = if locked { BOX_LOCKED } else { BOX_LIVE };
    let arrow = if locked { ARROW_LOCKED } else { ARROW_LIVE };
    let up = Rect::new(cx - half, top + 15, cx + half, top + 30);
    let value_box = Rect::new(cx - half, top + 33, cx + half, top + 64);
    let down = Rect::new(cx - half, top + 64, cx + half, top + 78);
    for r in [up, value_box, down] {
        outline(&mut d.image, r, frame);
    }
    fill(
        &mut d.image,
        Rect::new(cx - 3, top + 21, cx + 3, top + 23),
        arrow,
    );
    fill(
        &mut d.image,
        Rect::new(cx - 3, top + 70, cx + 3, top + 72),
        arrow,
    );
    let numerals = &layout.numerals;
    let width = (expected_width(value, numerals).expect("a number") * 10.0).round() as i32;
    let peak = if locked {
        layout.locked_peak
    } else {
        layout.editable_peak
    };
    d.figure_at((cx - width / 2, top + 44), 10, value, value, numerals, peak);
    value_box
}

/// Draw a dim figure centred under a spinner on `cx`.
fn draw_figure_under(d: &mut Lettering, cx: i32, top: i32, text: &str, numerals: &NumeralMetrics) {
    match expected_width(text, numerals) {
        Some(units) => {
            let width = (units * 8.0).round() as i32;
            d.figure_at((cx - width / 2, top + 88), 8, text, text, numerals, 120);
        }
        None => {
            let width = 6 * text.chars().count() as i32;
            d.text_at(
                Rect::new(
                    cx - width / 2,
                    top + 88,
                    cx - width / 2 + width - 1,
                    top + 95,
                ),
                text,
                120,
            );
        }
    }
}

/// Draw a sell window into `d`'s image.
pub fn draw_sale_window(d: &mut Lettering, layout: &SaleLayout) {
    let w = layout.window;
    draw_window(&mut d.image, w);
    d.text(
        Rect::new(w.x0 + 80, w.y0 + 22, w.x0 + 330, w.y0 + 33),
        "AUCTION / SELL",
    );
    let numerals = layout.numerals.clone();
    let edges = sale_column_edges(w);
    let (left, right) = (edges[0], *edges.last().expect("edges"));
    // The heading: its words, then the count between brackets taller
    // than the digits, all one phrase.
    let (heading, count) = layout.heading;
    let y = w.y0 + 70;
    let label = Rect::new(left + 30, y, left + 30 + 7 * heading.len() as i32, y + 9);
    // As wide as the client's numerals make the bracketed count, at
    // the brackets' height.
    let span =
        (expected_width(&format!("({count})"), &numerals).expect("a count") * 12.0).round() as i32;
    let open = Rect::new(label.x1 + 5, y - 1, label.x1 + 6, y + 10);
    let close = Rect::new(open.x0 + span - 2, y - 1, open.x0 + span - 1, y + 10);
    let number = Rect::new(open.x1 + 2, y, close.x0 - 2, y + 9);
    d.blocks(
        &[label, open, number, close],
        &format!("{heading} ({count})"),
        250,
    );
    // The price toggle at the top right: the mode in force is lit.
    let ped = Rect::new(right - 150, y, right - 126, y + 9);
    let markup = Rect::new(right - 110, y, right - 64, y + 9);
    if layout.markup_mode {
        d.text_unlit(ped, "PED");
        d.text(markup, "Markup");
    } else {
        d.text(ped, "PED");
        d.text_unlit(markup, "Markup");
    }
    // The inventory list: each name in the list's smaller type.
    for (j, (name, count)) in layout.inventory.iter().enumerate() {
        let y = w.y0 + 160 + 28 * j as i32;
        let width = 6 * name.chars().count() as i32;
        d.text(Rect::new(w.x0 + 30, y, w.x0 + 30 + width - 1, y + 9), name);
        let digits = (expected_width(count, &numerals).expect("a count") * 10.0).round() as i32;
        d.text(Rect::new(w.x0 + 260, y, w.x0 + 259 + digits, y + 9), count);
    }
    let rule = |d: &mut Lettering, y: i32| fill(&mut d.image, Rect::new(left, y, right, y), RULE);
    let header_top = w.y0 + 110;
    rule(d, header_top);
    rule(d, header_top + SALE_HEADER_HEIGHT + 1);
    for (k, (heading, _)) in SALE_COLUMNS.iter().enumerate() {
        if heading.is_empty() {
            continue;
        }
        let width = 8 * heading.chars().count() as i32;
        let x0 = if k == 0 {
            edges[k] + 8
        } else {
            (edges[k] + edges[k + 1]) / 2 - width / 2
        };
        d.text(
            Rect::new(x0, header_top + 11, x0 + width - 1, header_top + 19),
            heading,
        );
    }
    let mut bottom = header_top + SALE_HEADER_HEIGHT + 1;
    for (k, row) in layout.rows.iter().enumerate() {
        let top = sale_row_top(w, k);
        let centre = |c: usize| (edges[c] + edges[c + 1]) / 2;
        // The name cell: a picture in its dim frame, then the name over
        // the TT value.
        let x0 = edges[0] + 1;
        let frame = Rect::new(x0 + 8, top + 37, x0 + 48, top + 77);
        outline(&mut d.image, frame, PICTURE_FRAME);
        fill(
            &mut d.image,
            Rect::new(x0 + 14, top + 42, x0 + 42, top + 72),
            PICTURE,
        );
        let width = 7 * row.name.chars().count() as i32;
        d.words(
            Rect::new(x0 + 60, top + 42, x0 + 60 + width - 1, top + 52),
            row.name,
            4,
            232,
        );
        d.figure_at((x0 + 60, top + 63), 8, row.tt, row.tt, &numerals, 230);
        draw_spinner(d, (centre(1), 32), top, row.quantity, layout);
        if !row.quantity.1 {
            draw_figure_under(d, centre(1), top, row.stack, &numerals);
        }
        let bid = draw_spinner(d, (centre(2), 32), top, row.opening_bid, layout);
        d.text_at(
            Rect::new(bid.x1 + 4, top + 45, bid.x1 + 16, top + 52),
            "0-9",
            120,
        );
        draw_figure_under(d, centre(2), top, row.bid_figure, &numerals);
        let buyout = draw_spinner(d, (centre(3), 32), top, row.buyout, layout);
        d.text_at(
            Rect::new(buyout.x1 + 4, top + 45, buyout.x1 + 16, top + 52),
            "0-9",
            120,
        );
        outline(
            &mut d.image,
            Rect::new(buyout.x1 + 22, top + 44, buyout.x1 + 32, top + 54),
            if row.buyout_lit { TAG_LIT } else { TAG_UNLIT },
        );
        draw_figure_under(
            d,
            centre(3),
            top,
            row.buyout_figure.unwrap_or("-"),
            &numerals,
        );
        draw_spinner(d, (centre(4), 18), top, (row.days, false), layout);
        draw_figure_under(d, centre(4), top, row.days_range, &numerals);
        let fee_width =
            (expected_width(row.fee, &numerals).expect("an amount") * 9.0).round() as i32;
        d.figure_at(
            (centre(5) - fee_width / 2, top + 50),
            9,
            row.fee,
            row.fee,
            &numerals,
            201,
        );
        let head_width =
            (expected_width(row.markup, &numerals).expect("a markup") * 10.0).round() as i32;
        d.figure_at(
            (centre(6) - head_width / 2, top + 15),
            10,
            row.markup,
            row.markup,
            &numerals,
            234,
        );
        for (j, stat) in ["D N/A", "W N/A", "M +141.00 1"].iter().enumerate() {
            let y = top + 32 + 12 * j as i32;
            d.text_at(
                Rect::new(centre(6) - 45, y, centre(6) + 45, y + 7),
                stat,
                198,
            );
        }
        for j in 0..3 {
            let y = top + 15 + 30 * j;
            fill(
                &mut d.image,
                Rect::new(centre(7) - 5, y, centre(7) + 5, y + 10),
                SIDE_ICON,
            );
        }
        let last = k + 1 == layout.rows.len();
        match layout.cut_last {
            Some(cut) if last => {
                // The view ends inside the row: below the pane's edge
                // nothing of it is drawn.
                let edge = top + SALE_ROW_PITCH - cut;
                fill(
                    &mut d.image,
                    Rect::new(left, edge, right, w.y1 - 4),
                    [8, 11, 11],
                );
                rule(d, edge);
                bottom = edge;
            }
            _ => {
                rule(d, top + SALE_ROW_PITCH);
                bottom = top + SALE_ROW_PITCH;
            }
        }
    }
    for (k, x) in edges.iter().enumerate().take(edges.len() - 1).skip(1) {
        if layout.hidden_dividers.contains(&k) {
            continue;
        }
        fill(
            &mut d.image,
            Rect::new(*x, header_top + 1, *x, bottom - 1),
            DIVIDER,
        );
    }
}

/// Three queued rows the way the client shows them: an item whose markup
/// is stated in PED, its buyout set; a stack whose markup is a
/// percentage, its buyout switched off; and a single item with its
/// quantity greyed and its buyout switched off. The inventory list shows
/// none of them.
pub fn standard_sale_layout() -> SaleLayout {
    SaleLayout {
        window: Rect::new(20, 20, 1380, 800),
        heading: ("Sell Queue", "3"),
        rows: vec![
            SaleRowDrawing {
                name: "Electric Attack Nanochip 3",
                tt: "11.52 PED",
                quantity: ("1", true),
                stack: "",
                opening_bid: ("162", false),
                bid_figure: "TT+150.47 PED",
                buyout: ("172", false),
                buyout_lit: true,
                buyout_figure: Some("TT+160.47 PED"),
                days: "7",
                days_range: "1-7",
                fee: "7.33 PED",
                markup: "TT+160.47 PED",
            },
            SaleRowDrawing {
                name: "Output Amplifier Component",
                tt: "20.80 PED",
                quantity: ("52", false),
                stack: "/ 52",
                opening_bid: ("42", false),
                bid_figure: "201.92%",
                buyout: ("0", true),
                buyout_lit: false,
                buyout_figure: None,
                days: "7",
                days_range: "1-7",
                fee: "1.54 PED",
                markup: "201.92%",
            },
            SaleRowDrawing {
                name: "Skyripper Loyalist (L)",
                tt: "24.54 PED",
                quantity: ("1", true),
                stack: "",
                opening_bid: ("30", false),
                bid_figure: "122.25%",
                buyout: ("30", true),
                buyout_lit: false,
                buyout_figure: None,
                days: "7",
                days_range: "1-7",
                fee: "0.77 PED",
                markup: "122.25%",
            },
        ],
        cut_last: None,
        inventory: vec![
            ("Animal Hide", "2"),
            ("Entropia Unreal Token", "20"),
            ("Mind Essence", "454733"),
        ],
        locked_peak: 66,
        editable_peak: 232,
        hidden_dividers: Vec::new(),
        markup_mode: false,
        numerals: sale_numerals(),
    }
}

// Creatures' nameplates, drawn the way the client draws them over the
// scene at the default interface scale: the bar's coloured part 164 by 6
// pixels, inside a two-pixel bevel and a one-pixel frame, the level and
// the name printed above it (as lettering on the scene, see
// [`RadarScene::plate`]). The locked plate is drawn solid, every other
// plate washed out: blended half and half with the scene behind it.

/// A plate's pixel sizes: the bar's coloured part `(width, height)`, the
/// frame's thickness and the bevel's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlateGeometry {
    pub bar: (i32, i32),
    pub frame: i32,
    pub bevel: i32,
}

impl PlateGeometry {
    /// How much larger than the default scale this geometry is drawn.
    pub fn factor(&self) -> f64 {
        self.bar.0 as f64 / PLATE_DEFAULT.bar.0 as f64
    }
}

/// The plate at the default interface scale, as measured on captures.
pub const PLATE_DEFAULT: PlateGeometry = PlateGeometry {
    bar: (164, 6),
    frame: 1,
    bevel: 2,
};

/// Where the line of lettering over a plate starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineAlign {
    /// Seven pixels in from the bar's left end (scaled), as the captures
    /// show it.
    Left,
    /// Centred on the bar, which the captures equally fit.
    Centred,
}

pub const PLATE_FRAME: [u8; 3] = [3, 7, 5];
pub const PLATE_BEVEL: [u8; 3] = [60, 68, 66];
pub const PLATE_EMPTY: [u8; 3] = [6, 11, 8];
const PLATE_SHADE: [u8; 3] = [16, 23, 20];
const PLATE_CAP: [u8; 3] = [137, 152, 149];
const PLATE_CAP_EDGE: [u8; 3] = [98, 110, 107];
const PLATE_CAP_FOOT: [u8; 3] = [76, 87, 84];
const PLATE_CAP_DARK: [u8; 3] = [38, 46, 43];
const POINTER_RED: [u8; 3] = [232, 61, 61];
const POINTER_WHITE: [u8; 3] = [210, 207, 202];
const ICON_GREEN: [u8; 3] = [76, 150, 56];

/// The level's pale cyan, and its glyphs' pitch; the name's pitch, in
/// whole pixels so its glyphs stand evenly.
pub const LEVEL_TINT: [u8; 3] = [168, 212, 212];
pub const LEVEL_PITCH: f64 = 19.0 / 3.0;
pub const NAME_PITCH: f64 = 7.0;

/// The hue the client's bar shows along it, `(place, degrees)`, as
/// measured on its captures.
const PLATE_HUES: [(f64, f64); 11] = [
    (0.0, 0.0),
    (0.146, 18.0),
    (0.244, 38.5),
    (0.34, 56.7),
    (0.49, 60.0),
    (0.78, 54.3),
    (0.83, 56.0),
    (0.878, 70.6),
    (0.927, 85.0),
    (0.976, 99.3),
    (1.0, 105.0),
];

/// How a plate is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateLook {
    /// The locked creature's: opaque and vivid.
    Solid,
    /// Every other creature's: blended with the scene.
    Washed,
}

/// The small pointer the client draws under some plates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatePointer {
    Absent,
    Red,
    White,
}

/// One plate to draw: the top-left of the bar's coloured part, the
/// creature's health, how it is drawn, at what scale, and how its line of
/// lettering is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateDrawing {
    pub at: (i32, i32),
    pub fill: f64,
    pub look: PlateLook,
    pub pointer: PlatePointer,
    pub geometry: PlateGeometry,
    pub line: LineAlign,
}

impl PlateDrawing {
    /// The locked creature's plate at `fill`, with its red pointer.
    pub fn locked(at: (i32, i32), fill: f64) -> Self {
        Self {
            at,
            fill,
            look: PlateLook::Solid,
            pointer: PlatePointer::Red,
            geometry: PLATE_DEFAULT,
            line: LineAlign::Left,
        }
    }

    /// Another creature's plate, at full health.
    pub fn washed(at: (i32, i32)) -> Self {
        Self {
            at,
            fill: 1.0,
            look: PlateLook::Washed,
            pointer: PlatePointer::Absent,
            geometry: PLATE_DEFAULT,
            line: LineAlign::Left,
        }
    }
}

/// The bar's colour at column `c` of `width`, on its fill row `row`: the
/// gradient's hue at full saturation, the top two rows lit and the bottom
/// two shaded, as the client draws it.
fn bar_colour(c: i32, width: i32, row: i32) -> [u8; 3] {
    let place = (c as f64 + 0.5) / width as f64;
    let mut h = PLATE_HUES[PLATE_HUES.len() - 1].1;
    for pair in PLATE_HUES.windows(2) {
        let ((p0, h0), (p1, h1)) = (pair[0], pair[1]);
        if place <= p1 {
            h = h0 + (h1 - h0) * (place - p0) / (p1 - p0);
            break;
        }
    }
    let value = if h <= 60.0 {
        250.0
    } else {
        250.0 - (h - 60.0) / 45.0 * 38.0
    };
    let (r, g) = if h <= 60.0 {
        (value, value * h.max(0.0) / 60.0)
    } else {
        (value * (120.0 - h) / 60.0, value)
    };
    let lit = [30.0, 13.0, 0.0, 0.0, 0.0, 0.0][row.clamp(0, 5) as usize];
    let shade = [1.0, 1.0, 1.0, 1.0, 0.935, 0.835][row.clamp(0, 5) as usize];
    [r, g, 0.0].map(|v: f64| ((v + lit).min(255.0) * shade).round() as u8)
}

/// Paint `rgb` at `(x, y)` as `look` draws it: as it is, or blended half
/// and half with what is there.
fn paint(image: &mut BgrImage, x: i32, y: i32, rgb: [u8; 3], look: PlateLook) {
    if x < 0 || y < 0 || x >= image.w as i32 || y >= image.h as i32 {
        return;
    }
    let shown = match look {
        PlateLook::Solid => rgb,
        PlateLook::Washed => {
            let i = (y as usize * image.w + x as usize) * 3;
            let under = [image.data[i + 2], image.data[i + 1], image.data[i]];
            [0, 1, 2].map(|k| ((rgb[k] as u16 + under[k] as u16) / 2) as u8)
        }
    };
    put(image, x, y, shown);
}

/// Draw a plate's bar the way the client does: the frame, the bevel with
/// its pale caps at the left and dark ones at the right, the gradient as
/// far as the creature's health reaches and the black empty part after
/// it, and the pointer under it. At the default scale every pixel is as
/// measured on captures; another geometry draws the same parts at its own
/// sizes. Parts off the frame are clipped. Returns the bar's coloured
/// part.
pub fn draw_plate_bar(image: &mut BgrImage, plate: &PlateDrawing) -> Rect {
    let g = plate.geometry;
    let (w, h) = g.bar;
    let (bx, by) = plate.at;
    let look = plate.look;
    let bar = Rect::new(bx, by, bx + w - 1, by + h - 1);
    let (bevel, inset) = (g.bevel, g.frame + g.bevel);
    for k in 0..g.frame {
        let edge = bar.padded(inset - k);
        for x in edge.x0..=edge.x1 {
            paint(image, x, edge.y0, PLATE_FRAME, look);
            paint(image, x, edge.y1, PLATE_FRAME, look);
        }
        for y in edge.y0 + 1..edge.y1 {
            paint(image, edge.x0, y, PLATE_FRAME, look);
            paint(image, edge.x1, y, PLATE_FRAME, look);
        }
    }
    for x in bx..bx + w {
        for y in by - bevel..by {
            paint(image, x, y, PLATE_BEVEL, look);
        }
        for y in by + h..by + h + bevel {
            paint(image, x, y, PLATE_SHADE, look);
        }
    }
    // The caps: pale at the left end, dark at the right, each turning the
    // bevel's corner so that the bevel runs exactly the bar's width.
    if g == PLATE_DEFAULT {
        let caps = [
            (bx - 2, by - 2, PLATE_CAP_EDGE),
            (bx - 1, by - 2, PLATE_BEVEL),
            (bx - 1, by - 1, PLATE_CAP_EDGE),
            (bx - 2, by + h + 1, PLATE_CAP_FOOT),
            (bx - 1, by + h, PLATE_CAP_FOOT),
            (bx - 1, by + h + 1, PLATE_SHADE),
            (bx + w, by - 2, PLATE_BEVEL),
            (bx + w, by - 1, PLATE_CAP_DARK),
            (bx + w + 1, by - 2, PLATE_CAP_DARK),
        ];
        for y in by - 1..=by + h {
            paint(image, bx - 2, y, PLATE_CAP, look);
        }
        for y in by..by + h {
            paint(image, bx - 1, y, PLATE_CAP, look);
        }
        for y in by..=by + h + 1 {
            paint(image, bx + w, y, PLATE_SHADE, look);
        }
        for y in by - 1..=by + h + 1 {
            paint(image, bx + w + 1, y, PLATE_SHADE, look);
        }
        for (x, y, rgb) in caps {
            paint(image, x, y, rgb, look);
        }
    } else {
        for k in 1..=bevel {
            for y in by - bevel..by + h + bevel {
                let left = if k == 1 && y == by - bevel {
                    PLATE_BEVEL
                } else {
                    PLATE_CAP
                };
                paint(image, bx - k, y, left, look);
                let right = if k == 1 && y == by - bevel {
                    PLATE_BEVEL
                } else {
                    PLATE_SHADE
                };
                paint(image, bx + w - 1 + k, y, right, look);
            }
        }
    }
    let reach = (plate.fill.clamp(0.0, 1.0) * w as f64).round() as i32;
    for c in 0..w {
        for row in 0..h {
            let rgb = if c < reach {
                bar_colour(c, w, row * 6 / h)
            } else {
                PLATE_EMPTY
            };
            paint(image, bx + c, by + row, rgb, look);
        }
    }
    let pointer = match plate.pointer {
        PlatePointer::Absent => None,
        PlatePointer::Red => Some(POINTER_RED),
        PlatePointer::White => Some(POINTER_WHITE),
    };
    if let Some(rgb) = pointer {
        // Under the bar's middle, below the frame: at the default scale
        // two rows four wide, then three rows two wide.
        let cx = bx + w / 2 - 2;
        let top = by + h + inset + 5;
        let rows = (5.0 * g.factor()).round() as i32;
        for dy in 0..rows {
            let (from, to) = if dy < rows * 2 / 5 { (0, 3) } else { (1, 2) };
            for dx in from..=to {
                put(image, cx + dx, top + dy, rgb);
            }
        }
    }
    bar
}

/// The status icon the client draws after some names: a green tile with
/// a white swirl.
pub fn draw_status_icon(image: &mut BgrImage, (x, y): (i32, i32)) {
    fill(image, Rect::new(x, y, x + 22, y + 14), ICON_GREEN);
    fill(
        image,
        Rect::new(x + 5, y + 4, x + 17, y + 4),
        [240, 240, 240],
    );
    fill(
        image,
        Rect::new(x + 5, y + 10, x + 17, y + 10),
        [240, 240, 240],
    );
    fill(
        image,
        Rect::new(x + 10, y + 6, x + 12, y + 8),
        [240, 240, 240],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No palette colour, drawn at any brightness the drawers use or
    /// brightened back as a reader does, reads as another palette colour,
    /// and no colour the drawers use for anything but lettering (as drawn,
    /// or brightened) reads as any palette colour.
    #[test]
    fn the_palette_cannot_be_mistaken_under_scaling() {
        let palette: Vec<[u8; 3]> = (0..144).map(palette_colour).chain(UNLIT).collect();
        let scale = |rgb: [u8; 3], k: f32| rgb.map(|c| (c as f32 * k).round().min(255.0) as u8);
        for (i, colour) in palette.iter().enumerate() {
            for peak in [40u8, 66, 120, 198, 232, 250] {
                let drawn = dimmed(*colour, peak);
                let seen = drawn.iter().copied().max().unwrap_or(1) as f32;
                for shown in [drawn, scale(drawn, 232.0 / seen)] {
                    assert!(scaled_from(shown, *colour), "{colour:?} at {peak}");
                    for (j, other) in palette.iter().enumerate() {
                        assert!(
                            i == j || !scaled_from(shown, *other),
                            "{colour:?} at {peak} reads as {other:?}"
                        );
                    }
                }
            }
        }
        for chrome in DRAWN_CHROME {
            for k in [1.0f32, 1.5, 3.5, 9.0] {
                let shown = scale(chrome, k);
                assert!(
                    palette.iter().all(|c| !scaled_from(shown, *c)),
                    "{chrome:?} scaled by {k} reads as lettering"
                );
            }
        }
    }
}
