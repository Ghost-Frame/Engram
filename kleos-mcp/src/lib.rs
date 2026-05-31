//! kleos-mcp -- MCP transport adapter that forwards JSON-RPC requests to
//! the server-side POST /mcp endpoint, signing each request with the
//! local PIV / Ed25519 identity.
//!
//! The server handles all dispatch, scope enforcement, rate limiting, and
//! tool registry. This binary is a thin bridge between the MCP stdio/HTTP
//! transport and the authenticated server endpoint.

pub mod tools;
pub mod transport;

use kleos_client::{find_by_name, Client};
use serde_json::{json, Value};
use std::sync::Arc;

/// Application state -- a single shared HTTP client.
#[derive(Clone)]
pub struct App {
    pub client: Arc<Client>,
}

/// App lifecycle and bootstrap helpers.
impl App {
    /// Bootstrap an App from environment variables.
    ///
    /// Reads `KLEOS_URL` for the server endpoint (default
    /// `http://127.0.0.1:4200`, matching the rest of the workspace) and
    /// loads a PIV / Ed25519 signer via the standard
    /// `RequestSigner::from_env_or_file` path.
    pub fn from_env() -> Result<Self, String> {
        let base_url =
            std::env::var("KLEOS_URL").unwrap_or_else(|_| "http://127.0.0.1:4200".to_string());
        let host_label = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".into());
        let agent_label = std::env::var("KLEOS_AGENT_LABEL").unwrap_or_else(|_| "kleos-mcp".into());
        let model_label = std::env::var("KLEOS_MODEL_LABEL").unwrap_or_else(|_| "none".into());

        let signer = kleos_lib::auth_piv::RequestSigner::from_env_or_file(
            &host_label,
            &agent_label,
            &model_label,
        )
        .map_err(|e| format!("PIV identity load failed: {e}"))?;

        let api_key = std::env::var("KLEOS_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());

        if signer.is_none() && api_key.is_none() {
            return Err(
                "no auth configured: set KLEOS_IDENTITY_PATH (or run `kleos-cli identity init`) \
                 for PIV signing, or set KLEOS_API_KEY as a bearer fallback. Refusing to start \
                 unauthenticated."
                    .to_string(),
            );
        }

        let client = Client::new(base_url, api_key, signer);
        Ok(Self {
            client: Arc::new(client),
        })
    }
}

/// Builds a JSON-RPC error response with the given code and message.
fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }})
}

/// Extracts the request id from a JSON-RPC envelope.
fn request_id(req: &Value) -> Option<Value> {
    req.get("id").cloned()
}

/// Forwards one JSON-RPC request to the server-side POST /mcp endpoint.
/// Returns the server's response, or None for notifications.
/// On transport errors, wraps the error as a JSON-RPC error envelope.
///
/// `tools/list` is served from the local registry so VS Code receives
/// underscore-named tools (its validator rejects dot-notation names).
/// `tools/call` translates alias names back to canonical dot-names
/// before forwarding so the upstream server can dispatch them.
#[tracing::instrument(skip(app, req), fields(method = req.get("method").and_then(|v| v.as_str()).unwrap_or("")))]
pub async fn handle_jsonrpc(app: &App, req: Value) -> Option<Value> {
    let id = request_id(&req);
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");

    match method {
        "tools/list" => {
            // Serve the curated registry. All names are underscore-normalised
            // so VS Code's validator accepts them; tools/call translates back.
            let tools = tools::registry();
            id.map(|id| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": tools }
                })
            })
        }
        "tools/call" => {
            // Translate alias name (e.g. "tasks_list") to canonical dot-name
            // (e.g. "tasks.list") so the upstream server can dispatch it.
            let mut forwarded = req.clone();
            if let Some(name) = forwarded
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
            {
                if let Some(route) = find_by_name(&name) {
                    if route.name != name {
                        if let Some(params) = forwarded.get_mut("params") {
                            params["name"] = json!(route.name);
                        }
                    }
                }
            }
            match app.client.post_mcp(&forwarded).await {
                Ok(resp) => resp,
                Err(e) => id.map(|id| error_response(id, -32603, &e)),
            }
        }
        _ => match app.client.post_mcp(&req).await {
            Ok(resp) => resp,
            Err(e) => id.map(|id| error_response(id, -32603, &e)),
        },
    }
}
