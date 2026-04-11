//! Sync receive -- apply changes from another engram instance.

use serde::{Deserialize, Serialize};

use crate::db::Database;
#[cfg(feature = "db_pool")]
use crate::memory::uses_pool_backend;
use crate::memory;
use crate::memory::types::StoreRequest;
#[cfg(feature = "db_pool")]
use crate::EngError;
use crate::Result;

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

                #[cfg(feature = "db_pool")]
                if uses_pool_backend(db) {
                    let sync_id = change.sync_id.clone();
                    let exists = db
                        .read(move |conn| {
                            let mut stmt = conn
                                .prepare("SELECT id FROM memories WHERE sync_id = ?1 AND user_id = ?2")
                                .map_err(rusqlite_to_eng_error)?;
                            let mut rows = stmt
                                .query(rusqlite::params![sync_id, user_id])
                                .map_err(rusqlite_to_eng_error)?;
                            Ok(rows
                                .next()
                                .map_err(rusqlite_to_eng_error)?
                                .is_some())
                        })
                        .await?;
                    if exists {
                        skipped += 1;
                        continue;
                    }
                } else {
                    let mut existing = db
                        .conn
                        .query(
                            "SELECT id FROM memories WHERE sync_id = ?1 AND user_id = ?2",
                            libsql::params![change.sync_id.clone(), user_id],
                        )
                        .await?;
                    if existing.next().await?.is_some() {
                        skipped += 1;
                        continue;
                    }
                }

                #[cfg(not(feature = "db_pool"))]
                {
                    let mut existing = db
                        .conn
                        .query(
                            "SELECT id FROM memories WHERE sync_id = ?1 AND user_id = ?2",
                            libsql::params![change.sync_id.clone(), user_id],
                        )
                        .await?;
                    if existing.next().await?.is_some() {
                        skipped += 1;
                        continue;
                    }
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
                #[cfg(feature = "db_pool")]
                let affected = if uses_pool_backend(db) {
                    let sync_id = change.sync_id.clone();
                    db.write(move |conn| {
                        conn.execute(
                            "UPDATE memories SET is_forgotten = 1 WHERE sync_id = ?1 AND user_id = ?2",
                            rusqlite::params![sync_id, user_id],
                        )
                        .map_err(rusqlite_to_eng_error)
                    })
                    .await? as u64
                } else {
                    db.conn
                        .execute(
                            "UPDATE memories SET is_forgotten = 1 WHERE sync_id = ?1 AND user_id = ?2",
                            libsql::params![change.sync_id.clone(), user_id],
                        )
                        .await?
                };

                #[cfg(not(feature = "db_pool"))]
                let affected = db
                    .conn
                    .execute(
                        "UPDATE memories SET is_forgotten = 1 WHERE sync_id = ?1 AND user_id = ?2",
                        libsql::params![change.sync_id.clone(), user_id],
                    )
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

#[cfg(feature = "db_pool")]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}
