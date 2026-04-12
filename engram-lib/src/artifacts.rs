//! Artifact storage, encryption, and full-text indexing.
//!
//! Ports: artifacts/encryption.ts, artifacts/fts.ts, artifacts/storage.ts

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::Result;

const ARTIFACT_FTS_MAX_SIZE: usize = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: i64,
    pub memory_id: Option<i64>,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub storage_mode: String,
    pub is_encrypted: bool,
    pub is_indexed: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactStats {
    pub total_count: i64,
    pub total_bytes: i64,
    pub inline_count: i64,
    pub inline_bytes: i64,
    pub disk_count: i64,
    pub disk_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

const INDEXABLE_APP_TYPES: &[&str] = &[
    "application/json",
    "application/yaml",
    "application/x-yaml",
    "application/xml",
    "application/javascript",
    "application/typescript",
    "application/toml",
    "application/x-sh",
    "application/x-python",
];

pub fn is_indexable_mime_type(mime: &str) -> bool {
    if mime.starts_with("text/") {
        return true;
    }
    INDEXABLE_APP_TYPES.contains(&mime)
}

pub async fn index_artifact(
    db: &Database,
    artifact_id: i64,
    user_id: i64,
    mime_type: &str,
    data: &[u8],
) -> bool {
    if !is_indexable_mime_type(mime_type) {
        return false;
    }
    let text = match std::str::from_utf8(&data[..data.len().min(ARTIFACT_FTS_MAX_SIZE)]) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return false,
    };
    let owned = match db
        .conn
        .query(
            "SELECT 1 FROM artifacts a \
             INNER JOIN memories m ON a.memory_id = m.id \
             WHERE a.id = ?1 AND m.user_id = ?2",
            libsql::params![artifact_id, user_id],
        )
        .await
    {
        Ok(mut rows) => matches!(rows.next().await, Ok(Some(_))),
        Err(_) => false,
    };
    if !owned {
        tracing::warn!(
            artifact_id,
            user_id,
            "artifact FTS index rejected: not owned"
        );
        return false;
    }
    if db
        .conn
        .execute(
            "INSERT INTO artifacts_fts (rowid, content) VALUES (?1, ?2)",
            libsql::params![artifact_id, text.clone()],
        )
        .await
        .is_err()
    {
        tracing::warn!(artifact_id, "artifact FTS index failed");
        return false;
    }
    let _ = db
        .conn
        .execute(
            "UPDATE artifacts SET is_indexed = 1 WHERE id = ?1",
            libsql::params![artifact_id],
        )
        .await;
    true
}

/// Insert an artifact row attached to `memory_id`.
///
/// SECURITY (MT-F3): callers must pass the authenticated `user_id` so we
/// can verify the target memory belongs to that tenant *before* inserting.
/// Without this gate, a tenant holding any numeric memory id could attach
/// files to another tenant's memory row.
#[allow(clippy::too_many_arguments)]
pub async fn store_artifact(
    db: &Database,
    user_id: i64,
    memory_id: i64,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    sha256: &str,
    storage_mode: &str,
    data: Option<Vec<u8>>,
    disk_path: Option<&str>,
    is_encrypted: bool,
) -> Result<i64> {
    let mut owner_rows = db
        .conn
        .query(
            "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
            libsql::params![memory_id, user_id],
        )
        .await?;
    if owner_rows.next().await?.is_none() {
        return Err(crate::EngError::NotFound(
            "memory not found for this tenant".into(),
        ));
    }

    let mut rows = db.conn.query(
        "INSERT INTO artifacts (memory_id, filename, mime_type, size_bytes, sha256, storage_mode, data, disk_path, is_encrypted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
        libsql::params![memory_id, filename.to_string(), mime_type.to_string(), size_bytes, sha256.to_string(), storage_mode.to_string(), data, disk_path.map(|s| s.to_string()), is_encrypted as i64],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("failed to insert artifact".into()))?;
    row.get::<i64>(0)
        .map_err(|e| crate::EngError::Internal(e.to_string()))
}

pub async fn get_artifacts_by_memory(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<ArtifactRow>> {
    let mut rows = db.conn.query(
        "SELECT a.id, a.memory_id, a.filename, a.mime_type, a.size_bytes, a.sha256, a.storage_mode, a.is_encrypted, a.is_indexed, a.created_at
         FROM artifacts a
         INNER JOIN memories m ON a.memory_id = m.id
         WHERE a.memory_id = ?1 AND m.user_id = ?2",
        libsql::params![memory_id, user_id],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(row_to_artifact(&row)?);
    }
    Ok(result)
}

pub async fn get_artifact_by_id(
    db: &Database,
    artifact_id: i64,
    user_id: i64,
) -> Result<Option<ArtifactRow>> {
    let mut rows = db.conn.query(
        "SELECT a.id, a.memory_id, a.filename, a.mime_type, a.size_bytes, a.sha256, a.storage_mode, a.is_encrypted, a.is_indexed, a.created_at
         FROM artifacts a
         INNER JOIN memories m ON a.memory_id = m.id
         WHERE a.id = ?1 AND m.user_id = ?2",
        libsql::params![artifact_id, user_id],
    ).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_to_artifact(&row)?)),
        None => Ok(None),
    }
}

/// Per-tenant artifact statistics (SECURITY: MT-F4).
///
/// The previous helper accepted `Option<i64>` where `None` meant "every
/// tenant's artifacts collapsed into one number." That was an admin
/// backdoor reachable from an otherwise tenant-scoped handler. Split into
/// two explicit entry points so the scope is obvious at the call site.
pub async fn get_artifact_stats(db: &Database, user_id: i64) -> Result<ArtifactStats> {
    let mut rows = db
        .conn
        .query(
            "SELECT COUNT(*), \
                    COALESCE(SUM(a.size_bytes),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='inline' THEN a.size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='disk' THEN a.size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='inline' THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='disk' THEN 1 ELSE 0 END),0) \
             FROM artifacts a \
             JOIN memories m ON a.memory_id = m.id \
             WHERE m.user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    stats_from_row(&mut rows).await
}

/// Cluster-wide artifact statistics. Only call from explicitly admin-gated
/// routes. Never expose to tenant-scoped handlers.
pub async fn get_artifact_stats_all(db: &Database) -> Result<ArtifactStats> {
    let mut rows = db
        .conn
        .query(
            "SELECT COUNT(*), \
                    COALESCE(SUM(size_bytes),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='inline' THEN size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='disk' THEN size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='inline' THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='disk' THEN 1 ELSE 0 END),0) \
             FROM artifacts",
            libsql::params![],
        )
        .await?;
    stats_from_row(&mut rows).await
}

async fn stats_from_row(rows: &mut libsql::Rows) -> Result<ArtifactStats> {
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("stats query empty".into()))?;
    Ok(ArtifactStats {
        total_count: row.get::<i64>(0).unwrap_or(0),
        total_bytes: row.get::<i64>(1).unwrap_or(0),
        inline_bytes: row.get::<i64>(2).unwrap_or(0),
        disk_bytes: row.get::<i64>(3).unwrap_or(0),
        inline_count: row.get::<i64>(4).unwrap_or(0),
        disk_count: row.get::<i64>(5).unwrap_or(0),
    })
}

pub async fn enrich_with_artifacts(
    db: &Database,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<std::collections::HashMap<i64, Vec<ArtifactSummary>>> {
    let mut map = std::collections::HashMap::new();
    for &mid in memory_ids {
        let arts = get_artifacts_by_memory(db, mid, user_id).await?;
        let summaries: Vec<ArtifactSummary> = arts
            .into_iter()
            .map(|a| ArtifactSummary {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size_bytes: a.size_bytes,
            })
            .collect();
        if !summaries.is_empty() {
            map.insert(mid, summaries);
        }
    }
    Ok(map)
}

pub async fn get_artifact_data(
    db: &Database,
    artifact_id: i64,
    user_id: i64,
) -> Result<Option<Vec<u8>>> {
    let mut rows = db
        .conn
        .query(
            "SELECT a.data FROM artifacts a \
             INNER JOIN memories m ON a.memory_id = m.id \
             WHERE a.id = ?1 AND m.user_id = ?2",
            libsql::params![artifact_id, user_id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0).unwrap_or(None)),
        None => Ok(None),
    }
}

fn row_to_artifact(row: &libsql::Row) -> Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        memory_id: row.get(1).unwrap_or(None),
        filename: row
            .get(2)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        mime_type: row
            .get(3)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        size_bytes: row
            .get(4)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        sha256: row.get(5).unwrap_or(None),
        storage_mode: row
            .get(6)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        is_encrypted: row.get::<i64>(7).unwrap_or(0) != 0,
        is_indexed: row.get::<i64>(8).unwrap_or(0) != 0,
        created_at: row
            .get(9)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indexable_mime_types() {
        assert!(is_indexable_mime_type("text/plain"));
        assert!(is_indexable_mime_type("application/json"));
        assert!(!is_indexable_mime_type("image/png"));
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64);
    }
}
