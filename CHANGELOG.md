# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-04-13

Initial public release. Ground-up Rust rewrite of the [TypeScript Engram](https://github.com/GhostFrame/Kleos).

### Added

- **Core memory system**: store, search, recall, update, forget, archive, delete
- **4-channel hybrid search**: vector similarity (bge-m3), FTS5 full-text, personality signals, and graph traversal fused via Reciprocal Rank Fusion
- **FSRS-6 spaced repetition**: memories strengthen with use, fade when ignored, power-law forgetting with trained parameters
- **Knowledge graph**: auto-linking, Louvain community detection, weighted PageRank, cooccurrence, structural analysis
- **Personality engine**: preferences, values, motivations, identity -- every recall shapes around who the agent is talking to
- **Atomic fact decomposition**: long memories split into self-contained facts linked to parent via `has_fact`
- **Contradiction detection**: surfaces conflicting information across the memory store
- **Guardrails**: agents check before they act -- stored rules return allow/warn/block on proposed actions
- **Episodic memory**: conversation episodes stored as searchable narratives
- **Bulk ingestion**: Markdown, PDFs, chat exports, ZIP archives through the async pipeline
- **In-process ONNX embeddings**: BAAI/bge-m3 (1024-dim) via `ort` -- no external API calls
- **Cross-encoder reranker**: IBM granite-embedding-reranker-english-r2 INT8 (optional)
- **SQLite + LanceDB**: rusqlite with FTS5 for relational memory, LanceDB for vector ANN at scale
- **Encryption at rest**: SQLCipher with keyfile, environment variable, or YubiKey HMAC-SHA1 challenge-response (off by default)
- **Coordination services**: Axon (event bus), Broca (action log), Chiasm (task tracking), Soma (agent registry), Loom (workflows), Thymus (quality eval)
- **Multi-tenant RBAC**: isolated memory per user, role-based access, quota enforcement
- **Audit trail**: every mutation logged with who, what, when, from where
- **REST API**: 80+ endpoints, drop-in compatible with the TypeScript reference
- **CLI**: `engram-cli` for store, search, context, recall, list, bootstrap, guard
- **MCP server**: `engram-mcp` for LLM tool integration via Model Context Protocol (stdio or HTTP)
- **Sidecar**: `engram-sidecar` for session-scoped memory with file watcher and local caching
- **Credential manager**: `engram-cred` CLI + `engram-credd` daemon with encrypted vault and YubiKey support
- **Claude Code hooks**: ready-to-use hooks for session memory, context injection, and tool tracking
- **TypeScript SDK**: `@engram/sdk` client library
- **Docker support**: multi-stage Dockerfile for containerized deployment
- **GitHub Actions CI**: build, test, clippy, format checks on every push

### Architecture

- 10-crate Cargo workspace: engram-lib, engram-server, engram-cli, engram-sidecar, engram-mcp, engram-cred, engram-credd, engram-approval-tui, engram-migrate, agent-forge
- Tokio + Axum async runtime
- Single static binary, single SQLite database, local embeddings
- No cloud dependencies -- runs fully offline

[0.1.0]: https://github.com/GhostFrame/Kleos/releases/tag/v0.1.0
