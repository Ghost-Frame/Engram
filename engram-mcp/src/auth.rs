use crate::App;
use engram_lib::auth::{validate_key, AuthContext, Scope};
use engram_lib::{EngError, Result};
use serde_json::Value;

pub async fn resolve_auth(app: &App, args: &Value) -> Result<AuthContext> {
    let token = args
        .get("bearer_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| std::env::var("ENGRAM_MCP_BEARER_TOKEN").ok())
        .ok_or_else(|| {
            EngError::Auth(
                "missing bearer token; set ENGRAM_MCP_BEARER_TOKEN or pass bearer_token".into(),
            )
        })?;
    validate_key(&app.db, &token).await
}

pub fn require_admin(auth: &AuthContext) -> Result<()> {
    if auth.has_scope(&Scope::Admin) {
        Ok(())
    } else {
        Err(EngError::Auth("admin scope required".into()))
    }
}
