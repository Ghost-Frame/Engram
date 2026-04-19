# R8 Benchmark Baseline

Zero-cloud criterion bench suite. All benches are fully local: ONNX
embedding runs against the cached model file; every DB bench uses a
tempfile SQLite seeded with deterministic synthetic data.

## Environment

- Rocky (canonical): Linux, x86_64, rustc 1.95.0 (59807616e 2026-04-14)
- WSL-dev (iteration): 6.6.114.1-microsoft-standard-WSL2, x86_64, ext4 on NVMe
- Profile: `[profile.bench]` thin-LTO, codegen-units 16, strip symbols
- Commit: f0dad06 (kleos-lib v0.3.2)
- Date: 2026-04-19

Criterion reports each measurement as `[lower p50 upper]`, the
confidence-interval triplet for the mean. Tail numbers (p95/p99) are
computed from each bench's `new/sample.json` by
`kleos-lib/benches/criterion_percentiles.py`. For `n=100` sample
benches (auth, pitr) the tail is well-resolved; for `n=15..30` benches
(graph, memory_search, embeddings) p95 and p99 often collapse to the
same sample due to coarse indexing. Re-run with a larger
`sample_size` in the bench source for finer tails.

## How to run

```
# WSL-dev loop (fast iteration, noisy numbers):
cargo bench --no-run -p kleos-lib --features test-utils    # compile only
cargo bench -p kleos-lib --features test-utils             # run + report

# Rocky canonical baseline:
cargo bench -p kleos-lib --features test-utils --jobs 1    # avoid SMT noise

# Embeddings bench requires ORT shared lib -- set ORT_DYLIB_PATH before
# running (ort crate uses load-dynamic). On Rocky:
ORT_DYLIB_PATH=/path/to/libonnxruntime.so \
    cargo bench -p kleos-lib --features test-utils --bench embeddings

# Embeddings bench also requires the bge-m3 model pre-staged under
# KLEOS_EMBEDDING_MODEL_DIR (default ~/.local/share/kleos/models/bge-m3/).
# If missing, the bench logs a skip notice and returns a no-op (~ns).
```

Reports land at `target/criterion/**/report/index.html`.

## Bench targets

| Target | File | What it measures |
|---|---|---|
| embeddings/encode_single | benches/embeddings.rs | ONNX encode of one short sentence (cache-hit path) |
| embeddings/encode_batch | benches/embeddings.rs | ONNX encode of batch sizes 1/8/32 (cache-hit path) |
| embeddings/encode_single_cold | benches/embeddings.rs | Real ONNX forward pass, rotates unique strings per iter |
| embeddings/encode_batch_cold | benches/embeddings.rs | Real ONNX forward pass at batch 1/8/32, rotates unique strings |
| memory_search/tiered | benches/memory_search.rs | hybrid_search over tempfile SQLite at 1k/10k/100k rows (cache-hit path) |
| memory_search/tiered_cold | benches/memory_search.rs | hybrid_search over tempfile SQLite at 1k/10k/100k rows (cache-miss path, 64 rotating queries) |
| memory_search/tiered_vector | benches/memory_search.rs | hybrid_search with a 1024-dim embedding so vector_search runs (cache masks most of the cost) |
| memory_search/vector_search_direct | benches/memory_search.rs | Raw vector_search() with 64 rotating embeddings, isolates P-001 serialize hotspot |
| memory_search/tiered_concurrent | benches/memory_search.rs | N=4/16/64 parallel readers on a shared DB, surfaces SEARCH_CACHE Mutex contention (P-009) |
| graph/build_graph_data | benches/graph_traversal.rs | build_graph_data with fan-out 4 at 1k/10k nodes |
| pitr/collect_from | benches/pitr_collect.rs | snapshot discovery in dirs of 10/100/1000 segments |
| auth_middleware/valid | benches/auth_middleware.rs | validate_key hot path (normalize + peppered SHA-256 + DB prefix lookup + ct_eq) |
| auth_middleware/rejected | benches/auth_middleware.rs | validate_key short-circuit on malformed bearer |

## Rocky canonical numbers (2026-04-19)

Full-sample `cargo bench -p kleos-lib --features test-utils --jobs 1`
on Rocky (LAN 192.168.8.133). Cold compile: 111m 06s under `--jobs 1`
with thin LTO. Bench run: ~3 min for all 5 targets.

### embeddings

Model now staged on Rocky at `~/.local/share/kleos/models/bge-m3/`
(tokenizer.json + model_quantized.onnx, ~570MB rsync'd from WSL).
Real ONNX runs execute under `ORT_DYLIB_PATH` pointing at
`libonnxruntime.so.1` from a nearby node_modules install.

| bench | p50 | p95 | p99 | throughput (elem/s) |
|---|---|---|---|---|
| encode_single/tiny_sentence | 7.34 µs | 7.80 µs | 7.87 µs | 140k |
| encode_batch/1 | 7.44 µs | 7.79 µs | 7.79 µs | 134k |
| encode_batch/8 | 64.77 µs | 102 µs | 102 µs | 123k per-item |
| encode_batch/32 | 253 µs | 347 µs | 347 µs | 126k per-item |

**Caveat (cache-hit trap):** `OnnxProvider` has a module-level
`EMBEDDING_CACHE` (LRU keyed on text SipHash). The hot benches above
reuse the same input strings every iter, so after the first iter
every call hits the cache. Those numbers therefore measure **the
EMBEDDING_CACHE hit path**, not the ONNX forward-pass cost. 7µs/text
is the cost of hash + LRU lookup + `Vec<f32>` clone of the 1024-dim
embedding; the ~1024x4=4KB clone plus ~sub-ns hash explains the
floor. Batch sizes scale linearly at ~7-8µs per cached text.

### embeddings (cold: real ONNX forward pass)

Cold variants (`encode_single_cold`, `encode_batch_cold`) rotate
unique input strings via an `AtomicUsize` cursor so every iter misses
`EMBEDDING_CACHE` and routes through tokenize + ORT inference +
L2-normalize + cache insert. These are the real bge-m3 CPU forward
pass numbers.

| bench | p50 | p95 | p99 | throughput (elem/s) |
|---|---|---|---|---|
| encode_single_cold/tiny_sentence | 511.20 ms | 535.23 ms | 535.23 ms | 1.95 |
| encode_batch_cold/1 | 510.43 ms | 526.77 ms | 526.77 ms | 1.95 |
| encode_batch_cold/8 | 4.10 s | 4.14 s | 4.14 s | 1.95 per-item |
| encode_batch_cold/32 | 16.43 s | 16.58 s | 16.58 s | 1.95 per-item |

**Key finding:** cold path is ~70,000x slower than cached (513 ms vs
7.3 µs). Per-item cost is flat across batch sizes (~513 ms/text),
indicating `OnnxProvider::embed_batch` serialises element-wise rather
than running a batched session tensor. Plausible wins:
(1) batch inputs into a single `session.run` call with a padded
`[batch, seq]` tensor; (2) enable ONNX Runtime intra-op threading
(`SessionBuilder::with_intra_threads`) to recruit idle cores on
Rocky (12-core Xeon, currently single-threaded); (3) switch from
`model_quantized.onnx` to a smaller fp16/int8 distillation for the
auto-embed hot path. EMBEDDING_CACHE absorbs the vast majority of
production load so cold tail is only exercised on genuinely novel
text, but a 100x improvement there is still table stakes before
turning on auto-embed for heavy ingestion.

### memory_search

Cache-hit (`tiered`) repeats the same SearchRequest every iter so the
module-level SEARCH_CACHE serves after the first miss -- this measures
the Vec<SearchResult> clone cost (P-003).

Cache-miss (`tiered_cold`) rotates 64 distinct `topic {N}` queries via
an AtomicUsize cursor so every iter forces FTS5 scan + lexical scoring
+ result assembly + cache insert.

| rows | hot p50 | hot p95 | hot p99 | cold p50 | cold p95 | cold p99 |
|---|---|---|---|---|---|---|
| 1,000 | 8.43 µs | 10.56 µs | 10.56 µs | 7.57 µs | 7.78 µs | 7.78 µs |
| 10,000 | 8.42 µs | 20.93 µs | 20.93 µs | 9.09 µs | 18.79 µs | 18.79 µs |
| 100,000 | 8.53 µs | 15.04 µs | 15.04 µs | 9.06 µs | 10.21 µs | 10.21 µs |

**Key finding (P-003):** at 1k rows cold is *faster* than hot
(7.62 µs vs 8.40 µs). The Vec<SearchResult> clone on cache hit costs
more than the FTS scan it's meant to skip for small working sets.
The cache only starts paying off at 10k+ rows where FTS cost
dominates clone cost. `Arc<Vec<SearchResult>>` remediation will be a
net win at every tier and a dramatic win at 1k (clone becomes ~1
atomic op).

### memory_search vector-path (P-001)

Two variants exercise the 1024-dim vector path:

1. `tiered_vector` supplies a fixed 1024-dim embedding through
   `hybrid_search`. SEARCH_CACHE absorbs the work after the first 64
   unique keys, so this variant reports numbers comparable to the
   cold FTS path (~8-10 µs) even though `vector_search` is doing
   real serialize work under the hood. Kept in the suite as a
   proxy for end-to-end hot-path vector queries in production (the
   cache hit ratio is ~100% for repeat queries).

2. `vector_search_direct` bypasses `hybrid_search` entirely and hits
   `kleos_lib::memory::vector::vector_search` head-on with 64
   rotating embeddings so SEARCH_CACHE cannot mask the serialize
   cost. This is the number that isolates the P-001 hotspot:
   `format!("[{}]", embedding.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(","))`
   at `kleos-lib/src/memory/vector.rs:19-26`.

| bench | p50 | p95 | p99 | throughput |
|---|---|---|---|---|
| tiered_vector/1000 | 9.41 µs | 10.87 µs | 10.87 µs | 106k q/s |
| tiered_vector/10000 | 8.27 µs | 9.02 µs | 9.02 µs | 121k q/s |
| tiered_vector/100000 | 8.48 µs | 10.85 µs | 10.85 µs | 118k q/s |
| vector_search_direct/1024d | 160.61 µs | 200.13 µs | 240.21 µs | 5.6k q/s |

**Key finding (P-001):** raw vector_search() costs ~160 µs per call
with sqlite-vec NOT loaded (vector_top_k SQL fails and the function
returns `Ok(vec![])`). That means the full 160 µs is spent on the
format! + 1024x f32.to_string + Vec<String> alloc + join + DB
round-trip for a query that returns zero rows. This is a 20x gap
vs the cached hybrid_search path. Remediation: swap the format! +
join for a single `write!` into a pre-allocated `String` with
capacity hint of `1024 * 12` chars, or serialize directly to a
BLOB parameter via `bincode` if sqlite-vec accepts raw bytes.
Follow-up: re-run after sqlite-vec is loaded on Rocky; the real
hot path will also incur actual index lookup cost that is hidden
here, so the 160 µs floor should be treated as the lower bound.

### memory_search concurrent (P-009)

`tiered_concurrent` spawns N parallel tokio tasks each calling
`hybrid_search` against a shared 10k-row DB and a shared 64-entry
request pool. Total time covers all N calls; per-elem throughput
divides out. A contention-free system would hold per-call latency
roughly constant as N grows (throughput would scale linearly). A
mutex-bound one sees per-call latency grow with N.

| N readers | p50 (total) | p95 | p99 | per-call p50 | aggregate thrpt |
|---|---|---|---|---|---|
| 4 | 36.72 µs | 44.38 µs | 44.38 µs | 9.18 µs | 107k q/s |
| 16 | 150.90 µs | 160.05 µs | 160.05 µs | 9.43 µs | 105k q/s |
| 64 | 609.12 µs | 1.28 ms | 1.28 ms | 9.52 µs | 91k q/s |

**Key finding (P-009):** aggregate throughput is flat at ~100k q/s
regardless of N. A truly parallel cache-hit path on a 12-core Rocky
should scale to ~1.3M q/s at N=16. The SEARCH_CACHE
`Mutex<LruCache>` serializes readers: every `cache_get` takes the
mutex, does an LRU `get` (which mutates recency order so needs
&mut), clones the `Vec<SearchResult>`, and releases. p99 at N=64
balloons to 1.28 ms (4x p50), the classic contention tail.
Remediation: swap to `RwLock<LruCache>` if reads dominate (but
LRU get needs mutation), or better: sharded cache keyed on
`user_id % SHARD_COUNT`, or drop the LRU entirely for a lock-free
`DashMap` + async-TTL. P-003 remediation (`Arc<Vec<SearchResult>>`)
also shrinks the critical section since the clone goes from
O(results) to one atomic ref-bump, which directly shortens mutex
hold time here.

### graph/build_graph_data

| nodes | p50 | p95 | p99 |
|---|---|---|---|
| 1,000 | 3.72 ms | 5.15 ms | 5.15 ms |
| 10,000 | 11.41 ms | 12.10 ms | 12.10 ms |

`build_graph_data` defaults to `limit: 500` so the 1k vs 10k delta
reflects SQL scan + link lookup over the larger candidate pool, not
node count in the emitted graph.

### pitr/collect_from

| segments | p50 | p95 | p99 |
|---|---|---|---|
| 10 | 23.88 µs | 25.01 µs | 27.31 µs |
| 100 | 163.81 µs | 174.10 µs | 185.57 µs |
| 1,000 | 1.63 ms | 1.78 ms | 2.21 ms |

Scales near-linearly with segment count (fs::read_dir + regex parse
per entry). Fan-out of 4 lanes in the dir iterator is already
reasonable; no obvious win flagged.

### auth_middleware

| path | p50 | p95 | p99 |
|---|---|---|---|
| valid/bearer_ok | 60.02 µs | 72.41 µs | 75.73 µs |
| rejected/missing_bearer | 34.18 µs | 38.67 µs | 45.89 µs |

Valid-path cost breakdown (approximate): v2 peppered SHA-256 of the
bearer + prefix lookup in `api_keys` via deadpool-sqlite reader + row
scan + constant-time `ct_eq`. Rejected path short-circuits in
`normalize_key` before any DB hit, hence ~2x faster.

## WSL-dev numbers (2026-04-19, reference)

Kept for regression-triangulation between the two hosts. WSL runs use
the same bench binary but different host: a WSL2 kernel on NVMe
through the Windows host. WSL is consistently ~2x FASTER than Rocky
on everything I/O-heavy and CPU-intensive (graph, memory_search,
pitr) and ~2x SLOWER on the pure-crypto auth_middleware/valid path
-- suggesting Rocky's CPU wins on SHA-256 but loses on SQLite/IO
throughput relative to the WSL host's NVMe path.

| bench | WSL p50 | Rocky p50 | ratio (Rocky/WSL) |
|---|---|---|---|
| auth_middleware/valid/bearer_ok | 124 µs | 61.3 µs | 0.49x |
| auth_middleware/rejected/missing_bearer | 60.7 µs | 33.5 µs | 0.55x |
| memory_search/tiered/1000 (hot) | 5.06 µs | 8.40 µs | 1.66x |
| memory_search/tiered/10000 (hot) | 5.19 µs | 8.46 µs | 1.63x |
| memory_search/tiered/100000 (hot) | 5.10 µs | 10.02 µs | 1.97x |
| memory_search/tiered_cold/1000 | 4.49 µs | 7.62 µs | 1.70x |
| graph/build_graph_data/1000 | 2.29 ms | 4.04 ms | 1.77x |
| graph/build_graph_data/10000 | 6.59 ms | 11.49 ms | 1.74x |
| pitr/collect_from/10 | 8.15 µs | 23.78 µs | 2.92x |
| pitr/collect_from/100 | 73.5 µs | 164.23 µs | 2.23x |
| pitr/collect_from/1000 | 725 µs | 1.65 ms | 2.28x |

## R8 audit follow-up tied to these numbers

Performance findings that benchmarks should quantify:

- **P-001** 1024x `.to_string()` per vector serialize -- not exercised
  by the current memory_search bench (no vector search triggered
  because SearchRequest.embedding is None). Add a vector-path bench
  variant that supplies a 1024-dim query embedding to measure
  vector_search + JSON serialize cost.
- **P-002** PageRank inner-loop `Vec::clone()` -- not in the bench
  suite yet; add a pagerank bench once graph_traversal numbers
  stabilize.
- **P-003** SearchResult cache clone -- **confirmed as a regression
  at small working sets**: Rocky 1k cold (7.62 µs) beats Rocky 1k hot
  (8.40 µs). The cache is net-negative when Vec<SearchResult> clone
  > FTS5 scan. Remediation: `Arc<Vec<SearchResult>>` reduces clone to
  one atomic refcount bump. Capture a before/after delta column once
  the refactor lands.
- **P-009** SEARCH_CACHE Mutex contention -- single-threaded bench
  can't surface this. Add a concurrent-reader variant (N tasks
  hammering hybrid_search with shared-request-space) to show RwLock
  impact.

## Next steps

1. Stage bge-m3 on Rocky: **done** (model_quantized.onnx +
   tokenizer.json at `~/.local/share/kleos/models/bge-m3/`, rsync
   from WSL, ~570MB). Embeddings numbers above are cache-hit not
   cold encode (see caveat in that section).
2. **Embeddings cold-path variant:** done. `encode_single_cold` and
   `encode_batch_cold/1|8|32` rotate unique strings via an
   `AtomicUsize` cursor so every iter misses `EMBEDDING_CACHE`. Real
   cold numbers captured on Rocky above: ~513 ms per text, flat
   across batch sizes. Follow-up: investigate batched session
   tensor + intra-op threads to collapse the 513 ms floor.
3. Land P-003 `Arc<Vec<SearchResult>>` remediation; re-run
   memory_search hot+cold on Rocky and fill a post-remediation column
   in the table above. Expected: hot path drops below cold at 1k
   (currently 8.43 µs hot > 7.57 µs cold at 1k is the confirmed
   regression).
4. P-001 vector-path variants: done. `tiered_vector` runs through
   hybrid_search (cache masks cost after 64 unique keys: ~9 us p50).
   `vector_search_direct` bypasses the cache and pins the real
   serialize cost at 160 us p50, a 20x gap. Remediation: replace
   format! + join with a pre-allocated `String` + `write!`, or
   route the embedding as a BLOB via bincode if sqlite-vec accepts
   raw bytes. Re-run once sqlite-vec is loaded on Rocky so real
   index lookup cost lands on top of the 160 us serialize floor.
5. P-009 concurrent-reader variant: done. tiered_concurrent at
   N=4/16/64 on Rocky shows aggregate throughput flat at ~100k q/s
   across concurrency levels (vs ideal ~1.3M q/s at N=16 on
   12-core Rocky). p99 at N=64 balloons to 1.28 ms, the classic
   contention tail. Remediation candidates: sharded cache,
   DashMap + async-TTL, or P-003 Arc<Vec<SearchResult>> to shrink
   the critical section.
6. Percentile extractor: **done** (`kleos-lib/benches/criterion_percentiles.py`
   walks `target/criterion` and prints p50/p95/p99 per bench). Bump
   `sample_size` on the 15/20/30-sample benches (graph, memory_search,
   embeddings) for finer tail resolution; auth + pitr already use 100.
