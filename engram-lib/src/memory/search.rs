use super::fts::fts_search;
use super::vector::vector_search;
use super::{row_to_memory, rusqlite_to_eng_error, MEMORY_COLUMNS};
use crate::db::Database;
use crate::memory::scoring::{
    self, blend_strategies, classify_question_mixed, question_strategy, rrf_score, DECAY_FLOOR,
    RERANKER_TOP_K,
};
use crate::memory::types::{
    LinkedMemory, QuestionType, SearchRequest, SearchResult, VersionChainEntry,
};
use crate::Result;
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

const DEFAULT_LIMIT: usize = 10;

/// Hard ceiling on results returned by hybrid_search. Applied at the library
/// level so all consumers (HTTP routes, MCP, sidecar, CLI) inherit the cap.
const MAX_LIMIT: usize = 100;

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

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(ids_owned.len() + 1);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        let mut hydrated = Vec::new();
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
        let mut linked = Vec::new();
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
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(ids_owned.len() + 1);
        for id in &ids_owned {
            params.push(Box::new(*id));
        }
        params.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

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
        let mut links = Vec::new();
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

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
        let mut chain = Vec::new();
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

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
pub async fn hybrid_search(db: &Database, req: SearchRequest) -> Result<Vec<SearchResult>> {
    // SECURITY (SEC-MED-6): clamp at library entry point so MCP, sidecar,
    // and CLI callers inherit the cap. HTTP route-level clamp is kept as
    // defense-in-depth.
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
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
                    .map(|hit| super::vector::VectorHit {
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

    // RRF fusion across channels
    let mut rrf_scores: HashMap<i64, f64> = HashMap::new();
    let vector_set: HashSet<i64> = vector_ranked.iter().map(|(id, _)| *id).collect();
    let fts_set: HashSet<i64> = fts_ranked.iter().map(|(id, _)| *id).collect();
    let fts_score_map: HashMap<i64, f64> = fts_ranked.iter().cloned().collect();

    for (rank, (id, _)) in vector_ranked.iter().enumerate() {
        *rrf_scores.entry(*id).or_default() += rrf_score(rank);
    }
    for (rank, (id, _)) in fts_ranked.iter().enumerate() {
        *rrf_scores.entry(*id).or_default() += rrf_score(rank);
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

        for (seed_id, _) in &top_ids {
            if let Ok(rows) = fetch_graph_neighbors(db, *seed_id, user_id).await {
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

    Ok(final_results)
}

/// Auto-link a memory to similar memories based on embedding similarity.
/// Matches TS autoLink function.
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
