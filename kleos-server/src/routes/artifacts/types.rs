//! DTOs for /artifacts routes.

use serde::Deserialize;

/// Request body for `POST /artifacts/search`.
#[derive(Debug, Deserialize)]
pub struct ArtifactSearchBody {
    /// FTS query string (passed to FTS5 MATCH).
    pub query: String,
    /// Maximum rows to return (server caps at 100).
    pub limit: Option<usize>,
    /// Optional filter to search within a single memory's attachments.
    pub memory_id: Option<i64>,
}
