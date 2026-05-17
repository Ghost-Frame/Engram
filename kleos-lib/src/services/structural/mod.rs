//! Structural analysis of EN-syntax system descriptions.
//!
//! EN (Engram Notation) describes a directed dependency graph one line at a
//! time:
//!
//! ```text
//! Subject do: action needs: inputs yields: outputs.
//! ```
//!
//! Multiple `needs:` / `yields:` items are comma-separated; multiple
//! statements share a single source string and are split on periods.
//! Edges run from a `yields:` producer to any consumer whose `needs:`
//! mentions the same token (so an upstream node "yields: x" links to any
//! downstream node "needs: x").
//!
//! The module is intentionally dependency-free at the algorithm layer so the
//! analyzer can be reused from any future graph context (e.g., the
//! `memory_graph` variant).

pub mod analyze;
pub mod graph;
pub mod parser;

pub use analyze::{
    analyze_source, detail_source, distance_in_source, node_betweenness_in_source, trace_in_source,
    AnalyzeReport, BridgeInfo, DetailReport, DistanceReport, NodeRoleEntry, TraceReport,
};
pub use graph::{Graph, NodeRole, Topology};
pub use parser::{parse_en_source, EnStatement};
