//! Memory reconsolidation -- periodic re-evaluation of memory importance/confidence.
//!
//! Inspired by neuroscience: the brain periodically pulls old memories back into
//! active state, re-evaluates them against current context, and either strengthens
//! or rewrites them.

use crate::db::Database;
use crate::intelligence::types::{ReconsolidationAction, ReconsolidationResult};
use crate::Result;
use tracing::{info, warn};

/// Re-evaluate a single memory against current knowledge.
///
/// Checks:
/// 1. Is this memory contradicted by newer, higher-confidence memories?
/// 2. Has this memory been accessed often (useful) or ignored (irrelevant)?
/// 3. Is the memory's FSRS stability declining (being forgotten)?
/// 4. Age + static classification -- very old dynamic memories decay
pub async fn reconsolidate_memory(
    db: &Database,
    memory_id: i64,
    _user_id: i64,
) -> Result<ReconsolidationResult> {
    let conn = db.connection();

    // Fetch the memory
    let mut rows = conn
        .query(
            "SELECT id, importance, confidence, is_static, access_count, \
                    recall_hits, recall_misses, fsrs_stability, created_at \
             FROM memories WHERE id = ?1",
            libsql::params![memory_id],
        )
        .await?;

    let row = match rows.next().await? {
        Some(r) => r,
        None => {
            return Err(crate::EngError::NotFound(format!(
                "memory {} not found",
                memory_id
            )));
        }
    };

    let importance: i32 = row.get(1)?;
    let confidence: f64 = row.get(2)?;
    let is_static: bool = row.get::<i64>(3).map(|v| v != 0)?;
    let access_count: i32 = row.get(4)?;
    let recall_hits: i32 = row.get(5)?;
    let recall_misses: i32 = row.get(6)?;
    let fsrs_stability: Option<f64> = row.get(7)?;
    let created_at: String = row.get(8)?;

    let mut new_importance = importance;
    let mut new_confidence = confidence;
    let mut reason = String::new();

    // Check 1: Contradictions -- newer memories that supersede this one
    let mut contra_rows = conn
        .query(
            "SELECT COUNT(*) FROM memory_links \
             WHERE target_id = ?1 AND type IN ('corrects', 'updates', 'contradicts')",
            libsql::params![memory_id],
        )
        .await?;

    if let Some(row) = contra_rows.next().await? {
        let contra_count: i64 = row.get(0)?;
        if contra_count > 0 {
            new_confidence = (new_confidence * 0.5).max(0.1);
            new_importance = (new_importance - 2).max(1);
            reason.push_str(&format!("Superseded by {} newer memories. ", contra_count));
        }
    }

    // Check 2: Access patterns -- adaptive importance
    let total_recalls = recall_hits + recall_misses;
    if total_recalls > 3 {
        let hit_rate = recall_hits as f64 / total_recalls as f64;
        if hit_rate > 0.7 {
            new_importance = (new_importance + 1).min(10);
            new_confidence = (new_confidence + 0.1).min(1.0);
            reason.push_str(&format!(
                "High recall utility ({:.0}% hit rate). ",
                hit_rate * 100.0
            ));
        } else if hit_rate < 0.3 {
            new_importance = (new_importance - 1).max(1);
            reason.push_str(&format!(
                "Low recall utility ({:.0}% hit rate). ",
                hit_rate * 100.0
            ));
        }
    }

    // Check 3: FSRS stability -- if very low, memory is being forgotten
    if let Some(stability) = fsrs_stability {
        if stability < 0.5 {
            new_confidence = (new_confidence * 0.8).max(0.1);
            reason.push_str(&format!("Low FSRS stability ({:.2}). ", stability));
        }
    }

    // Check 4: Age + static classification
    // Parse created_at and compute age in days
    if !is_static && access_count < 3 {
        // Rough age check: if created_at is more than 30 days ago
        if let Ok(created) = chrono::NaiveDateTime::parse_from_str(&created_at, "%Y-%m-%d %H:%M:%S")
        {
            let now = chrono::Utc::now().naive_utc();
            let age_days = (now - created).num_days();
            if age_days > 30 {
                new_importance = (new_importance - 1).max(1);
                reason.push_str("Old dynamic memory with low access. ");
            }
        }
    }

    // Determine action
    let action = if new_importance == importance && (new_confidence - confidence).abs() < 0.05 {
        ReconsolidationAction::Unchanged
    } else if new_importance > importance || new_confidence > confidence {
        ReconsolidationAction::Strengthened
    } else if new_confidence < confidence * 0.6 {
        ReconsolidationAction::Corrected
    } else {
        ReconsolidationAction::Weakened
    };

    // Apply changes if any
    if action != ReconsolidationAction::Unchanged {
        conn.execute(
            "UPDATE memories SET importance = ?1, confidence = ?2, updated_at = datetime('now') \
             WHERE id = ?3",
            libsql::params![new_importance, new_confidence, memory_id],
        )
        .await?;

        // Update adaptive score
        let adaptive_score = if total_recalls > 0 {
            recall_hits as f64 / total_recalls as f64
        } else {
            0.5
        };
        conn.execute(
            "UPDATE memories SET adaptive_score = ?1 WHERE id = ?2",
            libsql::params![adaptive_score, memory_id],
        )
        .await?;

        // Record reconsolidation
        conn.execute(
            "INSERT INTO reconsolidations \
             (memory_id, old_content, new_content, reason, user_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            libsql::params![
                memory_id,
                format!("importance:{}, confidence:{:.2}", importance, confidence),
                format!(
                    "importance:{}, confidence:{:.2}",
                    new_importance, new_confidence
                ),
                reason.trim().to_string(),
                _user_id
            ],
        )
        .await?;

        info!(
            memory_id,
            action = ?action,
            old_imp = importance,
            new_imp = new_importance,
            "reconsolidated"
        );
    }

    Ok(ReconsolidationResult {
        memory_id,
        action,
        old_importance: importance,
        new_importance,
        old_confidence: confidence,
        new_confidence,
        reason: if reason.is_empty() {
            "No changes needed".to_string()
        } else {
            reason.trim().to_string()
        },
    })
}

/// Run a reconsolidation sweep over memories that need re-evaluation.
/// Called periodically (e.g., every hour).
pub async fn run_reconsolidation_sweep(
    db: &Database,
    user_id: i64,
    batch_size: usize,
) -> Result<Vec<ReconsolidationResult>> {
    let conn = db.connection();

    // Find candidates: old memories with low access, or memories with recall data
    let mut rows = conn
        .query(
            "SELECT id FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 \
               AND (recall_hits + recall_misses > 0 \
                    OR (access_count < 3 AND created_at < datetime('now', '-7 days'))) \
             ORDER BY updated_at ASC \
             LIMIT ?2",
            libsql::params![user_id, batch_size as i64],
        )
        .await?;

    let mut candidate_ids: Vec<i64> = Vec::new();
    while let Some(row) = rows.next().await? {
        candidate_ids.push(row.get(0)?);
    }

    let mut results = Vec::new();
    let candidate_count = candidate_ids.len();
    for &mem_id in &candidate_ids {
        match reconsolidate_memory(db, mem_id, user_id).await {
            Ok(result) => {
                if result.action != ReconsolidationAction::Unchanged {
                    results.push(result);
                }
            }
            Err(e) => {
                warn!(memory_id = mem_id, error = %e, "reconsolidation_error");
            }
        }
    }

    if !results.is_empty() {
        info!(
            processed = candidate_count,
            changed = results.len(),
            "reconsolidation_sweep"
        );
    }

    Ok(results)
}

/// Record whether a recalled memory was useful.
/// Called by search/recall endpoints when results are used or discarded.
pub async fn record_recall_outcome(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    useful: bool,
) -> Result<()> {
    let conn = db.connection();

    let affected = if useful {
        conn.execute(
            "UPDATE memories SET recall_hits = recall_hits + 1 WHERE id = ?1 AND user_id = ?2",
            libsql::params![memory_id, user_id],
        )
        .await?
    } else {
        conn.execute(
            "UPDATE memories SET recall_misses = recall_misses + 1 WHERE id = ?1 AND user_id = ?2",
            libsql::params![memory_id, user_id],
        )
        .await?
    };

    if affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "memory {} not found or not owned by user",
            memory_id
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconsolidation_action_unchanged() {
        let importance = 5;
        let confidence: f64 = 0.9;
        let new_importance = 5;
        let new_confidence: f64 = 0.9;

        let action = if new_importance == importance && (new_confidence - confidence).abs() < 0.05 {
            ReconsolidationAction::Unchanged
        } else {
            ReconsolidationAction::Weakened
        };

        assert_eq!(action, ReconsolidationAction::Unchanged);
    }

    #[test]
    fn test_reconsolidation_action_strengthened() {
        let importance = 5;
        let confidence: f64 = 0.8;
        let new_importance = 6;
        let new_confidence: f64 = 0.9;

        let action = if new_importance == importance && (new_confidence - confidence).abs() < 0.05 {
            ReconsolidationAction::Unchanged
        } else if new_importance > importance || new_confidence > confidence {
            ReconsolidationAction::Strengthened
        } else {
            ReconsolidationAction::Weakened
        };

        assert_eq!(action, ReconsolidationAction::Strengthened);
    }

    #[test]
    fn test_adaptive_score_calculation() {
        let hits = 7;
        let misses = 3;
        let total = hits + misses;
        let score = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.5
        };
        assert!((score - 0.7).abs() < 0.001);
    }
}
