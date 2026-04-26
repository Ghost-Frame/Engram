use crate::auth::{require_write, resolve_auth};
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use kleos_lib::intelligence::{
    causal, consolidation, contradiction, decomposition, extraction, reflections, sentiment,
    temporal,
};
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;
use kleos_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef { name: "intelligence.consolidate", description: "Consolidate related memories.", input_schema: || with_auth_props(json!({"memory_ids":{"type":"array","items":{"type":"integer"}}}), &["memory_ids"]) },
        ToolDef { name: "intelligence.detect_contradictions", description: "Detect contradictions for one memory or whole account.", input_schema: || with_auth_props(json!({"memory_id":{"type":"integer"}}), &[]) },
        ToolDef { name: "intelligence.decompose", description: "Decompose a memory into facts.", input_schema: || with_auth_props(json!({"memory_id":{"type":"integer"}}), &["memory_id"]) },
        ToolDef { name: "intelligence.temporal_summary", description: "Summarize temporal patterns.", input_schema: || with_auth_props(json!({"detect":{"type":"boolean"},"limit":{"type":"integer"}}), &[]) },
        ToolDef { name: "intelligence.reflect", description: "Create a reflection.", input_schema: || with_auth_props(json!({"content":{"type":"string"},"reflection_type":{"type":"string"},"source_memory_ids":{"type":"array","items":{"type":"integer"}},"confidence":{"type":"number"}}), &["content"]) },
        ToolDef { name: "intelligence.extract_facts", description: "Extract structured facts from a memory or raw content.", input_schema: || with_auth_props(json!({"memory_id":{"type":"integer"},"content":{"type":"string"},"category":{"type":"string"},"source":{"type":"string"}}), &[]) },
        ToolDef { name: "intelligence.sentiment", description: "Score sentiment for text or a memory.", input_schema: || with_auth_props(json!({"text":{"type":"string"},"memory_id":{"type":"integer"}}), &[]) },
        ToolDef { name: "intelligence.causal_trace", description: "Create or fetch causal chains.", input_schema: || with_auth_props(json!({"chain_id":{"type":"integer"},"root_memory_id":{"type":"integer"},"description":{"type":"string"},"links":{"type":"array"}}), &[]) },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.consolidate"))]
pub async fn consolidate(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_write(&auth)?;
    let ids: Vec<String> = serde_json::from_value::<Vec<i64>>(
        args.get("memory_ids")
            .cloned()
            .ok_or_else(|| invalid_input("memory_ids required"))?,
    )
    .map_err(|e| invalid_input(e.to_string()))?
    .into_iter()
    .map(|id| id.to_string())
    .collect();
    Ok(json!(
        consolidation::consolidate(&app.db, &ids, auth.user_id).await?
    ))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.detect_contradictions"))]
pub async fn detect_contradictions(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    if let Some(memory_id) = args.get("memory_id").and_then(Value::as_i64) {
        let memory = memory::get(&app.db, memory_id, auth.user_id).await?;
        return Ok(
            json!({"contradictions": contradiction::detect_contradictions(&app.db, &memory).await?}),
        );
    }
    Ok(
        json!({"contradictions": contradiction::scan_all_contradictions(&app.db, auth.user_id).await?}),
    )
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.decompose"))]
pub async fn decompose(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_write(&auth)?;
    let memory_id = args
        .get("memory_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("memory_id required"))?;
    Ok(json!({"child_ids": decomposition::decompose(&app.db, memory_id).await?}))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.temporal_summary"))]
pub async fn temporal_summary(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    if args.get("detect").and_then(Value::as_bool).unwrap_or(true) {
        let patterns = temporal::detect_patterns(&app.db).await?;
        return Ok(json!({"patterns": patterns, "count": patterns.len()}));
    }
    let patterns = temporal::list_patterns(&app.db).await?;
    Ok(json!({"patterns": patterns, "count": patterns.len()}))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.reflect"))]
pub async fn reflect(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_write(&auth)?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("content required"))?;
    let reflection_type = args
        .get("reflection_type")
        .and_then(Value::as_str)
        .unwrap_or("general");
    let source_memory_ids = args
        .get("source_memory_ids")
        .cloned()
        .map(serde_json::from_value::<Vec<i64>>)
        .transpose()
        .map_err(|e| invalid_input(e.to_string()))?
        .unwrap_or_default();
    let confidence = args
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    Ok(json!(
        reflections::create_reflection(
            &app.db,
            content,
            reflection_type,
            &source_memory_ids,
            confidence,
            auth.user_id
        )
        .await?
    ))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.extract_facts"))]
pub async fn extract_facts(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_write(&auth)?;
    let memory_id = if let Some(id) = args.get("memory_id").and_then(Value::as_i64) {
        id
    } else {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_input("memory_id or content required"))?;
        memory::store(
            &app.db,
            StoreRequest {
                content: content.to_string(),
                category: args
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or("general")
                    .to_string(),
                source: args
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp.extract_facts")
                    .to_string(),
                importance: 5,
                tags: None,
                embedding: None,
                session_id: None,
                is_static: Some(false),
                user_id: Some(auth.user_id),
                space_id: None,
                parent_memory_id: None,
            },
        )
        .await?
        .id
    };
    let memory = memory::get(&app.db, memory_id, auth.user_id).await?;
    Ok(json!({
        "memory_id": memory_id,
        "stats": extraction::fast_extract_facts(&app.db, &memory.content, memory_id, auth.user_id, memory.episode_id).await?
    }))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.sentiment"))]
pub async fn sentiment(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let text = if let Some(text) = args.get("text").and_then(Value::as_str) {
        text.to_string()
    } else if let Some(memory_id) = args.get("memory_id").and_then(Value::as_i64) {
        memory::get(&app.db, memory_id, auth.user_id).await?.content
    } else {
        return Err(invalid_input("text or memory_id required"));
    };
    let (sum, count) = sentiment::score_text_sum(&text);
    Ok(json!({"score": sentiment::score_text(&text), "sum": sum, "lexicon_hits": count}))
}

#[tracing::instrument(skip(app, args), fields(tool = "intelligence.causal_trace"))]
pub async fn causal_trace(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    if let Some(chain_id) = args.get("chain_id").and_then(Value::as_i64) {
        return Ok(json!(
            causal::get_chain(&app.db, chain_id, auth.user_id).await?
        ));
    }
    require_write(&auth)?;
    let chain = causal::create_chain(
        &app.db,
        args.get("root_memory_id").and_then(Value::as_i64),
        args.get("description").and_then(Value::as_str),
        auth.user_id,
    )
    .await?;
    if let Some(links) = args.get("links").and_then(Value::as_array) {
        for (idx, link) in links.iter().enumerate() {
            if let (Some(cause), Some(effect)) = (
                link.get("cause_memory_id").and_then(Value::as_i64),
                link.get("effect_memory_id").and_then(Value::as_i64),
            ) {
                let _ = causal::add_link(
                    &app.db,
                    chain.id,
                    cause,
                    effect,
                    link.get("strength").and_then(Value::as_f64).unwrap_or(1.0),
                    link.get("order_index")
                        .and_then(Value::as_i64)
                        .unwrap_or(idx as i64) as i32,
                    auth.user_id,
                )
                .await?;
            }
        }
    }
    Ok(json!(
        causal::get_chain(&app.db, chain.id, auth.user_id).await?
    ))
}
