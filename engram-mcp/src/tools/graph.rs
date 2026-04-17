use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use kleos_lib::graph::{communities, cooccurrence, pagerank, search};
use kleos_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef { name: "graph.get_neighbors", description: "Get graph neighborhood around a node.", input_schema: || with_auth_props(json!({
            "node_id":{"type":"string"},"memory_id":{"type":"integer"},"depth":{"type":"integer"}
        }), &[]) },
        ToolDef { name: "graph.pagerank_top", description: "Return top PageRank memories.", input_schema: || with_auth_props(json!({
            "limit":{"type":"integer"},"refresh":{"type":"boolean"}
        }), &[]) },
        ToolDef { name: "graph.louvain_communities", description: "Detect or summarize graph communities.", input_schema: || with_auth_props(json!({
            "run":{"type":"boolean"},"max_iterations":{"type":"integer"},"community_id":{"type":"integer"},"limit":{"type":"integer"}
        }), &[]) },
        ToolDef { name: "graph.cooccurrence", description: "Rebuild or query entity cooccurrence.", input_schema: || with_auth_props(json!({
            "rebuild":{"type":"boolean"},"entity_id":{"type":"integer"},"limit":{"type":"integer"}
        }), &[]) },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "graph.get_neighbors"))]
pub async fn get_neighbors(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let node_id = args
        .get("node_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            args.get("memory_id")
                .and_then(Value::as_i64)
                .map(|id| format!("m{id}"))
        })
        .ok_or_else(|| invalid_input("node_id or memory_id required"))?;
    let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(2) as u32;
    let (nodes, edges) = search::neighborhood(&app.db, &node_id, depth, auth.user_id).await?;
    Ok(json!({"nodes": nodes, "edges": edges}))
}

#[tracing::instrument(skip(app, args), fields(tool = "graph.pagerank_top"))]
pub async fn pagerank_top(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    if args
        .get("refresh")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let _ = pagerank::update_pagerank_scores(&app.db, auth.user_id).await?;
    }
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let mut scores = pagerank::compute_pagerank_for_user(&app.db, auth.user_id).await?;
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(json!({"scores": scores.into_iter().take(limit).collect::<Vec<_>>()}))
}

#[tracing::instrument(skip(app, args), fields(tool = "graph.louvain_communities"))]
pub async fn louvain_communities(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    if args.get("run").and_then(Value::as_bool).unwrap_or(true) {
        let result = communities::detect_communities(
            &app.db,
            auth.user_id,
            args.get("max_iterations")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32,
        )
        .await?;
        return Ok(json!({
            "result": result,
            "stats": communities::get_community_stats(&app.db, auth.user_id).await?
        }));
    }
    if let Some(community_id) = args.get("community_id").and_then(Value::as_i64) {
        return Ok(json!({
            "members": communities::get_community_members(&app.db, community_id, auth.user_id, args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize).await?
        }));
    }
    Ok(json!({"stats": communities::get_community_stats(&app.db, auth.user_id).await?}))
}

#[tracing::instrument(skip(app, args), fields(tool = "graph.cooccurrence"))]
pub async fn cooccurrence(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    if args
        .get("rebuild")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(
            json!({"pairs": cooccurrence::rebuild_cooccurrences(&app.db, auth.user_id).await?}),
        );
    }
    let entity_id = args
        .get("entity_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("entity_id required when rebuild=false"))?;
    Ok(json!({
        "entities": cooccurrence::get_cooccurring_entities(&app.db, entity_id, auth.user_id, args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize).await?
    }))
}
