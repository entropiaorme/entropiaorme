//! Measuring and replay tool for the interface profile.
//!
//! Everything the application knows about how the game client draws its
//! interface is data in the interface profile. When the client's
//! interface changes, the profile is measured again from saved
//! full-screen captures; this tool does the measuring, shows what each
//! reader makes of a frame, and replays a whole corpus of labelled frames
//! against a profile under edit. It works from PNG files only and calls
//! the readers the application runs, so what it prints is what the
//! application would read.
//!
//! ```sh
//! cargo run -p eo-services --example ui_measure -- <subcommand> ...
//! ```
//!
//! Subcommands taking a frame (`<frame.png>`, a lossless full-screen
//! capture):
//!
//! - `windows <frame>`: every window found, and the kind its title names.
//! - `runs <frame> [--window <kind>]`: the runs of text in each window.
//! - `field <frame> <window kind> <field>`: one labelled field, read.
//! - `table <frame> --window <kind>`: the rule lines of a window's table,
//!   and the upright rules that cross each row.
//! - `radar <frame> [--planet <name>]`: the radar, its coordinates, rings
//!   and scale.
//! - `plates <frame>`: every nameplate, and the locked one read.
//! - `sale-queue <frame>`, `skills <frame>`, `terminal <frame>`: the whole
//!   typed read of each reader, row by row.
//! - `measure-chrome <frame> [--rect x0,y0,x1,y1] [--depth <n>]`: a
//!   window's frame, measured, as a fragment of the profile's `chrome`.
//! - `measure-plate <frame>`: a `plates.scales[]` entry measured from the
//!   solid nameplates in view.
//! - `measure-numerals <frame> <window kind> <field> <expected text>`, or
//!   `measure-numerals <frame> --rect x0,y0,x1,y1 <expected text>`
//!   (`--numerals sale-queue` for the sell queue's metrics): one figure's
//!   ink against the numeral metrics, and a sample line for `fit-numerals`.
//! - `timing <frame> [--repeat <n>] [--planet <name>]`: how long the reads
//!   that run live take.
//!
//! Every one of them takes `--profile <path>` (default: the bundled
//! profile) and `--overlay <out.png>`, which saves the frame with what
//! was found drawn on it; `--overlay-area x0,y0,x1,y1` saves only that
//! part of it, at native size.
//!
//! - `fit-numerals <samples file> [--numerals sale-queue]`: numeral
//!   metrics fitted to sample lines by least squares.
//! - `replay <corpus dir> [--profile <path>] [--held-out]`: every reader
//!   over every frame of a corpus (`frames/*.png`, `labels.json`,
//!   `manifest.json`), with the tallies the real-capture tests print.
//!   Exits non-zero when a wrong value was accepted or a refusal is not
//!   accounted for. Frames the manifest holds out are skipped unless
//!   `--held-out` asks for them alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::field::{locate_field, read_run, Recogniser, Remembering};
use eo_services::game_ui::geometry::{BgrImage, Rect};
use eo_services::game_ui::locate::{
    identify_windows, locate, read_window_field, Identified, Located,
};
use eo_services::game_ui::mob_names::{resolve, MobCatalogue};
use eo_services::game_ui::plates::{
    find_locked_plate, find_plates, measure_plates, plate_line, read_locked_plate, PlateKind,
    PlateScratch,
};
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::radar::{
    find_radar, find_rings, read_coordinates, read_planet_label, ring_scale, CoordinateBounds,
};
use eo_services::game_ui::sale_queue::{read_sale_queue, ItemCatalogue};
use eo_services::game_ui::seams::{frame_reader, GameFrame};
use eo_services::game_ui::skills::read_skills_page;
use eo_services::game_ui::table::{column_rules, row_rules, rows, Panel};
use eo_services::game_ui::validate::{expected_width, ink_size, NumeralMetrics};
use eo_services::game_ui::window::{find_windows, measure_chrome, FoundWindow, Side};
use eo_services::ocr_engine::{load_bgr_png, resize_bilinear, OcrEngine};
use eo_services::planet_maps::PlanetMapStore;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders, Terminal};
use serde_json::{json, Value};

#[path = "../tests/game_ui_replay/mod.rs"]
mod replay;

use replay::{resources, Corpus, Outcome, Replay, Shard, REPAIR_COST, TRADE_TOTAL};

/// How far in from a window's edge its frame is measured, unless
/// `--depth` says otherwise: past the thickest ridge the client draws.
const DEFAULT_DEPTH: i32 = 6;

/// The width the smaller timing frame is scaled to: a 1920x1080 screen's.
const SMALLER_WIDTH: usize = 1920;

const RED: [u8; 3] = [255, 60, 60];
const GREEN: [u8; 3] = [60, 255, 90];
const YELLOW: [u8; 3] = [255, 230, 60];
const CYAN: [u8; 3] = [60, 230, 255];
const MAGENTA: [u8; 3] = [255, 80, 255];

/// The command line: positional words, and `--name value` options
/// (`--held-out` alone takes no value).
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
        let mut raw = raw.peekable();
        while let Some(word) = raw.next() {
            match word.strip_prefix("--") {
                Some("held-out") => {
                    args.options.insert("held-out".into(), String::new());
                }
                Some(name) => {
                    let value = raw
                        .next()
                        .ok_or_else(|| format!("--{name} needs a value"))?;
                    args.options.insert(name.to_string(), value);
                }
                None => args.words.push(word),
            }
        }
        Ok(args)
    }

    fn word(&self, index: usize, what: &str) -> Result<&str, String> {
        self.words
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {what}"))
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.option(name)
            .map(|text| {
                text.parse()
                    .map_err(|_| format!("--{name} {text:?} is not a number"))
            })
            .transpose()
    }

    fn rect(&self) -> Result<Option<Rect>, String> {
        self.rect_option("rect")
    }

    fn rect_option(&self, name: &str) -> Result<Option<Rect>, String> {
        let Some(text) = self.option(name) else {
            return Ok(None);
        };
        let parts: Vec<i32> = text
            .split(',')
            .map(|part| part.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("--{name} {text:?} is not x0,y0,x1,y1"))?;
        match parts[..] {
            [x0, y0, x1, y1] if x0 <= x1 && y0 <= y1 => Ok(Some(Rect::new(x0, y0, x1, y1))),
            _ => Err(format!("--{name} {text:?} is not x0,y0,x1,y1")),
        }
    }
}

fn profile_path(args: &Args) -> PathBuf {
    args.option("profile")
        .map(PathBuf::from)
        .unwrap_or_else(|| resources().join("ui_profile.json"))
}

fn load_profile(args: &Args) -> Result<UiProfile, String> {
    let path = profile_path(args);
    UiProfile::load(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn load_frame(path: &str) -> Result<BgrImage, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let (data, h, w) = load_bgr_png(&bytes).map_err(|error| format!("{path}: {error}"))?;
    Ok(BgrImage { data, h, w })
}

/// The bundled recogniser, or why there is none. The runtime is pinned
/// to the bundled library unless the environment names another: left to
/// search, the loader can hang on a foreign-format library.
fn load_recogniser() -> Result<Arc<OcrEngine>, String> {
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
    // A runtime that is not there is said so, never handed to the loader:
    // it can hang on a library it cannot use rather than report it.
    if let Some(named) = std::env::var_os("ORT_DYLIB_PATH") {
        if !Path::new(&named).is_file() {
            return Err(format!(
                "ORT_DYLIB_PATH names no file ({})",
                Path::new(&named).display()
            ));
        }
    }
    let models = resources().join("models");
    OcrEngine::new(
        &models.join("svtrv2_rec.onnx"),
        &models.join("ppocr_keys_v1.txt"),
    )
    .map(Arc::new)
    .map_err(|error| format!("the recogniser did not load ({error})"))
}

/// The recogniser the readers are handed: the bundled one, or one that
/// reads nothing, with a line saying so. The subcommands that only need
/// geometry still work without it.
fn recogniser() -> Arc<dyn Recogniser> {
    match load_recogniser() {
        Ok(engine) => {
            let read: Arc<dyn Recogniser> = engine;
            read
        }
        Err(why) => {
            println!("readings are unavailable: {why}");
            Arc::new(|_: &BgrImage| None)
        }
    }
}

/// A run's text as the recogniser reads it.
fn reading(image: &BgrImage, rect: Rect, profile: &UiProfile, read: &dyn Recogniser) -> String {
    match read_run(image, rect, &profile.read, read) {
        Some((text, confidence)) => format!("{text:?} ({confidence:.2})"),
        None => "unread".to_string(),
    }
}

fn show(rect: Rect) -> String {
    format!(
        "[{}, {}, {}, {}] {}x{}",
        rect.x0,
        rect.y0,
        rect.x1,
        rect.y1,
        rect.width(),
        rect.height()
    )
}

/// The frame with what was found drawn on it, saved where `--overlay`
/// says when a subcommand finishes.
struct Overlay {
    out: Option<PathBuf>,
    /// The part of the frame to save, when `--overlay-area` names one: a
    /// native crop, since a whole frame scaled to fit a viewer hides
    /// small lettering.
    area: Option<Rect>,
    canvas: image::RgbImage,
}

impl Overlay {
    fn new(frame: &BgrImage, args: &Args) -> Result<Overlay, String> {
        let mut canvas = image::RgbImage::new(frame.w as u32, frame.h as u32);
        for (pixel, bgr) in canvas.pixels_mut().zip(frame.data.chunks_exact(3)) {
            *pixel = image::Rgb([bgr[2], bgr[1], bgr[0]]);
        }
        Ok(Overlay {
            out: args.option("overlay").map(PathBuf::from),
            area: args.rect_option("overlay-area")?,
            canvas,
        })
    }

    /// A box drawn just outside `rect`, so the lettering inside it stays
    /// legible.
    fn around(&mut self, rect: Rect, rgb: [u8; 3]) {
        self.rect(rect.padded(2), rgb);
    }

    fn put(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as u32) < self.canvas.width() && (y as u32) < self.canvas.height()
        {
            self.canvas.put_pixel(x as u32, y as u32, image::Rgb(rgb));
        }
    }

    fn rect(&mut self, rect: Rect, rgb: [u8; 3]) {
        for x in rect.x0..=rect.x1 {
            self.put(x, rect.y0, rgb);
            self.put(x, rect.y1, rgb);
        }
        for y in rect.y0..=rect.y1 {
            self.put(rect.x0, y, rgb);
            self.put(rect.x1, y, rgb);
        }
    }

    fn row(&mut self, y: i32, x0: i32, x1: i32, rgb: [u8; 3]) {
        for x in x0..=x1 {
            self.put(x, y, rgb);
        }
    }

    fn column(&mut self, x: i32, y0: i32, y1: i32, rgb: [u8; 3]) {
        for y in y0..=y1 {
            self.put(x, y, rgb);
        }
    }

    fn circle(&mut self, centre: (f64, f64), radius: f64, rgb: [u8; 3]) {
        let steps = (radius * std::f64::consts::TAU).ceil().max(8.0) as usize;
        for k in 0..steps {
            let angle = std::f64::consts::TAU * k as f64 / steps as f64;
            self.put(
                (centre.0 + radius * angle.cos()).round() as i32,
                (centre.1 + radius * angle.sin()).round() as i32,
                rgb,
            );
        }
    }

    fn save(self) -> Result<(), String> {
        let Some(out) = self.out else { return Ok(()) };
        let whole = Rect::new(
            0,
            0,
            self.canvas.width() as i32 - 1,
            self.canvas.height() as i32 - 1,
        );
        let part = match self.area {
            Some(area) => area
                .intersect(&whole)
                .ok_or("--overlay-area lies off the frame")?,
            None => whole,
        };
        image::imageops::crop_imm(
            &self.canvas,
            part.x0 as u32,
            part.y0 as u32,
            part.width() as u32,
            part.height() as u32,
        )
        .to_image()
        .save(&out)
        .map_err(|error| format!("{}: {error}", out.display()))?;
        println!("overlay written to {}", out.display());
        Ok(())
    }
}

/// What one frame subcommand works on.
struct Session<'a> {
    args: &'a Args,
    image: BgrImage,
    profile: UiProfile,
    overlay: Overlay,
}

impl<'a> Session<'a> {
    fn open(args: &'a Args) -> Result<Session<'a>, String> {
        let image = load_frame(args.word(1, "the frame's path")?)?;
        let profile = load_profile(args)?;
        println!(
            "frame {}x{}, profile {} ({})",
            image.w,
            image.h,
            profile_path(args).display(),
            profile.client_interface
        );
        let overlay = Overlay::new(&image, args)?;
        Ok(Session {
            args,
            image,
            profile,
            overlay,
        })
    }

    /// The windows of the frame, identified, or with the recogniser
    /// unavailable found only.
    fn windows(&self, read: &dyn Recogniser) -> Vec<Identified> {
        identify_windows(&self.image, &self.profile, read).unwrap_or_else(|_| {
            let mut in_front: Vec<Rect> = Vec::new();
            find_windows(&self.image, &self.profile.chrome)
                .into_iter()
                .map(|window| {
                    let rect = window.rect;
                    let found = Identified {
                        located: Located {
                            window,
                            in_front: in_front.clone(),
                        },
                        kind: None,
                        title: Vec::new(),
                    };
                    in_front.push(rect);
                    found
                })
                .collect()
        })
    }
}

fn side(side: Side) -> &'static str {
    match side {
        Side::Seen => "seen",
        Side::Hidden => "hidden",
        Side::BeyondFrame => "beyond the frame",
    }
}

fn describe_window(index: usize, window: &FoundWindow, profile: &UiProfile) -> String {
    format!(
        "window {index}: {}\n  sides: left {}, top {}, right {}, bottom {}\n  ridge {} px{}; title marker {}; border visible {:.0}%; extent {}; {}",
        show(window.rect),
        side(window.left),
        side(window.top),
        side(window.right),
        side(window.bottom),
        window.ridge_px,
        if window.scaled_frame {
            " (the frame looks scaled by the desktop)"
        } else {
            ""
        },
        if window.marker { "seen" } else { "not seen" },
        window.visible_border * 100.0,
        if window.extent_certain() {
            "certain"
        } else {
            "uncertain"
        },
        if window.fully_visible(&profile.chrome) {
            "fully visible"
        } else {
            "partly covered or off the frame"
        },
    )
}

fn cmd_windows(mut s: Session) -> Result<(), String> {
    let read = recogniser();
    let read = Remembering::new(&*read);
    let found = s.windows(&read);
    println!("{} windows, front to back", found.len());
    for (index, window) in found.iter().enumerate() {
        println!(
            "{}",
            describe_window(index, &window.located.window, &s.profile)
        );
        match &window.kind {
            Some(kind) => println!(
                "  kind: {kind} (title {:?} in the profile)",
                s.profile.windows[kind].title
            ),
            None => println!("  kind: none (no title read as a kind the profile knows)"),
        }
        for (rect, text) in &window.title {
            println!("  title strip: {} reads {text:?}", show(*rect));
            s.overlay.around(*rect, YELLOW);
        }
        let colour = if window.located.window.extent_certain() {
            GREEN
        } else {
            RED
        };
        s.overlay.rect(window.located.window.rect, colour);
    }
    println!("overlay: green a window of certain extent, red an uncertain one, yellow title runs");
    s.overlay.save()
}

/// The windows of the kind `--window` names, or every window.
fn chosen_windows(s: &Session, read: &dyn Recogniser) -> Result<Vec<Identified>, String> {
    let found = s.windows(read);
    let Some(kind) = s.args.option("window") else {
        return Ok(found);
    };
    if !s.profile.windows.contains_key(kind) {
        return Err(format!(
            "the profile describes no window called {kind:?}; it has {:?}",
            s.profile.windows.keys().collect::<Vec<_>>()
        ));
    }
    let chosen: Vec<Identified> = found
        .into_iter()
        .filter(|w| w.kind.as_deref() == Some(kind))
        .collect();
    if chosen.is_empty() {
        return Err(format!("no {kind} window is open, or its title is covered"));
    }
    Ok(chosen)
}

fn cmd_runs(mut s: Session) -> Result<(), String> {
    let read = recogniser();
    let read = Remembering::new(&*read);
    for window in chosen_windows(&s, &read)? {
        println!(
            "window {} ({})",
            show(window.located.window.rect),
            window.kind.as_deref().unwrap_or("no kind")
        );
        s.overlay.rect(window.located.window.rect, GREEN);
        for run in window.located.runs(&s.image, &s.profile) {
            println!(
                "  {} ink {} rgb {:?}: {}",
                show(run.rect),
                run.ink,
                run.rgb,
                reading(&s.image, run.rect, &s.profile, &read)
            );
            s.overlay.around(run.rect, YELLOW);
        }
    }
    println!("overlay: green windows, yellow runs");
    s.overlay.save()
}

fn cmd_field(mut s: Session) -> Result<(), String> {
    let window = s.args.word(2, "the window kind")?;
    let field = s.args.word(3, "the field")?;
    let read = recogniser();
    let read = Remembering::new(&*read);
    // Where the field stands, whether or not its value then validates.
    let place = s
        .profile
        .windows
        .get(window)
        .and_then(|w| w.fields.get(field))
        .and_then(|entry| {
            let located = locate(&s.image, &s.profile, window, &read).ok()?;
            let runs = located.runs(&s.image, &s.profile);
            let (label, value) =
                locate_field(&s.image, &runs, entry, &s.profile.read, &read).ok()?;
            Some((located.window.rect, label.rect, value.rect))
        });
    if let Some((window_rect, label, value)) = place {
        println!("window: {}", show(window_rect));
        println!(
            "label:  {} reads {}",
            show(label),
            reading(&s.image, label, &s.profile, &read)
        );
        println!(
            "value:  {} reads {}",
            show(value),
            reading(&s.image, value, &s.profile, &read)
        );
        s.overlay.rect(window_rect, GREEN);
        s.overlay.around(label, YELLOW);
        s.overlay.around(value, CYAN);
    }
    match read_window_field(&s.image, &s.profile, window, field, &read) {
        Ok(got) => println!(
            "read: {:?} (number {:?}), raw {:?}, confidence {:.2}",
            got.parsed.text, got.parsed.number, got.raw, got.confidence
        ),
        Err(refusal) => println!("refused: {refusal}"),
    }
    println!("overlay: green the window, yellow the label, cyan the value");
    s.overlay.save()
}

fn cmd_table(mut s: Session) -> Result<(), String> {
    if s.args.option("window").is_none() {
        return Err("table needs --window <kind>".into());
    }
    let read = recogniser();
    let read = Remembering::new(&*read);
    let panel = Panel {
        rgb: s.profile.chrome.panel_rgb,
        tolerance: s.profile.chrome.panel_tolerance,
    };
    for window in chosen_windows(&s, &read)? {
        let area = window.located.window.rect;
        println!("window {}", show(area));
        let rules = row_rules(&s.image, area, panel, &s.profile.rules);
        println!("{} row rules (first row, last row):", rules.len());
        for (first, last) in &rules {
            println!("  y {first}..{last}");
            for y in *first..=*last {
                s.overlay.row(y, area.x0, area.x1, MAGENTA);
            }
        }
        let bands = rows(&s.image, area, panel, &s.profile.rules);
        println!(
            "{} bands of a plausible row height, each with the upright rules that cross it:",
            bands.len()
        );
        // A table's column dividers run the height of each of its rows;
        // a band that is not a table's row has only the window's sides.
        for (top, bottom) in &bands {
            let band = Rect::new(area.x0, *top, area.x1, *bottom);
            let dividers = column_rules(&s.image, band, &s.profile.rules);
            println!(
                "  y {top}..{bottom} ({} px): upright rules at x {dividers:?}",
                bottom - top + 1
            );
            for x in dividers {
                s.overlay.column(x, *top, *bottom, CYAN);
            }
        }
    }
    println!("overlay: magenta row rules, cyan column dividers");
    s.overlay.save()
}

/// Every name the bundled planet maps give a planet.
fn planet_names(maps: &PlanetMapStore) -> Vec<String> {
    let mut names = Vec::new();
    for record in maps.records() {
        names.push(record.name.clone());
        names.extend(record.technical_name.clone());
    }
    names
}

/// The coordinate range of the planet named, from the bundled maps; any
/// coordinate at all when no planet is known, with a line saying so.
fn bounds_for(maps: &PlanetMapStore, planet: Option<&str>) -> CoordinateBounds {
    let bounds = planet
        .and_then(|name| maps.record(name))
        .and_then(|record| record.calibration.as_ref())
        .map(|calibration| calibration.bounds);
    match (planet, bounds) {
        (Some(name), Some(b)) => {
            println!(
                "bounds: {name}, lon {}..{}, lat {}..{}",
                b.lon_min, b.lon_max, b.lat_min, b.lat_max
            );
            CoordinateBounds {
                lon: (b.lon_min, b.lon_max),
                lat: (b.lat_min, b.lat_max),
            }
        }
        _ => {
            println!(
                "bounds: none ({}); pass --planet <name> to hold the values to a planet's",
                planet.map_or("no planet known".to_string(), |name| format!(
                    "the bundled maps do not calibrate {name:?}"
                ))
            );
            CoordinateBounds {
                lon: (i64::MIN, i64::MAX),
                lat: (i64::MIN, i64::MAX),
            }
        }
    }
}

fn load_maps() -> Result<PlanetMapStore, String> {
    PlanetMapStore::new(&resources().join("maps")).map_err(|error| format!("planet maps: {error}"))
}

fn cmd_radar(mut s: Session) -> Result<(), String> {
    let read = recogniser();
    let read = Remembering::new(&*read);
    let Some(radar) = find_radar(&s.image, &s.profile) else {
        println!("no radar is in view: it is hidden, covered by a window, or off the screen");
        return s.overlay.save();
    };
    println!(
        "radar: centre ({:.1}, {:.1}), radius {:.1} px",
        radar.x, radar.y, radar.radius
    );
    s.overlay.circle((radar.x, radar.y), radar.radius, GREEN);
    s.overlay
        .rect(radar.neighbourhood(&s.profile.radar), MAGENTA);
    let maps = load_maps()?;
    let names = planet_names(&maps);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let label = read_planet_label(&s.image, &radar, &s.profile, &names, &read);
    println!("planet name over the radar: {label:?}");
    let planet = s.args.option("planet").map(String::from).or(label);
    let bounds = bounds_for(&maps, planet.as_deref());
    match read_coordinates(&s.image, &s.profile, &bounds, &read) {
        Ok(coordinates) => {
            for (name, value) in [("lon", &coordinates.lon), ("lat", &coordinates.lat)] {
                match value {
                    Ok(v) => {
                        println!(
                            "{name}: {:?} = {} (raw {:?}) at {}",
                            v.text,
                            v.value,
                            v.raw,
                            show(v.rect)
                        );
                        s.overlay.around(v.rect, CYAN);
                    }
                    Err(refusal) => println!("{name}: refused: {refusal}"),
                }
            }
        }
        Err(refusal) => println!("coordinates: refused: {refusal}"),
    }
    match find_rings(&s.image, &radar, &s.profile) {
        Some(rings) => {
            println!(
                "rings: {} at radii {:?} px ({:?} of the disc's radius)",
                rings.len(),
                rings
                    .iter()
                    .map(|r| (r * 10.0).round() / 10.0)
                    .collect::<Vec<_>>(),
                rings
                    .iter()
                    .map(|r| (r / radar.radius * 1000.0).round() / 1000.0)
                    .collect::<Vec<_>>()
            );
            for ring in rings {
                s.overlay.circle((radar.x, radar.y), ring, YELLOW);
            }
        }
        None => println!(
            "rings: not told (too little of a circle is in view, or a ring lies at an end of the span searched)"
        ),
    }
    match ring_scale(&s.image, &radar, &s.profile, &read) {
        Some(scale) => {
            println!("ring labels as read: {:?}", scale.labels);
            for (radius, metres) in &scale.rings {
                println!("  {metres} m at {radius:.1} px");
            }
            for radius in &scale.unlabelled {
                println!("  no label read at {radius:.1} px");
            }
            println!(
                "scale: {:.4} m/px; the disc's edge is {:.1} m from the player",
                scale.metres_per_pixel, scale.edge_range_m
            );
        }
        None => println!(
            "scale: none (the distances are hidden or did not read, or they disagree with the rings)"
        ),
    }
    println!(
        "overlay: green the disc, yellow rings, cyan coordinates, magenta everything the reader looks at"
    );
    s.overlay.save()
}

fn cmd_plates(mut s: Session) -> Result<(), String> {
    let read = recogniser();
    let plates = find_plates(&s.image, &s.profile);
    println!("{} plates, top to bottom", plates.len());
    for plate in &plates {
        let scale = &s.profile.plates.scales[plate.scale];
        match plate.kind {
            PlateKind::Locked { fill } => println!(
                "  bar {} at scale {} (bar {:?}): LOCKED, fill {fill:.3}",
                show(plate.bar),
                plate.scale,
                scale.bar
            ),
            PlateKind::Unlocked => println!(
                "  bar {} at scale {} (bar {:?}): not locked",
                show(plate.bar),
                plate.scale,
                scale.bar
            ),
        }
        s.overlay.rect(
            plate.bar.padded(scale.frame_width + scale.bevel_rows),
            if plate.is_locked() { GREEN } else { YELLOW },
        );
    }
    let mut scratch = PlateScratch::default();
    let Some(locked) = find_locked_plate(&s.image, &s.profile, &mut scratch) else {
        println!("no plate is locked, or the lock cannot be told");
        println!("overlay: green the locked plate, yellow the others");
        return s.overlay.save();
    };
    match plate_line(&s.image, &locked, &[], &s.profile) {
        Ok(line) => {
            println!("line over the locked bar: {}", show(line.rect));
            println!(
                "  level piece {} reads {}",
                show(line.level),
                reading(&s.image, line.level, &s.profile, &*read)
            );
            s.overlay.around(line.level, CYAN);
            match line.name {
                Some(name) => {
                    println!(
                        "  name piece  {} reads {}",
                        show(name),
                        reading(&s.image, name, &s.profile, &*read)
                    );
                    s.overlay.around(name, MAGENTA);
                }
                None => println!("  the line is one piece: no name stands after the level"),
            }
        }
        Err(refusal) => println!("line over the locked bar: refused: {refusal}"),
    }
    if let Some(plate) = read_locked_plate(&s.image, &s.profile, &*read) {
        match &plate.level {
            Ok(level) => println!("level: {level}"),
            Err(refusal) => println!("level: refused: {refusal}"),
        }
        match &plate.name {
            Ok(name) => {
                println!("name: {name:?}");
                let store = GameDataStore::new(&resources().join("snapshot"))
                    .map_err(|error| format!("the bundled game data: {error}"))?;
                let catalogue = MobCatalogue::from_store(&store, Vec::new(), &s.profile.mob_names);
                println!(
                    "resolved against the bundled creatures: {:?}",
                    resolve(
                        name,
                        plate.level.as_ref().ok().copied(),
                        &catalogue,
                        &s.profile.mob_names
                    )
                );
            }
            Err(refusal) => println!("name: refused: {refusal}"),
        }
    }
    println!(
        "overlay: green the locked plate, yellow the others, cyan the level's piece, magenta the name's"
    );
    s.overlay.save()
}

/// The profile file as JSON, for printing entries in its own shape.
fn profile_json(args: &Args) -> Result<Value, String> {
    let path = profile_path(args);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn cmd_measure_plate(mut s: Session) -> Result<(), String> {
    let measured = measure_plates(&s.image, &s.profile);
    if measured.is_empty() {
        println!(
            "no solid plate was measured: lock on to a creature so its plate is drawn solid, \
             clear of windows, and capture again"
        );
        return s.overlay.save();
    }
    let entries = profile_json(s.args)?["plates"]["scales"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for plate in &measured {
        let (w, h) = (plate.bar.width(), plate.bar.height());
        println!(
            "solid plate: bar {}, frame {} px, bevel {} rows, fill {:.3}, {} pointer pixels{}",
            show(plate.bar),
            plate.frame_width,
            plate.bevel_rows,
            plate.fill,
            plate.pointer_pixels,
            if plate.pointer_pixels == 0 {
                " (not the locked plate)"
            } else {
                ""
            }
        );
        s.overlay.rect(
            plate.bar.padded(plate.frame_width + plate.bevel_rows),
            GREEN,
        );
        s.overlay.rect(plate.bar, CYAN);
        let listed = s.profile.plates.scales.iter().position(|scale| {
            scale.bar == [w, h]
                && scale.frame_width == plate.frame_width
                && scale.bevel_rows == plate.bevel_rows
        });
        if let Some(index) = listed {
            let floor = s.profile.plates.scales[index].pointer_pixels;
            println!(
                "  the profile lists this scale as plates.scales[{index}] (pointer floor {floor})"
            );
            continue;
        }
        // The nearest listed scale lends what a capture does not measure.
        let nearest =
            (0..entries.len()).min_by_key(|i| (s.profile.plates.scales[*i].bar[0] - w).abs());
        let Some(nearest) = nearest else {
            println!("  the profile lists no scale to carry the unmeasured entries from");
            continue;
        };
        let mut entry = entries[nearest].clone();
        entry["bar"] = json!([w, h]);
        entry["frame_width"] = json!(plate.frame_width);
        entry["bevel_rows"] = json!(plate.bevel_rows);
        let factor = w as f64 / s.profile.plates.scales[nearest].bar[0] as f64;
        let floor = s.profile.plates.scales[nearest].pointer_pixels as f64 * factor * factor;
        entry["pointer_pixels"] = json!(floor.floor().max(1.0) as u64);
        println!(
            "  not a scale the profile lists. A plates.scales[] entry for it: bar, frame_width \
             and bevel_rows are measured here; pointer_pixels is scales[{nearest}]'s floor scaled \
             by area ({} pointer pixels were seen); width_tolerance, min_rows, lettering and \
             stroke are carried from scales[{nearest}] ({factor:.3} times its size) and are to \
             be checked with `plates` on captures at this scale:",
            plate.pointer_pixels
        );
        println!("{}", pretty(&entry));
    }
    println!("overlay: green the plate's outer edge, cyan its bar");
    s.overlay.save()
}

fn cmd_sale_queue(s: Session) -> Result<(), String> {
    let read = recogniser();
    let store = GameDataStore::new(&resources().join("snapshot"))
        .map_err(|error| format!("the bundled game data: {error}"))?;
    let catalogue = ItemCatalogue::from_store(&store);
    match read_sale_queue(&s.image, &s.profile, &catalogue, &*read) {
        Ok(queue) => {
            println!(
                "count: {:?}; {} rows read, {} unread, {} partly in view",
                queue.count,
                queue.rows.len(),
                queue.unread.len(),
                queue.partial
            );
            for (index, row) in queue.rows.iter().enumerate() {
                println!("row {index}: {:?}", row.name);
                println!(
                    "  tt {:?}; quantity {:?} ({:?}, stack {:?}); opening bid {:?} ({:?})",
                    row.tt.text,
                    row.quantity.text,
                    row.quantity.setting,
                    row.stack,
                    row.opening_bid.text,
                    row.opening_bid.setting
                );
                println!(
                    "  buyout {:?}; days {:?} ({:?}, allowed {:?}); fee {:?} (fixed by the fee model: {}); markup {:?} ({:?})",
                    row.buyout,
                    row.days.text,
                    row.days.setting,
                    row.days_allowed,
                    row.fee.text,
                    row.fee_fixed,
                    row.markup.text,
                    row.markup.kind
                );
            }
            for (index, row) in queue.unread.iter().enumerate() {
                println!("unread {index}: name {:?}", row.raw_name);
                for refusal in &row.refusals {
                    println!("  refused: {refusal}");
                }
            }
        }
        Err(refusal) => println!("refused: {refusal}"),
    }
    s.overlay.save()
}

fn cmd_skills(s: Session) -> Result<(), String> {
    let read = recogniser();
    let vocabulary = replay::skill_vocabulary();
    match read_skills_page(&s.image, &s.profile, &vocabulary, &*read) {
        Ok(page) => {
            println!(
                "category {:?}, total {:?}, page {:?}; whole {}, overflows {}, {} partly in view",
                page.category, page.total, page.page, page.whole, page.overflows, page.partial
            );
            for (index, row) in page.rows.iter().enumerate() {
                println!(
                    "row {index}: {:?} (raw {:?}) = {} ({})",
                    row.name, row.raw_name, row.points_text, row.points
                );
            }
            for (index, row) in page.unread.iter().enumerate() {
                println!(
                    "unread {index}: name {:?}, points {:?}",
                    row.raw_name, row.raw_points
                );
            }
        }
        Err(refusal) => println!("refused: {refusal}"),
    }
    s.overlay.save()
}

fn cmd_terminal(s: Session) -> Result<(), String> {
    let read = recogniser();
    let profile = Arc::new(s.profile.clone());
    for (name, terminal) in [("repair", Terminal::Repair), ("trade", Terminal::Trade)] {
        let image = s.image.clone();
        let service = RepairOcrService::new(RepairProviders {
            frame: Arc::new(move |_| Ok(GameFrame::whole(image.clone(), (0, 0)))),
            profile: Some(profile.clone()),
            read_text: Some(frame_reader(read.clone())),
        });
        let got = match terminal {
            Terminal::Repair => service.scan_repair_cost(),
            Terminal::Trade => service.scan_trade_terminal_value(),
        };
        match got {
            Ok(got) => println!(
                "{name} terminal ({}.{}): {:?} = {} PED (raw {:?})",
                terminal.window(),
                terminal.field(),
                got.text,
                got.value_ped,
                got.raw_text
            ),
            Err(refusal) => println!(
                "{name} terminal ({}.{}): refused: {refusal}",
                terminal.window(),
                terminal.field()
            ),
        }
    }
    s.overlay.save()
}

fn cmd_measure_chrome(mut s: Session) -> Result<(), String> {
    let depth = s.args.number("depth")?.unwrap_or(DEFAULT_DEPTH);
    let rect = match s.args.rect()? {
        Some(rect) => rect,
        None => {
            let found = find_windows(&s.image, &s.profile.chrome);
            let front = found.first().ok_or(
                "no window was found with this profile; give the front-most window's outer \
                 rectangle as --rect x0,y0,x1,y1 (read it off the capture in an image viewer)",
            )?;
            println!("{}", describe_window(0, front, &s.profile));
            front.rect
        }
    };
    let chrome = &s.profile.chrome;
    let Some(measured) = measure_chrome(&s.image, rect, chrome, depth) else {
        return Err(format!("{} lies off the frame", show(rect)));
    };
    s.overlay.rect(rect, GREEN);
    let names = ["left", "top", "right", "bottom"];
    let mut widths: Vec<u8> = Vec::new();
    let (mut lo, mut hi, mut max_chroma) = (u8::MAX, 0u8, 0u8);
    let mut step: Option<u8> = None;
    for (name, ridge) in names.iter().zip(&measured.sides) {
        match ridge {
            Some(r) => {
                println!(
                    "{name}: ridge {} px along {:.0}% of the side, brightness {}..{}, chroma up to {}, step {:?}; the profile's limits admit {:.0}% of the side (the finder needs {:.0}%)",
                    r.thickness,
                    r.support * 100.0,
                    r.lum[0],
                    r.lum[1],
                    r.max_chroma,
                    r.step,
                    r.admitted * 100.0,
                    chrome.side_support * 100.0
                );
                if !widths.contains(&r.thickness) {
                    widths.push(r.thickness);
                }
                lo = lo.min(r.lum[0]);
                hi = hi.max(r.lum[1]);
                max_chroma = max_chroma.max(r.max_chroma);
                if let Some(rise) = r.step {
                    step = Some(step.map_or(rise, |least| least.min(rise)));
                }
            }
            None => println!("{name}: no ridge seen (the side is covered, off the frame, or the rectangle is not the window's outer edge)"),
        }
    }
    let mut fragment = json!({
        "panel_rgb": measured.panel_rgb,
        "panel_tolerance": measured.panel_spread,
    });
    if !widths.is_empty() {
        fragment["ridge_lum"] = json!([lo, hi]);
        fragment["ridge_max_chroma"] = json!(max_chroma);
        fragment["ridge_widths"] = json!(widths);
    }
    if let Some(step) = step {
        fragment["ridge_step"] = json!(step);
    }
    match measured.marker {
        Some(marker) => {
            let box_ = marker.rect;
            fragment["marker"] = json!({
                "search": [box_.x0, box_.y0, box_.x1, box_.y1],
                "min_pixels": marker.pixels,
                "max_width": box_.width(),
                "min_height": box_.height(),
            });
            s.overlay.rect(box_.offset(rect.x0, rect.y0), YELLOW);
        }
        None => println!(
            "marker: no pixel of the marker's colour (min_red {}, min_red_over_blue {}) in the search box {:?}",
            chrome.marker.min_red, chrome.marker.min_red_over_blue, chrome.marker.search
        ),
    }
    println!("measured on this window, in the profile's chrome entry names:");
    println!("{}", pretty(&fragment));
    println!("the profile has:");
    println!(
        "{}",
        pretty(&json!({
            "panel_rgb": chrome.panel_rgb,
            "panel_tolerance": chrome.panel_tolerance,
            "ridge_lum": chrome.ridge_lum,
            "ridge_max_chroma": chrome.ridge_max_chroma,
            "ridge_widths": chrome.ridge_widths,
            "ridge_step": chrome.ridge_step,
            "marker": {
                "search": chrome.marker.search,
                "min_pixels": chrome.marker.min_pixels,
                "max_width": chrome.marker.max_width,
                "min_height": chrome.marker.min_height,
            },
        }))
    );
    println!(
        "the measurements are the extremes one window shows; the profile's entries are limits. \
         The ridge's limits (ridge_lum, ridge_max_chroma, ridge_step) need not admit every pixel: \
         they must admit the finder's share of each side, on every capture, and no more than \
         that needs, since looser limits take rule lines for ridges. The marker's search box must \
         hold the box measured, with max_width no smaller and min_pixels and min_height no \
         larger; panel_tolerance no smaller than measured"
    );
    println!("overlay: green the rectangle measured, yellow the marker");
    s.overlay.save()
}

/// The numeral metrics a subcommand measures against: the labelled
/// fields', or the sell queue's with `--numerals sale-queue`.
fn numeral_metrics(args: &Args, profile: &UiProfile) -> Result<NumeralMetrics, String> {
    match args.option("numerals") {
        None | Some("read") => Ok(profile.read.numerals.clone()),
        Some("sale-queue") => profile
            .windows
            .values()
            .find_map(|w| w.sale_queue.as_ref())
            .map(|queue| queue.numerals.clone())
            .ok_or_else(|| "the profile describes no sell queue".to_string()),
        Some(other) => Err(format!(
            "--numerals {other:?} is not `read` or `sale-queue`"
        )),
    }
}

/// How many of each glyph the metrics measure `text` has, in the order
/// of [`TERMS`], the constant term first.
fn glyph_counts(text: &str) -> Option<[f64; TERMS.len()]> {
    let mut counts = [0.0; TERMS.len()];
    counts[0] = 1.0;
    let mut rest = text;
    if let Some(after) = rest.strip_prefix("TT+") {
        counts[6] = 1.0;
        rest = after;
    }
    if let Some(before) = rest.strip_suffix(" PED") {
        counts[5] = 1.0;
        rest = before;
    } else if let Some(before) = rest.strip_suffix('%') {
        counts[7] = 1.0;
        rest = before;
    }
    for c in rest.chars() {
        let term = match c {
            '1' => 2,
            '0' | '2'..='9' => 1,
            ',' => 3,
            '.' => 4,
            '(' | ')' => 8,
            _ => return None,
        };
        counts[term] += 1.0;
    }
    Some(counts)
}

/// The numeral metrics' entries a fit can set, by the profile's names.
const TERMS: [&str; 9] = [
    "bias",
    "digit",
    "one",
    "comma",
    "period",
    "ped_suffix",
    "above_tt_prefix",
    "percent",
    "bracket",
];

fn term_values(metrics: &NumeralMetrics) -> [Option<f32>; TERMS.len()] {
    [
        Some(metrics.bias),
        Some(metrics.digit),
        Some(metrics.one),
        Some(metrics.comma),
        Some(metrics.period),
        Some(metrics.ped_suffix),
        metrics.above_tt_prefix,
        metrics.percent,
        metrics.bracket,
    ]
}

fn cmd_measure_numerals(mut s: Session) -> Result<(), String> {
    let metrics = numeral_metrics(s.args, &s.profile)?;
    let (rect, expected) = match s.args.rect()? {
        Some(rect) => (rect, s.args.word(2, "the expected text")?),
        None => {
            let window = s.args.word(2, "the window kind (or --rect)")?;
            let field = s.args.word(3, "the field")?;
            let expected = s.args.word(4, "the expected text")?;
            let entry = s
                .profile
                .windows
                .get(window)
                .and_then(|w| w.fields.get(field))
                .ok_or_else(|| format!("the profile describes no field {window}.{field}"))?;
            let read = recogniser();
            let read = Remembering::new(&*read);
            let located = locate(&s.image, &s.profile, window, &read)
                .map_err(|refusal| format!("{window}: {refusal}"))?;
            let runs = located.runs(&s.image, &s.profile);
            let (label, value) = locate_field(&s.image, &runs, entry, &s.profile.read, &read)
                .map_err(|refusal| format!("{window}.{field}: {refusal}"))?;
            println!(
                "label {} ; value {} reads {}",
                show(label.rect),
                show(value.rect),
                reading(&s.image, value.rect, &s.profile, &read)
            );
            s.overlay.around(label.rect, YELLOW);
            (value.rect, expected)
        }
    };
    s.overlay.around(rect, CYAN);
    let Some((width, height)) = ink_size(&s.image, rect, &metrics) else {
        return Err(format!("{} holds no ink", show(rect)));
    };
    let ratio = width as f32 / height as f32;
    println!("ink: {width} px wide, digits {height} px tall: {ratio:.3} digit heights wide");
    match expected_width(expected, &metrics) {
        Some(predicted) => {
            let residual = ratio - predicted;
            println!(
                "the metrics predict {predicted:.3} for {expected:?}: residual {residual:+.3} (tolerance {:.3}): {}",
                metrics.tolerance,
                if residual.abs() <= metrics.tolerance {
                    "agrees"
                } else {
                    "REFUSED as the wrong width"
                }
            );
        }
        None => println!(
            "the metrics do not cover every character of {expected:?}, so its width is not checked"
        ),
    }
    if glyph_counts(expected).is_none() {
        println!("{expected:?} holds a character no numeral metric measures: not a sample");
    } else {
        println!("sample line for fit-numerals (append it to a samples file):");
        println!("{ratio:.4}\t{expected}");
    }
    println!("overlay: yellow the label, cyan the figure measured");
    s.overlay.save()
}

/// Solve `a x = b` for a small symmetric system by elimination with
/// pivoting; `None` when the system cannot tell its unknowns apart.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|i, j| a[*i][col].abs().total_cmp(&a[*j][col].abs()))?;
        if a[pivot][col].abs() < 1e-9 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (upper, lower) = a.split_at_mut(col + 1);
        let pivot_row = &upper[col];
        for (offset, row) in lower.iter_mut().enumerate() {
            let factor = row[col] / pivot_row[col];
            for (value, above) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                *value -= factor * above;
            }
            b[col + 1 + offset] -= factor * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let known: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - known) / a[row][row];
    }
    Some(x)
}

/// Fit the numeral metrics to samples by least squares. Each sample is a
/// figure's measured width in digit heights and its true text; the width
/// the metrics predict is a constant plus so much per glyph, so the
/// advances that best fit every sample solve a small linear system.
fn cmd_fit_numerals(args: &Args) -> Result<(), String> {
    let path = args.word(1, "the samples file")?;
    let profile = load_profile(args)?;
    let metrics = numeral_metrics(args, &profile)?;
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let mut samples: Vec<(f64, [f64; TERMS.len()], String)> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (ratio, figure) = line
            .split_once('\t')
            .ok_or_else(|| format!("{line:?} is not `<width in digit heights><TAB><text>`"))?;
        let ratio: f64 = ratio
            .trim()
            .parse()
            .map_err(|_| format!("{ratio:?} is not a width"))?;
        let counts = glyph_counts(figure)
            .ok_or_else(|| format!("{figure:?} holds a character no numeral metric measures"))?;
        samples.push((ratio, counts, figure.to_string()));
    }
    // The terms the samples exercise are fitted; the rest keep the
    // profile's values.
    let fitted: Vec<usize> = (0..TERMS.len())
        .filter(|t| samples.iter().any(|(_, counts, _)| counts[*t] > 0.0))
        .collect();
    if samples.len() <= fitted.len() {
        return Err(format!(
            "{} samples cannot fit {} entries ({}); measure more figures",
            samples.len(),
            fitted.len(),
            fitted
                .iter()
                .map(|t| TERMS[*t])
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let n = fitted.len();
    let mut normal = vec![vec![0.0; n]; n];
    let mut rhs = vec![0.0; n];
    for (ratio, counts, _) in &samples {
        for (i, ti) in fitted.iter().enumerate() {
            rhs[i] += counts[*ti] * ratio;
            for (j, tj) in fitted.iter().enumerate() {
                normal[i][j] += counts[*ti] * counts[*tj];
            }
        }
    }
    let solved = solve(normal, rhs).ok_or(
        "the samples cannot tell the entries apart: every sample has the same number of some \
         glyph as of another, or the same unit; add figures that differ in how many digits, \
         ones, commas and stops they hold, and some with and some without their unit",
    )?;
    let current = term_values(&metrics);
    let mut entry = serde_json::Map::new();
    for (i, t) in fitted.iter().enumerate() {
        let value = (solved[i] * 1000.0).round() / 1000.0;
        println!(
            "{}: fitted {value:.3} (the profile has {})",
            TERMS[*t],
            current[*t].map_or("none".to_string(), |v| format!("{v:.3}"))
        );
        entry.insert(TERMS[*t].to_string(), json!(value));
    }
    let mut worst: f64 = 0.0;
    for (ratio, counts, figure) in &samples {
        let predicted: f64 = fitted
            .iter()
            .enumerate()
            .map(|(i, t)| solved[i] * counts[*t])
            .sum();
        let residual = ratio - predicted;
        worst = worst.max(residual.abs());
        println!(
            "  {figure:?}: measured {ratio:.3}, fitted {predicted:.3}, residual {residual:+.3}"
        );
    }
    println!(
        "largest residual {worst:.3}; the profile's tolerance is {:.3}. A reading one digit short \
         is narrower by a digit's advance, so the tolerance must stay under half the narrowest \
         glyph that can be lost",
        metrics.tolerance
    );
    println!("the numerals entries fitted, in the profile's names:");
    println!("{}", pretty(&Value::Object(entry)));
    Ok(())
}

/// `image` scaled so it is `SMALLER_WIDTH` wide, keeping its shape; the
/// image itself when it is no wider than that.
fn smaller(image: &BgrImage) -> Option<BgrImage> {
    if image.w <= SMALLER_WIDTH {
        return None;
    }
    let h = (image.h * SMALLER_WIDTH + image.w / 2) / image.w;
    Some(BgrImage {
        data: resize_bilinear(&image.data, image.h, image.w, h, SMALLER_WIDTH),
        h,
        w: SMALLER_WIDTH,
    })
}

fn cmd_timing(s: Session) -> Result<(), String> {
    let repeat: u32 = s.args.number("repeat")?.unwrap_or(5).max(1);
    if cfg!(debug_assertions) {
        println!(
            "this is an unoptimised build, many times slower than the application: build with \
             --release for figures that mean anything"
        );
    }
    let read = recogniser();
    let maps = load_maps()?;
    let bounds = bounds_for(&maps, s.args.option("planet"));
    let scaled = smaller(&s.image);
    let mut frames = vec![("as captured", &s.image)];
    if let Some(scaled) = &scaled {
        frames.push(("scaled down", scaled));
    }
    let time = |f: &mut dyn FnMut()| {
        let started = Instant::now();
        for _ in 0..repeat {
            f();
        }
        started.elapsed().as_secs_f64() * 1000.0 / f64::from(repeat)
    };
    for (what, image) in frames {
        println!("{what}, {}x{}, mean of {repeat} runs:", image.w, image.h);
        let mut scratch = PlateScratch::default();
        let mut locked = None;
        let ms = time(&mut || {
            locked = std::hint::black_box(find_locked_plate(image, &s.profile, &mut scratch));
        });
        println!(
            "  locked plate search: {ms:.1} ms ({})",
            if locked.is_some() {
                "a locked plate was found"
            } else {
                "no locked plate"
            }
        );
        let mut plate = None;
        let ms = time(&mut || {
            plate = std::hint::black_box(read_locked_plate(image, &s.profile, &*read));
        });
        println!(
            "  whole plate read:    {ms:.1} ms ({})",
            match &plate {
                Some(p) => format!(
                    "level {}, name {}",
                    if p.level.is_ok() { "read" } else { "refused" },
                    if p.name.is_ok() { "read" } else { "refused" }
                ),
                None => "no locked plate".to_string(),
            }
        );
        let mut radar = None;
        let ms = time(&mut || {
            radar = std::hint::black_box(find_radar(image, &s.profile));
        });
        println!(
            "  radar search:        {ms:.1} ms ({})",
            if radar.is_some() { "found" } else { "no radar" }
        );
        let mut coordinates = None;
        let ms = time(&mut || {
            coordinates =
                std::hint::black_box(read_coordinates(image, &s.profile, &bounds, &*read).ok());
        });
        println!(
            "  radar read:          {ms:.1} ms (search and coordinates: {})",
            match &coordinates {
                Some(c) => format!(
                    "lon {}, lat {}",
                    if c.lon.is_ok() { "read" } else { "refused" },
                    if c.lat.is_ok() { "read" } else { "refused" }
                ),
                None => "no radar".to_string(),
            }
        );
        let mut windows = 0;
        let ms = time(&mut || {
            windows = std::hint::black_box(find_windows(image, &s.profile.chrome)).len();
        });
        println!("  window search:       {ms:.1} ms ({windows} windows)");
    }
    println!(
        "a frame scaled down stands in for a smaller screen's pixel count only: scaling blurs \
         the interface, so reads that find their target as captured may not find it there"
    );
    s.overlay.save()
}

fn cmd_replay(args: &Args) -> Result<bool, String> {
    let dir = args.word(1, "the corpus directory")?;
    let profile = load_profile(args)?;
    let held_out = args.option("held-out").is_some();
    let corpus = Corpus::open(Path::new(dir), held_out)?;
    println!(
        "replaying {} frames ({}) with profile {} ({})",
        corpus.frames().len(),
        if held_out {
            "the held-out frames alone"
        } else {
            "held-out frames skipped"
        },
        profile_path(args).display(),
        profile.client_interface
    );
    let mut outcomes: Vec<(&str, Outcome)> = Vec::new();
    let all = Shard(None);
    outcomes.push(("windows", replay::windows(&corpus, &profile, all)));
    let mut unavailable = None;
    match load_recogniser() {
        Ok(engine) => {
            let r = Replay {
                corpus,
                profile,
                read: engine,
            };
            outcomes.push(("fields", replay::field(&r, &REPAIR_COST)));
            outcomes.push(("fields", replay::field(&r, &TRADE_TOTAL)));
            outcomes.push(("terminals", replay::terminal(&r, &REPAIR_COST)));
            outcomes.push(("terminals", replay::terminal(&r, &TRADE_TOTAL)));
            outcomes.push(("skills", replay::skills(&r, all)));
            outcomes.push(("radar", replay::coordinates(&r, all)));
            outcomes.push(("radar", replay::coordinate_digit_loss(&r, all)));
            outcomes.push(("sale queue", replay::sale_queue(&r, all)));
            outcomes.push(("sale queue", replay::sale_queue_digit_loss(&r, all)));
            outcomes.push(("plates", replay::plates(&r, all)));
        }
        Err(why) => unavailable = Some(why),
    }
    let exact: usize = outcomes.iter().map(|(_, outcome)| outcome.exact).sum();
    let failures: Vec<String> = outcomes
        .iter()
        .flat_map(|(surface, outcome)| {
            outcome
                .failures
                .iter()
                .map(move |line| format!("[{surface}] {line}"))
        })
        .collect();
    println!(
        "replay: {exact} values and findings as labelled over every surface, {} failures",
        failures.len()
    );
    for line in &failures {
        println!("FAILED {line}");
    }
    if let Some(why) = &unavailable {
        println!("only the windows were replayed: readings are unavailable: {why}");
    }
    Ok(failures.is_empty() && unavailable.is_none())
}

fn usage() -> String {
    "usage: ui_measure <subcommand> ...\n  \
     windows|runs|field|table|radar|plates|sale-queue|skills|terminal|measure-chrome|\
     measure-plate|measure-numerals|timing <frame.png> ... [--profile <path>] [--overlay <out.png> \
     [--overlay-area x0,y0,x1,y1]]\n  \
     fit-numerals <samples file> [--profile <path>] [--numerals sale-queue]\n  \
     replay <corpus dir> [--profile <path>] [--held-out]\n\
     see the head of examples/ui_measure.rs for each subcommand's arguments"
        .to_string()
}

fn run(args: &Args) -> Result<bool, String> {
    let subcommand = args.word(0, "a subcommand").map_err(|_| usage())?;
    match subcommand {
        "replay" => return cmd_replay(args),
        "fit-numerals" => return cmd_fit_numerals(args).map(|()| true),
        _ => {}
    }
    let frame: fn(Session) -> Result<(), String> = match subcommand {
        "windows" => cmd_windows,
        "runs" => cmd_runs,
        "field" => cmd_field,
        "table" => cmd_table,
        "radar" => cmd_radar,
        "plates" => cmd_plates,
        "measure-plate" => cmd_measure_plate,
        "sale-queue" => cmd_sale_queue,
        "skills" => cmd_skills,
        "terminal" => cmd_terminal,
        "measure-chrome" => cmd_measure_chrome,
        "measure-numerals" => cmd_measure_numerals,
        "timing" => cmd_timing,
        other => return Err(format!("unknown subcommand {other:?}\n{}", usage())),
    };
    frame(Session::open(args)?).map(|()| true)
}

fn main() -> ExitCode {
    let outcome = Args::parse(std::env::args().skip(1)).and_then(|args| run(&args));
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("ui_measure: {message}");
            ExitCode::from(2)
        }
    }
}
