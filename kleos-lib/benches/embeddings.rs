//! ONNX embedding throughput bench.
//!
//! Loads `OnnxProvider` from the locally cached bge-m3 model in offline
//! mode. Zero network, zero cloud. If the model is not cached the bench
//! logs a skip line and exercises no-op closures so the suite still
//! compiles + completes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod common;

use kleos_lib::config::Config;
use kleos_lib::embeddings::onnx::OnnxProvider;
use kleos_lib::embeddings::EmbeddingProvider;

static COLD_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn cold_text() -> String {
    let n = COLD_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("bench cold text {n}")
}

fn try_load_provider(rt: &tokio::runtime::Runtime) -> Option<Arc<OnnxProvider>> {
    let mut cfg = Config::default();
    cfg.embedding_offline_only = true;
    match rt.block_on(OnnxProvider::new(&cfg)) {
        Ok(p) => Some(Arc::new(p)),
        Err(e) => {
            eprintln!("embeddings bench: skipping -- OnnxProvider unavailable (offline-only): {e}");
            None
        }
    }
}

fn bench_encode_single(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let provider = try_load_provider(&rt);
    let text = "the quick brown fox jumps over the lazy dog";

    let mut group = c.benchmark_group("embeddings/encode_single");
    group.sample_size(30);
    group.throughput(Throughput::Elements(1));
    group.bench_function("tiny_sentence", |b| {
        b.iter(|| match provider.as_ref() {
            Some(p) => {
                let v = rt.block_on(p.embed(text)).expect("embed");
                criterion::black_box(v);
            }
            None => {
                criterion::black_box(text.len());
            }
        });
    });
    group.finish();
}

fn bench_encode_batch(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let provider = try_load_provider(&rt);

    let mut group = c.benchmark_group("embeddings/encode_batch");
    group.sample_size(20);
    for batch in [1_usize, 8, 32] {
        let corpus: Vec<String> = (0..batch)
            .map(|i| format!("synthetic sentence number {i} for embedding bench"))
            .collect();
        group.throughput(Throughput::Elements(batch as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch), &corpus, |b, corpus| {
            b.iter(|| match provider.as_ref() {
                Some(p) => {
                    let v = rt.block_on(p.embed_batch(corpus)).expect("embed_batch");
                    criterion::black_box(v);
                }
                None => {
                    criterion::black_box(corpus.iter().map(|s| s.len()).sum::<usize>());
                }
            });
        });
    }
    group.finish();
}

fn bench_encode_single_cold(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let provider = try_load_provider(&rt);

    let mut group = c.benchmark_group("embeddings/encode_single_cold");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("tiny_sentence", |b| {
        b.iter(|| match provider.as_ref() {
            Some(p) => {
                let text = cold_text();
                let v = rt.block_on(p.embed(&text)).expect("embed");
                criterion::black_box(v);
            }
            None => {
                criterion::black_box(cold_text().len());
            }
        });
    });
    group.finish();
}

fn bench_encode_batch_cold(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let provider = try_load_provider(&rt);

    let mut group = c.benchmark_group("embeddings/encode_batch_cold");
    group.sample_size(20);
    for batch in [1_usize, 8, 32] {
        group.throughput(Throughput::Elements(batch as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch), &batch, |b, &batch| {
            b.iter(|| {
                let corpus: Vec<String> = (0..batch).map(|_| cold_text()).collect();
                match provider.as_ref() {
                    Some(p) => {
                        let v = rt.block_on(p.embed_batch(&corpus)).expect("embed_batch");
                        criterion::black_box(v);
                    }
                    None => {
                        criterion::black_box(corpus.iter().map(|s| s.len()).sum::<usize>());
                    }
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_encode_single,
    bench_encode_batch,
    bench_encode_single_cold,
    bench_encode_batch_cold
);
criterion_main!(benches);
