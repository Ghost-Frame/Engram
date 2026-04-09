# Worktree J: Contract Tests and Verification

## Worktree Path
`/home/zan/Projects/engram-rust-wt-J` -- branch `feat/contract-tests`

## Goal
Add parity tests that verify the Rust server produces the same API responses as the TypeScript engram. Currently there are ZERO contract tests.

## Source of Truth
- `C:\Users\Zan\Projects\engram\tests\api.test.mjs` -- TS API test suite (33 tests, 14 suites)
- `C:\Users\Zan\Projects\engram\server.ts` -- TS server for endpoint reference

## Tasks

### 1. Read TS Test Suite First
Read `C:\Users\Zan\Projects\engram\tests\api.test.mjs` to understand what's tested and what the expected response shapes are.

### 2. Create Server Integration Tests
- Create: `engram-server/tests/api_parity.rs` (or similar)
- Use `axum::test` or `tower::ServiceExt` for in-process testing
- Set up test harness with in-memory DB (`Database::connect_memory().await`)
- Test each route family:

#### Memory Routes
- POST /store -- verify response shape (id, content, category, etc.)
- GET /memory/:id -- verify response shape
- GET /memories -- verify list response
- POST /search -- verify search results shape
- DELETE /memory/:id -- verify deletion

#### Auth Routes
- POST /keys -- verify key creation response
- GET /keys -- verify list
- Key rotation

#### Service Routes
- Health check
- Admin endpoints (with auth)

#### Intelligence Routes
- Consolidation candidates
- Contradiction detection

#### Graph Routes
- Build graph
- Entity CRUD
- Search

### 3. Add Fixture-Based Tests in engram-lib
- Tests for ingestion (parse markdown, CSV, HTML)
- Tests for graph building
- Tests for context assembly
- Tests for FSRS calculations
- Tests for guard rule evaluation

### 4. Multi-Tenant Isolation Tests
- Create user A and user B
- Store memories for each
- Verify user A cannot read/modify user B's data through any endpoint
- This validates worktree F's hardening work

## Constraints
- Read TS test suite FIRST to understand expected shapes
- Use `Database::connect_memory().await` for all tests
- Tests must be self-contained (no external dependencies)
- Match TS response JSON field names exactly
- Run `cargo test --workspace` to verify
- Run `cargo clippy --workspace` before committing
- No em dashes

## Verification
1. `cargo test --workspace` passes with all new tests
2. At minimum 20+ test cases covering the major route families
3. Multi-tenant isolation tests pass
4. `cargo clippy --workspace` clean
