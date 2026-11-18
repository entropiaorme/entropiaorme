//! Stand-alone probe for the Linux screen-capture engine: acquires the
//! portal stream (reusing the restore token from `EO_CAPTURE_TOKEN_PATH`
//! when present), then captures the same rectangle three times across
//! twelve seconds. Each capture must observe a newer frame generation,
//! so a cached first frame cannot masquerade as a live stream.
//!
//! ```sh
//! EO_CAPTURE_TOKEN_PATH=<data-dir>/capture-restore-token \
//!   cargo run -p eo-services --features linux-capture --example capture_probe -- \
//!   <x> <y> <width> <height>
//! ```
//!
//! With `rate [seconds]` in place of the rectangle it measures the stream
//! instead: the frames a second the compositor sends and how many are
//! kept. It saves nothing.

#[cfg(all(target_os = "linux", feature = "linux-capture"))]
fn capture_region_from_args() -> Result<(i64, i64, i64, i64), String> {
    let values = std::env::args().skip(1).collect::<Vec<_>>();
    if values.len() != 4 {
        return Err("expected capture rectangle: <x> <y> <width> <height>".to_string());
    }
    let mut parsed = Vec::with_capacity(4);
    for value in values {
        parsed.push(
            value
                .parse::<i64>()
                .map_err(|_| format!("capture rectangle value is not an integer: {value}"))?,
        );
    }
    if parsed[2] <= 0 || parsed[3] <= 0 {
        return Err("capture width and height must be positive".to_string());
    }
    Ok((parsed[0], parsed[1], parsed[2], parsed[3]))
}

/// Measure the stream for `seconds` while it is captured from: how many
/// frames a second the compositor sends and how many are kept. Sent near
/// the limit, the compositor is keeping to it; sent well past it, the
/// frames are being dropped on arrival instead. Nothing captured is saved.
#[cfg(all(target_os = "linux", feature = "linux-capture"))]
fn measure_rate(seconds: u64) {
    use eo_services::screen_capture::{
        capturable_area, capture_frame_generation, capture_frames_offered, capture_region_bgr,
    };
    use std::time::{Duration, Instant};

    if !eo_services::screen_capture::start_capture() {
        eprintln!("probe rate: capture did not start");
        std::process::exit(1);
    }
    let Some((x, y, _, _)) = capturable_area() else {
        eprintln!("probe rate: no capturable area");
        std::process::exit(1);
    };
    let generation = || capture_frame_generation().unwrap_or_default();
    let (started, kept, offered) = (Instant::now(), generation(), capture_frames_offered());
    while started.elapsed() < Duration::from_secs(seconds) {
        if capture_region_bgr(x, y, 1, 1).is_none() {
            eprintln!("probe rate: capture returned None");
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let per_second = |count: u64| count as f64 / seconds as f64;
    eprintln!(
        "probe rate: over {seconds} s the compositor sent {:.1} frames a second and {:.1} were kept",
        per_second(capture_frames_offered() - offered),
        per_second(generation() - kept),
    );
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(all(target_os = "linux", feature = "linux-capture"))]
    if std::env::args().nth(1).as_deref() == Some("rate") {
        let seconds = std::env::args()
            .nth(2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(10);
        measure_rate(seconds);
        return;
    }
    #[cfg(all(target_os = "linux", feature = "linux-capture"))]
    {
        let (x, y, width, height) = capture_region_from_args().unwrap_or_else(|error| {
            eprintln!("probe: {error}");
            eprintln!("usage: capture_probe <x> <y> <width> <height>");
            std::process::exit(2);
        });
        let mut failed = false;
        let mut previous_generation = None;
        for (label, delay_ms) in [("t+0s", 0u64), ("t+3s", 3000), ("t+12s", 9000)] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            match eo_services::screen_capture::capture_region_png(x, y, width, height) {
                Some(png) => {
                    let generation = eo_services::screen_capture::capture_frame_generation();
                    let out = std::env::temp_dir().join(format!("eo-capture-probe-{label}.png"));
                    let _ = std::fs::write(&out, &png);
                    eprintln!(
                        "probe {label}: OK ({} bytes, frame {generation:?}) -> {}",
                        png.len(),
                        out.display()
                    );
                    if generation.is_none() || generation <= previous_generation {
                        eprintln!(
                            "probe {label}: stream did not advance beyond frame {previous_generation:?}"
                        );
                        failed = true;
                    }
                    previous_generation = generation;
                }
                None => {
                    eprintln!("probe {label}: capture returned None");
                    failed = true;
                }
            }
        }
        if failed {
            std::process::exit(1);
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "linux-capture")))]
    eprintln!("probe: linux-capture feature not enabled on this build");
}
