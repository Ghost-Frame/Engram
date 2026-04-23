use axum::{
    extract::Query,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::intelligence::{
    growth::{list_observations, materialize, reflect},
    types::{GrowthObservation, GrowthReflectRequest},
};

mod types;
use types::{MaterializeBody, ObservationsQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/growth/reflect", post(reflect_handler))
        .route("/growth/observations", get(observations_handler))
        .route("/growth/materialize", post(materialize_handler))
}

async fn reflect_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<GrowthReflectRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let result = reflect(&db, &body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

async fn observations_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ObservationsQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let observations: Vec<GrowthObservation> =
        list_observations(&db, auth.user_id, limit).await?;
    let count = observations.len();
    Ok(Json(
        json!({ "observations": observations, "count": count }),
    ))
}

async fn materialize_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<MaterializeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let new_id = materialize(&db, body.observation_id, auth.user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": new_id })),
    ))
}
