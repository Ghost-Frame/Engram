//! PITR snapshot discovery bench.
//!
//! Creates N synthetic `engram-backup-YYYYMMDD-HHMMSS.db` files in a
//! tempdir and benchmarks `list_snapshots` (the public wrapper that
//! drives the private `collect_from` scan + parse pipeline).

use chrono::{Duration, TimeZone, Utc};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::fs::File;
use std::path::Path;
use tempfile::TempDir;

mod common;

use kleos_lib::db::pitr::list_snapshots;

fn seed_backups(dir: &Path, count: usize) {
    // Start at a fixed epoch so runs are byte-identical.
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap();
    for i in 0..count {
        let ts = base + Duration::minutes(i as i64);
        let name = format!("engram-backup-{}.db", ts.format("%Y%m%d-%H%M%S"));
        let path = dir.join(&name);
        File::create(&path).expect("create snapshot file");
    }
}

fn bench_collect_from(c: &mut Criterion) {
    let mut group = c.benchmark_group("pitr/collect_from");
    for &segments in &[10_usize, 100, 1_000] {
        let dir = tempfile::Builder::new()
            .prefix("kleos-pitr-bench-")
            .tempdir()
            .expect("tempdir");
        seed_backups(dir.path(), segments);

        group.throughput(Throughput::Elements(segments as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(segments),
            &dir,
            |b, dir: &TempDir| {
                b.iter(|| {
                    let snaps = list_snapshots(dir.path());
                    criterion::black_box(snaps);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_collect_from);
criterion_main!(benches);
