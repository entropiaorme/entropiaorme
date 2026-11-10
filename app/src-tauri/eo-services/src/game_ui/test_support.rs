//! Shared helpers for the reading pipeline's tests: the bundled
//! profile, frames drawn the way the client draws them, and access to
//! the private capture corpus where one is present.

use std::path::{Path, PathBuf};

use crate::ocr_engine::load_bgr_png;
use crate::skill_panel::BgrImage;

use super::geometry::Rect;
use super::profile::UiProfile;

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
