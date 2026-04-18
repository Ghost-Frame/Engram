//! Dead-letter store for service calls that exhausted all retry attempts.
//!
//! Failed service calls are persisted to `service_dead_letters` so operators
//! can inspect and replay them. The table is created by migration 22.
//!
//! Pattern mirrors `webhook_dead_letters` from `kleos_lib::webhooks`.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A dead-letter entry for a service call that failed after all retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDeadLetter {
    pub id: i64,
    /// Name of the service (e.g. "reranker", "embedder", "brain").
    pub service: String,
    /// Operation name within that service (e.g. "rerank", "embed").
    pub operation: String,
    /// JSON-serialized request payload, if available.
    pub payload_json: Option<String>,
    /// Human-readable error string from the last failure.
    pub error: Option<String>,
    /// Total number of attempts made before giving up.
    pub retry_count: i64,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

fn rusqlite_to_eng(e: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(e.to_string())
}

/// Record a dead-letter entry for a service call that exhausted all retries.
///
/// `payload` is serialised to JSON. Pass `serde_json::Value::Null` if there
/// is no meaningful payload (e.g. for read-only queries).
#[tracing::instrument(skip(db, payload), fields(service, operation))]
pub async fn record_dead_letter(
    db: &Database,
    service: &str,
    operation: &str,
    payload: serde_json::Value,
    error: &str,
    retry_count: u32,
) -> Result<()> {
    let svc = service.to_string();
    let op = operation.to_string();
    let payload_s = serde_json::to_string(&payload).ok();
    let err_s = error.to_string();
    let retries = retry_count as i64;

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO service_dead_letters \
             (service, operation, payload_json, error, retry_count) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![svc, op, payload_s, err_s, retries],
        )
        .map_err(rusqlite_to_eng)?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Read (for operators / admin UI)
// ---------------------------------------------------------------------------

/// List dead-letter entries, most recent first.
///
/// `service` -- if `Some`, filters to a specific service name.
/// `limit` -- maximum number of rows to return.
#[tracing::instrument(skip(db))]
pub async fn list_dead_letters(
    db: &Database,
    service: Option<&str>,
    limit: i64,
) -> Result<Vec<ServiceDeadLetter>> {
    let svc = service.map(|s| s.to_string());

    db.read(move |conn| {
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if svc.is_some() {
            (
                "SELECT id, service, operation, payload_json, error, retry_count, created_at \
                 FROM service_dead_letters \
                 WHERE service = ?1 \
                 ORDER BY created_at DESC LIMIT ?2",
                vec![
                    Box::new(svc.clone().unwrap()),
                    Box::new(limit),
                ],
            )
        } else {
            (
                "SELECT id, service, operation, payload_json, error, retry_count, created_at \
                 FROM service_dead_letters \
                 ORDER BY created_at DESC LIMIT ?1",
                vec![Box::new(limit)],
            )
        };

        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec.iter().map(|b| b.as_ref())))
            .map_err(rusqlite_to_eng)?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng)? {
            result.push(ServiceDeadLetter {
                id: row.get(0).map_err(rusqlite_to_eng)?,
                service: row.get(1).map_err(rusqlite_to_eng)?,
                operation: row.get(2).map_err(rusqlite_to_eng)?,
                payload_json: row.get(3).unwrap_or(None),
                error: row.get(4).unwrap_or(None),
                retry_count: row.get(5).map_err(rusqlite_to_eng)?,
                created_at: row.get(6).map_err(rusqlite_to_eng)?,
            });
        }
        Ok(result)
    })
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_dead_letter() {
        let db = Database::connect_memory().await.unwrap();

        record_dead_letter(
            &db,
            "reranker",
            "rerank",
            serde_json::json!({"query": "hello"}),
            "HTTP 503",
            3,
        )
        .await
        .unwrap();

        let letters = list_dead_letters(&db, Some("reranker"), 50).await.unwrap();
        assert_eq!(letters.len(), 1);
        assert_eq!(letters[0].service, "reranker");
        assert_eq!(letters[0].operation, "rerank");
        assert_eq!(letters[0].retry_count, 3);
        assert_eq!(letters[0].error.as_deref(), Some("HTTP 503"));
    }

    #[tokio::test]
    async fn list_all_services() {
        let db = Database::connect_memory().await.unwrap();

        record_dead_letter(&db, "embedder", "embed", serde_json::Value::Null, "timeout", 3)
            .await
            .unwrap();
        record_dead_letter(&db, "reranker", "rerank", serde_json::Value::Null, "503", 3)
            .await
            .unwrap();

        let all = list_dead_letters(&db, None, 50).await.unwrap();
        assert_eq!(all.len(), 2);

        let embedder_only = list_dead_letters(&db, Some("embedder"), 50).await.unwrap();
        assert_eq!(embedder_only.len(), 1);
        assert_eq!(embedder_only[0].service, "embedder");
    }
}
