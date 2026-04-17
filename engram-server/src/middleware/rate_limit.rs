use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use engram_lib::auth::AuthContext;
use engram_lib::ratelimit;
use std::net::SocketAddr;

use crate::state::AppState;

const OPEN_PATHS: &[&str] = &["/health", "/live", "/ready", "/bootstrap"];
const PREAUTH_IP_LIMIT: i64 = 60;

fn too_many_requests(retry_after: i64) -> Response {
    let body = serde_json::json!({
        "error": "Rate limit exceeded.",
        "retry_after": retry_after,
    });
    axum::response::Response::builder()
        .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
        .header("Content-Type", "application/json")
        .header("Retry-After", retry_after.to_string())
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
}

// -- Per-endpoint cost multipliers (3.16) ------------------------------------
//
// Expensive operations consume more rate-limit tokens per request.
// This prevents a caller from burning all their budget on LLM-heavy
// endpoints while keeping cheap reads affordable.

/// Return the cost multiplier for a given request path and method.
/// Default cost is 1 for reads, 2 for writes.
fn endpoint_cost(path: &str, method: &axum::http::Method) -> i64 {
    // Context assembly -- involves search + embedding + LLM inference
    if path.starts_with("/context") {
        return 5;
    }
    // Batch operations -- up to 100 sub-ops
    if path.starts_with("/batch") {
        return 10;
    }
    // Ingestion -- embedding + chunking
    if path.starts_with("/ingest") {
        return 3;
    }
    // Search -- embedding + reranking
    if path.starts_with("/search") || path.starts_with("/memories/search") {
        return 2;
    }
    // Graph pagerank recompute
    if path.starts_with("/graph/pagerank") && *method == axum::http::Method::POST {
        return 3;
    }
    // Store/update memory -- embedding + indexing
    if path.starts_with("/memories")
        && (*method == axum::http::Method::POST || *method == axum::http::Method::PUT)
    {
        return 2;
    }
    // Prometheus metrics scrape -- cheap but shouldn't be called rapidly
    if path.starts_with("/metrics") {
        return 1;
    }
    // Default: reads cost 1, writes cost 2
    match *method {
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS => 1,
        _ => 2,
    }
}

/// SECURITY: extract client IP for rate-limit keying.
///
/// 1. Always read the real TCP peer address from ConnectInfo.
/// 2. Only honour X-Forwarded-For when the peer IP is in the configured
///    trusted_proxies list. This prevents direct clients from spoofing
///    arbitrary rate-limit keys via XFF headers.
/// 3. If ConnectInfo is unavailable (should not happen after the serve
///    fix), fall back to "unknown" -- but never trust XFF in that case.
fn client_ip_key(request: &Request, trusted_proxies: &[String]) -> String {
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    let ip = match &peer_ip {
        Some(peer)
            if !trusted_proxies.is_empty() && trusted_proxies.iter().any(|tp| tp == peer) =>
        {
            // Peer is a trusted reverse proxy -- use first XFF hop.
            request
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(String::from)
                .unwrap_or_else(|| peer.clone())
        }
        Some(peer) => {
            // Direct client or untrusted proxy -- use real peer IP.
            peer.clone()
        }
        None => {
            tracing::warn!(
                "ConnectInfo<SocketAddr> not available; rate-limit key will be \"unknown\""
            );
            "unknown".to_string()
        }
    };

    format!("ip:{}", ip)
}

#[tracing::instrument(skip_all, fields(middleware = "server.preauth_rate_limit"))]
pub async fn preauth_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    if OPEN_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{}/", p)))
    {
        return next.run(request).await;
    }

    let key = client_ip_key(&request, &state.config.trusted_proxies);
    match ratelimit::check_and_increment(&state.db, &key, PREAUTH_IP_LIMIT, 60).await {
        Ok(true) => next.run(request).await,
        Ok(false) => too_many_requests(60),
        Err(e) => {
            tracing::error!("preauth rate_limit check failed for {}: {}", key, e);
            let body = serde_json::json!({
                "error": "Rate limit backend unavailable. Retry shortly.",
                "retry_after": 5,
            });
            axum::response::Response::builder()
                .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .header("Retry-After", "5")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
        }
    }
}

/// Axum middleware implementing per-user sliding-window rate limiting.
///
/// Uses the DB-backed rate limiter from engram-lib. The limit (requests/minute)
/// is read from the authenticated API key's `rate_limit` field.
///
/// Per-endpoint cost multipliers (3.16) make expensive operations (context,
/// batch, ingest) consume more rate-limit tokens than cheap reads.
///
/// Returns HTTP 429 with a `Retry-After` header when the limit is exceeded.
/// Open paths and unauthenticated requests bypass the limiter.
#[tracing::instrument(skip_all, fields(middleware = "server.rate_limit"))]
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip rate limiting for health/bootstrap paths.
    if OPEN_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{}/", p)))
    {
        return next.run(request).await;
    }

    let auth_ctx = request.extensions().get::<AuthContext>().cloned();

    let (user_id, limit) = match auth_ctx {
        Some(ctx) => (ctx.user_id, ctx.key.rate_limit as i64),
        // Unauthenticated requests are handled by auth middleware; pass through here.
        None => return next.run(request).await,
    };

    let key = format!("user:{}", user_id);
    let cost = endpoint_cost(&path, request.method());

    match ratelimit::check_and_increment_by(&state.db, &key, limit, 60, cost).await {
        Ok(true) => next.run(request).await,
        Ok(false) => too_many_requests(60),
        Err(e) => {
            // SECURITY: fail CLOSED on backend errors for authenticated
            // requests. Previously we passed the request through on error,
            // which turned any flaky query into a rate-limit bypass: an
            // attacker could intentionally poison the rate_limits table (e.g.
            // via heavy write contention) to get unlimited throughput.
            tracing::error!("rate_limit check failed for {}: {}", key, e);
            let body = serde_json::json!({
                "error": "Rate limit backend unavailable. Retry shortly.",
                "retry_after": 5,
            });
            axum::response::Response::builder()
                .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .header("Retry-After", "5")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
        }
    }
}
