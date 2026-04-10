use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::intelligence::{
    growth::{list_observations, materialize, reflect},
    types::GrowthReflectRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/growth/reflect", post(reflect_handler))
        .route("/growth/observations", get(observations_handler))
        .route("/growth/materialize", post(materialize_handler))
}

async fn reflect_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GrowthReflectRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let result = reflect(&state.db, &body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

#[derive(Deserialize)]
struct ObservationsQuery {
    limit: Option<usize>,
}

async fn observations_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ObservationsQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let observations: Vec<engram_lib::intelligence::growth::GrowthObservation> =
        list_observations(&state.db, auth.user_id, limit).await?;
    let count = observations.len();
    Ok(Json(
        json!({ "observations": observations, "count": count }),
    ))
}

#[derive(Deserialize)]
struct MaterializeBody {
    observation_id: i64,
}

async fn materialize_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<MaterializeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let new_id = materialize(&state.db, body.observation_id, auth.user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": new_id })),
    ))
}
