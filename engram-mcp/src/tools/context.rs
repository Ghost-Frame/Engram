use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use engram_lib::context::{assemble_context as assemble_context_lib, ContextOptions};
use engram_lib::prompts;
use engram_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef { name: "context.assemble_context", description: "Assemble retrieval context.", input_schema: || with_auth_props(json!({
            "query":{"type":"string"},"max_tokens":{"type":"integer"},"token_budget":{"type":"integer"},"budget":{"type":"integer"},"strategy":{"type":"string"},"depth":{"type":"integer"},"mode":{"type":"string"},"include_static":{"type":"boolean"},"include_recent":{"type":"boolean"},"include_episodes":{"type":"boolean"},"include_linked":{"type":"boolean"},"include_inference":{"type":"boolean"},"include_current_state":{"type":"boolean"},"include_preferences":{"type":"boolean"},"include_structured_facts":{"type":"boolean"},"include_working_memory":{"type":"boolean"},"max_memory_tokens":{"type":"integer"},"dedup_threshold":{"type":"number"},"min_relevance":{"type":"number"},"semantic_ceiling":{"type":"number"},"semantic_limit":{"type":"integer"},"source":{"type":"string"},"session":{"type":"string"}
        }), &["query"]) },
        ToolDef { name: "context.get_header", description: "Generate a task header from recent multi-model activity.", input_schema: || with_auth_props(json!({
            "actor_model":{"type":"string"},"actor_role":{"type":"string"},"context":{"type":"string"},"limit":{"type":"integer"}
        }), &[]) },
        ToolDef { name: "context.generate_prompt", description: "Generate a packed memory prompt.", input_schema: || with_auth_props(json!({
            "format":{"type":"string"},"token_budget":{"type":"integer"},"context":{"type":"string"}
        }), &[]) },
    ]);
}

pub async fn assemble_context(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let opts: ContextOptions =
        serde_json::from_value(args.clone()).map_err(|e| invalid_input(e.to_string()))?;
    Ok(json!(
        assemble_context_lib(&app.db, opts, auth.user_id, None, None).await?
    ))
}

pub async fn get_header(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(
        prompts::generate_header(
            &app.db,
            args.get("actor_model")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            args.get("actor_role")
                .and_then(Value::as_str)
                .unwrap_or("assistant"),
            args.get("context").and_then(Value::as_str).unwrap_or(""),
            args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize,
            auth.user_id,
        )
        .await?
    ))
}

pub async fn generate_prompt(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(
        prompts::generate_prompt(
            &app.db,
            args.get("format").and_then(Value::as_str).unwrap_or("raw"),
            args.get("token_budget")
                .and_then(Value::as_u64)
                .unwrap_or(4000) as usize,
            args.get("context").and_then(Value::as_str).unwrap_or(""),
            auth.user_id,
        )
        .await?
    ))
}
