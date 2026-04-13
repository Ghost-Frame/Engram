use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::gate::{
    check_command_with_context, cleanup_expired_approvals, complete_gate, respond_to_gate,
    GateCheckRequest, PendingApproval, APPROVAL_TIMEOUT_SECS, TOOLS_REQUIRING_APPROVAL,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gate/check", post(check_handler))
        .route("/gate/respond", post(respond_handler))
        .route("/gate/complete", post(complete_handler))
        // Alias for parity with original engram
        .route("/guard", post(guard_handler))
}

async fn check_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GateCheckRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let resolved_command = state
        .credd
        .resolve_text(&state.db, auth.user_id, &body.agent, &body.command)
        .await?;
    let mut resolved_patterns = Vec::new();
    for pattern in &state.config.eidolon.gate.blocked_patterns {
        resolved_patterns.push(
            state
                .credd
                .resolve_text(&state.db, auth.user_id, &body.agent, pattern)
                .await?,
        );
    }
    let result = check_command_with_context(
        &state.db,
        &body,
        auth.user_id,
        Some(&resolved_command),
        &resolved_patterns,
        &state.config,
    )
    .await?;

    // If the command was allowed (not blocked, not pending secrets), and the tool
    // requires human approval, pause here and wait for a decision via /gate/respond.
    if result.allowed && !result.requires_approval {
        let tool_name = body.tool_name.as_deref().unwrap_or("");
        if TOOLS_REQUIRING_APPROVAL.contains(&tool_name) {
            let gate_id = result.gate_id;
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();

            {
                let mut approvals = state.pending_approvals.lock().await;
                // Prune stale entries while we have the lock.
                cleanup_expired_approvals(&mut approvals);
                approvals.insert(
                    gate_id,
                    (
                        PendingApproval {
                            gate_id,
                            agent: body.agent.clone(),
                            tool_name: tool_name.to_string(),
                            command: body.command.clone(),
                            created_at: std::time::Instant::now(),
                        },
                        tx,
                    ),
                );
            }

            // Notify any watchers (e.g. TUI) that a new approval is pending.
            if let Some(ref notify) = state.approval_notify {
                let _ = notify.send(());
            }

            let approved = match tokio::time::timeout(
                std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(decision)) => decision,
                _ => {
                    // Timeout or channel dropped -- clean up and deny.
                    let mut approvals = state.pending_approvals.lock().await;
                    approvals.remove(&gate_id);
                    false
                }
            };

            // Ensure the entry is removed after a decision.
            {
                let mut approvals = state.pending_approvals.lock().await;
                approvals.remove(&gate_id);
            }

            if approved {
                tracing::info!(
                    "gate: APPROVED by user gate_id={} tool={} agent={}",
                    gate_id, tool_name, body.agent
                );
                return Ok((StatusCode::CREATED, Json(json!(result))));
            } else {
                tracing::warn!(
                    "gate: DENIED/TIMEOUT gate_id={} tool={} agent={}",
                    gate_id, tool_name, body.agent
                );
                let denied_result = engram_lib::gate::GateCheckResult {
                    allowed: false,
                    reason: Some(format!("{} denied -- approval timed out or rejected", tool_name)),
                    resolved_command: result.resolved_command.clone(),
                    gate_id,
                    requires_approval: false,
                    enrichment: None,
                };
                return Ok((StatusCode::CREATED, Json(json!(denied_result))));
            }
        }
    }

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
    // Signal the waiting check_handler (if still waiting) through the oneshot channel.
    {
        let mut approvals = state.pending_approvals.lock().await;
        if let Some((_, tx)) = approvals.remove(&body.gate_id) {
            let _ = tx.send(body.approved);
        }
    }

    let result = respond_to_gate(
        &state.db,
        body.gate_id,
        body.approved,
        body.reason.as_deref(),
        auth.user_id,
    )
    .await?;
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
    complete_gate(
        &state.db,
        body.gate_id,
        &body.output,
        &body.known_secrets,
        auth.user_id,
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct GuardBody {
    action: String,
}

/// Simple guard endpoint that checks if an action conflicts with high-importance static rules.
/// This is a simplified version without LLM integration - it only does keyword matching.
async fn guard_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GuardBody>,
) -> Result<Json<Value>, AppError> {
    if body.action.trim().is_empty() {
        return Err(AppError::from(engram_lib::EngError::InvalidInput(
            "action (string) required - describe what you are about to do".into(),
        )));
    }

    // Search for high-importance static memories that might conflict
    let user_id = auth.user_id;
    let rules: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, importance FROM memories
                 WHERE user_id = ?1 AND is_static = 1 AND importance >= 8 AND is_forgotten = 0
                 ORDER BY importance DESC LIMIT 20",
            )?;
            let rows = stmt.query_map(params![user_id], |row| {
                let id: i64 = row.get(0)?;
                let content: String = row.get(1)?;
                let importance: i64 = row.get(2)?;
                Ok((id, content, importance))
            })?;
            let mut rules = Vec::new();
            for row in rows {
                let (id, content, importance) = row?;
                rules.push(json!({
                    "id": id,
                    "content": content,
                    "importance": importance,
                }));
            }
            Ok(rules)
        })
        .await?;

    if rules.is_empty() {
        return Ok(Json(json!({
            "signal": "allow",
            "action": body.action,
            "rules": [],
            "message": "No conflicting rules found.",
        })));
    }

    // Simple heuristic: if any rule contains prohibition keywords and the action
    // contains related terms, warn. Without LLM, we can't do semantic matching.
    let action_lower = body.action.to_lowercase();
    let prohibition_keywords = [
        "never",
        "don't",
        "do not",
        "must not",
        "prohibited",
        "forbidden",
    ];

    let mut matched_rules = Vec::new();
    for rule in &rules {
        let content = rule["content"].as_str().unwrap_or("").to_lowercase();
        let has_prohibition = prohibition_keywords.iter().any(|k| content.contains(k));

        // Very basic: check if any significant word from the action appears in the rule
        let action_words: Vec<&str> = action_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();
        let has_overlap = action_words.iter().any(|w| content.contains(w));

        if has_prohibition && has_overlap {
            matched_rules.push(rule.clone());
        }
    }

    let (signal, message) = if matched_rules.is_empty() {
        (
            "allow",
            "No direct conflicts detected. Note: LLM-based semantic matching not available.",
        )
    } else {
        (
            "warn",
            "Potential rule conflicts detected. Review before proceeding.",
        )
    };

    Ok(Json(json!({
        "signal": signal,
        "action": body.action,
        "rules": matched_rules,
        "message": message,
    })))
}
