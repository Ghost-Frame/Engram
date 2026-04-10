# Task: LanceDB HNSW Vector Search

**Branch:** feat/scale-lance
**Effort:** 12 hours
**Model:** Opus (complex integration)

## Goal

Replace brute-force vector_top_k with LanceDB HNSW index.
Target: 500ms -> 5ms at 1M memories.

## Architecture

SQLite remains source of truth. LanceDB stores only: memory_id, user_id, embedding vector.

Store: SQLite first, then LanceDB
Search: LanceDB for top-k ids, SQLite for full records
Delete: Both

## Files

- Create: kleos-lib/src/vector/mod.rs
- Create: kleos-lib/src/vector/lance.rs
- Modify: kleos-lib/Cargo.toml
- Modify: kleos-lib/src/lib.rs
- Modify: kleos-lib/src/config.rs
- Modify: kleos-lib/src/db/mod.rs
- Modify: kleos-lib/src/memory/mod.rs
- Modify: kleos-lib/src/memory/search.rs

---

## Step 1: Add Dependencies

In kleos-lib/Cargo.toml, add:
- lancedb = "0.4"
- arrow-array = "51"
- arrow-schema = "51"
- async-trait = "0.1"

- [ ] Add dependencies
- [ ] cargo check -p kleos-lib

---

## Step 2: Create Vector Module

Create kleos-lib/src/vector/mod.rs with:
- VectorHit struct (memory_id, distance, rank)
- VectorIndex trait (insert, search, delete, count)
- pub use lance::LanceIndex

Add pub mod vector; to lib.rs

---

## Step 3: Implement LanceIndex

Create kleos-lib/src/vector/lance.rs:

LanceIndex struct:
- db: lancedb::Connection
- table: RwLock<Option<lancedb::Table>>
- dimensions: usize

Methods:
- open(path, dimensions) - connect to lance, open or create table
- ensure_table() - create table with schema if not exists
- insert() - add record with id, user_id, vector
- search() - vector_search with user_id filter
- delete() - delete by id
- count() - count_rows

Schema: id (i64), user_id (i64), vector (FixedSizeList f32 x 1024)

Use arrow-array for RecordBatch construction.
Use futures::TryStreamExt for collecting search results.

---

## Step 4: Update Config

Add to Config struct:
- lance_index_path: Option<String>
- vector_dimensions: usize (default 1024)
- use_lance_index: bool (default true)

Env vars:
- KLEOS_LANCE_INDEX_PATH
- KLEOS_VECTOR_DIMENSIONS
- KLEOS_USE_LANCE_INDEX

---

## Step 5: Update Database Struct

Add field: vector_index: Option<Arc<dyn VectorIndex>>

In connect(), if use_lance_index is true:
- Compute lance_path (config or data_dir/lance)
- Open LanceIndex
- Store as Some(Arc::new(idx))
- On error, warn and use None (graceful degradation)

---

## Step 6: Update Store Flow

In memory/mod.rs store():
After SQLite insert and embedding generation:
- If vector_index exists and embedding exists
- Call index.insert(memory_id, user_id, embedding)
- Log warning on error, do not fail

---

## Step 7: Update Search Flow

In memory/search.rs hybrid_search():
Replace vector_search call with:
- If vector_index exists, use index.search()
- On error or if index is None, fallback to vector_search()
- Convert hits to expected format

---

## Step 8: Update Delete Flow

In memory delete/mark_forgotten:
- If vector_index exists
- Call index.delete(memory_id)
- Log warning on error

---

## Step 9: Migration Function

Create build_lance_index_from_existing(db):
- Query all memories with embeddings
- For each, insert into vector_index
- Log progress every 1000
- Return count

---

## Step 10: Test and Commit

- [ ] cargo check -p kleos-lib
- [ ] cargo test -p kleos-lib
- [ ] cargo clippy -p kleos-lib
- [ ] git add -A
- [ ] git commit -m "feat(vector): add LanceDB HNSW index for vector search"

---

## Done When

- LanceDB compiles and initializes
- Store inserts into both
- Search uses Lance with fallback
- Delete removes from both
- Tests pass
- Commit made
