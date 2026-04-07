#![allow(dead_code)]

pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod embeddings;
pub mod episodes;
pub mod fsrs;
pub mod graph;
pub mod guard;
pub mod intelligence;
pub mod memory;
pub mod personality;
pub mod reranker;
pub mod services;
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
}

pub type Result<T> = std::result::Result<T, EngError>;
