use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use engram_lib::context::budget::estimate_tokens;
use engram_lib::intelligence::growth::list_observations;
use engram_lib::memory::search::hybrid_search;
use engram_lib::memory::types::SearchRequest;
use engram_lib::prompts::{
    build_living_prompt, scrub_credentials, ContradictionInfo, MemorySummary,
};
use engram_lib::services::brain::BrainQueryOptions;
use engram_lib::EngError;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

mod types;
use types::{GeneratePromptRequest, HeaderBody, PromptQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/prompt", get(get_prompt))
        .route("/prompt/generate", post(post_prompt_generate))
        .route("/header", post(post_header))
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
                    // Query 1: task-specific recall
                    let task_opts = BrainQueryOptions {
                        query: task.to_string(),
                        top_k: Some(brain_limit.max(12)),
                        beta: None,
                        spread_hops: None,
                    };
                    let task_result = brain
                        .query(embedder.as_ref(), task, &task_opts)
                        .await
                        .unwrap_or_default();

                    // Query 2: infrastructure context
                    let infra_opts = BrainQueryOptions {
                        query: "server infrastructure deployment SSH configuration".to_string(),
                        top_k: Some(8),
                        beta: None,
                        spread_hops: None,
                    };
                    let infra_result = brain
                        .query(
                            embedder.as_ref(),
                            "server infrastructure deployment SSH configuration",
                            &infra_opts,
                        )
                        .await
                        .unwrap_or_default();

                    // Query 3: past failures related to this task
                    let failure_query = format!("failure problem error blocked mistake {}", task);
                    let failure_opts = BrainQueryOptions {
                        query: failure_query.clone(),
                        top_k: Some(6),
                        beta: None,
                        spread_hops: None,
                    };
                    let failure_result = brain
                        .query(embedder.as_ref(), &failure_query, &failure_opts)
                        .await
                        .unwrap_or_default();

                    // Convert brain results into MemorySummary lists
                    let task_memories: Vec<MemorySummary> = task_result
                        .activated
                        .iter()
                        .map(|m| MemorySummary {
                            id: m.id,
                            content: scrub_credentials(&m.content),
                            category: m.category.clone(),
                            activation: m.activation,
                        })
                        .collect();

                    let infra_memories: Vec<MemorySummary> = infra_result
                        .activated
                        .iter()
                        .map(|m| MemorySummary {
                            id: m.id,
                            content: scrub_credentials(&m.content),
                            category: m.category.clone(),
                            activation: m.activation,
                        })
                        .collect();

                    let failure_memories: Vec<MemorySummary> = failure_result
                        .activated
                        .iter()
                        .map(|m| MemorySummary {
                            id: m.id,
                            content: scrub_credentials(&m.content),
                            category: m.category.clone(),
                            activation: m.activation,
                        })
                        .collect();

                    // Build contradiction pairs from task result
                    let task_contradictions: Vec<ContradictionInfo> = task_result
                        .contradictions
                        .iter()
                        .filter_map(|c| {
                            let winner =
                                task_result.activated.iter().find(|m| m.id == c.winner_id)?;
                            let loser =
                                task_result.activated.iter().find(|m| m.id == c.loser_id)?;
                            Some(ContradictionInfo {
                                winner_content: scrub_credentials(&winner.content),
                                loser_content: scrub_credentials(&loser.content),
                                reason: c.reason.clone(),
                            })
                        })
                        .collect();

                    // Record source metadata
                    for m in &task_memories {
                        sources.push(json!({
                            "id": m.id,
                            "kind": "brain",
                            "activation": m.activation,
                            "category": m.category,
                        }));
                    }

                    let engram_url = state
                        .config
                        .eidolon
                        .url
                        .as_deref()
                        .unwrap_or("http://127.0.0.1:4200");

                    let living = build_living_prompt(
                        task,
                        &task_memories,
                        &task_contradictions,
                        &infra_memories,
                        &failure_memories,
                        engram_url,
                        &state.config.servers,
                        &state.config.safety.rules,
                    );
                    sections.push(living);
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
