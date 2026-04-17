//! Artifact storage, encryption, and full-text indexing.
//!
//! Ports: artifacts/encryption.ts, artifacts/fts.ts, artifacts/storage.ts

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::{EngError, Result};

const ARTIFACT_FTS_MAX_SIZE: usize = 1_048_576;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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

#[tracing::instrument(skip(db, data), fields(artifact_id, user_id, mime_type = %mime_type, data_len = data.len()))]
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

    let owned = db
        .read(move |conn| {
            let found = conn
                .query_row(
                    "SELECT 1 FROM artifacts a \
                     INNER JOIN memories m ON a.memory_id = m.id \
                     WHERE a.id = ?1 AND m.user_id = ?2",
                    params![artifact_id, user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(rusqlite_to_eng_error)?;
            Ok(found.is_some())
        })
        .await
        .unwrap_or(false);

    if !owned {
        tracing::warn!(
            artifact_id,
            user_id,
            "artifact FTS index rejected: not owned"
        );
        return false;
    }

    let indexed = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO artifacts_fts (rowid, content) VALUES (?1, ?2)",
                params![artifact_id, text],
            )
            .map_err(rusqlite_to_eng_error)?;
            conn.execute(
                "UPDATE artifacts SET is_indexed = 1 WHERE id = ?1",
                params![artifact_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await;

    if indexed.is_err() {
        tracing::warn!(artifact_id, "artifact FTS index failed");
        return false;
    }

    true
}

/// Options for storing an artifact, beyond the required fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreArtifactOpts {
    pub artifact_type: Option<String>,
    pub content: Option<String>,
    pub source_url: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub metadata: Option<String>,
}

/// Insert an artifact row attached to `memory_id`.
///
/// SECURITY (MT-F3): callers must pass the authenticated `user_id` so we
/// can verify the target memory belongs to that tenant *before* inserting.
/// Without this gate, a tenant holding any numeric memory id could attach
/// files to another tenant's memory row.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, data, disk_path, opts), fields(user_id, memory_id, mime_type = %mime_type, size_bytes, sha256 = %sha256, storage_mode = %storage_mode, is_encrypted))]
pub async fn store_artifact(
    db: &Database,
    user_id: i64,
    memory_id: i64,
    name: &str,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    sha256: &str,
    storage_mode: &str,
    data: Option<Vec<u8>>,
    disk_path: Option<&str>,
    is_encrypted: bool,
    opts: &StoreArtifactOpts,
) -> Result<i64> {
    let name = name.to_string();
    let filename = filename.to_string();
    let mime_type = mime_type.to_string();
    let sha256 = sha256.to_string();
    let storage_mode = storage_mode.to_string();
    let disk_path = disk_path.map(|s| s.to_string());
    let artifact_type = opts
        .artifact_type
        .clone()
        .unwrap_or_else(|| "file".to_string());
    let content = opts.content.clone();
    let source_url = opts.source_url.clone();
    let agent = opts.agent.clone();
    let session_id = opts.session_id.clone();
    let metadata = opts.metadata.clone();

    db.write(move |conn| {
        let owned = conn
            .query_row(
                "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                params![memory_id, user_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?;

        if owned.is_none() {
            return Err(crate::EngError::NotFound(
                "memory not found for this tenant".into(),
            ));
        }

        conn.query_row(
            "INSERT INTO artifacts \
             (name, memory_id, filename, artifact_type, content, mime_type, \
              size_bytes, sha256, storage_mode, data, disk_path, is_encrypted, \
              source_url, agent, session_id, metadata, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
             RETURNING id",
            params![
                name,
                memory_id,
                filename,
                artifact_type,
                content,
                mime_type,
                size_bytes,
                sha256,
                storage_mode,
                data,
                disk_path,
                is_encrypted as i64,
                source_url,
                agent,
                session_id,
                metadata,
                user_id
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| crate::EngError::Internal(format!("failed to insert artifact: {e}")))
    })
    .await
}

#[tracing::instrument(skip(db), fields(memory_id, user_id))]
pub async fn get_artifacts_by_memory(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<ArtifactRow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT a.id, a.memory_id, a.filename, a.mime_type, a.size_bytes, \
                        a.sha256, a.storage_mode, a.is_encrypted, a.is_indexed, a.created_at \
                 FROM artifacts a \
                 INNER JOIN memories m ON a.memory_id = m.id \
                 WHERE a.memory_id = ?1 AND m.user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![memory_id, user_id], row_to_artifact)
            .map_err(rusqlite_to_eng_error)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[tracing::instrument(skip(db), fields(artifact_id, user_id))]
pub async fn get_artifact_by_id(
    db: &Database,
    artifact_id: i64,
    user_id: i64,
) -> Result<Option<ArtifactRow>> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT a.id, a.memory_id, a.filename, a.mime_type, a.size_bytes, \
                    a.sha256, a.storage_mode, a.is_encrypted, a.is_indexed, a.created_at \
             FROM artifacts a \
             INNER JOIN memories m ON a.memory_id = m.id \
             WHERE a.id = ?1 AND m.user_id = ?2",
            params![artifact_id, user_id],
            row_to_artifact,
        )
        .optional()
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Per-tenant artifact statistics (SECURITY: MT-F4).
///
/// The previous helper accepted `Option<i64>` where `None` meant "every
/// tenant's artifacts collapsed into one number." That was an admin
/// backdoor reachable from an otherwise tenant-scoped handler. Split into
/// two explicit entry points so the scope is obvious at the call site.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_artifact_stats(db: &Database, user_id: i64) -> Result<ArtifactStats> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), \
                    COALESCE(SUM(a.size_bytes),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='inline' THEN a.size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='disk' THEN a.size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='inline' THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN a.storage_mode='disk' THEN 1 ELSE 0 END),0) \
             FROM artifacts a \
             JOIN memories m ON a.memory_id = m.id \
             WHERE m.user_id = ?1",
            params![user_id],
            stats_from_row,
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Cluster-wide artifact statistics. Only call from explicitly admin-gated
/// routes. Never expose to tenant-scoped handlers.
#[tracing::instrument(skip(db))]
pub async fn get_artifact_stats_all(db: &Database) -> Result<ArtifactStats> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), \
                    COALESCE(SUM(size_bytes),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='inline' THEN size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='disk' THEN size_bytes ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='inline' THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN storage_mode='disk' THEN 1 ELSE 0 END),0) \
             FROM artifacts",
            [],
            stats_from_row,
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[tracing::instrument(skip(db, memory_ids), fields(memory_count = memory_ids.len(), user_id))]
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

#[tracing::instrument(skip(db), fields(artifact_id, user_id))]
pub async fn get_artifact_data(
    db: &Database,
    artifact_id: i64,
    user_id: i64,
) -> Result<Option<Vec<u8>>> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT a.data FROM artifacts a \
             INNER JOIN memories m ON a.memory_id = m.id \
             WHERE a.id = ?1 AND m.user_id = ?2",
            params![artifact_id, user_id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map(|opt| opt.flatten())
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

fn stats_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactStats> {
    Ok(ArtifactStats {
        total_count: row.get::<_, i64>(0).unwrap_or(0),
        total_bytes: row.get::<_, i64>(1).unwrap_or(0),
        inline_bytes: row.get::<_, i64>(2).unwrap_or(0),
        disk_bytes: row.get::<_, i64>(3).unwrap_or(0),
        inline_count: row.get::<_, i64>(4).unwrap_or(0),
        disk_count: row.get::<_, i64>(5).unwrap_or(0),
    })
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row.get(0)?,
        memory_id: row.get(1)?,
        filename: row.get(2)?,
        mime_type: row.get(3)?,
        size_bytes: row.get(4)?,
        sha256: row.get(5)?,
        storage_mode: row.get(6)?,
        is_encrypted: row.get::<_, i64>(7).unwrap_or(0) != 0,
        is_indexed: row.get::<_, i64>(8).unwrap_or(0) != 0,
        created_at: row.get(9)?,
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
