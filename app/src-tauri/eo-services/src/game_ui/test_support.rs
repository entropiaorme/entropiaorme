//! Shared helpers for the reading pipeline's tests: the bundled
//! profile, frames drawn the way the client draws them, and access to
//! the private capture corpus where one is present.

use std::path::{Path, PathBuf};

use crate::ocr_engine::load_bgr_png;
use crate::skill_panel::BgrImage;

use super::geometry::Rect;
use super::profile::UiProfile;
use super::validate::{expected_width, NumeralMetrics};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The profile the application ships.
pub(crate) fn bundled_profile() -> UiProfile {
    UiProfile::load(&repo_root().join("app/src-tauri/entropia-orme/resources/ui_profile.json"))
        .expect("the bundled interface profile loads")
}

/// The private corpus of real captures, when this machine has one.
/// Real gameplay screens stay out of the public tree, so tests over
/// them run only where `EO_GAME_UI_CORPUS_DIR` names the corpus and
/// say so when they skip.
pub(crate) fn corpus_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("EO_GAME_UI_CORPUS_DIR")?);
    dir.is_dir().then_some(dir)
}

/// One captured frame from the private corpus.
pub(crate) fn corpus_frame(dir: &Path, frame: &str) -> BgrImage {
    let path = dir.join("frames").join(format!("{frame}.png"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let (data, h, w) = load_bgr_png(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    BgrImage { data, h, w }
}

/// A frame filled with one colour.
pub(crate) fn blank(w: usize, h: usize, rgb: [u8; 3]) -> BgrImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for _ in 0..w * h {
        data.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
    }
    BgrImage { data, h, w }
}

pub(crate) fn put(image: &mut BgrImage, x: i32, y: i32, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= image.w as i32 || y >= image.h as i32 {
        return;
    }
    let i = (y as usize * image.w + x as usize) * 3;
    image.data[i] = rgb[2];
    image.data[i + 1] = rgb[1];
    image.data[i + 2] = rgb[0];
}

pub(crate) fn fill(image: &mut BgrImage, rect: Rect, rgb: [u8; 3]) {
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
pub(crate) fn draw_window(image: &mut BgrImage, rect: Rect) {
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
pub(crate) struct Lettering {
    pub(crate) image: BgrImage,
    palette: Vec<([u8; 3], String)>,
}

impl Lettering {
    pub(crate) fn new(image: BgrImage) -> Self {
        Self {
            image,
            palette: Vec::new(),
        }
    }

    /// Draw a block of ink filling `rect` that reads as `text`.
    pub(crate) fn text(&mut self, rect: Rect, text: &str) {
        let i = self.palette.len() as u8;
        let rgb = [250 - i * 3, 120 + (i % 7) * 15, 130 + (i % 5) * 20];
        fill(&mut self.image, rect, rgb);
        self.palette.push((rgb, text.to_string()));
    }

    /// Draw a figure `h` tall with its top-left at `at`, its ink as wide
    /// as the client's numerals make `drawn`, that reads as `reads`.
    /// Returns where its ink is.
    pub(crate) fn figure(
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

    pub(crate) fn recogniser(&self) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
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

/// A small deterministic generator, so randomised layouts replay.
pub(crate) struct Lcg(pub u64);

impl Lcg {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    /// A value in `[lo, hi]`.
    pub(crate) fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}
