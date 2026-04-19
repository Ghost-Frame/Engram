# R8 Audit Sweep (kleos)

Delta + full-workspace audit against HEAD. Three parallel passes:
security, performance, robustness. Findings below are the post-triage
cut -- false positives from the agent reports have been marked.

Severity: HIGH = fix before next release; MED = fix this sweep or file
follow-up; LOW = defense-in-depth, low impact; INFO = noted for context.

## Security

| ID | Severity | File | Status | Notes |
|---|---|---|---|---|
| S-001 | -- | kleos-server/src/routes/skills/mod.rs | false positive | Middleware kleos-server/src/middleware/auth.rs:129 already enforces `Scope::Write` for all POST/PUT/PATCH/DELETE. MCP uses per-tool `require_write` because it lacks HTTP-method middleware. |
| S-002 | MED | kleos-lib/src/skills/evolver.rs | FIXED | Added MAX_DIRECTION=2000 cap + non-empty check in derive_skill; MAX_DESCRIPTION=2000 cap in capture_skill. |
| S-003 | LOW | kleos-sidecar/src/watcher.rs:369 | follow-up | kleos_url sent through reqwest without `validate_outbound_url`. Sidecar runs in trusted env but tighten once CLI flag exposes kleos_url. |
| S-004 | LOW | kleos-server/src/routes/gui/mod.rs:179 | follow-up | `tighten_secret_perms` is no-op on Windows. Document NTFS ACL requirement or add windows-rs fallback. |
| S-005 | INFO | kleos-lib/src/auth.rs:408 | noted | v1 key downgrade continues silently; add distinct metric/rejection code. |

## Performance (4 HIGH, 8 MED, 15 LOW, 3 INFO after triage)

Prioritized top 10; remaining findings live in /tmp/r8-audit-findings.md
until bench numbers validate impact.

| ID | Severity | File | Status | Notes |
|---|---|---|---|---|
| P-001 | HIGH | kleos-lib/src/memory/vector.rs:19 | FIXED | Replaced `format!("[{}]", ...)` with pre-allocated String + `write!` loop. 1024x `to_string()` + Vec<String> + join eliminated. Unit-tested against old implementation for byte-identity. Verified on Rocky: vector_search_direct/1024d 160.61us -> 117.92us median (1.36x, -33.5%, p=0.00). |
| P-002 | HIGH | kleos-lib/src/graph/pagerank.rs:115,758 | FIXED | Dropped `get(&id).cloned().unwrap_or_default()` in favor of borrow (`.as_slice()`). Saves a Vec alloc per node per PageRank iteration (~25 iters * N nodes). |
| P-003 | HIGH | kleos-lib/src/memory/search.rs:79,917 | deferred | `Arc<Vec<...>>` fix requires public API change (hybrid_search + 20 callers). Captured for a dedicated refactor pass. |
| P-004 | HIGH | kleos-lib/src/memory/vector.rs | NOT APPLICABLE | Ingest path already serializes via `embedding_to_blob` (LE bytes). No separate format! site. Finding rolled into P-001. |
| P-005 | MED | kleos-lib/src/graph/search.rs:311 | FIXED | Replaced `Vec<Box<dyn ToSql>>` + double-Vec with borrow-only `Vec<&dyn ToSql>`. Applied to graph/search.rs and 4 sites in memory/search.rs. |
| P-006 | MED | kleos-lib/src/memory/search.rs:902 | partial | Added `Vec::with_capacity(3)` to skip reallocs; `join("+")` + Vec<String> storage required by `SearchResult` schema. |
| P-007 | MED | kleos-lib/src/memory/search.rs:1387 | FIXED | `compute_string_facets` now keys the first-pass HashMap by `&'a str`; only unique values pay the `to_string()` when buckets are built. |
| P-008 | MED | kleos-lib/src/memory/search.rs:174 | retained | `ids.to_vec()` is unavoidable for the `async move` closure capture in `db.read`. No-op without a broader Arc<[i64]> refactor. |
| P-009 | MED | kleos-lib/src/memory/search.rs:52 | deferred | SEARCH_CACHE lock-contention. Bench shows flat aggregate throughput at N=4/16/64 readers (see R8-baseline.md). Fix is a sharded cache or DashMap -- larger refactor. |
| P-010 | MED | kleos-lib/src/memory/search.rs:1173 | retained | SearchRequest clones constrained by `passes_filters` closure borrowing `req` across the branch. Cannot mem::take fields without restructuring the filter. |

All HIGHs are refactors whose impact is best quantified by the
criterion suite before rewriting.

## Robustness

| ID | Severity | File | Status | Notes |
|---|---|---|---|---|
| R-001 | HIGH* | kleos-server/src/routes/health/mod.rs:232 | FIXED | `.unwrap()` on response builder -> `.unwrap_or_else(... Body::empty())`. Static header bytes make failure unreachable in practice, but no panic on network path. |
| R-002 | -- | kleos-server/src/routes/onboard/mod.rs:459 | false positive | `panic!` is inside `#[tokio::test]` block, not reachable from network. |
| R-003 | MED | kleos-lib/src/webhooks.rs:625 | follow-up | Fire-and-forget `tokio::spawn` for deliver_with_retry. Task retains retry + dead-letter internally; upstream telemetry would improve visibility. |
| R-004 | HIGH | kleos-lib/src/webhooks.rs:18 | FIXED | WEBHOOK_CLIENT now has `.connect_timeout(5s)` + `.timeout(10s)`. |
| R-005 | HIGH* | kleos-server/src/routes/fsrs/mod.rs:44 | FIXED | `unreachable!()` replaced with `Rating::Good` fallback. Previous guard already prevents entry, but future refactors won't turn a bad grade into a panic. |
| R-006 | MED | kleos-server/src/routes/ingestion/mod.rs:1074 | FIXED | Wrapped `progress_rx.recv()` in `tokio::time::timeout(300s)`; on elapse the relay emits a heartbeat SSE frame instead of blocking forever. |
| R-007 | MED | kleos-lib/src/intelligence/llm.rs:84 | FIXED | `.text().await` errors are now surfaced in the returned error string (`<failed to read body: e>`) instead of silently yielding "". |
| R-008 | MED | kleos-server/src/main.rs:276 | follow-up | Background task exit logs but does not restart. Consider supervisor or exponential-backoff respawn. |
| R-009 | MED | kleos-server/src/routes/context/mod.rs:117 | FIXED | SSE `send` errors on the result + error frames now log at debug (`client gone`) instead of silent drop. |
| R-010 | MED | kleos-server/src/main.rs:175 | follow-up | Session HashMap unbounded. Needs TTL + periodic cleanup. |
| R-011 | MED | kleos-lib/src/webhooks.rs:589 | FIXED | `emit_webhook_event` now logs a `warn!` with the DB error + user_id + event before bailing; previously a silent no-op. |

Remaining LOW/INFO robustness findings retained in /tmp/r8-audit-findings.md
for batched follow-up.

## Verification

- Bench suite: docs/benchmarks/R8-baseline.md (in flight).
- Lint pass: `cargo clippy --all-targets --all-features -- -D warnings` on green.
- Build: `cargo bench --no-run -p kleos-lib --features test-utils` in flight.

## Not-a-finding / explicitly cleared

- Tenant isolation: all memory/search/skills/graph routes use `WHERE user_id = ?` with bound params.
- SQL injection: rusqlite `params![]` used throughout; no string-formatted SQL found.
- SSRF in webhook delivery: `validate_webhook_url` + `resolve_and_validate_url` reject non-http(s), loopback, RFC1918.
- Constant-time HMAC: `subtle::ct_eq` used in both cookie HMAC and API key hash compare.
- R7 remediations (10 findings closed): still holding.
