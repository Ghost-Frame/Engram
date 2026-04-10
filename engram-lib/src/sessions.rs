use crate::db::Database;
use crate::Result;
use libsql::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub agent: String,
    pub user_id: i64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    pub agent: String,
}

pub async fn create_session(db: &Database, req: &SessionCreateRequest, user_id: i64) -> Result<SessionInfo> {
    let id = Uuid::new_v4().to_string();
    db.conn.execute(
        "INSERT INTO sessions (id, agent, user_id) VALUES (?1, ?2, ?3)",
        params![id.clone(), req.agent.clone(), user_id],
    ).await?;

    // Fetch back the row so timestamps come from the DB (not local clock)
    get_session(db, &id, user_id).await
}

pub async fn get_session(db: &Database, session_id: &str, user_id: i64) -> Result<SessionInfo> {
    let mut rows = db.conn.query(
        "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions WHERE id = ?1 AND user_id = ?2",
        params![session_id.to_string(), user_id],
    ).await?;
    rows.next().await?
        .map(|row| row_to_session(&row))
        .transpose()?
        .ok_or_else(|| crate::EngError::NotFound(format!("session {} not found", session_id)))
}

pub async fn list_sessions(db: &Database, user_id: i64) -> Result<Vec<SessionInfo>> {
    let mut rows = db.conn.query(
        "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 100",
        params![user_id],
    ).await?;

    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await? {
        sessions.push(row_to_session(&row)?);
    }
    Ok(sessions)
}

pub async fn append_output(db: &Database, session_id: &str, line: &str, user_id: i64) -> Result<()> {
    // Verify session exists and belongs to user
    let mut rows = db.conn.query(
        "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
        params![session_id.to_string(), user_id],
    ).await?;
    if rows.next().await?.is_none() {
        return Err(crate::EngError::NotFound(format!("session {} not found", session_id)));
    }

    db.conn.execute(
        "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
        params![session_id.to_string(), line.to_string()],
    ).await?;

    // Update session updated_at
    db.conn.execute(
        "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
        params![session_id.to_string()],
    ).await?;

    Ok(())
}

pub async fn get_session_output(db: &Database, session_id: &str, user_id: i64) -> Result<Vec<String>> {
    // Verify ownership
    let mut rows = db.conn.query(
        "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
        params![session_id.to_string(), user_id],
    ).await?;
    if rows.next().await?.is_none() {
        return Err(crate::EngError::NotFound(format!("session {} not found", session_id)));
    }

    let mut lines_rows = db.conn.query(
        "SELECT line FROM session_output WHERE session_id = ?1 ORDER BY id ASC LIMIT 10000",
        params![session_id.to_string()],
    ).await?;

    let mut lines = Vec::new();
    while let Some(row) = lines_rows.next().await? {
        lines.push(row.get::<String>(0)?);
    }
    Ok(lines)
}

fn row_to_session(row: &libsql::Row) -> Result<SessionInfo> {
    Ok(SessionInfo {
        id: row.get(0)?,
        agent: row.get(1)?,
        user_id: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_session() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest { agent: "claude-code".to_string() };
        let session = create_session(&db, &req, 1).await.expect("create");
        assert!(!session.id.is_empty());
        assert_eq!(session.agent, "claude-code");

        let fetched = get_session(&db, &session.id, 1).await.expect("get");
        assert_eq!(fetched.id, session.id);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest { agent: "test-agent".to_string() };
        create_session(&db, &req, 1).await.expect("create");

        let sessions = list_sessions(&db, 1).await.expect("list");
        assert!(!sessions.is_empty());
    }

    #[tokio::test]
    async fn test_append_and_get_output() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest { agent: "test-agent".to_string() };
        let session = create_session(&db, &req, 1).await.expect("create");

        append_output(&db, &session.id, "line 1", 1).await.expect("append");
        append_output(&db, &session.id, "line 2", 1).await.expect("append");

        let output = get_session_output(&db, &session.id, 1).await.expect("get output");
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], "line 1");
    }

    #[tokio::test]
    async fn test_append_to_nonexistent_session() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let result = append_output(&db, "nonexistent-id", "line", 1).await;
        assert!(result.is_err());
    }
}
