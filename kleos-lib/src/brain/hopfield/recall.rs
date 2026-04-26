pub use super::types::{DecayStats, RecallResult};

use crate::db::Database;
use crate::{EngError, Result};

use super::edges::{self, EdgeType};
use super::network::{self, HopfieldNetwork};
use super::pattern::{self, BrainPattern};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---------------------------------------------------------------------------
// Causal keyword tables -- ported from eidolon absorb.rs
// ---------------------------------------------------------------------------

const STRONG_CAUSAL: &[&str] = &[
    "caused by",
    "resulted in",
    "led to",
    "as a result",
    "due to",
    "thanks to",
    "triggered",
];

const CONTEXT_CAUSAL: &[&str] = &["because", "since", "therefore", "consequently", "after"];

const WEAK_CAUSAL: &[&str] = &["broke", "fixed"];

const NEGATION: &[&str] = &[
    "not",
    "never",
    "didn't",
    "wasn't",
    "isn't",
    "won't",
    "can't",
    "couldn't",
    "wouldn't",
    "shouldn't",
    "no",
];

// ---------------------------------------------------------------------------
// Constants -- ported from eidolon decay.rs
// ---------------------------------------------------------------------------

/// Base decay rate per tick. Multiplied once per tick, so after N ticks
/// a pattern at importance=0 has strength *= BASE_DECAY_RATE^N.
const BASE_DECAY_RATE: f32 = 0.995;

/// Per-importance-point reduction in the effective decay rate.
/// importance=5 -> effective = 0.995 + 5*0.002 = 1.005 -> capped at 0.9999.
const IMPORTANCE_PROTECTION: f32 = 0.002;

/// The maximum effective decay rate (prevents importance from making
/// patterns immortal).
const MAX_EFFECTIVE_RATE: f32 = 0.9999;

/// Patterns whose strength drops below this threshold are considered dead
/// and eligible for pruning.
pub const DEATH_THRESHOLD: f32 = 0.05;

/// Recall boost applied on access: new_strength = s + RECALL_BOOST * (1 - s).
/// Rapidly rescues fading patterns when they prove relevant.
const RECALL_BOOST: f32 = 0.3;

/// Edge weight decay rate per tick.
const EDGE_DECAY_RATE: f32 = 0.998;

/// Edges below this weight are prunable.
const EDGE_PRUNE_THRESHOLD: f32 = 0.01;

/// Cosine similarity threshold for merge_similar.
const MERGE_SIMILARITY_THRESHOLD: f32 = 0.92;

// ---------------------------------------------------------------------------
// Operations -- the PLAN deliverables
// ---------------------------------------------------------------------------

/// Store a new pattern in both the in-memory network and the database.
#[tracing::instrument(skip(db, network, embedding), fields(pattern_id = id, user_id, importance, strength, embedding_len = embedding.len()))]
pub async fn store_pattern(
    db: &Database,
    network: &mut HopfieldNetwork,
    id: i64,
    embedding: &[f32],
    user_id: i64,
    importance: i32,
    strength: f32,
) -> Result<()> {
    // Store in network (L2-normalizes internally)
    network.store(id, embedding, strength);

    // Persist to database
    let bp = BrainPattern {
        id,
        user_id,
        pattern: embedding.to_vec(),
        strength,
        importance,
        access_count: 0,
        last_activated_at: None,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    pattern::store_pattern(db, &bp).await?;

    Ok(())
}

/// Store a new pattern and then detect causal edges to existing patterns.
///
/// This extends `store_pattern` with NLP-scored causal edge creation.
/// For each existing pattern within the 24h temporal window that has
/// moderate cosine similarity (0.3-0.75), the combined text is scanned
/// for causal keywords. A `Causal` edge is created when the score >= 3.0.
///
/// Parameters mirror those of `store_pattern` plus the content and
/// category fields needed for causal keyword matching, and `created_at`
/// for the temporal window check.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, network, embedding, content), fields(pattern_id = id, user_id, importance, strength, embedding_len = embedding.len(), content_len = content.len(), created_at = %created_at))]
pub async fn store_pattern_with_causal_edges(
    db: &Database,
    network: &mut HopfieldNetwork,
    id: i64,
    embedding: &[f32],
    user_id: i64,
    importance: i32,
    strength: f32,
    content: &str,
    created_at: &str,
) -> Result<()> {
    // Step 1: persist the new pattern.
    store_pattern(db, network, id, embedding, user_id, importance, strength).await?;

    // Step 2: load existing patterns to check for causal relationships.
    let existing_patterns = pattern::list_patterns(db, user_id).await?;
    if existing_patterns.is_empty() {
        return Ok(());
    }

    // Load content for existing patterns from the memories table.
    let existing_ids: Vec<i64> = existing_patterns.iter().map(|p| p.id).collect();
    let content_map = load_memory_content(db, &existing_ids).await?;

    let new_ts = parse_datetime_approx(created_at);
    let normalized_new = network::l2_normalize(embedding);

    for ep in &existing_patterns {
        if ep.id == id {
            continue;
        }
        if ep.pattern.is_empty() {
            continue;
        }

        // Temporal window: only consider patterns within 24 hours.
        let existing_content = match content_map.get(&ep.id) {
            Some(c) => c,
            None => continue, // ghost or deleted memory -- skip
        };

        let existing_ts = parse_datetime_approx(&ep.created_at);
        let time_diff = (new_ts - existing_ts).abs();
        if time_diff > TEMPORAL_WINDOW_SECS {
            continue;
        }

        let normalized_existing = network::l2_normalize(&ep.pattern);
        let sim = network::cosine_similarity(&normalized_new, &normalized_existing);

        // Causal scoring applies only to moderate similarity (not contradictions).
        if !(0.3..=0.75).contains(&sim) {
            continue;
        }

        let combined = format!("{} {}", content, existing_content).to_lowercase();
        let words: Vec<&str> = combined.split_whitespace().collect();
        let causal_score = compute_causal_score(&combined, &words);

        if causal_score >= 3.0 {
            let edge_weight = sim * 0.5;
            // existing -> new: the new memory is the consequence.
            if let Err(e) =
                edges::store_edge(db, ep.id, id, edge_weight, EdgeType::Causal, user_id).await
            {
                tracing::warn!(source = ep.id, target = id, error = %e, "store_edge forward causal failed");
            }

            // Also check the reverse direction using existing content alone.
            let existing_lower = existing_content.to_lowercase();
            let existing_words: Vec<&str> = existing_lower.split_whitespace().collect();
            let reverse_score = compute_causal_score(&existing_lower, &existing_words);
            if reverse_score >= 3.0 {
                if let Err(e) =
                    edges::store_edge(db, id, ep.id, edge_weight, EdgeType::Causal, user_id).await
                {
                    tracing::warn!(source = id, target = ep.id, error = %e, "store_edge reverse causal failed");
                }
            }
        }
    }

    Ok(())
}

/// Temporal window used for causal edge candidate selection (seconds).
const TEMPORAL_WINDOW_SECS: f64 = 86400.0;

/// Parse an ISO-8601 / SQLite datetime string into a floating-point Unix
/// timestamp (seconds since epoch). Falls back to 0.0 on parse failure.
///
/// Handles the two common formats produced by this codebase:
/// - "YYYY-MM-DD HH:MM:SS" (SQLite datetime())
/// - "YYYY-MM-DDTHH:MM:SSZ" (ISO 8601)
pub(crate) fn parse_datetime_approx(s: &str) -> f64 {
    // Normalise: replace 'T' with ' ' and strip trailing 'Z'
    let s = s.replace('T', " ").replace('Z', "");
    let parts: Vec<&str> = s.split(' ').collect();
    if parts.len() < 2 {
        return 0.0;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();
    if date_parts.len() < 3 || time_parts.len() < 3 {
        return 0.0;
    }
    let year: i64 = date_parts[0].parse().unwrap_or(1970);
    let month: i64 = date_parts[1].parse().unwrap_or(1);
    let day: i64 = date_parts[2].parse().unwrap_or(1);
    let hour: i64 = time_parts[0].parse().unwrap_or(0);
    let min: i64 = time_parts[1].parse().unwrap_or(0);
    let sec: i64 = time_parts[2].parse().unwrap_or(0);

    // Very rough epoch approximation (ignores leap years/months precisely).
    let days = (year - 1970) * 365
        + (year - 1969) / 4
        + [0i64, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
            [(month.clamp(1, 12) - 1) as usize]
        + (day - 1);
    (days * 86400 + hour * 3600 + min * 60 + sec) as f64
}

/// Load the text content of memories by their IDs from the `memories` table.
/// Ghost patterns (negative IDs) are skipped silently.
async fn load_memory_content(
    db: &Database,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Only positive IDs exist in the memories table.
    let positive_ids: Vec<i64> = ids.iter().copied().filter(|&id| id > 0).collect();
    if positive_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let ids_cap = positive_ids.clone();

    db.read(move |conn| {
        // Build parameterised query with one ?N per ID.
        let placeholders: Vec<String> = (1..=ids_cap.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT id, content FROM memories WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids_cap.len());
        for id in &ids_cap {
            params.push(Box::new(*id));
        }

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let map = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id: i64 = row.get(0)?;
                let content: String = row.get(1)?;
                Ok((id, content))
            })
            .map_err(rusqlite_to_eng_error)?
            .map(|r| r.map_err(rusqlite_to_eng_error))
            .collect::<Result<std::collections::HashMap<i64, String>>>()?;

        Ok(map)
    })
    .await
}

/// Compute the tiered NLP causal score for a combined text string.
///
/// Scoring:
/// - STRONG_CAUSAL keywords: 2 points each
/// - CONTEXT_CAUSAL keywords: 0.5 pt alone, 2 pts if another causal keyword
///   is within 5 word-positions
/// - WEAK_CAUSAL keywords: 1 point each
/// - Negation within 3 words before a keyword: halves its score
///
/// Returns the total score. A score >= 3.0 triggers a causal edge.
fn compute_causal_score(text: &str, words: &[&str]) -> f32 {
    let mut score = 0.0f32;

    // Pre-compute word indices of all causal keywords.
    let mut all_kw_word_indices: Vec<usize> = Vec::new();
    for (wi, _) in words.iter().enumerate() {
        let prefix_len: usize = words[..wi].iter().map(|w| w.len() + 1).sum();
        if prefix_len > text.len() {
            break;
        }
        let remaining = &text[prefix_len..];
        let is_causal_kw = STRONG_CAUSAL
            .iter()
            .chain(CONTEXT_CAUSAL.iter())
            .chain(WEAK_CAUSAL.iter())
            .any(|kw| remaining.starts_with(kw));
        if is_causal_kw {
            all_kw_word_indices.push(wi);
        }
    }

    let has_negation = |word_idx: usize| -> bool {
        let start = word_idx.saturating_sub(3);
        (start..word_idx).any(|i| NEGATION.contains(&words[i]))
    };

    let has_nearby_causal = |word_idx: usize| -> bool {
        all_kw_word_indices
            .iter()
            .any(|&pos| pos != word_idx && (pos as isize - word_idx as isize).unsigned_abs() <= 5)
    };

    for kw in STRONG_CAUSAL {
        if let Some(pos) = text.find(kw) {
            let word_idx = text[..pos].split_whitespace().count();
            let mut pts = 2.0f32;
            if word_idx < words.len() && has_negation(word_idx) {
                pts *= 0.5;
            }
            score += pts;
        }
    }

    for kw in CONTEXT_CAUSAL {
        if let Some(pos) = text.find(kw) {
            let word_idx = text[..pos].split_whitespace().count();
            let negated = word_idx < words.len() && has_negation(word_idx);
            let has_context = has_nearby_causal(word_idx);
            let mut pts = if has_context { 2.0f32 } else { 0.5f32 };
            if negated {
                pts *= 0.5;
            }
            score += pts;
        }
    }

    for kw in WEAK_CAUSAL {
        if let Some(pos) = text.find(kw) {
            let word_idx = text[..pos].split_whitespace().count();
            let mut pts = 1.0f32;
            if word_idx < words.len() && has_negation(word_idx) {
                pts *= 0.5;
            }
            score += pts;
        }
    }

    score
}

/// Recall patterns from a (possibly partial/noisy) cue. Uses the
/// modern Hopfield softmax attention to find the best-matching stored
/// patterns, then optionally runs iterative completion to refine the
/// query into a full pattern.
#[tracing::instrument(skip(db, network, query_embedding), fields(user_id, top_k, beta, embedding_len = query_embedding.len()))]
pub async fn recall_pattern(
    db: &Database,
    network: &HopfieldNetwork,
    query_embedding: &[f32],
    user_id: i64,
    top_k: usize,
    beta: f32,
) -> Result<Vec<RecallResult>> {
    let results = network.retrieve(query_embedding, top_k, beta);

    // Touch each recalled pattern in the DB (update access tracking)
    for &(id, _) in &results {
        if let Err(e) = pattern::touch_pattern(db, id, user_id).await {
            tracing::warn!(pattern_id = id, error = %e, "touch_pattern failed");
        }
    }

    Ok(results
        .into_iter()
        .map(|(id, activation)| RecallResult {
            pattern_id: id,
            activation,
        })
        .collect())
}

/// Reinforce a pattern by applying a recall boost to its strength.
/// Returns the new strength.
///
/// Formula: new = old + RECALL_BOOST * (1 - old)
/// This rapidly rescues fading patterns: 0.1 -> 0.37, 0.5 -> 0.65,
/// 0.9 -> 0.93.
#[tracing::instrument(skip(db, network), fields(pattern_id = id, user_id))]
pub async fn reinforce(
    db: &Database,
    network: &mut HopfieldNetwork,
    id: i64,
    user_id: i64,
) -> Result<f32> {
    let old = network
        .strength(id)
        .ok_or_else(|| crate::EngError::NotFound(format!("brain pattern {}", id)))?;

    let new_strength = (old + RECALL_BOOST * (1.0 - old)).min(1.0);

    // Update in network
    network.update_strength(id, new_strength);

    // Persist to DB
    pattern::update_strength(db, id, user_id, new_strength).await?;
    pattern::touch_pattern(db, id, user_id).await?;

    Ok(new_strength)
}

/// Apply temporal decay to all patterns and edges. Each tick reduces
/// pattern strength by a factor that depends on the pattern's importance.
/// Dead patterns (strength < DEATH_THRESHOLD) are removed.
///
/// Returns statistics about what was decayed and removed.
#[tracing::instrument(skip(db, network), fields(user_id, ticks))]
pub async fn decay_tick(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    ticks: u32,
) -> Result<DecayStats> {
    let mut patterns_decayed = 0usize;
    let mut dead_ids = Vec::new();

    // Load importance values from DB for decay calculation
    let db_patterns = pattern::list_patterns(db, user_id).await?;
    let importance_map: std::collections::HashMap<i64, i32> =
        db_patterns.iter().map(|p| (p.id, p.importance)).collect();

    // Decay each pattern in the network
    for &id in network.pattern_ids().to_vec().iter() {
        let old_strength = match network.strength(id) {
            Some(s) => s,
            None => continue,
        };

        let importance = importance_map.get(&id).copied().unwrap_or(5);
        let effective_rate =
            (BASE_DECAY_RATE + importance as f32 * IMPORTANCE_PROTECTION).min(MAX_EFFECTIVE_RATE);

        let new_strength = old_strength * effective_rate.powi(ticks as i32);
        network.update_strength(id, new_strength);
        patterns_decayed += 1;

        if new_strength < DEATH_THRESHOLD {
            dead_ids.push(id);
        }
    }

    // Persist decayed strengths
    for &id in network.pattern_ids() {
        if let Some(s) = network.strength(id) {
            if let Err(e) = pattern::update_strength(db, id, user_id, s).await {
                tracing::warn!(pattern_id = id, strength = s, error = %e, "update_strength (decay persist) failed");
            }
        }
    }

    // Remove dead patterns
    let patterns_removed = dead_ids.len();
    for id in &dead_ids {
        network.remove(*id);
        if let Err(e) = pattern::delete_pattern(db, *id, user_id).await {
            tracing::warn!(pattern_id = *id, error = %e, "delete_pattern (dead cleanup) failed");
        }
    }

    // Decay edges
    let edges_decayed = edges::decay_edges(db, user_id, EDGE_DECAY_RATE.powi(ticks as i32)).await?;
    let edges_removed = edges::prune_edges(db, user_id, EDGE_PRUNE_THRESHOLD).await?;

    Ok(DecayStats {
        patterns_decayed,
        patterns_removed,
        edges_decayed,
        edges_removed,
    })
}

/// Prune patterns whose strength has fallen below a threshold.
/// Returns the count of removed patterns.
#[tracing::instrument(skip(db, network), fields(user_id, threshold))]
pub async fn prune_weak(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    threshold: f32,
) -> Result<usize> {
    let mut dead = Vec::new();
    for &id in network.pattern_ids().to_vec().iter() {
        if let Some(s) = network.strength(id) {
            if s < threshold {
                dead.push(id);
            }
        }
    }

    for &id in &dead {
        network.remove(id);
    }

    // Also remove from DB
    pattern::delete_weak_patterns(db, user_id, threshold).await?;

    Ok(dead.len())
}

/// Find pairs of patterns that are very similar (cosine_sim >= threshold)
/// and merge the weaker into the stronger. The merged (loser) pattern is
/// removed from both the network and the database.
///
/// Returns (winner_id, loser_id) pairs that were merged.
#[tracing::instrument(skip(db, network), fields(user_id, threshold))]
pub async fn merge_similar(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    threshold: f32,
) -> Result<Vec<(i64, i64)>> {
    let threshold = if threshold <= 0.0 {
        MERGE_SIMILARITY_THRESHOLD
    } else {
        threshold
    };

    // Collect all patterns for pairwise comparison
    let db_patterns = pattern::list_patterns(db, user_id).await?;
    let normalized: Vec<(i64, Vec<f32>)> = db_patterns
        .iter()
        .map(|p| (p.id, network::l2_normalize(&p.pattern)))
        .collect();

    let mut merged = Vec::new();
    let mut removed: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for i in 0..normalized.len() {
        if removed.contains(&normalized[i].0) {
            continue;
        }
        for j in (i + 1)..normalized.len() {
            if removed.contains(&normalized[j].0) {
                continue;
            }
            let sim = network::cosine_similarity(&normalized[i].1, &normalized[j].1);
            if sim >= threshold {
                let id_a = normalized[i].0;
                let id_b = normalized[j].0;
                let s_a = network.strength(id_a).unwrap_or(0.0);
                let s_b = network.strength(id_b).unwrap_or(0.0);

                let (winner, loser) = if s_a >= s_b {
                    (id_a, id_b)
                } else {
                    (id_b, id_a)
                };

                // Remove loser from network and DB
                network.remove(loser);
                if let Err(e) = pattern::delete_pattern(db, loser, user_id).await {
                    tracing::warn!(pattern_id = loser, error = %e, "delete_pattern (merge loser) failed");
                }

                // Boost winner strength to max of both
                let winner_strength = s_a.max(s_b).min(1.0);
                network.update_strength(winner, winner_strength);
                if let Err(e) = pattern::update_strength(db, winner, user_id, winner_strength).await
                {
                    tracing::warn!(pattern_id = winner, strength = winner_strength, error = %e, "update_strength (merge winner boost) failed");
                }

                removed.insert(loser);
                merged.push((winner, loser));
            }
        }
    }

    Ok(merged)
}
