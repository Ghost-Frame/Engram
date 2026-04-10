# Task: Per-Tenant Database Sharding

**Branch:** feat/scale-shard
**Phase:** 5
**Effort:** ~43 hours
**Model:** Opus (largest scope, highest risk, requires invariants)
**Depends on:** Phase 1-2 and Phase 4 (connection pool). Will rebase onto Phase 4 once that lands.

---

## Why (Critical Context)

Current "multi-tenant" is logical only. Every row has a `user_id`, every query has `WHERE user_id = ?`. The previous attempt added user_id filters but did not physically isolate data. Master was explicit: this was NOT done correctly. Physical sharding is required.

Problems with logical-only isolation:
1. Blast radius: a bad query or runaway migration touches every tenant.
2. Noisy neighbor: one tenant with 10M memories slows another with 10K.
3. Backup granularity: cannot restore one tenant without restoring everyone.
4. GDPR: cannot cleanly delete a tenant's data.
5. Horizontal scale: cannot move a hot tenant to a bigger box independently.
6. HNSW filtering: Phase 2 HNSW over-fetches and post-filters by user_id. Per-tenant index removes this entirely.

## Goal

One SQLite file plus one LanceDB index per tenant. A `TenantRegistry` maps `user_id -> handles`. Every request resolves a tenant handle before touching data. The `user_id` column is removed from per-tenant tables entirely so that missed query rewrites fail loudly.

## Full Spec

See `~/Documents/specs/2026-04-10-engram-scalability-phase3-5.md` section "Phase 5".

## Layout

```
data_dir/
  tenants/
    <tenant_id>/
      engram.db
      engram.db-wal
      engram.db-shm
      hnsw/memories.lance
      blobs/
  system/
    registry.db
    audit.db
```

`<tenant_id>` is the raw `user_id` if alphanumeric+dash+underscore and <= 64 chars, otherwise `t_<sha256_prefix>`.

## Tenant Registry Schema

```sql
CREATE TABLE tenants (
    tenant_id TEXT PRIMARY KEY,
    user_id TEXT UNIQUE NOT NULL,
    created_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    data_path TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    quota_bytes INTEGER,
    quota_memories INTEGER,
    last_access INTEGER NOT NULL
);
CREATE INDEX idx_tenants_user_id ON tenants(user_id);
CREATE INDEX idx_tenants_last_access ON tenants(last_access);
```

## Types

```rust
pub struct TenantHandle {
    pub tenant_id: String,
    pub user_id: String,
    pub db: Arc<Database>,              // phase 4 pooled Database
    pub vector_index: Arc<dyn VectorIndex>,
    pub created_at: SystemTime,
    pub last_access: Mutex<Instant>,
}

pub struct TenantRegistry {
    registry_db: Arc<Database>,
    data_root: PathBuf,
    handles: RwLock<HashMap<String, Arc<TenantHandle>>>,
    config: TenantConfig,
}

pub struct TenantConfig {
    pub max_resident: usize,      // default 512
    pub idle_timeout: Duration,   // default 15min
    pub preload_on_start: bool,   // default false
}
```

## Files to Create

- `engram-lib/src/tenant/mod.rs` -- registry, handle, config types
- `engram-lib/src/tenant/registry_db.rs` -- system/registry.db access
- `engram-lib/src/tenant/loader.rs` -- lazy load, LRU eviction
- `engram-lib/src/tenant/id.rs` -- `tenant_id_from_user` helper with safe fallback hashing
- `engram-cli/src/bin/engram-migrate.rs` -- monolithic-to-sharded migration tool

## Files to Modify

- `engram-server/src/state.rs` -- `AppState` holds `Arc<TenantRegistry>` instead of `Arc<Database>`
- Every HTTP handler under `engram-server/src/routes/` -- resolve tenant from auth before any db call
- Every per-tenant table definition in `engram-lib/src/db/schema.rs` -- drop `user_id` column
- Every per-tenant SQL query -- drop `WHERE user_id = ?` clause
- `engram-lib/src/jobs/*` -- iterate tenants via registry, cap concurrency
- `engram-server/src/routes/admin/*` -- add tenant lifecycle endpoints, use registry

## Request Path Rewrite

Before:
```rust
let user_id = extract_user_id(&req)?;
let memories = memory::list(&state.db, &user_id).await?;
```

After:
```rust
let user_id = extract_user_id(&req)?;
let tenant = state.tenants.get_or_create(&user_id).await?;
let memories = memory::list(&tenant.db).await?;
```

Every per-tenant query loses its `user_id` filter because the database only contains one tenant.

## Lazy Loading and Eviction

Loading 10K tenants at startup is wasteful. Lazy-load on first request. LRU evicts idle handles after `idle_timeout`. Eviction closes reader/writer pools and drops the LanceDB handle. Next request reloads.

## Tables That Lose user_id

- memories
- structured_facts
- associations
- episodes
- memory_pagerank (Phase 3)
- inferences
- events (if per-tenant)
- every other per-row table

Keep user_id only in `system/registry.db` and `system/audit.db`.

## Migration Tool (engram-migrate)

Algorithm:
1. Open old monolithic DB read-only.
2. Scan `SELECT DISTINCT user_id FROM memories` to find tenants.
3. For each user_id:
   a. Compute `tenant_id`.
   b. Create `data_dir/tenants/<tenant_id>/` and empty schema.
   c. `ATTACH DATABASE` old DB, `INSERT INTO tenant.memories SELECT ... WHERE user_id = ?` for each table.
   d. Drop `user_id` column from tenant tables (SQLite: recreate table without the column, copy data).
   e. Rebuild HNSW index from memory embeddings.
   f. Insert registry row.
4. Verify row counts per tenant match expected.
5. Rename old DB to `<name>.pre-shard-backup`.

Modes:
- `--dry-run`: report what would happen, write nothing.
- `--reverse`: consolidate shards back into a monolithic DB (for rollback).
- `--verify`: after migration, run a verification pass comparing checksums.

## Background Jobs Per Tenant

```rust
async fn run_pagerank_for_all_dirty(&self) -> Result<()> {
    let dirty = self.registry.list_dirty().await?;
    let semaphore = Arc::new(Semaphore::new(2));
    let mut handles = Vec::new();
    for tenant in dirty {
        let sem = semaphore.clone();
        let registry = self.registry.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await?;
            let handle = registry.get(&tenant.user_id).await?;
            refresh_pagerank(&handle).await
        }));
    }
    for h in handles { h.await??; }
    Ok(())
}
```

Cap concurrency so one bad tenant does not starve others.

## Admin Endpoints

- `GET  /admin/tenants` -- list
- `POST /admin/tenants` -- create
- `GET  /admin/tenants/:id` -- detail and stats
- `DELETE /admin/tenants/:id` -- delete with audit entry
- `POST /admin/tenants/:id/suspend`
- `POST /admin/tenants/:id/resume`

All go through the registry, never through a tenant handle.

## Quota Enforcement

On write paths, check `quota_bytes` and `quota_memories` from the registry row. Return 402 or 429 on violation. Sample-based checks acceptable if per-write checks become hot.

## Audit Log

`system/audit.db` logs:
- tenant.create
- tenant.delete
- tenant.suspend
- tenant.quota_exceeded
- admin.cross_tenant_access

## Metrics

- `tenants_resident`
- `tenants_total`
- `tenant_load_ms` (histogram)
- `tenant_evictions_total`
- `tenant_quota_exceeded_total`

## HNSW Simplification (Bonus)

Phase 2 HNSW over-fetched and post-filtered because the index was shared. Per-tenant shards remove this:
- Delete the post-filter pass in `memory/search.rs`.
- Drop the over-fetch multiplier.
- Each tenant has its own dedicated LanceDB index with exact results.

## Tests

- Unit: registry create/get/delete, `tenant_id_from_user` with adversarial inputs (emoji, slashes, traversal attempts, extreme length).
- Unit: eviction LRU correctness.
- Integration: create 10 tenants, verify physical isolation (one cannot see another's data even if handler bug).
- Migration: run `engram-migrate --dry-run` and `engram-migrate` on a fixture monolithic DB, verify row counts and schemas.
- Load: 1000 tenants lazy-loaded, verify memory stays below budget, verify evictions happen on idle.
- Deletion: delete tenant, verify files removed and audit entry present.
- Recovery: kill server mid-write to a tenant DB, restart, verify WAL recovers.
- Security: craft token for user A, send request with user B in path, verify rejection.

## Rollback

Not trivially reversible. Mitigations:
- Keep `.pre-shard-backup` monolithic DB for at least one release.
- `engram-migrate --reverse` consolidates shards back.
- Feature flag `tenant_sharding_enabled` lets new installs opt in while existing runs stay monolithic until migration is scheduled.

## Risks

- **Handler coverage**: a single missed query that references `user_id` fails because the column is gone. Good (fails loudly). Bad if it takes down production. Mitigation: staged rollout, feature flag, and comprehensive grep for `user_id` in SQL strings.
- **File descriptor exhaustion**: 10K tenants = 10K SQLite files potentially open. OS `ulimit -n` matters. Mitigation: `max_resident` cap plus eviction.
- **WAL file buildup per shard**: eviction must checkpoint before closing.
- **Backup tooling**: existing backup scripts assume one DB. Rewrite to walk `tenants/` directory or provide `engram-backup` helper.
- **Cross-tenant search**: if ever needed, requires federation. Document as known tradeoff.

## Implementation Order

1. Tenant types and registry (no handlers use them yet).
2. `tenant_id_from_user` with full test coverage.
3. Lazy loader and eviction.
4. Registry DB schema and admin endpoints.
5. Port one handler end-to-end to the registry as a proof of concept.
6. Drop `user_id` column from the first ported table.
7. Port remaining handlers in waves (memory, intelligence, ingestion, export, admin).
8. Drop `user_id` from remaining tables.
9. Update background jobs.
10. Write `engram-migrate` tool.
11. Run migration on a staging snapshot.
12. Load test 1000 tenants.
13. Security test cross-tenant isolation.

## Verification

- [ ] Every request resolves a tenant handle before touching data
- [ ] `user_id` column removed from every per-tenant table
- [ ] Cross-tenant request returns 404 or 403, never wrong data
- [ ] `engram-migrate` converts a monolithic DB to sharded layout
- [ ] `engram-migrate --dry-run` reports correctly without writing
- [ ] 1000-tenant load test stays within memory budget
- [ ] Per-tenant HNSW index works without post-filtering
- [ ] Tenant delete removes all files AND writes audit entry
- [ ] Admin endpoints for tenant lifecycle work
- [ ] Quota enforcement blocks writes past limit
- [ ] Audit log captures tenant lifecycle events
- [ ] Backup tooling handles the new layout

## Commit Message Style

```
feat(tenant): phase 5 physical per-tenant sharding

Introduce TenantRegistry with lazy loading and LRU eviction. Each
tenant gets its own SQLite file and LanceDB index under
data_dir/tenants/<id>/. Removes user_id columns from per-tenant
tables so missed query rewrites fail loudly. Adds engram-migrate
tool for monolithic-to-sharded conversion.
```

No em dashes. Use -- or rewrite.
