//! Guided calibration for the docked sale window: record each rectangle
//! by hovering its corners, and write the result into
//! `panel_geometry.json` as an entry the scan presets read back.
//!
//! The panel docks in the game window's bottom-right corner, so every
//! recorded corner converts to an offset from that corner the moment it
//! is taken, and the field rectangles convert to panel-relative extents.
//! Nothing absolute survives the recording, which is what lets the same
//! numbers work at another resolution, on another monitor, and after the
//! window has been moved (including between two samples of one run).
//!
//! ```sh
//! cargo run -p eo-services --features linux-capture --example panel_calibrate
//! cargo run -p eo-services --features linux-capture --example panel_calibrate -- --only quantity,buyout
//! ```
//!
//! Confirmation is a press of Enter in this terminal while the pointer
//! rests on the corner, which asks that the terminal keep keyboard focus
//! while the pointer sits over the game: hover and press, do not click
//! into the game first. The shared keystroke hook is not an option here,
//! being wired on Windows only.

use std::io::Write;

use eo_services::eu_window;
use eo_services::scan_presets::{
    compute_region, CellGeometry, PanelAnchor, ScanPresets, SALE_WINDOW_KEY,
};
use serde_json::Value;

/// The sale window's fields, in the order they are recorded. The item
/// name's rectangle is drawn wide enough for a long name rather than
/// tight to the sample on screen.
const SALE_WINDOW_FIELDS: &[(&str, &str)] = &[
    (
        "landmark",
        "the 'Item Name' LABEL itself, not its value (this is what proves the window is the \
         sale window before any figure is believed)",
    ),
    ("item_name", "item name (draw it wide: long names must fit)"),
    ("quantity", "quantity"),
    ("tt_value", "TT value"),
    ("auction_fee", "auction fee"),
    ("auction_days", "auction days"),
    ("starting_bid", "starting bid"),
    ("buyout", "buyout"),
];

/// A recorded point, as offsets from the game window's bottom-right
/// corner: larger `dx` is further left, larger `dy` is further up.
#[derive(Debug, Clone, Copy)]
struct Corner {
    dx: i64,
    dy: i64,
}

struct Options {
    out: std::path::PathBuf,
    shots: Option<std::path::PathBuf>,
    only: Vec<String>,
}

fn default_out() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../entropia-orme/resources/panel_geometry.json")
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        out: default_out(),
        shots: None,
        only: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => options.out = value?.into(),
            "--shots" => options.shots = Some(value?.into()),
            "--only" => {
                options.only = value?
                    .split(',')
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect()
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    for name in &options.only {
        if !SALE_WINDOW_FIELDS.iter().any(|(field, _)| field == name) {
            return Err(format!("unknown field: {name}"));
        }
    }
    Ok(options)
}

/// What a prompt came back with.
enum Answer {
    Corner(Corner),
    Skip,
    Quit,
}

fn read_line() -> String {
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return "q".to_string();
    }
    line.trim().to_lowercase()
}

/// Prompt until the pointer is somewhere usable, converting the reading
/// to window-relative offsets against the geometry read at that same
/// moment. A pointer outside the game window is refused rather than
/// recorded: off an X surface the position goes stale instead of going
/// missing, so an unchecked reading would look perfectly plausible.
fn prompt_corner(label: &str) -> Answer {
    loop {
        eprint!("    {label}: hover, then Enter  (s = skip, q = quit) > ");
        let _ = std::io::stderr().flush();
        match read_line().as_str() {
            "q" => return Answer::Quit,
            "s" => return Answer::Skip,
            _ => {}
        }
        let Some(handle) = eu_window::find_game_window() else {
            eprintln!("      the game window is no longer there; start it and try again");
            continue;
        };
        let Some((win_x, win_y, win_w, win_h)) = eu_window::get_window_geometry(handle) else {
            eprintln!("      the game window could not be measured; try again");
            continue;
        };
        let Some((x, y)) = eu_window::cursor_position() else {
            eprintln!("      the pointer position could not be read; try again");
            continue;
        };
        if x < win_x || y < win_y || x >= win_x + win_w || y >= win_y + win_h {
            eprintln!("      ({x}, {y}) is outside the game window; not recorded");
            continue;
        }
        let corner = Corner {
            dx: win_x + win_w - x,
            dy: win_y + win_h - y,
        };
        eprintln!(
            "      recorded {}px left and {}px up from the corner",
            corner.dx, corner.dy
        );
        return Answer::Corner(corner);
    }
}

/// Both corners of one rectangle, retried until they enclose an area.
fn prompt_rect(name: &str) -> Option<(Corner, Corner)> {
    loop {
        eprintln!("  {name}");
        let Answer::Corner(top_left) = prompt_corner("top-left") else {
            return None;
        };
        let Answer::Corner(bottom_right) = prompt_corner("bottom-right") else {
            return None;
        };
        if bottom_right.dx < top_left.dx && bottom_right.dy < top_left.dy {
            return Some((top_left, bottom_right));
        }
        eprintln!("      those corners do not enclose an area; recording that rectangle again");
    }
}

/// Read the entry already in the file, if any.
fn existing_entry(path: &std::path::Path, key: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let document: Value = serde_json::from_str(&raw).ok()?;
    document.get(key).cloned()
}

fn merge_entry(path: &std::path::Path, key: &str, entry: Value) -> Result<(), String> {
    let mut document = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|error| format!("{path:?}: {error}"))?,
        Err(_) => Value::Object(serde_json::Map::new()),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| format!("{path:?} is not a JSON object"))?;
    object.insert(key.to_string(), entry);
    let mut text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("{path:?}: {error}"))
}

/// Capture the calibrated panel and each field, so the rectangles can be
/// checked against what they actually contain rather than trusted.
fn write_shots(anchor: &PanelAnchor, dir: &std::path::Path) -> Result<(), String> {
    let handle = eu_window::find_game_window().ok_or("the game window is no longer there")?;
    let geometry = eu_window::get_window_geometry(handle).ok_or("the window cannot be measured")?;
    let (top_left, bottom_right) =
        compute_region(anchor, geometry).ok_or("the calibrated panel has no area")?;
    let (x, y) = (top_left[0], top_left[1]);
    let (w, h) = (bottom_right[0] - x, bottom_right[1] - y);
    let png = eo_services::screen_capture::capture_region_png(x, y, w, h)
        .ok_or("the screen capture returned nothing")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{dir:?}: {error}"))?;
    std::fs::write(dir.join("panel.png"), &png).map_err(|error| error.to_string())?;
    let panel = image::load_from_memory(&png).map_err(|error| error.to_string())?;
    for (name, cell) in &anchor.cells {
        let (cx, cy, cw, ch) = cell.single_rect();
        if cx < 0 || cy < 0 || cx + cw > w || cy + ch > h {
            eprintln!("shots: {name} falls outside the panel; skipped");
            continue;
        }
        let crop = panel.crop_imm(cx as u32, cy as u32, cw as u32, ch as u32);
        crop.save(dir.join(format!("{name}.png")))
            .map_err(|error| format!("{name}: {error}"))?;
    }
    eprintln!("shots: written to {}", dir.display());
    Ok(())
}

fn main() {
    let options = parse_args().unwrap_or_else(|error| {
        eprintln!("calibrate: {error}");
        eprintln!(
            "usage: panel_calibrate [--out <panel_geometry.json>] [--shots <dir>] [--only <fields>]"
        );
        std::process::exit(2);
    });

    if eu_window::find_game_window().is_none() {
        eprintln!("calibrate: the Entropia Universe window was not found; start the game first");
        std::process::exit(1);
    }

    eprintln!("Dock the sale window in the game's bottom-right corner at default interface scale.");
    eprintln!("Keep this terminal focused: hover a corner with the pointer and press Enter here.");
    eprintln!();

    let key = SALE_WINDOW_KEY;
    let previous = existing_entry(&options.out, key);
    let recalibrating = !options.only.is_empty();

    // The panel rect: re-recorded on a full run, reused when only some
    // fields are being redone (their extents are relative to it, so a
    // fresh panel rect would move rectangles nobody asked to move).
    let panel = if recalibrating {
        let previous = previous.as_ref().unwrap_or_else(|| {
            eprintln!("calibrate: --only needs an existing entry to build on; run a full calibration first");
            std::process::exit(1);
        });
        let read = |key: &str| previous.get(key).and_then(Value::as_i64);
        match (
            read("width"),
            read("height"),
            read("right_offset"),
            read("bottom_offset"),
        ) {
            (Some(width), Some(height), Some(right_offset), Some(bottom_offset)) => {
                eprintln!("Reusing the recorded panel rect: {width}x{height} at {right_offset}/{bottom_offset}.");
                (width, height, right_offset, bottom_offset)
            }
            _ => {
                eprintln!(
                    "calibrate: the existing entry has no panel rect; run a full calibration first"
                );
                std::process::exit(1);
            }
        }
    } else {
        let Some((top_left, bottom_right)) = prompt_rect("the whole sale window") else {
            eprintln!("calibrate: stopped without recording anything");
            std::process::exit(1);
        };
        (
            top_left.dx - bottom_right.dx,
            top_left.dy - bottom_right.dy,
            bottom_right.dx,
            bottom_right.dy,
        )
    };
    let (width, height, right_offset, bottom_offset) = panel;
    // The panel's top-left, in the same bottom-right-relative terms, is
    // what the field rectangles measure from.
    let origin = Corner {
        dx: right_offset + width,
        dy: bottom_offset + height,
    };

    // Keep whatever was recorded before for fields this run does not touch.
    let mut cells: Vec<(String, CellGeometry)> = Vec::new();
    if let Some(previous_cells) = previous
        .as_ref()
        .and_then(|entry| entry.get("cells"))
        .and_then(Value::as_object)
    {
        for (name, raw) in previous_cells {
            let read = |key: &str| raw.get(key).and_then(Value::as_i64);
            if let (Some(x_left), Some(x_right), Some(first_y_top), Some(last_y_top), Some(h)) = (
                read("x_left"),
                read("x_right"),
                read("first_y_top"),
                read("last_y_top"),
                read("height"),
            ) {
                cells.push((
                    name.clone(),
                    CellGeometry {
                        x_left,
                        x_right,
                        first_y_top,
                        last_y_top,
                        height: h,
                    },
                ));
            }
        }
    }

    eprintln!();
    for (name, description) in SALE_WINDOW_FIELDS {
        if recalibrating && !options.only.iter().any(|only| only == name) {
            continue;
        }
        let Some((top_left, bottom_right)) = prompt_rect(description) else {
            eprintln!("      {name} skipped");
            continue;
        };
        let cell = CellGeometry {
            x_left: origin.dx - top_left.dx,
            x_right: origin.dx - bottom_right.dx,
            first_y_top: origin.dy - top_left.dy,
            last_y_top: origin.dy - top_left.dy,
            height: top_left.dy - bottom_right.dy,
        };
        if cell.x_left < 0 || cell.first_y_top < 0 || cell.x_right > width {
            eprintln!("      warning: {name} falls outside the panel rectangle");
        }
        cells.retain(|(existing, _)| existing != name);
        cells.push((name.to_string(), cell));
    }

    cells.sort_by(|(a, _), (b, _)| a.cmp(b));
    let anchor = PanelAnchor {
        width,
        height,
        right_offset,
        bottom_offset,
        n_rows: None,
        cells,
    };

    if let Err(error) = merge_entry(&options.out, key, anchor.to_geometry_entry()) {
        eprintln!("calibrate: {error}");
        std::process::exit(1);
    }
    eprintln!();
    eprintln!(
        "Wrote {} fields to {}",
        anchor.cells.len(),
        options.out.display()
    );

    // Read the file back through the loader the app uses, so what was
    // written is confirmed to be what the app will see.
    let presets = ScanPresets::new(&options.out);
    if presets.sale_window != anchor {
        eprintln!("calibrate: the file did not read back as recorded");
        std::process::exit(1);
    }

    if let Some(dir) = options.shots {
        if let Err(error) = write_shots(&anchor, &dir) {
            eprintln!("shots: {error}");
        }
    }
}
