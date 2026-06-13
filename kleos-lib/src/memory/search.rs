use super::fts::fts_search;
use super::vector::{chunk_vector_search, vector_search};
use super::{row_to_memory, MEMORY_COLUMNS};
use crate::db::Database;
use crate::memory::scoring::{
    self, blend_strategies, classify_question_mixed, question_strategy, rrf_score,
};
use crate::memory::types::{
    FacetBucket, FacetedSearchRequest, FacetedSearchResponse, LinkedMemory, QuestionType,
    SearchBudget, SearchRequest, SearchResult, TagCooccurrence, VersionChainEntry,
};
use crate::personality;
use crate::validation::{DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT, RERANKER_TOP_K};
use crate::Result;
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Instant;
use tracing::{info, warn};

const DEFAULT_LIMIT: usize = DEFAULT_SEARCH_LIMIT;

/// Hard ceiling on results returned by hybrid_search. Applied at the library
/// level so all consumers (HTTP routes, MCP, sidecar, CLI) inherit the cap.
const MAX_LIMIT: usize = MAX_SEARCH_LIMIT;

// ---------------------------------------------------------------------------
// Search result cache (3.5)
//
// Per-user generation counter + LRU keyed by (user_id, generation, query_hash).
// On any write for a user, bump the generation so old entries auto-miss.
// TTL provides a secondary eviction policy.
// ---------------------------------------------------------------------------

const CACHE_CAPACITY: usize = 2048;
const CACHE_TTL_SECS: u64 = 15;
const N_SHARDS: usize = 32;

/// Cached search results plus their insertion timestamp for TTL eviction.
struct CacheEntry {
    results: Arc<Vec<SearchResult>>,
    inserted: Instant,
}

/// Cache key tuple of user, generation, and parameter hash.
type CacheKey = (i64, u64, u64);

/// Sharded cache state for search results and user generations.
struct SearchCacheShards {
    shards: [Mutex<LruCache<CacheKey, CacheEntry>>; N_SHARDS],
    generations: RwLock<HashMap<i64, u64>>,
}

static SEARCH_CACHE: LazyLock<SearchCacheShards> = LazyLock::new(|| {
    let per_shard_cap = NonZeroUsize::new(CACHE_CAPACITY / N_SHARDS).unwrap();
    SearchCacheShards {
        shards: std::array::from_fn(|_| Mutex::new(LruCache::new(per_shard_cap))),
        generations: RwLock::new(HashMap::new()),
    }
});

#[inline]
/// Map a user and parameter hash to a stable cache shard index.
fn shard_idx(user_id: i64, param_hash: u64) -> usize {
    let h = param_hash
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(user_id as u64);
    (h as usize) & (N_SHARDS - 1)
}

/// Hash ALL search parameters that affect results.
fn hash_search_params(req: &SearchRequest) -> u64 {
    let mut h = DefaultHasher::new();
    req.query.hash(&mut h);
    req.limit.hash(&mut h);
    req.category.hash(&mut h);
    req.source.hash(&mut h);
    req.tags.hash(&mut h);
    req.question_type.hash(&mut h);
    req.space_id.hash(&mut h);
    req.include_forgotten.hash(&mut h);
    req.exclude_consolidated.hash(&mut h);
    req.threshold.map(|t| t.to_bits()).hash(&mut h);
    req.source_filter.hash(&mut h);
    req.include_links.hash(&mut h);
    req.include_archived.hash(&mut h);
    req.include_noise.hash(&mut h);
    req.latest_only.hash(&mut h);
    req.mode.hash(&mut h);
    req.expand_relationships.hash(&mut h);
    req.budget.hash(&mut h);
    h.finish()
}

/// Read a cached search result set if the generation and TTL still match.
fn cache_get(user_id: i64, param_hash: u64) -> Option<Arc<Vec<SearchResult>>> {
    let gen = {
        let gens = SEARCH_CACHE.generations.read().ok()?;
        *gens.get(&user_id).unwrap_or(&0)
    };
    let key = (user_id, gen, param_hash);
    let shard = &SEARCH_CACHE.shards[shard_idx(user_id, param_hash)];
    let mut s = shard.lock().ok()?;
    if let Some(entry) = s.get(&key) {
        if entry.inserted.elapsed().as_secs() < CACHE_TTL_SECS {
            return Some(Arc::clone(&entry.results));
        }
        s.pop(&key);
    }
    None
}

/// Store a search result set in the per-user shard cache.
fn cache_put(user_id: i64, param_hash: u64, results: Arc<Vec<SearchResult>>) {
    let gen = {
        let Ok(gens) = SEARCH_CACHE.generations.read() else {
            return;
        };
        *gens.get(&user_id).unwrap_or(&0)
    };
    let key = (user_id, gen, param_hash);
    let shard = &SEARCH_CACHE.shards[shard_idx(user_id, param_hash)];
    if let Ok(mut s) = shard.lock() {
        s.put(
            key,
            CacheEntry {
                results,
                inserted: Instant::now(),
            },
        );
    }
}

/// Invalidate all cached search results for a user. Call on any memory write.
/// Entries become unreachable via the generation mismatch and age out of their
/// shard naturally; no full sweep needed.
pub fn invalidate_search_cache(user_id: i64) {
    if let Ok(mut gens) = SEARCH_CACHE.generations.write() {
        let gen = gens.entry(user_id).or_insert(0);
        *gen += 1;
    }
}

/// Internal candidate accumulator used during search pipeline.
struct Candidate {
    id: i64,
    content: String,
    category: String,
    source: Option<String>,
    model: Option<String>,
    importance: i32,
    created_at: String,
    version: Option<i32>,
    is_latest: Option<bool>,
    is_static: bool,
    source_count: i32,
    /// Lineage is resolved later from the hydrated `Memory` row
    /// (`r.memory.root_memory_id`), so this slot stays `None` on the
    /// Candidate itself. Kept so the Candidate shape stays aligned with
    /// the memories table if a future consolidation step wants to read
    /// lineage before hydration.
    #[allow(dead_code)]
    root_memory_id: Option<i64>,
    access_count: i32,
    pagerank_score: f64,
    /// Per-memory FSRS stability (in days). None when the column is NULL --
    /// new or never-reviewed memories. The decay block falls back to
    /// `initial_stability(Rating::Good)` in that case.
    fsrs_stability: Option<f64>,
    semantic_score: Option<f64>,
    personality_signal_score: Option<f64>,
    score: f64,
    combined_score: f64,
    decay_score: Option<f64>,
    temporal_boost: Option<f64>,
    rrf_pre_boost: Option<f64>,
    verbose_decay_factor: Option<f64>,
    verbose_pr_boost: Option<f64>,
    verbose_src_boost: Option<f64>,
    verbose_stat_boost: Option<f64>,
    verbose_contradiction: Option<f64>,
    is_archived: bool,
    is_consolidated: bool,
}

/// Hydrated memory columns needed to finish score assembly.
struct HydratedCandidateRow {
    id: i64,
    created_at: String,
    importance: i32,
    is_static: bool,
    source_count: i32,
    version: Option<i32>,
    is_latest: Option<bool>,
    source: Option<String>,
    model: Option<String>,
    access_count: i32,
    pagerank_score: f64,
    fsrs_stability: Option<f64>,
    content: String,
    category: String,
    is_archived: bool,
    is_consolidated: bool,
}

/// Joined memory metadata for graph expansion candidates.
struct GraphExpansionRow {
    link_id: i64,
    similarity: f64,
    link_type: String,
    content: String,
    category: String,
    importance: i32,
    created_at: String,
    is_latest: bool,
    is_forgotten: bool,
    version: Option<i32>,
    source_count: i32,
    model: Option<String>,
    source: Option<String>,
}

/// Convert a vector distance into a bounded semantic score.
fn semantic_score_from_distance(distance: f64) -> f64 {
    (1.0 - distance).clamp(0.0, 1.0)
}

/// Apply the personality multiplier and keep both score fields coherent.
fn apply_personality_boost(
    candidate: &mut Candidate,
    strategy: &crate::memory::types::SearchStrategy,
) {
    if strategy.include_personality_signals && !candidate.content.is_empty() {
        let signals = personality::detect_signals(&candidate.content);
        if !signals.is_empty() {
            let avg_intensity = signals.iter().map(|(_, v)| v).sum::<f64>() / signals.len() as f64;
            let clamped = avg_intensity.clamp(0.0, 1.0);
            candidate.personality_signal_score = Some((clamped * 1000.0).round() / 1000.0);
            candidate.score *= 1.0 + clamped * strategy.personality_weight;
            candidate.combined_score = candidate.score;
        }
    }
}

/// Apply a graph RRF increment without discarding earlier additive boosts.
fn apply_graph_rrf_increment(candidate: &mut Candidate, rrf_delta: f64) {
    candidate.score += rrf_delta;
    candidate.combined_score = candidate.score;
}

/// Insert graph-hop candidates while enforcing a global hop1 cap.
fn inject_graph_hop1_neighbors(
    results: &mut HashMap<i64, Candidate>,
    graph_score_map: &mut HashMap<i64, f64>,
    neighbor_results: Vec<Vec<GraphExpansionRow>>,
    strategy: &crate::memory::types::SearchStrategy,
) {
    let mut added = 0usize;

    for rows in neighbor_results.into_iter() {
        for row in rows {
            if added >= strategy.hop1_limit {
                break;
            }
            if row.is_forgotten {
                continue;
            }

            let tw = scoring::link_type_weight(&row.link_type);
            let gs = row.similarity * tw * strategy.relationship_multiplier;
            let prev = graph_score_map.get(&row.link_id).copied().unwrap_or(0.0);
            graph_score_map.insert(row.link_id, prev.max(gs));

            if let std::collections::hash_map::Entry::Vacant(e) = results.entry(row.link_id) {
                e.insert(Candidate {
                    id: row.link_id,
                    content: row.content,
                    category: row.category,
                    source: row.source,
                    model: row.model,
                    importance: row.importance,
                    created_at: row.created_at,
                    version: row.version,
                    is_latest: Some(row.is_latest),
                    is_static: false,
                    source_count: row.source_count,
                    root_memory_id: None,
                    access_count: 0,
                    pagerank_score: 0.0,
                    fsrs_stability: None,
                    semantic_score: None,
                    personality_signal_score: None,
                    score: 0.0,
                    combined_score: 0.0,
                    decay_score: None,
                    temporal_boost: None,
                    rrf_pre_boost: None,
                    verbose_decay_factor: None,
                    verbose_pr_boost: None,
                    verbose_src_boost: None,
                    verbose_stat_boost: None,
                    verbose_contradiction: None,
                    is_archived: false,
                    is_consolidated: false,
                });
                added += 1;
            }
        }
        if added >= strategy.hop1_limit {
            break;
        }
    }
}

/// Hydrate candidate rows by ID so the scorer can finish assembling results.
async fn hydrate_candidates(
    db: &Database,
    ids: Arc<[i64]>,
    user_id: i64,
) -> Result<Vec<HydratedCandidateRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    // Scope to the owner (bound after the id list) so single-DB mode never
    // hydrates another user's candidate; a no-op in a single-owner shard.
    let sql = format!(
        "SELECT id, created_at, importance, is_static, source_count, \
         version, is_latest, source, model, access_count, pagerank_score, \
         fsrs_stability, content, category, is_archived, is_consolidated \
         FROM memories WHERE id IN ({}) AND user_id = ?",
        placeholders
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(ids.len() + 1);
        for id in ids.iter() {
            params.push(id);
        }
        params.push(&user_id);

        let mut rows = stmt.query(params.as_slice())?;
        // 6.9 capacity hint: upper bound is the input id set.
        let mut hydrated = Vec::with_capacity(ids.len());
        while let Some(row) = rows.next()? {
            hydrated.push(HydratedCandidateRow {
                id: row.get(0)?,
                created_at: row.get(1)?,
                importance: row.get(2)?,
                is_static: row.get::<_, i32>(3)? != 0,
                source_count: row.get(4)?,
                version: row.get(5)?,
                is_latest: row.get::<_, Option<i32>>(6)?.map(|value| value != 0),
                source: row.get(7)?,
                model: row.get(8)?,
                access_count: row.get(9)?,
                pagerank_score: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                fsrs_stability: row.get(11)?,
                content: row.get(12)?,
                category: row.get(13)?,
                is_archived: row.get::<_, i32>(14)? != 0,
                is_consolidated: row.get::<_, i32>(15)? != 0,
            });
        }
        Ok(hydrated)
    })
    .await
}

/// Fetch graph neighbors for a seed memory without crossing user boundaries.
async fn fetch_graph_neighbors(
    db: &Database,
    seed_id: i64,
    user_id: i64,
) -> Result<Vec<GraphExpansionRow>> {
    // Scope the joined memory to the owner (?2) so graph expansion never crosses
    // into another user's memories in single-DB mode; a no-op in a shard.
    let link_sql = "SELECT ml.target_id, ml.similarity, ml.type, \
        m.content, m.category, m.importance, m.created_at, \
        m.is_latest, m.is_forgotten, m.version, m.source_count, m.model, m.source \
        FROM memory_links ml JOIN memories m ON m.id = ml.target_id \
        WHERE ml.source_id = ?1 AND m.user_id = ?2 \
        UNION \
        SELECT ml.source_id, ml.similarity, ml.type, \
        m.content, m.category, m.importance, m.created_at, \
        m.is_latest, m.is_forgotten, m.version, m.source_count, m.model, m.source \
        FROM memory_links ml JOIN memories m ON m.id = ml.source_id \
        WHERE ml.target_id = ?1 AND m.user_id = ?2";

    db.read(move |conn| {
        let mut stmt = conn.prepare(link_sql)?;
        let mut rows = stmt.query(rusqlite::params![seed_id, user_id])?;
        // 6.9 capacity hint: typical graph-neighbor fanout.
        let mut linked = Vec::with_capacity(16);
        while let Some(row) = rows.next()? {
            linked.push(GraphExpansionRow {
                link_id: row.get(0)?,
                similarity: row.get(1)?,
                link_type: row.get(2)?,
                content: row.get(3)?,
                category: row.get(4)?,
                importance: row.get(5)?,
                created_at: row.get(6)?,
                is_latest: row.get::<_, i32>(7)? != 0,
                is_forgotten: row.get::<_, i32>(8)? != 0,
                version: row.get(9)?,
                source_count: row.get(10)?,
                model: row.get(11)?,
                source: row.get(12)?,
            });
        }
        Ok(linked)
    })
    .await
}

/// Batch-fetch multiple memories by ID in a single query. Returns a HashMap
/// keyed by memory ID for O(1) lookup during result assembly.
async fn fetch_memories_batch(
    db: &Database,
    ids: Arc<[i64]>,
    user_id: i64,
) -> Result<HashMap<i64, crate::memory::types::Memory>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    // Scope to the owner (bound after the id list); a no-op in a single-owner shard.
    let fetch_sql = format!(
        "SELECT {} FROM memories \
         WHERE id IN ({}) AND user_id = ? AND is_forgotten = 0 AND is_latest = 1",
        MEMORY_COLUMNS, placeholders
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&fetch_sql)?;

        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(ids.len() + 1);
        for id in ids.iter() {
            params.push(id);
        }
        params.push(&user_id);

        let mut rows = stmt.query(params.as_slice())?;

        let mut map = HashMap::new();
        while let Some(row) = rows.next()? {
            let mem = row_to_memory(row, user_id)?;
            map.insert(mem.id, mem);
        }
        Ok(map)
    })
    .await
}

/// Batch-fetch links for multiple memory IDs in a single query. Returns a
/// HashMap keyed by the source memory ID.
async fn fetch_links_batch(
    db: &Database,
    memory_ids: Arc<[i64]>,
    user_id: i64,
) -> Result<HashMap<i64, Vec<LinkedMemory>>> {
    if memory_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = memory_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // For each memory_id we need both directions. We tag each row with the
    // "owner" memory ID so we can group results into the right bucket. The
    // joined memory is scoped to the owner (one extra `?` per half) so single-DB
    // mode never returns a link into another user's memory; a no-op in a shard.
    let link_sql = format!(
        "SELECT ml.source_id AS owner, ml.target_id, ml.similarity, ml.type, \
             m.content, m.category, m.is_forgotten \
         FROM memory_links ml JOIN memories m ON m.id = ml.target_id \
         WHERE ml.source_id IN ({placeholders}) AND m.user_id = ? \
         UNION ALL \
         SELECT ml.target_id AS owner, ml.source_id, ml.similarity, ml.type, \
             m.content, m.category, m.is_forgotten \
         FROM memory_links ml JOIN memories m ON m.id = ml.source_id \
         WHERE ml.target_id IN ({placeholders}) AND m.user_id = ?"
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&link_sql)?;

        let mut params: Vec<&dyn rusqlite::types::ToSql> =
            Vec::with_capacity(memory_ids.len() * 2 + 2);
        for id in memory_ids.iter() {
            params.push(id);
        }
        params.push(&user_id);
        for id in memory_ids.iter() {
            params.push(id);
        }
        params.push(&user_id);

        let mut rows = stmt.query(params.as_slice())?;

        let mut map: HashMap<i64, Vec<LinkedMemory>> = HashMap::new();
        while let Some(row) = rows.next()? {
            // Skip forgotten memories
            if row.get::<_, i32>(6)? != 0 {
                continue;
            }
            let owner: i64 = row.get(0)?;
            let link = LinkedMemory {
                id: row.get(1)?,
                similarity: ((row.get::<_, f64>(2)? * 1000.0).round()) / 1000.0,
                link_type: row.get(3)?,
                content: row.get(4)?,
                category: row.get(5)?,
            };
            map.entry(owner).or_default().push(link);
        }
        Ok(map)
    })
    .await
}

/// Batch-fetch version chains for multiple root IDs in a single query.
/// Returns a HashMap keyed by root_memory_id.
async fn fetch_version_chains_batch(
    db: &Database,
    root_ids: Arc<[i64]>,
    user_id: i64,
) -> Result<HashMap<i64, Vec<VersionChainEntry>>> {
    if root_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = root_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    // Scope the whole chain to the owner (final `?`) so version chains never
    // surface another user's rows in single-DB mode; a no-op in a shard.
    let chain_sql = format!(
        "SELECT COALESCE(root_memory_id, id) AS root, id, content, version, is_latest \
         FROM memories \
         WHERE (root_memory_id IN ({placeholders}) OR id IN ({placeholders})) AND user_id = ? \
         ORDER BY root, version ASC"
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&chain_sql)?;

        let mut params: Vec<&dyn rusqlite::types::ToSql> =
            Vec::with_capacity(root_ids.len() * 2 + 1);
        for id in root_ids.iter() {
            params.push(id);
        }
        for id in root_ids.iter() {
            params.push(id);
        }
        params.push(&user_id);

        let mut rows = stmt.query(params.as_slice())?;

        let mut map: HashMap<i64, Vec<VersionChainEntry>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let root: i64 = row.get(0)?;
            let entry = VersionChainEntry {
                id: row.get(1)?,
                content: row.get(2)?,
                version: row.get(3)?,
                is_latest: row.get::<_, i32>(4)? != 0,
            };
            map.entry(root).or_default().push(entry);
        }
        Ok(map)
    })
    .await
}

/// Resolve question type and search strategy from request.
fn resolve_strategy(req: &SearchRequest) -> (QuestionType, crate::memory::types::SearchStrategy) {
    if let Some(qt) = req.question_type {
        return (qt, question_strategy(qt));
    }
    // Use mixed classification and blending
    let weights = classify_question_mixed(&req.query);
    let dominant = *weights
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap()
        .0;
    let strategy = blend_strategies(&weights);
    (dominant, strategy)
}

/// Try the LanceDB centroid index first, fall back to the SQLite-vec
/// `vector_search`. Centroid hits come back from the trait-object index
/// in the `vector::VectorHit` shape and need re-mapping into the
/// `memory::types::VectorHit` used by the rest of search.rs.
async fn centroid_or_sqlite_vector(
    db: &Database,
    embedding: &[f32],
    candidate_target: usize,
    user_id: i64,
) -> Result<Vec<super::types::VectorHit>> {
    if let Some(index) = db.vector_index.as_ref() {
        match index.search(embedding, candidate_target).await {
            Ok(hits) => Ok(hits
                .into_iter()
                .map(|hit| super::types::VectorHit {
                    memory_id: hit.memory_id,
                    distance: hit.distance,
                    rank: hit.rank,
                    matching_chunk_text: None,
                })
                .collect()),
            Err(e) => {
                warn!(
                    "LanceDB vector search failed, falling back to SQLite vectors: {}",
                    e
                );
                vector_search(db, embedding, candidate_target, user_id).await
            }
        }
    } else {
        vector_search(db, embedding, candidate_target, user_id).await
    }
}

/// Run the full hybrid search pipeline matching TS hybridSearch.
#[tracing::instrument(
    name = "hybrid_search",
    skip_all,
    fields(
        user_id = ?req.user_id,
        query_len = req.query.len(),
        limit = ?req.limit,
    )
)]
/// Run the full hybrid memory search pipeline.
pub async fn hybrid_search(db: &Database, req: SearchRequest) -> Result<Arc<Vec<SearchResult>>> {
    // SECURITY (SEC-MED-6): clamp at library entry point so MCP, sidecar,
    // and CLI callers inherit the cap. HTTP route-level clamp is kept as
    // defense-in-depth.
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;

    // 3.5: Check cache before running the full pipeline.
    let param_hash = hash_search_params(&req);
    if let Some(cached) = cache_get(user_id, param_hash) {
        return Ok(cached);
    }

    let (question_type, strategy) = resolve_strategy(&req);

    let candidate_target = limit
        .max((limit * strategy.candidate_multiplier).max(RERANKER_TOP_K))
        .min(200);
    let fts_limit = limit.max((limit * strategy.fts_limit_multiplier).min(250));
    let budget = req.budget.unwrap_or(SearchBudget::High);

    // Ranked lists for RRF fusion
    let mut vector_ranked: Vec<(i64, f64)> = Vec::new();
    let mut fts_ranked: Vec<(i64, f64)> = Vec::new();
    let mut chunk_text_map: HashMap<i64, String> = HashMap::new();
    let mut results: HashMap<i64, Candidate> = HashMap::new();

    // Channel 1: Vector ANN search
    if let Some(ref embedding) = req.embedding {
        let prefer_chunks = db.use_chunk_vector_search && db.chunk_vector_index.is_some();
        let vector_hits = if prefer_chunks {
            match chunk_vector_search(db, embedding, candidate_target, user_id).await {
                Ok(hits) if !hits.is_empty() => Ok(hits),
                Ok(_) => {
                    // Empty chunk result. Fall back to centroid index so
                    // partially-backfilled deployments still surface hits.
                    warn!("chunk vector search returned no hits, falling back to centroid");
                    centroid_or_sqlite_vector(db, embedding, candidate_target, user_id).await
                }
                Err(e) => {
                    warn!(
                        "LanceDB chunk vector search failed, falling back to centroid: {}",
                        e
                    );
                    centroid_or_sqlite_vector(db, embedding, candidate_target, user_id).await
                }
            }
        } else {
            centroid_or_sqlite_vector(db, embedding, candidate_target, user_id).await
        };

        match vector_hits {
            Ok(hits) => {
                for hit in &hits {
                    if let Some(ref text) = hit.matching_chunk_text {
                        chunk_text_map.insert(hit.memory_id, text.clone());
                    }
                    vector_ranked.push((hit.memory_id, hit.rank as f64));
                    let semantic = hit.distance.map(|d| semantic_score_from_distance(d as f64));
                    let entry = results.entry(hit.memory_id).or_insert_with(|| Candidate {
                        id: hit.memory_id,
                        content: String::new(),
                        category: String::new(),
                        source: None,
                        model: None,
                        importance: 5,
                        created_at: String::new(),
                        version: None,
                        is_latest: None,
                        is_static: false,
                        source_count: 1,
                        root_memory_id: None,
                        access_count: 0,
                        pagerank_score: 0.0,
                        fsrs_stability: None,
                        semantic_score: semantic,
                        personality_signal_score: None,
                        score: 0.0,
                        combined_score: 0.0,
                        decay_score: None,
                        temporal_boost: None,
                        rrf_pre_boost: None,
                        verbose_decay_factor: None,
                        verbose_pr_boost: None,
                        verbose_src_boost: None,
                        verbose_stat_boost: None,
                        verbose_contradiction: None,
                        is_archived: false,
                        is_consolidated: false,
                    });
                    // If the candidate already existed (e.g. from FTS), prefer
                    // the most recent semantic_score we have. LanceDB hits only
                    // arrive here, so this is the only place semantic_score
                    // gets populated.
                    if entry.semantic_score.is_none() {
                        entry.semantic_score = semantic;
                    }
                }
            }
            Err(e) => warn!("vector search failed: {}", e),
        }
    }

    // Channel 2: FTS5 search (skipped when the caller wants vector-only recall).
    if !req.query.is_empty() && budget >= SearchBudget::Mid {
        if let Ok(hits) = fts_search(db, &req.query, fts_limit.max(candidate_target), user_id).await
        {
            for hit in &hits {
                fts_ranked.push((hit.memory_id, hit.bm25_score));
                let entry = results.entry(hit.memory_id).or_insert_with(|| Candidate {
                    id: hit.memory_id,
                    content: String::new(),
                    category: String::new(),
                    source: None,
                    model: None,
                    importance: 5,
                    created_at: String::new(),
                    version: None,
                    is_latest: None,
                    is_static: false,
                    source_count: 1,
                    root_memory_id: None,
                    access_count: 0,
                    pagerank_score: 0.0,
                    fsrs_stability: None,
                    semantic_score: None,
                    personality_signal_score: None,
                    score: 0.0,
                    combined_score: 0.0,
                    decay_score: None,
                    temporal_boost: None,
                    rrf_pre_boost: None,
                    verbose_decay_factor: None,
                    verbose_pr_boost: None,
                    verbose_src_boost: None,
                    verbose_stat_boost: None,
                    verbose_contradiction: None,
                    is_archived: false,
                    is_consolidated: false,
                });
                // FTS provides content we can use
                let _ = entry;
            }
        }
    }

    if results.is_empty() {
        return Ok(Arc::new(Vec::new()));
    }

    // RRF fusion across channels, weighted by strategy.
    // Confidence gate: if vector search returned few results relative to what
    // we asked for, semantic confidence is low -- amplify FTS weight by 1.5x.
    let mut rrf_scores: HashMap<i64, f64> = HashMap::new();
    let vector_set: HashSet<i64> = vector_ranked.iter().map(|(id, _)| *id).collect();
    let fts_set: HashSet<i64> = fts_ranked.iter().map(|(id, _)| *id).collect();
    let fts_score_map: HashMap<i64, f64> = fts_ranked.iter().cloned().collect();

    let semantic_confident =
        vector_ranked.len() >= (candidate_target / 3).max(3) || vector_ranked.len() >= 10;
    let effective_fts_weight = if semantic_confident {
        strategy.fts_weight
    } else {
        (strategy.fts_weight * 1.5).min(1.0)
    };

    for (rank, (id, _)) in vector_ranked.iter().enumerate() {
        *rrf_scores.entry(*id).or_default() += rrf_score(rank) * strategy.vector_weight;
    }
    for (rank, (id, _)) in fts_ranked.iter().enumerate() {
        *rrf_scores.entry(*id).or_default() += rrf_score(rank) * effective_fts_weight;
    }

    // Temporal boost date extraction
    let query_date = if question_type == QuestionType::Temporal {
        scoring::extract_query_date(&req.query)
    } else {
        None
    };

    // Hydrate missing fields from DB (created_at, importance, etc.)
    // Warm the pagerank cache for this user if it is empty (first-time or cold start).
    let _ = crate::graph::pagerank::ensure_pagerank_for_user(db, user_id).await;
    {
        let ids: Arc<[i64]> = results.keys().copied().collect::<Vec<i64>>().into();
        if !ids.is_empty() {
            if let Ok(rows) = hydrate_candidates(db, Arc::clone(&ids), user_id).await {
                for row in rows {
                    if let Some(c) = results.get_mut(&row.id) {
                        c.created_at = row.created_at;
                        c.importance = row.importance;
                        c.is_static = row.is_static;
                        c.source_count = row.source_count.max(c.source_count);
                        if c.version.is_none() {
                            c.version = row.version;
                        }
                        if c.is_latest.is_none() {
                            c.is_latest = row.is_latest;
                        }
                        if c.source.is_none() {
                            c.source = row.source;
                        }
                        if c.model.is_none() {
                            c.model = row.model;
                        }
                        c.access_count = row.access_count;
                        c.pagerank_score = row.pagerank_score;
                        c.fsrs_stability = row.fsrs_stability;
                        if c.content.is_empty() {
                            c.content = row.content;
                        }
                        if c.category.is_empty() {
                            c.category = row.category;
                        }
                        c.is_archived = row.is_archived;
                        c.is_consolidated = row.is_consolidated;
                    }
                }
            }
        }
    }

    // Exclude noise categories and archived rows unless explicitly requested
    let include_noise = req.include_noise.unwrap_or(false);
    let include_archived = req.include_archived.unwrap_or(false);
    let exclude_consolidated = req.exclude_consolidated.unwrap_or(false);
    results.retain(|_id, c| {
        if !include_noise && (c.category == "activity" || c.category == "growth") {
            return false;
        }
        if !include_archived && c.is_archived {
            return false;
        }
        if exclude_consolidated && c.is_consolidated {
            return false;
        }
        true
    });
    if results.is_empty() {
        return Ok(Arc::new(Vec::new()));
    }

    // Apply RRF + decay + boosts to each candidate
    for c in results.values_mut() {
        let rrf = rrf_scores.get(&c.id).copied().unwrap_or(0.0);

        // Live FSRS retrievability, decayed by age for EVERY memory including
        // is_static ones. is_static previously forced retrievability = 1.0, but
        // the flag is caller-set (and hardcoded on every consolidation) and had
        // grown to ~43% of the store, pinning nearly half of all memories at full
        // strength so stale "permanent" memories dominated recall forever. The
        // flag still protects durability (no auto-prune) and gate guard lookups
        // (both use is_static via direct SQL); it just no longer freezes ranking.
        // Read per-memory `fsrs_stability` when available; fall back to
        // `initial_stability(Rating::Good)` when the column is NULL.
        let retrievability = {
            let stability = c.fsrs_stability.unwrap_or_else(|| {
                crate::fsrs::initial_stability(crate::fsrs::Rating::Good) as f64
            });
            let ref_str = &c.created_at;
            let elapsed = if !ref_str.is_empty() {
                let normalized = if ref_str.contains('Z') {
                    ref_str.to_string()
                } else {
                    format!("{}Z", ref_str.replace(' ', "T"))
                };
                if let Ok(dt) = normalized.parse::<chrono::DateTime<chrono::Utc>>() {
                    let ms = (chrono::Utc::now().timestamp_millis() - dt.timestamp_millis()).max(0);
                    ms as f64 / 86_400_000.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            crate::fsrs::retrievability(stability as f32, elapsed as f32) as f64
        };

        c.decay_score = Some((c.importance as f64 * retrievability * 1000.0).round() / 1000.0);

        let decay_factor = {
            let floor = scoring::decay_floor();
            floor + (1.0 - floor) * retrievability
        };
        let src_boost = scoring::source_count_boost(c.source_count, c.is_consolidated);
        let stat_boost = scoring::static_boost(c.is_static, c.is_consolidated);

        let temp_boost = if let Some(ref qd) = query_date {
            if !c.created_at.is_empty() {
                let b = scoring::temporal_proximity_boost(qd, &c.created_at);
                if b > 1.0 {
                    c.temporal_boost = Some((b * 1000.0).round() / 1000.0);
                }
                b
            } else {
                1.0
            }
        } else {
            1.0
        };

        let pr_boost = scoring::pagerank_boost(c.pagerank_score);
        let contr = scoring::contradiction_penalty(&c.content, c.is_latest.unwrap_or(true));

        let recency = scoring::recency_score(&c.created_at);
        let recency_boost = 1.0 + recency * scoring::RECENCY_WEIGHT;

        c.score = rrf
            * decay_factor
            * src_boost
            * stat_boost
            * temp_boost
            * pr_boost
            * contr
            * recency_boost;
        c.combined_score = c.score;

        // Personality signal boost: detect emotion/preference signals in the
        // candidate content and apply as a multiplicative boost when the
        // strategy requests personality-weighted recall.
        apply_personality_boost(c, &strategy);

        let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
        c.rrf_pre_boost = Some(r3(rrf));
        c.verbose_decay_factor = Some(r3(decay_factor));
        c.verbose_pr_boost = Some(r3(pr_boost));
        c.verbose_src_boost = Some(r3(src_boost));
        c.verbose_stat_boost = Some(r3(stat_boost));
        c.verbose_contradiction = Some(r3(contr));
    }

    // Relationship expansion (2-hop) -- graph RRF channel
    let mut graph_score_map: HashMap<i64, f64> = HashMap::new();
    if strategy.expand_relationships && budget >= SearchBudget::High {
        let mut top_ids: Vec<(i64, f64)> = results
            .iter()
            .map(|(&id, c)| (id, c.combined_score))
            .collect();
        top_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        top_ids.truncate(strategy.relationship_seed_limit);

        // Fire all neighbor fetches in parallel instead of sequential per-seed.
        let neighbor_futures: Vec<_> = top_ids
            .iter()
            .map(|(seed_id, _)| fetch_graph_neighbors(db, *seed_id, user_id))
            .collect();
        let neighbor_results = futures::future::join_all(neighbor_futures).await;
        let neighbor_rows: Vec<Vec<GraphExpansionRow>> = neighbor_results
            .into_iter()
            .filter_map(Result::ok)
            .collect();

        inject_graph_hop1_neighbors(&mut results, &mut graph_score_map, neighbor_rows, &strategy);

        // Apply graph RRF scores
        let mut graph_ranked: Vec<(i64, f64)> =
            graph_score_map.iter().map(|(&id, &s)| (id, s)).collect();
        graph_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (id, _)) in graph_ranked.iter().enumerate() {
            if let Some(c) = results.get_mut(id) {
                apply_graph_rrf_increment(c, rrf_score(rank));
            }
        }
    }
    let graph_set: HashSet<i64> = graph_score_map.keys().copied().collect();

    // SEC-recall-1.2: enforce strategy.vector_floor as a real filter. Drop
    // candidates whose vector channel score is below the floor AND that did
    // not also surface via FTS or graph (those channels carry their own
    // signal independent of cosine similarity). Hits with no semantic_score
    // (libSQL fallback path that doesn't project distance) are not filtered
    // -- we have no signal to reject them on. The floor itself can be
    // overridden via KLEOS_VECTOR_FLOOR for emergency tuning without a
    // restart-blocking config change.
    let env_floor: Option<f64> = std::env::var("KLEOS_VECTOR_FLOOR")
        .ok()
        .and_then(|v| v.parse().ok());
    let effective_floor = env_floor.unwrap_or(strategy.vector_floor);
    if effective_floor > 0.0 {
        results.retain(|id, c| {
            // Always keep candidates that surfaced from FTS or graph -- they
            // carry signal beyond cosine similarity.
            if fts_set.contains(id) || graph_set.contains(id) {
                return true;
            }
            // Vector-only candidate: enforce the floor when we have a real
            // semantic_score. Hits with None (fallback path) pass through.
            match c.semantic_score {
                Some(s) => s >= effective_floor,
                None => true,
            }
        });
    }

    // Guard NaN, sort, limit, and annotate channels
    for c in results.values_mut() {
        if c.score.is_nan() {
            c.score = 0.0;
        }
        if c.combined_score.is_nan() {
            c.combined_score = c.score;
        }
        if let Some(d) = c.decay_score {
            if d.is_nan() {
                c.decay_score = Some(0.0);
            }
        }
    }

    let mut sorted: Vec<&Candidate> = results.values().collect();
    sorted.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let candidate_count = sorted.len();
    sorted.truncate(limit);

    // Build final SearchResult vec -- batch-fetch all memories in one query
    // instead of N separate round-trips.
    let candidate_ids: Arc<[i64]> = sorted.iter().map(|c| c.id).collect::<Vec<i64>>().into();
    let memory_map = fetch_memories_batch(db, Arc::clone(&candidate_ids), user_id).await?;

    let mut final_results: Vec<SearchResult> = Vec::with_capacity(sorted.len());

    for c in &sorted {
        // Build channel list. Capacity 3 avoids reallocs during push;
        // static &str -> String is one 6-byte heap slot per hit (R8 P-006).
        let mut channels: Vec<String> = Vec::with_capacity(3);
        if vector_set.contains(&c.id) {
            channels.push(String::from("vector"));
        }
        if fts_set.contains(&c.id) {
            channels.push(String::from("fts"));
        }
        if graph_set.contains(&c.id) {
            channels.push(String::from("graph"));
        }

        // Look up from pre-fetched batch
        let memory = match memory_map.get(&c.id) {
            Some(mem) => mem.clone(),
            None => continue,
        };

        let fts_s = fts_score_map
            .get(&c.id)
            .map(|s| (*s * 1000.0).round() / 1000.0);
        let graph_s = graph_score_map
            .get(&c.id)
            .map(|s| (*s * 1000.0).round() / 1000.0);

        final_results.push(SearchResult {
            memory,
            score: c.combined_score,
            search_type: channels.join("+"),
            decay_score: c.decay_score,
            combined_score: Some(c.combined_score),
            semantic_score: c.semantic_score,
            fts_score: fts_s,
            graph_score: graph_s,
            personality_signal_score: c.personality_signal_score,
            temporal_boost: c.temporal_boost,
            channels: Some(channels),
            question_type: Some(question_type),
            reranked: Some(false),
            reranker_ms: Some(0.0),
            candidate_count: Some(candidate_count),
            rrf_pre_boost: c.rrf_pre_boost,
            decay_factor: c.verbose_decay_factor,
            pr_boost: c.verbose_pr_boost,
            src_boost: c.verbose_src_boost,
            stat_boost: c.verbose_stat_boost,
            contradiction: c.verbose_contradiction,
            matching_chunk: chunk_text_map.get(&c.id).cloned(),
            linked: None,
            version_chain: None,
        });
    }

    // 3.11: Post-filters -- apply category, source, tags, space_id, threshold
    // filters that SearchRequest carries but were previously ignored.
    if req.category.is_some()
        || req.source.is_some()
        || req.source_filter.is_some()
        || req.tags.is_some()
        || req.space_id.is_some()
        || req.threshold.is_some()
    {
        let filter_category = req.category.as_deref();
        let filter_source = req.source.as_deref().or(req.source_filter.as_deref());
        let filter_tags: Option<Vec<String>> = req.tags.as_ref().map(|t| {
            t.iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        });
        let filter_space = req.space_id;
        let filter_threshold = req.threshold;

        final_results.retain(|r| {
            let m = &r.memory;
            if let Some(cat) = filter_category {
                if !m.category.eq_ignore_ascii_case(cat) {
                    return false;
                }
            }
            if let Some(src) = filter_source {
                if !m.source.eq_ignore_ascii_case(src) {
                    return false;
                }
            }
            if let Some(ref wanted) = filter_tags {
                let mem_tags: HashSet<String> =
                    super::parse_tags_json(&m.tags).into_iter().collect();
                if !wanted.iter().all(|t| mem_tags.contains(t)) {
                    return false;
                }
            }
            if let Some(sid) = filter_space {
                if m.space_id != Some(sid) {
                    return false;
                }
            }
            if let Some(thr) = filter_threshold {
                if r.score < thr as f64 {
                    return false;
                }
            }
            true
        });
    }

    // Include linked memories + version chain if requested -- batch queries
    if req.include_links {
        let result_ids: Arc<[i64]> = final_results
            .iter()
            .map(|r| r.memory.id)
            .collect::<Vec<i64>>()
            .into();
        let root_ids: Arc<[i64]> = final_results
            .iter()
            .map(|r| r.memory.root_memory_id.unwrap_or(r.memory.id))
            .collect::<Vec<i64>>()
            .into();

        let links_map = fetch_links_batch(db, Arc::clone(&result_ids), user_id).await?;
        let chains_map = fetch_version_chains_batch(db, Arc::clone(&root_ids), user_id).await?;

        for result in &mut final_results {
            if let Some(links) = links_map.get(&result.memory.id) {
                if !links.is_empty() {
                    result.linked = Some(links.clone());
                }
            }

            let root_id = result.memory.root_memory_id.unwrap_or(result.memory.id);
            if let Some(chain) = chains_map.get(&root_id) {
                if chain.len() > 1 {
                    result.version_chain = Some(chain.clone());
                }
            }
        }
    }

    info!(
        query_len = req.query.len(),
        results = final_results.len(),
        candidates = candidate_count,
        question_type = %question_type,
        "hybrid search completed"
    );

    let arc_results = Arc::new(final_results);
    cache_put(user_id, param_hash, Arc::clone(&arc_results));

    Ok(arc_results)
}

/// SEC-recall-1.5: run `hybrid_search` then apply the supplied reranker.
/// Wrapping (rather than threading reranker into `hybrid_search` itself)
/// keeps the existing 10+ call sites untouched while letting any in-process
/// caller opt into reranking by passing `Some(reranker)`. The original query
/// string is required because the cross-encoder scores query-document pairs.
///
/// Behaviour matches the route-level path: clone the cached `Arc<Vec<...>>`
/// once so the caller can mutate, run `rerank_results` in place (which
/// re-sorts internally), and return a fresh `Arc`. Reranker errors degrade
/// gracefully -- a tracing warning, then the unranked results.
pub async fn hybrid_search_reranked(
    db: &Database,
    req: SearchRequest,
    query_for_rerank: &str,
    reranker: Option<std::sync::Arc<dyn crate::reranker::Reranker>>,
) -> Result<Arc<Vec<SearchResult>>> {
    let arc_results = hybrid_search(db, req).await?;
    let Some(reranker) = reranker else {
        return Ok(arc_results);
    };
    let mut results = (*arc_results).clone();
    if let Err(e) = reranker
        .rerank_results(query_for_rerank, &mut results)
        .await
    {
        tracing::warn!("reranker failed in hybrid_search_reranked: {}", e);
        return Ok(arc_results);
    }
    Ok(Arc::new(results))
}

// ---------------------------------------------------------------------------
// 3.11: Faceted / multi-tag search
// ---------------------------------------------------------------------------

/// Faceted search: runs hybrid search (if query present) OR direct DB scan,
/// applies structured tag/category/source/importance/date filters, then
/// computes requested facet aggregations over the matched set.
#[tracing::instrument(
    skip(db, req),
    fields(
        user_id = req.user_id.unwrap_or(0),
        query_len = req.query.len(),
        limit = req.limit,
    )
)]
/// Run faceted search over the filtered result set.
pub async fn faceted_search(
    db: &Database,
    mut req: FacetedSearchRequest,
) -> Result<FacetedSearchResponse> {
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let limit = req.limit.min(MAX_LIMIT);
    let facet_limit = req.facet_limit.unwrap_or(20).min(100);
    let requested_facets: HashSet<String> = req
        .facets
        .as_ref()
        .map(|v| v.iter().map(|s| s.to_lowercase()).collect())
        .unwrap_or_default();

    // Normalize tag filter sets once.
    let tags_all: Vec<String> = req
        .tags_all
        .as_ref()
        .map(|t| {
            t.iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let tags_any: HashSet<String> = req
        .tags_any
        .as_ref()
        .map(|t| {
            t.iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let tags_none: HashSet<String> = req
        .tags_none
        .as_ref()
        .map(|t| {
            t.iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Date bounds parsed once.
    let date_from = req.date_from.as_deref().and_then(parse_iso_date);
    let date_to = req.date_to.as_deref().and_then(parse_iso_date);

    // R8 P-010: move the embedding out of req up front so the inner
    // SearchRequest does not have to clone 4 KB of floats. The predicate
    // closure below does not read embedding, so zeroing it here is safe.
    let taken_embedding = req.embedding.take();

    let passes_filters = |m: &super::types::Memory| -> bool {
        // Category
        if let Some(ref cat) = req.category {
            if !m.category.eq_ignore_ascii_case(cat) {
                return false;
            }
        }
        // Source
        if let Some(ref src) = req.source {
            if !m.source.eq_ignore_ascii_case(src) {
                return false;
            }
        }
        // Space
        if let Some(sid) = req.space_id {
            if m.space_id != Some(sid) {
                return false;
            }
        }
        // Importance range
        if let Some(imin) = req.importance_min {
            if m.importance < imin {
                return false;
            }
        }
        if let Some(imax) = req.importance_max {
            if m.importance > imax {
                return false;
            }
        }
        // Date range
        if let Some(ref dt) = date_from {
            if m.created_at < *dt {
                return false;
            }
        }
        if let Some(ref dt) = date_to {
            if m.created_at > *dt {
                return false;
            }
        }
        // Tags
        let mem_tags: HashSet<String> = super::parse_tags_json(&m.tags).into_iter().collect();
        if !tags_all.is_empty() && !tags_all.iter().all(|t| mem_tags.contains(t)) {
            return false;
        }
        if !tags_any.is_empty() && !tags_any.iter().any(|t| mem_tags.contains(t)) {
            return false;
        }
        if !tags_none.is_empty() && tags_none.iter().any(|t| mem_tags.contains(t)) {
            return false;
        }
        true
    };

    let results: Vec<SearchResult>;

    if !req.query.is_empty() {
        // Semantic mode: delegate to hybrid_search with generous limit,
        // then post-filter. We request more candidates so filtering
        // doesn't starve the result set.
        let over_fetch = (limit * 5).min(MAX_LIMIT);
        let search_req = SearchRequest {
            query: req.query.clone(),
            embedding: taken_embedding,
            limit: Some(over_fetch),
            category: req.category.clone(),
            source: req.source.clone(),
            tags: req.tags_all.clone(),
            user_id: Some(user_id),
            space_id: req.space_id,
            include_forgotten: Some(false),
            ..Default::default()
        };
        let arc = hybrid_search(db, search_req).await?;
        let mut candidates = (*arc).clone();
        candidates.retain(|r| passes_filters(&r.memory));
        candidates.truncate(limit);
        results = candidates;
    } else {
        // Filter-only mode: direct DB query (no semantic ranking).
        let matched = faceted_db_scan(db, user_id, &req, limit).await?;
        results = matched
            .into_iter()
            .filter(|r| passes_filters(&r.memory))
            .take(limit)
            .collect();
    }

    // Compute facets over the matched set.
    let total_matched = results.len();

    let facets_tags = if requested_facets.contains("tags") {
        Some(compute_tag_facets(&results, facet_limit))
    } else {
        None
    };

    let facets_categories = if requested_facets.contains("categories") {
        Some(compute_string_facets(
            results.iter().map(|r| r.memory.category.as_str()),
            facet_limit,
        ))
    } else {
        None
    };

    let facets_sources = if requested_facets.contains("sources") {
        Some(compute_string_facets(
            results.iter().map(|r| r.memory.source.as_str()),
            facet_limit,
        ))
    } else {
        None
    };

    let facets_importance = if requested_facets.contains("importance") {
        Some(compute_string_facets(
            results.iter().map(|r| leak_i32_str(r.memory.importance)),
            facet_limit,
        ))
    } else {
        None
    };

    let tag_cooccurrence = if requested_facets.contains("tags") {
        Some(compute_tag_cooccurrence(&results, facet_limit))
    } else {
        None
    };

    Ok(FacetedSearchResponse {
        results,
        total_matched,
        facets_tags,
        facets_categories,
        facets_sources,
        facets_importance,
        tag_cooccurrence,
    })
}

/// Direct DB scan for filter-only faceted search (no semantic query).
async fn faceted_db_scan(
    db: &Database,
    user_id: i64,
    req: &FacetedSearchRequest,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Build SQL with applicable WHERE clauses pushed to DB level.
    let mut conditions = vec!["is_forgotten = 0".to_string(), "is_latest = 1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql + Send>> = vec![];
    let mut idx = 1usize;

    // Always scope to the owner so single-DB (shared) mode is isolated; a no-op
    // in a single-owner shard.
    conditions.push(format!("user_id = ?{}", idx));
    params_vec.push(Box::new(user_id));
    idx += 1;

    if let Some(ref cat) = req.category {
        conditions.push(format!("category = ?{}", idx));
        params_vec.push(Box::new(cat.clone()));
        idx += 1;
    }
    if let Some(ref src) = req.source {
        conditions.push(format!("source = ?{}", idx));
        params_vec.push(Box::new(src.clone()));
        idx += 1;
    }
    if let Some(sid) = req.space_id {
        conditions.push(format!("space_id = ?{}", idx));
        params_vec.push(Box::new(sid));
        idx += 1;
    }
    if let Some(imin) = req.importance_min {
        conditions.push(format!("importance >= ?{}", idx));
        params_vec.push(Box::new(imin));
        idx += 1;
    }
    if let Some(imax) = req.importance_max {
        conditions.push(format!("importance <= ?{}", idx));
        params_vec.push(Box::new(imax));
        idx += 1;
    }
    if let Some(ref dt) = req.date_from {
        conditions.push(format!("created_at >= ?{}", idx));
        params_vec.push(Box::new(dt.clone()));
        idx += 1;
    }
    if let Some(ref dt) = req.date_to {
        conditions.push(format!("created_at <= ?{}", idx));
        params_vec.push(Box::new(dt.clone()));
        let _ = idx;
    }

    let where_clause = conditions.join(" AND ");
    let sql = format!(
        "SELECT {} FROM memories WHERE {} ORDER BY created_at DESC LIMIT {}",
        MEMORY_COLUMNS,
        where_clause,
        limit * 3 // over-fetch for tag filtering in Rust
    );

    db.read(move |conn| {
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(param_refs.as_slice())?;
        // 6.9 capacity hint: SQL over-fetches limit*3 for tag filtering.
        let mut memories = Vec::with_capacity(limit.saturating_mul(3));
        while let Some(row) = rows.next()? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories
            .into_iter()
            .map(|m| SearchResult {
                score: m.importance as f64 / 10.0,
                memory: m,
                search_type: "filter".to_string(),
                decay_score: None,
                combined_score: None,
                semantic_score: None,
                fts_score: None,
                graph_score: None,
                personality_signal_score: None,
                temporal_boost: None,
                channels: Some(vec!["filter".to_string()]),
                question_type: None,
                reranked: None,
                reranker_ms: None,
                candidate_count: None,
                rrf_pre_boost: None,
                decay_factor: None,
                pr_boost: None,
                src_boost: None,
                stat_boost: None,
                contradiction: None,
                matching_chunk: None,
                linked: None,
                version_chain: None,
            })
            .collect())
    })
    .await
}

// ---------------------------------------------------------------------------
// Facet computation helpers
// ---------------------------------------------------------------------------

fn compute_tag_facets(results: &[SearchResult], limit: usize) -> Vec<FacetBucket> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in results {
        for tag in super::parse_tags_json(&r.memory.tags) {
            *counts.entry(tag).or_insert(0) += 1;
        }
    }
    let mut buckets: Vec<FacetBucket> = counts
        .into_iter()
        .map(|(value, count)| FacetBucket { value, count })
        .collect();
    buckets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    buckets.truncate(limit);
    buckets
}

/// Aggregate string facet counts for a filtered result set.
fn compute_string_facets<'a>(
    values: impl Iterator<Item = &'a str>,
    limit: usize,
) -> Vec<FacetBucket> {
    // Borrow-keyed first pass: only unique values pay the String alloc
    // when we build the output buckets (R8 P-007).
    let mut counts: HashMap<&'a str, usize> = HashMap::new();
    for v in values {
        if !v.is_empty() {
            *counts.entry(v).or_insert(0) += 1;
        }
    }
    let mut buckets: Vec<FacetBucket> = counts
        .into_iter()
        .map(|(value, count)| FacetBucket {
            value: value.to_string(),
            count,
        })
        .collect();
    buckets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    buckets.truncate(limit);
    buckets
}

/// Cheap way to get &str from i32 for facet counting without allocation per call.
fn leak_i32_str(v: i32) -> &'static str {
    // For importance 1-10, we use a static array.
    static NUMS: [&str; 11] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
    if (0..=10).contains(&v) {
        NUMS[v as usize]
    } else {
        // Leak is fine for edge cases -- importance is clamped 1-10.
        Box::leak(v.to_string().into_boxed_str())
    }
}

/// Count co-occurring tag pairs for the supplied search results.
fn compute_tag_cooccurrence(results: &[SearchResult], limit: usize) -> Vec<TagCooccurrence> {
    let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();
    for r in results {
        let mut tags: Vec<String> = super::parse_tags_json(&r.memory.tags);
        tags.sort();
        tags.dedup();
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                let key = (tags[i].clone(), tags[j].clone());
                *pair_counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    let mut pairs: Vec<TagCooccurrence> = pair_counts
        .into_iter()
        .map(|((tag_a, tag_b), count)| TagCooccurrence {
            tag_a,
            tag_b,
            count,
        })
        .collect();
    pairs.sort_by_key(|b| std::cmp::Reverse(b.count));
    pairs.truncate(limit);
    pairs
}

/// Parse an ISO-8601 date string into a comparable string (normalize format).
fn parse_iso_date(s: &str) -> Option<String> {
    // MEM-3: actually validate the input is a date before using it as a SQL
    // string bound against created_at. The old `len() >= 10` check let any
    // 10+ char string (e.g. "AAAAAAAAAA") through, silently widening or
    // suppressing the date filter. Accept a bare date ("2024-01-15"), an
    // RFC3339 timestamp ("2024-01-15T00:00:00Z"), or the DB's own
    // "YYYY-MM-DD HH:MM:SS" form, and return the comparable string only when one
    // of those parses succeeds.
    let trimmed = s.trim();
    let is_valid = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_ok()
        || chrono::DateTime::parse_from_rfc3339(trimmed).is_ok()
        || chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S").is_ok();
    if is_valid {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Regression tests for cache hashing and ranking edge cases.
#[cfg(test)]
mod tests {
    use super::{
        apply_graph_rrf_increment, apply_personality_boost, hash_search_params,
        inject_graph_hop1_neighbors, parse_iso_date, semantic_score_from_distance, Candidate,
        GraphExpansionRow,
    };
    use crate::memory::types::{SearchBudget, SearchRequest, SearchStrategy};

    /// MEM-3: only real dates are accepted as date-range bounds; arbitrary
    /// long strings must be rejected so they cannot silently alter SQL filters.
    #[test]
    fn parse_iso_date_validates_real_dates() {
        // Valid forms round-trip to the trimmed comparable string.
        assert_eq!(parse_iso_date("2024-01-15").as_deref(), Some("2024-01-15"));
        assert_eq!(
            parse_iso_date("  2024-01-15  ").as_deref(),
            Some("2024-01-15")
        );
        assert_eq!(
            parse_iso_date("2024-01-15T00:00:00Z").as_deref(),
            Some("2024-01-15T00:00:00Z")
        );
        assert_eq!(
            parse_iso_date("2024-01-15 13:45:00").as_deref(),
            Some("2024-01-15 13:45:00")
        );

        // Garbage that the old len()>=10 check let through is now rejected.
        assert_eq!(parse_iso_date("AAAAAAAAAA"), None);
        assert_eq!(parse_iso_date("not-a-date!"), None);
        assert_eq!(parse_iso_date("2024-13-99"), None);
        assert_eq!(parse_iso_date("2024-01"), None);
        assert_eq!(parse_iso_date(""), None);
    }

    /// Keeps cache entries distinct when callers trim the search pipeline differently.
    #[test]
    fn different_budgets_produce_different_hashes() {
        let base = SearchRequest {
            query: "test query".into(),
            ..Default::default()
        };

        let mut with_low = base.clone();
        with_low.budget = Some(SearchBudget::Low);

        let mut with_mid = base.clone();
        with_mid.budget = Some(SearchBudget::Mid);

        let h_none = hash_search_params(&base);
        let h_low = hash_search_params(&with_low);
        let h_mid = hash_search_params(&with_mid);

        assert_ne!(h_none, h_low, "None vs Low should differ");
        assert_ne!(h_low, h_mid, "Low vs Mid should differ");
    }

    /// Clamps semantic distances above 1.0 to a zero score.
    #[test]
    fn semantic_distance_above_one_clamps_to_zero() {
        assert_eq!(semantic_score_from_distance(1.25), 0.0);
    }

    /// Enforces the hop1 cap across all graph seeds instead of per seed.
    #[test]
    fn hop1_limit_caps_total_neighbors_across_all_seeds() {
        let strategy = SearchStrategy {
            vector_floor: 0.0,
            vector_weight: 1.0,
            fts_weight: 1.0,
            candidate_multiplier: 1,
            fts_limit_multiplier: 1,
            expand_relationships: true,
            relationship_seed_limit: 2,
            hop1_limit: 1,
            hop2_limit: 1,
            relationship_multiplier: 1.0,
            include_personality_signals: false,
            personality_limit: 0,
            personality_weight: 0.0,
        };

        let mut results = std::collections::HashMap::new();
        let mut graph_score_map = std::collections::HashMap::new();
        let neighbor_results = vec![
            vec![GraphExpansionRow {
                link_id: 2,
                similarity: 0.8,
                link_type: "related".into(),
                content: "seed one".into(),
                category: "general".into(),
                importance: 5,
                created_at: "2026-05-31T00:00:00Z".into(),
                is_latest: true,
                is_forgotten: false,
                version: Some(1),
                source_count: 1,
                model: None,
                source: None,
            }],
            vec![GraphExpansionRow {
                link_id: 3,
                similarity: 0.9,
                link_type: "related".into(),
                content: "seed two".into(),
                category: "general".into(),
                importance: 5,
                created_at: "2026-05-31T00:00:00Z".into(),
                is_latest: true,
                is_forgotten: false,
                version: Some(1),
                source_count: 1,
                model: None,
                source: None,
            }],
        ];

        inject_graph_hop1_neighbors(
            &mut results,
            &mut graph_score_map,
            neighbor_results,
            &strategy,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(graph_score_map.len(), 1);
        assert!(results.contains_key(&2));
        assert!(!results.contains_key(&3));
    }

    /// Preserves the personality boost when graph RRF adds its final increment.
    #[test]
    fn personality_boost_survives_graph_rrf_merge() {
        let strategy = SearchStrategy {
            vector_floor: 0.0,
            vector_weight: 1.0,
            fts_weight: 1.0,
            candidate_multiplier: 1,
            fts_limit_multiplier: 1,
            expand_relationships: false,
            relationship_seed_limit: 1,
            hop1_limit: 1,
            hop2_limit: 1,
            relationship_multiplier: 1.0,
            include_personality_signals: true,
            personality_limit: 1,
            personality_weight: 0.5,
        };

        let mut candidate = Candidate {
            id: 7,
            content: "I feel really excited about this project. I love building things.".into(),
            category: "general".into(),
            source: None,
            model: None,
            importance: 5,
            created_at: "2026-05-31T00:00:00Z".into(),
            version: None,
            is_latest: Some(true),
            is_static: false,
            source_count: 1,
            root_memory_id: None,
            access_count: 0,
            pagerank_score: 0.0,
            fsrs_stability: None,
            semantic_score: None,
            personality_signal_score: None,
            score: 1.0,
            combined_score: 1.0,
            decay_score: None,
            temporal_boost: None,
            rrf_pre_boost: None,
            verbose_decay_factor: None,
            verbose_pr_boost: None,
            verbose_src_boost: None,
            verbose_stat_boost: None,
            verbose_contradiction: None,
            is_archived: false,
            is_consolidated: false,
        };

        apply_personality_boost(&mut candidate, &strategy);
        let boosted_score = candidate.score;

        apply_graph_rrf_increment(&mut candidate, 0.25);

        assert!(boosted_score > 1.0);
        assert_eq!(candidate.score, candidate.combined_score);
        assert!(candidate.score > boosted_score);
    }
}
