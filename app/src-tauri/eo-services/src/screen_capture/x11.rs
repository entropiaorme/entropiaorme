//! The Xorg capturer: on an X11 session the X server composes the whole
//! screen into the root window, so any client may read a rectangle of it
//! with `GetImage`. No portal, no consent prompt, no stream to hold open:
//! each call reads exactly the requested pixels, like the Windows GDI path.
//!
//! Only an Xorg session takes this path. Under Wayland the root window of
//! XWayland holds X clients' pixels alone (every native window reads as
//! black), so the portal stays the capturer there.
//!
//! One connection is kept for the process and dropped on any error, so a
//! restarted X server costs a single failed capture before the next call
//! reconnects.

use std::sync::{Mutex, OnceLock};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

/// Whether this process runs inside an Xorg session, read once: the session
/// type cannot change under a running process.
pub fn session_is_x11() -> bool {
    static X11: OnceLock<bool> = OnceLock::new();
    *X11.get_or_init(|| is_x11_session(std::env::var("XDG_SESSION_TYPE").ok().as_deref()))
}

fn is_x11_session(session_type: Option<&str>) -> bool {
    session_type.is_some_and(|value| value.eq_ignore_ascii_case("x11"))
}

/// How the server lays out one pixel of a `ZPixmap` image of the root
/// window: the depth's pixmap format plus the root visual's channel masks.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PixelLayout {
    bits_per_pixel: u8,
    scanline_pad: u8,
    lsb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
}

struct Display {
    conn: RustConnection,
    root: Window,
    width: i64,
    height: i64,
    layout: PixelLayout,
}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);

fn connect() -> Option<Display> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let setup = conn.setup();
    let screen = setup.roots.get(screen_num)?;
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == screen.root_depth)?;
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == screen.root_visual)?;
    let layout = PixelLayout {
        bits_per_pixel: format.bits_per_pixel,
        scanline_pad: format.scanline_pad,
        lsb_first: setup.image_byte_order == ImageOrder::LSB_FIRST,
        red_mask: visual.red_mask,
        green_mask: visual.green_mask,
        blue_mask: visual.blue_mask,
    };
    let (root, width, height) = (
        screen.root,
        i64::from(screen.width_in_pixels),
        i64::from(screen.height_in_pixels),
    );
    Some(Display {
        conn,
        root,
        width,
        height,
        layout,
    })
}

/// Capture the rectangle (global screen coordinates, which on Xorg are the
/// root window's) as top-down 32-bit BGRA. `None` on a non-positive or
/// off-screen rectangle, an unsupported pixel format, or any X error.
pub fn capture_bgra(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut slot = DISPLAY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.is_none() {
        *slot = connect();
        if slot.is_none() {
            tracing::warn!(target: "eo::capture", "no X display reachable for capture");
            return None;
        }
    }
    let server = slot.as_ref()?;
    if x < 0 || y < 0 || x + w > server.width || y + h > server.height {
        tracing::warn!(
            target: "eo::capture",
            rect = ?(x, y, w, h),
            screen = ?(server.width, server.height),
            "capture rectangle falls outside the X screen"
        );
        return None;
    }
    let reply = server
        .conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            server.root,
            i16::try_from(x).ok()?,
            i16::try_from(y).ok()?,
            u16::try_from(w).ok()?,
            u16::try_from(h).ok()?,
            u32::MAX,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let Some(reply) = reply else {
        tracing::warn!(target: "eo::capture", "X GetImage failed; reconnecting next capture");
        *slot = None;
        return None;
    };
    let bgra = to_bgra(&reply.data, w as usize, h as usize, server.layout);
    if bgra.is_none() {
        tracing::warn!(
            target: "eo::capture",
            layout = ?server.layout,
            "unsupported X pixel format for capture"
        );
    }
    bgra
}

/// Decode a `ZPixmap` image into BGRA (alpha opaque). Only 32 bits per
/// pixel is supported, which covers every true-colour depth a desktop runs
/// (24 and 30); each channel is read through its mask and scaled to 8 bits.
fn to_bgra(data: &[u8], w: usize, h: usize, layout: PixelLayout) -> Option<Vec<u8>> {
    if layout.bits_per_pixel != 32 || layout.scanline_pad == 0 {
        return None;
    }
    let pad_bits = usize::from(layout.scanline_pad);
    let stride = (w * 32).div_ceil(pad_bits) * pad_bits / 8;
    if data.len() < stride * h {
        return None;
    }
    let channels = [layout.blue_mask, layout.green_mask, layout.red_mask];
    if channels.contains(&0) {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for col in 0..w {
            let at = row * stride + col * 4;
            let bytes = [data[at], data[at + 1], data[at + 2], data[at + 3]];
            let pixel = if layout.lsb_first {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            };
            for mask in channels {
                out.push(channel(pixel, mask));
            }
            out.push(0xff);
        }
    }
    Some(out)
}

/// One channel of `pixel` under `mask`, scaled to 8 bits.
fn channel(pixel: u32, mask: u32) -> u8 {
    let shift = mask.trailing_zeros();
    let width = (mask >> shift).count_ones();
    let value = (pixel & mask) >> shift;
    if width >= 8 {
        (value >> (width - 8)) as u8
    } else {
        // Replicate the high bits so a full channel maps to 0xff.
        let max = (1u32 << width) - 1;
        (value * 255 / max) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPTH_24: PixelLayout = PixelLayout {
        bits_per_pixel: 32,
        scanline_pad: 32,
        lsb_first: true,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
    };

    #[test]
    fn only_an_x11_session_type_selects_the_x_capturer() {
        assert!(is_x11_session(Some("x11")));
        assert!(is_x11_session(Some("X11")));
        assert!(!is_x11_session(Some("wayland")));
        assert!(!is_x11_session(Some("tty")));
        assert!(!is_x11_session(None));
    }

    #[test]
    fn depth_24_little_endian_decodes_to_bgra() {
        // Two pixels: pure red, then (b=1, g=2, r=3) with a junk pad byte.
        let data = [0x00, 0x00, 0xff, 0x7f, 0x01, 0x02, 0x03, 0x99];
        let out = to_bgra(&data, 2, 1, DEPTH_24).expect("decodes");
        assert_eq!(out, vec![0, 0, 0xff, 0xff, 1, 2, 3, 0xff]);
    }

    #[test]
    fn a_big_endian_server_reads_the_word_the_other_way() {
        let layout = PixelLayout {
            lsb_first: false,
            ..DEPTH_24
        };
        let data = [0x00, 0x03, 0x02, 0x01];
        assert_eq!(to_bgra(&data, 1, 1, layout), Some(vec![1, 2, 3, 0xff]));
    }

    #[test]
    fn depth_30_channels_scale_down_to_eight_bits() {
        let layout = PixelLayout {
            red_mask: 0x3ff0_0000,
            green_mask: 0x000f_fc00,
            blue_mask: 0x0000_03ff,
            ..DEPTH_24
        };
        // Full red, half green (0x200 of 0x3ff), zero blue.
        let pixel: u32 = (0x3ff << 20) | (0x200 << 10);
        let out = to_bgra(&pixel.to_le_bytes(), 1, 1, layout).expect("decodes");
        assert_eq!(out, vec![0, 0x80, 0xff, 0xff]);
    }

    #[test]
    fn rows_honour_the_scanline_pad() {
        // A 64-bit pad leaves one unused 4-byte word after a 1-pixel row.
        let layout = PixelLayout {
            scanline_pad: 64,
            ..DEPTH_24
        };
        let data = [
            1, 2, 3, 0, 0xee, 0xee, 0xee, 0xee, 4, 5, 6, 0, 0xee, 0xee, 0xee, 0xee,
        ];
        let out = to_bgra(&data, 1, 2, layout).expect("decodes");
        assert_eq!(out, vec![1, 2, 3, 0xff, 4, 5, 6, 0xff]);
    }

    #[test]
    fn unsupported_layouts_and_short_buffers_refuse() {
        let packed24 = PixelLayout {
            bits_per_pixel: 24,
            ..DEPTH_24
        };
        assert_eq!(to_bgra(&[0; 3], 1, 1, packed24), None);
        assert_eq!(to_bgra(&[0; 4], 2, 1, DEPTH_24), None);
        let maskless = PixelLayout {
            red_mask: 0,
            ..DEPTH_24
        };
        assert_eq!(to_bgra(&[0; 4], 1, 1, maskless), None);
    }

    #[test]
    fn non_positive_rectangles_never_reach_the_server() {
        assert_eq!(capture_bgra(0, 0, 0, 4), None);
        assert_eq!(capture_bgra(0, 0, 4, -1), None);
    }

    /// Reads a solid rectangle painted by a window of its own back off a
    /// live X server. Needs a display with no compositor effects on the
    /// test window, which a bare virtual server gives:
    /// `xvfb-run -a cargo test -p eo-services screen_capture::x11 -- --ignored`.
    #[test]
    #[ignore = "needs an X display (run under xvfb-run)"]
    fn reads_a_painted_rectangle_on_a_live_x_server() {
        use x11rb::protocol::xproto::{
            ChangeGCAux, CreateGCAux, CreateWindowAux, EventMask, Rectangle, WindowClass,
        };
        use x11rb::wrapper::ConnectionExt as _;

        let (conn, screen_num) = x11rb::connect(None).expect("X display");
        let screen = conn.setup().roots[screen_num].clone();
        let window = conn.generate_id().unwrap();
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            40,
            30,
            64,
            48,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::EXPOSURE),
        )
        .unwrap();
        conn.map_window(window).unwrap();
        let gc = conn.generate_id().unwrap();
        conn.create_gc(gc, window, &CreateGCAux::new()).unwrap();
        conn.sync().unwrap();
        // Paint after the map has landed: r=0xc0, g=0x40, b=0x20 on a
        // depth-24 visual.
        conn.change_gc(gc, &ChangeGCAux::new().foreground(0x00c0_4020))
            .unwrap();
        conn.poly_fill_rectangle(
            window,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 64,
                height: 48,
            }],
        )
        .unwrap();
        conn.sync().unwrap();

        let bgra = capture_bgra(50, 40, 4, 2).expect("captured");
        assert_eq!(bgra.len(), 4 * 2 * 4);
        for pixel in bgra.chunks_exact(4) {
            assert_eq!(pixel, [0x20, 0x40, 0xc0, 0xff]);
        }
        // Outside the screen is a refusal, never a partial frame.
        assert_eq!(
            capture_bgra(i64::from(screen.width_in_pixels) - 1, 0, 4, 4),
            None
        );
    }
}
