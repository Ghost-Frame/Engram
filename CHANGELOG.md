# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `/search/web` endpoint -- thin proxy over private SearXNG instance with auth, input validation, and normalized response schema
- `kleos-cli skill` subcommands (search, list, get, execute, capture, fix, derive, stats, lineage, evolve) backed by REST API
- Autonomous skill evolution in the dreamer background task -- capture, fix, and derive skills via local LLM on a 30-minute interval
- Skill evolvers (capture/fix/derive) wired to local LLM (Ollama) with three-shot prompting, sanitized output, and lineage tracking
- Activity fan-out now includes skill search on task.completed/error.raised
- Broca action ledger entries reference their Axon event ID
- `/chiasm/*` route aliases for Syntheos-convention task endpoints
- SvelteKit GUI vendored into the repo under `gui/`; graph view default raised to 5000 nodes
- Multi-arch container workflow (linux/amd64 + linux/arm64) publishing to ghcr.io/ghost-frame/kleos
- Criterion benchmark suite: embeddings, memory_search (hybrid + vector + concurrent), graph traversal, PITR, auth middleware
- `release-prod` build profile with thin LTO for faster production builds

### Changed

- `hybrid_search` returns `Arc<Vec<SearchResult>>` -- cache hits are a refcount bump instead of a deep clone (P-003)
- Search cache sharded to 32 Mutex-guarded LruCache segments instead of one global lock; capacity bumped 512 to 2048 (P-009)
- `hydrate_candidates` / `fetch_memories_batch` / `fetch_links_batch` take `Arc<[i64]>` to avoid cloning ID vecs into async moves (P-008)
- `faceted_search` takes the 4KB embedding from the request before building the inner SearchRequest (P-010)
- `embedding_to_json_array` uses preallocated buffer + `write!` loop -- 1.36x speedup (P-001)
- PageRank inner loop borrows via `.as_slice()` instead of cloning the Vec (P-002)
- Five sites in graph/search.rs and memory/search.rs use `&dyn ToSql` instead of `Box<dyn ToSql>` (P-005)
- `compute_string_facets` uses `HashMap<&str, _>` -- only unique keys allocate a String (P-007)
- `/health`, `/health/live`, `/health/ready` report `CARGO_PKG_VERSION` instead of hardcoded 0.1.0

### Fixed

- `kleos-cli store` sends importance as u8 0--10, matching server schema (was f32 0.0--1.0, causing 422)

### Security

- R8 audit: ingestion SSE relay has 5-minute timeout + heartbeat (R-006); LLM response errors surfaced instead of swallowed (R-007); context SSE logs on send failure (R-009); webhook emit logs on list error (R-011)
- R8 hardening: `validate_outbound_url` gate on sidecar's `engram_url`; non-unix `tighten_secret_perms` emits a warning; v1 API key acceptance emits counter + warning; webhook `deliver_with_retry` wrapped in panic-catching spawn with counters
- R7 audit: MCP write scope enforcement across 20+ handlers; SSRF hardening via `safe_client_builder`; bounded mpsc channels for ingestion/context progress; pepper fail-closed in release builds; YubiKey challenge-response rate limit; Content-Security-Policy baseline; PITR symlink-follow filter; GUI cookie future-timestamp rejection; CORS null origin dropped; plaintext http:// origin warning; SECURITY.md documents preauth rate limits
- Codex audit (F1--F9): sidecar partial-flush fix; trusted-proxy IP resolution shared between rate_limit and audit; migration validate returns Err on discrepancy; foreign_key_check after migration copy; /batch 207 semantics documented; CI hard-fails on missing deny.toml; Rust toolchain pinned to 1.94.0; credd shares build_router with tests; retry jitter arithmetic fixed
- Six CodeQL alerts closed: path injection in GUI secret path, four SSRF sites routed through `validate_outbound_url`; rustls-webpki bumped to 0.103.12
- Audit remediation sweep (14 tasks): PITR path traversal sandbox, atomic gate CAS, context fallback for unknown labels, ingestion session expiry propagation, libsql default-features trimmed, lru bumped 0.12 to 0.16, covering indices on memory_links, dead code removed from version-chain fetch, vector_sync batch helper, AppState embedder/reranker clone Arc out of RwLock, admin endpoint rate costs adjusted, dreamer/audit use monotonic clock, install.sh verifies SHA256 checksums

### Infrastructure

- Background task supervisor with factory-based respawn and exponential backoff, tied to shared CancellationToken for clean shutdown (R-008)
- Session reaper task evicts entries idle >1h with zero subscribers on a 60-second scan (R-010)
- Dreamer background consolidation: intelligence pipeline + dream_cycle + evolution per active user, idle-gated by audit middleware timestamp
- Gate hardening: brain-grounded check via Hopfield recall, DNS rebind/SSRF defense, agent allowlist, engram_stores enforcement
- `cognithor` module renamed to `cognitive`

## [0.3.2] - 2026-04-14

Full rename from Engram to Kleos.

### Changed

- All 10 workspace crates renamed `engram-*` to `kleos-*`
- Internal Rust identifiers swept from `engram_*` to `kleos_*`
- Binary names changed to `kleos-server`, `kleos-cli`, etc. with `engram-*` symlink aliases
- `KLEOS_*` environment variable prefix with `ENGRAM_*` fallback via `migrate_env_prefix()`
- Default database path changed to `kleos.db` with `engram.db` fallback
- Python SDK package renamed `engram-client` to `kleos-client`; Go SDK import path updated
- Dockerfile primary binary is `kleos-server`; CI publishes `kleos-*` binaries
- Repository URLs updated to github.com/Ghost-Frame/Kleos
- Install scripts, Homebrew formula, and flake.nix added

## [0.3.1] - 2026-04-14

### Fixed

- Dockerfile: added `libprotobuf-dev` to builder stage -- lance-encoding v4.0.0 needs protobuf descriptor files at build time
- Scoped crates.io publishing to `engram-lib` and `engram-cred` only (three other names held by third parties)

## [0.3.0] - 2026-04-14

### Added

- Python SDK (`sdk/python`) with sync and async clients, Pydantic v2 models, SSE support
- Go SDK (`sdk/go`) with stdlib-only typed sub-clients for 15 endpoint groups
- 16 structural + 4 skill MCP tools ported from the TypeScript codebase
- POST `/skills/:id/recompute` endpoint to reset rollup counters
- Resilience module: circuit breaker, retry with dead-letter recording, `ServiceGuard` wrapper
- Migration rollback support: `migrate_down`, `migration_status`, GET/POST `/admin/migrations`
- Per-route test harness (35 tests across health, memory, search, agents, auth_keys)
- Tracing instrumentation (`tracing::instrument`) across all public async functions in all crates
- Composite covering indices on `memory_links` for graph-expansion queries
- EXPLAIN QUERY PLAN documentation for five hot SQL paths

### Changed

- All 46 route modules restructured from flat files to `module/mod.rs` + `types.rs` folders
- Route-local DTOs moved from `mod.rs` into `types.rs` as `pub(super)`
- Release workflow split into staged builds to avoid OOM on free-tier CI runners
- `engram-mcp` added to release artifact collection

## [0.1.0] - 2026-04-13

Initial release. Ground-up Rust rewrite of the [TypeScript Engram](https://github.com/Ghost-Frame/Kleos).

### Added

- Core memory system: store, search, recall, update, forget, archive, delete
- 4-channel hybrid search: vector similarity (bge-m3), FTS5 full-text, personality signals, graph traversal fused via Reciprocal Rank Fusion
- FSRS-6 spaced repetition with power-law forgetting
- Knowledge graph: auto-linking, Louvain community detection, weighted PageRank, cooccurrence, structural analysis
- Personality engine: preferences, values, motivations, identity
- Atomic fact decomposition with contradiction detection
- Guardrails: stored rules return allow/warn/block on proposed agent actions
- Episodic memory as searchable narratives
- Bulk ingestion: Markdown, PDFs, chat exports, ZIP archives
- In-process ONNX embeddings: BAAI/bge-m3 (1024-dim) via `ort`
- Cross-encoder reranker: IBM granite-embedding-reranker-english-r2 INT8
- SQLite + LanceDB dual storage with FTS5
- Optional encryption at rest via SQLCipher (keyfile, env var, or YubiKey HMAC-SHA1)
- Coordination services: Axon, Broca, Chiasm, Soma, Loom, Thymus
- Multi-tenant RBAC with audit trail
- 80+ REST API endpoints
- `engram-cli`, `engram-mcp`, `engram-sidecar`, `engram-cred`/`engram-credd`
- Claude Code hooks for session memory and context injection
- TypeScript SDK, Docker support, GitHub Actions CI

### Architecture

- 10-crate Cargo workspace
- Tokio + Axum async runtime
- Single static binary, single SQLite database, local embeddings
- No cloud dependencies -- runs fully offline

[Unreleased]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Ghost-Frame/Kleos/compare/v0.1.0...v0.3.0
[0.1.0]: https://github.com/Ghost-Frame/Kleos/releases/tag/v0.1.0
