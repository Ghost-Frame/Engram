//! agent-forge library crate. Exposes the stateless compute functions --
//! tree-sitter AST analysis, comment scanning, prompt builders, and I/O helpers
//! -- so that kleos-server and other crates can call them directly without
//! re-implementing the logic.
//!
//! The binary target (`agent-forge`) uses this crate as its module source via
//! `use agent_forge::...` re-exports.

/// Database access: SQLite forge DB open/migrate/query.
pub mod db;

/// Persistent, repository-local code indexing and context retrieval.
pub mod code_context;

/// Emission layer: renders the captured audit trail into committed markdown.
/// Compiled only under the `fluency` feature, which is off by default.
#[cfg(feature = "fluency")]
pub mod emit;

/// JSON file I/O and the canonical `Output` envelope used by every tool.
pub mod json_io;

/// Blocking HTTP client for the Kleos skills API.
pub mod kleos_client;

/// Structured fields shared by task specifications and Fluency renderers.
pub mod spec_types;

/// Local MCP transport and tool registry. Fluency is required because this
/// surface exists to keep repository-local checkpoint and review in one DB.
#[cfg(feature = "fluency")]
pub mod mcp;

/// All agent-forge tools: AST analysis, comment checking, hypothesis tracking,
/// reasoning prompts, verification, and more.
pub mod tools;

/// Tree-sitter language registry and file parser.
pub mod treesitter;
