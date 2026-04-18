# Query plans for hot paths

This file captures `EXPLAIN QUERY PLAN` output for the five SQL queries
that dominate engram-rust's read traffic, plus the index each plan is
expected to use. If you change the schema or a hot query, rerun
`EXPLAIN QUERY PLAN` against a representative database and update the
expected plan here.

Generated against migration 20 (see
`engram-lib/src/db/migrations.rs`) with the memory-links covering
indices from plan 3.1.

## How to reproduce

```sh
sqlite3 "$ENGRAM_DB_PATH" <<'SQL'
.headers on
.mode column
.eqp on
-- paste the query from one of the sections below, with placeholder
-- bind parameters replaced by concrete test values.
SQL
```

Alternatively, from a connected `rusqlite` session:

```rust
let plan: Vec<(i64, i64, i64, String)> = conn
    .prepare("EXPLAIN QUERY PLAN <sql>")?
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
    .collect::<rusqlite::Result<_>>()?;
```

---

## 1. Hybrid FTS stage -- `memory/fts.rs::fts_search`

**Call site:** `engram-lib/src/memory/fts.rs:57`

```sql
SELECT m.id, -memories_fts.rank as bm25_score
FROM memories_fts
JOIN memories m ON m.id = memories_fts.rowid
WHERE memories_fts MATCH ?1
  AND m.is_forgotten = 0
  AND m.is_latest = 1
  AND m.is_consolidated = 0
  AND m.user_id = ?2
ORDER BY memories_fts.rank
LIMIT ?3;
```

**Expected plan:**

```
SCAN memories_fts VIRTUAL TABLE INDEX 2:(MATCH ?)
SEARCH m USING INTEGER PRIMARY KEY (rowid=?)
```

**Notes:** FTS5 drives the outer loop via the MATCH virtual index. The
join to `memories` is by rowid (PK), which is O(log n) per row. The
`is_forgotten`/`is_latest`/`is_consolidated`/`user_id` filters fall
through as residual predicates after the PK lookup; we accept that
because FTS limits the candidate set to `LIMIT` rows before filtering.

**Watch for:** a plan that performs `SCAN memories` would indicate the
FTS index is not being used -- check that the `memories_fts` virtual
table still exists.

---

## 2. Graph neighbor fetch -- `memory/search.rs::fetch_graph_neighbors`

**Call site:** `engram-lib/src/memory/search.rs:234`

```sql
SELECT ml.target_id, ml.similarity, ml.type, m.content, m.category,
       m.importance, m.created_at, m.is_latest, m.is_forgotten,
       m.version, m.source_count, m.model, m.source
FROM memory_links ml
JOIN memories m ON m.id = ml.target_id
WHERE ml.source_id = ?1 AND m.user_id = ?2
UNION
SELECT ml.source_id, ml.similarity, ml.type, m.content, m.category,
       m.importance, m.created_at, m.is_latest, m.is_forgotten,
       m.version, m.source_count, m.model, m.source
FROM memory_links ml
JOIN memories m ON m.id = ml.source_id
WHERE ml.target_id = ?1 AND m.user_id = ?2;
```

**Expected plan (each UNION arm):**

```
SEARCH ml USING COVERING INDEX idx_links_source_covering (source_id=?)
SEARCH m USING INTEGER PRIMARY KEY (rowid=?)
USE TEMP B-TREE FOR UNION
```

The second arm uses `idx_links_target_covering (target_id=?)`.

**Notes:** The covering indices `(source_id, target_id, similarity, type)`
and `(target_id, source_id, similarity, type)` were added in migration
20 specifically so the planner can satisfy the UNION's link-row columns
from the index alone, avoiding a rowid lookup on `memory_links` per
neighbor. Only `memories` requires a PK lookup, which is unavoidable
because we pull the full memory payload.

**Watch for:** `SEARCH ml USING INDEX idx_links_source` (the
non-covering index) indicates the covering index was not picked;
re-run `ANALYZE` or check migration state.

---

## 3. Static memory fetch -- `memory/mod.rs::get_memories`

**Call site:** `engram-lib/src/memory/mod.rs` (paginated list endpoint)

```sql
SELECT id, content, category, importance, created_at, updated_at,
       user_id, session_id, source, version, parent_memory_id,
       is_latest, is_forgotten, source_count, model
FROM memories
WHERE user_id = ?1
  AND is_forgotten = 0
  AND is_latest = 1
ORDER BY created_at DESC
LIMIT ?2 OFFSET ?3;
```

**Expected plan:**

```
SEARCH memories USING INDEX idx_memories_user_created
   (user_id=? AND created_at<?)
```

**Notes:** Requires `idx_memories_user_created(user_id, created_at DESC,
is_forgotten, is_latest)` from migration 13. The ORDER BY matches the
index direction, so no temp sort is needed. OFFSET scans are linear --
clients should paginate with a `created_at` cursor for deep pages.

**Watch for:** `USE TEMP B-TREE FOR ORDER BY` means the index lost the
DESC direction; fix the index DESC specifier.

---

## 4. PageRank neighbor weight aggregation -- `graph/pagerank.rs`

**Call site:** `engram-lib/src/graph/pagerank.rs:490`

```sql
SELECT target_id, similarity, type
FROM memory_links
WHERE source_id = ?1;
```

**Expected plan:**

```
SEARCH memory_links USING COVERING INDEX idx_links_source_covering
   (source_id=?)
```

**Notes:** Pure index scan; no `memories` join, so the covering index
supplies everything the read needs. This runs once per vertex per
PageRank iteration, so index-only retrieval is critical -- a
non-covering index would force a per-row rowid lookup on
`memory_links`, adding a ~10x latency multiplier on hot tenants.

**Watch for:** `SEARCH memory_links USING INDEX idx_links_source`
rather than the covering variant -- same fix as query #2.

---

## 5. Entity-filtered FTS inside graph search -- `graph/entities.rs`

**Call site:** `engram-lib/src/graph/entities.rs:559`

```sql
SELECT m.id, m.content, m.category, m.importance, m.created_at
FROM memories m
WHERE m.user_id = ?1
  AND m.is_forgotten = 0
  AND m.id IN (SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?3)
  AND m.category = ?2
ORDER BY m.created_at DESC
LIMIT ?4;
```

**Expected plan:**

```
SEARCH m USING INDEX idx_memories_user_category_created
   (user_id=? AND category=?)
LIST SUBQUERY
  SCAN memories_fts VIRTUAL TABLE INDEX 2:(MATCH ?)
```

**Notes:** The outer memory scan is driven by the composite
`(user_id, category, created_at DESC)` index (migration 14), and the
FTS subquery is evaluated once and materialized into a list. The
`IN (SELECT rowid ...)` shape keeps the FTS set small enough to fit in
memory for realistic queries.

**Watch for:** `SCAN memories` means the composite index is missing or
the query planner has stale stats; run `ANALYZE` and recheck.

---

## Maintenance

- After schema migrations that touch `memories`, `memory_links`, or
  `memories_fts`, re-run `EXPLAIN QUERY PLAN` for each section and
  update the expected plan strings above.
- Run `ANALYZE;` periodically on production databases; the stats power
  SQLite's plan choices and go stale as the data distribution shifts.
- If a production trace shows a query taking longer than expected,
  confirm the plan matches what is documented here before changing
  application code.
