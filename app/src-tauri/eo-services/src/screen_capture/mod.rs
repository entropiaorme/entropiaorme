//! Screen-region capture for OCR. A region (`x`, `y`, `w`, `h`, global
//! screen coordinates) goes in; PNG bytes (the skill-scan path, RGB-encoded)
//! or a [`BgrImage`] (the repair-OCR path) comes out.
//!
//! The backend is per platform: a GDI `BitBlt` of the screen device context
//! on Windows; on Linux the X server's root window on an Xorg session
//! (`x11`) and the ScreenCast portal elsewhere (`portal`, behind the
//! `linux-capture` feature). Every other host returns `None`, so the
//! scan/repair routes report "engine unavailable" rather than serving an
//! empty capture.

use image::ImageEncoder;

use crate::skill_panel::BgrImage;

/// Capture a screen rectangle as PNG bytes (RGB-encoded). `None` on a
/// non-positive region, a capture failure, or a host with no capturer.
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
/// on a non-positive region, a capture failure, or a host with no capturer.
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

/// Encode a captured BGR frame as PNG bytes.
pub fn frame_to_png(frame: &BgrImage) -> Option<Vec<u8>> {
    use image::ImageEncoder as _;
    let mut rgb = Vec::with_capacity(frame.w * frame.h * 3);
    for px in frame.data.chunks_exact(3) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &rgb,
            frame.w as u32,
            frame.h as u32,
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(png)
}

/// Drop one captured frame under `dir` as `name`, best effort.
///
/// The standing instrument for a read that will not read: the pixels the
/// recogniser actually saw, kept where a later look can find them. One
/// small overwritten file per scan kind, local only.
pub fn write_debug_frame(dir: &std::path::Path, name: &str, frame: &BgrImage) {
    if let Err(error) = std::fs::create_dir_all(dir) {
        tracing::warn!(target: "eo::capture", %error, "debug dir not creatable");
        return;
    }
    let Some(png) = frame_to_png(frame) else {
        tracing::warn!(target: "eo::capture", "debug frame not encodable");
        return;
    };
    if let Err(error) = std::fs::write(dir.join(name), &png) {
        tracing::warn!(target: "eo::capture", %error, "debug frame not writable");
    }
}

/// Return the generation of the newest Linux portal frame.
///
/// This is intentionally hidden from the generated API documentation: it is
/// an assembly diagnostic used by the stand-alone capture probe to prove that
/// the stream is still advancing rather than repeatedly cropping a cached
/// first frame.
#[cfg(all(target_os = "linux", feature = "linux-capture"))]
#[doc(hidden)]
pub fn capture_frame_generation() -> Option<u64> {
    portal::frame_generation()
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

/// The Linux capturer: the X server's own screen on an Xorg session, the
/// ScreenCast portal everywhere else (Wayland cannot hand a client the
/// screen without consent). Both expose the same per-call BGRA crop.
#[cfg(target_os = "linux")]
mod platform {
    pub fn capture_bgra(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
        if super::x11::session_is_x11() {
            return super::x11::capture_bgra(x, y, w, h);
        }
        portal_capture(x, y, w, h)
    }

    #[cfg(feature = "linux-capture")]
    fn portal_capture(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
        super::portal::capture_bgra(x, y, w, h)
    }

    /// Without the portal backend compiled in, a Wayland session has no
    /// capturer and stands down.
    #[cfg(not(feature = "linux-capture"))]
    fn portal_capture(_x: i64, _y: i64, _w: i64, _h: i64) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(all(target_os = "linux", feature = "linux-capture"))]
mod portal;
#[cfg(target_os = "linux")]
mod x11;

/// The stand-down capturer: every host without a capture backend (neither
/// Windows nor Linux). Returns `None` so the scan/repair routes report
/// "engine unavailable" rather than serving an empty capture.
#[cfg(not(any(windows, target_os = "linux")))]
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
