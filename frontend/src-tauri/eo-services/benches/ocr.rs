//! OCR preprocess microbenchmark.
//!
//! Measures the shared preprocess (resize + normalise + pad) fresh (allocating
//! its resize buffer per call) against the pooled `preprocess_into` (reusing a
//! caller-held buffer), on a synthetic cell so it needs no model or private
//! bench data. Preprocess is compute-bound, so pooling is an allocation win
//! rather than a latency one; the bench confirms it does not regress.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use eo_services::ocr_engine::{preprocess, preprocess_into};

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
            let out = preprocess(black_box(&img), h, w);
            black_box(out);
        });
    });
    let mut resize_buf = Vec::new();
    group.bench_function("pooled", |b| {
        b.iter(|| {
            let out = preprocess_into(black_box(&img), h, w, &mut resize_buf);
            black_box(out);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_preprocess);
criterion_main!(benches);
