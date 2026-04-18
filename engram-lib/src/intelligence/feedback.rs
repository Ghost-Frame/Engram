//! Memory feedback -- user ratings on memory recall quality.
//! Adjusts importance based on feedback signals.

use super::types::{FeedbackRequest, FeedbackStats};
use crate::db::Database;
use crate::memory;
use crate::{EngError, Result};
use rusqlite::params;
use std::collections::HashMap;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

const VALID_RATINGS: &[&str] = &["helpful", "irrelevant", "off-topic", "outdated"];

/// Record user feedback on a memory and adjust its importance accordingly.
#[tracing::instrument(skip(db, req), fields(memory_id = req.memory_id, rating = %req.rating))]
pub async fn record_feedback(db: &Database, user_id: i64, req: &FeedbackRequest) -> Result<()> {
    // Validate rating
    if !VALID_RATINGS.contains(&req.rating.as_str()) {
        return Err(EngError::InvalidInput(format!(
            "invalid rating '{}'; must be one of: {}",
            req.rating,
            VALID_RATINGS.join(", ")
        )));
    }

    // Validate memory exists and belongs to user
    let _mem = memory::get(db, req.memory_id, user_id).await?;

    // Insert feedback record
    let memory_id = req.memory_id;
    let rating = req.rating.clone();
    let context = req.context.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO memory_feedback (memory_id, user_id, rating, context) \
             VALUES (?1, ?2, ?3, ?4)",
            params![memory_id, user_id, rating, context],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
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

/// Compute per-category preference scores for a user, aggregated from their
/// feedback history. Each `helpful` rating contributes +1.0, each
/// `irrelevant`, `off-topic`, or `outdated` rating contributes -1.0. The
/// returned value per category is the mean contribution across feedback rows
/// on memories of that category, naturally in `[-1.0, 1.0]`.
///
/// Used by retrieval scoring to boost/demote categories the user has
/// consistently marked helpful or unhelpful.
#[tracing::instrument(skip(db))]
pub async fn category_preferences(db: &Database, user_id: i64) -> Result<HashMap<String, f64>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT m.category, f.rating, COUNT(*) \
                 FROM memory_feedback f \
                 JOIN memories m ON m.id = f.memory_id \
                 WHERE f.user_id = ?1 \
                 GROUP BY m.category, f.rating",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![user_id], |row| {
                let cat: String = row.get(0)?;
                let rating: String = row.get(1)?;
                let count: i64 = row.get(2)?;
                Ok((cat, rating, count))
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut totals: HashMap<String, (f64, f64)> = HashMap::new();
        for row in rows {
            let (cat, rating, count) = row.map_err(rusqlite_to_eng_error)?;
            let weight: f64 = match rating.as_str() {
                "helpful" => 1.0,
                "irrelevant" | "off-topic" | "outdated" => -1.0,
                _ => continue,
            };
            let c = count as f64;
            let entry = totals.entry(cat).or_insert((0.0, 0.0));
            entry.0 += weight * c;
            entry.1 += c;
        }

        let mut out = HashMap::with_capacity(totals.len());
        for (cat, (sum, count)) in totals {
            if count > 0.0 {
                out.insert(cat, sum / count);
            }
        }
        Ok(out)
    })
    .await
}

/// Get aggregated feedback statistics for a user.
#[tracing::instrument(skip(db))]
pub async fn feedback_stats(db: &Database, user_id: i64) -> Result<FeedbackStats> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT rating, COUNT(*) FROM memory_feedback \
                 WHERE user_id = ?1 GROUP BY rating",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![user_id], |row| {
                let rating: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((rating, count))
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut stats = FeedbackStats {
            helpful: 0,
            irrelevant: 0,
            off_topic: 0,
            outdated: 0,
            total: 0,
        };

        for row in rows {
            let (rating, count) = row.map_err(rusqlite_to_eng_error)?;
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
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::StoreRequest;

    fn store_req(content: &str, category: &str, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: category.to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn seed_memory(db: &Database, content: &str, category: &str, user_id: i64) -> i64 {
        crate::memory::store(db, store_req(content, category, user_id))
            .await
            .expect("store")
            .id
    }

    async fn add_feedback(db: &Database, mid: i64, user_id: i64, rating: &str) {
        record_feedback(
            db,
            user_id,
            &FeedbackRequest {
                memory_id: mid,
                rating: rating.to_string(),
                context: None,
            },
        )
        .await
        .expect("feedback");
    }

    #[tokio::test]
    async fn category_preferences_empty_when_no_feedback() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let prefs = category_preferences(&db, 1).await.expect("prefs");
        assert!(prefs.is_empty());
    }

    #[tokio::test]
    async fn category_preferences_zero_when_helpful_equals_negative() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let m1 = seed_memory(&db, "alpha unique rust fact one", "code", uid).await;
        let m2 = seed_memory(&db, "beta unique rust fact two", "code", uid).await;
        add_feedback(&db, m1, uid, "helpful").await;
        add_feedback(&db, m2, uid, "irrelevant").await;
        let prefs = category_preferences(&db, uid).await.expect("prefs");
        assert!((prefs["code"] - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn category_preferences_outdated_weights_same_as_irrelevant() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let m1 = seed_memory(&db, "gamma manual page content", "docs", uid).await;
        let m2 = seed_memory(&db, "delta handbook chapter three", "docs", uid).await;
        add_feedback(&db, m1, uid, "outdated").await;
        add_feedback(&db, m2, uid, "off-topic").await;
        let prefs = category_preferences(&db, uid).await.expect("prefs");
        assert!((prefs["docs"] - (-1.0)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn category_preferences_separates_by_category() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let liked = seed_memory(&db, "epsilon notebook entry alpha", "notes", uid).await;
        let disliked = seed_memory(&db, "zeta idle chatter stream", "chatter", uid).await;
        add_feedback(&db, liked, uid, "helpful").await;
        add_feedback(&db, disliked, uid, "irrelevant").await;
        let prefs = category_preferences(&db, uid).await.expect("prefs");
        assert!((prefs["notes"] - 1.0).abs() < 1e-9);
        assert!((prefs["chatter"] - (-1.0)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn category_preferences_isolated_per_user() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mine = seed_memory(&db, "eta private note item", "code", 1).await;
        add_feedback(&db, mine, 1, "helpful").await;
        let prefs = category_preferences(&db, 2).await.expect("prefs");
        assert!(prefs.is_empty());
    }
}
