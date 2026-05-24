//! Artifact storage, retrieval, deletion, and full-text search.
//!
//! Each tenant DB has an `artifacts` table (inline BLOB or disk-tier) and an
//! `artifacts_fts` FTS5 virtual table maintained by AFTER triggers. Encryption
//! primitives live in the sibling `artifacts_crypto` module.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::validation::ARTIFACT_FTS_MAX_SIZE;
use crate::{EngError, Result};

/// Converts a rusqlite error into an EngError for uniform error propagation.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Row representation of a stored artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: i64,
    pub memory_id: Option<i64>,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub storage_mode: String,
    /// Filesystem path for disk-tier artifacts (None for inline storage).
    pub disk_path: Option<String>,
    pub is_encrypted: bool,
    pub is_indexed: bool,
    pub created_at: String,
}

/// Aggregate statistics for artifact storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactStats {
    pub total_count: i64,
    pub total_bytes: i64,
    pub inline_count: i64,
    pub inline_bytes: i64,
    pub disk_count: i64,
    pub disk_bytes: i64,
}

/// Lightweight artifact metadata used in enrichment results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
}

/// Compute SHA-256 hash of byte data, returned as a hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Compute the sharded blob path for a given SHA-256 hash.
///
/// Format: `{blobs_dir}/{sha[0:2]}/{sha[2:4]}/{sha}.bin` (or `.enc` if
/// encrypted). The two-level fan-out keeps per-directory inode counts
/// manageable even at scale (max 256 dirs per level).
pub fn blob_path(blobs_dir: &std::path::Path, sha256: &str, encrypted: bool) -> std::path::PathBuf {
    let ext = if encrypted { "enc" } else { "bin" };
    blobs_dir
        .join(&sha256[..2])
        .join(&sha256[2..4])
        .join(format!("{}.{}", sha256, ext))
}

/// Application MIME type subtypes that are eligible for FTS indexing.
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

/// Check if a MIME type should be full-text indexed.
pub fn is_indexable_mime_type(mime: &str) -> bool {
    if mime.starts_with("text/") {
        return true;
    }
    INDEXABLE_APP_TYPES.contains(&mime)
}

/// Truncate `data` to the FTS indexing cap, decode as UTF-8, and trim. Returns
/// `None` if the bytes aren't valid UTF-8 or the trimmed result is empty.
///
/// Used to compute the `content` column value for indexable artifacts. The FTS
/// triggers in `schema_sql.rs` populate `artifacts_fts` directly from that
/// column on INSERT/UPDATE, so the application no longer maintains the FTS
/// index manually.
pub fn extract_indexable_content(mime_type: &str, data: &[u8]) -> Option<String> {
    if !is_indexable_mime_type(mime_type) {
        return None;
    }
    let head = &data[..data.len().min(ARTIFACT_FTS_MAX_SIZE)];
    let text = std::str::from_utf8(head).ok()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
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
/// Tenant isolation is handled at the database level (each tenant has its
/// own DB file). The memory existence check ensures the target memory is
/// valid within this tenant's database.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, data, disk_path, opts), fields(memory_id, mime_type = %mime_type, size_bytes, sha256 = %sha256, storage_mode = %storage_mode, is_encrypted))]
pub async fn store_artifact(
    db: &Database,
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
        let exists = conn
            .query_row(
                "SELECT 1 FROM memories WHERE id = ?1",
                params![memory_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?;

        if exists.is_none() {
            return Err(crate::EngError::NotFound("memory not found".into()));
        }

        // is_indexed reflects whether the `content` column carries indexable
        // text -- the FTS triggers in schema_sql.rs use that column verbatim,
        // so populated content == searchable artifact.
        let is_indexed = content.is_some() as i64;

        conn.query_row(
            "INSERT INTO artifacts \
             (name, memory_id, filename, artifact_type, content, mime_type, \
              size_bytes, sha256, storage_mode, data, disk_path, is_encrypted, \
              is_indexed, source_url, agent, session_id, metadata) \
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
                is_indexed,
                source_url,
                agent,
                session_id,
                metadata
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| crate::EngError::Internal(format!("failed to insert artifact: {e}")))
    })
    .await
}

/// List all artifacts attached to a memory.
#[tracing::instrument(skip(db), fields(memory_id))]
pub async fn get_artifacts_by_memory(db: &Database, memory_id: i64) -> Result<Vec<ArtifactRow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, memory_id, filename, mime_type, size_bytes, \
                        sha256, storage_mode, disk_path, is_encrypted, is_indexed, created_at \
                 FROM artifacts \
                 WHERE memory_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![memory_id], row_to_artifact)
            .map_err(rusqlite_to_eng_error)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Retrieve a single artifact's metadata by ID.
#[tracing::instrument(skip(db), fields(artifact_id))]
pub async fn get_artifact_by_id(db: &Database, artifact_id: i64) -> Result<Option<ArtifactRow>> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, memory_id, filename, mime_type, size_bytes, \
                    sha256, storage_mode, disk_path, is_encrypted, is_indexed, created_at \
             FROM artifacts \
             WHERE id = ?1",
            params![artifact_id],
            row_to_artifact,
        )
        .optional()
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Per-tenant artifact statistics.
///
/// Tenant isolation is at the database level -- each tenant DB contains
/// only that tenant's data, so no user_id filter is needed.
#[tracing::instrument(skip(db))]
pub async fn get_artifact_stats(db: &Database) -> Result<ArtifactStats> {
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

/// Batch-load artifact summaries for a set of memory IDs.
#[tracing::instrument(skip(db, memory_ids), fields(memory_count = memory_ids.len()))]
pub async fn enrich_with_artifacts(
    db: &Database,
    memory_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<ArtifactSummary>>> {
    let mut map = std::collections::HashMap::new();
    for &mid in memory_ids {
        let arts = get_artifacts_by_memory(db, mid).await?;
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

/// Retrieve the raw binary data for an artifact.
#[tracing::instrument(skip(db), fields(artifact_id))]
pub async fn get_artifact_data(db: &Database, artifact_id: i64) -> Result<Option<Vec<u8>>> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT data FROM artifacts WHERE id = ?1",
            params![artifact_id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map(|opt| opt.flatten())
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Delete a single artifact by ID.
///
/// The `artifacts_fts_delete` trigger fires automatically on DELETE, so no
/// manual FTS cleanup is needed. Returns `Ok(None)` when no artifact with
/// that ID exists (idempotent). Returns `Ok(Some(path))` when the deleted
/// row carried a `disk_path` that the caller should unlink from the
/// filesystem.
#[tracing::instrument(skip(db), fields(artifact_id))]
pub async fn delete_artifact(db: &Database, artifact_id: i64) -> Result<Option<String>> {
    db.write(move |conn| {
        let disk_path: Option<String> = conn
            .query_row(
                "SELECT disk_path FROM artifacts WHERE id = ?1",
                params![artifact_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .flatten();

        let rows_deleted = conn
            .execute("DELETE FROM artifacts WHERE id = ?1", params![artifact_id])
            .map_err(rusqlite_to_eng_error)?;

        if rows_deleted == 0 {
            Ok(None)
        } else {
            Ok(disk_path)
        }
    })
    .await
}

/// Full-text search across artifact name and content via the `artifacts_fts`
/// FTS5 virtual table. Returns matching rows ordered by BM25 rank (best match
/// first). An optional `memory_id` narrows the search to a single memory's
/// attachments.
///
/// The caller is responsible for capping `limit` to a sane upper bound before
/// calling this function (the server layer caps at 100).
#[tracing::instrument(skip(db), fields(%query, limit, memory_id))]
pub async fn search_artifacts(
    db: &Database,
    query: &str,
    limit: usize,
    memory_id: Option<i64>,
) -> Result<Vec<ArtifactRow>> {
    if query.trim().is_empty() {
        return Err(crate::EngError::InvalidInput(
            "artifact search query must not be empty".into(),
        ));
    }

    let query = query.to_string();
    let limit = limit as i64;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT a.id, a.memory_id, a.filename, a.mime_type, a.size_bytes, \
                        a.sha256, a.storage_mode, a.disk_path, a.is_encrypted, \
                        a.is_indexed, a.created_at \
                 FROM artifacts a \
                 JOIN artifacts_fts ON artifacts_fts.rowid = a.id \
                 WHERE artifacts_fts MATCH ?1 \
                   AND (?2 IS NULL OR a.memory_id = ?2) \
                 ORDER BY bm25(artifacts_fts) \
                 LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![query, memory_id, limit], row_to_artifact)
            .map_err(rusqlite_to_eng_error)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Maps a rusqlite row to an ArtifactStats value.
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

/// Maps a rusqlite row to an ArtifactRow value.
fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row.get(0)?,
        memory_id: row.get(1)?,
        filename: row.get(2)?,
        mime_type: row.get(3)?,
        size_bytes: row.get(4)?,
        sha256: row.get(5)?,
        storage_mode: row.get(6)?,
        disk_path: row.get(7)?,
        is_encrypted: row.get::<_, i64>(8).unwrap_or(0) != 0,
        is_indexed: row.get::<_, i64>(9).unwrap_or(0) != 0,
        created_at: row.get(10)?,
    })
}

/// Unit tests for artifact operations.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MIME type classification for FTS indexability.
    #[test]
    fn test_indexable_mime_types() {
        assert!(is_indexable_mime_type("text/plain"));
        assert!(is_indexable_mime_type("application/json"));
        assert!(!is_indexable_mime_type("image/png"));
    }

    /// Verifies SHA-256 hex output is 64 characters long.
    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64);
    }
}
