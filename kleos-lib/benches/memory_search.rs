//! End-to-end memory search bench (hybrid FTS + vector).
//!
//! Seeds a tempfile SQLite at three row-count tiers (1k / 10k / 100k)
//! with deterministic synthetic content and benchmarks
//! `kleos_lib::memory::search::hybrid_search`. Zero cloud calls: no
//! embedding is supplied, so hybrid_search exercises the FTS + lexical
//! path only. Seed writes happen once per tier outside the timed region.
//!
//! Two bench groups:
//!
//!   - `memory_search/tiered`: repeats the same SearchRequest every iter,
//!     so after the first call the module-level SEARCH_CACHE serves it.
//!     Measures the cache-hit hot path (audit finding P-003).
//!   - `memory_search/tiered_cold`: rotates the query text across a pool
//!     so each iter misses the cache. Measures the real search work
//!     (FTS scan + scoring + result assembly -- audit findings P-006,
//!     P-007).
//!   - `memory_search/tiered_vector`: same cold rotation but supplies a
//!     deterministic 1024-dim embedding so hybrid_search routes through
//!     vector_search(). Exercises the P-001 hotspot: 1024x f32.to_string
//!     + Vec<String> alloc + join in kleos-lib/src/memory/vector.rs.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicUsize, Ordering};

mod common;

use common::{synthetic_memories, BenchDb, TIER_LARGE, TIER_MED, TIER_SMALL};

use kleos_lib::db::Database;
use kleos_lib::memory::search::hybrid_search;
use kleos_lib::memory::types::SearchRequest;
use kleos_lib::memory::vector::vector_search;

fn seed_db(db: &Database, rt: &tokio::runtime::Runtime, rows: Vec<common::SyntheticMemory>) {
    rt.block_on(async {
        db.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO memories (content, category, source, importance, user_id, is_latest)
                         VALUES (?1, ?2, 'bench', 5, 1, 1)",
                    )
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                for m in &rows {
                    stmt.execute(rusqlite::params![m.content, m.kind])
                        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                }
            }
            tx.commit()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed memories")
    });
}

fn build_db(rt: &tokio::runtime::Runtime, n: usize) -> (BenchDb, Database) {
    let bench = BenchDb::new();
    let db = rt
        .block_on(Database::connect(bench.path.to_str().expect("utf8 path")))
        .expect("Database::connect");
    seed_db(&db, rt, synthetic_memories(n));
    (bench, db)
}

fn bench_search_tiers(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();

    let mut group = c.benchmark_group("memory_search/tiered");
    group.sample_size(20);
    for &n in &[TIER_SMALL, TIER_MED, TIER_LARGE] {
        let (_bench, db) = build_db(&rt, n);

        let req = SearchRequest {
            query: "topic".to_string(),
            embedding: None,
            limit: Some(20),
            category: None,
            source: None,
            tags: None,
            threshold: None,
            user_id: Some(1),
            space_id: None,
            include_forgotten: Some(false),
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            latest_only: true,
            source_filter: None,
        };

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let r = rt
                    .block_on(hybrid_search(&db, req.clone()))
                    .expect("hybrid_search");
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

fn cold_request_pool() -> Vec<SearchRequest> {
    // Synthetic memories use `topic {t}` with t in 0..64 and `anchor {a}`
    // with a in 0..1024. 64 distinct topic queries guarantees every
    // iteration hits a unique cache key.
    (0..64u32)
        .map(|t| SearchRequest {
            query: format!("topic {t}"),
            embedding: None,
            limit: Some(20),
            category: None,
            source: None,
            tags: None,
            threshold: None,
            user_id: Some(1),
            space_id: None,
            include_forgotten: Some(false),
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            latest_only: true,
            source_filter: None,
        })
        .collect()
}

fn bench_search_tiers_cold(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();

    let mut group = c.benchmark_group("memory_search/tiered_cold");
    group.sample_size(20);
    for &n in &[TIER_SMALL, TIER_MED, TIER_LARGE] {
        let (_bench, db) = build_db(&rt, n);
        let pool = cold_request_pool();
        let cursor = AtomicUsize::new(0);

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let idx = cursor.fetch_add(1, Ordering::Relaxed) % pool.len();
                let req = pool[idx].clone();
                let r = rt.block_on(hybrid_search(&db, req)).expect("hybrid_search");
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

/// Deterministic 1024-dim query embedding. Values derive from index so
/// the vector is identical across runs + hosts. Kept as a constant-time
/// builder to avoid poisoning the per-iter measurement with Vec alloc.
fn fixed_query_embedding() -> Vec<f32> {
    (0..1024u32).map(|i| (i as f32).sin() * 0.25).collect()
}

fn bench_search_tiers_vector(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();

    let mut group = c.benchmark_group("memory_search/tiered_vector");
    group.sample_size(20);
    for &n in &[TIER_SMALL, TIER_MED, TIER_LARGE] {
        let (_bench, db) = build_db(&rt, n);
        let embedding = fixed_query_embedding();
        let pool: Vec<SearchRequest> = (0..64u32)
            .map(|t| SearchRequest {
                query: format!("topic {t}"),
                embedding: Some(embedding.clone()),
                limit: Some(20),
                category: None,
                source: None,
                tags: None,
                threshold: None,
                user_id: Some(1),
                space_id: None,
                include_forgotten: Some(false),
                mode: None,
                question_type: None,
                expand_relationships: false,
                include_links: false,
                latest_only: true,
                source_filter: None,
            })
            .collect();
        let cursor = AtomicUsize::new(0);

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let idx = cursor.fetch_add(1, Ordering::Relaxed) % pool.len();
                let req = pool[idx].clone();
                let r = rt.block_on(hybrid_search(&db, req)).expect("hybrid_search");
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

/// Direct micro-bench of vector_search(). Bypasses hybrid_search so
/// SEARCH_CACHE cannot mask the P-001 hotspot (1024x f32.to_string +
/// Vec<String> alloc + join on every call). Rotates 64 distinct
/// embeddings via AtomicUsize cursor so the serialize runs fresh
/// each iter. Runs at the medium tier only since the format! cost
/// is independent of row count.
fn bench_vector_search_direct(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();

    let (_bench, db) = build_db(&rt, TIER_MED);

    // Pre-build 64 distinct 1024-dim embeddings so per-iter cost is
    // vector_search() itself, not Vec<f32> construction.
    let pool: Vec<Vec<f32>> = (0..64u32)
        .map(|k| {
            (0..1024u32)
                .map(|i| ((i + k) as f32).sin() * 0.25)
                .collect()
        })
        .collect();
    let cursor = AtomicUsize::new(0);

    let mut group = c.benchmark_group("memory_search/vector_search_direct");
    group.sample_size(30);
    group.throughput(Throughput::Elements(1));
    group.bench_function("1024d", |b| {
        b.iter(|| {
            let idx = cursor.fetch_add(1, Ordering::Relaxed) % pool.len();
            let r = rt
                .block_on(vector_search(&db, &pool[idx], 20, 1))
                .expect("vector_search");
            criterion::black_box(r);
        });
    });
    group.finish();
}

/// Multi-reader bench. Spawns N parallel tokio tasks hammering
/// hybrid_search against a shared DB + request pool. Surfaces
/// SEARCH_CACHE Mutex<LruCache> contention (audit finding P-009):
/// a single-threaded bench cannot show lock wait, but parallel
/// readers serialize on the cache mutex and per-call latency grows
/// with N. Fixed at the 10k-row tier (cache cost dominates FTS
/// there) and uses a dedicated multi-thread runtime sized to the
/// reader count since common::bench_runtime is single-worker.
fn bench_search_concurrent(c: &mut Criterion) {
    use std::sync::Arc;

    let mut group = c.benchmark_group("memory_search/tiered_concurrent");
    group.sample_size(20);
    for &n_readers in &[4usize, 16, 64] {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(n_readers.max(num_cpus_fallback()))
            .enable_all()
            .build()
            .expect("tokio runtime");
        let _guard = rt.enter();
        let (_bench, db) = build_db(&rt, TIER_MED);
        let db = Arc::new(db);
        let pool = Arc::new(cold_request_pool());
        let cursor = Arc::new(AtomicUsize::new(0));

        group.throughput(Throughput::Elements(n_readers as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(n_readers),
            &n_readers,
            |b, &n| {
                b.iter(|| {
                    let futures_iter = (0..n).map(|_| {
                        let db = db.clone();
                        let pool = pool.clone();
                        let cursor = cursor.clone();
                        async move {
                            let idx = cursor.fetch_add(1, Ordering::Relaxed) % pool.len();
                            hybrid_search(&db, pool[idx].clone()).await
                        }
                    });
                    let results = rt.block_on(futures::future::join_all(futures_iter));
                    criterion::black_box(results);
                });
            },
        );
    }
    group.finish();
}

fn num_cpus_fallback() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

criterion_group!(
    benches,
    bench_search_tiers,
    bench_search_tiers_cold,
    bench_search_tiers_vector,
    bench_vector_search_direct,
    bench_search_concurrent
);
criterion_main!(benches);
