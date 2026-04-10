#![allow(dead_code)]

pub mod activity;
pub mod admin;
pub mod agents;
pub mod apikeys;
pub mod artifacts;
pub mod audit;
pub mod auth;
pub mod cognithor;
pub mod config;
pub mod context;
pub mod conversations;
pub mod db;
pub mod embeddings;
pub mod episodes;
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
pub mod pack;
pub mod personality;
pub mod preferences;
pub mod projects;
pub mod prompts;
pub mod quota;
pub mod ratelimit;
pub mod reranker;
pub mod scratchpad;
pub mod services;
pub mod sessions;
pub mod skills;
pub mod sync;
pub mod vector;
pub mod webhooks;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngError {
    #[error("database error: {0}")]
    Database(#[from] libsql::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),
}

pub type Result<T> = std::result::Result<T, EngError>;
