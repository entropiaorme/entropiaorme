//! Shared helpers for the reading pipeline's tests: the bundled
//! profile, frames drawn the way the client draws them, and access to
//! the private capture corpus where one is present. Compiled for this
//! crate's tests, and for the facade's through the `test-support`
//! feature.

use std::path::{Path, PathBuf};

use crate::game_ui::geometry::BgrImage;
use crate::ocr_engine::load_bgr_png;

use super::geometry::Rect;
use super::profile::UiProfile;
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
