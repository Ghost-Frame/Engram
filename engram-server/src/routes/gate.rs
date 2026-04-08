use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::gate::{check_command, complete_gate, respond_to_gate, GateCheckRequest};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gate/check", post(check_handler))
        .route("/gate/respond", post(respond_handler))
        .route("/gate/complete", post(complete_handler))
}

async fn check_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GateCheckRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let result = check_command(&state.db, &body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

#[derive(Deserialize)]
struct RespondBody {
    gate_id: i64,
    approved: bool,
    reason: Option<String>,
}

async fn respond_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RespondBody>,
) -> Result<Json<Value>, AppError> {
    let result = respond_to_gate(&state.db, body.gate_id, body.approved, body.reason.as_deref(), auth.user_id).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct CompleteBody {
    gate_id: i64,
    output: String,
    #[serde(default)]
    known_secrets: Vec<String>,
}

async fn complete_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CompleteBody>,
) -> Result<Json<Value>, AppError> {
    complete_gate(&state.db, body.gate_id, &body.output, &body.known_secrets, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}
