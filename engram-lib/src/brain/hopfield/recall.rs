use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

use super::edges;
use super::network::{self, HopfieldNetwork};
use super::pattern::{self, BrainPattern};

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
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub pattern_id: i64,
    pub activation: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayStats {
    pub patterns_decayed: usize,
    pub patterns_removed: usize,
    pub edges_decayed: usize,
    pub edges_removed: usize,
}

// ---------------------------------------------------------------------------
// Operations -- the PLAN deliverables
// ---------------------------------------------------------------------------

/// Store a new pattern in both the in-memory network and the database.
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

/// Recall patterns from a (possibly partial/noisy) cue. Uses the
/// modern Hopfield softmax attention to find the best-matching stored
/// patterns, then optionally runs iterative completion to refine the
/// query into a full pattern.
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
        let _ = pattern::touch_pattern(db, id, user_id).await;
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
            let _ = pattern::update_strength(db, id, user_id, s).await;
        }
    }

    // Remove dead patterns
    let patterns_removed = dead_ids.len();
    for id in &dead_ids {
        network.remove(*id);
        let _ = pattern::delete_pattern(db, *id, user_id).await;
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
                let _ = pattern::delete_pattern(db, loser, user_id).await;

                // Boost winner strength to max of both
                let winner_strength = s_a.max(s_b).min(1.0);
                network.update_strength(winner, winner_strength);
                let _ = pattern::update_strength(db, winner, user_id, winner_strength).await;

                removed.insert(loser);
                merged.push((winner, loser));
            }
        }
    }

    Ok(merged)
}
