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
- `engram-cli/src/migrate/mod.rs` -- migration tool library code (shared with bin)
- `engram-cli/src/migrate/libsql_reader.rs` -- libsql read side
- `engram-cli/src/migrate/rusqlite_writer.rs` -- rusqlite + LanceDB write side
- `engram-cli/src/migrate/verify.rs` -- post-migration verification

## Cargo Dependency Policy

- `engram-lib`: rusqlite only, no libsql
- `engram-server`: no libsql
- `engram-cli`: BOTH libsql (for migration) and rusqlite (via engram-lib). This is the only crate that links both.

This is enforced by a workspace check in Phase 4's final commit.

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

This tool is the single bridge from the old world (libsql, monolithic, user_id-filtered) to the new world (rusqlite, sharded, per-tenant). It is the ONLY place libsql is still used after Phase 4 lands.

### Why two drivers in one binary

Phase 4 drops libsql from `engram-lib` and `engram-server`, but existing deployments have libsql-format databases containing the `memories_vec_1024_idx` virtual table from prior schema versions. Vanilla rusqlite cannot open a schema that references libsql-specific virtual tables without errors. The migration tool uses:

- **libsql crate** to READ the old monolithic database
- **rusqlite crate** to WRITE the new per-tenant shards
- **lancedb crate** to build per-tenant HNSW indexes from embeddings

`engram-cli/Cargo.toml` keeps libsql as a dependency. After a release or two, once users have migrated, a cleanup commit can remove libsql entirely.

### Binary Location

```
engram-cli/src/bin/engram-migrate.rs
```

### Algorithm

1. **Lock**: write `<data_dir>/.migration-in-progress` lockfile. Fail fast if it already exists.
2. **Read old DB**: open `engram.db` via libsql (read-only). Verify schema version and report it.
3. **Enumerate tenants**: `SELECT DISTINCT user_id FROM memories` (and other tables that might have user_id-only rows such as associations or structured_facts that reference memories not yet created).
4. **For each user_id**:
   a. Compute `tenant_id` via `tenant_id_from_user`.
   b. `mkdir -p data_dir/tenants/<tenant_id>/{hnsw,blobs}`.
   c. Create a fresh `engram.db` in that directory via rusqlite with the NEW schema (no `user_id` column, no `memories_vec_1024_idx` virtual table, includes Phase 3 `memory_pagerank` tables).
   d. Stream rows from libsql per table, insert via rusqlite inside a single transaction per table. Drop the `user_id` column at copy time by simply not selecting it.
   e. For each memory with an embedding, decode the embedding (was stored by libsql as BLOB or as libsql vector type) to `Vec<f32>` and insert into a fresh LanceDB index at `data_dir/tenants/<tenant_id>/hnsw/memories.lance`.
   f. Insert a row into `data_dir/system/registry.db` with status=active, schema_version=current.
   g. Write an audit entry `tenant.created_via_migration`.
5. **Verify**: for each tenant, compare `SELECT COUNT(*)` from the old DB (filtered by user_id) with the new shard's `SELECT COUNT(*)`. Mismatch aborts and rolls back that tenant.
6. **Global tables**: any rows not associated with a user_id (schema meta, global caches) go into `data_dir/system/` tables, not tenants.
7. **Swap**: rename old `engram.db` to `engram.db.pre-shard-backup`. Do NOT delete.
8. **Unlock**: remove `.migration-in-progress` lockfile.
9. **Summary**: print tenant count, row counts, time taken, disk used.

### Modes

- `engram-migrate --data-dir <dir> --dry-run`
  - Reports distinct user_id count, per-tenant row counts, estimated disk use. Writes nothing.
- `engram-migrate --data-dir <dir>`
  - Full migration. Requires server to be stopped.
- `engram-migrate --data-dir <dir> --verify`
  - After a successful migration, reruns counts and checksums as a sanity pass.
- `engram-migrate --data-dir <dir> --reverse`
  - Consolidates `data_dir/tenants/*` back into a single rusqlite `engram.db` at the root. Rolls back the sharding transformation. Note: the rollback target is vanilla SQLite, NOT libsql, because we are not regressing drivers. Rollback does not restore libsql-specific virtual tables.

### Embedding Conversion

Check how embeddings are currently stored in libsql. Two cases:

- **Case A: TEXT/JSON**: `serde_json::from_str::<Vec<f32>>(&text)?`. Easy.
- **Case B: libsql vector BLOB**: libsql stores vectors in a typed BLOB. Use libsql's typed getter: `row.get::<Vec<f32>>(col)?`, or if that is not exposed, read the raw BLOB and decode with the libsql vector format. Document the format clearly.

The decoded `Vec<f32>` goes into LanceDB AND is stored in the new rusqlite DB as a plain BLOB (just `bincode` serialized `Vec<f32>`) so rebuilds remain possible.

### Schema Drift Handling

Old deployments may have different schema versions. The tool must:

1. Read `schema_migrations` or equivalent from the old DB to determine version.
2. Apply any forward migrations that are idempotent to bring the old data to a canonical pre-migration state BEFORE sharding (e.g. if an old version is missing a column the tool expects).
3. Refuse to run on unknown or unsupported versions with a clear error.

Add a `MIGRATION_SUPPORTED_VERSIONS: &[u32]` constant in the tool source. Update it as new schema versions ship.

### Partial Failure Recovery

If the tool crashes mid-run:
- Source DB is untouched (we only rename at the final step).
- `.migration-in-progress` lockfile remains -- user must investigate before rerunning.
- Any partially-created tenant directories can be deleted by the user after investigation, OR the tool can accept `--resume` to skip tenants whose registry row already exists and whose row counts match.

### Self-Host Case

Single-user deployments still run the tool. They get one tenant shard at `data_dir/tenants/<user_id>/`. Overhead is a handful of files. This preserves the self-hosting story.

### Operational Notes

- **Server must be stopped**: migration is NOT online. The tool refuses to run if it detects the WAL is active or if a pid lock is held.
- **Document downtime**: README updates and release notes must call out the required downtime window.
- **Backup first**: the tool refuses to run unless `--i-have-a-backup` is passed OR the source DB is under 100MB (in which case it creates its own backup automatically).

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
10. Write `engram-migrate` tool:
    a. libsql reader that enumerates user_ids and streams rows.
    b. rusqlite writer that creates shards with the new schema.
    c. LanceDB index builder from embeddings.
    d. Registry writer.
    e. Verification pass.
    f. Dry-run mode.
    g. Reverse mode (shards to monolithic vanilla SQLite).
    h. Lockfile + resume support.
11. Test migration on a fixture libsql DB at each supported schema version.
12. Run migration on a staging snapshot of real data.
13. Load test 1000 tenants.
14. Security test cross-tenant isolation.

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
- [ ] `engram-migrate --dry-run` runs cleanly on a libsql fixture
- [ ] `engram-migrate` successfully converts a libsql fixture with vector virtual tables
- [ ] Row counts verified per tenant after migration
- [ ] LanceDB indexes built from libsql embeddings produce correct nearest-neighbor results
- [ ] `engram-migrate --reverse` consolidates shards back to vanilla SQLite
- [ ] `engram-migrate --resume` picks up after a partial failure
- [ ] Lockfile prevents concurrent runs
- [ ] libsql is ONLY referenced from the engram-cli crate, not engram-lib or engram-server

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
