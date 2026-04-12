use crate::App;
use engram_lib::auth::{validate_key, AuthContext, Scope};
use engram_lib::{EngError, Result};
use serde_json::Value;

/// Resolve authentication from the JSON-RPC arguments.
///
/// For **stdio** transport the caller passes `bearer_token` in each request
/// (or the process-level `ENGRAM_MCP_BEARER_TOKEN` env is used as fallback).
///
/// For **HTTP** transport, transport-level middleware already validates the
/// bearer token before any handler runs.  The per-tool `bearer_token` field
/// is still honoured so an HTTP caller can downgrade to a scoped key.
pub async fn resolve_auth(app: &App, args: &Value) -> Result<AuthContext> {
    let token = args
        .get("bearer_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| EngError::Auth("missing bearer_token argument".into()))?;
    validate_key(&app.db, &token).await
}

pub fn require_admin(auth: &AuthContext) -> Result<()> {
    if auth.has_scope(&Scope::Admin) {
        Ok(())
    } else {
        Err(EngError::Auth("admin scope required".into()))
    }
}
