//! Session learning persistence: `session_learn` records a mid-session
//! discovery; `session_recall` retrieves past learnings by keyword search.
//!
//! `checkpoint` and `rollback` are deliberately NOT ported here because they
//! depend on local git state (`git rev-parse HEAD`, `git checkout`) which the
//! server cannot execute without client-side cooperation. Those operations remain
//! in the agent-forge binary as client-side tools.

use crate::db::Database;
use crate::EngError;
use chrono::Utc;
use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;

/// Persist a mid-session discovery to `forge_session_learns` and return its ID.
///
/// `tags` is serialised as a JSON array. `spec_id` is optional; if supplied it
/// must be the ID of an existing `forge_specs` row (FK enforced by schema).
pub async fn session_learn(
    db: &Database,
    user_id: i64,
    discovery: String,
    context: Option<String>,
    tags: Option<Vec<String>>,
    spec_id: Option<String>,
) -> crate::Result<Value> {
    let id = format!("learn_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();
    let tags_json = tags
        .map(|t| serde_json::to_string(&t))
        .transpose()
        .map_err(EngError::Serialization)?;
    let id_clone = id.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO forge_session_learns
             (id, user_id, created_at, discovery, context, tags, spec_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id_clone, user_id, now, discovery, context, tags_json, spec_id],
        )?;
        Ok(())
    })
    .await?;

    Ok(serde_json::json!({ "id": id, "message": "Learning recorded" }))
}

/// Search `forge_session_learns` for rows whose `discovery` text contains `query`.
///
/// Returns the most-recent matches up to `limit` (default 10), scoped to
/// `user_id`. LIKE pattern is `%<query>%`.
pub async fn session_recall(
    db: &Database,
    user_id: i64,
    query: Option<String>,
    limit: Option<usize>,
) -> crate::Result<Value> {
    let query = query.unwrap_or_default();
    let limit = limit.unwrap_or(10) as i64;
    let pattern = format!("%{query}%");

    let results: Vec<Value> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, discovery, context, tags
                 FROM forge_session_learns
                 WHERE user_id = ?1 AND discovery LIKE ?2
                 ORDER BY created_at DESC
                 LIMIT ?3",
            )?;
            let rows: Vec<Value> = stmt
                .query_map(params![user_id, pattern, limit], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "discovery": row.get::<_, String>(1)?,
                        "context": row.get::<_, Option<String>>(2)?,
                        "tags": row.get::<_, Option<String>>(3)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    Ok(serde_json::json!({ "results": results, "count": results.len() }))
}
