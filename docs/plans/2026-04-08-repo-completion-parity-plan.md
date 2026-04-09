# Engram Rust Completion Parity Plan

**Goal:** Close the highest-value parity gaps between `engram-rust`, `C:\Users\Zan\Projects\engram`, and the Eidolon behaviors currently expected by the ecosystem in `C:\Users\Zan\Projects\eidolon`.

**Architecture:** Treat `engram` as the source of truth for the core memory server, data model, and HTTP/CLI contract. Treat `eidolon` as the source of truth for brain-adjacent agent workflows: activity fan-out, gate enforcement, growth reflection, prompt generation, and session streaming. Port by dependency order: schema first, library logic second, route layer third, then verification and cleanup.

**Tech Stack:** Rust workspace (`engram-lib`, `engram-server`, `engram-cli`, `engram-sidecar`), `axum`, `tokio`, `libsql`, ONNX Runtime, source repos `engram` (TypeScript) and `eidolon` (Rust).

---

## Source Of Truth Map

`engram` is authoritative for:
- Memory CRUD/search/store behavior
- Context assembly and prompt/header generation
- Graph, intelligence, ingestion, FSRS, grounding, artifacts, agents, projects, inbox, webhooks
- Service endpoints: Chiasm, Axon, Broca, Soma, Loom, Thymus, Brain
- DB schema in `src/db/schema/{base,episodes,fts,intelligence,migrations,services,tier4}.ts`

`eidolon` is authoritative for:
- `/activity` unified fan-out
- `/gate/*` command validation, approvals, secret resolution, scrubbing, SSH/systemctl enrichment
- `/growth/*` reflection, observations, materialization
- `/sessions/*` streaming session output
- `/prompt/generate`
- Neural substrate and daemon-side brain integration patterns

## Current Snapshot

Already in place:
- Broad route/module wiring in [`engram-server/src/routes/mod.rs`](C:\Users\Zan\Projects\engram-rust\engram-server\src\routes\mod.rs) and [`engram-lib/src/lib.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\lib.rs)
- Working service families for Chiasm, Axon, Broca, Soma, Loom, Thymus, Brain
- CLI/server compatibility fixes already landed
- Sidecar moved off placeholder handlers and onto direct DB-backed routes

Still partial or missing:
- Hard `todo!()` in [`engram-lib/src/audit.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\audit.rs), [`engram-lib/src/guard.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\guard.rs), [`engram-lib/src/graph/builder.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\graph\builder.rs), [`engram-lib/src/graph/cooccurrence.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\graph\cooccurrence.rs), [`engram-lib/src/graph/search.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\graph\search.rs), [`engram-lib/src/intelligence/consolidation.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\intelligence\consolidation.rs), [`engram-lib/src/intelligence/contradiction.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\intelligence\contradiction.rs), [`engram-lib/src/db/migrations.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\db\migrations.rs)
- Stubbed or incomplete ingestion/parser paths in [`engram-lib/src/ingestion/parsers/pdf.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\ingestion\parsers\pdf.rs), [`engram-lib/src/ingestion/parsers/docx.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\ingestion\parsers\docx.rs), [`engram-lib/src/ingestion/parsers/zip.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\ingestion\parsers\zip.rs), [`engram-lib/src/skills/cloud.rs`](C:\Users\Zan\Projects\engram-rust\engram-lib\src\skills\cloud.rs)
- Stubbed middleware in [`engram-server/src/middleware/audit.rs`](C:\Users\Zan\Projects\engram-rust\engram-server\src\middleware\audit.rs) and [`engram-server/src/middleware/rate_limit.rs`](C:\Users\Zan\Projects\engram-rust\engram-server\src\middleware\rate_limit.rs)
- Entire route families missing from the Rust server even though the upstream repos expose them

## Missing Route Families

Missing from `engram-rust` but present in `engram`:
- `agents` from `C:\Users\Zan\Projects\engram\src\agents\routes.ts`
- `artifacts` from `C:\Users\Zan\Projects\engram\src\artifacts\routes.ts`
- split auth-key routes from `C:\Users\Zan\Projects\engram\src\auth-keys\routes.ts`
- `fsrs` from `C:\Users\Zan\Projects\engram\src\fsrs\routes.ts`
- `grounding` from `C:\Users\Zan\Projects\engram\src\grounding\routes.ts`
- `search` from `C:\Users\Zan\Projects\engram\src\search\routes.ts`
- `docs` and `openapi` export surface from `C:\Users\Zan\Projects\engram\src\docs\routes.ts` and `C:\Users\Zan\Projects\engram\src\openapi.ts`
- `onboard` from `C:\Users\Zan\Projects\engram\src\onboard\routes.ts`

Missing from `engram-rust` but present in `eidolon`:
- `activity` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\activity.rs`
- `gate` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\gate.rs`
- `growth` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\growth.rs`
- `sessions` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\sessions.rs`
- `prompt/generate` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\prompt.rs`

## Recommended Execution Order

1. Schema and migrations
2. Missing Engram route families that already have Rust lib support
3. Stubbed core logic: graph, intelligence, audit, guard
4. Ingestion and embedding/reranker parity
5. Eidolon route families and daemon behaviors
6. Middleware, auth, and verification hardening

### Task 1: Finish Schema And Migration Parity

**Files:**
- Modify: `engram-lib/src/db/schema.rs`
- Modify: `engram-lib/src/db/migrations.rs`
- Inspect against: `engram/src/db/schema/base.ts`
- Inspect against: `engram/src/db/schema/episodes.ts`
- Inspect against: `engram/src/db/schema/fts.ts`
- Inspect against: `engram/src/db/schema/intelligence.ts`
- Inspect against: `engram/src/db/schema/services.ts`
- Inspect against: `engram/src/db/schema/tier4.ts`

- [ ] Diff the TypeScript schema files against the Rust schema builder and write a gap list by table, index, trigger, and column.
- [ ] Replace the `todo!()` migration entrypoint with ordered Rust migrations that preserve the TypeScript schema names and data semantics.
- [ ] Add migrations for service tables, intelligence/tier4 tables, and any missing artifacts/agents/grounding/FSRS support tables.
- [ ] Add migration verification tests in `engram-lib` that create an empty DB, run migrations once, run them a second time, and assert idempotence.
- [ ] Run `cargo check --workspace --offline`.
- [ ] Run targeted tests once `link.exe` is available: `cargo test -p engram-lib db:: --offline`.

### Task 2: Expose Missing Engram Route Families

**Files:**
- Create: `engram-server/src/routes/agents.rs`
- Create: `engram-server/src/routes/artifacts.rs`
- Create: `engram-server/src/routes/auth_keys.rs`
- Create: `engram-server/src/routes/fsrs.rs`
- Create: `engram-server/src/routes/grounding.rs`
- Create: `engram-server/src/routes/search.rs`
- Create: `engram-server/src/routes/docs.rs`
- Create: `engram-server/src/routes/onboard.rs`
- Modify: `engram-server/src/routes/mod.rs`
- Modify: `engram-server/src/server.rs`
- Modify as needed: `engram-lib/src/agents.rs`
- Modify as needed: `engram-lib/src/artifacts.rs`
- Modify as needed: `engram-lib/src/apikeys.rs`
- Modify as needed: `engram-lib/src/fsrs/mod.rs`
- Modify as needed: `engram-lib/src/grounding/{mod.rs,client.rs,search.rs,quality.rs,shell.rs}`

- [ ] Port the HTTP path surface from the matching TypeScript route files without inventing new payload shapes.
- [ ] Reuse the existing `security.rs` only for internal consolidation if the response contracts remain identical. If they do not, keep separate route files.
- [ ] Make `search` a first-class route family rather than hiding everything behind `/search` on the memory router.
- [ ] Add docs/openapi endpoints only after the route tree above them is stable enough to describe.
- [ ] Run `cargo check --workspace --offline`.

### Task 3: Complete Stubbed Core Modules

**Files:**
- Modify: `engram-lib/src/audit.rs`
- Modify: `engram-lib/src/guard.rs`
- Modify: `engram-lib/src/graph/builder.rs`
- Modify: `engram-lib/src/graph/cooccurrence.rs`
- Modify: `engram-lib/src/graph/search.rs`
- Modify: `engram-lib/src/intelligence/consolidation.rs`
- Modify: `engram-lib/src/intelligence/contradiction.rs`
- Inspect against: `engram/src/graph/{builder,cooccurrence,db,pagerank,communities,structural}.ts`
- Inspect against: `engram/src/intelligence/{consolidation,extraction,decomposition,growth,personality,temporal}.ts`
- Inspect against: `engram/src/guard/routes.ts`
- Inspect against: `engram/src/middleware/audit.ts`

- [ ] Replace each `todo!()` with the corresponding upstream behavior, starting with graph builder/search because multiple routes depend on them.
- [ ] Port contradiction and consolidation logic before trying to tune higher-level intelligence routes.
- [ ] Implement audit query/write helpers before wiring mutation middleware.
- [ ] Implement guard rule evaluation and return shapes aligned with the TypeScript route contract.
- [ ] Add unit tests per module instead of relying on route-level checks only.

### Task 4: Finish Ingestion, Search, And Context Parity

**Files:**
- Modify: `engram-lib/src/ingestion/parsers/{pdf.rs,docx.rs,zip.rs}`
- Modify: `engram-lib/src/ingestion/processors/{raw.rs,extract.rs}`
- Modify: `engram-lib/src/ingestion/{detect.rs,chunker.rs,mod.rs}`
- Modify: `engram-lib/src/context/{mod.rs,deps.rs,scoring.rs,budget.rs,modes.rs}`
- Modify: `engram-lib/src/memory/{mod.rs,search.rs,scoring.rs,vector.rs,simhash.rs}`
- Modify: `engram-lib/src/reranker/mod.rs`
- Modify: `engram-lib/src/embeddings/{mod.rs,onnx.rs,download.rs,chunking.rs,normalize.rs}`
- Inspect against: `engram/src/ingestion/**/*`
- Inspect against: `engram/src/context/**/*`
- Inspect against: `engram/src/memory/**/*`
- Inspect against: `engram/src/search/**/*`
- Inspect against: `engram/src/embeddings/**/*`
- Inspect against: `engram/src/reranker/**/*`

- [ ] Port PDF, DOCX, and ZIP parsing instead of leaving them dependency notes.
- [ ] Finish raw/extract post-store work: embeddings, simhash dedupe, and post-store job hooks.
- [ ] Replace stubbed context phases for embedding dedupe, query embedding, reranking, inference, scratchpad injection, and personality injection.
- [ ] Match the TypeScript search weighting and candidate flow before tuning performance.
- [ ] Re-run `cargo check --workspace --offline`.

### Task 5: Restore FSRS, Grounding, Jobs, And Artifact Workflows

**Files:**
- Modify: `engram-lib/src/fsrs/{mod.rs,decay.rs}`
- Modify: `engram-lib/src/grounding/{mod.rs,client.rs,search.rs,quality.rs,shell.rs}`
- Modify: `engram-lib/src/jobs/mod.rs`
- Modify: `engram-lib/src/artifacts.rs`
- Inspect against: `engram/src/fsrs/**/*`
- Inspect against: `engram/src/grounding/**/*`
- Inspect against: `engram/src/jobs/**/*`
- Inspect against: `engram/src/artifacts/**/*`

- [ ] Port the FSRS review/update endpoints and make sure they operate on the same schema fields as the TypeScript implementation.
- [ ] Port grounding backend behavior, especially shell-backed execution and quality scoring.
- [ ] Turn `jobs` into a real scheduling/execution layer if ingestion and post-store flows depend on it.
- [ ] Port artifact storage/search/encryption only after the schema and route surfaces exist.

### Task 6: Port Eidolon Activity, Gate, Growth, Sessions, And Prompt Generation

**Files:**
- Create: `engram-server/src/routes/activity.rs`
- Create: `engram-server/src/routes/gate.rs`
- Create: `engram-server/src/routes/growth.rs`
- Create: `engram-server/src/routes/sessions.rs`
- Modify: `engram-server/src/routes/prompts.rs`
- Modify: `engram-server/src/routes/mod.rs`
- Modify: `engram-server/src/server.rs`
- Modify: `engram-server/src/state.rs`
- Modify or add supporting modules under `engram-server/src/` for session buffering, approvals, secret resolution, and scrubbing
- Inspect against: `eidolon/eidolon-daemon/src/routes/{activity.rs,gate.rs,growth.rs,prompt.rs,sessions.rs}`
- Inspect against: `eidolon/eidolon-daemon/src/{session.rs,secrets.rs,scrubbing.rs,config.rs,absorber.rs}`
- Inspect against: `eidolon/eidolon-lib/src/{brain.rs,growth.rs,types.rs}`

- [ ] Port `/activity` fan-out first because it depends mostly on already-existing Engram service routes.
- [ ] Port `/gate/check`, `/gate/respond`, and `/gate/complete` with the same blocking/enrichment logic, including SSH parsing and reserved-target checks.
- [ ] Port `/growth/reflect`, `/growth/observations`, and `/growth/materialize`.
- [ ] Add session state and websocket streaming for `/sessions` and `/sessions/{id}/stream`.
- [ ] Extend the prompt router with `/prompt/generate` instead of collapsing it into the existing Engram prompt/header endpoints.
- [ ] Keep Eidolon-specific config isolated in server state rather than mixing it into unrelated Engram config fields.

### Task 7: Finish Middleware And Security Hardening

**Files:**
- Modify: `engram-server/src/middleware/audit.rs`
- Modify: `engram-server/src/middleware/rate_limit.rs`
- Modify: `engram-server/src/middleware/auth.rs`
- Modify: `engram-lib/src/auth.rs`
- Modify: `engram-lib/src/apikeys.rs`
- Inspect against: `engram/src/middleware/{auth,audit,validate}.ts`
- Inspect against: `eidolon/eidolon-daemon/src/audit.rs`
- Inspect against: `eidolon/eidolon-daemon/src/rate_limit.rs`

- [ ] Replace placeholder middleware with actual mutation logging and token-bucket or equivalent request limiting.
- [ ] Preserve already-added `eg_` compatibility while aligning the rest of the key lifecycle with upstream behavior.
- [ ] Add request validation coverage for the new route families instead of leaving validation to handler bodies only.

### Task 8: Verification, Contract Tests, And Toolchain Closure

**Files:**
- Modify or add tests under each affected crate
- Add contract-focused tests in `engram-server` for route payloads and status codes
- Add fixture-based tests in `engram-lib` for ingestion, graph, intelligence, and grounding

- [ ] Add parity tests that compare Rust JSON response shapes against fixtures captured from the source repos.
- [ ] Add focused crate-level test commands to the repo README or a developer plan so verification is reproducible.
- [ ] Restore a working MSVC toolchain or run CI on a Linux builder so `cargo build` and `cargo test` become real gates instead of best-effort checks.
- [ ] Keep `cargo clippy --workspace --offline` as cleanup, not as the primary parity milestone.

## Suggested Milestones

Milestone A:
- Task 1 complete
- Task 2 complete for `agents`, `artifacts`, `auth_keys`, `fsrs`, `grounding`, `search`
- `cargo check --workspace --offline` clean

Milestone B:
- Task 3 and Task 4 complete
- Graph/intelligence/context/search no longer contain `todo!()`
- Ingestion supports PDF, DOCX, and ZIP

Milestone C:
- Task 5 and Task 6 complete
- Rust server covers the Engram API families plus the Eidolon daemon families actually used by the stack

Milestone D:
- Task 7 and Task 8 complete
- `cargo build --workspace` and `cargo test --workspace` pass on a real toolchain

## Blockers And Non-Goals

Blockers:
- This machine still lacks MSVC `link.exe`, so full `cargo build` and `cargo test` cannot currently serve as acceptance gates.
- Some parity work depends on choosing crate additions for parser support and ONNX/runtime packaging.

Non-goals for the first completion pass:
- GUI parity
- TUI parity from the standalone `eidolon-tui` crate
- Re-architecting the Rust repo into the exact crate split used by the TypeScript and Eidolon source repos

## Definition Of "Closer To Completion"

The repo is materially closer to completion when:
- Every upstream route family that matters to the stack exists in Rust with the same payload contract
- The remaining core modules no longer contain `todo!()` placeholders
- Ingestion, graph, intelligence, and guard paths perform real work rather than returning scaffolding
- The server can cover both Engram memory APIs and the Eidolon agent workflow APIs without relying on the old repos at runtime
