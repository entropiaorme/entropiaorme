//! The reading pipeline against real captures of the client.
//!
//! The unit tests draw their own windows, which proves the geometry
//! but not that the recogniser can read what the client actually
//! renders. These tests replay a private corpus of full-screen
//! captures, each value in it transcribed independently by two people
//! and reconciled, and hold the pipeline to one rule: a value is read
//! exactly as it was on screen, or it is refused. A wrong value
//! accepted is the only failure that matters.
//!
//! Real gameplay screens stay out of the public tree, so every test
//! here runs only where `EO_GAME_UI_CORPUS_DIR` names the corpus and
//! the bundled ONNX Runtime is loadable, and says why when it skips.
//! Frames marked held out in the corpus are replayed only when
//! `EO_GAME_UI_HELD_OUT` is set: they exist to test, once, that the
//! pipeline generalises to an interface scale nothing was tuned on.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use eo_services::game_ui::locate::{read_window_field, ReadError};
use eo_services::game_ui::profile::UiProfile;
use eo_services::ocr_engine::{load_bgr_png, OcrEngine};
use eo_services::skill_panel::BgrImage;
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn resources() -> PathBuf {
    repo_root().join("app/src-tauri/entropia-orme/resources")
}

fn profile() -> UiProfile {
    UiProfile::load(&resources().join("ui_profile.json"))
        .expect("the bundled interface profile loads")
}

struct Corpus {
    dir: PathBuf,
    labels: Value,
    manifest: Value,
}

impl Corpus {
    fn open() -> Option<Corpus> {
        let Some(dir) = std::env::var_os("EO_GAME_UI_CORPUS_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
        else {
            eprintln!("skipped: EO_GAME_UI_CORPUS_DIR is not set, so there are no real captures to replay");
            return None;
        };
        let json = |name: &str| -> Value {
            serde_json::from_str(
                &std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        Some(Corpus {
            labels: json("labels.json"),
            manifest: json("manifest.json"),
            dir,
        })
    }

    /// The frames to replay, with their labels: every frame, less the
    /// held-out ones unless those were asked for.
    fn frames(&self) -> Vec<(String, &Value)> {
        let held_out_wanted = std::env::var_os("EO_GAME_UI_HELD_OUT").is_some();
        self.labels["frames"]
            .as_object()
            .expect("frames")
            .iter()
            .filter(|(name, _)| {
                let held_out = self.manifest["frames"][name.as_str()]["held_out"]
                    .as_bool()
                    .unwrap_or(false);
                held_out == held_out_wanted
            })
            .map(|(name, labels)| (name.clone(), labels))
            .collect()
    }

    fn frame(&self, name: &str) -> BgrImage {
        let path = self.dir.join("frames").join(format!("{name}.png"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let (data, h, w) = load_bgr_png(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        BgrImage { data, h, w }
    }
}

/// The bundled recogniser, loaded once for the whole test binary. The
/// runtime is pinned to the committed library: left to search, the
/// loader can hang on a foreign-format library instead of reporting it.
fn recogniser() -> Option<Arc<OcrEngine>> {
    static ENGINE: OnceLock<Option<Arc<OcrEngine>>> = OnceLock::new();
    static LOAD: Mutex<()> = Mutex::new(());
    let _guard = LOAD.lock().unwrap_or_else(|p| p.into_inner());
    ENGINE
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            let dylib = resources().join("ort-linux/libonnxruntime.so");
            #[cfg(not(target_os = "linux"))]
            let dylib = resources().join("ort/onnxruntime.dll");
            if !dylib.is_file() {
                eprintln!(
                    "skipped: the bundled ONNX Runtime is absent at {}",
                    dylib.display()
                );
                return None;
            }
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
            let models = resources().join("models");
            match OcrEngine::new(
                &models.join("svtrv2_rec.onnx"),
                &models.join("ppocr_keys_v1.txt"),
            ) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(error) => {
                    eprintln!("skipped: the recogniser did not load ({error})");
                    None
                }
            }
        })
        .clone()
}

fn reader(engine: Arc<OcrEngine>) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
    move |image: &BgrImage| engine.recognize_bgr(&image.data, image.h, image.w).ok()
}

/// What became of one labelled value.
#[derive(Default)]
struct Tally {
    exact: usize,
    refused: Vec<String>,
    wrong: Vec<String>,
}

impl Tally {
    fn report(&self, what: &str) {
        println!(
            "{what}: {} read exactly, {} refused, {} WRONG",
            self.exact,
            self.refused.len(),
            self.wrong.len()
        );
        for line in &self.refused {
            println!("  refused: {line}");
        }
        assert!(
            self.wrong.is_empty(),
            "wrong values were accepted:\n{}",
            self.wrong.join("\n")
        );
    }
}

/// Replay one labelled field of one window kind over every frame whose
/// labels carry `surface.label_key`.
fn replay_field(window: &str, field: &str, surface: &str, label_key: &str) -> Option<Tally> {
    let corpus = Corpus::open()?;
    let engine = recogniser()?;
    let read = reader(engine);
    let profile = profile();
    let mut tally = Tally::default();
    for (name, labels) in corpus.frames() {
        let Some(want) = labels
            .get(surface)
            .and_then(|s| s.get(label_key))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let image = corpus.frame(&name);
        match read_window_field(&image, &profile, window, field, &read) {
            Ok(got) if got.parsed.text == want => tally.exact += 1,
            Ok(got) => tally.wrong.push(format!(
                "{name}: read {:?}, the screen shows {want:?}",
                got.parsed.text
            )),
            Err(ReadError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => tally.refused.push(format!("{name} ({want}): {error}")),
        }
    }
    Some(tally)
}

#[test]
fn repair_cost_is_read_exactly_or_refused() {
    let Some(tally) = replay_field("repair_terminal", "repair_cost", "repair", "repair_cost")
    else {
        return;
    };
    tally.report("repair cost");
    assert!(
        tally.refused.is_empty(),
        "every repair frame shows its cost in full"
    );
}

#[test]
fn trade_terminal_total_is_read_exactly_or_refused() {
    let Some(tally) = replay_field("trade_terminal", "total", "trade", "total") else {
        return;
    };
    tally.report("trade terminal total");
    assert!(
        tally.refused.is_empty(),
        "every trade frame shows its total in full"
    );
}

/// A frame with the terminal closed must refuse, not read something
/// else's number.
#[test]
fn a_closed_terminal_is_a_refusal() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let mut checked = 0;
    for (name, labels) in corpus.frames() {
        if labels.get("repair").is_some() || labels.get("trade").is_some() {
            continue;
        }
        // Frames with other windows open are the interesting ones.
        if labels["windows"].as_array().is_none_or(|w| w.is_empty()) || checked >= 6 {
            continue;
        }
        let image = corpus.frame(&name);
        for (window, field) in [
            ("repair_terminal", "repair_cost"),
            ("trade_terminal", "total"),
        ] {
            let got = read_window_field(&image, &profile, window, field, &read);
            assert!(
                matches!(got, Err(ReadError::WindowNotFound)),
                "{name}: {window}.{field} answered {got:?} with the terminal closed"
            );
        }
        checked += 1;
    }
    println!("closed terminal: {checked} frames with other windows open, all refused");
}
