<div align="center">

# Engram (Rust)

**The cognitive layer for AI agents. Rewritten in Rust.**

[![License](https://img.shields.io/badge/License-Elastic--2.0-blue.svg)](LICENSE) [![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org) [![Workspace](https://img.shields.io/badge/workspace-10%20crates-brightgreen.svg)](Cargo.toml)

</div>

<div align="center">

[Why Rust?](#why-rust) · [Quick Start](#quick-start) · [Features](#features) · [Workspace](#workspace) · [Architecture](#architecture) · [API](#api-reference) · [CLI](#cli) · [Sidecar](#sidecar) · [Config](#configuration)

</div>

---

## Why Rust?

Engram started as a Node.js server. It worked. But a memory system that injects into every agent turn has to stay fast under load, and Node made that harder than it should have been.

This repo is the ground-up Rust port. Same cognitive model, same API, same data format. Different runtime.

- **Single static binary.** `cargo build --release` gives you one file. No Node, no `node_modules`, no flags.
- **Tokio + Axum.** Async from the socket down to SQLite. Thousands of concurrent agent requests on a small VPS.
- **In-process ONNX.** `ort` runs embeddings and the cross-encoder reranker inside the server process. No Python, no worker threads, no sidecar model server.
- **SQLite + LanceDB.** rusqlite (with optional SQLCipher) holds relational memory and FTS5. LanceDB holds the vector index once the corpus outgrows memory.
- **One workspace.** Library, server, CLI, and sidecar build from one `cargo` command.

One binary. One SQLite database. Local embeddings. No OpenAI key. Your hardware, your data.

The TypeScript engram remains the reference design. This is the runtime.

---

## Quick Start

```bash
git clone https://github.com/Ghost-Frame/Engram-rust.git && cd engram-rust
cargo build --release
./target/release/engram-server
```

Server binds to `127.0.0.1:4200` and writes to `./data` by default. Bootstrap an admin key, then store and search:

```bash
curl -X POST http://localhost:4200/bootstrap \
  -H "Content-Type: application/json" \
  -d '{"name": "admin"}'

curl -X POST http://localhost:4200/store \
  -H "Authorization: Bearer eg_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"content": "Production DB is PostgreSQL 16 on db.example.com:5432", "category": "reference"}'

curl -X POST http://localhost:4200/search \
  -H "Authorization: Bearer eg_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "database connection info"}'
```

The Rust server keeps the TypeScript API surface. Existing SDKs, MCP clients, and CLI wrappers point at the new binary without changes.

MCP stdio entrypoint: `ENGRAM_MCP_BEARER_TOKEN=engram_... cargo run -p engram-mcp`

---

![Engram CLI demo](tools/cli-demo.gif)

![Engram memory graph visualization](tools/gui-demo-v3.gif)

---

## Features

- **FSRS-6 Spaced Repetition.** Memories strengthen with use and fade when ignored. Power-law forgetting with trained parameters.
- **4-Channel Hybrid Search.** Vector similarity, FTS5 full-text, personality signals, and graph traversal fused via Reciprocal Rank Fusion.
- **Knowledge Graph.** Auto-linking, Louvain community detection, weighted PageRank, cooccurrence, structural analysis.
- **Personality Engine.** Preferences, values, motivations, identity. Every recall shapes around who the agent is talking to.
- **Self-Hosted.** One Rust binary. One SQLite database. Local ONNX embeddings. No cloud keys.
- **Encryption at Rest.** SQLCipher database encryption with keyfile, environment variable, or YubiKey HMAC-SHA1 challenge-response.
- **Atomic Fact Decomposition.** Long memories split into self-contained facts. Each fact links back to its parent via `has_fact`.
- **Contradiction Detection.** When agents learn conflicting information, Engram surfaces the conflict.
- **Guardrails.** Agents check before they act. Stored rules return allow/warn/block on proposed actions.
- **Episodic Memory.** Conversation episodes stored as searchable narratives.
- **Bulk Ingestion.** Markdown, PDFs, chat exports, ZIP archives through the async ingestion pipeline.
- **LanceDB Vector Index.** Optional ANN backend for large corpora. Small tenants fall back to in-memory scan.
- **Claude Code Hooks.** Ready-to-use hooks for session memory, context injection, and tool tracking. See [`hooks/README.md`](hooks/README.md).

<details>
<summary><strong>Full Capabilities</strong></summary>

### Smart Memory
- **Dual-Strength Model** (Bjork & Bjork): storage strength never decays, retrieval strength resets on access
- **Versioning**: update memories without losing history, full version chain preserved per user
- **Auto-Deduplication**: SimHash 64-bit locality-sensitive hashing catches near-identical memories
- **Auto-Forget / TTL**: set memories to expire, background sweep via async job worker

### Intelligence Layer
- **Fact Extraction**: structured facts with temporal validity windows (`valid_at`, `invalid_at`)
- **Conversation Extraction**: feed raw chat logs, get structured memories
- **Reflections & Consolidation**: meta-analysis and cluster compression, non-destructive
- **Causal and Valence**: detect cause/effect links and emotional charge on new memories
- **Growth**: self-improving observations from agent activity, stored as growth memories

### Developer Platform
- **REST API**: 80+ endpoints, drop-in compatible with the reference engram
- **Rust CLI**: `engram-cli` for store, search, context, recall, list, bootstrap
- **MCP Server**: `engram-mcp` for LLM tool integration via Model Context Protocol (stdio or HTTP)
- **Sidecar**: `engram-sidecar` for session-scoped agent runs
- **Credential Manager**: `engram-cred` CLI + `engram-credd` daemon for encrypted credential vault with YubiKey support
- **Multi-Tenant + RBAC**: isolated memory per user, role-based access, quota enforcement
- **Webhooks & Digests**: event hooks and scheduled digests
- **Audit Trail**: every mutation logged with who, what, when, from where
- **Scratchpad**: ephemeral working memory with TTL auto-purge

### Coordination Services
Engram bundles a set of coordination services behind the same auth and database:

- **Axon**: event bus with channels, subscriptions, SSE streaming, cursor polling
- **Brain**: cross-service orchestration primitives
- **Broca**: structured action log with auto-narration
- **Chiasm**: task tracking with audit trails
- **Loom**: workflow orchestration with step callbacks
- **Soma**: agent registry, heartbeats, capability search, groups
- **Thymus**: rubric-driven quality evaluation and metrics

### Organization
- **Graph Endpoints**: full memory graph (nodes + edges), timeline, community browse
- **Entities & Projects**: people, servers, tools, projects
- **Review Queue / Inbox**: approve, reject, or edit before memories enter recall
- **Community Detection**: label propagation surfaces memory clusters
- **PageRank**: structural importance boosts search results

</details>

---

## Workspace

Ten Cargo crates:

| Crate | Role |
|-------|------|
| `engram-lib` | Core library. Memory, search, embeddings, graph, intelligence, services, auth, jobs. |
| `engram-server` | Axum HTTP server. Binds routes to library functions. Handles middleware, auth, rate limiting, GUI. |
| `engram-cli` | Command-line client over the HTTP API. |
| `engram-sidecar` | Session-scoped memory proxy with file watcher and local caching. Talks to `engram-server`. |
| `engram-mcp` | MCP (Model Context Protocol) stdio and HTTP entrypoint for LLM tool integration. |
| `engram-cred` | Credential management CLI with encrypted vault and YubiKey support. |
| `engram-credd` | Credential management daemon. Serves credentials over local HTTP with session auth. |
| `engram-approval-tui` | Terminal UI for human approval workflow. Ratatui-based interactive review queue. (WIP) |
| `engram-migrate` | ETL tool for migrating from libsql to rusqlite + LanceDB. |
| `agent-forge` | Agent-facing protocol helpers: spec, hypothesis, verify primitives. Tree-sitter AST parsing. |

```bash
cargo build --release --workspace   # build everything
cargo test --workspace               # run the test suite
cargo clippy --workspace             # lint
```

---

<a id="architecture"></a>
<details>
<summary><strong>Architecture</strong></summary>

### Runtime Stack

- Server: Axum 0.8 on Tokio, `tower-http` for tracing and CORS
- Database: rusqlite with FTS5 and optional SQLCipher encryption, behind an async connection pool
- Vector index: LanceDB (optional, toggled via `use_lance_index`)
- Embeddings: BAAI/bge-m3, 1024-dim, ONNX via `ort` with the `tokenizers` crate
- Reranker: IBM granite-embedding-reranker-english-r2 INT8 cross-encoder (optional)
- Decay: FSRS-6 with trained parameters and power-law forgetting
- LLM: optional. Any OpenAI-compatible endpoint for fact extraction, decomposition, consolidation

### Search Pipeline

Every query fans out across four channels, then merges via Reciprocal Rank Fusion:

1. **Vector similarity**: cosine against bge-m3 embeddings. LanceDB ANN when enabled, in-memory scan otherwise.
2. **FTS5 full-text**: BM25 across content and tags.
3. **Personality signals**: match against extracted preferences, values, identity markers.
4. **Graph relationships**: 2-hop traversal weighted by edge type and PageRank.

Question-type detection (fact recall, preference, reasoning, generalization, timeline) reweights the channels before scoring. The ONNX cross-encoder reranks the top-K for semantic precision.

### Memory Lifecycle

1. **Store**: SimHash checks for near-duplicates. Unique memories get embedded by `ort` and written to SQLite with FTS5 indexing and an optional LanceDB insert.
2. **Auto-link**: the new memory gets compared against existing ones via cosine similarity. Typed edges form: similarity, updates, extends, contradicts, caused_by, prerequisite_for.
3. **FSRS-6 init**: each memory receives starting stability, difficulty, storage strength, retrieval strength.
4. **Fact extraction**: when an LLM is configured, structured facts get pulled with temporal validity windows.
5. **Atomic decomposition**: long memories split into self-contained facts linked to the parent via `has_fact`. The parent stays intact.
6. **Entity cooccurrence**: entities in the same memory update the weighted cooccurrence graph.
7. **Personality extraction**: six signal types scanned: preference, value, motivation, decision, emotion, identity.
8. **Recall**: RRF fuses four channels. Every recalled memory receives an implicit FSRS review graded "Good".
9. **Spaced repetition**: archived or forgotten memories receive "Again". Stable memories can reach months or years between reviews.
10. **Dual-strength decay**: storage strength accumulates. Retrieval strength decays via power law.
11. **Community detection and PageRank**: background workers rerun Louvain grouping and weighted PageRank on a schedule and on dirty-edge thresholds.

### Security

The Rust port closes boundaries the TypeScript version left open:

- `user_id` scoping on every query, including parent lookups during versioning
- Transactional store/update paths with forward-compatible migrations
- FTS5 query sanitization against DoS and injection
- Rate limiting layered inside auth so the limiter sees the resolved tenant
- Bearer-token auth with scope enforcement and audit logging on every mutation

### ASCII Diagram

```
┌──────────────────────────────────────────────────────┐
│                 engram-server (Axum)                 │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │  FSRS-6  │  │   RRF    │  │  FTS5    │            │
│  │  Engine  │  │  Scorer  │  │  Search  │            │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘            │
│       │             │             │                  │
│  ┌────┴─────────────┴─────────────┴────┐             │
│  │  SQLite/SQLCipher + FTS5            │             │
│  │  + LanceDB (vector ANN, optional)   │             │
│  └─────────────────────────────────────┘             │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │ ort/ONNX │  │ Reranker │  │  Graph   │            │
│  │  bge-m3  │  │ (Granite)│  │  Engine  │            │
│  └──────────┘  └──────────┘  └──────────┘            │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │ SimHash  │  │Personality│ │ Temporal │            │
│  │  Dedup   │  │  Engine   │ │  Facts   │            │
│  └──────────┘  └──────────┘  └──────────┘            │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │  Atomic  │  │  Async   │  │Consolida-│            │
│  │  Decomp  │  │ Job Pool │  │  tion    │            │
│  └──────────┘  └──────────┘  └──────────┘            │
│                                                      │
│  Services: axon · brain · broca · chiasm ·           │
│  loom · soma · thymus                                │
└──────────────────────────────────────────────────────┘
```

</details>

<a id="api-reference"></a>
<details>
<summary><strong>API Reference</strong></summary>

Every endpoint needs `Authorization: Bearer eg_...` unless the server runs in open-access mode. Shapes match the reference engram: OpenAPI clients, SDKs, and MCP integrations work without changes.

### Core

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/store` | Store a memory |
| `POST` | `/search` | RRF search across vector, FTS5, personality, and graph channels |
| `POST` | `/context` | Budget-aware context assembly for RAG |
| `GET` | `/list` | List recent memories |
| `GET` | `/graph` | Full memory graph (nodes + edges) |

### Memory Management

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/memory/:id/update` | Create new version |
| `POST` | `/memory/:id/forget` | Soft delete |
| `POST` | `/memory/:id/archive` | Archive (hidden from recall) |
| `POST` | `/memory/:id/unarchive` | Restore from archive |
| `DELETE` | `/memory/:id` | Permanent delete |
| `GET` | `/versions/:id` | Version chain for a memory |

### FSRS-6

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/fsrs/review` | Manual review (grade 1-4: Again/Hard/Good/Easy) |
| `GET` | `/fsrs/state` | Retrievability, stability, next review interval |
| `POST` | `/fsrs/init` | Backfill FSRS state for all memories |

### Intelligence

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/add` | Extract memories from conversations |
| `POST` | `/ingest` | Extract facts from URLs, text, or files |
| `POST` | `/guard` | Pre-action guardrail check (allow/warn/block) |
| `POST` | `/reflect` | Generate period or growth reflection |
| `GET` | `/contradictions` | Find conflicting memories |
| `GET` | `/facts` | Query structured facts |
| `GET` | `/memory-health` | Diagnostic report: stale, duplicates, unlinked |
| `POST` | `/feedback` | Submit retrieval feedback |

### Graph and Organization

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/communities` | List memory communities |
| `GET` | `/graph/timeline` | Weekly graph growth |
| `GET` | `/tags` | List all tags |
| `POST` | `/episodes` | Create episode |
| `POST` | `/entities` | Create entity |
| `POST` | `/projects` | Create project |

### Coordination Services

| Prefix | Service | Highlights |
|--------|---------|------------|
| `/axon/*` | Event bus | publish, subscribe, SSE stream, cursor poll |
| `/broca/*` | Action log | log, narrate, feed, natural-language query |
| `/soma/*` | Agent registry | register, heartbeat, capability search, groups |
| `/thymus/*` | Quality eval | rubrics, scored evaluations, metrics |
| `/loom/*` | Workflows | definitions, runs, step callbacks |
| `/tasks/*` | Chiasm tasks | create, update, audit trail, feed |

### Platform and Admin

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/users` | Create user (admin) |
| `POST` | `/keys` | Create API key |
| `POST` | `/keys/rotate` | Rotate API key atomically |
| `POST` | `/spaces` | Create scoped memory space |
| `GET` | `/admin/quotas` | View tenant quotas |
| `POST` | `/admin/reembed` | Re-embed with current provider |
| `POST` | `/admin/rebuild-fts` | Rebuild FTS5 index |
| `POST` | `/admin/detect-communities` | Run Louvain community detection |
| `POST` | `/admin/decompose-sweep` | Retroactively decompose memories |
| `POST` | `/admin/compact` | VACUUM and ANALYZE |
| `GET` | `/audit` | Query audit log |
| `GET` | `/stats` | Detailed statistics |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/openapi.json` | OpenAPI 3.1 spec |

The full endpoint list (inbox, sync, webhooks, digests, pack, prompts, scratchpad, gate, grounding, growth, sessions, agents, artifacts, portability, onboarding, docs) lives in `engram-server/src/server.rs` and `engram-server/src/routes/`.

</details>

<a id="cli"></a>
<details>
<summary><strong>CLI</strong></summary>

`engram-cli` wraps the HTTP API. Same workspace, same build.

```bash
cargo build --release -p engram-cli
# or
cargo run -p engram-cli -- --help
```

```bash
export ENGRAM_URL=http://localhost:4200
export ENGRAM_API_KEY=eg_your_key
```

```bash
engram-cli store "Deployed auth migration to production" --category state --importance 0.9
engram-cli search "deployment history" --limit 5
engram-cli context "current infrastructure state" --limit 5
engram-cli recall 42
engram-cli guard "rm -rf /opt/engram/data"
engram-cli list --limit 20
engram-cli delete 42
engram-cli bootstrap
```

Every command takes `--server` and `--key` overrides, or reads `ENGRAM_URL` / `ENGRAM_API_KEY` from the environment.

</details>

<a id="sidecar"></a>
<details>
<summary><strong>Sidecar</strong></summary>

`engram-sidecar` runs next to a single agent session. It proxies requests to the main Engram server while providing:

- **Session-scoped context** with local caching for fast recall
- **File watcher** for Claude Code session JSONL files (auto-extracts memories)
- **LLM re-probe** for analyzing conversation context
- **Instant flush** to push memories to the server on demand

```bash
ENGRAM_URL=http://localhost:4200 \
ENGRAM_API_KEY=eg_your_key \
engram-sidecar --session-id my-session --watch
```

The sidecar is a lightweight HTTP service that talks to `engram-server` for storage and search. It doesn't run embeddings locally.

</details>

<a id="configuration"></a>
<details>
<summary><strong>Configuration</strong></summary>

`engram_lib::config::Config` reads environment variables. Defaults live in `engram-lib/src/config.rs`.

### Core

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_HOST` | `127.0.0.1` | Bind address |
| `ENGRAM_PORT` | `4200` | Server port |
| `ENGRAM_DB_PATH` | `engram.db` | SQLite database file |
| `ENGRAM_DATA_DIR` | `./data` | Data directory for models, LanceDB, artifacts |
| `ENGRAM_API_KEY` | unset | Bootstrap admin key override |
| `ENGRAM_GUI_PASSWORD` | unset | GUI login password |
| `RUST_LOG` | `info` | `tracing-subscriber` filter: `debug`, `info`, `warn`, `error` |

### Embeddings and Reranker

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_EMBEDDING_DIM` | `1024` | Embedding dimension |
| `ENGRAM_EMBEDDING_MODEL` | `BAAI/bge-m3` | Embedding model name |
| `ENGRAM_EMBEDDING_MODEL_DIR` | auto | Override ONNX model directory (must hold `tokenizer.json` + ONNX file) |
| `ENGRAM_EMBEDDING_ONNX_FILE` | `model_quantized.onnx` | Model filename inside the model dir |
| `ENGRAM_EMBEDDING_MAX_SEQ` | `512` | Max token sequence length |
| `ENGRAM_RERANKER_ENABLED` | `1` | Set `0` to disable cross-encoder reranking |
| `ENGRAM_RERANKER_TOP_K` | `12` | Rerank top K candidates |
| `ENGRAM_USE_LANCE_INDEX` | `1` | Set `0` to disable the LanceDB vector backend |

### LLM

Set `LLM_URL`, `LLM_API_KEY`, and `LLM_MODEL` for any OpenAI-compatible provider. The LLM drives fact extraction, decomposition, consolidation, and growth reflection. The core memory pipeline runs without one.

### PageRank and Graph

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_PAGERANK_ENABLED` | `1` | Set `0` to skip background PageRank refresh |
| `ENGRAM_PAGERANK_REFRESH_INTERVAL_SECS` | `300` | Worker refresh cadence |
| `ENGRAM_PAGERANK_DIRTY_THRESHOLD` | `100` | Dirty-edge count that forces a refresh |
| `ENGRAM_PAGERANK_MAX_CONCURRENT` | `2` | Max concurrent PageRank workers |

### Encryption at Rest

Engram supports SQLCipher for database encryption. Set `ENGRAM_ENCRYPTION_MODE` to one of:

| Mode | Description |
|------|-------------|
| `none` | No encryption (default). Database opens without `PRAGMA key`. |
| `keyfile` | Read a 32-byte raw key from `~/.config/engram/dbkey`. File must be `chmod 600` on Unix. |
| `env` | Read `ENGRAM_DB_KEY` environment variable (64 hex characters = 32 bytes). |
| `yubikey` | Derive key from YubiKey HMAC-SHA1 challenge-response. Requires `engram-credd`. |

**Keyfile setup:**

```bash
head -c 32 /dev/urandom > ~/.config/engram/dbkey
chmod 600 ~/.config/engram/dbkey
ENGRAM_ENCRYPTION_MODE=keyfile engram-server
```

**Environment variable setup:**

```bash
export ENGRAM_DB_KEY=$(head -c 32 /dev/urandom | xxd -p -c 64)
ENGRAM_ENCRYPTION_MODE=env engram-server
```

**YubiKey setup:**

YubiKey mode uses HMAC-SHA1 challenge-response (slot 2) to derive the encryption key via Argon2id. The YubiKey must be present to unlock the database. This is the strongest option for single-user deployments.

```bash
# Program YubiKey slot 2 with a secret (one-time setup)
ykman otp chalresp --generate 2

# Start the credential daemon (unlocks on first use)
engram-credd --encryption-mode yubikey

# Server connects to credd for key material
ENGRAM_ENCRYPTION_MODE=yubikey engram-server
```

The key derivation uses Argon2id (64 MiB memory, 3 iterations) so cold starts take ~1 second. Once unlocked, the key stays in memory until the process exits.

See `engram-lib/src/config.rs` for the full set, including decomposition tunables, search floors, and Eidolon integration flags.

</details>

---

## Development

```bash
cargo check --workspace                 # verify it compiles
cargo test --workspace                  # run tests (in-memory SQLite)
cargo test -p engram-lib                # library only
cargo test -p engram-server             # server only
cargo clippy --workspace -- -D warnings # lint
```

[`CLAUDE.md`](CLAUDE.md) has the import-path reference, workspace conventions, and the patterns this codebase follows: async, ownership, error handling, route signatures.

---

## Relationship to the Reference Engram

This repo is the Rust port of the TypeScript engram. API shapes, database schemas, and behavior stay compatible so existing clients, MCP tools, and data migrate without rewrites. The reference implementation holds the design spec. New features land in Rust first once the runtime catches up.

---

<div align="center">

[engram.lol](https://engram.lol) · [Reference implementation (GitHub)](https://github.com/Ghost-Frame/engram)

Elastic License 2.0

</div>
