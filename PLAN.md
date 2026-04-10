# PLAN: feat/intelligence-endpoints -- Missing Intelligence & Analysis Endpoints

## Overview

The intelligence module in engram-rust has solid lib-layer implementations for consolidation, contradiction, decomposition, temporal, digests, reflections, and causal chains. These are already wired to `/intelligence/*` routes. However, the TS source has many more intelligence-adjacent endpoints that are NOT yet implemented in Rust. This worktree fills those gaps.

Additionally: wire eidolon's neural intelligence features (Hopfield pattern completion, dream cycles, interference resolution, growth reflection) as optional enhanced backends for these operations.

## Current State

### Already implemented (lib + routes):
- `/intelligence/consolidate` POST -- run consolidation
- `/intelligence/consolidation-candidates` POST -- find candidates
- `/intelligence/consolidations` GET -- list past consolidations
- `/intelligence/contradictions/{memory_id}` GET -- detect contradictions for a memory
- `/intelligence/contradictions` POST -- scan all contradictions
- `/intelligence/decompose/{memory_id}` POST -- decompose memory into facts
- `/intelligence/temporal/detect` POST -- detect temporal patterns
- `/intelligence/temporal/patterns` GET -- list patterns
- `/intelligence/digests/generate` POST -- generate digest
- `/intelligence/digests` GET -- list digests
- `/intelligence/reflections` POST/GET -- create/list reflections
- `/intelligence/causal/chains` POST/GET -- create/list causal chains
- `/intelligence/causal/chains/{id}` GET -- get chain
- `/intelligence/causal/links` POST -- add causal link

### Existing lib modules WITHOUT routes:
- `intelligence/sentiment.rs` (223 lines) -- sentiment analysis
- `intelligence/valence.rs` (169 lines) -- emotional valence scoring
- `intelligence/predictive.rs` (255 lines) -- predictive recall
- `intelligence/reconsolidation.rs` (331 lines) -- memory reconsolidation
- `intelligence/growth.rs` (412 lines) -- growth observations
- `intelligence/extraction.rs` (348 lines) -- fact extraction

### Missing from TS (no lib OR route):
- `/timetravel` POST -- query memories at a point in time
- `/sweep` POST -- consolidation sweep (batch consolidation)
- `/correct` POST -- memory correction pipeline
- `/memory-health` GET -- memory health report
- `/feedback` POST + `/feedback/stats` GET -- feedback mechanism
- `/duplicates` GET -- find duplicate memories
- `/deduplicate` POST -- merge/remove duplicates

---

## Prerequisites

- `cargo check --workspace` passes before starting
- Read ALL files in `engram-lib/src/intelligence/` to understand existing implementations
- Read `engram-server/src/routes/intelligence.rs` for existing route patterns
- Read `engram-server/src/routes/growth.rs` for growth endpoint patterns
- Read the TS source at approximately lines 2120-3900 of `engram/src/routes/index.ts` for the missing endpoint implementations

---

## Step 1: Wire Existing Lib Modules to Routes

### 1a. Sentiment Analysis Endpoints

**Read:** `engram-lib/src/intelligence/sentiment.rs`

Identify the public functions. Expected: `analyze_sentiment(content: &str) -> SentimentResult` or similar.

**Add routes to `engram-server/src/routes/intelligence.rs`:**

- `POST /intelligence/sentiment/analyze` -- analyze sentiment of provided text
  - Body: `{ "content": "text to analyze" }` OR `{ "memory_id": 123 }`
  - If memory_id provided, fetch memory content first (with user_id scoping!)
  - Returns: `{ "score": 0.7, "label": "positive", ... }`

- `GET /intelligence/sentiment/history` -- sentiment trend for a user's memories
  - Query params: `?limit=50&since=2024-01-01`
  - Returns array of `{ "memory_id": N, "score": F, "created_at": "..." }`

### 1b. Valence (Emotional) Endpoints

**Read:** `engram-lib/src/intelligence/valence.rs`

Identify functions. Expected: `store_valence()`, `get_valence()`, or similar.

**Add routes:**

- `POST /intelligence/valence/score` -- compute and store valence for a memory
  - Body: `{ "memory_id": 123 }`
  - Must validate memory belongs to auth.user_id!
  - Returns: `{ "valence": 0.5, "arousal": 0.3, "emotion": "neutral" }`

- `GET /intelligence/valence/{memory_id}` -- get stored valence
  - Must validate memory belongs to auth.user_id!

### 1c. Predictive Recall Endpoints

**Read:** `engram-lib/src/intelligence/predictive.rs`

**Add routes:**

- `POST /intelligence/predictive/recall` -- predict what memories will be needed
  - Body: `{ "context": "what the user is working on", "limit": 5 }`
  - Returns: array of predicted memories with confidence scores

- `GET /intelligence/predictive/patterns` -- list learned prediction patterns

### 1d. Reconsolidation Endpoints

**Read:** `engram-lib/src/intelligence/reconsolidation.rs`

**Add routes:**

- `POST /intelligence/reconsolidate/{memory_id}` -- trigger reconsolidation for a memory
  - Reconsolidation updates a memory's representation based on new context
  - Body: `{ "new_context": "additional information" }`
  - Must validate memory belongs to auth.user_id!

- `GET /intelligence/reconsolidation/candidates` -- find memories that need reconsolidation
  - Query: `?limit=20`

### 1e. Extraction Endpoints

**Read:** `engram-lib/src/intelligence/extraction.rs`

Expected: `extract_facts(content: &str) -> Vec<Fact>` or similar.

**Add routes:**

- `POST /intelligence/extract` -- extract facts from text or memory
  - Body: `{ "content": "text" }` OR `{ "memory_id": 123 }`
  - Returns: `{ "facts": [...], "entities": [...] }`

---

## Step 2: Implement Missing TS Endpoints

### 2a. Time Travel

**Endpoint:** `POST /timetravel`

**What it does in TS (line ~2348):** Retrieves the state of memories at a specific point in time using version chains and created_at timestamps.

**Implement in lib:** `engram-lib/src/intelligence/temporal.rs` (extend existing file)

```rust
pub async fn time_travel(
    db: &Database,
    user_id: i64,
    query: &str,
    timestamp: &str,
    limit: i64,
) -> Result<Vec<TimeTravelResult>> {
    // 1. Find memories that existed at the given timestamp
    // 2. For versioned memories, find the version that was current at that time
    // 3. Run search against those historical states
    let mut rows = db.conn.query(
        "SELECT id, content, category, source, importance, created_at \
         FROM memories \
         WHERE user_id = ?1 AND created_at <= ?2 AND is_forgotten = 0 \
         ORDER BY created_at DESC LIMIT ?3",
        libsql::params![user_id, timestamp, limit],
    ).await?;
    // ... collect and return ...
}
```

**Add type in `intelligence/types.rs`:**
```rust
#[derive(Debug, Serialize)]
pub struct TimeTravelResult {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub created_at: String,
    pub was_current_version: bool,
}
```

**Route handler:**
```rust
#[derive(Deserialize)]
struct TimeTravelBody {
    query: Option<String>,
    timestamp: String,  // ISO 8601
    #[serde(default = "default_limit")]
    limit: i64,
}

async fn timetravel_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<TimeTravelBody>,
) -> Result<Json<Value>, AppError> {
    let results = engram_lib::intelligence::temporal::time_travel(
        &state.db, auth.user_id, body.query.as_deref().unwrap_or(""), &body.timestamp, body.limit,
    ).await?;
    Ok(Json(json!({ "results": results, "timestamp": body.timestamp })))
}
```

**Register:** `.route("/timetravel", post(timetravel_handler))`

### 2b. Consolidation Sweep

**Endpoint:** `POST /sweep`

**What it does in TS (line ~4674):** Runs a full consolidation sweep across all memories -- finds similar clusters and merges/summarizes them.

**Implement in lib:** Extend `engram-lib/src/intelligence/consolidation.rs`

```rust
pub async fn sweep(
    db: &Database,
    user_id: i64,
    threshold: f64,  // similarity threshold, default 0.85
) -> Result<SweepResult> {
    // 1. Find all consolidation candidates (reuse find_consolidation_candidates)
    // 2. For each candidate pair above threshold, consolidate
    // 3. Return summary of what was consolidated
    let candidates = find_consolidation_candidates(db, user_id, threshold, 100).await?;
    let mut consolidated = 0;
    for pair in &candidates {
        // Auto-consolidate each pair
        consolidate(db, user_id, pair.memory_a, pair.memory_b).await?;
        consolidated += 1;
    }
    Ok(SweepResult { pairs_found: candidates.len() as i64, consolidated })
}
```

**Add type:**
```rust
#[derive(Debug, Serialize)]
pub struct SweepResult {
    pub pairs_found: i64,
    pub consolidated: i64,
}
```

**Register:** `.route("/sweep", post(sweep_handler))`

### 2c. Memory Correction

**Endpoint:** `POST /correct`

**What it does in TS (line ~3471):** Stores a correction for an existing memory -- creates a new version that supersedes the old one.

**Implement in lib:** `engram-lib/src/memory/mod.rs` or new file `engram-lib/src/intelligence/correction.rs`

```rust
pub async fn correct_memory(
    db: &Database,
    user_id: i64,
    memory_id: i64,
    corrected_content: &str,
    reason: Option<&str>,
) -> Result<Memory> {
    // 1. Verify original memory belongs to user
    let original = memory::get(db, memory_id, user_id).await?;
    // 2. Create new memory with corrected content
    let new_memory = memory::store(db, StoreRequest {
        content: corrected_content.to_string(),
        category: original.category.clone(),
        source: original.source.clone(),
        importance: original.importance,
        user_id,
        // ... copy other fields ...
    }).await?;
    // 3. Link new -> old with "supersedes" relationship
    memory::insert_link(db, new_memory.id, memory_id, 1.0, "supersedes", user_id).await?;
    // 4. Mark original as superseded
    db.conn.execute(
        "UPDATE memories SET is_superseded = 1 WHERE id = ?1 AND user_id = ?2",
        libsql::params![memory_id, user_id],
    ).await?;
    Ok(new_memory)
}
```

**Route body:**
```rust
#[derive(Deserialize)]
struct CorrectBody {
    memory_id: i64,
    content: String,
    reason: Option<String>,
}
```

**Register:** `.route("/correct", post(correct_handler))`

### 2d. Memory Health Report

**Endpoint:** `GET /memory-health`

**What it does in TS (line ~3625):** Returns health stats about the user's memories -- how many have embeddings, how many are orphaned, average importance, staleness, etc.

**Implement in lib:** `engram-lib/src/intelligence/mod.rs` or new file `health.rs`

```rust
pub async fn memory_health(db: &Database, user_id: i64) -> Result<MemoryHealthReport> {
    let total = count_query(db, "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0", user_id).await?;
    let no_embedding = count_query(db, "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND embedding IS NULL", user_id).await?;
    let archived = count_query(db, "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_archived = 1", user_id).await?;
    let with_links = count_query(db, "SELECT COUNT(DISTINCT source_id) FROM links WHERE source_id IN (SELECT id FROM memories WHERE user_id = ?1)", user_id).await?;

    // Average importance
    let mut rows = db.conn.query(
        "SELECT AVG(importance) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
        libsql::params![user_id],
    ).await?;
    let avg_importance: f64 = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or(5.0),
        None => 5.0,
    };

    // Oldest memory
    let mut rows = db.conn.query(
        "SELECT MIN(created_at) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
        libsql::params![user_id],
    ).await?;
    let oldest: String = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or_default(),
        None => String::new(),
    };

    Ok(MemoryHealthReport {
        total_memories: total,
        without_embeddings: no_embedding,
        archived,
        with_links,
        orphaned: total - with_links,
        avg_importance,
        oldest_memory: oldest,
        embedding_coverage_pct: if total > 0 { ((total - no_embedding) as f64 / total as f64) * 100.0 } else { 0.0 },
    })
}

async fn count_query(db: &Database, sql: &str, user_id: i64) -> Result<i64> {
    let mut rows = db.conn.query(sql, libsql::params![user_id]).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0).unwrap_or(0)),
        None => Ok(0),
    }
}
```

**Register:** `.route("/memory-health", get(memory_health_handler))`

### 2e. Feedback System

**Endpoints:** `POST /feedback` and `GET /feedback/stats`

**What it does in TS (line ~3733):** Users mark memories as helpful/irrelevant/off-topic. This adjusts importance scores and tracks feedback stats.

**Implement in lib:** New file `engram-lib/src/intelligence/feedback.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct FeedbackRequest {
    pub memory_id: i64,
    pub rating: String,  // "helpful", "irrelevant", "off-topic", "outdated"
    pub context: Option<String>,
}

pub async fn record_feedback(
    db: &Database,
    user_id: i64,
    req: &FeedbackRequest,
) -> Result<()> {
    // 1. Verify memory belongs to user
    // 2. Store feedback
    db.conn.execute(
        "INSERT INTO memory_feedback (memory_id, user_id, rating, context, created_at) \
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        libsql::params![req.memory_id, user_id, req.rating, req.context],
    ).await?;
    // 3. Adjust importance based on feedback
    let adjustment = match req.rating.as_str() {
        "helpful" => 1,
        "irrelevant" | "off-topic" => -1,
        "outdated" => -2,
        _ => 0,
    };
    if adjustment != 0 {
        db.conn.execute(
            "UPDATE memories SET importance = MAX(1, MIN(10, importance + ?1)) \
             WHERE id = ?2 AND user_id = ?3",
            libsql::params![adjustment, req.memory_id, user_id],
        ).await?;
    }
    Ok(())
}

pub async fn feedback_stats(db: &Database, user_id: i64) -> Result<Value> {
    // Count by rating type
    let mut rows = db.conn.query(
        "SELECT rating, COUNT(*) FROM memory_feedback WHERE user_id = ?1 GROUP BY rating",
        libsql::params![user_id],
    ).await?;
    let mut stats = serde_json::Map::new();
    while let Some(row) = rows.next().await? {
        let rating: String = row.get(0).unwrap_or_default();
        let count: i64 = row.get(1).unwrap_or(0);
        stats.insert(rating, json!(count));
    }
    Ok(json!({ "feedback_counts": stats }))
}
```

**IMPORTANT:** This requires a `memory_feedback` table. Check if it exists in the schema. If not, create it:
```sql
CREATE TABLE IF NOT EXISTS memory_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    rating TEXT NOT NULL,
    context TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (memory_id) REFERENCES memories(id)
);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_memory ON memory_feedback(memory_id);
```

Add this migration to `engram-lib/src/db/` (check existing migration pattern).

**Register:** `.route("/feedback", post(feedback_handler))` `.route("/feedback/stats", get(feedback_stats_handler))`

### 2f. Duplicate Detection

**Endpoint:** `GET /duplicates`

**What it does in TS (line ~4231):** Finds near-duplicate memories using SimHash or embedding similarity.

**Implement in lib:** New file `engram-lib/src/intelligence/duplicates.rs`

```rust
pub async fn find_duplicates(
    db: &Database,
    user_id: i64,
    threshold: f64,  // default 0.9
    limit: i64,
) -> Result<Vec<DuplicatePair>> {
    // Strategy 1: Check simhash column if it exists
    // Strategy 2: Find memories with very similar content using FTS overlap
    // Strategy 3: If embeddings available, find high-cosine-similarity pairs
    
    // Simplest approach: use the links table where type = 'similarity' and score > threshold
    let mut rows = db.conn.query(
        "SELECT l.source_id, l.target_id, l.score, m1.content, m2.content \
         FROM links l \
         JOIN memories m1 ON l.source_id = m1.id \
         JOIN memories m2 ON l.target_id = m2.id \
         WHERE m1.user_id = ?1 AND m2.user_id = ?1 \
         AND l.score >= ?2 AND l.link_type = 'similarity' \
         AND m1.is_forgotten = 0 AND m2.is_forgotten = 0 \
         ORDER BY l.score DESC LIMIT ?3",
        libsql::params![user_id, threshold, limit],
    ).await?;
    // ... collect pairs ...
}
```

**Register:** `.route("/duplicates", get(duplicates_handler))`

### 2g. Deduplication

**Endpoint:** `POST /deduplicate`

**What it does in TS (line ~4289):** Merges duplicate memories -- keeps the best one, marks others as superseded.

**Implement in lib:** Extend `duplicates.rs`

```rust
pub async fn deduplicate(
    db: &Database,
    user_id: i64,
    threshold: f64,
    dry_run: bool,
) -> Result<DeduplicateResult> {
    let pairs = find_duplicates(db, user_id, threshold, 100).await?;
    if dry_run {
        return Ok(DeduplicateResult { pairs_found: pairs.len() as i64, merged: 0, dry_run: true });
    }
    let mut merged = 0;
    for pair in &pairs {
        // Keep the one with higher importance, newer date, or more links
        let keep_id = if pair.importance_a >= pair.importance_b { pair.id_a } else { pair.id_b };
        let remove_id = if keep_id == pair.id_a { pair.id_b } else { pair.id_a };
        // Mark the duplicate as superseded
        db.conn.execute(
            "UPDATE memories SET is_superseded = 1 WHERE id = ?1 AND user_id = ?2",
            libsql::params![remove_id, user_id],
        ).await?;
        // Transfer links from removed to kept
        db.conn.execute(
            "UPDATE links SET source_id = ?1 WHERE source_id = ?2",
            libsql::params![keep_id, remove_id],
        ).await?;
        merged += 1;
    }
    Ok(DeduplicateResult { pairs_found: pairs.len() as i64, merged, dry_run: false })
}
```

**Body:** `{ "threshold": 0.9, "dry_run": false }`

**Register:** `.route("/deduplicate", post(deduplicate_handler))`

---

## Step 3: Eidolon Neural Intelligence Integration

If `state.brain` or `state.eidolon_config` is configured, enhance intelligence endpoints with eidolon's neural capabilities.

### 3a. Enhanced Contradiction Detection

When eidolon is available, use its interference resolution (temporal + importance heuristics) alongside the existing regex/FTS-based contradiction detection.

In the `/intelligence/contradictions` POST handler, if brain is available:
```rust
if let Some(brain) = &state.brain {
    // Also query brain for contradiction patterns
    // Merge results with existing contradiction detection
}
```

### 3b. Dream Cycle Endpoint

**Endpoint:** `POST /intelligence/dream`

Triggers eidolon's offline consolidation cycle (replay, merge, prune, discover, decorrelate).

```rust
async fn dream_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let brain = state.brain.as_ref()
        .ok_or_else(|| AppError(EngError::Internal("brain not configured".into())))?;
    // Call brain.dream() or proxy to eidolon /brain/dream
    Ok(Json(json!({ "status": "dream cycle triggered" })))
}
```

### 3c. Growth Observation Integration

The `/growth/*` routes already exist in `engram-server/src/routes/growth.rs`. Verify they call into `intelligence/growth.rs` properly. If growth.rs relies on eidolon's LLM reflection, ensure the LLM client (`state.llm`) is passed through.

---

## Step 4: Database Schema Changes

### 4a. Memory Feedback Table

Add to schema initialization (check `engram-lib/src/db/` for pattern):

```sql
CREATE TABLE IF NOT EXISTS memory_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id),
    user_id INTEGER NOT NULL,
    rating TEXT NOT NULL,
    context TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_memory ON memory_feedback(memory_id);
```

### 4b. Ensure is_superseded Column Exists

Check if `memories` table has `is_superseded` column. If not, add migration:
```sql
ALTER TABLE memories ADD COLUMN is_superseded INTEGER NOT NULL DEFAULT 0;
```

---

## Step 5: Module Registration

### 5a. New lib modules

If you create new files (feedback.rs, duplicates.rs, correction.rs, health.rs), add them to `engram-lib/src/intelligence/mod.rs`:

```rust
pub mod feedback;
pub mod duplicates;
// correction.rs can go in memory/ or intelligence/
```

### 5b. Route updates

Update the router in `engram-server/src/routes/intelligence.rs` to include all new routes:

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        // === Existing routes (do not modify) ===
        .route("/intelligence/consolidate", post(consolidate_handler))
        .route("/intelligence/consolidation-candidates", post(candidates_handler))
        .route("/intelligence/consolidations", get(list_consolidations_handler))
        .route("/intelligence/contradictions/{memory_id}", get(contradictions_handler))
        .route("/intelligence/contradictions", post(scan_contradictions_handler))
        .route("/intelligence/decompose/{memory_id}", post(decompose_handler))
        .route("/intelligence/temporal/detect", post(detect_temporal_handler))
        .route("/intelligence/temporal/patterns", get(list_temporal_handler))
        .route("/intelligence/digests/generate", post(generate_digest_handler))
        .route("/intelligence/digests", get(list_digests_handler))
        .route("/intelligence/reflections", post(create_reflection_handler).get(list_reflections_handler))
        .route("/intelligence/causal/chains", post(create_chain_handler).get(list_chains_handler))
        .route("/intelligence/causal/chains/{id}", get(get_chain_handler))
        .route("/intelligence/causal/links", post(add_link_handler))
        // === New routes ===
        // Sentiment
        .route("/intelligence/sentiment/analyze", post(sentiment_analyze_handler))
        .route("/intelligence/sentiment/history", get(sentiment_history_handler))
        // Valence
        .route("/intelligence/valence/score", post(valence_score_handler))
        .route("/intelligence/valence/{memory_id}", get(get_valence_handler))
        // Predictive
        .route("/intelligence/predictive/recall", post(predictive_recall_handler))
        .route("/intelligence/predictive/patterns", get(predictive_patterns_handler))
        // Reconsolidation
        .route("/intelligence/reconsolidate/{memory_id}", post(reconsolidate_handler))
        .route("/intelligence/reconsolidation/candidates", get(reconsolidation_candidates_handler))
        // Extraction
        .route("/intelligence/extract", post(extract_handler))
        // Time travel
        .route("/timetravel", post(timetravel_handler))
        // Sweep
        .route("/sweep", post(sweep_handler))
        // Correction
        .route("/correct", post(correct_handler))
        // Health
        .route("/memory-health", get(memory_health_handler))
        // Feedback
        .route("/feedback", post(feedback_handler))
        .route("/feedback/stats", get(feedback_stats_handler))
        // Duplicates
        .route("/duplicates", get(duplicates_handler))
        .route("/deduplicate", post(deduplicate_handler))
        // Dream (eidolon)
        .route("/intelligence/dream", post(dream_handler))
}
```

---

## Step 6: Multi-Tenant Compliance

EVERY new handler MUST:
1. Extract `Auth(auth): Auth` and use `auth.user_id`
2. Pass `user_id` to every lib function that touches the database
3. Never allow `WHERE id = ?1` without `AND user_id = ?2`

Audit every new lib function to ensure user_id is in all queries.

---

## Step 7: Verify

1. `cargo check --workspace` -- MUST pass
2. `cargo clippy --workspace` -- fix all warnings
3. `cargo test --workspace` -- all tests pass
4. Test each new endpoint manually or with integration tests
5. Verify multi-tenant isolation: no endpoint leaks data across users
6. If eidolon is not configured, neural endpoints return 503 gracefully (not panic)

---

## Endpoint Checklist

| Endpoint | Lib Exists | Route Exists | Action |
|----------|-----------|-------------|--------|
| POST /intelligence/sentiment/analyze | yes (sentiment.rs) | NO | wire |
| GET /intelligence/sentiment/history | NO | NO | implement + wire |
| POST /intelligence/valence/score | yes (valence.rs) | NO | wire |
| GET /intelligence/valence/{id} | partial | NO | implement + wire |
| POST /intelligence/predictive/recall | yes (predictive.rs) | NO | wire |
| GET /intelligence/predictive/patterns | partial | NO | wire |
| POST /intelligence/reconsolidate/{id} | yes (reconsolidation.rs) | NO | wire |
| GET /intelligence/reconsolidation/candidates | NO | NO | implement + wire |
| POST /intelligence/extract | yes (extraction.rs) | NO | wire |
| POST /timetravel | NO | NO | implement + wire |
| POST /sweep | partial (consolidation) | NO | implement + wire |
| POST /correct | NO | NO | implement + wire |
| GET /memory-health | NO | NO | implement + wire |
| POST /feedback | NO | NO | implement + wire |
| GET /feedback/stats | NO | NO | implement + wire |
| GET /duplicates | NO | NO | implement + wire |
| POST /deduplicate | NO | NO | implement + wire |
| POST /intelligence/dream | NO (eidolon) | NO | implement + wire |
