//! The recogniser's ground-truth comparison: every graded cell of the
//! screen-verbatim OCR bench read through the native engine, with the
//! raw exact-match count held to the figure the original engine
//! recorded over the same cells (262 of 594: the raw model text is
//! strict against screen-verbatim grading, with spacing and case
//! drift the downstream name resolution recovers; the production
//! read path's recovered grading sits far higher on the same bench).
//!
//! The bench is a locally-held capture set (real gameplay screens stay
//! out of the public tree), so the test runs only where
//! `EO_OCR_BENCH_DIR` points at it and the ONNX Runtime library is
//! loadable; everywhere else it skips with its reasons stated rather
//! than passing vacuously.

use std::path::{Path, PathBuf};

use eo_services::ocr_engine::OcrEngine;
use serde_json::Value;

/// The original engine's recorded raw-exact count over the same bench
/// (the runtime comparison's recorded figure, identical across both
/// implementations; the criterion is "not below").
const ORIGINAL_RAW_EXACT: usize = 262;
const EXPECTED_CELLS: usize = 594;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn model_paths() -> (PathBuf, PathBuf) {
    let assets = repo_root().join("frontend/src-tauri/entropia-orme/resources/models");
    (
        assets.join("svtrv2_rec.onnx"),
        assets.join("ppocr_keys_v1.txt"),
    )
}

struct GradedCell {
    id: String,
    path: PathBuf,
    expected: String,
}

/// Every graded data cell, in the bench's deterministic order.
fn graded_cells(bench: &Path) -> Vec<GradedCell> {
    let raw = std::fs::read_to_string(bench.join("ground_truth.json")).expect("ground truth");
    let gt: Value = serde_json::from_str(&raw).expect("ground truth parses");
    let panels = gt["panels"].as_object().expect("panels");
    let mut cells = Vec::new();
    for (kind, panel) in panels {
        for page in panel["pages"].as_array().expect("pages") {
            let page_index = page["page_index"].as_i64().expect("page index");
            let page_dir = bench
                .join("crops")
                .join(kind)
                .join(format!("page-{page_index:02}"));
            for row in page["rows"].as_array().expect("rows") {
                if row["kind"].as_str() != Some("data") {
                    continue;
                }
                let row_index = row["row"].as_i64().expect("row index");
                for (key, value) in row.as_object().expect("row object") {
                    if key == "row" || key == "kind" {
                        continue;
                    }
                    let path = page_dir.join(format!("row-{row_index:02}-{key}.png"));
                    cells.push(GradedCell {
                        id: format!("{kind}/page-{page_index:02}/row-{row_index:02}-{key}"),
                        path,
                        expected: value.as_str().expect("graded text").to_string(),
                    });
                }
            }
        }
    }
    cells
}

#[test]
fn the_native_recogniser_holds_the_original_engines_exact_rate() {
    let Ok(bench) = std::env::var("EO_OCR_BENCH_DIR") else {
        eprintln!(
            "EO_OCR_BENCH_DIR unset: the locally-held bench is absent on this host; skipping"
        );
        return;
    };
    let bench = PathBuf::from(bench);
    let (model, dict) = model_paths();
    let engine = match OcrEngine::new(&model, &dict) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("ONNX Runtime unavailable on this host ({error}); skipping");
            return;
        }
    };

    let cells = graded_cells(&bench);
    assert_eq!(
        cells.len(),
        EXPECTED_CELLS,
        "the graded bench carries exactly {EXPECTED_CELLS} data cells"
    );

    let mut exact = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for cell in &cells {
        let png = std::fs::read(&cell.path)
            .unwrap_or_else(|error| panic!("unreadable crop {}: {error}", cell.path.display()));
        let (text, _score) = engine
            .recognize_png(&png)
            .unwrap_or_else(|error| panic!("recognition failed on {}: {error}", cell.id));
        if text == cell.expected {
            exact += 1;
        } else if mismatches.len() < 10 {
            mismatches.push(format!(
                "{}: expected {:?}, read {:?}",
                cell.id, cell.expected, text
            ));
        }
    }

    assert!(
        exact >= ORIGINAL_RAW_EXACT,
        "the native engine read {exact}/{} cells exactly; the original's recorded \
         figure is {ORIGINAL_RAW_EXACT} and the port must not fall below it.\n\
         first mismatches:\n{}",
        cells.len(),
        mismatches.join("\n")
    );
    eprintln!(
        "native raw-exact {exact}/{} (the original's recorded figure: {ORIGINAL_RAW_EXACT})",
        cells.len()
    );
}
