//! Measures what reading the game's screen costs on this machine.
//!
//! A reader that watches the screen spends three things each time it
//! looks: a capture, a search for what it reads, and the recogniser. This
//! tool measures each of them apart and then together, from saved
//! full-screen captures, so that what a new watching reader would cost
//! can be worked out before it is built. It calls the readers and the
//! recogniser the application runs, on the execution provider asked for.
//!
//! ```sh
//! cargo run --release -p eo-services --example read_cost -- <subcommand> ...
//! ```
//!
//! Every subcommand takes `--provider gpu|cpu` (default `gpu`: the
//! application's own choice, the graphics card where there is one) and
//! prints one JSON object per line.
//!
//! - `load`: what loading the recogniser costs in time and memory, and its
//!   first reads.
//! - `sweep <frame> --rect x0,y0,x1,y1 [--repeat <n>]`: one recognition
//!   against the height and width of what is read. The rectangle is a
//!   piece of lettering in the frame, tiled to each size.
//! - `threads <frame> --rect x0,y0,x1,y1 [--seconds <n>]`: recognitions
//!   asked for from one, two and four threads at once.
//! - `locate <frame>`: the searches that find something to read, over the
//!   whole frame and over parts of it.
//! - `copy`: what handing a captured area to a reader costs, by area.
//! - `tool <frame>... --rect x0,y0,x1,y1 [--jitter <px>]`: one rectangle
//!   read in each frame, as given and moved about by a few pixels.
//! - `match <names file> <text>...`: a text matched against a list of
//!   names, one per line, as a name read off the tool bar is (a text
//!   ending in dots was cut short): the nearest names with their scores,
//!   what it resolves to, and how long each takes.
//! - `together <frame> [--plate-ms <n>] [--radar-ms <n>] [--tool-ms <n>
//!   --tool-rect x0,y0,x1,y1] [--capture none|whole|part] [--seconds <n>]`:
//!   the watching readers run side by side at their intervals, as the
//!   application runs them, with what each look took and what the process
//!   used.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eo_services::game_ui::field::{label_key, read_run};
use eo_services::game_ui::geometry::{crop, BgrImage, Rect};
use eo_services::game_ui::plates::{
    find_locked_plate, find_plates_in, read_found_plate, PlateScratch,
};
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::radar::{find_radar, read_coordinates, CoordinateBounds};
use eo_services::game_ui::text::text_runs;
use eo_services::game_ui::tool_bar::ToolName;
use eo_services::game_ui::tool_names::{self, BundledTool, ToolCatalogue};
use eo_services::game_ui::window::find_windows;
use eo_services::ocr_engine::{load_bgr_png, preprocess_into, resize_bilinear, OcrEngine};
use serde_json::{json, Value};

fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources")
}

/// The command line: positional words and `--name value` options.
struct Args {
    words: Vec<String>,
    options: BTreeMap<String, String>,
}

impl Args {
    fn parse(raw: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut args = Args {
            words: Vec::new(),
            options: BTreeMap::new(),
        };
        let mut raw = raw;
        while let Some(word) = raw.next() {
            match word.strip_prefix("--") {
                Some(name) => {
                    let value = raw
                        .next()
                        .ok_or_else(|| format!("--{name} takes a value"))?;
                    args.options.insert(name.to_string(), value);
                }
                None => args.words.push(word),
            }
        }
        Ok(args)
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.option(name) {
            Some(text) => text
                .parse()
                .map_err(|_| format!("--{name} is not a number: {text}")),
            None => Ok(default),
        }
    }

    fn rect(&self, name: &str) -> Result<Rect, String> {
        let text = self
            .option(name)
            .ok_or_else(|| format!("--{name} x0,y0,x1,y1 is needed"))?;
        let parts: Vec<i32> = text
            .split(',')
            .map(|part| part.trim().parse::<i32>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("--{name} is not x0,y0,x1,y1: {text}"))?;
        match parts[..] {
            [x0, y0, x1, y1] if x1 >= x0 && y1 >= y0 => Ok(Rect::new(x0, y0, x1, y1)),
            _ => Err(format!("--{name} is not x0,y0,x1,y1: {text}")),
        }
    }

    fn word(&self, index: usize, what: &str) -> Result<&str, String> {
        self.words
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("{what} is needed"))
    }
}

fn load_frame(path: &str) -> Result<BgrImage, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let (data, h, w) = load_bgr_png(&bytes).map_err(|error| format!("{path}: {error}"))?;
    Ok(BgrImage { data, h, w })
}

fn load_profile() -> Result<UiProfile, String> {
    let path = resources().join("ui_profile.json");
    UiProfile::load(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The recogniser on the provider asked for. The runtime is pinned to the
/// bundled library unless the environment names another. Asking for the
/// processor hides the graphics drivers from the provider ladder on
/// Linux, which is how the application itself comes to run there on a
/// machine without one; elsewhere it loads the runtime's own default.
fn load_engine(args: &Args) -> Result<OcrEngine, String> {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        #[cfg(target_os = "linux")]
        let dylib = resources().join("ort-linux/libonnxruntime.so");
        #[cfg(not(target_os = "linux"))]
        let dylib = resources().join("ort/onnxruntime.dll");
        if !dylib.is_file() {
            return Err(format!(
                "the bundled ONNX Runtime is absent at {}",
                dylib.display()
            ));
        }
        std::env::set_var("ORT_DYLIB_PATH", &dylib);
    }
    let models = resources().join("models");
    let (model, dict) = (
        models.join("svtrv2_rec.onnx"),
        models.join("ppocr_keys_v1.txt"),
    );
    let engine = match args.option("provider").unwrap_or("gpu") {
        "gpu" => OcrEngine::new_with_providers(&model, &dict),
        "cpu" if cfg!(target_os = "linux") => {
            std::env::set_var("VK_DRIVER_FILES", "/nonexistent/driver.json");
            std::env::remove_var("VK_ICD_FILENAMES");
            OcrEngine::new_with_providers(&model, &dict)
        }
        "cpu" => OcrEngine::new(&model, &dict),
        other => return Err(format!("--provider is gpu or cpu, not {other}")),
    };
    engine.map_err(|error| format!("the recogniser did not load ({error})"))
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// The spread of a set of timings, in milliseconds.
fn spread(mut samples: Vec<f64>) -> Value {
    if samples.is_empty() {
        return json!({ "n": 0 });
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    json!({
        "n": samples.len(),
        "min": round(samples[0]),
        "p50": round(at(0.5)),
        "p95": round(at(0.95)),
        "p99": round(at(0.99)),
        "max": round(samples[samples.len() - 1]),
        "mean": round(mean),
    })
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let out = f();
    (out, ms(started.elapsed()))
}

fn sample(repeat: usize, mut f: impl FnMut()) -> Value {
    let mut samples = Vec::with_capacity(repeat);
    for _ in 0..repeat {
        let ((), took) = timed(&mut f);
        samples.push(took);
    }
    spread(samples)
}

/// Resident memory of this process in megabytes, where the platform says.
fn resident_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(round(kb / 1024.0))
}

/// Processor seconds this process has used, all threads, where the
/// platform says (to the scheduler's tick).
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks: f64 = fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

/// `source` tiled sideways and scaled to `h` by `w`.
fn tiled(source: &BgrImage, h: usize, w: usize) -> BgrImage {
    let wide = (w * source.h).div_ceil(h).max(1);
    let mut data = Vec::with_capacity(source.h * wide * 3);
    for y in 0..source.h {
        for x in 0..wide {
            let i = (y * source.w + x % source.w) * 3;
            data.extend_from_slice(&source.data[i..i + 3]);
        }
    }
    BgrImage {
        data: resize_bilinear(&data, source.h, wide, h, w),
        h,
        w,
    }
}

fn whole(image: &BgrImage) -> Rect {
    Rect::new(0, 0, image.w as i32 - 1, image.h as i32 - 1)
}

fn cmd_load(args: &Args) -> Result<(), String> {
    let before = resident_mb();
    let (engine, load_ms) = timed(|| load_engine(args));
    let engine = engine?;
    let loaded = resident_mb();
    let ((), warm_ms) = timed(|| engine.warm_up());
    let profile = load_profile()?;
    let image = BgrImage {
        data: vec![200; 24 * 300 * 3],
        h: 24,
        w: 300,
    };
    let (_, first_ms) = timed(|| read_run(&image, whole(&image), &profile.read, &engine));
    let steady = sample(50, || {
        black_box(read_run(&image, whole(&image), &profile.read, &engine));
    });
    println!(
        "{}",
        json!({
            "what": "load", "provider": engine.provider(),
            "load_ms": round(load_ms), "warm_up_ms": round(warm_ms),
            "first_read_ms": round(first_ms), "steady": steady,
            "resident_mb": { "before": before, "loaded": loaded, "after_reads": resident_mb() },
        })
    );
    Ok(())
}

fn cmd_sweep(args: &Args) -> Result<(), String> {
    let frame = load_frame(args.word(1, "a frame")?)?;
    let source = crop(&frame, args.rect("rect")?);
    let repeat: usize = args.number("repeat", 40)?;
    let profile = load_profile()?;
    let engine = load_engine(args)?;
    engine.warm_up();
    let heights = [12usize, 16, 24, 32, 48, 96];
    let aspects = [1.0f64, 3.0, 6.67, 8.0, 10.0, 12.0, 16.0, 24.0, 32.0, 48.0];
    for h in heights {
        for aspect in aspects {
            let w = ((h as f64) * aspect).round() as usize;
            let image = tiled(&source, h, w);
            let rect = whole(&image);
            let handed = crop(&image, rect);
            let mut buffer = Vec::new();
            let (_, input_w) = preprocess_into(&handed.data, handed.h, handed.w, &mut buffer);
            let (_, first_ms) = timed(|| read_run(&image, rect, &profile.read, &engine));
            let steady = sample(repeat, || {
                black_box(read_run(&image, rect, &profile.read, &engine));
            });
            let prepare = sample(repeat, || {
                black_box(preprocess_into(
                    &handed.data,
                    handed.h,
                    handed.w,
                    &mut buffer,
                ));
            });
            println!(
                "{}",
                json!({
                    "what": "sweep", "provider": engine.provider(),
                    "h": h, "w": w, "aspect": aspect, "model_input_w": input_w,
                    "first_ms": round(first_ms), "read": steady, "prepare": prepare,
                })
            );
        }
    }
    Ok(())
}

fn cmd_threads(args: &Args) -> Result<(), String> {
    let frame = load_frame(args.word(1, "a frame")?)?;
    let rect = args.rect("rect")?;
    let seconds: f64 = args.number("seconds", 5.0)?;
    let profile = Arc::new(load_profile()?);
    let engine = Arc::new(load_engine(args)?);
    engine.warm_up();
    let image = Arc::new(crop(&frame, rect.padded(8)));
    for threads in [1usize, 2, 4] {
        let cpu_before = cpu_seconds();
        let started = Instant::now();
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (engine, image, profile) = (engine.clone(), image.clone(), profile.clone());
                std::thread::spawn(move || {
                    let mut samples = Vec::new();
                    while started.elapsed().as_secs_f64() < seconds {
                        let (_, took) =
                            timed(|| read_run(&image, whole(&image), &profile.read, &*engine));
                        samples.push(took);
                    }
                    samples
                })
            })
            .collect();
        let samples: Vec<f64> = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect();
        let wall = started.elapsed().as_secs_f64();
        let cores = cpu_before
            .zip(cpu_seconds())
            .map(|(before, after)| round((after - before) / wall));
        println!(
            "{}",
            json!({
                "what": "threads", "provider": engine.provider(), "threads": threads,
                "reads_per_second": round(samples.len() as f64 / wall),
                "cores_used": cores, "each_read": spread(samples),
            })
        );
    }
    Ok(())
}

fn cmd_locate(args: &Args) -> Result<(), String> {
    let frame = load_frame(args.word(1, "a frame")?)?;
    let repeat: usize = args.number("repeat", 30)?;
    let profile = load_profile()?;
    let (w, h) = (frame.w as i32, frame.h as i32);
    let mut scratch = PlateScratch::default();
    let line = |name: &str, area: Rect, spread: Value| {
        println!(
            "{}",
            json!({
                "what": "locate", "search": name,
                "area": [area.width(), area.height()],
                "megapixels": round(f64::from(area.width()) * f64::from(area.height()) / 1e6),
                "took": spread,
            })
        );
    };
    let full = whole(&frame);
    line(
        "locked plate (the watcher's search)",
        full,
        sample(repeat, || {
            black_box(find_locked_plate(&frame, &profile, &mut scratch));
        }),
    );
    line(
        "radar",
        full,
        sample(repeat, || {
            black_box(find_radar(&frame, &profile));
        }),
    );
    line(
        "windows",
        full,
        sample(repeat, || {
            black_box(find_windows(&frame, &profile.chrome));
        }),
    );
    let areas = [
        ("whole frame", full),
        ("upper half", Rect::new(0, 0, w - 1, h / 2)),
        (
            "middle quarter",
            Rect::new(w / 4, h / 4, w * 3 / 4, h * 3 / 4),
        ),
        ("bottom tenth", Rect::new(0, h * 9 / 10, w - 1, h - 1)),
        (
            "bottom middle strip",
            Rect::new(w / 4, h * 9 / 10, w * 3 / 4, h - 1),
        ),
    ];
    for (name, area) in areas {
        line(
            &format!("every plate, {name}"),
            area,
            sample(repeat, || {
                black_box(find_plates_in(&frame, area, &profile));
            }),
        );
        line(
            &format!("runs of lettering, {name}"),
            area,
            sample(repeat, || {
                black_box(text_runs(&frame, area, &profile.text));
            }),
        );
    }
    Ok(())
}

/// What a capture hands a reader: the area cut out of the latest screen
/// frame (four bytes a pixel) and repacked three bytes a pixel, the two
/// copies the capturer makes on every platform.
fn handed_over(screen: &[u8], screen_w: usize, area: Rect) -> BgrImage {
    let (w, h) = (area.width() as usize, area.height() as usize);
    let mut cut = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let start = ((area.y0 as usize + y) * screen_w + area.x0 as usize) * 4;
        cut.extend_from_slice(&screen[start..start + w * 4]);
    }
    let mut data = Vec::with_capacity(w * h * 3);
    for px in cut.chunks_exact(4) {
        data.push(px[0]);
        data.push(px[1]);
        data.push(px[2]);
    }
    BgrImage { data, h, w }
}

fn cmd_copy(args: &Args) -> Result<(), String> {
    let repeat: usize = args.number("repeat", 30)?;
    let (sw, sh) = (3840usize, 2160usize);
    let screen: Vec<u8> = (0..sw * sh * 4).map(|i| (i % 251) as u8).collect();
    for (w, h) in [
        (3840, 2160),
        (2560, 1440),
        (1920, 1080),
        (2560, 144),
        (1280, 144),
        (640, 360),
        (400, 40),
        (170, 30),
    ] {
        let area = Rect::new(0, 0, w - 1, h - 1);
        println!(
            "{}",
            json!({
                "what": "copy", "area": [w, h],
                "megapixels": round(f64::from(w) * f64::from(h) / 1e6),
                "megabytes_handed": round(f64::from(w) * f64::from(h) * 3.0 / 1e6),
                "took": sample(repeat, || { black_box(handed_over(&screen, sw, area)); }),
            })
        );
    }
    Ok(())
}

fn cmd_tool(args: &Args) -> Result<(), String> {
    let rect = args.rect("rect")?;
    let jitter: i32 = args.number("jitter", 0)?;
    let profile = load_profile()?;
    let engine = load_engine(args)?;
    engine.warm_up();
    for path in &args.words[1..] {
        let frame = load_frame(path)?;
        let name = Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path);
        let mut moves = vec![(0, 0, 0)];
        if jitter > 0 {
            for d in [-jitter, jitter] {
                moves.extend([(d, 0, 0), (0, d, 0), (0, 0, d)]);
            }
        }
        for (dx, dy, grow) in moves {
            let moved = rect.offset(dx, dy).padded(grow);
            // Read once to settle the shape, then time the reads after it.
            let read = read_run(&frame, moved, &profile.read, &engine);
            let took = sample(20, || {
                black_box(read_run(&frame, moved, &profile.read, &engine));
            });
            let (text, score) = read.unwrap_or_default();
            println!(
                "{}",
                json!({
                    "what": "tool", "provider": engine.provider(), "frame": name,
                    "moved": [dx, dy], "grown": grow,
                    "text": text, "score": round(score), "took": took,
                })
            );
        }
    }
    Ok(())
}

fn cmd_match(args: &Args) -> Result<(), String> {
    let path = args.word(1, "a file of names")?;
    let names: Vec<String> = std::fs::read_to_string(path)
        .map_err(|error| format!("{path}: {error}"))?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let profile = load_profile()?;
    let p = &profile.tool_names;
    let catalogue = ToolCatalogue::new(
        Vec::new(),
        names.iter().enumerate().map(|(line, name)| BundledTool {
            endpoint: path.to_string(),
            item_id: (line + 1).to_string(),
            name: name.clone(),
        }),
        Vec::new(),
    );
    let tools: Vec<(Vec<char>, &str)> = catalogue
        .bundled()
        .map(|tool| (label_key(&tool.name), tool.name.as_str()))
        .collect();
    for text in &args.words[2..] {
        // A text ending in an ellipsis was cut short by the game.
        let trimmed = text.trim_end();
        let read = ToolName {
            text: trimmed.trim_end_matches(['.', '\u{2026}']).to_string(),
            cut_short: trimmed.ends_with("...") || trimmed.ends_with('\u{2026}'),
        };
        let wanted = read.key();
        let mut scored: Vec<(f64, &str)> = Vec::new();
        let took = sample(50, || {
            scored = tools
                .iter()
                .filter_map(|(key, name)| {
                    tool_names::score(&wanted, read.cut_short, key, p).map(|s| (s, *name))
                })
                .collect();
            black_box(&scored);
        });
        let resolved = tool_names::resolve(&read, &catalogue, p);
        let resolve_took = sample(50, || {
            black_box(tool_names::resolve(&read, &catalogue, p));
        });
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let best: Vec<Value> = scored
            .iter()
            .take(5)
            .map(|(s, name)| json!([round(*s), name]))
            .collect();
        println!(
            "{}",
            json!({
                "what": "match", "text": text, "cut_short": read.cut_short,
                "names": names.len(), "best": best, "took": took,
                "resolved": format!("{resolved:?}"), "resolve_took": resolve_took,
            })
        );
    }
    Ok(())
}

/// One watching reader's schedule: looks every `every`, a look that runs
/// past the next one's moment dropping the looks it missed, as the
/// application's watchers do.
fn watch(
    name: &'static str,
    every: Duration,
    offset: Duration,
    stop: Arc<AtomicBool>,
    mut look: impl FnMut() + Send + 'static,
) -> std::thread::JoinHandle<Value> {
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut due = offset;
        let mut samples = Vec::new();
        let mut dropped = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let now = started.elapsed();
            if now < due {
                std::thread::sleep((due - now).min(Duration::from_millis(20)));
                continue;
            }
            let ((), took) = timed(&mut look);
            samples.push(took);
            due += every;
            while due < started.elapsed() {
                due += every;
                dropped += 1;
            }
        }
        json!({
            "reader": name, "every_ms": every.as_millis() as u64,
            "dropped_looks": dropped, "look": spread(samples),
        })
    })
}

fn cmd_together(args: &Args) -> Result<(), String> {
    let frame = Arc::new(load_frame(args.word(1, "a frame")?)?);
    let seconds: f64 = args.number("seconds", 30.0)?;
    let plate_ms: u64 = args.number("plate-ms", 1000)?;
    let radar_ms: u64 = args.number("radar-ms", 500)?;
    let tool_ms: u64 = args.number("tool-ms", 0)?;
    let capture = args.option("capture").unwrap_or("whole").to_string();
    if !["none", "whole", "part"].contains(&capture.as_str()) {
        return Err(format!("--capture is none, whole or part, not {capture}"));
    }
    let profile = Arc::new(load_profile()?);
    let engine = Arc::new(load_engine(args)?);
    engine.warm_up();
    // The screen a capture is cut from, the frame's size.
    let screen: Arc<Vec<u8>> = Arc::new(vec![90u8; frame.w * frame.h * 4]);
    let full = whole(&frame);
    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();

    if plate_ms > 0 {
        let (frame, profile, engine, screen, capture) = (
            frame.clone(),
            profile.clone(),
            engine.clone(),
            screen.clone(),
            capture.clone(),
        );
        let mut scratch = PlateScratch::default();
        readers.push(watch(
            "creature plate",
            Duration::from_millis(plate_ms),
            Duration::ZERO,
            stop.clone(),
            move || {
                // The creature watcher captures the whole window each look.
                if capture != "none" {
                    black_box(handed_over(&screen, frame.w, full));
                }
                if let Some(locked) = find_locked_plate(&frame, &profile, &mut scratch) {
                    black_box(read_found_plate(&frame, &locked, &profile, &*engine));
                }
            },
        ));
    }
    if radar_ms > 0 {
        let (frame, profile, engine, screen, capture) = (
            frame.clone(),
            profile.clone(),
            engine.clone(),
            screen.clone(),
            capture.clone(),
        );
        let bounds = CoordinateBounds {
            lon: (i64::MIN, i64::MAX),
            lat: (i64::MIN, i64::MAX),
        };
        readers.push(watch(
            "radar coordinates",
            Duration::from_millis(radar_ms),
            Duration::from_millis(137),
            stop.clone(),
            move || {
                if capture != "none" {
                    black_box(handed_over(&screen, frame.w, full));
                }
                black_box(read_coordinates(&frame, &profile, &bounds, &*engine).ok());
            },
        ));
    }
    if tool_ms > 0 {
        let rect = args.rect("tool-rect")?;
        let (frame, profile, engine, screen, capture) = (
            frame.clone(),
            profile.clone(),
            engine.clone(),
            screen.clone(),
            capture.clone(),
        );
        readers.push(watch(
            "held tool",
            Duration::from_millis(tool_ms),
            Duration::from_millis(61),
            stop.clone(),
            move || {
                match capture.as_str() {
                    "whole" => {
                        black_box(handed_over(&screen, frame.w, full));
                    }
                    "part" => {
                        black_box(handed_over(&screen, frame.w, rect.padded(8)));
                    }
                    _ => {}
                }
                black_box(read_run(&frame, rect, &profile.read, &*engine));
            },
        ));
    }
    if readers.is_empty() {
        return Err("no reader was asked for".to_string());
    }
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(seconds));
    stop.store(true, Ordering::Relaxed);
    let wall = started.elapsed().as_secs_f64();
    let cores = cpu_before
        .zip(cpu_seconds())
        .map(|(before, after)| round((after - before) / wall));
    let readers: Vec<Value> = readers
        .into_iter()
        .filter_map(|reader| reader.join().ok())
        .collect();
    println!(
        "{}",
        json!({
            "what": "together", "provider": engine.provider(), "capture": capture,
            "seconds": round(wall), "cores_used": cores,
            "resident_mb": resident_mb(), "readers": readers,
        })
    );
    Ok(())
}

fn run(args: &Args) -> Result<(), String> {
    match args.words.first().map(String::as_str) {
        Some("load") => cmd_load(args),
        Some("sweep") => cmd_sweep(args),
        Some("threads") => cmd_threads(args),
        Some("locate") => cmd_locate(args),
        Some("copy") => cmd_copy(args),
        Some("tool") => cmd_tool(args),
        Some("match") => cmd_match(args),
        Some("together") => cmd_together(args),
        _ => Err(
            "subcommands: load, sweep, threads, locate, copy, tool, match, together \
             (see the top of examples/read_cost.rs)"
                .to_string(),
        ),
    }
}

fn main() -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!(
            "this is an unoptimised build, many times slower than the application: build with \
             --release for figures that mean anything"
        );
    }
    let outcome = Args::parse(std::env::args().skip(1)).and_then(|args| run(&args));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("read_cost: {why}");
            ExitCode::FAILURE
        }
    }
}
