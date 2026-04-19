<div align="center">

<img src="docs/assets/banner.svg" width="800" alt="Kleos Banner" />

# Kleos

**Native Nervous System for Autonomous Agents. Built in Rust.**

> **High-integrity backbone for agentic reasoning, deterministic coordination, and associative memory. One binary. Total autonomy.**

[![License](https://img.shields.io/badge/License-Elastic--2.0-blue.svg)](LICENSE) [![Rust](https://img.shields.io/badge/rust-1.94%2B-orange.svg)](https://www.rust-lang.org) [![Native](https://img.shields.io/badge/Native-No%20Python-blueviolet.svg)](#)

</div>

<div align="center">

[Why Rust?](#why-rust) · [Quick Start](#quick-start) · [Features](#features) · [Workspace](#workspace) · [Architecture](#architecture) · [API](#api-reference) · [CLI](#cli) · [Sidecar](#sidecar) · [Config](#configuration) · [Wiki](https://github.com/Ghost-Frame/Kleos/wiki)

</div>

---

## Why Kleos?

Building agents on loose collections of Python scripts creates fragile systems. Kleos provides the structural integrity autonomous software requires.

- **Unified Cognitive OS.** Seven built-in coordination services including Axon for events, Chiasm for tasks, and Loom for workflows. These form a deterministic foundation for complex behavior.
- **High-Integrity Reasoning.** You enforce engineering standards with Agent-Forge. It uses Tree-sitter for symbol-aware repository mapping and mandates a Spec, Hypothesis, and Verify workflow to prevent chaos.
- **Native Performance and Security.** You get an async Tokio core and in-process ONNX models. No Python dependencies or massive Docker images. SQLCipher encryption and YubiKey authority keep data on your hardware.
- **Biologically-Inspired State.** Kleos manages state with FSRS-6 decay and background dream cycles. It replays and consolidates memory traces to keep knowledge stable and relevant.

---

## Quick Start

```bash
git clone https://github.com/Ghost-Frame/Kleos.git && cd Kleos
cargo build --release
./target/release/kleos-server
```

> Note: the repository will be renamed to `kleos-rust` in a future step. The clone URL above reflects the current name.

Server binds to `127.0.0.1:4200` by default. Set a bootstrap secret, start the server, then claim the admin key:

```bash
# Start the server with a bootstrap secret
KLEOS_BOOTSTRAP_SECRET=my-setup-secret ./target/release/kleos-server

# Bootstrap the admin key (one-time only)
curl -X POST http://localhost:4200/bootstrap \
  -H "Content-Type: application/json" \
  -d '{"secret": "my-setup-secret"}'

# Store a memory
curl -X POST http://localhost:4200/store \
  -H "Authorization: Bearer eg_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"content": "Production DB is PostgreSQL 16 on db.example.com:5432", "category": "reference"}'

# Search
curl -X POST http://localhost:4200/search \
  -H "Authorization: Bearer eg_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "database connection info"}'
```

MCP stdio entrypoint:

```bash
KLEOS_MCP_BEARER_TOKEN=eg_... cargo run -p kleos-mcp
```

---

![Kleos CLI demo](tools/cli-demo.gif)

![Kleos memory graph visualization](tools/gui-demo-v3.gif)

---

## The Ecosystem

| Component         | Role           | Highlights                                                        |
| ----------------- | -------------- | ----------------------------------------------------------------- |
| **Kleos-Server**  | The Brain      | FSRS-6, 4-channel Hybrid Search, 7 Coordination Services.         |
| **Agent-Forge**   | The Discipline | AST-aware Repo Mapping, Structured Reasoning, Adversarial Review. |
| **Kleos-MCP**     | The Interface  | 57+ tools for LLM integration via Model Context Protocol.         |
| **Kleos-Credd**   | The Shield     | Hardware-backed vault with YubiKey HMAC-SHA1 challenge-response.  |
| **Kleos-Sidecar** | The Guardian   | Session persistence with batched observation flushing.            |

---

## Features

- **FSRS-6 Spaced Repetition.** Memories strengthen with use and decay when ignored. Power-law forgetting with trained parameters.
- **4-Channel Hybrid Search.** Vector similarity, FTS5 full-text, personality signals, and graph traversal fused via Reciprocal Rank Fusion.
- **Knowledge Graph.** Auto-linking, Louvain community detection, weighted PageRank, cooccurrence, structural analysis.
- **Personality Engine.** Preferences, values, motivations, identity. Recall is shaped by the agent's current personality context.
- **Self-Hosted.** One Rust binary. One SQLite database. Local ONNX embeddings. No cloud keys.
- **Encryption at Rest.** SQLCipher database encryption with keyfile, environment variable, or YubiKey HMAC-SHA1 challenge-response.
- **Atomic Fact Decomposition.** Long memories split into self-contained facts. Each fact links back to its parent via `has_fact`.
- **Contradiction Detection.** When agents learn conflicting information, Engram surfaces the conflict.
- **Guardrails.** The gate system checks commands before agents act. Stored rules return allow/warn/block on proposed actions, with optional human-in-the-loop approval.
- **Episodic Memory.** Conversation episodes stored as searchable narratives.
- **Bulk Ingestion.** 11 parsers: Markdown, PDF, HTML, DOCX, CSV, JSONL, ZIP archives, ChatGPT exports, Claude exports, and raw message formats through the async ingestion pipeline.
- **LanceDB Vector Index.** Optional ANN backend for large corpora. Small tenants fall back to in-memory scan.
- **Claude Code Hooks.** Ready-to-use hooks for session memory, context injection, and tool tracking. See [`hooks/README.md`](hooks/README.md).

<details>
<summary><strong>Researcher Deep Dive: Technical Differentiators</strong></summary>

### 1. Neural Associative Recall

Unlike simple vector search, Kleos leverages in-process Hopfield Networks for pattern-based associative recall. The system reconstructs context from partial cues rather than just finding similar text.

### 2. Biological Memory Lifecycle

- **Dual-Strength Model**: Tracks storage stability against retrieval accessibility.
- **Consolidation**: Background workers replay and merge memory traces to optimize the knowledge graph while agents rest.
- **Evolution**: A self-improving weight system trains node importance based on retrieval feedback.

### 3. Hardware-is-Law Security

Kleos secures environments where cloud providers are liabilities. All embeddings run locally. You gate sensitive agent actions with physical YubiKey touch to ensure humans remain the ultimate authority.

_For full implementation details, algorithm specs, and API docs, see the [Project Wiki](https://github.com/Ghost-Frame/Kleos/wiki)._

</details>

---

## Workspace

Ten Cargo crates:

| Crate                | Role                                                                                                                                                                                                                                                                      |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kleos-lib`          | Core library. Memory, search, embeddings, graph, intelligence, services, auth, jobs, 50+ modules. Previously published as `engram-lib` (last: 0.3.1).                                                                                                                     |
| `kleos-server`       | Axum HTTP server. 46 route modules, middleware (auth, rate limiting, safe mode, JSON depth, metrics), GUI.                                                                                                                                                                |
| `kleos-cli`          | Command-line client over the HTTP API. Memory ops and credential management via credd.                                                                                                                                                                                    |
| `kleos-sidecar`      | Session-scoped memory proxy with file watcher, batched observation flushing, and persistent session store.                                                                                                                                                                |
| `kleos-mcp`          | MCP (Model Context Protocol) server. 57+ tools across memory, context, graph, intelligence, services, structural, skills, and admin. Stdio transport; HTTP behind feature flag.                                                                                           |
| `kleos-cred`         | Credential management library. Crypto primitives, YubiKey challenge-response, key derivation. Previously published as `engram-cred` (last: 0.3.1).                                                                                                                        |
| `kleos-credd`        | Credential management daemon. HTTP server with master key + agent key two-tier auth, ChaCha20-Poly1305 encryption.                                                                                                                                                        |
| `kleos-approval-tui` | Terminal UI for human approval workflow. Ratatui-based interactive review queue. (WIP)                                                                                                                                                                                    |
| `kleos-migrate`      | ETL tool for migrating from libsql to rusqlite + LanceDB. One-shot utility.                                                                                                                                                                                               |
| `agent-forge`        | Structured reasoning CLI: spec-task, consider-approaches, log-hypothesis, log-outcome, recall-errors, verify, challenge-code, checkpoint, rollback, session-learn, session-recall, session-diff, think, declare-unknowns, repo-map, search-code. Tree-sitter AST parsing. |

```bash
cargo build --release --workspace   # build everything
cargo test --workspace               # run the test suite
cargo clippy --workspace             # lint
```

---

<div align="center">

[Wiki](https://github.com/Ghost-Frame/Kleos/wiki) · [Issues](https://github.com/Ghost-Frame/Kleos/issues)

Elastic License 2.0

</div>
