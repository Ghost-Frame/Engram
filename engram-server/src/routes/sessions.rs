use axum::{
    extract::ws::{Message, WebSocket},
    extract::{Path, State, WebSocketUpgrade},
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

    // Register in-memory broadcast state
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(
            session.id.clone(),
            Arc::new(tokio::sync::Mutex::new(SessionBroadcast::new())),
        );
    }

    Ok((StatusCode::CREATED, Json(json!(session))))
}

async fn list_sessions_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let sessions: Vec<engram_lib::sessions::SessionInfo> =
        list_sessions(&state.db, auth.user_id).await?;
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

    // Broadcast to any WebSocket subscribers
    {
        let sessions = state.sessions.read().await;
        if let Some(broadcast) = sessions.get(&id) {
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
    // Verify session exists and get buffered output
    let (buffered, rx) = {
        let sessions = state.sessions.read().await;
        match sessions.get(&session_id) {
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
    let mut rx = rx;
    loop {
        match rx.recv().await {
            Ok(line) => {
                let msg = json!({"type": "output", "data": line});
                if socket
                    .send(Message::Text(msg.to_string().into()))
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
}
