use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::EngError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/onboard/status", get(status_handler))
        .route("/onboard/bootstrap", post(bootstrap_handler))
}

#[derive(Debug, Deserialize)]
struct BootstrapBody {
    username: Option<String>,
    default_space: Option<String>,
}

async fn status_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
            libsql::params![auth.user_id],
        )
        .await
        .map_err(EngError::Database)?;
    let count = rows
        .next()
        .await
        .map_err(EngError::Database)?
        .map(|r| r.get::<i64>(0).map_err(EngError::Database))
        .transpose()?
        .unwrap_or(0);
    Ok(Json(json!({
        "user_id": auth.user_id,
        "space_count": count,
        "is_bootstrapped": count > 0
    })))
}

async fn bootstrap_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<BootstrapBody>,
) -> Result<Json<Value>, AppError> {
    if let Some(username) = body.username {
        state
            .db
            .conn
            .execute(
                "UPDATE users SET username = ?1 WHERE id = ?2",
                libsql::params![username, auth.user_id],
            )
            .await
            .map_err(EngError::Database)?;
    }

    let space = body.default_space.unwrap_or_else(|| "default".to_string());
    state
        .db
        .conn
        .execute(
            "INSERT OR IGNORE INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3)",
            libsql::params![auth.user_id, space, "Default workspace"],
        )
        .await
        .map_err(EngError::Database)?;

    Ok(Json(json!({ "bootstrapped": true, "user_id": auth.user_id })))
}
