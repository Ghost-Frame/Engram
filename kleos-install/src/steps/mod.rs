//! Step modules for each screen of the Kleos installer wizard.
//!
//! Each sub-module owns the local UI state and rendering/input logic for one
//! wizard step. The wizard orchestrator in `wizard.rs` delegates to these
//! modules by calling their `draw_*` and `handle_*_input` functions.

/// Advanced (expert) toggle configuration step.
pub mod advanced;
/// Component and profile selection step.
pub mod components;
/// Embedding and reranker provider configuration step.
pub mod embeddings;
/// Security key generation and configuration step.
pub mod security;
/// Server host, port, and path configuration step.
pub mod server;
/// Installation summary and confirmation step.
pub mod summary;
/// System service integration step.
pub mod system;
