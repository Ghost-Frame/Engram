#![cfg(feature = "http")]

use crate::{handle_jsonrpc, App};
use axum::{extract::State, routing::post, Json, Router};
use engram_lib::{EngError, Result};
use serde_json::Value;
use std::net::SocketAddr;

async fn mcp(State(app): State<App>, Json(body): Json<Value>) -> Json<Value> {
    let response = handle_jsonrpc(&app, body)
        .await
        .unwrap_or_else(|| serde_json::json!({}));
    Json(response)
}

pub fn router(app: App) -> Router {
    Router::new().route("/mcp", post(mcp)).with_state(app)
}

pub async fn serve(app: App, listen: &str) -> Result<()> {
    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| EngError::Internal(format!("invalid listen address: {e}")))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| EngError::Internal(e.to_string()))?;
    axum::serve(listener, router(app))
        .await
        .map_err(|e| EngError::Internal(e.to_string()))
}
