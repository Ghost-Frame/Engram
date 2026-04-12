use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

/// A single pattern stored in the Hopfield substrate. Each pattern
/// corresponds to a memory embedding projected into the brain's vector
/// space. `strength` tracks how "alive" the pattern is (0.0 = dead,
/// 1.0 = fully consolidated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainPattern {
    pub id: i64,
    pub user_id: i64,
    pub pattern: Vec<f32>,
    pub strength: f32,
    pub importance: i32,
    pub access_count: i32,
    pub last_activated_at: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Blob serialization (little-endian f32)
// ---------------------------------------------------------------------------

pub fn pattern_to_blob(pattern: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(pattern.len() * 4);
    for &f in pattern {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}

pub fn blob_to_pattern(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// ---------------------------------------------------------------------------
// Database CRUD
// ---------------------------------------------------------------------------

/// Insert or replace a pattern row. Uses INSERT OR REPLACE so calling
/// with an existing id updates in place.
pub async fn store_pattern(db: &Database, pattern: &BrainPattern) -> Result<()> {
    let blob = pattern_to_blob(&pattern.pattern);
    db.conn
        .execute(
            "INSERT OR REPLACE INTO brain_patterns \
             (id, user_id, pattern, strength, importance, access_count, \
              last_activated_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                pattern.id,
                pattern.user_id,
                blob,
                pattern.strength as f64,
                pattern.importance,
                pattern.access_count,
                pattern.last_activated_at.clone(),
                pattern.created_at.clone()
            ],
        )
        .await?;
    Ok(())
}

/// Load a single pattern by id and user_id.
pub async fn get_pattern(db: &Database, id: i64, user_id: i64) -> Result<BrainPattern> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, pattern, strength, importance, access_count, \
                    last_activated_at, created_at \
             FROM brain_patterns WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("brain pattern {}", id)))?;

    row_to_pattern(&row)
}

/// Load all patterns for a user. Used to populate the in-memory network
/// at startup.
pub async fn list_patterns(db: &Database, user_id: i64) -> Result<Vec<BrainPattern>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, pattern, strength, importance, access_count, \
                    last_activated_at, created_at \
             FROM brain_patterns WHERE user_id = ?1 \
             ORDER BY id",
            libsql::params![user_id],
        )
        .await?;

    let mut patterns = Vec::new();
    while let Some(row) = rows.next().await? {
        patterns.push(row_to_pattern(&row)?);
    }
    Ok(patterns)
}

/// Update the strength (decay_factor) of a pattern.
pub async fn update_strength(db: &Database, id: i64, user_id: i64, strength: f32) -> Result<()> {
    let affected = db
        .conn
        .execute(
            "UPDATE brain_patterns SET strength = ?1 WHERE id = ?2 AND user_id = ?3",
            libsql::params![strength as f64, id, user_id],
        )
        .await?;

    if affected == 0 {
        return Err(EngError::NotFound(format!("brain pattern {}", id)));
    }
    Ok(())
}

/// Increment access_count and set last_activated_at to now.
pub async fn touch_pattern(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn
        .execute(
            "UPDATE brain_patterns \
             SET access_count = access_count + 1, \
                 last_activated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    Ok(())
}

/// Delete a single pattern.
pub async fn delete_pattern(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn
        .execute(
            "DELETE FROM brain_patterns WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    // Also clean up edges referencing this pattern
    db.conn
        .execute(
            "DELETE FROM brain_edges WHERE (source_id = ?1 OR target_id = ?1) AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    Ok(())
}

/// Delete all patterns whose strength is below the given threshold.
/// Returns the number of deleted patterns.
pub async fn delete_weak_patterns(db: &Database, user_id: i64, threshold: f32) -> Result<usize> {
    // First collect IDs so we can clean edges
    let mut rows = db
        .conn
        .query(
            "SELECT id FROM brain_patterns WHERE user_id = ?1 AND strength < ?2",
            libsql::params![user_id, threshold as f64],
        )
        .await?;

    let mut dead_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        dead_ids.push(id);
    }

    if dead_ids.is_empty() {
        return Ok(0);
    }

    let count = dead_ids.len();

    // Delete patterns
    db.conn
        .execute(
            "DELETE FROM brain_patterns WHERE user_id = ?1 AND strength < ?2",
            libsql::params![user_id, threshold as f64],
        )
        .await?;

    // Clean edges referencing dead patterns
    for id in &dead_ids {
        db.conn
            .execute(
                "DELETE FROM brain_edges WHERE (source_id = ?1 OR target_id = ?1) AND user_id = ?2",
                libsql::params![*id, user_id],
            )
            .await?;
    }

    Ok(count)
}

/// Count patterns for a user.
pub async fn count_patterns(db: &Database, user_id: i64) -> Result<i64> {
    let mut rows = db
        .conn
        .query(
            "SELECT COUNT(*) FROM brain_patterns WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_pattern(row: &libsql::Row) -> Result<BrainPattern> {
    let id: i64 = row.get(0)?;
    let user_id: i64 = row.get(1)?;
    let blob: Vec<u8> = row.get(2)?;
    let strength: f64 = row.get(3)?;
    let importance: i32 = row.get(4)?;
    let access_count: i32 = row.get(5)?;
    let last_activated_at: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;

    Ok(BrainPattern {
        id,
        user_id,
        pattern: blob_to_pattern(&blob),
        strength: strength as f32,
        importance,
        access_count,
        last_activated_at,
        created_at,
    })
}
