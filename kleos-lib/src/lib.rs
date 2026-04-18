#![allow(dead_code)]

pub mod activity;
pub mod admin;
pub mod agents;
pub mod approvals;
pub mod artifacts;
pub mod audit;
pub mod auth;
pub mod cognitive;
pub mod config;
pub mod context;
pub mod conversations;
pub mod cred;
pub mod db;
pub mod embeddings;
pub mod encryption;
pub mod episodes;
pub mod errors_log;
pub mod facts;
pub mod fsrs;
pub mod gate;
pub mod graph;
pub mod grounding;
pub mod guard;
pub mod inbox;
pub mod ingestion;
pub mod intelligence;
pub mod jobs;
pub mod llm;
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
pub mod sync;
pub mod tenant;
pub mod validation;
pub mod vector;
pub mod webhooks;

#[cfg(feature = "brain_hopfield")]
pub mod brain;

use thiserror::Error;

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

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("encryption error: {0}")]
    Encryption(String),
}

pub type Result<T> = std::result::Result<T, EngError>;
