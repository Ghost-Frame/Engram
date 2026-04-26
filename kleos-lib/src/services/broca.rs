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
    "id, agent, service, action, payload, narrative, axon_event_id, created_at";

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
        user_id: 1,
        created_at: row.get(7).map_err(rusqlite_to_eng_error)?,
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
                    (agent, service, action, payload, narrative, axon_event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    agent,
                    svc,
                    action,
                    payload_str,
                    narrative,
                    axon_event_id,
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
    let mut sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions");
    let mut clauses: Vec<String> = Vec::new();
    let mut param_idx = 1usize;
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(a) = agent {
        clauses.push(format!("agent = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = service {
        clauses.push(format!("service = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        param_idx += 1;
    }
    if let Some(act) = action {
        clauses.push(format!("action = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(act.to_string()));
        param_idx += 1;
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
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
    let sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE id = ?1");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;
        row_to_action_entry(row)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_stats(db: &Database) -> Result<BrocaStats> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT agent), COUNT(DISTINCT service)
             FROM broca_actions",
            [],
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
        Database::connect_memory().await.expect("db")
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
        let fetched = get_action(&db, entry.id, 1).await.unwrap();
        assert_eq!(fetched.id, entry.id);
    }

    /// Phase 5.6 dropped user_id from broca_actions: tenant isolation is
    /// at the database level now, so a shared in-memory DB no longer
    /// separates user 1 and user 2. The tenant-aware form of this
    /// invariant lands in kleos-server/tests once Phase 4.2 wires the
    /// tenant-aware test harness.
    #[tokio::test]
    #[ignore]
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
        assert!(other.is_empty());
    }
}
