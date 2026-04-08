use axum::{routing::{get, post}, extract::{State, Query}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::extractors::Auth;
use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/prompt", get(get_prompt))
        .route("/prompt/generate", post(generate_prompt_handler))
        .route("/header", post(post_header))
}

#[derive(Deserialize)]
struct PromptQuery {
    format: Option<String>,
    tokens: Option<usize>,
    context: Option<String>,
}

async fn get_prompt(
    Auth(auth): Auth, State(state): State<AppState>, Query(q): Query<PromptQuery>,
) -> Result<Json<Value>, AppError> {
    let format = q.format.as_deref().unwrap_or("raw");
    let budget = q.tokens.unwrap_or(4000).clamp(100, 128000);
    let context = q.context.as_deref().unwrap_or("");
    let result = engram_lib::prompts::generate_prompt(&state.db, format, budget, context, auth.user_id).await?;
    Ok(Json(json!({
        "prompt": result.prompt,
        "format": result.format,
        "memories_included": result.memories_included,
        "tokens_estimated": result.tokens_estimated,
    })))
}

#[derive(Deserialize)]
struct GenerateBody {
    agent: Option<String>,
    task: Option<String>,
    max_tokens: Option<usize>,
    format: Option<String>,
    context: Option<String>,
}

async fn generate_prompt_handler(
    Auth(auth): Auth, State(state): State<AppState>, Json(body): Json<GenerateBody>,
) -> Result<(axum::http::StatusCode, Json<Value>), AppError> {
    let fmt = body.format.as_deref().unwrap_or("raw");
    let budget = body.max_tokens.unwrap_or(4000).clamp(100, 128000);
    let agent = body.agent.as_deref().unwrap_or("unknown");
    let context = match (&body.context, &body.task) {
        (Some(ctx), _) => ctx.clone(),
        (None, Some(task)) => format!("Task: {}", task),
        (None, None) => String::new(),
    };
    tracing::info!(agent = agent, user_id = auth.user_id, "generate_prompt_handler called");
    let result = engram_lib::prompts::generate_prompt(&state.db, fmt, budget, &context, auth.user_id).await?;
    Ok((axum::http::StatusCode::OK, Json(json!({
        "prompt": result.prompt,
        "format": result.format,
        "memories_included": result.memories_included,
        "tokens_estimated": result.tokens_estimated,
        "agent": agent,
    }))))
}

#[derive(Deserialize)]
struct HeaderBody {
    actor_model: Option<String>,
    actor_role: Option<String>,
    context: Option<String>,
    limit: Option<usize>,
}

async fn post_header(
    Auth(auth): Auth, State(state): State<AppState>, Json(body): Json<HeaderBody>,
) -> Result<Json<Value>, AppError> {
    let actor_model = body.actor_model.as_deref().unwrap_or("unknown");
    let actor_role = body.actor_role.as_deref().unwrap_or("assistant");
    let context = body.context.as_deref().unwrap_or("");
    let limit = body.limit.unwrap_or(10).min(30);
    let result = engram_lib::prompts::generate_header(&state.db, actor_model, actor_role, context, limit, auth.user_id).await?;
    Ok(Json(json!({
        "header": result.header,
        "text": result.text,
        "actor_model": result.actor_model,
        "prior_models": result.prior_models,
    })))
}
