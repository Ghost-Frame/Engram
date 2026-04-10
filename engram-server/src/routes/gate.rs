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
        // Alias for parity with original engram
        .route("/guard", post(guard_handler))
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
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT id, content, importance FROM memories
             WHERE user_id = ?1 AND is_static = 1 AND importance >= 8 AND is_forgotten = 0
             ORDER BY importance DESC LIMIT 20",
            libsql::params![auth.user_id],
        )
        .await
        .map_err(|e| AppError::from(engram_lib::EngError::Database(e)))?;

    let mut rules = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError::from(engram_lib::EngError::Database(e)))?
    {
        let id: i64 = row.get(0).unwrap_or(0);
        let content: String = row.get(1).unwrap_or_default();
        let importance: i64 = row.get(2).unwrap_or(0);
        rules.push(json!({
            "id": id,
            "content": content,
            "importance": importance,
        }));
    }

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
    let prohibition_keywords = ["never", "don't", "do not", "must not", "prohibited", "forbidden"];

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
        ("allow", "No direct conflicts detected. Note: LLM-based semantic matching not available.")
    } else {
        ("warn", "Potential rule conflicts detected. Review before proceeding.")
    };

    Ok(Json(json!({
        "signal": signal,
        "action": body.action,
        "rules": matched_rules,
        "message": message,
    })))
}
