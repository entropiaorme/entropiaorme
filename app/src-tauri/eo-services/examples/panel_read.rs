//! Run the bundled recogniser over a calibrated panel and print what it
//! reads, field by field. Answers the only question calibration leaves
//! open: whether the rectangles contain text the engine can actually
//! resolve, as opposed to text a person can.
//!
//! Reads a saved panel image when given one, so a capture can be
//! re-examined offline as the parsing changes, and captures live
//! otherwise.
//!
//! ```sh
//! cargo run -p eo-services --features linux-capture --example panel_read -- --panel shots/panel.png
//! cargo run -p eo-services --features linux-capture --example panel_read
//! ```

use std::path::{Path, PathBuf};

use eo_services::game_ui::geometry::BgrImage;
use eo_services::ocr_engine::{load_bgr_png, OcrEngine};
use eo_services::scan_presets::ScanPresets;
use eo_services::{eu_window, screen_capture};

fn resource(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../entropia-orme/resources")
        .join(relative)
}

/// The panel pixels: a saved capture, or a fresh one from the calibrated
/// region.
fn panel_image(saved: Option<&Path>, presets: &ScanPresets) -> Result<BgrImage, String> {
    if let Some(path) = saved {
        let bytes = std::fs::read(path).map_err(|error| format!("{path:?}: {error}"))?;
        let (data, h, w) = load_bgr_png(&bytes).map_err(|error| format!("{path:?}: {error}"))?;
        return Ok(BgrImage { data, h, w });
    }
    // Point the capture at the stored consent token before it opens a
    // stream. Without one it asks the portal for fresh consent, and that
    // dialog opens behind a fullscreen game: the wait then looks like a
    // hang, with nothing on screen to say what it is waiting for.
    if std::env::var_os("EO_CAPTURE_TOKEN_PATH").is_none() {
        // The data directory this checkout is configured with, falling back
        // to the repository default only when nothing set one.
        let data_dir = std::env::var_os("ENTROPIAORME_DATA_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data"),
            PathBuf::from,
        );
        let token = data_dir.join("capture-restore-token");
        if token.is_file() {
            std::env::set_var("EO_CAPTURE_TOKEN_PATH", &token);
            eprintln!("consent: reusing {}", token.display());
        } else {
            eprintln!(
                "consent: no stored token, so the portal will ask. Its dialog can open behind a \
                 fullscreen game: alt-tab to it, or window the game for this run."
            );
        }
    }
    let (top_left, bottom_right) = eu_window::sale_window_region(presets)
        .ok_or("no sale-window region: the game must be running and the panel calibrated")?;
    let (x, y) = (top_left[0], top_left[1]);
    let (w, h) = (bottom_right[0] - x, bottom_right[1] - y);
    eprintln!("region: {w}x{h} at ({x}, {y}); capturing");

    // A capture that stalls stalls silently, so say so on a beat. The
    // two log lines to watch for are the portal stream being acquired
    // and the stream's first frame: between them means the stream came
    // up but is producing nothing.
    // The app holds one warm stream for its whole life, so its captures
    // read a frame that is already there. A one-shot run has to reach
    // that state first, and a cold stream can take a while to produce
    // anything, so keep asking rather than making one cold attempt and
    // calling the screen unreadable.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut attempts = 0;
    loop {
        if let Some(frame) = screen_capture::capture_region_bgr(x, y, w, h) {
            eprintln!("captured on attempt {}", attempts + 1);
            return Ok(frame);
        }
        attempts += 1;
        if std::time::Instant::now() >= deadline {
            return Err(format!("no frame after {attempts} attempts over 60s"));
        }
        if attempts % 5 == 0 {
            eprintln!("  still no frame after {attempts} attempts");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn main() {
    // The capture path reports what it is doing through tracing, which
    // is worth nothing without a subscriber attached.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut saved: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--panel" => saved = args.next().map(PathBuf::from),
            other => {
                eprintln!("read: unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let presets = ScanPresets::new(&resource("panel_geometry.json"));
    let anchor = &presets.sale_window;
    if anchor.cells.is_empty() {
        eprintln!("read: the sale window has no calibrated fields");
        std::process::exit(1);
    }

    let panel = panel_image(saved.as_deref(), &presets).unwrap_or_else(|error| {
        eprintln!("read: {error}");
        std::process::exit(1);
    });
    eprintln!("panel: {}x{}", panel.w, panel.h);

    let started = std::time::Instant::now();
    // The production provider ladder, so what this prints is what the
    // app would read rather than what a different session would.
    let engine = OcrEngine::new_with_providers(
        &resource("models/svtrv2_rec.onnx"),
        &resource("models/ppocr_keys_v1.txt"),
    )
    .unwrap_or_else(|error| {
        eprintln!("read: the recogniser did not load: {error}");
        std::process::exit(1);
    });
    eprintln!("engine loaded in {:?}", started.elapsed());

    for (name, cell) in &anchor.cells {
        let (x, y, w, h) = cell.single_rect();
        let crop = panel.crop(y, y + h, x, x + w);
        let at = std::time::Instant::now();
        match engine.recognize_bgr(&crop.data, crop.h, crop.w) {
            Ok((text, confidence)) => {
                eprintln!(
                    "  {name:14} {confidence:>5.3}  {text:?}   [{:>4}x{:<3} {:?}]",
                    crop.w,
                    crop.h,
                    at.elapsed()
                )
            }
            Err(error) => eprintln!("  {name:14}    -    <{error}>"),
        }
    }
}
