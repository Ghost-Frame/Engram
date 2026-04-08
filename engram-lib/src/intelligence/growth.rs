//! Growth reflection -- observe patterns, record learnings, materialize to actionable memories.

use crate::db::Database;
use crate::Result;
use crate::intelligence::types::{GrowthReflectRequest, GrowthReflectResult};
use libsql::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub content: String,
    pub source: String,
    pub created_at: String,
}

/// Generate a rule-based observation text from activity context.
/// (No LLM required -- pattern-based summarization.)
pub fn generate_observation_text(service: &str, context: &[String]) -> String {
    if context.is_empty() {
        return format!("{} has no recent activity to reflect on.", service);
    }

    let completed = context.iter().filter(|c| c.contains("task.completed")).count();
    let errors = context.iter().filter(|c| c.contains("error.raised")).count();
    let blocked = context.iter().filter(|c| c.contains("task.blocked")).count();
    let total = context.len();

    let mut parts = Vec::new();
    if completed > 0 {
        parts.push(format!("{} tasks completed", completed));
    }
    if errors > 0 {
        parts.push(format!("{} errors raised", errors));
    }
    if blocked > 0 {
        parts.push(format!("{} tasks blocked", blocked));
    }

    let summary = if parts.is_empty() {
        format!("{} items of activity observed", total)
    } else {
        parts.join(", ")
    };

    format!(
        "Growth reflection for {}: {}. Recent context: {}",
        service,
        summary,
        context.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
    )
}

/// Reflect on recent activity and store an observation as a growth memory.
pub async fn reflect(db: &Database, request: &GrowthReflectRequest, user_id: i64) -> Result<GrowthReflectResult> {
    // Note: request.existing_growth and request.prompt_override are reserved for
    // a future LLM-backed reflection path. Currently using rule-based generation.
    let observation_text = generate_observation_text(&request.service, &request.context);

    // Store as a "growth" category memory
    db.conn.execute(
        "INSERT INTO memories (content, category, source, importance, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            observation_text.clone(),
            "growth".to_string(),
            format!("{}-growth", request.service),
            5i64,
            user_id,
        ],
    ).await?;

    let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let memory_id: i64 = rows.next().await?
        .ok_or_else(|| crate::EngError::Internal("no rowid".into()))?
        .get(0)?;

    Ok(GrowthReflectResult {
        observation: Some(observation_text),
        stored_memory_id: Some(memory_id),
        reflection_id: None,
    })
}

/// List growth observations (memories with category = "growth").
pub async fn list_observations(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Observation>> {
    let mut rows = db.conn.query(
        "SELECT id, content, source, created_at
         FROM memories
         WHERE category = 'growth' AND user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
         ORDER BY id DESC LIMIT ?2",
        params![user_id, limit as i64],
    ).await?;

    let mut observations = Vec::new();
    while let Some(row) = rows.next().await? {
        observations.push(Observation {
            id: row.get(0)?,
            content: row.get(1)?,
            source: row.get::<Option<String>>(2)?.unwrap_or_default(),
            created_at: row.get(3)?,
        });
    }
    Ok(observations)
}

/// Materialize an observation: create an actionable memory derived from it.
/// The original growth memory is kept; a new "discovery" memory is created.
pub async fn materialize(db: &Database, observation_id: i64, user_id: i64) -> Result<i64> {
    // Fetch the observation
    let mut rows = db.conn.query(
        "SELECT content FROM memories WHERE id = ?1 AND user_id = ?2 AND category = 'growth'",
        params![observation_id, user_id],
    ).await?;

    let content: String = rows.next().await?
        .ok_or_else(|| crate::EngError::NotFound(format!("observation {} not found", observation_id)))?
        .get(0)?;

    // Create an actionable discovery memory
    let actionable = format!("Actionable: {}", content);
    db.conn.execute(
        "INSERT INTO memories (content, category, source, importance, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            actionable,
            "discovery".to_string(),
            "growth-materialization".to_string(),
            6i64,
            user_id,
        ],
    ).await?;

    let mut id_rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let new_id: i64 = id_rows.next().await?
        .ok_or_else(|| crate::EngError::Internal("no rowid".into()))?
        .get(0)?;

    Ok(new_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_observation_from_context() {
        let context = vec![
            "task.completed: Fixed auth bug".to_string(),
            "task.completed: Added rate limiting".to_string(),
            "error.raised: Build failed".to_string(),
        ];
        let obs = generate_observation_text("test-agent", &context);
        assert!(!obs.is_empty());
        assert!(obs.len() > 10);
    }

    #[test]
    fn test_generate_observation_empty_context() {
        let obs = generate_observation_text("test-agent", &[]);
        assert!(!obs.is_empty()); // still generates something
    }

    #[tokio::test]
    async fn test_reflect_stores_growth_memory() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("in-memory db");

        let request = GrowthReflectRequest {
            service: "test-agent".to_string(),
            context: vec!["task.completed: Did some work".to_string()],
            existing_growth: None,
            prompt_override: None,
        };
        let result = reflect(&db, &request, 1).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.observation.is_some());
        assert!(r.stored_memory_id.is_some());
    }

    #[tokio::test]
    async fn test_list_observations_returns_growth_memories() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("in-memory db");

        // Store a growth memory directly
        db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params!["Test growth observation".to_string(), "growth".to_string(), "test".to_string(), 5i64, 1i64],
        ).await.expect("insert");

        let obs = list_observations(&db, 1, 10).await.expect("list");
        assert!(!obs.is_empty());
    }

    #[tokio::test]
    async fn test_materialize_observation() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("in-memory db");

        // Insert a growth memory
        db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params!["Growth obs to materialize".to_string(), "growth".to_string(), "test".to_string(), 5i64, 1i64],
        ).await.expect("insert");
        let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await.expect("rowid");
        let id: i64 = rows.next().await.expect("row").expect("exists").get(0).expect("id");

        let result = materialize(&db, id, 1).await;
        assert!(result.is_ok());
    }
}
