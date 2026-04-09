# Worktree I: Remaining Library Completions

## Worktree Path
`/home/zan/Projects/engram-rust-wt-I` -- branch `feat/lib-completions`

## Goal
Close the remaining lib-level gaps: FSRS decay, grounding backends, jobs system, skills cloud, and docs/openapi route.

## Source of Truth
- `C:\Users\Zan\Projects\engram\src\fsrs\` -- TS FSRS implementation
- `C:\Users\Zan\Projects\engram\src\grounding\` -- TS grounding implementation
- `C:\Users\Zan\Projects\engram\src\jobs\` -- TS jobs implementation
- `C:\Users\Zan\Projects\engram\src\docs\routes.ts` -- TS docs/openapi routes
- `C:\Users\Zan\Projects\engram\src\openapi.ts` -- TS openapi spec

## Tasks

### 1. FSRS Decay Review
- File: `engram-lib/src/fsrs/decay.rs`
- Compare with TS `fsrs/` implementation
- Verify the decay calculation matches (stability, difficulty, retrievability)
- Fix any deviations

### 2. Grounding Backend Completion
- Files: `engram-lib/src/grounding/{client.rs, search.rs, quality.rs, shell.rs}`
- Read TS `grounding/` to understand shell-backed execution and quality scoring
- Verify shell.rs has real execution (not stubs)
- Verify quality scoring matches TS behavior

### 3. Jobs System
- File: `engram-lib/src/jobs/mod.rs`
- Read TS `jobs/` to understand scheduling/execution
- If post-store flows (embeddings, simhash dedup, hooks) depend on jobs, wire them
- If jobs is just a basic queue, implement it as such

### 4. Skills Cloud
- File: `engram-lib/src/skills/cloud.rs` (currently 48 lines)
- Read TS skills/cloud implementation
- Implement or mark as optional if it depends on external cloud services

### 5. Docs/OpenAPI Route
- Create: `engram-server/src/routes/docs.rs`
- Modify: `engram-server/src/routes/mod.rs` (add `pub mod docs;`)
- Modify: `engram-server/src/server.rs` (wire router)
- Read TS `docs/routes.ts` and `openapi.ts` for the expected endpoint shape
- At minimum: serve a JSON openapi spec describing all current routes

### 6. HMAC Passport Signing (agents)
- File: `engram-server/src/routes/agents.rs`
- Currently: `"signature": "not_implemented"` in passport endpoint
- Read TS agents implementation to understand HMAC signing
- Implement if feasible, or document what's needed (signing secret infra)

## Constraints
- Read TS source for each item BEFORE implementing
- Read existing Rust code BEFORE editing
- Run `cargo check --workspace` after every change
- Run `cargo clippy --workspace` before committing
- No em dashes
- Match existing code patterns

## Verification
1. `cargo check --workspace` passes
2. `cargo clippy --workspace` passes
3. `cargo test --workspace` passes
4. No remaining stubs in modified files
