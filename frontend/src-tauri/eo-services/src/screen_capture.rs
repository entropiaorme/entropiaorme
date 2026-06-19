//! Screen-region capture for OCR: the mss-based `backend/ocr/capturer.py`
//! equivalent. A region (`x`, `y`, `w`, `h`) goes in; PNG bytes (the
//! skill-scan path, RGB-encoded to match `mss.tools.to_png(shot.rgb)`) or
//! a [`BgrImage`] (the repair-OCR path) comes out, via a GDI `BitBlt` of
//! the screen device context.
//!
//! Windows-only: off Windows the captures return `None`, exactly as the
//! OCR engine and the keystroke hook stand down, so the scan/repair
//! routes report "engine unavailable" rather than serving an empty capture.
//! Capture is on-demand with no persistent handle (mirroring the Python
//! capturer's per-call grab), so there is nothing to leak between scans.

use image::ImageEncoder;

use crate::skill_panel::BgrImage;

/// Capture a screen rectangle as PNG bytes (RGB-encoded). `None` on a
/// non-positive region, a capture failure, or a non-Windows host.
pub fn capture_region_png(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
    let bgra = platform::capture_bgra(x, y, w, h)?;
    let (pw, ph) = (w as u32, h as u32);
    // BGRA (top-down) -> RGB, the order `mss.tools.to_png(shot.rgb)` encodes.
    let mut rgb = Vec::with_capacity((pw as usize) * (ph as usize) * 3);
    for px in bgra.chunks_exact(4) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgb, pw, ph, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

/// Capture a screen rectangle as a BGR image (the repair-OCR path). `None`
/// on a non-positive region, a capture failure, or a non-Windows host.
pub fn capture_region_bgr(x: i64, y: i64, w: i64, h: i64) -> Option<BgrImage> {
    let bgra = platform::capture_bgra(x, y, w, h)?;
    // BGRA -> BGR (drop the alpha), matching the `[:, :, :3]` slice of the
    // Python capturer's BGR ndarray.
    let mut data = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in bgra.chunks_exact(4) {
        data.push(px[0]);
        data.push(px[1]);
        data.push(px[2]);
    }
    Some(BgrImage {
        data,
        h: h as usize,
        w: w as usize,
    })
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HGDIOBJ, SRCCOPY,
    };

    /// Capture the rectangle as top-down 32-bit BGRA bytes via GDI. Every
    /// handle acquired is released on every path; the screen DC is released
    /// last. `None` on any GDI failure or a non-positive region.
    pub fn capture_bgra(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let (x, y, w, h) = (x as i32, y as i32, w as i32, h as i32);
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return None;
            }
            let result = capture_into(screen, x, y, w, h);
            ReleaseDC(None, screen);
            result
        }
    }

    /// The inner GDI dance, factored so the screen DC release in the caller
    /// runs on every exit. SAFETY: called only with a valid screen DC; each
    /// created object is selected out and deleted before return.
    unsafe fn capture_into(
        screen: windows::Win32::Graphics::Gdi::HDC,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Option<Vec<u8>> {
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            return None;
        }
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem);
            return None;
        }
        let previous = SelectObject(mem, HGDIOBJ::from(bitmap));
        let blitted = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY).is_ok();

        // A negative height requests a top-down DIB (row 0 is the top), so
        // the byte order matches the Python grab without a vertical flip.
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buffer = vec![0u8; (w as usize) * (h as usize) * 4];
        let rows = if blitted {
            GetDIBits(
                mem,
                bitmap,
                0,
                h as u32,
                Some(buffer.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        SelectObject(mem, previous);
        let _ = DeleteObject(HGDIOBJ::from(bitmap));
        let _ = DeleteDC(mem);

        if rows == h {
            Some(buffer)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn capture_bgra(_x: i64, _y: i64, _w: i64, _h: i64) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_positive_region_never_captures() {
        // The dimension guard short-circuits before any GDI call (and the
        // off-Windows stub returns None regardless), so both capture forms
        // refuse a zero or negative region on every host.
        assert!(capture_region_png(10, 10, 0, 5).is_none());
        assert!(capture_region_png(10, 10, 5, 0).is_none());
        assert!(capture_region_png(10, 10, -1, 5).is_none());
        assert!(capture_region_bgr(10, 10, 0, 5).is_none());
        assert!(capture_region_bgr(10, 10, 5, -3).is_none());
    }
}
