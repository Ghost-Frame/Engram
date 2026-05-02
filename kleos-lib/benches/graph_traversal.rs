//! Graph traversal bench (build_graph_data node+edge assembly).
//!
//! Seeds memories + memory_links with a synthetic concept graph (fanout
//! 4) and benchmarks graph build at two node-count tiers.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

mod common;

use common::{synthetic_memories, BenchDb, TIER_MED, TIER_SMALL};

use kleos_lib::db::Database;
use kleos_lib::graph::builder::build_graph_data;
use kleos_lib::graph::types::GraphBuildOptions;

fn seed_graph(db: &Database, rt: &tokio::runtime::Runtime, n: usize) {
    let rows = synthetic_memories(n);
    rt.block_on(async {
        db.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            {
                let mut mstmt = tx
                    .prepare(
                        "INSERT INTO memories (content, category, source, importance, user_id, is_latest)
                         VALUES (?1, ?2, 'bench', 5, 1, 1)",
                    )
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                for m in &rows {
                    mstmt
                        .execute(rusqlite::params![m.content, m.kind])
                        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                }
            }
            // Fanout 4 ring: link i -> i+1, i+2, i+3, i+4 (mod n). Skip self.
            let count = n as i64;
            {
                let mut lstmt = tx
                    .prepare(
                        "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type)
                         VALUES (?1, ?2, ?3, 'similarity')",
                    )
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                for i in 1..=count {
                    for k in 1..=4_i64 {
                        let mut j = i + k;
                        if j > count {
                            j -= count;
                        }
                        if j == i {
                            continue;
                        }
                        let sim = 0.5_f32 + (k as f32) * 0.1;
                        lstmt
                            .execute(rusqlite::params![i, j, sim])
                            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                    }
                }
            }
            tx.commit()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed graph")
    });
}

fn build_db(rt: &tokio::runtime::Runtime, n: usize) -> (BenchDb, Database) {
    let bench = BenchDb::new();
    let db = rt
        .block_on(Database::connect(bench.path.to_str().expect("utf8 path")))
        .expect("Database::connect");
    seed_graph(&db, rt, n);
    (bench, db)
}

fn bench_build_graph(c: &mut Criterion) {
    let rt = common::bench_runtime();
    let _guard = rt.enter();

    let mut group = c.benchmark_group("graph/build_graph_data");
    group.sample_size(15);
    for &n in &[TIER_SMALL, TIER_MED] {
        let (_bench, db) = build_db(&rt, n);
        let opts = GraphBuildOptions {
            user_id: 1,
            limit: None,
        };
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let r = rt
                    .block_on(build_graph_data(&db, &opts))
                    .expect("build_graph_data");
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_build_graph);
criterion_main!(benches);
