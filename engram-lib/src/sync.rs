//! Sync receive -- apply changes from another engram instance.

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::memory;
use crate::memory::types::StoreRequest;
use crate::EngError;
use crate::Result;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Deserialize)]
pub struct SyncReceiveChange {
    pub sync_id: String,
    pub change_type: String,
    pub content: Option<String>,
    pub category: Option<String>,
    pub importance: Option<i32>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SyncReceiveResult {
    pub applied: i64,
    pub skipped: i64,
}

#[tracing::instrument(skip(db, changes), fields(change_count = changes.len()))]
pub async fn receive_sync(
    db: &Database,
    user_id: i64,
    changes: Vec<SyncReceiveChange>,
) -> Result<SyncReceiveResult> {
    let mut applied = 0i64;
    let mut skipped = 0i64;

    for change in &changes {
        match change.change_type.as_str() {
            "insert" | "update" => {
                let content = match change.content.as_deref().filter(|c| !c.trim().is_empty()) {
                    Some(c) => c.to_string(),
                    None => {
                        skipped += 1;
                        continue;
                    }
                };

                let sync_id = change.sync_id.clone();
                let exists = db
                    .read(move |conn| {
                        let mut stmt = conn
                            .prepare("SELECT id FROM memories WHERE sync_id = ?1 AND user_id = ?2")
                            .map_err(rusqlite_to_eng_error)?;
                        let mut rows = stmt
                            .query(rusqlite::params![sync_id, user_id])
                            .map_err(rusqlite_to_eng_error)?;
                        Ok(rows.next().map_err(rusqlite_to_eng_error)?.is_some())
                    })
                    .await?;
                if exists {
                    skipped += 1;
                    continue;
                }

                let req = StoreRequest {
                    content,
                    category: change
                        .category
                        .clone()
                        .unwrap_or_else(|| "general".to_string()),
                    source: "sync".to_string(),
                    importance: change.importance.unwrap_or(5),
                    tags: None,
                    embedding: None,
                    session_id: None,
                    is_static: None,
                    user_id: Some(user_id),
                    space_id: None,
                    parent_memory_id: None,
                };
                memory::store(db, req).await?;
                applied += 1;
            }
            "delete" => {
                let sync_id = change.sync_id.clone();
                let affected = db
                    .write(move |conn| {
                        conn.execute(
                            "UPDATE memories SET is_forgotten = 1 WHERE sync_id = ?1 AND user_id = ?2",
                            rusqlite::params![sync_id, user_id],
                        )
                        .map_err(rusqlite_to_eng_error)
                    })
                    .await?;
                if affected > 0 {
                    applied += 1;
                } else {
                    skipped += 1;
                }
            }
            _ => {
                skipped += 1;
            }
        }
    }

    Ok(SyncReceiveResult { applied, skipped })
}
