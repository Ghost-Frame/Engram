# Task: Connection Pool Migration (libsql -> tokio-rusqlite + deadpool)

**Branch:** feat/scale-pool
**Phase:** 4
**Effort:** ~22 hours
**Model:** Opus (large mechanical refactor, many files, high risk)
**Depends on:** Phase 1-2 (merged to main). Parallelizable with Phase 3 and Phase 5 but Phase 5 will need to rebase onto this.

---

## Why

Current `engram-lib/src/db/mod.rs` holds a single `libsql::Connection` behind a `Mutex`. Every read, every write, every transaction serializes through one handle. Writes block reads, burst traffic queues, `spawn_blocking` workers waste time waiting on the lock, long ingestion transactions stall all other traffic.

## Goal

Replace the single-handle libsql setup with a real connection pool. Two pools: one for readers, one for the single writer. Drop the libsql dependency from `engram-lib` and `engram-server`. libsql stays available ONLY to the `engram-migrate` tool (Phase 5) as the one-time read bridge for existing data.

## libsql Handoff to Phase 5

IMPORTANT: do NOT remove libsql from the workspace `Cargo.lock` in the final commit of this phase. Instead:

- Remove libsql from `engram-lib/Cargo.toml`.
- Remove libsql from `engram-server/Cargo.toml`.
- Leave libsql available as a dependency of `engram-cli` (or a new `engram-migrate` crate under the CLI).
- Phase 5 will use libsql inside `engram-migrate` to read the old monolithic database while writing the new sharded layout with rusqlite.
- After Phase 5 ships and users have had a release or two to migrate, a later cleanup commit can remove libsql entirely.

This matters because existing deployments have libsql-format databases with the `memories_vec_1024_idx` virtual table in their schema. rusqlite cannot open a schema that references libsql-specific virtual tables without errors. The migration tool (Phase 5) handles the conversion. Without the bridge, existing users would have no upgrade path.

## Decision

Use `tokio-rusqlite` + `deadpool-sqlite`. Not `sqlx`.

Rationale: libsql's API is close to rusqlite, so most queries port with minimal rewriting. `sqlx` would force a macro-driven rewrite of every query. Phase 2 already removed the last libsql-specific feature (`vector_top_k`) when LanceDB landed, so nothing holds us to libsql.

## Full Spec

See `~/Documents/specs/2026-04-10-engram-scalability-phase3-5.md` section "Phase 4".

## Pool Config

```rust
pub struct DbPoolConfig {
    pub max_readers: usize,      // default = num_cpus * 2
    pub writer_count: usize,     // always 1 (SQLite is single-writer)
    pub busy_timeout_ms: u64,    // default 5000
    pub wal_autocheckpoint: u64, // default 1000
}
```

## Pragmas Applied Per Connection

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
PRAGMA cache_size = -65536;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
```

## API Shape

```rust
pub struct Database {
    reader: deadpool_sqlite::Pool,
    writer: deadpool_sqlite::Pool,
    pub vector_index: Arc<dyn VectorIndex>,
}

impl Database {
    pub async fn read<F, T>(&self, f: F) -> Result<T>
    where F: FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static, T: Send + 'static;

    pub async fn write<F, T>(&self, f: F) -> Result<T>
    where F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static, T: Send + 'static;

    pub async fn transaction<F, T>(&self, f: F) -> Result<T>
    where F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<T> + Send + 'static, T: Send + 'static;
}
```

## Files to Create

- `engram-lib/src/db/pool.rs` -- pool builder, pragma application, connection customizer

## Files to Modify

- `engram-lib/Cargo.toml` -- add `rusqlite = "0.31"`, `tokio-rusqlite = "0.5"`, `deadpool-sqlite = "0.8"`, remove `libsql` once all paths ported
- `engram-server/Cargo.toml` -- remove libsql if present
- `engram-cli/Cargo.toml` -- KEEP libsql (Phase 5 migration tool needs it)
- `engram-lib/src/db/mod.rs` -- swap Database to hold two pools
- `engram-lib/src/db/schema.rs` -- migrations run against writer pool
- Every file under `engram-lib/src/memory/`, `intelligence/`, `ingestion/`, `export/`, `consolidation/` that uses `db.conn.query` or `db.conn.execute` -- convert to `db.read(|c| ...)` or `db.write(|c| ...)` closures
- All existing tests that construct in-memory DBs

## Migration Strategy (Important)

This is a large mechanical change. Do NOT rewrite everything in one commit. Phased:

1. **Commit 1:** Add `db/pool.rs` plus a `DatabaseBackend` enum with `Libsql` and `Pool` variants. Default `Libsql`. Feature flag `db_pool`.
2. **Commit 2-N:** Port one module at a time behind the enum. Each commit runs full test suite against both backends. Recommended order:
   - `db/schema.rs` migrations
   - `memory/mod.rs` CRUD
   - `memory/search.rs`
   - `memory/vector.rs` (already thin since LanceDB landed)
   - `intelligence/contradiction.rs`
   - `intelligence/pagerank.rs`
   - `intelligence/inference.rs`
   - `ingestion/*`
   - `export/*`
   - `consolidation/*`
   - Everything else
3. **Final commit:** Flip default to `Pool`, remove `libsql` from `Cargo.toml`, remove the enum, delete dead libsql paths.

## Conversion Pattern

Before:
```rust
let mut rows = db.conn.query(
    "SELECT id, content FROM memories WHERE id = ?1",
    libsql::params![memory_id]
).await?;
while let Some(row) = rows.next().await? {
    let id: i64 = row.get(0)?;
    let content: String = row.get(1)?;
}
```

After:
```rust
db.read(move |conn| {
    let mut stmt = conn.prepare("SELECT id, content FROM memories WHERE id = ?1")?;
    let rows = stmt.query_map([memory_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
}).await?
```

Transactions:
```rust
db.transaction(move |tx| {
    tx.execute("INSERT INTO memories ...", params![...])?;
    tx.execute("INSERT INTO structured_facts ...", params![...])?;
    Ok(())
}).await?
```

## Pool Metrics

Expose via `/metrics`:
- `db_reader_pool_in_use`
- `db_reader_pool_idle`
- `db_writer_pool_in_use`
- `db_reader_wait_ms` (histogram)
- `db_writer_wait_ms` (histogram)
- `db_busy_retries_total`

## Writer BUSY Handling

SQLite can return `SQLITE_BUSY` under contention even with WAL. Add retry with exponential backoff in the writer path:
```rust
for attempt in 0..5 {
    match try_write().await {
        Err(SqliteError(Code(SQLITE_BUSY), _)) => {
            tokio::time::sleep(Duration::from_millis(10 * 2u64.pow(attempt))).await;
            continue;
        }
        result => return result,
    }
}
```

## Tests

- Unit: pool acquire/release, pragma verification, transaction rollback.
- Unit: BUSY retry path.
- Integration: run entire existing test suite against pooled backend (may need feature flag tests).
- Concurrency: spawn 50 readers and 5 writers, measure no deadlocks, verify reader latency is flat.
- Correctness: compare query results between libsql and pooled paths for a fixture of 1000 queries.
- Load test: 200 concurrent clients, measure tail latency and BUSY retry counts.

## Feature Flag

`db_pool` (default false during migration, true after all modules ported and validated). Final commit removes the flag.

## Verification

- [ ] `libsql` removed from `engram-lib/Cargo.toml` and `engram-server/Cargo.toml`
- [ ] `libsql` KEPT in `engram-cli/Cargo.toml` for Phase 5 migration tool
- [ ] All tests pass against pooled backend
- [ ] 50-reader load test shows flat latency
- [ ] Pool metrics exposed via `/metrics`
- [ ] Writer BUSY retry path tested and working
- [ ] No deadlocks under load
- [ ] Connection pragmas verified via `PRAGMA ...` sanity check at pool init
- [ ] Workspace still compiles with libsql only in engram-cli

## Risks

- **Missed closures**: if a caller awaits inside the closure, we lose pool concurrency. Add a clippy lint if possible.
- **Transaction deadlocks**: SQLite BUSY errors. Retry + backoff handles.
- **WAL checkpoint stalls**: auto-checkpoint pragma mitigates. Monitor WAL file size.
- **Test fixtures**: in-memory SQLite needs a distinct pool config (shared cache or single connection).

## Commit Message Style

```
refactor(db): phase 4 migrate to pooled rusqlite

Replace single libsql connection with deadpool-sqlite reader/writer
pools. Ports memory, intelligence, ingestion, export modules. Writer
retries on SQLITE_BUSY with exponential backoff. Pool metrics exposed
via /metrics. Removes libsql dependency.
```

No em dashes. Use -- or rewrite.
