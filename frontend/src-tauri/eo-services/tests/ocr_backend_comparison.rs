//! The ONNX-Runtime-vs-candle backend comparison: a faithfulness check that
//! the from-scratch candle SVTRv2 forward pass reproduces the default engine,
//! plus the corpus-level accuracy + latency sweep over the ground-truth bench.
//!
//! Both legs are gated on the locally-held bench (`EO_OCR_BENCH_DIR`, real
//! gameplay screens stay out of the public tree) and on the ONNX Runtime
//! library being loadable; everywhere else they skip with their reasons
//! stated rather than passing vacuously. Compiled only under the `candle`
//! feature (it constructs the candle engine).
//!
//! The single-cell leg compares raw post-softmax probabilities tensor-to-tensor
//! (cosine, max-abs, per-timestep argmax agreement): a faithful reimplementation
//! is near-identical, so this surfaces a wrong weight transpose, activation, or
//! norm epsilon in seconds, before the full sweep. The sweep leg holds candle
//! within two percentage points of the original's recorded raw-exact rate AND
//! at least 98% per-cell text agreement, and prints the committed comparison
//! table.

#![cfg(feature = "candle")]

use std::path::{Path, PathBuf};
use std::time::Instant;

use eo_services::ocr_candle::candle_engine;
use eo_services::ocr_engine::OcrEngine;
use serde_json::Value;

const EXPECTED_CELLS: usize = 594;
const ORIGINAL_RAW_EXACT: usize = 262;
/// Faithfulness bar: candle within 2pp of the original raw-exact (in cells)
/// and at least this per-cell agreement rate.
const AGREEMENT_FLOOR: f64 = 0.98;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn model_paths() -> (PathBuf, PathBuf) {
    let assets = repo_root().join("backend/assets/models");
    (
        assets.join("svtrv2_rec.onnx"),
        assets.join("ppocr_keys_v1.txt"),
    )
}

/// Pin the committed ONNX Runtime dylib so `OcrEngine::new` can load the
/// runtime without a system install, mirroring the engine unit tests.
fn ensure_ort_dylib() {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        let dylib =
            repo_root().join("frontend/src-tauri/entropia-orme/resources/ort/onnxruntime.dll");
        if dylib.exists() {
            std::env::set_var("ORT_DYLIB_PATH", dylib);
        }
    }
}

struct GradedCell {
    id: String,
    key: String,
    path: PathBuf,
    expected: String,
}

/// Every graded data cell, in the bench's deterministic order (the same
/// enumeration the native bench differential uses).
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
                        key: key.clone(),
                        path,
                        expected: value.as_str().expect("graded text").to_string(),
                    });
                }
            }
        }
    }
    cells
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b) {
        dot += x as f64 * y as f64;
        na += (x as f64).powi(2);
        nb += (y as f64).powi(2);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn max_abs(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).abs())
        .fold(0.0, f64::max)
}

/// Per-timestep argmax over `[t_len, n_classes]` row-major logits.
fn argmax_seq(logits: &[f32], t_len: usize, n_classes: usize) -> Vec<usize> {
    (0..t_len)
        .map(|t| {
            let row = &logits[t * n_classes..(t + 1) * n_classes];
            let mut best = 0usize;
            for (i, &v) in row.iter().enumerate() {
                if v > row[best] {
                    best = i;
                }
            }
            best
        })
        .collect()
}

#[test]
fn candle_matches_ort_on_one_cell() {
    let Ok(bench) = std::env::var("EO_OCR_BENCH_DIR") else {
        eprintln!("EO_OCR_BENCH_DIR unset: the locally-held bench is absent; skipping");
        return;
    };
    ensure_ort_dylib();
    let (model, dict) = model_paths();
    let ort = match OcrEngine::new(&model, &dict) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("ONNX Runtime unavailable ({error}); skipping");
            return;
        }
    };
    let candle = candle_engine(&model, &dict).expect("candle engine builds");

    let cells = graded_cells(Path::new(&bench));
    // The first non-trivial cell (a name, the widest input) is the sharpest probe.
    let cell = cells.iter().find(|c| c.key == "name").unwrap_or(&cells[0]);
    let png = std::fs::read(&cell.path).expect("read crop");
    let (lo, shape_o) = ort.logits_png(&png).expect("ort logits");
    let (lc, shape_c) = candle.logits_png(&png).expect("candle logits");

    assert_eq!(
        shape_o, shape_c,
        "output shape mismatch: ort {shape_o:?} vs candle {shape_c:?}"
    );
    let (t_len, n_classes) = shape_o;
    let cos = cosine(&lo, &lc);
    let mab = max_abs(&lo, &lc);
    let am_o = argmax_seq(&lo, t_len, n_classes);
    let am_c = argmax_seq(&lc, t_len, n_classes);
    let agree = am_o.iter().zip(&am_c).filter(|(a, b)| a == b).count();
    eprintln!(
        "cell {}: cosine {cos:.6}, max-abs {mab:.6}, per-timestep argmax agree {agree}/{t_len}",
        cell.id
    );
    let text_o = ort.recognize_png(&png).map(|(t, _)| t).unwrap_or_default();
    let text_c = candle.recognize_png(&png).map(|(t, _)| t).unwrap_or_default();
    eprintln!("  expected {:?}  ort {:?}  candle {:?}", cell.expected, text_o, text_c);
    assert!(cos > 0.99, "candle vs ort cosine {cos:.6} below 0.99: forward pass diverges");
}

#[test]
fn ort_vs_candle_accuracy_and_latency_sweep() {
    let Ok(bench) = std::env::var("EO_OCR_BENCH_DIR") else {
        eprintln!("EO_OCR_BENCH_DIR unset: the locally-held bench is absent; skipping");
        return;
    };
    ensure_ort_dylib();
    let (model, dict) = model_paths();
    let ort = match OcrEngine::new(&model, &dict) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("ONNX Runtime unavailable ({error}); skipping");
            return;
        }
    };
    let candle = candle_engine(&model, &dict).expect("candle engine builds");
    ort.warm_up();
    candle.warm_up();

    let cells = graded_cells(Path::new(&bench));
    assert_eq!(cells.len(), EXPECTED_CELLS, "graded bench cell count");

    let mut ort_exact = 0usize;
    let mut candle_exact = 0usize;
    let mut agree = 0usize;
    let mut ort_us: Vec<u128> = Vec::with_capacity(cells.len());
    let mut candle_us: Vec<u128> = Vec::with_capacity(cells.len());
    let mut disagreements: Vec<String> = Vec::new();

    for cell in &cells {
        let png = std::fs::read(&cell.path).expect("read crop");

        let t0 = Instant::now();
        let (ort_text, _) = ort.recognize_png(&png).expect("ort recognise");
        ort_us.push(t0.elapsed().as_micros());

        let t1 = Instant::now();
        let (candle_text, _) = candle.recognize_png(&png).expect("candle recognise");
        candle_us.push(t1.elapsed().as_micros());

        if ort_text == cell.expected {
            ort_exact += 1;
        }
        if candle_text == cell.expected {
            candle_exact += 1;
        }
        if ort_text == candle_text {
            agree += 1;
        } else if disagreements.len() < 15 {
            disagreements.push(format!(
                "  {} (gt {:?}): ort {:?} candle {:?}",
                cell.id, cell.expected, ort_text, candle_text
            ));
        }
    }

    let n = cells.len();
    let agreement = agree as f64 / n as f64;
    let mean = |v: &[u128]| v.iter().sum::<u128>() as f64 / v.len() as f64 / 1000.0;
    let p95 = |v: &[u128]| {
        let mut s = v.to_vec();
        s.sort_unstable();
        s[(s.len() as f64 * 0.95) as usize] as f64 / 1000.0
    };

    let table = format!(
        "| Backend | Raw-exact | Raw % | Mean ms/cell | p95 ms/cell |\n\
         | --- | --- | --- | --- | --- |\n\
         | ONNX Runtime (default) | {ort_exact}/{n} | {:.1}% | {:.2} | {:.2} |\n\
         | candle | {candle_exact}/{n} | {:.1}% | {:.2} | {:.2} |\n",
        100.0 * ort_exact as f64 / n as f64,
        mean(&ort_us),
        p95(&ort_us),
        100.0 * candle_exact as f64 / n as f64,
        mean(&candle_us),
        p95(&candle_us),
    );
    eprintln!("\n=== OCR backend sweep ({n} graded cells) ===\n{table}");
    eprintln!("per-cell ort==candle agreement: {agree}/{n} ({:.2}%)", 100.0 * agreement);
    if !disagreements.is_empty() {
        eprintln!("sample disagreements:\n{}", disagreements.join("\n"));
    }

    // Write the machine-readable sweep beside the bench (gitignored output);
    // the committed table is lifted from this run's printed output.
    let out_json = serde_json::json!({
        "ground_truth_cells": n,
        "baseline_raw_exact": ORIGINAL_RAW_EXACT,
        "ort": { "raw_exact": ort_exact, "mean_ms": mean(&ort_us), "p95_ms": p95(&ort_us) },
        "candle": { "raw_exact": candle_exact, "mean_ms": mean(&candle_us), "p95_ms": p95(&candle_us) },
        "per_cell_agreement": agreement,
    });
    if let Ok(dir) = std::env::var("EO_OCR_BENCH_DIR") {
        let _ = std::fs::write(
            Path::new(&dir).join("ocr_sweep_ort_vs_candle.json"),
            serde_json::to_string_pretty(&out_json).unwrap(),
        );
    }

    assert!(
        ort_exact >= ORIGINAL_RAW_EXACT,
        "ort raw-exact regressed: {ort_exact} < {ORIGINAL_RAW_EXACT}"
    );
    let floor = ORIGINAL_RAW_EXACT.saturating_sub((0.02 * n as f64).ceil() as usize);
    assert!(
        candle_exact >= floor,
        "candle raw-exact {candle_exact} below 2pp floor {floor}"
    );
    assert!(
        agreement >= AGREEMENT_FLOOR,
        "candle vs ort per-cell agreement {agreement:.4} below {AGREEMENT_FLOOR}"
    );
}
