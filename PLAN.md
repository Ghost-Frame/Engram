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

- Create: engram-lib/src/vector/mod.rs
- Create: engram-lib/src/vector/lance.rs
- Modify: engram-lib/Cargo.toml
- Modify: engram-lib/src/lib.rs
- Modify: engram-lib/src/config.rs
- Modify: engram-lib/src/db/mod.rs
- Modify: engram-lib/src/memory/mod.rs
- Modify: engram-lib/src/memory/search.rs

---

## Step 1: Add Dependencies

In engram-lib/Cargo.toml, add:
- lancedb = "0.4"
- arrow-array = "51"
- arrow-schema = "51"
- async-trait = "0.1"
- futures = "0.3"

Note: `lancedb 0.4` requires Arrow 51 and a `protoc` binary during build. This worktree pins Chrono to `0.4.38` for Arrow 51 compatibility and provides a repo-local vendored `protoc` wrapper through `.cargo/config.toml`.

- [x] Add dependencies
- [x] cargo check -p engram-lib

---

## Step 2: Create Vector Module

Create engram-lib/src/vector/mod.rs with:
- VectorHit struct (memory_id, distance, rank)
- VectorIndex trait (insert, search, delete, count)
- pub use lance::LanceIndex

Add pub mod vector; to lib.rs

---

## Step 3: Implement LanceIndex

Create engram-lib/src/vector/lance.rs:

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
- ENGRAM_LANCE_INDEX_PATH
- ENGRAM_VECTOR_DIMENSIONS
- ENGRAM_USE_LANCE_INDEX

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

- [x] cargo check -p engram-lib
- [x] cargo test -p engram-lib
- [x] cargo clippy -p engram-lib
- [ ] git add -A
- [ ] git commit -m "feat(vector): add LanceDB HNSW index for vector search"

---

## Done When

- [x] LanceDB compiles and initializes
- [x] Store inserts into both
- [x] Search uses Lance with fallback
- [x] Delete removes from both
- [x] Tests pass
- [ ] Commit made

---

# Task: Parallel Reranker with Session Pool

**Branch:** feat/scale-reranker
**Effort:** 6 hours
**Model:** Sonnet

## Goal

Replace single Mutex<Session> with pool of N sessions for parallel ONNX inference.
Target: 1.1s -> 150ms for reranking 12 results.

## Files

- Create: `engram-lib/src/reranker/pool.rs`
- Modify: `engram-lib/src/reranker/mod.rs`
- Modify: `engram-lib/src/config.rs`

---

## Step 1: Create Session Pool

Create `engram-lib/src/reranker/pool.rs`:

```rust
use crate::{EngError, Result};
use ort::session::Session;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore, OwnedSemaphorePermit};

pub struct SessionPool {
    sessions: Vec<Arc<Mutex<Session>>>,
    semaphore: Arc<Semaphore>,
}

pub struct PooledSession {
    session: Arc<Mutex<Session>>,
    _permit: OwnedSemaphorePermit,
}

impl SessionPool {
    pub fn new(sessions: Vec<Session>) -> Self {
        let count = sessions.len();
        Self {
            sessions: sessions.into_iter().map(|s| Arc::new(Mutex::new(s))).collect(),
            semaphore: Arc::new(Semaphore::new(count)),
        }
    }

    pub async fn acquire(&self) -> Result<PooledSession> {
        let permit = self.semaphore.clone().acquire_owned().await
            .map_err(|e| EngError::Internal(format!("semaphore closed: {}", e)))?;

        // Return first available session
        for session in &self.sessions {
            if let Ok(_) = session.try_lock() {
                return Ok(PooledSession {
                    session: session.clone(),
                    _permit: permit,
                });
            }
        }

        // All busy, wait on first
        Ok(PooledSession {
            session: self.sessions[0].clone(),
            _permit: permit,
        })
    }

    pub fn size(&self) -> usize {
        self.sessions.len()
    }
}

impl PooledSession {
    pub async fn lock(&self) -> tokio::sync::MutexGuard<Session> {
        self.session.lock().await
    }
}
```

- [ ] Create the file
- [ ] Add `mod pool; pub use pool::{SessionPool, PooledSession};` to mod.rs

---

## Step 2: Update Config

In `engram-lib/src/config.rs`:

- [ ] Add field to Config struct:
```rust
pub reranker_pool_size: usize,
```

- [ ] Add to from_env():
```rust
reranker_pool_size: std::env::var("ENGRAM_RERANKER_POOL_SIZE")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(4),
```

---

## Step 3: Update Reranker Struct

In `engram-lib/src/reranker/mod.rs`:

- [ ] Change struct:
```rust
pub struct Reranker {
    pool: SessionPool,
    tokenizer: Arc<Tokenizer>,  // Arc for sharing
    max_seq: usize,
    top_k: usize,
}
```

- [ ] Remove unsafe impl Send/Sync (pool handles this)

- [ ] Update new() to create pool of sessions:
```rust
pub async fn new(config: &Config) -> Result<Self> {
    let model_dir = config.model_dir("granite-reranker");
    let (tokenizer_path, model_path) = ensure_reranker_model(&model_dir).await?;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| EngError::Internal(format!("tokenizer: {}", e)))?;

    let mut sessions = Vec::with_capacity(config.reranker_pool_size);
    for i in 0..config.reranker_pool_size {
        let session = Session::builder()
            .map_err(|e| EngError::Internal(format!("ort builder: {}", e)))?
            .with_intra_threads(2)
            .map_err(|e| EngError::Internal(format!("ort threads: {}", e)))?
            .commit_from_file(&model_path)
            .map_err(|e| EngError::Internal(format!("model load: {}", e)))?;
        sessions.push(session);
        tracing::debug!(session = i, "reranker session created");
    }

    info!(
        model = %model_path.display(),
        pool_size = config.reranker_pool_size,
        top_k = config.reranker_top_k,
        "reranker pool initialized"
    );

    Ok(Self {
        pool: SessionPool::new(sessions),
        tokenizer: Arc::new(tokenizer),
        max_seq: 512,
        top_k: config.reranker_top_k,
    })
}
```

---

## Step 4: Extract Scoring Function

- [ ] Create standalone function that takes session as param:
```rust
fn score_pair_inner(
    session: &mut Session,
    tokenizer: &Tokenizer,
    query: &str,
    document: &str,
    max_seq: usize,
) -> Result<f32> {
    // Move existing score_pair logic here
    // Use session directly instead of self.session.lock()
}
```

---

## Step 5: Rewrite rerank_results for Concurrency

- [ ] Replace the sequential loop with parallel execution:
```rust
pub async fn rerank_results(
    &self,
    query: &str,
    results: &mut Vec<crate::memory::types::SearchResult>,
) -> Result<()> {
    if results.is_empty() {
        return Ok(());
    }

    let k = self.top_k.min(results.len());
    let start = std::time::Instant::now();

    // Collect data for parallel tasks
    let tasks: Vec<_> = (0..k).map(|i| {
        let doc = results[i].memory.content.clone();
        let q = query.to_string();
        let tokenizer = self.tokenizer.clone();
        let max_seq = self.max_seq;
        
        // Must acquire session outside spawn_blocking
        let pool = &self.pool;
        async move {
            let pooled = pool.acquire().await?;
            tokio::task::spawn_blocking(move || {
                // Block on async lock inside spawn_blocking
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    let mut session = pooled.lock().await;
                    score_pair_inner(&mut session, &tokenizer, &q, &doc, max_seq)
                })
            }).await.map_err(|e| EngError::Internal(format!("join: {}", e)))?
        }
    }).collect();

    let scores: Vec<Result<f32>> = futures::future::join_all(tasks).await;

    for (i, score_result) in scores.into_iter().enumerate() {
        match score_result {
            Ok(ce_score) => {
                results[i].score = ce_score as f64 * 0.7 + results[i].score * 0.3;
            }
            Err(e) => {
                tracing::warn!(idx = i, err = %e, "rerank score failed");
            }
        }
    }

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let elapsed = start.elapsed();
    tracing::info!(elapsed_ms = elapsed.as_millis(), k = k, "rerank complete");

    Ok(())
}
```

---

## Step 6: Add futures dependency if needed

- [ ] Check if futures is in Cargo.toml, add if missing:
```toml
futures = "0.3"
```

---

## Step 7: Test

- [ ] Run: `cargo test -p engram-lib`
- [ ] Run: `cargo clippy -p engram-lib`
- [ ] Manual benchmark if possible (time reranking)

---

## Step 8: Commit

- [ ] `git add -A`
- [ ] `git commit -m "feat(reranker): add session pool for parallel inference"`

---

## Done When

- Pool struct created and working
- Config accepts ENGRAM_RERANKER_POOL_SIZE
- rerank_results runs in parallel
- Tests pass
- Clippy clean
- Commit made
