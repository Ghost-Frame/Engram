use axum::{
    extract::ws::{Message, WebSocket},
    extract::{Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::{AppState, SessionBroadcast};
use engram_lib::sessions::{
    append_output, create_session, get_session, get_session_output, list_sessions,
    SessionCreateRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/sessions",
            get(list_sessions_handler).post(create_session_handler),
        )
        .route("/sessions/{id}", get(get_session_handler))
        .route("/sessions/{id}/append", post(append_handler))
        .route("/sessions/{id}/stream", get(stream_handler))
}

async fn create_session_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SessionCreateRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let session = create_session(&state.db, &body, auth.user_id).await?;

    // Register in-memory broadcast state keyed by (user_id, session_id) so
    // tenants cannot collide on an opaque session id (MT-F10).
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(
            (auth.user_id, session.id.clone()),
            Arc::new(tokio::sync::Mutex::new(SessionBroadcast::new())),
        );
    }

    Ok((StatusCode::CREATED, Json(json!(session))))
}

#[derive(Deserialize)]
struct ListSessionsParams {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn list_sessions_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListSessionsParams>,
) -> Result<Json<Value>, AppError> {
    let sessions: Vec<engram_lib::sessions::SessionInfo> =
        list_sessions(&state.db, auth.user_id, params.limit, params.offset).await?;
    Ok(Json(
        json!({ "sessions": sessions, "count": sessions.len() }),
    ))
}

async fn get_session_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let session = get_session(&state.db, &id, auth.user_id).await?;
    let output = get_session_output(&state.db, &id, auth.user_id).await?;
    Ok(Json(json!({ "session": session, "output": output })))
}

#[derive(Deserialize)]
struct AppendBody {
    line: String,
}

async fn append_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
    Json(body): Json<AppendBody>,
) -> Result<Json<Value>, AppError> {
    append_output(&state.db, &id, &body.line, auth.user_id).await?;

    // Broadcast to any WebSocket subscribers (scoped to this tenant only).
    {
        let sessions = state.sessions.read().await;
        if let Some(broadcast) = sessions.get(&(auth.user_id, id.clone())) {
            let mut b = broadcast.lock().await;
            const MAX_BUFFER: usize = 10_000;
            if b.buffer.len() >= MAX_BUFFER {
                b.buffer.remove(0); // drop oldest
            }
            b.buffer.push(body.line.clone());
            let _ = b.tx.send(body.line);
        }
    }

    Ok(Json(json!({ "ok": true })))
}

async fn stream_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state, id, auth.user_id))
}

async fn handle_ws(mut socket: WebSocket, state: AppState, session_id: String, user_id: i64) {
    // Verify the caller actually owns this session before attaching a
    // broadcast subscriber. Without this check a tenant with a valid API
    // key could stream another tenant's session by guessing the id
    // (MT-F10). We hit the DB here because the in-memory map might miss
    // and we still want DB fallback to run.
    if get_session(&state.db, &session_id, user_id).await.is_err() {
        let _ = socket
            .send(Message::Text(
                json!({"type": "session_end", "status": "not_found"})
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    }

    // Verify session exists and get buffered output
    let (buffered, rx) = {
        let sessions = state.sessions.read().await;
        match sessions.get(&(user_id, session_id.clone())) {
            Some(broadcast) => {
                let b = broadcast.lock().await;
                (b.buffer.clone(), b.tx.subscribe())
            }
            None => {
                // Session not in memory -- send buffered from DB and close
                drop(sessions);
                if let Ok(lines) = get_session_output(&state.db, &session_id, user_id).await {
                    for line in lines {
                        let msg = json!({"type": "output", "data": line});
                        if socket
                            .send(Message::Text(msg.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                let _ = socket
                    .send(Message::Text(
                        json!({"type": "session_end", "status": "closed"})
                            .to_string()
                            .into(),
                    ))
                    .await;
                return;
            }
        }
    };

    // Send buffered output first
    for line in buffered {
        let msg = json!({"type": "output", "data": line});
        if socket
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Stream new output
    // DOS-L3: ping every 30s, close after 10min idle, hard cap at 1h total.
    let session_start = tokio::time::Instant::now();
    const MAX_SESSION: std::time::Duration = std::time::Duration::from_secs(3600);
    const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
    const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

    let mut rx = rx;
    let mut last_activity = tokio::time::Instant::now();
    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    // Skip the immediate first tick so we don't ping before the session starts.
    ping_tick.tick().await;

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(line) => {
                        last_activity = tokio::time::Instant::now();
                        let out = json!({"type": "output", "data": line});
                        if socket
                            .send(Message::Text(out.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let _ = socket
                            .send(Message::Text(
                                json!({"type": "session_end"}).to_string().into(),
                            ))
                            .await;
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let _ = socket.send(Message::Text(
                            json!({"type": "warning", "message": format!("lagged: missed {} messages", n)}).to_string().into()
                        )).await;
                    }
                }
            }
            _ = ping_tick.tick() => {
                let now = tokio::time::Instant::now();
                if now.duration_since(session_start) >= MAX_SESSION {
                    let _ = socket.send(Message::Text(
                        json!({"type": "session_end", "status": "max_duration_reached"}).to_string().into()
                    )).await;
                    return;
                }
                if now.duration_since(last_activity) >= IDLE_TIMEOUT {
                    let _ = socket.send(Message::Text(
                        json!({"type": "session_end", "status": "idle_timeout"}).to_string().into()
                    )).await;
                    return;
                }
                // Send keepalive ping; ignore error (client may have closed).
                // Empty ping frame -- just a keepalive; tokio_util re-exports Bytes.
                let _ = socket.send(Message::Ping(axum::body::Bytes::new())).await;
            }
        }
    }
}
