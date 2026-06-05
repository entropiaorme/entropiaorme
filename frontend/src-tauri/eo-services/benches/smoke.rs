//! Placeholder micro-benchmark that keeps the criterion rail compiled and
//! runnable from day one; real hot-path benchmarks land with the services
//! they measure.

use criterion::{criterion_group, criterion_main, Criterion};

fn smoke(c: &mut Criterion) {
    c.bench_function("crate_name", |b| b.iter(eo_services::crate_name));
}

criterion_group!(benches, smoke);
criterion_main!(benches);
