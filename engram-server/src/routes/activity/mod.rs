use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::brain_absorber::absorb_activity_to_brain;
use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::activity::{process_activity, ActivityReport};

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new().route("/activity", post(report_activity))
}

async fn report_activity(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ActivityReport>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let memory_id = process_activity(&state.db, &body, auth.user_id).await?;

    // Brain absorption: fire-and-forget, best-effort, never fails the response.
    // Requires AppState brain + embedder which are not available in engram-lib.
    if let Some(brain) = state.brain.clone() {
        let embedder = state.embedder.clone();
        let content = if let Some(ref project) = body.project {
            format!(
                "Agent {} [{}] (project: {}): {}",
                body.agent, body.action, project, body.summary
            )
        } else {
            format!("Agent {} [{}]: {}", body.agent, body.action, body.summary)
        };
        let category = if body.action.starts_with("task.") {
            "task".to_string()
        } else {
            "activity".to_string()
        };
        let importance: f64 = match body.action.as_str() {
            "task.completed" => 6.0,
            "task.blocked" | "error.raised" => 7.0,
            _ => 4.0,
        };
        let source = body.agent.clone();

        tokio::spawn(async move {
            absorb_activity_to_brain(
                brain, embedder, memory_id, content, category, importance, source,
            )
            .await;
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": memory_id })),
    ))
}
