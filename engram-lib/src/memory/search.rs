use crate::db::Database;
use crate::memory::types::{QuestionType, SearchRequest, SearchResult};
use crate::memory::scoring::{
    self, classify_question_mixed, blend_strategies,
    question_strategy, rrf_score, DECAY_FLOOR, RERANKER_TOP_K,
};
use crate::Result;
use super::vector::vector_search;
use super::fts::fts_search;
use super::{row_to_memory, MEMORY_COLUMNS};
use tracing::{info, warn};
use std::collections::{HashMap, HashSet};

const DEFAULT_LIMIT: usize = 10;

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

/// Resolve question type and search strategy from request.
fn resolve_strategy(req: &SearchRequest) -> (QuestionType, crate::memory::types::SearchStrategy) {
    if let Some(qt) = req.question_type {
        return (qt, question_strategy(qt));
    }
    // Use mixed classification and blending
    let weights = classify_question_mixed(&req.query);
    let dominant = *weights.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
    let strategy = blend_strategies(&weights);
    (dominant, strategy)
}

/// Run the full hybrid search pipeline matching TS hybridSearch.
pub async fn hybrid_search(db: &Database, req: SearchRequest) -> Result<Vec<SearchResult>> {
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT);
    let user_id = req.user_id.unwrap_or(1);
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
        match vector_search(db, embedding, candidate_target, user_id).await {
            Ok(hits) => {
                for hit in &hits {
                    vector_ranked.push((hit.memory_id, hit.rank as f64));
                    results.entry(hit.memory_id).or_insert_with(|| Candidate {
                        id: hit.memory_id,
                        content: String::new(), category: String::new(),
                        source: None, model: None, importance: 5,
                        created_at: String::new(), version: None, is_latest: None,
                        is_static: false, source_count: 1, root_memory_id: None,
                        access_count: 0, pagerank_score: 0.0, semantic_score: None,
                        personality_signal_score: None, score: 0.0, combined_score: 0.0,
                        decay_score: None, temporal_boost: None,
                    });
                }
            }
            Err(e) => warn!("vector search failed: {}", e),
        }
    }

    // Channel 2: FTS5 search
    if !req.query.is_empty() {
        if let Ok(hits) = fts_search(db, &req.query, fts_limit.max(candidate_target), user_id).await {
                for hit in &hits {
                    fts_ranked.push((hit.memory_id, hit.bm25_score));
                    let entry = results.entry(hit.memory_id).or_insert_with(|| Candidate {
                        id: hit.memory_id,
                        content: String::new(), category: String::new(),
                        source: None, model: None, importance: 5,
                        created_at: String::new(), version: None, is_latest: None,
                        is_static: false, source_count: 1, root_memory_id: None,
                        access_count: 0, pagerank_score: 0.0, semantic_score: None,
                        personality_signal_score: None, score: 0.0, combined_score: 0.0,
                        decay_score: None, temporal_boost: None,
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
    {
        let ids: Vec<i64> = results.keys().copied().collect();
        if !ids.is_empty() {
            let placeholders: String = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let hydrate_sql = format!(
                "SELECT id, created_at, decay_score, importance, is_static, source_count, \
                 version, is_latest, source, model, access_count, fsrs_stability, \
                 last_accessed_at, pagerank_score, content, category \
                 FROM memories WHERE id IN ({})", placeholders
            );
            if let Ok(mut rows) = db.conn.query(&hydrate_sql, ()).await {
                while let Ok(Some(row)) = rows.next().await {
                    let id: i64 = row.get(0).unwrap_or(0);
                    if let Some(c) = results.get_mut(&id) {
                        c.created_at = row.get::<String>(1).unwrap_or_default();
                        c.importance = row.get::<i32>(3).unwrap_or(5);
                        c.is_static = row.get::<i32>(4).unwrap_or(0) != 0;
                        c.source_count = row.get::<i32>(5).unwrap_or(1).max(c.source_count);
                        if c.version.is_none() { c.version = row.get::<Option<i32>>(6).unwrap_or(None); }
                        if c.is_latest.is_none() { c.is_latest = Some(row.get::<i32>(7).unwrap_or(1) != 0); }
                        if c.source.is_none() { c.source = row.get::<Option<String>>(8).unwrap_or(None); }
                        if c.model.is_none() { c.model = row.get::<Option<String>>(9).unwrap_or(None); }
                        c.access_count = row.get::<i32>(10).unwrap_or(0);
                        c.pagerank_score = row.get::<Option<f64>>(13).unwrap_or(None).unwrap_or(0.0);
                        if c.content.is_empty() {
                            c.content = row.get::<String>(14).unwrap_or_default();
                        }
                        if c.category.is_empty() {
                            c.category = row.get::<String>(15).unwrap_or_default();
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
                let normalized = if ref_str.contains('Z') { ref_str.to_string() }
                    else { format!("{}Z", ref_str.replace(' ', "T")) };
                if let Ok(dt) = normalized.parse::<chrono::DateTime<chrono::Utc>>() {
                    let ms = (chrono::Utc::now().timestamp_millis() - dt.timestamp_millis()).max(0);
                    ms as f64 / 86_400_000.0
                } else { 0.0 }
            } else { 0.0 };
            crate::fsrs::retrievability(stability as f32, elapsed as f32) as f64
        };

        c.decay_score = Some((c.importance as f64 * retrievability * 1000.0).round() / 1000.0);

        let decay_factor = if c.is_static { 1.0 } else { DECAY_FLOOR + (1.0 - DECAY_FLOOR) * retrievability };
        let src_boost = scoring::source_count_boost(c.source_count);
        let stat_boost = scoring::static_boost(c.is_static);

        let temp_boost = if let Some(ref qd) = query_date {
            if !c.created_at.is_empty() {
                let b = scoring::temporal_proximity_boost(qd, &c.created_at);
                if b > 1.0 { c.temporal_boost = Some((b * 1000.0).round() / 1000.0); }
                b
            } else { 1.0 }
        } else { 1.0 };

        let pr_boost = scoring::pagerank_boost(c.pagerank_score);
        let contr = scoring::contradiction_penalty(&c.content, c.is_latest.unwrap_or(true));

        c.score = rrf * decay_factor * src_boost * stat_boost * temp_boost * pr_boost * contr;
        c.combined_score = c.score;
    }

    // Relationship expansion (2-hop) -- graph RRF channel
    let mut graph_score_map: HashMap<i64, f64> = HashMap::new();
    if strategy.expand_relationships {
        let mut top_ids: Vec<(i64, f64)> = results.iter()
            .map(|(&id, c)| (id, c.combined_score))
            .collect();
        top_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        top_ids.truncate(strategy.relationship_seed_limit);

        for (seed_id, _) in &top_ids {
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

            if let Ok(mut rows) = db.conn.query(link_sql, libsql::params![*seed_id, user_id]).await {
                let mut added = 0usize;
                while let Ok(Some(row)) = rows.next().await {
                    if added >= strategy.hop1_limit { break; }
                    let link_id: i64 = row.get(0).unwrap_or(0);
                    let similarity: f64 = row.get(1).unwrap_or(0.0);
                    let link_type: String = row.get(2).unwrap_or_default();
                    let is_forgotten: i32 = row.get(8).unwrap_or(0);
                    if is_forgotten != 0 { continue; }

                    let tw = scoring::link_type_weight(&link_type);
                    let gs = similarity * tw * strategy.relationship_multiplier;
                    let prev = graph_score_map.get(&link_id).copied().unwrap_or(0.0);
                    graph_score_map.insert(link_id, prev.max(gs));

                    if let std::collections::hash_map::Entry::Vacant(e) = results.entry(link_id) {
                        e.insert(Candidate {
                            id: link_id,
                            content: row.get::<String>(3).unwrap_or_default(),
                            category: row.get::<String>(4).unwrap_or_default(),
                            source: row.get::<Option<String>>(12).unwrap_or(None),
                            model: row.get::<Option<String>>(11).unwrap_or(None),
                            importance: row.get::<i32>(5).unwrap_or(5),
                            created_at: row.get::<String>(6).unwrap_or_default(),
                            version: row.get::<Option<i32>>(9).unwrap_or(None),
                            is_latest: Some(row.get::<i32>(7).unwrap_or(1) != 0),
                            is_static: false,
                            source_count: row.get::<i32>(10).unwrap_or(1),
                            root_memory_id: None, access_count: 0, pagerank_score: 0.0,
                            semantic_score: None, personality_signal_score: None,
                            score: 0.0, combined_score: 0.0,
                            decay_score: None, temporal_boost: None,
                        });
                        added += 1;
                    }
                }
            }
        }

        // Apply graph RRF scores
        let mut graph_ranked: Vec<(i64, f64)> = graph_score_map.iter()
            .map(|(&id, &s)| (id, s)).collect();
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
        if c.score.is_nan() { c.score = 0.0; }
        if c.combined_score.is_nan() { c.combined_score = c.score; }
        if let Some(d) = c.decay_score { if d.is_nan() { c.decay_score = Some(0.0); } }
    }

    let mut sorted: Vec<&Candidate> = results.values().collect();
    sorted.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));
    let candidate_count = sorted.len();
    sorted.truncate(limit);

    // Build final SearchResult vec
    let conn = db.connection();
    let fetch_sql = format!("SELECT {} FROM memories WHERE id = ?1", MEMORY_COLUMNS);
    let mut final_results: Vec<SearchResult> = Vec::with_capacity(sorted.len());

    for c in &sorted {
        // Build channel list
        let mut channels = Vec::new();
        if vector_set.contains(&c.id) { channels.push("vector".to_string()); }
        if fts_set.contains(&c.id) { channels.push("fts".to_string()); }
        if graph_set.contains(&c.id) { channels.push("graph".to_string()); }

        // Fetch full memory if needed
        let memory = match conn.query(&fetch_sql, libsql::params![c.id]).await {
            Ok(mut rows) => {
                match rows.next().await {
                    Ok(Some(row)) => match row_to_memory(&row) {
                        Ok(m) => m,
                        Err(e) => { warn!("row_to_memory failed for {}: {}", c.id, e); continue; }
                    },
                    _ => continue,
                }
            }
            Err(e) => { warn!("fetch memory {} failed: {}", c.id, e); continue; }
        };

        let fts_s = fts_score_map.get(&c.id).map(|s| (*s * 1000.0).round() / 1000.0);
        let graph_s = graph_score_map.get(&c.id).map(|s| (*s * 1000.0).round() / 1000.0);

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

    // Include linked memories + version chain if requested
    if req.include_links {
        for result in &mut final_results {
            // Links
            let link_sql = "SELECT ml.target_id, ml.similarity, ml.type, \
                m.content, m.category, m.is_forgotten \
                FROM memory_links ml JOIN memories m ON m.id = ml.target_id \
                WHERE ml.source_id = ?1 AND m.user_id = ?2 \
                UNION \
                SELECT ml.source_id, ml.similarity, ml.type, \
                m.content, m.category, m.is_forgotten \
                FROM memory_links ml JOIN memories m ON m.id = ml.source_id \
                WHERE ml.target_id = ?1 AND m.user_id = ?2";
            if let Ok(mut rows) = conn.query(link_sql, libsql::params![result.memory.id, user_id]).await {
                let mut links = Vec::new();
                while let Ok(Some(row)) = rows.next().await {
                    let is_forgotten: i32 = row.get(5).unwrap_or(0);
                    if is_forgotten != 0 { continue; }
                    links.push(crate::memory::types::LinkedMemory {
                        id: row.get(0).unwrap_or(0),
                        similarity: ((row.get::<f64>(1).unwrap_or(0.0)) * 1000.0).round() / 1000.0,
                        link_type: row.get(2).unwrap_or_default(),
                        content: row.get(3).unwrap_or_default(),
                        category: row.get(4).unwrap_or_default(),
                    });
                }
                if !links.is_empty() { result.linked = Some(links); }
            }

            // Version chain
            let root_id = result.memory.root_memory_id.unwrap_or(result.memory.id);
            let chain_sql = "SELECT id, content, version, is_latest FROM memories \
                WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2 \
                ORDER BY version ASC";
            if let Ok(mut rows) = conn.query(chain_sql, libsql::params![root_id, user_id]).await {
                let mut chain = Vec::new();
                while let Ok(Some(row)) = rows.next().await {
                    chain.push(crate::memory::types::VersionChainEntry {
                        id: row.get(0).unwrap_or(0),
                        content: row.get(1).unwrap_or_default(),
                        version: row.get(2).unwrap_or(1),
                        is_latest: row.get::<i32>(3).unwrap_or(0) != 0,
                    });
                }
                if chain.len() > 1 { result.version_chain = Some(chain); }
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
        if hit.memory_id == memory_id { continue; }
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
        // Insert bidirectional links
        let insert_sql = "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
            VALUES (?1, ?2, ?3, 'similarity')";
        let _ = db.conn.execute(insert_sql,
            libsql::params![memory_id, *target_id, *similarity]).await;
        let _ = db.conn.execute(insert_sql,
            libsql::params![*target_id, memory_id, *similarity]).await;
        linked += 1;
    }

    Ok(linked)
}
