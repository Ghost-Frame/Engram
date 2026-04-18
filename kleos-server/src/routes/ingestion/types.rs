use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct UploadInitBody {
    /// Optional original filename -- used to detect format on complete.
    pub filename: Option<String>,
    /// Optional MIME type; used as a fallback format hint.
    pub content_type: Option<String>,
    /// Tag stamped on every ingested memory ("upload" by default).
    pub source: Option<String>,
    /// Client-declared total byte size. Server enforces the hard cap
    /// regardless of what the client claims here.
    pub total_size: Option<i64>,
    /// Client-declared chunk count. Used only to short-circuit complete()
    /// when all chunks have arrived; the client may also pass it to
    /// complete() directly.
    pub total_chunks: Option<i64>,
    /// Advisory chunk size (bytes) the client intends to use. Recorded for
    /// diagnostics; the server enforces only MAX_UPLOAD_CHUNK_BYTES.
    pub chunk_size: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadChunkBody {
    pub upload_id: String,
    pub chunk_index: i64,
    /// Hex-encoded SHA-256 of the decoded chunk bytes.
    pub chunk_hash: String,
    /// Base64-encoded chunk payload.
    pub data: String,
}

/// Session row shape used by chunk / complete / status handlers.
pub(super) struct UploadSession {
    pub user_id: i64,
    pub status: String,
    pub source: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub total_chunks: Option<i64>,
    pub total_size: Option<i64>,
    pub expires_at: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadCompleteBody {
    pub upload_id: String,
    /// Client-declared chunk count. When present it must match the stored
    /// count; this catches the case where a chunk silently dropped and the
    /// client never noticed.
    pub total_chunks: Option<i64>,
    /// Optional hex SHA-256 of the full reassembled payload for end-to-end
    /// integrity verification.
    pub final_sha256: Option<String>,
    /// Ingest mode for the assembled payload. Defaults to "extract".
    pub mode: Option<String>,
    /// Optional format hint (overrides filename/content_type detection).
    pub format: Option<String>,
    /// Optional target category for generated memories.
    pub category: Option<String>,
    pub project_id: Option<i64>,
    pub episode_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadAbortBody {
    pub upload_id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportBulkBody {
    pub text: Option<String>,
    pub url: Option<String>,
    pub format: Option<String>,
    pub mode: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub project_id: Option<i64>,
    pub episode_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportJsonBody {
    pub version: Option<String>,
    pub memories: Option<Vec<ImportMemory>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportMemory {
    pub content: Option<String>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub session_id: Option<String>,
    pub importance: Option<i32>,
    pub tags: Option<serde_json::Value>,
    pub confidence: Option<f64>,
    pub is_static: Option<bool>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct IngestBody {
    pub url: Option<String>,
    pub text: Option<String>,
    pub title: Option<String>,
    pub source: Option<String>,
    pub entity_ids: Option<Vec<i64>>,
    #[allow(dead_code)]
    pub project_ids: Option<Vec<i64>>,
    pub episode_id: Option<i64>,
}
