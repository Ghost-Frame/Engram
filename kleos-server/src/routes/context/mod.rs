// ============================================================================
// CONTEXT ROUTES -- POST /context  (JSON + SSE streaming)
// ============================================================================

use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::context::{
    assemble_context, assemble_context_streaming, ContextOptions, ContextProgressEvent,
};

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/context", post(build_context))
        .route("/context/stream", post(build_context_stream))
        // S7-26: context assembly may run LLM inference + embedding; 30s cap.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        // S7-27: context query payloads are small; 64 KB is ample.
        .layer(DefaultBodyLimit::max(64 * 1024))
}

/// Standard JSON context assembly (backward-compatible).
async fn build_context(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ContextOptions>,
) -> Result<Json<Value>, AppError> {
    if body.query.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "query (string) required".to_string(),
        )));
    }

    let embedder = state.embedder.read().await.clone();
    let result =
        assemble_context(&state.db, body, auth.user_id, embedder, state.llm.clone()).await?;

    Ok(Json(json!(result)))
}

/// SSE streaming context assembly.
///
/// Emits progress events as each phase completes, then the final result:
///   data: {"type":"phase","phase":"semantic","count":12,"tokens":3400,"elapsed_ms":45}
///   data: {"type":"phase","phase":"linked","count":3,"tokens":4200,"elapsed_ms":62}
///   ...
///   data: {"type":"done","total_blocks":18,"total_tokens":5100,"elapsed_ms":120}
///   data: {"type":"result","data":{...full ContextResult...}}
async fn build_context_stream(
    State(state): State<AppState>,
    Auth(auth): Auth,
    headers: HeaderMap,
    Json(body): Json<ContextOptions>,
) -> Result<impl IntoResponse, AppError> {
    if body.query.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "query (string) required".to_string(),
        )));
    }

    // If client didn't ask for SSE, fall back to JSON (defense in depth).
    let accepts_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    if !accepts_sse {
        let embedder = state.embedder.read().await.clone();
        let result =
            assemble_context(&state.db, body, auth.user_id, embedder, state.llm.clone()).await?;
        // Wrap in SSE-style JSON so callers get a consistent shape.
        return Ok(Sse::new(futures::stream::once(async move {
            Ok::<_, Infallible>(
                Event::default()
                    .event("result")
                    .json_data(json!(result))
                    .unwrap_or_else(|_| Event::default().data("{}")),
            )
        }))
        .keep_alive(KeepAlive::default())
        .into_response());
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ContextProgressEvent>();
    let embedder = state.embedder.read().await.clone();
    let db = state.db.clone();
    let llm = state.llm.clone();
    let user_id = auth.user_id;

    // Output channel for SSE events (progress + final result).
    let (sse_tx, sse_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // Spawn assembly task.
    let sse_tx_clone = sse_tx.clone();
    tokio::spawn(async move {
        let result = assemble_context_streaming(&db, body, user_id, embedder, llm, tx).await;
        match result {
            Ok(ctx) => {
                let _ = sse_tx_clone.send(
                    Event::default()
                        .event("result")
                        .json_data(json!(ctx))
                        .unwrap_or_else(|_| Event::default().data("{}")),
                );
            }
            Err(e) => {
                let _ = sse_tx_clone.send(
                    Event::default()
                        .event("error")
                        .json_data(json!({"error": e.to_string()}))
                        .unwrap_or_else(|_| Event::default().data("{}")),
                );
            }
        }
    });

    // Spawn relay: progress channel -> SSE events channel.
    tokio::spawn(async move {
        let mut progress_rx = rx;
        while let Some(evt) = progress_rx.recv().await {
            let sse_event = Event::default()
                .event("progress")
                .json_data(&evt)
                .unwrap_or_else(|_| Event::default().data("{}"));
            if sse_tx.send(sse_event).is_err() {
                break;
            }
        }
    });

    // Adapt mpsc::UnboundedReceiver -> futures::Stream<Item = Result<Event, Infallible>>
    let stream = futures::stream::unfold(sse_rx, |mut rx| async move {
        rx.recv().await.map(|evt| (Ok::<_, Infallible>(evt), rx))
    });

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}
