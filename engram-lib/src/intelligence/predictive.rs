//! Predictive recall -- temporal pattern tracking and proactive memory surfacing.
//!
//! Surfaces memories BEFORE they are asked for based on time of day,
//! day of week, project context, and activity patterns.

use crate::db::Database;
use crate::intelligence::types::{PredictedProject, PredictiveContext, ProactiveMemory};
use crate::Result;
use tracing::info;

/// Track that a memory category was accessed at this time.
/// Builds the temporal pattern database over time.
pub async fn track_temporal_access(
    db: &Database,
    user_id: i64,
    category: &str,
    project_id: Option<i64>,
) -> Result<()> {
    let conn = db.connection();
    let now = chrono::Utc::now();
    let dow = now.format("%w").to_string().parse::<i32>().unwrap_or(0); // 0=Sun, 6=Sat
    let hour = now.format("%H").to_string().parse::<i32>().unwrap_or(0);

    conn.execute(
        "INSERT INTO temporal_patterns (pattern_type, description, memory_ids, confidence, user_id, created_at) \
         VALUES ('access', ?1, ?2, 1.0, ?3, datetime('now'))",
        libsql::params![
            format!("dow:{},hour:{},category:{}", dow, hour, category),
            project_id.map(|p| p.to_string()).unwrap_or_default(),
            user_id
        ],
    )
    .await?;

    Ok(())
}

/// Generate proactive context for the current moment.
/// Called at session start or periodically.
pub async fn predictive_recall(db: &Database, user_id: i64) -> Result<PredictiveContext> {
    let conn = db.connection();
    let now = chrono::Utc::now();
    let dow = now.format("%w").to_string().parse::<i32>().unwrap_or(0);
    let hour = now.format("%H").to_string().parse::<i32>().unwrap_or(0);

    // Time context string
    let day_names = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let day_name = day_names.get(dow as usize).unwrap_or(&"Unknown");
    let time_period = if hour < 12 {
        "morning"
    } else if hour < 17 {
        "afternoon"
    } else {
        "evening"
    };
    let time_context = format!("{} {}", day_name, time_period);

    // Get temporal patterns for this time slot
    let pattern_query = format!("%dow:{},hour:{}%", dow, hour);
    let mut pattern_rows = conn
        .query(
            "SELECT description FROM temporal_patterns \
             WHERE user_id = ?1 AND description LIKE ?2 \
             ORDER BY created_at DESC LIMIT 20",
            libsql::params![user_id, pattern_query],
        )
        .await?;

    let mut predicted_categories: Vec<String> = Vec::new();
    while let Some(row) = pattern_rows.next().await? {
        let desc: String = row.get(0)?;
        // Parse category from "dow:X,hour:Y,category:Z"
        if let Some(cat_start) = desc.find("category:") {
            let cat = &desc[cat_start + 9..];
            if !predicted_categories.contains(&cat.to_string()) {
                predicted_categories.push(cat.to_string());
            }
        }
    }

    // Get unfinished tasks (recent, not completed)
    let mut task_rows = conn
        .query(
            "SELECT id, content, category, importance \
             FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
               AND category = 'task' AND is_static = 0 \
               AND created_at > datetime('now', '-3 days') \
             ORDER BY importance DESC, created_at DESC LIMIT 5",
            libsql::params![user_id],
        )
        .await?;

    let mut proactive_memories: Vec<ProactiveMemory> = Vec::new();
    let mut suggested_actions: Vec<String> = Vec::new();

    while let Some(row) = task_rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let category: String = row.get(2)?;
        let importance: i32 = row.get(3)?;

        if suggested_actions.is_empty() {
            let truncated = if content.len() > 80 {
                &content[..80]
            } else {
                &content
            };
            suggested_actions.push(format!("Continue: {}", truncated));
        }

        proactive_memories.push(ProactiveMemory {
            id,
            content,
            category,
            importance,
            reason: "unfinished_task".to_string(),
            score: importance as f64 / 10.0 + 0.3,
        });
    }

    // Get active issues
    let mut issue_rows = conn
        .query(
            "SELECT id, content, category, importance \
             FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
               AND category = 'issue' \
               AND created_at > datetime('now', '-7 days') \
             ORDER BY importance DESC LIMIT 3",
            libsql::params![user_id],
        )
        .await?;

    while let Some(row) = issue_rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let category: String = row.get(2)?;
        let importance: i32 = row.get(3)?;

        if suggested_actions.len() < 3 {
            let truncated = if content.len() > 80 {
                &content[..80]
            } else {
                &content
            };
            suggested_actions.push(format!("Address issue: {}", truncated));
        }

        proactive_memories.push(ProactiveMemory {
            id,
            content,
            category,
            importance,
            reason: "active_issue".to_string(),
            score: importance as f64 / 10.0 + 0.2,
        });
    }

    // Get recent memories for session continuity
    let mut recent_rows = conn
        .query(
            "SELECT id, content, category, importance \
             FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
             ORDER BY created_at DESC LIMIT 3",
            libsql::params![user_id],
        )
        .await?;

    while let Some(row) = recent_rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let category: String = row.get(2)?;
        let importance: i32 = row.get(3)?;

        // Avoid duplicates
        if !proactive_memories.iter().any(|m| m.id == id) {
            proactive_memories.push(ProactiveMemory {
                id,
                content,
                category,
                importance,
                reason: "session_continuity".to_string(),
                score: importance as f64 / 10.0,
            });
        }
    }

    // Sort by composite score
    proactive_memories.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    proactive_memories.truncate(10);

    // Try to predict project from recent activity
    let predicted_project = predict_project(db, user_id).await?;
    if let Some(ref proj) = predicted_project {
        suggested_actions.push(format!("Expected project: {}", proj.name));
    }

    info!(
        time_context = %time_context,
        proactive = proactive_memories.len(),
        predicted_categories = predicted_categories.len(),
        "predictive_recall"
    );

    Ok(PredictiveContext {
        time_context,
        predicted_categories,
        predicted_project,
        proactive_memories,
        suggested_actions,
    })
}

/// Predict which project the user is likely working on based on recent memory-project links.
async fn predict_project(db: &Database, user_id: i64) -> Result<Option<PredictedProject>> {
    let conn = db.connection();

    let mut rows = conn
        .query(
            "SELECT p.id, p.name, COUNT(*) as cnt \
             FROM memory_projects mp \
             JOIN projects p ON p.id = mp.project_id \
             JOIN memories m ON m.id = mp.memory_id \
             WHERE p.user_id = ?1 AND m.created_at > datetime('now', '-24 hours') \
             GROUP BY p.id, p.name \
             ORDER BY cnt DESC \
             LIMIT 1",
            libsql::params![user_id],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(Some(PredictedProject {
            id: row.get(0)?,
            name: row.get(1)?,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_time_context_format() {
        let day_names = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        let dow = 3; // Wednesday
        let hour = 14;
        let day_name = day_names[dow];
        let time_period = if hour < 12 {
            "morning"
        } else if hour < 17 {
            "afternoon"
        } else {
            "evening"
        };
        let context = format!("{} {}", day_name, time_period);
        assert_eq!(context, "Wednesday afternoon");
    }
}
