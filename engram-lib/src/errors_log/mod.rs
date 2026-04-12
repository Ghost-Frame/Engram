//! Centralised error event log stored in the database.

use crate::{db::Database, EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub id: i64,
    pub source: String,
    pub level: String,
    pub message: String,
    pub context: Option<String>,
    pub created_at: String,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogErrorRequest {
    pub source: String,
    pub level: String,
    pub message: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListErrorsRequest {
    pub level: Option<String>,
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Log an error event to the database. Returns the new row id.
pub async fn log_error(db: &Database, req: LogErrorRequest, user_id: Option<&str>) -> Result<i64> {
    let mut rows = db
        .conn
        .query(
            "INSERT INTO error_events (source, level, message, context, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
            libsql::params![
                req.source,
                req.level,
                req.message,
                req.context,
                user_id.map(|s| s.to_string())
            ],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no id returned from error_events insert".into()))?;
    Ok(row.get(0)?)
}

/// List error events with optional level/source filters.
pub async fn list_errors(db: &Database, req: ListErrorsRequest) -> Result<Vec<ErrorEvent>> {
    let limit = req.limit.unwrap_or(50).clamp(1, 500);
    let offset = req.offset.unwrap_or(0).max(0);
    let level = req.level;
    let source = req.source;

    // Use IS NULL trick: if the bound param is NULL, the filter is skipped.
    let mut rows = db
        .conn
        .query(
            "SELECT id, source, level, message, context, created_at, user_id \
             FROM error_events \
             WHERE (?1 IS NULL OR level = ?1) \
               AND (?2 IS NULL OR source = ?2) \
             ORDER BY created_at DESC \
             LIMIT ?3 OFFSET ?4",
            libsql::params![level, source, limit, offset],
        )
        .await?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await? {
        events.push(ErrorEvent {
            id: row.get(0)?,
            source: row.get(1)?,
            level: row.get(2)?,
            message: row.get(3)?,
            context: row.get(4).unwrap_or(None),
            created_at: row.get(5).unwrap_or_default(),
            user_id: row.get(6).unwrap_or(None),
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_log_and_list_errors() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let id = log_error(
            &db,
            LogErrorRequest {
                source: "test-agent".to_string(),
                level: "error".to_string(),
                message: "something went wrong".to_string(),
                context: Some(r#"{"detail":"oops"}"#.to_string()),
            },
            Some("user-1"),
        )
        .await
        .expect("log_error");
        assert!(id > 0);

        let events = list_errors(&db, ListErrorsRequest::default())
            .await
            .expect("list_errors");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, "test-agent");
        assert_eq!(events[0].level, "error");
    }

    #[tokio::test]
    async fn test_list_errors_with_level_filter() {
        let db = Database::connect_memory().await.expect("in-memory db");

        for level in ["error", "warn", "error"] {
            log_error(
                &db,
                LogErrorRequest {
                    source: "svc".to_string(),
                    level: level.to_string(),
                    message: "msg".to_string(),
                    context: None,
                },
                None,
            )
            .await
            .expect("log_error");
        }

        let errors = list_errors(
            &db,
            ListErrorsRequest {
                level: Some("error".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("list filtered");
        assert_eq!(errors.len(), 2, "should return only error-level events");
    }
}
