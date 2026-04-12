pub mod scrub;

use crate::db::Database;
use crate::Result;
use rusqlite::{params, OptionalExtension};
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

pub async fn create_session(
    db: &Database,
    req: &SessionCreateRequest,
    user_id: i64,
) -> Result<SessionInfo> {
    let id = Uuid::new_v4().to_string();
    let agent = req.agent.clone();
    let id_for_insert = id.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO sessions (id, agent, user_id) VALUES (?1, ?2, ?3)",
            params![id_for_insert, agent, user_id],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    // Fetch back the row so timestamps come from the DB (not local clock)
    get_session(db, &id, user_id).await
}

pub async fn get_session(db: &Database, session_id: &str, user_id: i64) -> Result<SessionInfo> {
    let session_id = session_id.to_string();
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions WHERE id = ?1 AND user_id = ?2",
            params![session_id, user_id],
            |row| row_to_session(row),
        )
        .optional()
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?
        .ok_or_else(|| crate::EngError::NotFound("session not found".into()))
    })
    .await
}

/// DOS-L4: enforce per-request pagination -- default 50 rows, max 500.
pub async fn list_sessions(
    db: &Database,
    user_id: i64,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<SessionInfo>> {
    let limit = limit.unwrap_or(50).min(500) as i64;
    let offset = offset.unwrap_or(0) as i64;
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions \
                 WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| row_to_session(row))
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(sessions)
    })
    .await
}

pub async fn append_output(
    db: &Database,
    session_id: &str,
    line: &str,
    user_id: i64,
) -> Result<()> {
    let session_id_owned = session_id.to_string();
    let line_owned = line.to_string();

    // Verify session exists and belongs to user
    let sid_check = session_id_owned.clone();
    let exists = db
        .read(move |conn| {
            let result = conn
                .query_row(
                    "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
                    params![sid_check, user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            Ok(result.is_some())
        })
        .await?;

    if !exists {
        return Err(crate::EngError::NotFound(format!(
            "session {} not found",
            session_id_owned
        )));
    }

    let sid_insert = session_id_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
            params![sid_insert, line_owned],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    // Update session updated_at
    let sid_update = session_id_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![sid_update],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    Ok(())
}

pub async fn get_session_output(
    db: &Database,
    session_id: &str,
    user_id: i64,
) -> Result<Vec<String>> {
    let session_id_owned = session_id.to_string();

    // Verify ownership
    let sid_check = session_id_owned.clone();
    let exists = db
        .read(move |conn| {
            let result = conn
                .query_row(
                    "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
                    params![sid_check, user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            Ok(result.is_some())
        })
        .await?;

    if !exists {
        return Err(crate::EngError::NotFound(format!(
            "session {} not found",
            session_id_owned
        )));
    }

    let sid_query = session_id_owned.clone();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT line FROM session_output WHERE session_id = ?1 ORDER BY id ASC LIMIT 10000",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![sid_query], |row| row.get::<_, String>(0))
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut lines = Vec::new();
        for row in rows {
            lines.push(row.map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(lines)
    })
    .await
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInfo> {
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
        let req = SessionCreateRequest {
            agent: "claude-code".to_string(),
        };
        let session = create_session(&db, &req, 1).await.expect("create");
        assert!(!session.id.is_empty());
        assert_eq!(session.agent, "claude-code");

        let fetched = get_session(&db, &session.id, 1).await.expect("get");
        assert_eq!(fetched.id, session.id);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest {
            agent: "test-agent".to_string(),
        };
        create_session(&db, &req, 1).await.expect("create");

        let sessions = list_sessions(&db, 1, None, None).await.expect("list");
        assert!(!sessions.is_empty());
    }

    #[tokio::test]
    async fn test_append_and_get_output() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest {
            agent: "test-agent".to_string(),
        };
        let session = create_session(&db, &req, 1).await.expect("create");

        append_output(&db, &session.id, "line 1", 1)
            .await
            .expect("append");
        append_output(&db, &session.id, "line 2", 1)
            .await
            .expect("append");

        let output = get_session_output(&db, &session.id, 1)
            .await
            .expect("get output");
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
