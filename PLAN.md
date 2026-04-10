# Task: Background PageRank with Cached Scores

**Branch:** feat/scale-pagerank
**Phase:** 3
**Effort:** ~10 hours
**Model:** Sonnet (medium complexity, isolated scope)
**Depends on:** Phase 1-2 (merged to main)

---

## Why

Current PageRank runs on the memory graph at query time. On graphs with 100K+ memories it takes 5-10 seconds, producing P99 spikes in search latency. The graph changes slowly relative to read QPS, so we are recomputing nearly-identical results thousands of times.

## Goal

Compute PageRank in a background job, persist to a `memory_pagerank` table, have search read the cached value. Refresh on a schedule and on dirty threshold.

## Full Spec

See `~/Documents/specs/2026-04-10-kleos-scalability-phase3-5.md` section "Phase 3".

## Files to Create

- `kleos-lib/src/jobs/mod.rs` (if not already present)
- `kleos-lib/src/jobs/pagerank_refresh.rs`

## Files to Modify

- `kleos-lib/src/db/schema.rs` -- add `memory_pagerank` and dirty tracking tables
- `kleos-lib/src/intelligence/pagerank.rs` -- split compute from persist, read from cache
- `kleos-server/src/main.rs` -- start background job
- `kleos-lib/src/config.rs` -- add pagerank config fields
- Memory CRUD paths -- increment dirty counter on insert/delete
- Association/edge CRUD paths -- increment dirty counter

## Schema

```sql
CREATE TABLE IF NOT EXISTS memory_pagerank (
    memory_id INTEGER PRIMARY KEY,
    user_id TEXT NOT NULL,
    score REAL NOT NULL,
    computed_at INTEGER NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX idx_pagerank_user ON memory_pagerank(user_id);
CREATE INDEX idx_pagerank_score ON memory_pagerank(score DESC);

CREATE TABLE IF NOT EXISTS pagerank_dirty (
    user_id TEXT PRIMARY KEY,
    dirty_count INTEGER NOT NULL DEFAULT 0,
    last_refresh INTEGER NOT NULL DEFAULT 0
);
```

## Steps

1. Add migrations for `memory_pagerank` and `pagerank_dirty` tables.
2. Extract current `compute_pagerank` into a pure function `compute_pagerank_for_user(db, user_id) -> Vec<(i64, f64)>` that returns scores without writing.
3. Add `persist_pagerank(db, user_id, scores)` writer that upserts into the cache table and resets the dirty counter.
4. Add dirty increment helpers: `mark_pagerank_dirty(db, user_id, delta)` called from memory and edge CRUD paths.
5. Create `PagerankRefreshJob` with config-driven interval (default 300s) and dirty threshold (default 100 changes).
6. Job loop: every tick, list users with `dirty_count >= threshold` OR `last_refresh > interval ago`, cap concurrent users (default 2), recompute and persist.
7. Update search ranking to read from `memory_pagerank` instead of computing. If row missing for a memory, treat score as baseline (0.0 or mean).
8. First-query fallback: if `memory_pagerank` is empty for a user, run synchronous compute once and persist. Subsequent queries use cache.
9. Add config: `pagerank_refresh_interval_secs`, `pagerank_dirty_threshold`, `pagerank_max_concurrent`, `pagerank_enabled` (flag, default true).
10. Wire job start in `kleos-server/src/main.rs` alongside other background tasks with `CancellationToken`.
11. Add admin endpoint `POST /admin/pagerank/rebuild` for manual refresh (respects user_id query param or rebuilds all).

## Tests

- Unit: dirty counter increments on memory insert and delete.
- Unit: `persist_pagerank` upserts correctly and zeroes dirty counter.
- Integration: store 100 memories, wait for job tick, verify `memory_pagerank` populated.
- Integration: search that used to take 5s now returns <100ms after cache warm.
- Regression: first query after fresh install still returns reasonable ordering via fallback.
- Benchmark: measure search P99 before and after.

## Feature Flag

`pagerank_cached` (default true). If false, revert to synchronous compute. Table creation is additive and safe.

## Verification

- [ ] `memory_pagerank` populates for test users after job runs
- [ ] Search P99 drops by 5-10s in scenarios with large graphs
- [ ] First-query fallback works on fresh DB
- [ ] Admin rebuild endpoint works for single user and all users
- [ ] All existing search tests pass
- [ ] Feature flag off path still compiles and works

## Commit Message Style

```
feat(scale): phase 3 background pagerank with cached scores

Replace synchronous PageRank compute on search path with background
refresh job and memory_pagerank cache table. Dirty tracking triggers
recompute when thresholds exceeded. Removes 5-10s P99 latency spikes.
```

No em dashes in commit messages. Use -- or rewrite.
