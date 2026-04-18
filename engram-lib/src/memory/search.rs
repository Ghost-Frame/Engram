use super::fts::fts_search;
use super::vector::vector_search;
use super::{row_to_memory, rusqlite_to_eng_error, MEMORY_COLUMNS};
use crate::db::Database;
use crate::memory::scoring::{
    self, blend_strategies, classify_question_mixed, question_strategy, rrf_score, DECAY_FLOOR,
};
use crate::memory::types::{
    FacetBucket, FacetedSearchRequest, FacetedSearchResponse, LinkedMemory, QuestionType,
    SearchRequest, SearchResult, TagCooccurrence, VersionChainEntry,
};
use crate::validation::{DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT, RERANKER_TOP_K};
use crate::Result;
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{LazyLock, Mutex};
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

const CACHE_CAPACITY: usize = 512;
const CACHE_TTL_SECS: u64 = 15;

struct CacheEntry {
    results: Vec<SearchResult>,
    inserted: Instant,
}

/// Cache key: (user_id, generation, query_param_hash)
type CacheKey = (i64, u64, u64);

struct SearchCache {
    entries: LruCache<CacheKey, CacheEntry>,
    generations: HashMap<i64, u64>,
}

static SEARCH_CACHE: LazyLock<Mutex<SearchCache>> = LazyLock::new(|| {
    Mutex::new(SearchCache {
        entries: LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap()),
        generations: HashMap::new(),
    })
});

/// Hash the search parameters that affect results.
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
    h.finish()
}

fn cache_get(user_id: i64, param_hash: u64) -> Option<Vec<SearchResult>> {
    let mut cache = SEARCH_CACHE.lock().ok()?;
    let gen = *cache.generations.get(&user_id).unwrap_or(&0);
    let key = (user_id, gen, param_hash);
    if let Some(entry) = cache.entries.get(&key) {
        if entry.inserted.elapsed().as_secs() < CACHE_TTL_SECS {
            return Some(entry.results.clone());
        }
        // Expired -- remove
        cache.entries.pop(&key);
    }
    None
}

fn cache_put(user_id: i64, param_hash: u64, results: &[SearchResult]) {
    if let Ok(mut cache) = SEARCH_CACHE.lock() {
        let gen = *cache.generations.get(&user_id).unwrap_or(&0);
        let key = (user_id, gen, param_hash);
        cache.entries.put(
            key,
            CacheEntry {
                results: results.to_vec(),
                inserted: Instant::now(),
            },
        );
    }
}

/// Invalidate all cached search results for a user. Call on any memory write.
pub fn invalidate_search_cache(user_id: i64) {
    if let Ok(mut cache) = SEARCH_CACHE.lock() {
        let gen = cache.generations.entry(user_id).or_insert(0);
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
    root_memory_id: Option<i64>,
    access_count: i32,
    pagerank_score: f64,
    semantic_score: Option<f64>,
    personality_signal_score: Option<f64>,
    score: f64,
    combined_score: f64,
    decay_score: Option<f64>,
    temporal_boost: Option<f64>,
}

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
    content: String,
    category: String,
}

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

async fn hydrate_candidates(
    db: &Database,
    ids: &[i64],
    user_id: i64,
) -> Result<Vec<HydratedCandidateRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let ids_owned: Vec<i64> = ids.to_vec();
    let placeholders = ids_owned.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, created_at, importance, is_static, source_count, \
         version, is_latest, source, model, access_count, pagerank_score, \
         content, category \
         FROM memories WHERE id IN ({}) AND user_id = ?",
        placeholders
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(ids_owned.len() + 1);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: upper bound is the input id set.
        let mut hydrated = Vec::with_capacity(ids_owned.len());
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            hydrated.push(HydratedCandidateRow {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                created_at: row.get(1).map_err(rusqlite_to_eng_error)?,
                importance: row.get(2).map_err(rusqlite_to_eng_error)?,
                is_static: row.get::<_, i32>(3).map_err(rusqlite_to_eng_error)? != 0,
                source_count: row.get(4).map_err(rusqlite_to_eng_error)?,
                version: row.get(5).map_err(rusqlite_to_eng_error)?,
                is_latest: row
                    .get::<_, Option<i32>>(6)
                    .map_err(rusqlite_to_eng_error)?
                    .map(|value| value != 0),
                source: row.get(7).map_err(rusqlite_to_eng_error)?,
                model: row.get(8).map_err(rusqlite_to_eng_error)?,
                access_count: row.get(9).map_err(rusqlite_to_eng_error)?,
                pagerank_score: row
                    .get::<_, Option<f64>>(10)
                    .map_err(rusqlite_to_eng_error)?
                    .unwrap_or(0.0),
                content: row.get(11).map_err(rusqlite_to_eng_error)?,
                category: row.get(12).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(hydrated)
    })
    .await
}

async fn fetch_graph_neighbors(
    db: &Database,
    seed_id: i64,
    user_id: i64,
) -> Result<Vec<GraphExpansionRow>> {
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
        let mut stmt = conn.prepare(link_sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![seed_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: typical graph-neighbor fanout.
        let mut linked = Vec::with_capacity(16);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            linked.push(GraphExpansionRow {
                link_id: row.get(0).map_err(rusqlite_to_eng_error)?,
                similarity: row.get(1).map_err(rusqlite_to_eng_error)?,
                link_type: row.get(2).map_err(rusqlite_to_eng_error)?,
                content: row.get(3).map_err(rusqlite_to_eng_error)?,
                category: row.get(4).map_err(rusqlite_to_eng_error)?,
                importance: row.get(5).map_err(rusqlite_to_eng_error)?,
                created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
                is_latest: row.get::<_, i32>(7).map_err(rusqlite_to_eng_error)? != 0,
                is_forgotten: row.get::<_, i32>(8).map_err(rusqlite_to_eng_error)? != 0,
                version: row.get(9).map_err(rusqlite_to_eng_error)?,
                source_count: row.get(10).map_err(rusqlite_to_eng_error)?,
                model: row.get(11).map_err(rusqlite_to_eng_error)?,
                source: row.get(12).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(linked)
    })
    .await
}

async fn fetch_memory_for_search(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<Option<crate::memory::types::Memory>> {
    let fetch_sql = format!(
        "SELECT {} FROM memories \
         WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0 AND is_latest = 1 \
           AND is_consolidated = 0",
        MEMORY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&fetch_sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(Some(row_to_memory(row)?))
        } else {
            Ok(None)
        }
    })
    .await
}

/// Batch-fetch multiple memories by ID in a single query. Returns a HashMap
/// keyed by memory ID for O(1) lookup during result assembly.
async fn fetch_memories_batch(
    db: &Database,
    ids: &[i64],
    user_id: i64,
) -> Result<HashMap<i64, crate::memory::types::Memory>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let ids_owned: Vec<i64> = ids.to_vec();
    let placeholders = ids_owned.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let fetch_sql = format!(
        "SELECT {} FROM memories \
         WHERE id IN ({}) AND user_id = ? AND is_forgotten = 0 AND is_latest = 1 \
           AND is_consolidated = 0",
        MEMORY_COLUMNS, placeholders
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&fetch_sql).map_err(rusqlite_to_eng_error)?;

        // Build dynamic params: all IDs followed by user_id
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(ids_owned.len() + 1);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;

        let mut map = HashMap::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let mem = row_to_memory(row)?;
            map.insert(mem.id, mem);
        }
        Ok(map)
    })
    .await
}

async fn fetch_links_for_search(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<LinkedMemory>> {
    let link_sql = "SELECT ml.target_id, ml.similarity, ml.type, \
        m.content, m.category, m.is_forgotten \
        FROM memory_links ml JOIN memories m ON m.id = ml.target_id \
        WHERE ml.source_id = ?1 AND m.user_id = ?2 \
        UNION \
        SELECT ml.source_id, ml.similarity, ml.type, \
        m.content, m.category, m.is_forgotten \
        FROM memory_links ml JOIN memories m ON m.id = ml.source_id \
        WHERE ml.target_id = ?1 AND m.user_id = ?2";

    db.read(move |conn| {
        let mut stmt = conn.prepare(link_sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![memory_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: typical link fanout.
        let mut links = Vec::with_capacity(16);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            if row.get::<_, i32>(5).map_err(rusqlite_to_eng_error)? != 0 {
                continue;
            }
            links.push(LinkedMemory {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                similarity: ((row.get::<_, f64>(1).map_err(rusqlite_to_eng_error)? * 1000.0)
                    .round())
                    / 1000.0,
                link_type: row.get(2).map_err(rusqlite_to_eng_error)?,
                content: row.get(3).map_err(rusqlite_to_eng_error)?,
                category: row.get(4).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(links)
    })
    .await
}

/// Batch-fetch links for multiple memory IDs in a single query. Returns a
/// HashMap keyed by the source memory ID.
async fn fetch_links_batch(
    db: &Database,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<HashMap<i64, Vec<LinkedMemory>>> {
    if memory_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let ids_owned: Vec<i64> = memory_ids.to_vec();
    let placeholders = ids_owned.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // For each memory_id we need both directions. We tag each row with the
    // "owner" memory ID so we can group results into the right bucket.
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
        let mut stmt = conn.prepare(&link_sql).map_err(rusqlite_to_eng_error)?;

        // Params: [ids..., user_id, ids..., user_id]
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(ids_owned.len() * 2 + 2);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;

        let mut map: HashMap<i64, Vec<LinkedMemory>> = HashMap::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            // Skip forgotten memories
            if row.get::<_, i32>(6).map_err(rusqlite_to_eng_error)? != 0 {
                continue;
            }
            let owner: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
            let link = LinkedMemory {
                id: row.get(1).map_err(rusqlite_to_eng_error)?,
                similarity: ((row.get::<_, f64>(2).map_err(rusqlite_to_eng_error)? * 1000.0)
                    .round())
                    / 1000.0,
                link_type: row.get(3).map_err(rusqlite_to_eng_error)?,
                content: row.get(4).map_err(rusqlite_to_eng_error)?,
                category: row.get(5).map_err(rusqlite_to_eng_error)?,
            };
            map.entry(owner).or_default().push(link);
        }
        Ok(map)
    })
    .await
}

async fn fetch_version_chain_for_search(
    db: &Database,
    root_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let chain_sql = "SELECT id, content, version, is_latest FROM memories \
        WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2 \
        ORDER BY version ASC";

    db.read(move |conn| {
        let mut stmt = conn.prepare(chain_sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![root_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: version chains are usually short.
        let mut chain = Vec::with_capacity(8);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            chain.push(VersionChainEntry {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                content: row.get(1).map_err(rusqlite_to_eng_error)?,
                version: row.get(2).map_err(rusqlite_to_eng_error)?,
                is_latest: row.get::<_, i32>(3).map_err(rusqlite_to_eng_error)? != 0,
            });
        }
        Ok(chain)
    })
    .await
}

/// Batch-fetch version chains for multiple root IDs in a single query.
/// Returns a HashMap keyed by root_memory_id.
async fn fetch_version_chains_batch(
    db: &Database,
    root_ids: &[i64],
    user_id: i64,
) -> Result<HashMap<i64, Vec<VersionChainEntry>>> {
    if root_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let ids_owned: Vec<i64> = root_ids.to_vec();
    let placeholders = ids_owned.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let chain_sql = format!(
        "SELECT COALESCE(root_memory_id, id) AS root, id, content, version, is_latest \
         FROM memories \
         WHERE (root_memory_id IN ({placeholders}) OR id IN ({placeholders})) AND user_id = ? \
         ORDER BY root, version ASC"
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&chain_sql).map_err(rusqlite_to_eng_error)?;

        // Params: [ids..., ids..., user_id]
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(ids_owned.len() * 2 + 1);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;

        let mut map: HashMap<i64, Vec<VersionChainEntry>> = HashMap::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let root: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
            let entry = VersionChainEntry {
                id: row.get(1).map_err(rusqlite_to_eng_error)?,
                content: row.get(2).map_err(rusqlite_to_eng_error)?,
                version: row.get(3).map_err(rusqlite_to_eng_error)?,
                is_latest: row.get::<_, i32>(4).map_err(rusqlite_to_eng_error)? != 0,
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
pub async fn hybrid_search(db: &Database, req: SearchRequest) -> Result<Vec<SearchResult>> {
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

    // Ranked lists for RRF fusion
    let mut vector_ranked: Vec<(i64, f64)> = Vec::new();
    let mut fts_ranked: Vec<(i64, f64)> = Vec::new();
    let mut results: HashMap<i64, Candidate> = HashMap::new();

    // Channel 1: Vector ANN search
    if let Some(ref embedding) = req.embedding {
        let vector_hits = if let Some(index) = db.vector_index.as_ref() {
            match index.search(embedding, candidate_target, user_id).await {
                Ok(hits) => Ok(hits
                    .into_iter()
                    .map(|hit| super::types::VectorHit {
                        memory_id: hit.memory_id,
                        rank: hit.rank,
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
        };

        match vector_hits {
            Ok(hits) => {
                for hit in &hits {
                    vector_ranked.push((hit.memory_id, hit.rank as f64));
                    results.entry(hit.memory_id).or_insert_with(|| Candidate {
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
                        semantic_score: None,
                        personality_signal_score: None,
                        score: 0.0,
                        combined_score: 0.0,
                        decay_score: None,
                        temporal_boost: None,
                    });
                }
            }
            Err(e) => warn!("vector search failed: {}", e),
        }
    }

    // Channel 2: FTS5 search
    if !req.query.is_empty() {
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
                    semantic_score: None,
                    personality_signal_score: None,
                    score: 0.0,
                    combined_score: 0.0,
                    decay_score: None,
                    temporal_boost: None,
                });
                // FTS provides content we can use
                let _ = entry;
            }
        }
    }

    if results.is_empty() {
        return Ok(vec![]);
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
        let ids: Vec<i64> = results.keys().copied().collect();
        if !ids.is_empty() {
            if let Ok(rows) = hydrate_candidates(db, &ids, user_id).await {
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
                        if c.content.is_empty() {
                            c.content = row.content;
                        }
                        if c.category.is_empty() {
                            c.category = row.category;
                        }
                    }
                }
            }
        }
    }

    // Apply RRF + decay + boosts to each candidate
    for c in results.values_mut() {
        let rrf = rrf_scores.get(&c.id).copied().unwrap_or(0.0);

        // Live FSRS retrievability
        let retrievability = if c.is_static {
            1.0
        } else {
            let stability = {
                // Quick stability lookup -- use initial_stability(Good) as default
                let default_s = crate::fsrs::initial_stability(crate::fsrs::Rating::Good);
                default_s as f64
            };
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

        let decay_factor = if c.is_static {
            1.0
        } else {
            DECAY_FLOOR + (1.0 - DECAY_FLOOR) * retrievability
        };
        let src_boost = scoring::source_count_boost(c.source_count);
        let stat_boost = scoring::static_boost(c.is_static);

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

        c.score = rrf * decay_factor * src_boost * stat_boost * temp_boost * pr_boost * contr;
        c.combined_score = c.score;
    }

    // Relationship expansion (2-hop) -- graph RRF channel
    let mut graph_score_map: HashMap<i64, f64> = HashMap::new();
    if strategy.expand_relationships {
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

        for rows in neighbor_results.into_iter().flatten() {
            {
                let mut added = 0usize;
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

                    if let std::collections::hash_map::Entry::Vacant(e) = results.entry(row.link_id)
                    {
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
                            semantic_score: None,
                            personality_signal_score: None,
                            score: 0.0,
                            combined_score: 0.0,
                            decay_score: None,
                            temporal_boost: None,
                        });
                        added += 1;
                    }
                }
            }
        }

        // Apply graph RRF scores
        let mut graph_ranked: Vec<(i64, f64)> =
            graph_score_map.iter().map(|(&id, &s)| (id, s)).collect();
        graph_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (id, _)) in graph_ranked.iter().enumerate() {
            if let Some(c) = results.get_mut(id) {
                c.score += rrf_score(rank);
                c.combined_score = c.score;
            }
        }
    }
    let graph_set: HashSet<i64> = graph_score_map.keys().copied().collect();

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
    let candidate_ids: Vec<i64> = sorted.iter().map(|c| c.id).collect();
    let memory_map = fetch_memories_batch(db, &candidate_ids, user_id).await?;

    let mut final_results: Vec<SearchResult> = Vec::with_capacity(sorted.len());

    for c in &sorted {
        // Build channel list
        let mut channels = Vec::new();
        if vector_set.contains(&c.id) {
            channels.push("vector".to_string());
        }
        if fts_set.contains(&c.id) {
            channels.push("fts".to_string());
        }
        if graph_set.contains(&c.id) {
            channels.push("graph".to_string());
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
        let result_ids: Vec<i64> = final_results.iter().map(|r| r.memory.id).collect();
        let root_ids: Vec<i64> = final_results
            .iter()
            .map(|r| r.memory.root_memory_id.unwrap_or(r.memory.id))
            .collect();

        let links_map = fetch_links_batch(db, &result_ids, user_id).await?;
        let chains_map = fetch_version_chains_batch(db, &root_ids, user_id).await?;

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

    // 3.5: Populate cache before returning.
    cache_put(user_id, param_hash, &final_results);

    Ok(final_results)
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
pub async fn faceted_search(
    db: &Database,
    req: FacetedSearchRequest,
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

    // Predicate closure over a Memory: returns true if it passes all filters.
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
            embedding: req.embedding.clone(),
            limit: Some(over_fetch),
            category: req.category.clone(),
            source: req.source.clone(),
            tags: req.tags_all.clone(),
            threshold: None,
            user_id: Some(user_id),
            space_id: req.space_id,
            include_forgotten: Some(false),
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            latest_only: true,
            source_filter: None,
        };
        let mut candidates = hybrid_search(db, search_req).await?;
        // hybrid_search already applies category/source/tags_all/space_id,
        // but we still need importance range, date range, tags_any, tags_none.
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
    let mut conditions = vec![
        "user_id = ?1".to_string(),
        "is_forgotten = 0".to_string(),
        "is_latest = 1".to_string(),
        "is_consolidated = 0".to_string(),
    ];
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql + Send>> = vec![Box::new(user_id)];
    let mut idx = 2usize;

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
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: SQL over-fetches limit*3 for tag filtering.
        let mut memories = Vec::with_capacity(limit.saturating_mul(3));
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row)?);
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

fn compute_string_facets<'a>(
    values: impl Iterator<Item = &'a str>,
    limit: usize,
) -> Vec<FacetBucket> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for v in values {
        if !v.is_empty() {
            *counts.entry(v.to_string()).or_insert(0) += 1;
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
    pairs.sort_by(|a, b| b.count.cmp(&a.count));
    pairs.truncate(limit);
    pairs
}

/// Parse an ISO-8601 date string into a comparable string (normalize format).
fn parse_iso_date(s: &str) -> Option<String> {
    // Accept both "2024-01-15" and "2024-01-15T00:00:00Z" formats.
    // Return normalized form for string comparison against created_at.
    let trimmed = s.trim();
    if trimmed.len() >= 10 {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Auto-link a memory to similar memories based on embedding similarity.
/// Matches TS autoLink function.
#[tracing::instrument(skip(db, embedding), fields(embedding_dim = embedding.len()))]
pub async fn auto_link(
    db: &Database,
    memory_id: i64,
    embedding: &[f32],
    user_id: i64,
) -> Result<usize> {
    // Search for similar memories using vector search
    let hits = vector_search(db, embedding, 50, user_id).await?;

    let mut similarities: Vec<(i64, f64)> = Vec::new();
    // We use rank as a proxy for similarity since vector_top_k orders by distance
    // In a full implementation, we would compute cosine similarity directly
    for hit in &hits {
        if hit.memory_id == memory_id {
            continue;
        }
        // Approximate similarity from rank (closer rank = higher similarity)
        let approx_sim = 1.0 - (hit.rank as f64 / 50.0);
        if approx_sim >= scoring::AUTO_LINK_THRESHOLD {
            similarities.push((hit.memory_id, approx_sim));
        }
    }

    similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    similarities.truncate(scoring::AUTO_LINK_MAX);

    let mut linked = 0usize;
    for (target_id, similarity) in &similarities {
        let _ = crate::memory::insert_link(
            db,
            memory_id,
            *target_id,
            *similarity,
            "similarity",
            user_id,
        )
        .await;
        let _ = crate::memory::insert_link(
            db,
            *target_id,
            memory_id,
            *similarity,
            "similarity",
            user_id,
        )
        .await;
        linked += 1;
    }

    Ok(linked)
}
