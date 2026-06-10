//! Server-side MCP endpoint (Streamable HTTP transport).
//!
//! `POST /mcp` accepts JSON-RPC 2.0 requests and dispatches tool calls
//! through a middleware-free internal Axum router via `Router::oneshot()`.
//! Auth, rate limiting, and audit run once on the outer request; inner
//! dispatch inherits the pre-validated `AuthContext` via request extensions.

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use kleos_client::{find_by_name, render_path, Method, Scope as RouteScope};
use kleos_lib::auth::{AuthContext, Scope};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::extractors::Auth;
use crate::state::AppState;

/// MCP protocol version this endpoint speaks.
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Maximum number of JSON-RPC requests allowed in a single batch.
/// Prevents rate-limit bypass via fan-out and bounds response memory.
const MAX_BATCH_SIZE: usize = 20;

/// Builds the MCP route. When `KLEOS_MCP_ENABLED=0`, returns an empty router.
/// The `dispatch` router is the middleware-free internal router used for
/// in-process tool-call dispatch via `Router::oneshot()`.
pub fn router(dispatch: Router) -> Router<AppState> {
    if std::env::var("KLEOS_MCP_ENABLED")
        .unwrap_or_else(|_| "1".into())
        .trim()
        == "0"
    {
        tracing::info!("server-side MCP endpoint disabled (KLEOS_MCP_ENABLED=0)");
        return Router::new();
    }

    tracing::info!("server-side MCP endpoint enabled at POST /mcp");
    Router::new()
        .route("/mcp", post(mcp_handler))
        .layer(Extension(dispatch))
}

/// Handles one `POST /mcp` request. Supports single JSON-RPC requests and
/// batch arrays per the MCP streamable HTTP spec.
#[tracing::instrument(skip_all, fields(endpoint = "mcp"))]
async fn mcp_handler(
    State(state): State<AppState>,
    Extension(dispatch): Extension<Router>,
    Auth(auth): Auth,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // MCP spec MUST: validate Origin header to prevent DNS rebinding.
    // Reject requests with an Origin that looks like a browser cross-origin
    // attack. Requests without an Origin header (non-browser clients like
    // Claude Code) are allowed through.
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if !is_allowed_origin(origin) {
            return json_rpc_error_response(
                body.get("id").cloned(),
                -32001,
                "Forbidden: Origin not allowed",
            );
        }
    }

    // Batch support: JSON array of requests.
    if let Some(batch) = body.as_array() {
        if batch.len() > MAX_BATCH_SIZE {
            return json_rpc_error_response(
                None,
                -32600,
                &format!(
                    "Batch too large: {} requests exceeds limit of {}",
                    batch.len(),
                    MAX_BATCH_SIZE
                ),
            );
        }
        let mut responses = Vec::with_capacity(batch.len());
        for request in batch {
            if let Some(resp) = handle_single_rpc(&dispatch, &auth, &state, request).await {
                responses.push(resp);
            }
        }
        if responses.is_empty() {
            return StatusCode::NO_CONTENT.into_response();
        }
        return Json(Value::Array(responses)).into_response();
    }

    // Single request.
    match handle_single_rpc(&dispatch, &auth, &state, &body).await {
        Some(resp) => Json(resp).into_response(),
        // Notification (no id) -- spec says return 202 Accepted.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Dispatches one JSON-RPC request. Returns `None` for notifications (no id).
/// For `tools/call`, charges rate-limit tokens proportional to the endpoint
/// cost of the dispatched route.
async fn handle_single_rpc(
    dispatch: &Router,
    auth: &AuthContext,
    state: &AppState,
    req: &Value,
) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = match req.get("method").and_then(Value::as_str) {
        Some(m) => m,
        None => return id.map(|id| rpc_error(id, -32600, "Invalid Request: missing method")),
    };

    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => {
            let id = id?;
            Some(rpc_ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "kleos",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Kleos memory system. Use memory_search or memory_store for core operations. Use context_build for assembled context."
                }),
            ))
        }
        "notifications/initialized" => None,
        "ping" => {
            let id = id?;
            Some(rpc_ok(id, json!({})))
        }
        "tools/list" => {
            let id = id?;
            Some(rpc_ok(id, json!({ "tools": kleos_mcp::tools::registry() })))
        }
        "tools/call" => {
            let id = id?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            // Charge per-tool rate-limit tokens before dispatch.
            if let Some(route) = find_by_name(&name) {
                let http_method = match route.method {
                    Method::Get => axum::http::Method::GET,
                    Method::Post => axum::http::Method::POST,
                    Method::Put => axum::http::Method::PUT,
                    Method::Delete => axum::http::Method::DELETE,
                    Method::Patch => axum::http::Method::PATCH,
                };
                let cost = crate::middleware::rate_limit::endpoint_cost(route.path, &http_method);
                let key = format!("user:{}", auth.user_id);
                let limit = auth.key.rate_limit as i64;
                match kleos_lib::ratelimit::check_and_increment_by(&state.db, &key, limit, 60, cost)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        return Some(rpc_ok(
                            id,
                            json!({
                                "content": [{ "type": "text", "text": "Rate limit exceeded for this tool" }],
                                "isError": true
                            }),
                        ));
                    }
                    Err(e) => {
                        tracing::error!(tool = %name, error = %e, "rate-limit check failed during MCP dispatch");
                        return Some(rpc_ok(
                            id,
                            json!({
                                "content": [{ "type": "text", "text": "Rate limit backend unavailable" }],
                                "isError": true
                            }),
                        ));
                    }
                }
            }

            match dispatch_tool(dispatch, auth, &name, arguments).await {
                Ok(value) => Some(rpc_ok(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&value)
                                .unwrap_or_else(|_| value.to_string())
                        }],
                        "structuredContent": value,
                        "isError": false
                    }),
                )),
                Err(message) => Some(rpc_ok(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true
                    }),
                )),
            }
        }
        _ => id.map(|id| rpc_error(id, -32601, "Method not found")),
    }
}

/// Builds a client-safe error message for a failed internal dispatch.
/// Full details are logged server-side; the client sees only the tool name
/// and HTTP status to prevent leaking handler internals (CWE-209).
fn sanitize_dispatch_error(tool_name: &str, status_code: u16, internal_msg: &str) -> String {
    tracing::warn!(
        tool = tool_name,
        status = status_code,
        detail = internal_msg,
        "MCP tool dispatch failed"
    );
    format!("tool '{}' failed (HTTP {})", tool_name, status_code)
}

/// Dispatches a single tool call through the internal router.
///
/// Looks up the route, enforces scope, builds a synthetic HTTP request
/// with the caller's `AuthContext` pre-injected, and dispatches via
/// `Router::oneshot()`.
#[tracing::instrument(
    skip(dispatch, auth, args),
    fields(
        mcp.tool = %name,
        mcp.user_id = auth.user_id,
        mcp.scope,
        mcp.status,
    )
)]
/// Dispatches one tool call through the internal router with auth applied.
async fn dispatch_tool(
    dispatch: &Router,
    auth: &AuthContext,
    name: &str,
    mut args: Value,
) -> Result<Value, String> {
    // Secret-bearing routes (cred resolve/proxy) are dispatchable by name even
    // though tools/list omits them. Refuse them so a raw credential never lands
    // in the MCP response's structuredContent / text and, from there, in model
    // transcripts and logs.
    if kleos_client::is_mcp_blocked(name) {
        return Err(format!("tool '{name}' is not available over MCP"));
    }

    let route = find_by_name(name).ok_or_else(|| format!("unknown tool: {name}"))?;

    // Record the required scope in the span.
    tracing::Span::current().record(
        "mcp.scope",
        tracing::field::display(format_args!("{:?}", route.scope)),
    );

    // Enforce scope before dispatch.
    let required = route_scope_to_auth_scope(route.scope);
    if !auth.has_scope(&required) {
        return Err(format!(
            "insufficient scope: tool '{}' requires {:?}",
            name, required
        ));
    }

    let path = render_path(route.path, &mut args)?;

    // Build URI: for GET/DELETE, remaining args become query parameters.
    let uri = match route.method {
        Method::Get | Method::Delete => append_query_string(&path, &args),
        _ => path.clone(),
    };

    // Build the HTTP method.
    let http_method = match route.method {
        Method::Get => axum::http::Method::GET,
        Method::Post => axum::http::Method::POST,
        Method::Put => axum::http::Method::PUT,
        Method::Delete => axum::http::Method::DELETE,
        Method::Patch => axum::http::Method::PATCH,
    };

    // Build the request body for POST/PUT/PATCH.
    let (body, content_type) = match route.method {
        Method::Post | Method::Put | Method::Patch => {
            let bytes = serde_json::to_vec(&args).unwrap_or_default();
            (Body::from(bytes), Some("application/json"))
        }
        _ => (Body::empty(), None),
    };

    let mut builder = Request::builder().method(http_method).uri(&uri);
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }

    let mut request = builder
        .body(body)
        .map_err(|e| format!("failed to build internal request: {e}"))?;

    // Inject pre-validated AuthContext so handler extractors find it.
    request.extensions_mut().insert(auth.clone());

    // Dispatch through the internal router (no middleware, just handlers).
    let response = dispatch
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("internal dispatch failed: {e}"))?;

    let status = response.status();
    tracing::Span::current().record("mcp.status", status.as_u16() as i64);
    let body_bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| format!("failed to read internal response: {e}"))?;

    let parsed: Result<Value, _> = serde_json::from_slice(&body_bytes);
    if status.is_success() {
        parsed.or_else(|_| {
            let text = String::from_utf8_lossy(&body_bytes).into_owned();
            Ok(json!({ "content": text }))
        })
    } else {
        let internal_msg = parsed
            .as_ref()
            .ok()
            .and_then(|b| {
                b.get("error")
                    .or_else(|| b.get("message"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| {
                let s = String::from_utf8_lossy(&body_bytes);
                if s.len() > 512 {
                    format!(
                        "{}... ({} bytes)",
                        kleos_lib::validation::truncate_on_char_boundary(s.as_ref(), 512),
                        body_bytes.len()
                    )
                } else {
                    s.into_owned()
                }
            });
        Err(sanitize_dispatch_error(
            name,
            status.as_u16(),
            &internal_msg,
        ))
    }
}

/// Converts a route-registry scope to an auth-module scope.
fn route_scope_to_auth_scope(scope: RouteScope) -> Scope {
    match scope {
        RouteScope::Read => Scope::Read,
        RouteScope::Write => Scope::Write,
        RouteScope::Admin => Scope::Admin,
    }
}

/// Appends remaining JSON object fields as query-string parameters.
fn append_query_string(path: &str, args: &Value) -> String {
    let map = match args.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return path.to_string(),
    };
    let mut qs = String::new();
    for (k, v) in map {
        match v {
            Value::Null => continue,
            Value::Object(inner) if inner.is_empty() => continue,
            Value::Array(arr) => {
                for item in arr {
                    push_qparam(&mut qs, k, item);
                }
            }
            _ => push_qparam(&mut qs, k, v),
        }
    }
    if qs.is_empty() {
        return path.to_string();
    }
    format!("{path}?{}", qs.strip_prefix('&').unwrap_or(&qs))
}

/// Pushes one key=value pair onto the query string buffer.
fn push_qparam(qs: &mut String, key: &str, val: &Value) {
    let encoded_key = utf8_percent_encode(key, NON_ALPHANUMERIC);
    let raw = match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return,
    };
    let encoded_val = utf8_percent_encode(&raw, NON_ALPHANUMERIC);
    qs.push('&');
    qs.push_str(&encoded_key.to_string());
    qs.push('=');
    qs.push_str(&encoded_val.to_string());
}

/// Checks whether the Origin header is from a trusted source.
/// Rejects obvious cross-origin browser attacks while allowing
/// non-browser MCP clients (which don't send Origin).
fn is_allowed_origin(origin: &str) -> bool {
    let lower = origin.to_ascii_lowercase();

    if is_loopback_origin(&lower) {
        return true;
    }

    // Allow origins from the configured ENGRAM_ALLOWED_ORIGINS.
    if let Ok(allowed) = kleos_lib::kleos_env("ALLOWED_ORIGINS") {
        for allowed_origin in allowed.split(',').map(str::trim) {
            if lower == allowed_origin.to_ascii_lowercase() {
                return true;
            }
        }
    }
    false
}

/// Returns true if the origin is a loopback address (localhost, 127.0.0.1, [::1]).
/// Matches the exact authority only -- "http://localhost.evil.com" is rejected.
fn is_loopback_origin(lower: &str) -> bool {
    const LOOPBACK_PREFIXES: &[&str] = &[
        "http://localhost",
        "https://localhost",
        "http://127.0.0.1",
        "https://127.0.0.1",
        "http://[::1]",
        "https://[::1]",
    ];
    for prefix in LOOPBACK_PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if rest.is_empty() {
                return true;
            }
            // Only allow a numeric port suffix (":NNNN"), reject subdomains
            // like "localhost:3000.evil.com".
            if let Some(port_str) = rest.strip_prefix(':') {
                if !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit()) {
                    return true;
                }
            }
        }
    }
    false
}

// -- JSON-RPC helpers -------------------------------------------------------

/// Builds a successful JSON-RPC 2.0 response.
fn rpc_ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Builds a JSON-RPC 2.0 error response.
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Builds an HTTP response wrapping a JSON-RPC error (for pre-dispatch failures).
fn json_rpc_error_response(id: Option<Value>, code: i64, message: &str) -> Response {
    let id = id.unwrap_or(Value::Null);
    Json(rpc_error(id, code, message)).into_response()
}

/// Builds a bounded UTF-8 preview for testing and logging.
#[cfg(test)]
fn preview_utf8(text: &str, limit: usize) -> String {
    kleos_lib::validation::truncate_on_char_boundary(text, limit).to_string()
}

/// Tests the MCP routing helpers and origin guardrails.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies query string assembly for numeric arguments.
    #[test]
    fn query_string_builds_correctly() {
        let args = json!({"limit": 10, "offset": 5});
        let result = append_query_string("/list", &args);
        assert!(result.starts_with("/list?"));
        assert!(result.contains("limit=10"));
        assert!(result.contains("offset=5"));
    }

    /// Verifies null values are skipped when building a query string.
    #[test]
    fn query_string_skips_null() {
        let args = json!({"limit": 10, "filter": null});
        let result = append_query_string("/list", &args);
        assert!(result.contains("limit=10"));
        assert!(!result.contains("filter"));
    }

    /// Verifies empty argument maps return the original path.
    #[test]
    fn query_string_empty_returns_path() {
        assert_eq!(append_query_string("/list", &json!({})), "/list");
    }

    /// Verifies localhost origins are accepted.
    #[test]
    fn origin_allows_localhost() {
        assert!(is_allowed_origin("http://localhost:3000"));
        assert!(is_allowed_origin("https://127.0.0.1:8080"));
    }

    /// Verifies unknown origins are rejected.
    #[test]
    fn origin_rejects_unknown() {
        assert!(!is_allowed_origin("https://evil.example.com"));
    }

    /// Verifies route scopes map cleanly to auth scopes.
    #[test]
    fn scope_conversion() {
        assert_eq!(route_scope_to_auth_scope(RouteScope::Read), Scope::Read);
        assert_eq!(route_scope_to_auth_scope(RouteScope::Write), Scope::Write);
        assert_eq!(route_scope_to_auth_scope(RouteScope::Admin), Scope::Admin);
    }

    /// Verifies the hard batch cap stays at the configured constant.
    #[test]
    fn batch_cap_constant_is_set() {
        assert_eq!(MAX_BATCH_SIZE, 20);
    }

    /// Verifies localhost subdomains are rejected.
    #[test]
    fn origin_rejects_localhost_subdomain() {
        assert!(!is_allowed_origin("http://localhost.evil.com"));
        assert!(!is_allowed_origin("https://localhost.evil.com"));
    }

    /// Verifies port-suffixed subdomains are rejected.
    #[test]
    fn origin_rejects_port_suffixed_subdomain() {
        assert!(!is_allowed_origin("http://localhost:3000.evil.com"));
        assert!(!is_allowed_origin("https://127.0.0.1:8080.attacker.net"));
    }

    /// Verifies non-numeric ports are rejected.
    #[test]
    fn origin_rejects_non_numeric_port() {
        assert!(!is_allowed_origin("http://localhost:abc"));
        assert!(!is_allowed_origin("http://localhost:"));
    }

    /// Verifies bare localhost origins are accepted.
    #[test]
    fn origin_allows_localhost_bare() {
        assert!(is_allowed_origin("http://localhost"));
        assert!(is_allowed_origin("https://localhost"));
    }

    /// Verifies IPv6 loopback origins are accepted.
    #[test]
    fn origin_allows_ipv6_loopback_with_port() {
        assert!(is_allowed_origin("http://[::1]:3000"));
        assert!(is_allowed_origin("http://[::1]"));
    }

    /// Verifies internal dispatch errors do not leak backend details.
    #[test]
    fn error_response_is_sanitized() {
        let sanitized = sanitize_dispatch_error(
            "test_tool",
            500,
            "SQLITE_BUSY: database is locked at /data/tenants/42/kleos.db",
        );
        assert_eq!(sanitized, "tool 'test_tool' failed (HTTP 500)");
        assert!(!sanitized.contains("SQLITE"));
        assert!(!sanitized.contains("/data"));
    }

    /// Verifies multibyte output previews truncate without panicking.
    #[test]
    fn preview_utf8_truncates_multibyte_text() {
        let text = "💥".repeat(200);
        let preview = preview_utf8(&text, 512);
        assert!(preview.len() <= 512);
        assert!(text.starts_with(&preview));
    }
}
