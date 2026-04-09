# Kleos Rust Completion Parity Plan

**Goal:** Close the highest-value parity gaps between `kleos`, `C:\Users\Zan\Projects\kleos`, and the Eidolon behaviors currently expected by the ecosystem in `C:\Users\Zan\Projects\eidolon`.

**Architecture:** Treat `kleos` as the source of truth for the core memory server, data model, and HTTP/CLI contract. Treat `eidolon` as the source of truth for brain-adjacent agent workflows: activity fan-out, gate enforcement, growth reflection, prompt generation, and session streaming. Port by dependency order: schema first, library logic second, route layer third, then verification and cleanup.

**Tech Stack:** Rust workspace (`kleos-lib`, `kleos-server`, `kleos-cli`, `kleos-sidecar`), `axum`, `tokio`, `libsql`, ONNX Runtime, source repos `kleos` (TypeScript) and `eidolon` (Rust).

---

## Source Of Truth Map

`kleos` is authoritative for:
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
- Broad route/module wiring in [`kleos-server/src/routes/mod.rs`](C:\Users\Zan\Projects\kleos\kleos-server\src\routes\mod.rs) and [`kleos-lib/src/lib.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\lib.rs)
- Working service families for Chiasm, Axon, Broca, Soma, Loom, Thymus, Brain
- CLI/server compatibility fixes already landed
- Sidecar moved off placeholder handlers and onto direct DB-backed routes

Still partial or missing:
- Hard `todo!()` in [`kleos-lib/src/audit.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\audit.rs), [`kleos-lib/src/guard.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\guard.rs), [`kleos-lib/src/graph/builder.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\graph\builder.rs), [`kleos-lib/src/graph/cooccurrence.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\graph\cooccurrence.rs), [`kleos-lib/src/graph/search.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\graph\search.rs), [`kleos-lib/src/intelligence/consolidation.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\intelligence\consolidation.rs), [`kleos-lib/src/intelligence/contradiction.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\intelligence\contradiction.rs), [`kleos-lib/src/db/migrations.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\db\migrations.rs)
- Stubbed or incomplete ingestion/parser paths in [`kleos-lib/src/ingestion/parsers/pdf.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\ingestion\parsers\pdf.rs), [`kleos-lib/src/ingestion/parsers/docx.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\ingestion\parsers\docx.rs), [`kleos-lib/src/ingestion/parsers/zip.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\ingestion\parsers\zip.rs), [`kleos-lib/src/skills/cloud.rs`](C:\Users\Zan\Projects\kleos\kleos-lib\src\skills\cloud.rs)
- Stubbed middleware in [`kleos-server/src/middleware/audit.rs`](C:\Users\Zan\Projects\kleos\kleos-server\src\middleware\audit.rs) and [`kleos-server/src/middleware/rate_limit.rs`](C:\Users\Zan\Projects\kleos\kleos-server\src\middleware\rate_limit.rs)
- Entire route families missing from the Rust server even though the upstream repos expose them

## Missing Route Families

Missing from `kleos` but present in `kleos`:
- `agents` from `C:\Users\Zan\Projects\kleos\src\agents\routes.ts`
- `artifacts` from `C:\Users\Zan\Projects\kleos\src\artifacts\routes.ts`
- split auth-key routes from `C:\Users\Zan\Projects\kleos\src\auth-keys\routes.ts`
- `fsrs` from `C:\Users\Zan\Projects\kleos\src\fsrs\routes.ts`
- `grounding` from `C:\Users\Zan\Projects\kleos\src\grounding\routes.ts`
- `search` from `C:\Users\Zan\Projects\kleos\src\search\routes.ts`
- `docs` and `openapi` export surface from `C:\Users\Zan\Projects\kleos\src\docs\routes.ts` and `C:\Users\Zan\Projects\kleos\src\openapi.ts`
- `onboard` from `C:\Users\Zan\Projects\kleos\src\onboard\routes.ts`

Missing from `kleos` but present in `eidolon`:
- `activity` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\activity.rs`
- `gate` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\gate.rs`
- `growth` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\growth.rs`
- `sessions` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\sessions.rs`
- `prompt/generate` from `C:\Users\Zan\Projects\eidolon\eidolon-daemon\src\routes\prompt.rs`

## Recommended Execution Order

1. Schema and migrations
2. Missing Kleos route families that already have Rust lib support
3. Stubbed core logic: graph, intelligence, audit, guard
4. Ingestion and embedding/reranker parity
5. Eidolon route families and daemon behaviors
6. Middleware, auth, and verification hardening

### Task 1: Finish Schema And Migration Parity

**Files:**
- Modify: `kleos-lib/src/db/schema.rs`
- Modify: `kleos-lib/src/db/migrations.rs`
- Inspect against: `kleos/src/db/schema/base.ts`
- Inspect against: `kleos/src/db/schema/episodes.ts`
- Inspect against: `kleos/src/db/schema/fts.ts`
- Inspect against: `kleos/src/db/schema/intelligence.ts`
- Inspect against: `kleos/src/db/schema/services.ts`
- Inspect against: `kleos/src/db/schema/tier4.ts`

- [ ] Diff the TypeScript schema files against the Rust schema builder and write a gap list by table, index, trigger, and column.
- [ ] Replace the `todo!()` migration entrypoint with ordered Rust migrations that preserve the TypeScript schema names and data semantics.
- [ ] Add migrations for service tables, intelligence/tier4 tables, and any missing artifacts/agents/grounding/FSRS support tables.
- [ ] Add migration verification tests in `kleos-lib` that create an empty DB, run migrations once, run them a second time, and assert idempotence.
- [ ] Run `cargo check --workspace --offline`.
- [ ] Run targeted tests once `link.exe` is available: `cargo test -p kleos-lib db:: --offline`.

### Task 2: Expose Missing Kleos Route Families

**Files:**
- Create: `kleos-server/src/routes/agents.rs`
- Create: `kleos-server/src/routes/artifacts.rs`
- Create: `kleos-server/src/routes/auth_keys.rs`
- Create: `kleos-server/src/routes/fsrs.rs`
- Create: `kleos-server/src/routes/grounding.rs`
- Create: `kleos-server/src/routes/search.rs`
- Create: `kleos-server/src/routes/docs.rs`
- Create: `kleos-server/src/routes/onboard.rs`
- Modify: `kleos-server/src/routes/mod.rs`
- Modify: `kleos-server/src/server.rs`
- Modify as needed: `kleos-lib/src/agents.rs`
- Modify as needed: `kleos-lib/src/artifacts.rs`
- Modify as needed: `kleos-lib/src/apikeys.rs`
- Modify as needed: `kleos-lib/src/fsrs/mod.rs`
- Modify as needed: `kleos-lib/src/grounding/{mod.rs,client.rs,search.rs,quality.rs,shell.rs}`

- [ ] Port the HTTP path surface from the matching TypeScript route files without inventing new payload shapes.
- [ ] Reuse the existing `security.rs` only for internal consolidation if the response contracts remain identical. If they do not, keep separate route files.
- [ ] Make `search` a first-class route family rather than hiding everything behind `/search` on the memory router.
- [ ] Add docs/openapi endpoints only after the route tree above them is stable enough to describe.
- [ ] Run `cargo check --workspace --offline`.

### Task 3: Complete Stubbed Core Modules

**Files:**
- Modify: `kleos-lib/src/audit.rs`
- Modify: `kleos-lib/src/guard.rs`
- Modify: `kleos-lib/src/graph/builder.rs`
- Modify: `kleos-lib/src/graph/cooccurrence.rs`
- Modify: `kleos-lib/src/graph/search.rs`
- Modify: `kleos-lib/src/intelligence/consolidation.rs`
- Modify: `kleos-lib/src/intelligence/contradiction.rs`
- Inspect against: `kleos/src/graph/{builder,cooccurrence,db,pagerank,communities,structural}.ts`
- Inspect against: `kleos/src/intelligence/{consolidation,extraction,decomposition,growth,personality,temporal}.ts`
- Inspect against: `kleos/src/guard/routes.ts`
- Inspect against: `kleos/src/middleware/audit.ts`

- [ ] Replace each `todo!()` with the corresponding upstream behavior, starting with graph builder/search because multiple routes depend on them.
- [ ] Port contradiction and consolidation logic before trying to tune higher-level intelligence routes.
- [ ] Implement audit query/write helpers before wiring mutation middleware.
- [ ] Implement guard rule evaluation and return shapes aligned with the TypeScript route contract.
- [ ] Add unit tests per module instead of relying on route-level checks only.

### Task 4: Finish Ingestion, Search, And Context Parity

**Files:**
- Modify: `kleos-lib/src/ingestion/parsers/{pdf.rs,docx.rs,zip.rs}`
- Modify: `kleos-lib/src/ingestion/processors/{raw.rs,extract.rs}`
- Modify: `kleos-lib/src/ingestion/{detect.rs,chunker.rs,mod.rs}`
- Modify: `kleos-lib/src/context/{mod.rs,deps.rs,scoring.rs,budget.rs,modes.rs}`
- Modify: `kleos-lib/src/memory/{mod.rs,search.rs,scoring.rs,vector.rs,simhash.rs}`
- Modify: `kleos-lib/src/reranker/mod.rs`
- Modify: `kleos-lib/src/embeddings/{mod.rs,onnx.rs,download.rs,chunking.rs,normalize.rs}`
- Inspect against: `kleos/src/ingestion/**/*`
- Inspect against: `kleos/src/context/**/*`
- Inspect against: `kleos/src/memory/**/*`
- Inspect against: `kleos/src/search/**/*`
- Inspect against: `kleos/src/embeddings/**/*`
- Inspect against: `kleos/src/reranker/**/*`

- [ ] Port PDF, DOCX, and ZIP parsing instead of leaving them dependency notes.
- [ ] Finish raw/extract post-store work: embeddings, simhash dedupe, and post-store job hooks.
- [ ] Replace stubbed context phases for embedding dedupe, query embedding, reranking, inference, scratchpad injection, and personality injection.
- [ ] Match the TypeScript search weighting and candidate flow before tuning performance.
- [ ] Re-run `cargo check --workspace --offline`.

### Task 5: Restore FSRS, Grounding, Jobs, And Artifact Workflows

**Files:**
- Modify: `kleos-lib/src/fsrs/{mod.rs,decay.rs}`
- Modify: `kleos-lib/src/grounding/{mod.rs,client.rs,search.rs,quality.rs,shell.rs}`
- Modify: `kleos-lib/src/jobs/mod.rs`
- Modify: `kleos-lib/src/artifacts.rs`
- Inspect against: `kleos/src/fsrs/**/*`
- Inspect against: `kleos/src/grounding/**/*`
- Inspect against: `kleos/src/jobs/**/*`
- Inspect against: `kleos/src/artifacts/**/*`

- [ ] Port the FSRS review/update endpoints and make sure they operate on the same schema fields as the TypeScript implementation.
- [ ] Port grounding backend behavior, especially shell-backed execution and quality scoring.
- [ ] Turn `jobs` into a real scheduling/execution layer if ingestion and post-store flows depend on it.
- [ ] Port artifact storage/search/encryption only after the schema and route surfaces exist.

### Task 6: Port Eidolon Activity, Gate, Growth, Sessions, And Prompt Generation

**Files:**
- Create: `kleos-server/src/routes/activity.rs`
- Create: `kleos-server/src/routes/gate.rs`
- Create: `kleos-server/src/routes/growth.rs`
- Create: `kleos-server/src/routes/sessions.rs`
- Modify: `kleos-server/src/routes/prompts.rs`
- Modify: `kleos-server/src/routes/mod.rs`
- Modify: `kleos-server/src/server.rs`
- Modify: `kleos-server/src/state.rs`
- Modify or add supporting modules under `kleos-server/src/` for session buffering, approvals, secret resolution, and scrubbing
- Inspect against: `eidolon/eidolon-daemon/src/routes/{activity.rs,gate.rs,growth.rs,prompt.rs,sessions.rs}`
- Inspect against: `eidolon/eidolon-daemon/src/{session.rs,secrets.rs,scrubbing.rs,config.rs,absorber.rs}`
- Inspect against: `eidolon/eidolon-lib/src/{brain.rs,growth.rs,types.rs}`

- [ ] Port `/activity` fan-out first because it depends mostly on already-existing Kleos service routes.
- [ ] Port `/gate/check`, `/gate/respond`, and `/gate/complete` with the same blocking/enrichment logic, including SSH parsing and reserved-target checks.
- [ ] Port `/growth/reflect`, `/growth/observations`, and `/growth/materialize`.
- [ ] Add session state and websocket streaming for `/sessions` and `/sessions/{id}/stream`.
- [ ] Extend the prompt router with `/prompt/generate` instead of collapsing it into the existing Kleos prompt/header endpoints.
- [ ] Keep Eidolon-specific config isolated in server state rather than mixing it into unrelated Kleos config fields.

### Task 7: Finish Middleware And Security Hardening

**Files:**
- Modify: `kleos-server/src/middleware/audit.rs`
- Modify: `kleos-server/src/middleware/rate_limit.rs`
- Modify: `kleos-server/src/middleware/auth.rs`
- Modify: `kleos-lib/src/auth.rs`
- Modify: `kleos-lib/src/apikeys.rs`
- Inspect against: `kleos/src/middleware/{auth,audit,validate}.ts`
- Inspect against: `eidolon/eidolon-daemon/src/audit.rs`
- Inspect against: `eidolon/eidolon-daemon/src/rate_limit.rs`

- [ ] Replace placeholder middleware with actual mutation logging and token-bucket or equivalent request limiting.
- [ ] Preserve already-added `eg_` compatibility while aligning the rest of the key lifecycle with upstream behavior.
- [ ] Add request validation coverage for the new route families instead of leaving validation to handler bodies only.

### Task 8: Verification, Contract Tests, And Toolchain Closure

**Files:**
- Modify or add tests under each affected crate
- Add contract-focused tests in `kleos-server` for route payloads and status codes
- Add fixture-based tests in `kleos-lib` for ingestion, graph, intelligence, and grounding

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
- Rust server covers the Kleos API families plus the Eidolon daemon families actually used by the stack

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
- The server can cover both Kleos memory APIs and the Eidolon agent workflow APIs without relying on the old repos at runtime
