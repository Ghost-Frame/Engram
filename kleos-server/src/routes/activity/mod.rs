use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::brain_absorber::absorb_activity_to_brain;
use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::activity::{process_activity, ActivityReport};

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new().route("/activity", post(report_activity))
}

async fn report_activity(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<ActivityReport>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let memory_id = process_activity(&db, &body, auth.user_id).await?;

    // Brain absorption: fire-and-forget, best-effort, never fails the response.
    // Bounded by brain_absorb_sem (H-005); shutdown-propagated via shutdown_token (M-008).
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

        let permit = match state.brain_absorb_sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("brain_absorb semaphore closed; skipping background work");
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({ "ok": true, "memory_id": memory_id })),
                ));
            }
        };
        let shutdown = state.shutdown_token.clone();
        let mut bg = state.background_tasks.lock().await;
        bg.spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("background brain_absorb drained on shutdown");
                }
                _ = absorb_activity_to_brain(
                    brain, embedder, memory_id, content, category, importance, source,
                ) => {}
            }
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": memory_id })),
    ))
}
