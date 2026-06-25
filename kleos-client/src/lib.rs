//! Shared HTTP client used by `kleos-cli` and `kleos-mcp` to talk to
//! `kleos-server`. Handles PIV-signed envelope auth via
//! `kleos_lib::auth_piv::RequestSigner`, bearer-token fallback for legacy
//! callers, session-token capture, and uniform error formatting.

mod client;
pub mod routes;

pub use client::{body_excerpt, format_error_chain, format_reqwest_error, truncate, Client};
pub use routes::{
    find_by_name, is_mcp_blocked, render_path, resolve_tool_name, Method, Route, Scope,
    MCP_BLOCKED_ROUTES, ROUTES,
};
