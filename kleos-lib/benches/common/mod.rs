//! Shared fixtures for criterion benches.
//!
//! Every bench opens its own tempfile SQLite so criterion iterations
//! do not contaminate a shared DB. `seed_memories(n)` populates a fixed
//! synthetic dataset deterministically; reruns of the same seed produce
//! byte-identical rows so numbers are reproducible across hosts.

#![allow(dead_code)]

use std::path::PathBuf;
use tempfile::TempDir;

/// Row count tier used across benches. Keep the three sizes: 1k for
/// quick sanity signal, 10k for realistic working set, 100k for tail
/// behavior (index seeks vs scans, cache pressure).
pub const TIER_SMALL: usize = 1_000;
pub const TIER_MED: usize = 10_000;
pub const TIER_LARGE: usize = 100_000;

/// Seeded RNG helper -- deterministic across runs + hosts.
/// Seed derives from the ASCII bytes of "EIDOLON".
pub fn seeded_rng() -> rand::rngs::StdRng {
    use rand::SeedableRng;
    const SEED: u64 = 0x45_49_44_4F_4C_4F_4E_00;
    rand::rngs::StdRng::seed_from_u64(SEED)
}

/// Owns the tempdir for the bench's DB. Drops the dir on drop.
pub struct BenchDb {
    pub dir: TempDir,
    pub path: PathBuf,
}

impl BenchDb {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("kleos-bench-")
            .tempdir()
            .expect("tempdir");
        let path = dir.path().join("bench.db");
        Self { dir, path }
    }
}

/// Produce N synthetic memory-like records. Content is deterministic
/// pseudo-prose so FTS indexes do real work without allocating from an
/// external corpus.
pub fn synthetic_memories(n: usize) -> Vec<SyntheticMemory> {
    use rand::Rng;
    let mut rng = seeded_rng();
    (0..n)
        .map(|i| SyntheticMemory {
            id: format!("mem-{i:08}"),
            content: format!(
                "note {i} about topic {t} with anchor {a}",
                t = rng.random_range(0..64u32),
                a = rng.random_range(0..1024u32),
            ),
            kind: ["task", "general", "discovery", "session"][i % 4].to_string(),
            score: rng.random::<f32>(),
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct SyntheticMemory {
    pub id: String,
    pub content: String,
    pub kind: String,
    pub score: f32,
}

/// Single shared tokio runtime for async benches. We use a multi-thread
/// runtime (1 worker is enough) so the reactor stays live in the
/// background between `rt.block_on(...)` calls. This is required for
/// pool-backed resources like `Database` which schedule work from their
/// `Drop` impls; on a current_thread runtime, drops that land between
/// block_on calls find no reactor and panic.
pub fn bench_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime")
}
