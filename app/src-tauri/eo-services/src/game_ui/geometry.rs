//! Pixel geometry shared by every stage of a read, and the image buffer
//! every reader of the game's screen works on.

/// One BGR HWC image buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct BgrImage {
    pub data: Vec<u8>,
    pub h: usize,
    pub w: usize,
}

impl BgrImage {
    /// The rectangle `[y0, y1) x [x0, x1)`, clamped into bounds: a
    /// rectangle reaching past an edge is cut at it, and a negative
    /// coordinate clamps to zero.
    pub fn crop(&self, y0: i64, y1: i64, x0: i64, x1: i64) -> BgrImage {
        let y0 = y0.clamp(0, self.h as i64) as usize;
        let y1 = y1.clamp(y0 as i64, self.h as i64) as usize;
        let x0 = x0.clamp(0, self.w as i64) as usize;
        let x1 = x1.clamp(x0 as i64, self.w as i64) as usize;
        let (ch, cw) = (y1 - y0, x1 - x0);
        let mut data = Vec::with_capacity(ch * cw * 3);
        for y in y0..y1 {
            let start = (y * self.w + x0) * 3;
            data.extend_from_slice(&self.data[start..start + cw * 3]);
        }
        BgrImage { data, h: ch, w: cw }
    }
}

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

/// The pixels of `rect`, clamped into the frame, with every channel
/// scaled by `to / from` (saturating). Lettering the client draws dim,
/// such as a locked control's, is brought up to the brightness that the
/// thresholds for its bright lettering are stated in, so one set of
/// thresholds finds and measures both.
pub fn brightened(image: &BgrImage, rect: Rect, from: u8, to: u8) -> BgrImage {
    let mut out = crop(image, rect);
    let scale = to as f32 / from.max(1) as f32;
    for channel in out.data.iter_mut() {
        *channel = (*channel as f32 * scale).round().min(255.0) as u8;
    }
    out
}

/// Largest minus smallest channel: zero for a grey, large for a
/// saturated colour.
pub fn chroma(rgb: [u8; 3]) -> u8 {
    let hi = rgb[0].max(rgb[1]).max(rgb[2]);
    let lo = rgb[0].min(rgb[1]).min(rgb[2]);
    hi - lo
}

/// The hue of `rgb` in degrees, red at zero, yellow at 60 and green at
/// 120, with the reds just short of a full turn given as small negative
/// angles (`-60` to `300`), so a colour run through red stays continuous.
/// `None` for a grey, which has no hue.
pub fn hue(rgb: [u8; 3]) -> Option<f32> {
    let [r, g, b] = rgb.map(f32::from);
    let hi = r.max(g).max(b);
    let c = hi - r.min(g).min(b);
    if c <= 0.0 {
        return None;
    }
    let h = if hi == r {
        60.0 * (g - b) / c
    } else if hi == g {
        60.0 * (2.0 + (b - r) / c)
    } else {
        60.0 * (4.0 + (r - g) / c)
    };
    Some(if h >= 300.0 { h - 360.0 } else { h })
}

/// Whether every channel of `rgb` is within `tolerance` of `target`.
pub fn near(rgb: [u8; 3], target: [u8; 3], tolerance: u8) -> bool {
    rgb.iter()
        .zip(target.iter())
        .all(|(a, b)| a.abs_diff(*b) <= tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_is_cut_at_the_image_edges() {
        // A 3x2 image whose pixels carry their own coordinates.
        let mut data = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                data.extend_from_slice(&[x, y, 0]);
            }
        }
        let image = BgrImage { data, h: 2, w: 3 };
        let inner = image.crop(1, 2, 1, 3);
        assert_eq!((inner.w, inner.h), (2, 1));
        assert_eq!(inner.data, vec![1, 1, 0, 2, 1, 0]);
        let overrun = image.crop(-5, 9, 2, 9);
        assert_eq!((overrun.w, overrun.h), (1, 2));
        assert_eq!(overrun.data, vec![2, 0, 0, 2, 1, 0]);
        let empty = image.crop(1, 0, 0, 3);
        assert_eq!((empty.w, empty.h, empty.data.len()), (3, 0, 0));
    }

    #[test]
    fn hues_run_from_red_through_yellow_to_green_and_a_grey_has_none() {
        assert_eq!(hue([248, 1, 0]).map(f32::round), Some(0.0));
        assert_eq!(hue([255, 255, 0]), Some(60.0));
        assert_eq!(hue([0, 255, 0]), Some(120.0));
        assert_eq!(hue([0, 0, 255]), Some(240.0));
        // A red leaning to blue sits just below zero, not near 360.
        assert_eq!(hue([255, 0, 51]).map(f32::round), Some(-12.0));
        assert_eq!(hue([90, 90, 90]), None);
    }

    #[test]
    fn brightening_scales_every_channel_and_saturates() {
        let image = BgrImage {
            data: vec![10, 20, 66, 100, 0, 255],
            h: 1,
            w: 2,
        };
        let out = brightened(&image, Rect::new(1, 0, 1, 0), 66, 232);
        assert_eq!((out.w, out.h), (1, 1));
        assert_eq!(out.data, vec![255, 0, 255]);
        let out = brightened(&image, Rect::new(0, 0, 0, 0), 66, 232);
        assert_eq!(out.data, vec![35, 70, 232]);
    }
}
