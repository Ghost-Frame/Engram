use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

/// Mount the policy router. The single route, `GET /policy/mandatory`,
/// returns the operator-configured mandatory rules text that the CLI
/// injects into every agent session at start-up.
pub fn router() -> Router<AppState> {
    Router::new().route("/policy/mandatory", get(mandatory))
}

/// Return the mandatory rules text configured for this Kleos instance.
///
/// Source: the `KLEOS_MANDATORY_RULES` environment variable on the server
/// process. If unset or empty, returns an empty string so a fresh install
/// injects no rules. Operators set the env var (typically via the systemd
/// unit or shell profile) to push their own rules to every connected
/// agent session.
///
/// Response shape: `{ "rules": <string>, "etag": <sha256_hex> }`. The
/// etag lets the CLI skip re-injection when the rules have not changed.
async fn mandatory() -> Json<Value> {
    let rules = std::env::var("KLEOS_MANDATORY_RULES").unwrap_or_default();
    let hash = kleos_lib::artifacts::sha256_hex(rules.as_bytes());
    Json(json!({
        "rules": rules,
        "etag": hash,
    }))
}
