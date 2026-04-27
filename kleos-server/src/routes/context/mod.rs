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
// M-R3-007: assemble_context reads memories from the caller's DB. Using
// state.db (monolith) leaked context across tenants on a sharded
// deployment. Switching to ResolvedDb routes to the caller's shard.
async fn build_context(
    State(state): State<AppState>,
    Auth(auth): Auth,
    crate::extractors::ResolvedDb(db): crate::extractors::ResolvedDb,
    Json(body): Json<ContextOptions>,
) -> Result<Json<Value>, AppError> {
    if body.query.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "query (string) required".to_string(),
        )));
    }

    let embedder = state.embedder.read().await.clone();
    let result = assemble_context(&db, body, auth.user_id, embedder, state.llm.clone()).await?;

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
    crate::extractors::ResolvedDb(resolved_db): crate::extractors::ResolvedDb,
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
        let result = assemble_context(
            &resolved_db,
            body,
            auth.user_id,
            embedder,
            state.llm.clone(),
        )
        .await?;
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

    // R7-003: bounded channels prevent unbounded memory growth on stalled clients.
    const CHANNEL_CAP: usize = 256;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ContextProgressEvent>(CHANNEL_CAP);
    let embedder = state.embedder.read().await.clone();
    // M-R3-007: stream assembly also routes to the caller's shard.
    let db = resolved_db.clone();
    let llm = state.llm.clone();
    let user_id = auth.user_id;

    // Output channel for SSE events (progress + final result).
    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<Event>(CHANNEL_CAP);

    // Spawn assembly task. Goes on background_tasks JoinSet for clean shutdown (M-008).
    let sse_tx_clone = sse_tx.clone();
    let shutdown_asm = state.shutdown_token.clone();
    {
        let mut bg = state.background_tasks.lock().await;
        bg.spawn(async move {
            tokio::select! {
                _ = shutdown_asm.cancelled() => {
                    tracing::debug!("context SSE assembly drained on shutdown");
                }
                _ = async {
                    let result = assemble_context_streaming(&db, body, user_id, embedder, llm, tx).await;
                    match result {
                        Ok(ctx) => {
                            // R8 R-009: log rather than silently drop send errors --
                            // if the client disconnected we want that in traces.
                            if let Err(e) = sse_tx_clone
                                .send(
                                    Event::default()
                                        .event("result")
                                        .json_data(json!(ctx))
                                        .unwrap_or_else(|_| Event::default().data("{}")),
                                )
                                .await
                            {
                                tracing::debug!(error = %e, "context SSE result send failed (client gone)");
                            }
                        }
                        Err(e) => {
                            let _ = sse_tx_clone
                                .send(
                                    Event::default()
                                        .event("error")
                                        .json_data(json!({"error": e.to_string()}))
                                        .unwrap_or_else(|_| Event::default().data("{}")),
                                )
                                .await;
                        }
                    }
                } => {}
            }
        });
    }

    // Spawn relay: progress channel -> SSE events channel.
    // No semaphore needed; relay terminates when assembly task closes the channel.
    let shutdown_relay = state.shutdown_token.clone();
    {
        let mut bg = state.background_tasks.lock().await;
        bg.spawn(async move {
            tokio::select! {
                _ = shutdown_relay.cancelled() => {
                    tracing::debug!("context SSE relay drained on shutdown");
                }
                _ = async {
                    while let Some(evt) = rx.recv().await {
                        let sse_event = Event::default()
                            .event("progress")
                            .json_data(&evt)
                            .unwrap_or_else(|_| Event::default().data("{}"));
                        if sse_tx.send(sse_event).await.is_err() {
                            break;
                        }
                    }
                } => {}
            }
        });
    }

    // Adapt mpsc::UnboundedReceiver -> futures::Stream<Item = Result<Event, Infallible>>
    let stream = futures::stream::unfold(sse_rx, |mut rx| async move {
        rx.recv().await.map(|evt| (Ok::<_, Infallible>(evt), rx))
    });

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}
