//! Pixel geometry shared by every stage of a read.

use crate::skill_panel::BgrImage;

/// An axis-aligned pixel rectangle with inclusive corners, in the
/// coordinates of the frame it was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0 + 1
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0 + 1
    }

    /// The same rectangle moved by `(dx, dy)`.
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }

    /// The rectangle grown by `pad` on every side.
    pub fn padded(&self, pad: i32) -> Rect {
        Rect::new(self.x0 - pad, self.y0 - pad, self.x1 + pad, self.y1 + pad)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// The overlap of two rectangles, when they share at least a pixel.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect::new(
            self.x0.max(other.x0),
            self.y0.max(other.y0),
            self.x1.min(other.x1),
            self.y1.min(other.y1),
        );
        (r.x0 <= r.x1 && r.y0 <= r.y1).then_some(r)
    }
}

/// Read access to a frame in the terms the readers think in: red,
/// green and blue by position, whatever order the buffer stores.
pub trait Pixels {
    fn width(&self) -> i32;
    fn height(&self) -> i32;
    /// `[r, g, b]` at `(x, y)`; the caller keeps the point in bounds.
    fn rgb(&self, x: i32, y: i32) -> [u8; 3];

    /// Mean of the three channels, the brightness every threshold in
    /// the profile is stated in.
    fn lum(&self, x: i32, y: i32) -> u8 {
        let [r, g, b] = self.rgb(x, y);
        ((r as u16 + g as u16 + b as u16) / 3) as u8
    }

    fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.width() && y < self.height()
    }
}

impl Pixels for BgrImage {
    fn width(&self) -> i32 {
        self.w as i32
    }

    fn height(&self) -> i32 {
        self.h as i32
    }

    fn rgb(&self, x: i32, y: i32) -> [u8; 3] {
        let i = (y as usize * self.w + x as usize) * 3;
        [self.data[i + 2], self.data[i + 1], self.data[i]]
    }
}

/// The pixels of `rect`, clamped into the frame.
pub fn crop(image: &BgrImage, rect: Rect) -> BgrImage {
    image.crop(
        rect.y0 as i64,
        rect.y1 as i64 + 1,
        rect.x0 as i64,
        rect.x1 as i64 + 1,
    )
}

/// Largest minus smallest channel: zero for a grey, large for a
/// saturated colour.
pub fn chroma(rgb: [u8; 3]) -> u8 {
    let hi = rgb[0].max(rgb[1]).max(rgb[2]);
    let lo = rgb[0].min(rgb[1]).min(rgb[2]);
    hi - lo
}

/// Whether every channel of `rgb` is within `tolerance` of `target`.
pub fn near(rgb: [u8; 3], target: [u8; 3], tolerance: u8) -> bool {
    rgb.iter()
        .zip(target.iter())
        .all(|(a, b)| a.abs_diff(*b) <= tolerance)
}
