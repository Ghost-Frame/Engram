# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [1.7.3] - 2026-06-22

### CI

- release: build and publish `kleos-ingest` and `kleos-cleanup` as linux-x64 release artifacts so operators have a binary to pull. The `kleos-ingest` installer component previously had no matching release asset.

## [1.7.2] - 2026-06-18

### Security

- server: SSRF-harden outbound webhook and LLM delivery so callbacks cannot reach internal or link-local addresses (#103).
- auth: protect the owner account from deprovisioning so the last admin cannot be locked out (#103).
- auth: require `read` scope on read-only POST endpoints instead of leaving them unscoped (#103).
- cred: require a confidential transport for credential bearer tokens (#103).
- credd: deny the proxy by default unless an explicit domain allowlist is configured (#103).

### Build

- hooks: pre-commit now auto-formats staged Rust so CI `cargo fmt --all -- --check` cannot fail on push.

## [1.7.1] - 2026-06-16

### Added

- MCP: clients can attach artifacts inline on `memory.store` and on read tools (#98).
- MCP: underscore-normalized tool-name aliases so strict clients such as VS Code can call every tool (#97).
- forge: agent-forge absorbed into Kleos as a server-side CLI plus MCP tool surface (#96).
- cli: inject coordination read-back at session start so agents see live task/feed state.
- cli: derive the session-start bootstrap query from the cwd project and git branch.
- db: bounded `run_migrations_to` / `run_tenant_migrations_to` helpers for partial migration runs.

### Fixed

- gate: forge-authorized Write/Edit now bypass the human-approval wait instead of blocking on it (#99).
- chiasm: reap stale never-heartbeated idle tasks, not only overdue ones.

## [1.7.0] - 2026-06-13

### Added

- Frameshift cross-machine growth tenant (server-side), gated behind `KLEOS_FRAMESHIFT_GROWTH` (#94).
- kleos-phylax: secret-resolve modes. `exec` runs an allowlisted command with secrets injected into the child process, and `verify`/`sign`/`derive` let an agent use a secret without ever holding its plaintext.
- kleos-phylax: no-plaintext agent posture backed by fail-closed policy middleware.
- kleos-phylax: out-of-band approval notification and a capability-token decide endpoint.
- kleos-cleanup: `--delete-where` escape hatch for operator-specific junk.

### Security

- Security audit remediation and monolith multi-user isolation hardening (#93).
- kleos-phylax: scrub-totality property tests and an adversarial plaintext-bypass test.

### Fixed

- recall: `is_static` memories now decay by age in ranking instead of being pinned at full retrievability. The flag is caller-set and hardcoded on consolidations, and had grown to ~43% of the store, so stale "permanent" memories dominated recall regardless of age or relevance. `is_static` still protects durability (no auto-prune) and gate guard lookups.
- kleos-sidecar: drop the orphaned `GateResult.original_text` field.
- gui: untrack stale `.svelte-kit` build artifacts and restore the ignore rule.

### CI

- Pin the Rust toolchain to 1.94.0 across CI, Docker, and local dev so unpinned `stable` upgrades no longer break `clippy -D warnings`, and serialize the Syntheos mirror workflow to stop concurrent force-pushes from failing on the ref-lock CAS.

### Database

- approvals: add `decide_token_hash` column (migration 85).

## [1.6.1] - 2026-06-08

### Fixed

- kleos-sidecar: stop over-ingesting raw session content. `retain_tool_calls` now defaults to false, so raw tool results are no longer promoted to durable storage, and the file-watcher gate stores only its distilled summary instead of appending the full raw assistant turn.
- eidolon living prompt: removed the duplicate `EIDOLON LIVING CONTEXT` banner (the brain block now renders as a `## Brain Context` sub-section), and credential-scrubbed the injected `Relevant Memories` section.
- kleos-cleanup: open raw-keyed SQLCipher databases correctly (raw-hex `PRAGMA key`), and add a high-precision pollution purge step plus a `--purge-only` flag.

## [1.1.1] - 2026-05-14

### Fixed

- Release workflow: set OPENSSL_DIR on Windows so bundled-sqlcipher links against preinstalled OpenSSL
- Release workflow: tolerate partial matrix failure so assets upload when at least one build leg succeeds
- Release workflow: rename checksum file to SHASUMS256.txt to match installer expectations
- Installer scripts: point at Ghost-Frame/Kleos instead of Ghost-Frame/Engram
- agent-forge: add bundled rusqlite for Windows where no system sqlite3.lib exists
- PowerShell installer: add SHA-256 checksum verification (parity with Unix installer)

## [1.1.0] - 2026-05-12

### Added

- `kleos-client` shared crate: route registry + signed HTTP client extracted from `kleos-cli` so MCP, CLI, and any future tool share the same auth and dispatch code
- `kleos-mcp` route registry expanded from 229 to 466 canonical routes (485 tools including back-compat aliases) -- full kleos-server HTTP surface is now reachable via MCP
- `kleos-mcp` HTTP transport (`--transport http --listen <addr>`) -- one sidecar per host serves MCP over JSON-RPC to multiple local clients
- `kleos-cli activity` subcommand -- PIV-signed `/activity` reporter for agents that prefer the CLI over direct HTTP
- `render_path` helper for path template substitution with safe percent-encoding

### Changed

- `kleos-mcp` is now a thin proxy: dispatches every tool call to kleos-server via signed HTTP rather than holding its own database/LLM connections. Schema and per-tool logic live in `kleos-client::routes::ROUTES`, not per-handler code
- `kleos-server` no longer exposes `POST /mcp/dispatch`; dispatch is owned exclusively by the kleos-mcp sidecar
- Default LLM route timeout raised from 60s to 200s; global request timeout raised to 630s (env-overridable) then to 1800s to match CPU-only LLM workloads
- README rewritten to position Kleos as cognitive infrastructure rather than a memory engine
- Homepage URLs updated from engram.syntheos.dev to kleos.syntheos.dev

### Fixed

- `render_path` now percent-encodes path-segment substitutions (CWE-74 / CWE-918): an MCP client cannot pivot the URL to another route via `/`, `?`, `#`, or `%` in a string argument; non-scalar (array/object/null) path args are rejected outright. Regression coverage added in `kleos-client/src/routes.rs` tests
- `get_with_timeout` / `post_with_timeout` in `kleos-client`: `reqwest::ClientBuilder::build().unwrap_or_default()` silently disarmed the per-call timeout on builder failure; builder errors now propagate
- Drop `gui.serve_app_assets` from the route registry: the axum wildcard template `/_app/{*path}` produced the property key `*path` in the tool input schema, which fails Anthropic's MCP regex `^[a-zA-Z0-9_.-]{1,64}$` and rejected the entire 485-tool list. Tool count is 484. New `registry_property_keys_match_mcp_regex` test prevents regression

### Security

- `kleos-mcp` HTTP transport drops front-door bearer auth -- reachability (bind address) is the access-control boundary, consistent with kleos-cli. Operators must bind to a private interface (loopback, LAN, mesh)
- `kleos-mcp` hard-fails at startup if neither a PIV signer nor `KLEOS_API_KEY` is configured -- refuses to start unauthenticated

## [1.0.0] - 2026-05-10

### Added

- Skills Cloud v50: aliases, bundles, materializations, hybrid find with weighted tag/text/semantic search
- Plugin importer for YAML-based skill files (`kleos-cli import-plugins`)
- User management with enrollment invites and active status tracking
- MCP schema endpoint (auth-gated) and dispatch routes with scope-gated tool categories
- Identity key rotation and metadata support
- Agent tool runtime crate (`forge`) -- skill-driven dispatch for Kleos-authenticated tools

### Changed

- All 16 workspace crates bumped to 1.0.0
- Artifact ownership model cleanup
- reqwest workspace dep gains `multipart` feature

### Fixed

- Duplicate `/users` route registration that caused a panic on startup

### Security

- MCP schema endpoint moved behind auth middleware
- Scope-gated dispatch (Admin/Write/Read per tool category)
- Fixed greedy YAML prefix matching in plugin importer
- SkillKind validation on create and update

## [0.4.0] - 2026-05-08

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

[Unreleased]: https://github.com/Ghost-Frame/Kleos/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Ghost-Frame/Kleos/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Ghost-Frame/Kleos/compare/v0.1.0...v0.3.0
[0.1.0]: https://github.com/Ghost-Frame/Kleos/releases/tag/v0.1.0
