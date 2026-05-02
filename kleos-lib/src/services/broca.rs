use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    pub id: i64,
    pub agent: String,
    pub service: String,
    pub action: String,
    pub payload: serde_json::Value,
    pub narrative: Option<String>,
    pub axon_event_id: Option<i64>,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogActionRequest {
    pub agent: String,
    #[serde(default)]
    pub service: Option<String>,
    pub action: String,
    #[serde(default)]
    pub narrative: Option<String>,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    #[serde(default)]
    pub axon_event_id: Option<i64>,
    #[serde(default)]
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrocaStats {
    pub total_actions: i64,
    pub agents: i64,
    pub services: i64,
}

const ACTION_COLUMNS: &str =
    "id, agent, service, action, payload, narrative, axon_event_id, user_id, created_at";

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

fn row_to_action_entry(row: &rusqlite::Row<'_>) -> Result<ActionEntry> {
    let payload_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    Ok(ActionEntry {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        service: row.get(2).map_err(rusqlite_to_eng_error)?,
        action: row.get(3).map_err(rusqlite_to_eng_error)?,
        payload,
        narrative: row.get(5).map_err(rusqlite_to_eng_error)?,
        axon_event_id: row.get(6).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(7).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
    })
}

#[tracing::instrument(skip(db, req), fields(agent = %req.agent, action = %req.action, service = ?req.service, user_id = ?req.user_id))]
pub async fn log_action(db: &Database, req: LogActionRequest) -> Result<ActionEntry> {
    let service = req.service.clone().unwrap_or_else(|| "engram".to_string());
    let payload = req
        .payload
        .clone()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;

    let agent = req.agent.clone();
    let action = req.action.clone();
    let narrative = req.narrative.clone();
    let axon_event_id = req.axon_event_id;
    let svc = service.clone();
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO broca_actions
                    (agent, service, action, payload, narrative, axon_event_id, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    agent,
                    svc,
                    action,
                    payload_str,
                    narrative,
                    axon_event_id,
                    user_id,
                ],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    get_action(db, id, user_id).await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db), fields(agent = ?agent, service = ?service, action = ?action, limit, offset, user_id))]
pub async fn query_actions(
    db: &Database,
    agent: Option<&str>,
    service: Option<&str>,
    action: Option<&str>,
    limit: usize,
    offset: usize,
    user_id: i64,
) -> Result<Vec<ActionEntry>> {
    // H-R3-006: previously this took _user_id and ignored it; broca_actions
    // had no user_id column, so any caller could read any tenant's rows.
    // After tenant migration v42 + monolith migration v45 the column exists
    // and we filter on it.
    let mut sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE user_id = ?1");
    let mut params_vec: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Integer(user_id)];
    let mut param_idx = 2usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = service {
        sql.push_str(&format!(" AND service = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        param_idx += 1;
    }
    if let Some(act) = action {
        sql.push_str(&format!(" AND action = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(act.to_string()));
        param_idx += 1;
    }
    sql.push_str(&format!(
        " ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(rusqlite::types::Value::Integer(limit as i64));
    params_vec.push(rusqlite::types::Value::Integer(offset as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let params = rusqlite::params_from_iter(params_vec.iter().cloned());
        let mut rows = stmt.query(params).map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_action_entry(row)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db), fields(action_id = id, user_id))]
pub async fn get_action(db: &Database, id: i64, user_id: i64) -> Result<ActionEntry> {
    let sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE id = ?1 AND user_id = ?2");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;
        row_to_action_entry(row)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_stats(db: &Database, user_id: i64) -> Result<BrocaStats> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT agent), COUNT(DISTINCT service)
             FROM broca_actions WHERE user_id = ?1",
            rusqlite::params![user_id],
            |row| {
                Ok(BrocaStats {
                    total_actions: row.get(0)?,
                    agents: row.get(1)?,
                    services: row.get(2)?,
                })
            },
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    async fn setup() -> Database {
        let db = Database::connect_memory().await.expect("db");
        // Apply monolith migrations so broca_actions exists with user_id (v45).
        db.write(|conn| crate::db::migrations::run_migrations(conn))
            .await
            .expect("migrations");
        db
    }

    #[tokio::test]
    async fn log_and_get_action() {
        let db = setup().await;
        let entry = log_action(
            &db,
            LogActionRequest {
                agent: "claude-code".into(),
                service: Some("engram".into()),
                action: "task.started".into(),
                narrative: Some("starting a port".into()),
                payload: Some(serde_json::json!({"project": "engram-rust"})),
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .expect("log");
        assert_eq!(entry.service, "engram");
        assert_eq!(entry.action, "task.started");
        assert_eq!(entry.user_id, 1);
        let fetched = get_action(&db, entry.id, 1).await.unwrap();
        assert_eq!(fetched.id, entry.id);
    }

    /// H-R3-006 regression test (un-ignored from Phase 5.6). After v45
    /// re-added user_id, query_actions filters by user_id again so a row
    /// owned by user 1 must not surface to a query scoped to user 2.
    #[tokio::test]
    async fn query_is_scoped_by_user() {
        let db = setup().await;
        log_action(
            &db,
            LogActionRequest {
                agent: "a".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let other = query_actions(&db, None, None, None, 10, 0, 2)
            .await
            .unwrap();
        assert!(other.is_empty(), "user 2 must not see user 1's actions");
        let mine = query_actions(&db, None, None, None, 10, 0, 1)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "user 1 should see their own row");
        assert_eq!(mine[0].user_id, 1);
    }

    #[tokio::test]
    async fn get_stats_is_scoped_by_user() {
        let db = setup().await;
        log_action(
            &db,
            LogActionRequest {
                agent: "alice".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        log_action(
            &db,
            LogActionRequest {
                agent: "bob".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(2),
            },
        )
        .await
        .unwrap();
        let s1 = get_stats(&db, 1).await.unwrap();
        let s2 = get_stats(&db, 2).await.unwrap();
        assert_eq!(s1.total_actions, 1);
        assert_eq!(s2.total_actions, 1);
    }
}
