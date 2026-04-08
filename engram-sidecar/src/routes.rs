use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

use crate::config::SidecarConfig;
use crate::scoring::{inject_search_context, inject_store_context};
use crate::session::SharedSession;

#[derive(Clone)]
pub struct SidecarState {
    pub config: Arc<SidecarConfig>,
    pub session: SharedSession,
    pub client: Client,
}

pub fn router() -> Router<SidecarState> {
    Router::new()
        .route("/search", post(proxy_search))
        .route("/memories/search", post(proxy_search))
        .route("/store", post(proxy_store))
        .route("/memory", post(proxy_store))
        .route("/memories", post(proxy_store))
        .route("/recall", post(proxy_recall))
        .route("/health", get(health))
        .fallback(proxy_fallback)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "kleos-sidecar" }))
}

/// Forward a search request with mode/agent context injected.
async fn proxy_search(
    State(state): State<SidecarState>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Record query in session
    if let Some(q) = body.get("query").and_then(|v| v.as_str()) {
        state.session.write().await.record_query(q.to_string());
    }

    // Inject search context
    {
        let session = state.session.read().await;
        inject_search_context(&mut body, &session.agent, &session.mode);
    }

    forward_json(&state, "/search", body).await
}

/// Forward a store request with agent metadata injected.
async fn proxy_store(
    State(state): State<SidecarState>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    {
        let session = state.session.read().await;
        inject_store_context(&mut body, &session.agent);
    }

    forward_json(&state, "/store", body).await
}

/// Forward recall request with agent filter.
async fn proxy_recall(
    State(state): State<SidecarState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    forward_json(&state, "/recall", body).await
}

/// Transparent proxy for all other routes.
async fn proxy_fallback(
    State(state): State<SidecarState>,
    req: Request<Body>,
) -> Result<Response, (StatusCode, String)> {
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let method = req.method().clone();

    let url = format!("{}{}{}", state.config.kleos_url, path, query);

    // Read body bytes, limit to 10 MiB
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|e| {
            warn!("failed to read fallback body: {}", e);
            (StatusCode::BAD_REQUEST, format!("failed to read body: {}", e))
        })?;

    let resp = state
        .client
        .request(method, &url)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("upstream error: {}", e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let resp_bytes = resp
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("upstream body error: {}", e)))?;

    Ok((
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        resp_bytes,
    )
        .into_response())
}

/// Helper: POST JSON to upstream and return the response as JSON.
async fn forward_json(
    state: &SidecarState,
    path: &str,
    body: Value,
) -> Result<Json<Value>, (StatusCode, String)> {
    let url = format!("{}{}", state.config.kleos_url, path);

    let resp = state
        .client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("upstream error: {}", e)))?;

    let status = resp.status();
    let resp_body: Value = resp
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("upstream json error: {}", e)))?;

    if status.is_success() {
        Ok(Json(resp_body))
    } else {
        Err((
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            resp_body.to_string(),
        ))
    }
}
