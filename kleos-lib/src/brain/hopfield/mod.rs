//! Hopfield neural substrate -- in-process associative memory.
//!
//! This module implements a modern Hopfield network (Ramsauer et al. 2020)
//! for pattern completion and associative recall. Unlike the classic binary
//! Hopfield net, this variant stores continuous embedding vectors and uses
//! softmax attention for retrieval, giving it exponential capacity in the
//! pattern dimension.
//!
//! # Architecture
//!
//! - **network.rs** -- Core `HopfieldNetwork` struct: matrix storage,
//!   softmax-attention retrieval, iterative pattern completion.
//! - **pattern.rs** -- `BrainPattern` type and SQLite persistence
//!   (brain_patterns table).
//! - **edges.rs** -- `BrainEdge` type and SQLite persistence
//!   (brain_edges table).
//! - **recall.rs** -- High-level operations: store_pattern, recall_pattern,
//!   reinforce, decay_tick, prune_weak, merge_similar.
//!
//! # Usage
//!
//! ```ignore
//! use kleos_lib::brain::hopfield::{HopfieldNetwork, recall};
//!
//! let mut net = HopfieldNetwork::new();
//! recall::store_pattern(&db, &mut net, id, &embedding, user_id, importance, 1.0).await?;
//! let results = recall::recall_pattern(&db, &net, &query, user_id, 10, 8.0).await?;
//! ```

pub mod edges;
pub mod interference;
pub mod network;
pub mod pattern;
pub mod recall;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export the main types for convenience
pub use network::{HopfieldNetwork, DEFAULT_BETA};
pub use types::{BrainEdge, BrainPattern, DecayStats, EdgeType, RecallResult};
