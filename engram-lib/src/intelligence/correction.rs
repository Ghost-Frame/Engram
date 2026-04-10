//! Memory correction -- replace a memory with corrected content while preserving lineage.

use crate::db::Database;
use crate::memory;
use crate::memory::types::{Memory, StoreRequest};
use crate::Result;

/// Correct a memory: create a new version with corrected content, link it to
/// the original via 'supersedes', and mark the original as superseded.
pub async fn correct_memory(
    db: &Database,
    user_id: i64,
    memory_id: i64,
    corrected_content: &str,
    reason: Option<&str>,
) -> Result<Memory> {
    // Fetch the original (validates ownership)
    let original = memory::get(db, memory_id, user_id).await?;

    // Store the corrected version with same metadata
    let store_result = memory::store(
        db,
        StoreRequest {
            content: corrected_content.to_string(),
            category: original.category.clone(),
            source: original.source.clone(),
            importance: original.importance,
            tags: original
                .tags
                .as_ref()
                .and_then(|t| serde_json::from_str(t).ok()),
            embedding: None,
            session_id: original.session_id.clone(),
            is_static: Some(original.is_static),
            user_id: Some(user_id),
            space_id: original.space_id,
            parent_memory_id: None,
        },
    )
    .await?;

    let new_id = store_result.id;

    // Create 'supersedes' link from new to old
    memory::insert_link(db, new_id, memory_id, 1.0, "supersedes", user_id).await?;

    // Mark original as superseded
    db.conn
        .execute(
            "UPDATE memories SET is_superseded = 1, updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![memory_id, user_id],
        )
        .await?;

    // Record the correction in reconsolidations table
    let reason_text = reason.unwrap_or("manual correction");
    db.conn
        .execute(
            "INSERT INTO reconsolidations \
             (memory_id, old_content, new_content, reason, user_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            libsql::params![
                memory_id,
                original.content,
                corrected_content.to_string(),
                reason_text.to_string(),
                user_id
            ],
        )
        .await?;

    // Return the new memory
    memory::get(db, new_id, user_id).await
}
