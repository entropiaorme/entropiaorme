//! Shared helpers for the reading pipeline's tests: the bundled
//! profile, frames drawn the way the client draws them, and access to
//! the private capture corpus where one is present. Compiled for this
//! crate's tests, and for the facade's through the `test-support`
//! feature.

use std::path::{Path, PathBuf};

use crate::game_ui::geometry::BgrImage;
use crate::game_ui::seams::{FrameArea, FrameUnavailable, GameFrame};
use crate::ocr_engine::load_bgr_png;

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

/// The private corpus of real captures, when this machine has one.
/// Real gameplay screens stay out of the public tree, so tests over
/// them run only where `EO_GAME_UI_CORPUS_DIR` names the corpus and
/// say so when they skip.
pub fn corpus_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("EO_GAME_UI_CORPUS_DIR")?);
    dir.is_dir().then_some(dir)
}

/// One captured frame from the private corpus.
pub fn corpus_frame(dir: &Path, frame: &str) -> BgrImage {
    let path = dir.join("frames").join(format!("{frame}.png"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let (data, h, w) = load_bgr_png(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    BgrImage { data, h, w }
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
/// each piece of ink reads as.
pub struct Lettering {
    pub image: BgrImage,
    palette: Vec<([u8; 3], String)>,
}

impl Lettering {
    pub fn new(image: BgrImage) -> Self {
        Self {
            image,
            palette: Vec::new(),
        }
    }

    /// Draw a block of ink filling `rect` that reads as `text`.
    pub fn text(&mut self, rect: Rect, text: &str) {
        let i = self.palette.len() as u8;
        let rgb = [250 - i * 3, 120 + (i % 7) * 15, 130 + (i % 5) * 20];
        fill(&mut self.image, rect, rgb);
        self.palette.push((rgb, text.to_string()));
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

    pub fn recogniser(&self) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
        let palette = self.palette.clone();
        move |crop: &BgrImage| {
            let mut counts: Vec<usize> = vec![0; palette.len()];
            for px in crop.data.chunks(3) {
                let rgb = [px[2], px[1], px[0]];
                if let Some(i) = palette.iter().position(|(c, _)| *c == rgb) {
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

/// A scene with a radar drawn on it, and the stand-in recogniser for the
/// lettering drawn there.
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
