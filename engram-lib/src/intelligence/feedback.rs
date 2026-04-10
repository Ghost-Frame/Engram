//! Memory feedback -- user ratings on memory recall quality.
//! Adjusts importance based on feedback signals.

use crate::db::Database;
use crate::memory;
use crate::Result;
use serde::{Deserialize, Serialize};

const VALID_RATINGS: &[&str] = &["helpful", "irrelevant", "off-topic", "outdated"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRequest {
    pub memory_id: i64,
    pub rating: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackStats {
    pub helpful: i64,
    pub irrelevant: i64,
    pub off_topic: i64,
    pub outdated: i64,
    pub total: i64,
}

/// Record user feedback on a memory and adjust its importance accordingly.
pub async fn record_feedback(db: &Database, user_id: i64, req: &FeedbackRequest) -> Result<()> {
    // Validate rating
    if !VALID_RATINGS.contains(&req.rating.as_str()) {
        return Err(crate::EngError::InvalidInput(format!(
            "invalid rating '{}'; must be one of: {}",
            req.rating,
            VALID_RATINGS.join(", ")
        )));
    }

    // Validate memory exists and belongs to user
    let _mem = memory::get(db, req.memory_id, user_id).await?;

    // Insert feedback record
    db.conn
        .execute(
            "INSERT INTO memory_feedback (memory_id, user_id, rating, context) \
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![
                req.memory_id,
                user_id,
                req.rating.clone(),
                req.context.clone()
            ],
        )
        .await?;

    // Adjust importance based on rating
    let delta = match req.rating.as_str() {
        "helpful" => 1,
        "irrelevant" | "off-topic" => -1,
        "outdated" => -2,
        _ => 0,
    };

    if delta != 0 {
        memory::adjust_importance(db, req.memory_id, user_id, delta).await?;
    }

    Ok(())
}

/// Get aggregated feedback statistics for a user.
pub async fn feedback_stats(db: &Database, user_id: i64) -> Result<FeedbackStats> {
    let mut rows = db
        .conn
        .query(
            "SELECT rating, COUNT(*) FROM memory_feedback \
             WHERE user_id = ?1 GROUP BY rating",
            libsql::params![user_id],
        )
        .await?;

    let mut stats = FeedbackStats {
        helpful: 0,
        irrelevant: 0,
        off_topic: 0,
        outdated: 0,
        total: 0,
    };

    while let Some(row) = rows.next().await? {
        let rating: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        match rating.as_str() {
            "helpful" => stats.helpful = count,
            "irrelevant" => stats.irrelevant = count,
            "off-topic" => stats.off_topic = count,
            "outdated" => stats.outdated = count,
            _ => {}
        }
        stats.total += count;
    }

    Ok(stats)
}
