//! The sale-window read against a real captured panel.
//!
//! Every other test of this read supplies its own text, which proves the
//! parsing, the refusals and the gates but says nothing about whether the
//! calibrated rectangles sit over the figures or whether the recogniser
//! can make them out. That question needs real pixels, and the answer has
//! to survive a change to the rectangles, the model, or the parsing,
//! which is what makes it worth a regression floor.
//!
//! The captures are real gameplay screens and stay out of the public
//! tree, so this runs only where `EO_OCR_BENCH_DIR` points at the bench
//! and the ONNX Runtime library is loadable. Everywhere else it skips
//! with its reason stated rather than passing vacuously.
//!
//! The runtime is pinned to the committed dylib, as every test that
//! builds a real session is: left to search, the loader can hang on a
//! foreign-format library rather than report it, which reads as a test
//! that never finishes instead of one that skipped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_services::game_ui::geometry::BgrImage;
use eo_services::ocr_engine::{load_bgr_png, OcrEngine};
use eo_services::sale_window_ocr::{SaleWindowOcrService, SaleWindowProviders};
use eo_services::scan_presets::{ScanPresets, SALE_WINDOW_KEY};
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn model_paths() -> (PathBuf, PathBuf) {
    let assets = repo_root().join("app/src-tauri/entropia-orme/resources/models");
    (
        assets.join("svtrv2_rec.onnx"),
        assets.join("ppocr_keys_v1.txt"),
    )
}

/// The committed ONNX Runtime for this platform, in the dev layout.
fn ort_dylib() -> PathBuf {
    #[cfg(target_os = "linux")]
    let leaf = "app/src-tauri/entropia-orme/resources/ort-linux/libonnxruntime.so";
    #[cfg(not(target_os = "linux"))]
    let leaf = "app/src-tauri/entropia-orme/resources/ort/onnxruntime.dll";
    repo_root().join(leaf)
}

fn geometry_path() -> PathBuf {
    repo_root().join("app/src-tauri/entropia-orme/resources/panel_geometry.json")
}

fn bench_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("EO_OCR_BENCH_DIR")?);
    dir.is_dir().then_some(dir)
}

fn ground_truth(bench: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(bench.join("crops/sale-window/ground_truth.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

#[test]
fn the_calibrated_rectangles_read_a_real_sale_window() {
    let Some(bench) = bench_dir() else {
        eprintln!("EO_OCR_BENCH_DIR unset or not a directory; skipping");
        return;
    };
    let Some(truth) = ground_truth(&bench) else {
        eprintln!("no sale-window ground truth in the bench; skipping");
        return;
    };
    let (model, dict) = model_paths();
    if !model.is_file() {
        eprintln!("recogniser model absent; skipping");
        return;
    }
    if !cfg!(any(windows, target_os = "linux")) {
        eprintln!("no bundled ONNX Runtime on this platform; skipping");
        return;
    }
    let dylib = ort_dylib();
    if !dylib.is_file() {
        eprintln!(
            "bundled ONNX Runtime absent at {}; skipping",
            dylib.display()
        );
        return;
    }
    std::env::set_var("ORT_DYLIB_PATH", &dylib);

    let engine = match OcrEngine::new(&model, &dict) {
        Ok(engine) => Arc::new(engine),
        Err(error) => {
            eprintln!("recogniser did not load ({error}); skipping");
            return;
        }
    };

    let presets = ScanPresets::new(&geometry_path());
    assert!(
        !presets.sale_window.cells.is_empty(),
        "the shipped geometry must carry the {SALE_WINDOW_KEY} calibration"
    );

    let captures = truth["captures"].as_array().expect("captures array");
    assert!(
        !captures.is_empty(),
        "the bench carries no sale-window capture"
    );

    for capture in captures {
        let id = capture["id"].as_str().expect("capture id");
        let relative = capture["panel_screenshot"]
            .as_str()
            .expect("screenshot path");
        let bytes = std::fs::read(bench.join("crops").join(relative))
            .unwrap_or_else(|error| panic!("{id}: panel image unreadable: {error}"));
        let (data, h, w) = load_bgr_png(&bytes).expect("panel image decodes");
        let panel = BgrImage { data, h, w };

        // The whole service over the real anchor and the real recogniser;
        // only the window lookup and the grab are stood in for, since the
        // frame is the one thing a test cannot take for itself.
        let anchor = presets.sale_window.clone();
        let frame = panel.clone();
        let engine_for_read = engine.clone();
        let service = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(move || Some(([0, 0], [w as i64, h as i64]))),
            anchor: Arc::new(move || anchor.clone()),
            capture_region: Arc::new(move |_, _, _, _| Some(frame.clone())),
            read_text: Arc::new(move |crop: &BgrImage| {
                engine_for_read
                    .recognize_bgr(&crop.data, crop.h, crop.w)
                    .ok()
            }),
        });

        let read = service.scan_sale_window();
        assert_eq!(read.get("error"), None, "{id}: {read}");
        assert_eq!(
            read["unread"].as_array().map(Vec::len),
            Some(0),
            "{id}: every calibrated field must read: {read}"
        );

        // The landmark, proven on the same pixels rather than assumed: a
        // panel that has slipped from where it was calibrated must be
        // refused whole, not read field by field off whatever now sits at
        // those offsets. Twelve pixels is smaller than any row.
        let slipped = panel.crop(12, panel.h as i64, 12, panel.w as i64);
        let engine_for_slip = engine.clone();
        let anchor_for_slip = presets.sale_window.clone();
        let (sw, sh) = (slipped.w as i64, slipped.h as i64);
        let displaced = SaleWindowOcrService::new(SaleWindowProviders {
            sale_window_region: Arc::new(move || Some(([0, 0], [sw, sh]))),
            anchor: Arc::new(move || anchor_for_slip.clone()),
            capture_region: Arc::new(move |_, _, _, _| Some(slipped.clone())),
            read_text: Arc::new(move |crop: &BgrImage| {
                engine_for_slip
                    .recognize_bgr(&crop.data, crop.h, crop.w)
                    .ok()
            }),
        });
        let refused = displaced.scan_sale_window();
        assert!(
            refused["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("That is not the sale window")),
            "{id}: a displaced panel must be refused, got {refused}"
        );

        let expected = capture["fields"].as_object().expect("expected fields");
        for (field, want) in expected {
            let got = &read[field.as_str()];
            match want.as_str() {
                Some(text) => assert_eq!(got.as_str(), Some(text), "{id}/{field}"),
                None => {
                    let (got, want) = (
                        got.as_f64()
                            .unwrap_or_else(|| panic!("{id}/{field}: {read}")),
                        want.as_f64().expect("numeric expectation"),
                    );
                    // Exact: these are integers and hundredths read off a
                    // screen, not measurements. A near miss is a misread.
                    assert!(
                        (got - want).abs() < 1e-9,
                        "{id}/{field}: read {got}, window showed {want}"
                    );
                }
            }
        }
    }
}
