//! Cut an overlay window's pointer input down to the surfaces it draws.
//!
//! The overlay windows are transparent, chromeless rectangles floating over
//! the game, but what they render is rarely the whole rectangle: a strip with
//! narrower notices under it, panels with rounded corners. Neither X11 nor
//! Win32 passes a click through a transparent pixel, so every pixel the
//! window covers and does not draw would otherwise swallow the clicks meant
//! for whatever sits beneath it. Each overlay reports the surfaces it draws
//! (logical rects with a corner radius) and this module cuts the window's
//! input region to exactly them, rounded corners included.
//!
//! The command only ever shapes the window that invokes it, so a webview can
//! never reshape another window. An empty shape list restores the default
//! (the whole window takes input).

/// One drawn surface, in the window's logical (CSS) pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
pub struct HitShape {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The surface's corner radius; 0 for a square-cornered surface.
    pub radius: f64,
}

/// One integer rectangle of the input region, in device units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// No overlay draws anywhere near this many separate surfaces; the bound
/// keeps a misbehaving webview from handing the shell an unbounded region.
const MAX_SHAPES: usize = 64;
/// A coordinate or extent bound well past any real display.
const MAX_EXTENT: f64 = 32_768.0;

/// Reject a shape list no real overlay could produce.
pub fn validate(shapes: &[HitShape]) -> Result<(), String> {
    if shapes.len() > MAX_SHAPES {
        return Err(format!(
            "hit region has {} shapes; at most {MAX_SHAPES}",
            shapes.len()
        ));
    }
    for shape in shapes {
        let fields = [shape.x, shape.y, shape.width, shape.height, shape.radius];
        if fields.iter().any(|value| !value.is_finite()) {
            return Err("hit region shapes must be finite".into());
        }
        if shape.width < 0.0 || shape.height < 0.0 || shape.radius < 0.0 {
            return Err("hit region extents must not be negative".into());
        }
        if fields.iter().any(|value| value.abs() > MAX_EXTENT) {
            return Err("hit region shape lies outside any display".into());
        }
    }
    Ok(())
}

/// The input region for `shapes` at `scale` device units per logical pixel,
/// as integer spans. Each surface rounds OUTWARD to whole device pixels, and
/// a rounded corner is cut row by row at the circle's widest point within
/// each row, so a pixel the corner's anti-aliased edge partly covers stays
/// clickable and nothing the surface draws is ever cut away.
pub fn region_spans(shapes: &[HitShape], scale: f64) -> Vec<Span> {
    let mut spans = Vec::new();
    for shape in shapes {
        let left = (shape.x * scale).floor() as i32;
        let top = (shape.y * scale).floor() as i32;
        let right = ((shape.x + shape.width) * scale).ceil() as i32;
        let bottom = ((shape.y + shape.height) * scale).ceil() as i32;
        let width = right - left;
        let height = bottom - top;
        if width <= 0 || height <= 0 {
            continue;
        }
        let radius = ((shape.radius * scale).round() as i32)
            .min(width / 2)
            .min(height / 2)
            .max(0);
        if radius == 0 {
            spans.push(Span {
                x: left,
                y: top,
                width,
                height,
            });
            continue;
        }
        let r = f64::from(radius);
        for row in 0..radius {
            // The corner circle is widest at the row's edge nearer the
            // straight side, `row + 1` pixels in from the curved end.
            let dy = r - f64::from(row + 1);
            let inset = (r - (r * r - dy * dy).sqrt()).floor() as i32;
            let row_width = width - 2 * inset;
            spans.push(Span {
                x: left + inset,
                y: top + row,
                width: row_width,
                height: 1,
            });
            spans.push(Span {
                x: left + inset,
                y: bottom - 1 - row,
                width: row_width,
                height: 1,
            });
        }
        if height > 2 * radius {
            spans.push(Span {
                x: left,
                y: top + radius,
                width,
                height: height - 2 * radius,
            });
        }
    }
    spans
}

/// Cut the invoking window's input region to `shapes` (empty restores the
/// whole window).
#[tauri::command]
pub fn set_window_hit_region(
    window: tauri::WebviewWindow,
    shapes: Vec<HitShape>,
) -> Result<(), String> {
    validate(&shapes)?;
    platform::apply(window, shapes)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{region_spans, HitShape};
    use gtk::cairo::{RectangleInt, Region};
    use gtk::prelude::WidgetExt;

    /// GTK keeps a widget's input shape and applies it whenever the widget
    /// is realized, so this holds for a pre-spawned window that has never
    /// been shown (its native surface does not exist yet) and survives
    /// hide and show. GDK coordinates are logical, so no scaling applies.
    pub fn apply(window: tauri::WebviewWindow, shapes: Vec<HitShape>) -> Result<(), String> {
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                let Ok(gtk_window) = target.gtk_window() else {
                    return;
                };
                if shapes.is_empty() {
                    gtk_window.input_shape_combine_region(None);
                    return;
                }
                let rects: Vec<RectangleInt> = region_spans(&shapes, 1.0)
                    .into_iter()
                    .map(|span| RectangleInt::new(span.x, span.y, span.width, span.height))
                    .collect();
                gtk_window.input_shape_combine_region(Some(&Region::create_rectangles(&rects)));
            })
            .map_err(|error| error.to_string())
    }
}

#[cfg(windows)]
mod platform {
    use super::{region_spans, HitShape};
    use windows::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ, RGN_OR,
    };

    /// A window region clips both input and painting to the region, in
    /// physical pixels relative to the window's top-left. The system owns
    /// the region once `SetWindowRgn` accepts it.
    pub fn apply(window: tauri::WebviewWindow, shapes: Vec<HitShape>) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        if shapes.is_empty() {
            unsafe { SetWindowRgn(hwnd, None, true) };
            return Ok(());
        }
        let scale = window.scale_factor().map_err(|error| error.to_string())?;
        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            if region.is_invalid() {
                return Err("could not allocate the window region".into());
            }
            for span in region_spans(&shapes, scale) {
                let part = CreateRectRgn(span.x, span.y, span.x + span.width, span.y + span.height);
                if part.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ::from(region));
                    return Err("could not allocate the window region".into());
                }
                CombineRgn(Some(region), Some(region), Some(part), RGN_OR);
                let _ = DeleteObject(HGDIOBJ::from(part));
            }
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                let _ = DeleteObject(HGDIOBJ::from(region));
                return Err("the window refused its region".into());
            }
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::HitShape;

    /// No input-region support elsewhere: the window keeps taking input
    /// across its whole (already content-hugging) rectangle.
    pub fn apply(_window: tauri::WebviewWindow, _shapes: Vec<HitShape>) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(x: f64, y: f64, width: f64, height: f64, radius: f64) -> HitShape {
        HitShape {
            x,
            y,
            width,
            height,
            radius,
        }
    }

    /// Every device pixel the spans cover, for coverage assertions.
    fn covered(spans: &[Span]) -> std::collections::BTreeSet<(i32, i32)> {
        spans
            .iter()
            .flat_map(|span| {
                (span.y..span.y + span.height)
                    .flat_map(move |y| (span.x..span.x + span.width).map(move |x| (x, y)))
            })
            .collect()
    }

    #[test]
    fn a_square_surface_is_one_span() {
        assert_eq!(
            region_spans(&[shape(4.0, 6.0, 100.0, 40.0, 0.0)], 1.0),
            [Span {
                x: 4,
                y: 6,
                width: 100,
                height: 40
            }]
        );
    }

    #[test]
    fn fractional_edges_round_outward_so_nothing_drawn_is_cut() {
        assert_eq!(
            region_spans(&[shape(0.4, 0.6, 99.2, 39.1, 0.0)], 1.0),
            [Span {
                x: 0,
                y: 0,
                width: 100,
                height: 40
            }]
        );
    }

    #[test]
    fn the_scale_converts_logical_pixels_to_device_pixels() {
        // y spans 7.5..37.5 device pixels, which rounds outward to 7..38.
        assert_eq!(
            region_spans(&[shape(10.0, 5.0, 50.0, 20.0, 0.0)], 1.5),
            [Span {
                x: 15,
                y: 7,
                width: 75,
                height: 31
            }]
        );
    }

    #[test]
    fn rounded_corners_are_cut_and_the_straight_edges_are_not() {
        let spans = region_spans(&[shape(0.0, 0.0, 40.0, 30.0, 8.0)], 1.0);
        let pixels = covered(&spans);
        // The very corner pixel lies outside the curve; the edge midpoints
        // and the centre are inside.
        for corner in [(0, 0), (39, 0), (0, 29), (39, 29)] {
            assert!(!pixels.contains(&corner), "{corner:?} should be cut");
        }
        for inside in [(20, 0), (0, 15), (39, 15), (20, 29), (20, 15)] {
            assert!(pixels.contains(&inside), "{inside:?} should be kept");
        }
        // Nothing outside the bounding box, and no pixel counted twice.
        assert!(pixels
            .iter()
            .all(|&(x, y)| (0..40).contains(&x) && (0..30).contains(&y)));
        let area: i32 = spans.iter().map(|span| span.width * span.height).sum();
        assert_eq!(area as usize, pixels.len());
    }

    #[test]
    fn corner_cut_keeps_every_pixel_the_curve_touches() {
        // A pixel stays when any part of it lies inside the circle: the cut
        // never removes an anti-aliased edge pixel.
        let radius = 12.0;
        let spans = region_spans(&[shape(0.0, 0.0, 60.0, 60.0, radius)], 1.0);
        let pixels = covered(&spans);
        for y in 0..12 {
            for x in 0..12 {
                // The pixel's corner nearest the circle centre (12, 12).
                let (nx, ny) = (f64::from(x + 1), f64::from(y + 1));
                let touches = (radius - nx).powi(2) + (radius - ny).powi(2) < radius * radius;
                if touches {
                    assert!(pixels.contains(&(x, y)), "({x}, {y}) touches the curve");
                }
            }
        }
    }

    #[test]
    fn the_radius_is_clamped_to_half_the_shorter_side() {
        let spans = region_spans(&[shape(0.0, 0.0, 30.0, 10.0, 50.0)], 1.0);
        let pixels = covered(&spans);
        assert!(pixels.contains(&(15, 0)) && pixels.contains(&(15, 9)));
        assert!(pixels.iter().all(|&(_, y)| (0..10).contains(&y)));
    }

    #[test]
    fn empty_and_degenerate_surfaces_add_nothing() {
        assert!(region_spans(&[], 1.0).is_empty());
        assert!(region_spans(&[shape(5.0, 5.0, 0.0, 20.0, 4.0)], 1.0).is_empty());
    }

    #[test]
    fn separate_surfaces_leave_the_gap_between_them_open() {
        let spans = region_spans(
            &[
                shape(0.0, 0.0, 100.0, 40.0, 0.0),
                shape(0.0, 44.0, 60.0, 20.0, 0.0),
            ],
            1.0,
        );
        let pixels = covered(&spans);
        assert!(
            !pixels.contains(&(10, 42)),
            "the gap between surfaces passes clicks"
        );
        assert!(
            !pixels.contains(&(80, 50)),
            "beside the narrower surface passes clicks"
        );
        assert!(pixels.contains(&(10, 50)));
    }

    #[test]
    fn validation_rejects_what_no_overlay_could_send() {
        assert!(validate(&[shape(0.0, 0.0, 10.0, 10.0, 2.0)]).is_ok());
        assert!(validate(&[shape(f64::NAN, 0.0, 10.0, 10.0, 0.0)]).is_err());
        assert!(validate(&[shape(0.0, 0.0, -1.0, 10.0, 0.0)]).is_err());
        assert!(validate(&[shape(0.0, 0.0, 10.0, 10.0, -2.0)]).is_err());
        assert!(validate(&[shape(1e9, 0.0, 10.0, 10.0, 0.0)]).is_err());
        assert!(validate(&vec![shape(0.0, 0.0, 1.0, 1.0, 0.0); MAX_SHAPES + 1]).is_err());
    }
}
