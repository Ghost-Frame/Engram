//! agent-forge library crate. Exposes the stateless compute functions --
//! tree-sitter AST analysis, comment scanning, prompt builders, and I/O helpers
//! -- so that kleos-server and other crates can call them directly without
//! re-implementing the logic.
//!
//! The binary target (`agent-forge`) uses this crate as its module source via
//! `use agent_forge::...` re-exports.

/// Database access: SQLite forge DB open/migrate/query.
pub mod db;

/// JSON file I/O and the canonical `Output` envelope used by every tool.
pub mod json_io;

/// Blocking HTTP client for the Kleos skills API.
pub mod kleos_client;

/// All agent-forge tools: AST analysis, comment checking, hypothesis tracking,
/// reasoning prompts, verification, and more.
pub mod tools;

/// Tree-sitter language registry and file parser.
pub mod treesitter;
