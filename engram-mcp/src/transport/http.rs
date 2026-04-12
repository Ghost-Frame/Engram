#![cfg(feature = "http")]

use crate::{handle_jsonrpc, App};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use engram_lib::{EngError, Result};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tower_http::timeout::TimeoutLayer;

/// Maximum request body size for MCP HTTP: 2 MiB.
const BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Request timeout for MCP HTTP transport: 30 seconds.
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Pre-auth per-IP throttle for MCP HTTP transport.
const PREAUTH_IP_LIMIT: i64 = 60;

async fn mcp(State(app): State<App>, Json(body): Json<Value>) -> Json<Value> {
    let response = handle_jsonrpc(&app, body)
        .await
        .unwrap_or_else(|| serde_json::json!({}));
    Json(response)
}

/// Bearer token authentication middleware for MCP HTTP transport.
///
/// Requires `ENGRAM_MCP_BEARER_TOKEN` to be set when using HTTP transport.
/// The env-var fallback that stdio transport relies on is intentionally
/// mandatory here -- HTTP is network-exposed so ambient auth is never safe.
async fn bearer_auth(request: Request<Body>, next: Next) -> Response {
    let token = match std::env::var("ENGRAM_MCP_BEARER_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            tracing::error!(
                "ENGRAM_MCP_BEARER_TOKEN not set; MCP HTTP transport requires a bearer token"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "server misconfigured: bearer token not set"
                })),
            )
                .into_response();
        }
    };

    let presented = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if presented.is_empty()
        || presented.as_bytes().len() != token.as_bytes().len()
        || presented.as_bytes().ct_eq(token.as_bytes()).unwrap_u8() != 1
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid or missing bearer token" })),
        )
            .into_response();
    }

    next.run(request).await
}

async fn preauth_rate_limit(State(app): State<App>, request: Request<Body>, next: Next) -> Response {
    let key = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|ip| format!("mcp:http:{}", ip))
        .unwrap_or_else(|| "mcp:http:unknown".to_string());

    match engram_lib::ratelimit::check_and_increment(&app.db, &key, PREAUTH_IP_LIMIT, 60).await {
        Ok(true) => next.run(request).await,
        Ok(false) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "rate limit exceeded",
                "retry_after": 60
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, key = %key, "MCP HTTP rate-limit check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "rate limit backend unavailable",
                    "retry_after": 5
                })),
            )
                .into_response()
        }
    }
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/mcp", post(mcp))
        .layer(middleware::from_fn_with_state(app.clone(), preauth_rate_limit))
        .layer(middleware::from_fn(bearer_auth))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(app)
}

pub async fn serve(app: App, listen: &str) -> Result<()> {
    // Refuse to start without a bearer token configured.
    if std::env::var("ENGRAM_MCP_BEARER_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Err(EngError::Internal(
            "ENGRAM_MCP_BEARER_TOKEN must be set for HTTP transport".into(),
        ));
    }

    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| EngError::Internal(format!("invalid listen address: {e}")))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| EngError::Internal(e.to_string()))?;
    tracing::info!(addr = %addr, "MCP HTTP transport listening (auth required)");
    axum::serve(listener, router(app))
        .await
        .map_err(|e| EngError::Internal(e.to_string()))
}
