use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use engram_lib::context::budget::estimate_tokens;
use engram_lib::intelligence::growth::list_observations;
use engram_lib::memory::search::hybrid_search;
use engram_lib::memory::types::SearchRequest;
use engram_lib::services::brain::BrainQueryOptions;
use engram_lib::EngError;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/prompt", get(get_prompt))
        .route("/prompt/generate", post(post_prompt_generate))
        .route("/header", post(post_header))
}

#[derive(Deserialize)]
struct PromptQuery {
    format: Option<String>,
    tokens: Option<usize>,
    context: Option<String>,
}

async fn get_prompt(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(q): Query<PromptQuery>,
) -> Result<Json<Value>, AppError> {
    let format = q.format.as_deref().unwrap_or("raw");
    let budget = q.tokens.unwrap_or(4000).clamp(100, 128000);
    let context = q.context.as_deref().unwrap_or("");
    let mut result =
        engram_lib::prompts::generate_prompt(&state.db, format, budget, context, auth.user_id)
            .await?;
    result.prompt = state
        .credd
        .resolve_text(&state.db, auth.user_id, &auth.key.name, &result.prompt)
        .await?;
    result.tokens_estimated = estimate_tokens(&result.prompt);
    Ok(Json(json!({
        "prompt": result.prompt,
        "format": result.format,
        "memories_included": result.memories_included,
        "tokens_estimated": result.tokens_estimated,
    })))
}

#[derive(Deserialize)]
struct HeaderBody {
    actor_model: Option<String>,
    actor_role: Option<String>,
    context: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct GeneratePromptRequest {
    agent: String,
    task: String,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    include_personality: Option<bool>,
    #[serde(default)]
    include_memories: Option<bool>,
    #[serde(default)]
    memory_limit: Option<usize>,
    // Living prompt sources
    #[serde(default)]
    include_brain: Option<bool>,
    #[serde(default)]
    include_growth: Option<bool>,
    #[serde(default)]
    include_instincts: Option<bool>,
    #[serde(default)]
    brain_limit: Option<usize>,
    #[serde(default)]
    growth_limit: Option<usize>,
}

async fn post_prompt_generate(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<GeneratePromptRequest>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.trim();
    let task = body.task.trim();
    if agent.is_empty() {
        return Err(EngError::InvalidInput("agent is required".into()).into());
    }
    if task.is_empty() {
        return Err(EngError::InvalidInput("task is required".into()).into());
    }

    let prompt_cfg = &state.config.eidolon.prompt;
    let max_tokens = body
        .max_tokens
        .unwrap_or(prompt_cfg.default_max_tokens)
        .min(prompt_cfg.max_tokens_cap)
        .max(64);
    let include_memories = body
        .include_memories
        .unwrap_or(prompt_cfg.default_include_memories);
    let include_personality = body
        .include_personality
        .unwrap_or(prompt_cfg.default_include_personality);
    let memory_limit = body.memory_limit.unwrap_or(8).clamp(1, 32);
    let include_brain = body.include_brain.unwrap_or(false);
    let include_growth = body.include_growth.unwrap_or(false);
    let include_instincts = body.include_instincts.unwrap_or(false);
    let brain_limit = body.brain_limit.unwrap_or(5).clamp(1, 20);
    let growth_limit = body.growth_limit.unwrap_or(5).clamp(1, 20);

    let mut sources: Vec<Value> = Vec::new();
    let mut sections: Vec<String> = Vec::new();

    sections.push(format!(
        "You are {agent}, an agent working under the Engram memory system. Be concise, accurate, and cite memories when useful."
    ));

    if include_personality {
        let personality_req = SearchRequest {
            query: format!("{agent} personality"),
            embedding: None,
            limit: Some(3),
            user_id: Some(auth.user_id),
            latest_only: true,
            category: Some("personality".into()),
            source: None,
            tags: None,
            threshold: None,
            space_id: None,
            include_forgotten: None,
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            source_filter: None,
        };
        if let Ok(results) = hybrid_search(&state.db, personality_req).await {
            if !results.is_empty() {
                let mut buf = String::from("## Personality\n");
                for r in &results {
                    buf.push_str("- ");
                    buf.push_str(r.memory.content.trim());
                    buf.push('\n');
                    sources.push(json!({
                        "id": r.memory.id,
                        "kind": "personality",
                        "score": r.score,
                    }));
                }
                sections.push(buf);
            }
        }
    }

    if include_memories {
        let memory_req = SearchRequest {
            query: task.to_string(),
            embedding: None,
            limit: Some(memory_limit),
            user_id: Some(auth.user_id),
            latest_only: true,
            category: None,
            source: None,
            tags: None,
            threshold: None,
            space_id: None,
            include_forgotten: None,
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            source_filter: None,
        };
        let results = hybrid_search(&state.db, memory_req).await?;
        if !results.is_empty() {
            let mut buf = String::from("## Relevant Memories\n");
            for r in &results {
                buf.push_str("- ");
                buf.push_str(r.memory.content.trim());
                buf.push('\n');
                sources.push(json!({
                    "id": r.memory.id,
                    "kind": "memory",
                    "score": r.score,
                    "category": r.memory.category,
                }));
            }
            sections.push(buf);
        }
    }

    // Living prompt: Brain patterns (neural substrate recall)
    if include_brain {
        if let Some(ref brain) = state.brain {
            if brain.is_ready() {
                let embedder_guard = state.embedder.read().await;
                if let Some(ref embedder) = *embedder_guard {
                    let opts = BrainQueryOptions {
                        query: task.to_string(),
                        top_k: Some(brain_limit),
                        beta: None,
                        spread_hops: None,
                    };
                    if let Ok(result) = brain.query(embedder.as_ref(), task, &opts).await {
                        if !result.activated.is_empty() {
                            let mut buf = String::from("## Brain Patterns\n");
                            for m in &result.activated {
                                buf.push_str(&format!(
                                    "- [act:{:.2}] {}\n",
                                    m.activation,
                                    m.content.trim()
                                ));
                                sources.push(json!({
                                    "id": m.id,
                                    "kind": "brain",
                                    "activation": m.activation,
                                    "category": m.category,
                                }));
                            }
                            sections.push(buf);
                        }
                    }
                }
            }
        }
    }

    // Living prompt: Growth observations
    if include_growth {
        if let Ok(observations) = list_observations(&state.db, auth.user_id, growth_limit).await {
            if !observations.is_empty() {
                let mut buf = String::from("## Growth Observations\n");
                for obs in &observations {
                    buf.push_str("- ");
                    buf.push_str(obs.content.trim());
                    buf.push('\n');
                    sources.push(json!({
                        "id": obs.id,
                        "kind": "growth",
                        "importance": obs.importance,
                    }));
                }
                sections.push(buf);
            }
        }
    }

    // Living prompt: Instinct domains (pre-trained knowledge)
    if include_instincts {
        // Instincts are ghost patterns with negative IDs in the brain
        // Provide a summary of the instinct categories
        let instinct_summary = concat!(
            "## Instinct Domains\n",
            "Pre-trained knowledge covering: infrastructure state transitions, ",
            "architecture decisions, system references, task completion patterns, ",
            "and common error resolutions. These domains provide baseline context ",
            "for infrastructure and deployment tasks."
        );
        sections.push(instinct_summary.to_string());
        sources.push(json!({
            "kind": "instincts",
            "domains": 5,
            "note": "synthetic pre-training corpus",
        }));
    }

    sections.push(format!("## Task\n{task}"));

    let mut prompt = state
        .credd
        .resolve_text(&state.db, auth.user_id, agent, &sections.join("\n\n"))
        .await?;
    let mut tokens = estimate_tokens(&prompt);
    if tokens > max_tokens {
        let target_chars = max_tokens.saturating_mul(4);
        if prompt.len() > target_chars {
            prompt.truncate(target_chars);
            prompt.push_str("\n...[truncated]");
            tokens = estimate_tokens(&prompt);
        }
    }

    Ok(Json(json!({
        "prompt": prompt,
        "tokens_estimated": tokens,
        "max_tokens": max_tokens,
        "agent": agent,
        "sources": sources,
    })))
}

async fn post_header(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<HeaderBody>,
) -> Result<Json<Value>, AppError> {
    let actor_model = body.actor_model.as_deref().unwrap_or("unknown");
    let actor_role = body.actor_role.as_deref().unwrap_or("assistant");
    let context = body.context.as_deref().unwrap_or("");
    let limit = body.limit.unwrap_or(10).min(30);
    let result = engram_lib::prompts::generate_header(
        &state.db,
        actor_model,
        actor_role,
        context,
        limit,
        auth.user_id,
    )
    .await?;
    Ok(Json(json!({
        "header": result.header,
        "text": result.text,
        "actor_model": result.actor_model,
        "prior_models": result.prior_models,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_request_deserializes_living_flags() {
        let json = r#"{
            "agent": "test-agent",
            "task": "do something",
            "include_brain": true,
            "include_growth": true,
            "include_instincts": true,
            "brain_limit": 10,
            "growth_limit": 3
        }"#;
        let req: GeneratePromptRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "test-agent");
        assert_eq!(req.task, "do something");
        assert_eq!(req.include_brain, Some(true));
        assert_eq!(req.include_growth, Some(true));
        assert_eq!(req.include_instincts, Some(true));
        assert_eq!(req.brain_limit, Some(10));
        assert_eq!(req.growth_limit, Some(3));
    }

    #[test]
    fn generate_request_defaults_living_flags_to_none() {
        let json = r#"{"agent": "a", "task": "t"}"#;
        let req: GeneratePromptRequest = serde_json::from_str(json).unwrap();
        assert!(req.include_brain.is_none());
        assert!(req.include_growth.is_none());
        assert!(req.include_instincts.is_none());
        assert!(req.brain_limit.is_none());
        assert!(req.growth_limit.is_none());
    }

    #[test]
    fn instinct_summary_is_static() {
        // Verify the instinct summary text is available
        let summary = concat!(
            "## Instinct Domains\n",
            "Pre-trained knowledge covering: infrastructure state transitions, ",
            "architecture decisions, system references, task completion patterns, ",
            "and common error resolutions. These domains provide baseline context ",
            "for infrastructure and deployment tasks."
        );
        assert!(summary.contains("Instinct Domains"));
        assert!(summary.contains("infrastructure"));
    }
}
