//! Centralised error event log stored in the database.

pub mod types;
pub use types::*;

use crate::{db::Database, EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Log an error event to the database. Returns the new row id.
#[tracing::instrument(skip(db, req), fields(source = %req.source, level = %req.level, message_len = req.message.len(), user_id = ?user_id))]
pub async fn log_error(db: &Database, req: LogErrorRequest, user_id: Option<&str>) -> Result<i64> {
    let user_id_owned = user_id.map(|s| s.to_string());

    db.write(move |conn| {
        let id: i64 = conn
            .query_row(
                "INSERT INTO error_events (source, level, message, context, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
                rusqlite::params![
                    req.source,
                    req.level,
                    req.message,
                    req.context,
                    user_id_owned
                ],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(id)
    })
    .await
}

/// List error events scoped to `user_id`. Optional level/source filters
/// further narrow the result.
///
/// SECURITY: callers must always pass the calling user's id; events for
/// other users must never be returned. The route handler at
/// `kleos-server/src/routes/errors/mod.rs` is responsible for binding
/// `user_id` from `auth.user_id`.
#[tracing::instrument(skip(db, req), fields(user_id = %user_id, level = ?req.level, source = ?req.source, limit = ?req.limit, offset = ?req.offset))]
pub async fn list_errors(
    db: &Database,
    user_id: &str,
    req: ListErrorsRequest,
) -> Result<Vec<ErrorEvent>> {
    let limit = req.limit.unwrap_or(50).clamp(1, 500);
    let offset = req.offset.unwrap_or(0).max(0);
    let level = req.level;
    let source = req.source;
    let user_id_owned = user_id.to_string();

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, source, level, message, context, created_at, user_id \
                 FROM error_events \
                 WHERE user_id = ?1 \
                   AND (?2 IS NULL OR level = ?2) \
                   AND (?3 IS NULL OR source = ?3) \
                 ORDER BY created_at DESC \
                 LIMIT ?4 OFFSET ?5",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![
                user_id_owned,
                level,
                source,
                limit,
                offset
            ])
            .map_err(rusqlite_to_eng_error)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            events.push(ErrorEvent {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                source: row.get(1).map_err(rusqlite_to_eng_error)?,
                level: row.get(2).map_err(rusqlite_to_eng_error)?,
                message: row.get(3).map_err(rusqlite_to_eng_error)?,
                context: row.get(4).unwrap_or(None),
                created_at: row.get(5).unwrap_or_default(),
                user_id: row.get(6).unwrap_or(None),
            });
        }
        Ok(events)
    })
    .await
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

        let events = list_errors(&db, "user-1", ListErrorsRequest::default())
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
                Some("alice"),
            )
            .await
            .expect("log_error");
        }

        let errors = list_errors(
            &db,
            "alice",
            ListErrorsRequest {
                level: Some("error".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("list filtered");
        assert_eq!(errors.len(), 2, "should return only error-level events");
    }

    #[tokio::test]
    async fn list_errors_scopes_to_user() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let req = |source: &str| LogErrorRequest {
            source: source.to_string(),
            level: "error".to_string(),
            message: "x".to_string(),
            context: None,
        };

        log_error(&db, req("alice-svc"), Some("alice"))
            .await
            .expect("a");
        log_error(&db, req("alice-svc-2"), Some("alice"))
            .await
            .expect("a2");
        log_error(&db, req("bob-svc"), Some("bob"))
            .await
            .expect("b");

        let alice = list_errors(&db, "alice", ListErrorsRequest::default())
            .await
            .expect("alice list");
        assert_eq!(alice.len(), 2, "alice sees only her events");

        let bob = list_errors(&db, "bob", ListErrorsRequest::default())
            .await
            .expect("bob list");
        assert_eq!(bob.len(), 1);

        let stranger = list_errors(&db, "carol", ListErrorsRequest::default())
            .await
            .expect("stranger list");
        assert!(stranger.is_empty(), "unrelated user sees no events");
    }
}
