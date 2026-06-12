use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::brain_absorber::absorb_activity_to_brain;
use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::activity::{process_activity, ActivityReport};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/activity", post(report_activity))
        .layer(DefaultBodyLimit::max(16_384)) // 16 KB for activity payloads
}

async fn report_activity(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(mut body): Json<ActivityReport>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let caller_user_id = auth.effective_user_id();
    // Attribute to the authenticated caller when the client omits `agent` (the
    // documented action+summary quick-call), so the minimal MCP form succeeds
    // instead of failing validation. Fall back to the key id if the key is
    // unnamed.
    if body.agent.trim().is_empty() {
        body.agent = if auth.key.name.trim().is_empty() {
            format!("key:{}", auth.key.id)
        } else {
            auth.key.name.clone()
        };
    }
    let memory_id = process_activity(&db, &body, caller_user_id).await?;

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
                    brain, embedder, caller_user_id, memory_id, content, category, importance, source,
                ) => {}
            }
        });
    }

    // Thymus session-end judge: when an agent reports session.end, score its
    // recent persisted work (Broca actions) against the rubrics, off the
    // request path. Best-effort; never affects the response.
    if state.config.thymus_autoeval_enabled && body.action == "session.end" {
        let agent = body.agent.clone();
        let session_id = body
            .details
            .as_ref()
            .and_then(|d| d.get("session_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{agent}-session"));
        let task_desc = body.summary.clone();
        let db_judge = db.clone();
        let min_turns = state.config.thymus_autoeval_min_turns;
        tokio::spawn(async move {
            // Gather the agent's recent actions as the transcript window.
            let actions = match kleos_lib::services::broca::query_actions(
                &db_judge,
                Some(&agent),
                None,
                None,
                None,
                50,
                0,
                caller_user_id,
            )
            .await
            {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(error = %e, "thymus judge: query_actions failed");
                    return;
                }
            };
            let turn_count = actions.len() as i32;
            let mut transcript = String::new();
            for a in &actions {
                let line = a.narrative.clone().unwrap_or_else(|| a.payload.to_string());
                transcript.push_str(&format!("[{}] {}\n", a.action, line));
            }
            if !kleos_lib::intelligence::judge::judge_gate(turn_count, &transcript, min_turns) {
                return;
            }
            let input = kleos_lib::intelligence::judge::JudgeInput {
                session_id,
                agent: agent.clone(),
                task: task_desc,
                transcript,
                turn_count,
                user_id: caller_user_id,
            };
            let llm = kleos_lib::intelligence::judge::RealJudgeLlm;
            if let Err(e) =
                kleos_lib::intelligence::judge::judge_session(&db_judge, &llm, input).await
            {
                tracing::warn!(error = %e, "thymus judge failed");
            }
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": memory_id })),
    ))
}
