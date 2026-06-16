//! OCR hot-path microbenchmarks.
//!
//! `ocr_preprocess` measures the shared preprocess (resize + normalise + pad)
//! fresh (allocating its buffers per call) against the pooled `preprocess_into`
//! (reusing caller-held scratch), isolating the allocation-pooling delta on a
//! synthetic cell so it needs no model or private bench data.
//!
//! `ocr_recognize` measures end-to-end recognition for the ONNX Runtime and
//! (under `--features candle`) the candle backend on a real bench
//! crop. It runs only where `EO_OCR_BENCH_DIR` points at the locally-held
//! bench and the ONNX Runtime library is loadable; elsewhere it skips. candle
//! on CPU is far slower than the tuned ONNX Runtime kernels, so its sample
//! count is small.

use std::path::PathBuf;
use std::time::Duration;

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use eo_services::ocr_engine::{preprocess, preprocess_into, OcrEngine};

/// A synthetic 48x200 BGR cell (a deterministic gradient): representative
/// preprocess input with no dependency on the private bench.
fn synthetic_cell() -> (Vec<u8>, usize, usize) {
    let (h, w) = (48usize, 200usize);
    let mut img = vec![0u8; h * w * 3];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            img[i] = ((x * 255) / w) as u8;
            img[i + 1] = ((y * 255) / h) as u8;
            img[i + 2] = (((x + y) * 255) / (w + h)) as u8;
        }
    }
    (img, h, w)
}

fn bench_preprocess(c: &mut Criterion) {
    let (img, h, w) = synthetic_cell();
    let mut group = c.benchmark_group("ocr_preprocess");
    group.bench_function("fresh", |b| {
        b.iter(|| {
            let (tensor, width) = preprocess(black_box(&img), h, w);
            black_box((tensor, width));
        });
    });
    let mut resize_buf = Vec::new();
    let mut tensor = Vec::new();
    group.bench_function("pooled", |b| {
        b.iter(|| {
            let width = preprocess_into(black_box(&img), h, w, &mut resize_buf, &mut tensor);
            black_box(width);
            black_box(&tensor);
        });
    });
    group.finish();
}

fn model_paths() -> (PathBuf, PathBuf) {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("backend/assets/models");
    (
        assets.join("svtrv2_rec.onnx"),
        assets.join("ppocr_keys_v1.txt"),
    )
}

fn first_crop() -> Option<Vec<u8>> {
    let bench = std::env::var("EO_OCR_BENCH_DIR").ok()?;
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        let dylib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("frontend/src-tauri/entropia-orme/resources/ort/onnxruntime.dll");
        if dylib.exists() {
            std::env::set_var("ORT_DYLIB_PATH", dylib);
        }
    }
    // The first skill-name crop is the widest, sharpest probe.
    let path = PathBuf::from(bench).join("crops/skill/page-01/row-00-name.png");
    std::fs::read(path).ok()
}

fn bench_recognize(c: &mut Criterion) {
    let Some(png) = first_crop() else {
        eprintln!("EO_OCR_BENCH_DIR unset or crop missing; skipping ocr_recognize");
        return;
    };
    let (model, dict) = model_paths();
    let ort = match OcrEngine::new(&model, &dict) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("ONNX Runtime unavailable ({error}); skipping ocr_recognize");
            return;
        }
    };
    ort.warm_up();

    let mut group = c.benchmark_group("ocr_recognize");
    group.sample_size(10).measurement_time(Duration::from_secs(30));
    group.bench_function("ort", |b| {
        b.iter(|| ort.recognize_png(black_box(&png)).unwrap());
    });

    #[cfg(feature = "candle")]
    {
        let candle = eo_services::ocr_candle::candle_engine(&model, &dict)
            .expect("candle engine builds for the benchmark");
        candle.warm_up();
        group.bench_function("candle", |b| {
            b.iter(|| candle.recognize_png(black_box(&png)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_preprocess, bench_recognize);
criterion_main!(benches);
