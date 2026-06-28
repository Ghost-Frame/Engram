pub mod activity;
pub mod admin;
pub mod agents;
pub mod approvals;
pub mod artifacts;
pub mod artifacts_crypto;
pub mod attention;
pub mod audit;
pub mod auth;
pub mod auth_piv;
pub mod commerce;
// Re-exported from the shared `kleos-config` crate so existing call sites keep
// using `kleos_lib::config::*` (and `crate::config::*` within this crate) while
// the schema lives in one place that the installer also depends on.
pub use kleos_config::config;
pub mod context;
pub mod conversations;
pub mod cred;
pub mod db;
pub mod embeddings;
pub mod encryption;
pub mod episodes;
pub mod errors_log;
pub mod facts;
pub mod forge;
pub mod frameshift_growth;
pub mod fsrs;
pub mod gate;
pub mod graph;
pub mod grounding;
pub mod handoffs;
pub mod inbox;
pub mod ingestion;
pub mod intelligence;
pub mod jobs;
pub mod lang;
pub mod lexicon;
pub mod llm;
pub mod mcp_token;
pub mod memory;
pub mod net;
pub mod observability;
pub mod pack;
pub mod pagination;
pub mod personality;
pub mod preferences;
pub mod projects;
pub mod prompts;
pub mod quota;
pub mod ratelimit;
pub mod reranker;
pub mod resilience;
pub mod scratchpad;
pub mod services;
pub mod sessions;
pub mod skills;
pub mod spaces;
pub mod sync;
pub mod tenant;
pub mod validation;
pub mod vector;
pub mod webhooks;

#[cfg(feature = "brain_hopfield")]
pub mod brain;

// Environment resolution now lives in the shared `kleos-config` crate; re-export
// it at the historical paths so call sites use `kleos_lib::kleos_env(..)` (or
// `crate::kleos_env` within this crate) without an explicit import at every site.
pub use kleos_config::env;
pub use kleos_config::kleos_env;

use thiserror::Error;

/// Crate-wide error type spanning database, IO, auth, and domain failures.
#[derive(Debug, Error)]
pub enum EngError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("database error: {0}")]
    DatabaseMessage(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("encryption error: {0}")]
    Encryption(String),

    /// M-015: resource limit hit (e.g. brain pending queue full, spawn cap).
    #[error("resource limit: {0}")]
    Resource(String),

    /// E2: shard quota exceeded (content bytes or memory count).
    /// Maps to HTTP 507 Insufficient Storage.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
}

/// Convenience alias for results that fail with [`EngError`].
pub type Result<T> = std::result::Result<T, EngError>;

/// Tests for error construction and conversion behavior.
#[cfg(test)]
mod error_tests {
    use super::*;

    /// Confirm QuotaExceeded carries its message and displays correctly.
    #[test]
    fn quota_exceeded_display() {
        let e = EngError::QuotaExceeded("content quota: 100 + 50 > 100".to_string());
        assert!(e.to_string().contains("quota exceeded"));
        assert!(e.to_string().contains("content quota"));
    }
}
