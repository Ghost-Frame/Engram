//! agent-forge stateful reasoning tools -- server-side port.
//!
//! DB-backed stateful operations that previously lived in the local
//! `~/.agent-forge/forge.db`: `spec`, `hypothesis`, `approaches`, `session`,
//! and `verify`. Each follows the convention:
//!   `pub async fn name(db: &Database, user_id: i64, ...) -> crate::Result<Value>`
//!
//! Stateless compute tools (repo_map, search_code, comment_check, think,
//! declare_unknowns, challenge_code) are served by kleos-server reusing the
//! `agent_forge` library directly, so they are intentionally absent here.

pub mod approaches;
pub mod hypothesis;
pub mod session;
pub mod spec;
pub mod verify;
