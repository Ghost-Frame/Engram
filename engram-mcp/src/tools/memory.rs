use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use kleos_lib::memory;
use kleos_lib::memory::search::hybrid_search;
use kleos_lib::memory::types::{ListOptions, SearchRequest, StoreRequest, UpdateRequest};
use kleos_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef { name: "memory.store", description: "Store a memory.", input_schema: || with_auth_props(json!({
            "content": {"type":"string"},
            "category": {"type":"string"},
            "source": {"type":"string"},
            "importance": {"type":"integer"},
            "tags": {"type":"array","items":{"type":"string"}},
            "session_id": {"type":"string"},
            "is_static": {"type":"boolean"},
            "space_id": {"type":"integer"}
        }), &["content"]) },
        ToolDef { name: "memory.search", description: "Search memories.", input_schema: || with_auth_props(json!({
            "query": {"type":"string"},
            "limit": {"type":"integer"},
            "category": {"type":"string"},
            "source": {"type":"string"},
            "tags": {"type":"array","items":{"type":"string"}},
            "threshold": {"type":"number"},
            "space_id": {"type":"integer"},
            "include_forgotten": {"type":"boolean"},
            "mode": {"type":"string"},
            "question_type": {"type":"string"},
            "expand_relationships": {"type":"boolean"},
            "include_links": {"type":"boolean"},
            "latest_only": {"type":"boolean"},
            "source_filter": {"type":"string"}
        }), &["query"]) },
        ToolDef { name: "memory.get", description: "Fetch a memory by id.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "memory.list", description: "List memories.", input_schema: || with_auth_props(json!({
            "limit":{"type":"integer"},"offset":{"type":"integer"},"category":{"type":"string"},"source":{"type":"string"},"space_id":{"type":"integer"},"include_forgotten":{"type":"boolean"},"include_archived":{"type":"boolean"}
        }), &[]) },
        ToolDef { name: "memory.update", description: "Update a memory.", input_schema: || with_auth_props(json!({
            "id":{"type":"integer"},"content":{"type":"string"},"category":{"type":"string"},"source":{"type":"string"},"importance":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}},"status":{"type":"string"}
        }), &["id"]) },
        ToolDef { name: "memory.delete", description: "Delete a memory.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "memory.mark_forgotten", description: "Mark memory forgotten.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "memory.mark_archived", description: "Mark memory archived.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "memory.mark_unarchived", description: "Mark memory unarchived.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "memory.update_forget_reason", description: "Update forget reason.", input_schema: || with_auth_props(json!({"id":{"type":"integer"},"reason":{"type":"string"}}), &["id","reason"]) },
        ToolDef { name: "memory.adjust_importance", description: "Adjust memory importance.", input_schema: || with_auth_props(json!({"id":{"type":"integer"},"delta":{"type":"integer"}}), &["id","delta"]) },
        ToolDef { name: "memory.insert_link", description: "Insert memory link.", input_schema: || with_auth_props(json!({"source_id":{"type":"integer"},"target_id":{"type":"integer"},"similarity":{"type":"number"},"link_type":{"type":"string"}}), &["source_id","target_id"]) },
        ToolDef { name: "memory.get_by_content_hash", description: "Get memories by stored content hash.", input_schema: || with_auth_props(json!({"content_hash":{"type":"string"}}), &["content_hash"]) },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.store"))]
pub async fn store(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let req = StoreRequest {
        content: args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_input("content required"))?
            .to_string(),
        category: args
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("general")
            .to_string(),
        source: args
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("mcp")
            .to_string(),
        importance: args.get("importance").and_then(Value::as_i64).unwrap_or(5) as i32,
        tags: args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        embedding: None,
        session_id: args
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        is_static: args.get("is_static").and_then(Value::as_bool),
        user_id: Some(auth.user_id),
        space_id: args.get("space_id").and_then(Value::as_i64),
        parent_memory_id: args.get("parent_memory_id").and_then(Value::as_i64),
    };
    let stored = memory::store(&app.db, req).await?;
    let fetched = memory::get(&app.db, stored.id, auth.user_id).await?;
    Ok(json!({"store_result": stored, "memory": fetched}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.search"))]
pub async fn search(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let req = SearchRequest {
        query: args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_input("query required"))?
            .to_string(),
        embedding: None,
        limit: args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|v| v as usize),
        category: args
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_string),
        source: args
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        tags: args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        threshold: args
            .get("threshold")
            .and_then(Value::as_f64)
            .map(|v| v as f32),
        user_id: Some(auth.user_id),
        space_id: args.get("space_id").and_then(Value::as_i64),
        include_forgotten: args.get("include_forgotten").and_then(Value::as_bool),
        mode: args.get("mode").and_then(Value::as_str).map(str::to_string),
        question_type: args
            .get("question_type")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        expand_relationships: args
            .get("expand_relationships")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        include_links: args
            .get("include_links")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        latest_only: args
            .get("latest_only")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        source_filter: args
            .get("source_filter")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    Ok(json!({"results": hybrid_search(&app.db, req).await?}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.get"))]
pub async fn get(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    Ok(json!(memory::get(&app.db, id, auth.user_id).await?))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.list"))]
pub async fn list(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let opts = ListOptions {
        limit: args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize,
        offset: args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
        category: args
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_string),
        source: args
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        user_id: Some(auth.user_id),
        space_id: args.get("space_id").and_then(Value::as_i64),
        include_forgotten: args
            .get("include_forgotten")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        include_archived: args
            .get("include_archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    Ok(json!({"memories": memory::list(&app.db, opts).await?}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.update"))]
pub async fn update(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    let req = UpdateRequest {
        content: args
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        category: args
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_string),
        importance: args
            .get("importance")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        tags: args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        is_static: args.get("is_static").and_then(Value::as_bool),
        status: args
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        embedding: None,
    };
    Ok(json!(memory::update(&app.db, id, req, auth.user_id).await?))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.delete"))]
pub async fn delete(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    memory::delete(&app.db, id, auth.user_id).await?;
    Ok(json!({"deleted": true, "id": id}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.mark_forgotten"))]
pub async fn mark_forgotten(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    memory::mark_forgotten(&app.db, id, auth.user_id).await?;
    Ok(json!({"forgotten": true, "id": id}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.mark_archived"))]
pub async fn mark_archived(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    memory::mark_archived(&app.db, id, auth.user_id).await?;
    Ok(json!({"archived": true, "id": id}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.mark_unarchived"))]
pub async fn mark_unarchived(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    memory::mark_unarchived(&app.db, id, auth.user_id).await?;
    Ok(json!({"unarchived": true, "id": id}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.update_forget_reason"))]
pub async fn update_forget_reason(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("reason required"))?;
    memory::update_forget_reason(&app.db, id, reason, auth.user_id).await?;
    Ok(json!({"updated": true, "id": id, "reason": reason}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.adjust_importance"))]
pub async fn adjust_importance(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("id required"))?;
    let delta = args
        .get("delta")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("delta required"))? as i32;
    memory::adjust_importance(&app.db, id, auth.user_id, delta).await?;
    Ok(json!({"updated": true, "id": id, "delta": delta}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.insert_link"))]
pub async fn insert_link(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let source_id = args
        .get("source_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("source_id required"))?;
    let target_id = args
        .get("target_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("target_id required"))?;
    let similarity = args
        .get("similarity")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let link_type = args
        .get("link_type")
        .and_then(Value::as_str)
        .unwrap_or("similarity");
    memory::insert_link(
        &app.db,
        source_id,
        target_id,
        similarity,
        link_type,
        auth.user_id,
    )
    .await?;
    Ok(json!({"inserted": true, "source_id": source_id, "target_id": target_id, "type": link_type}))
}

#[tracing::instrument(skip(app, args), fields(tool = "memory.get_by_content_hash"))]
pub async fn get_by_content_hash(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let content_hash = args
        .get("content_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("content_hash required"))?;
    let content_hash_owned = content_hash.to_string();
    let user_id = auth.user_id;

    let ids: Vec<i64> = app.db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM memories WHERE content_hash = ?1 AND user_id = ?2 AND is_forgotten = 0 ORDER BY id DESC"
        ).map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        let mut rows = stmt.query(rusqlite::params![content_hash_owned, user_id])
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))? {
            let id: i64 = row.get(0).map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            ids.push(id);
        }
        Ok(ids)
    }).await?;

    let mut memories = Vec::new();
    for id in ids {
        memories.push(memory::get(&app.db, id, auth.user_id).await?);
    }
    Ok(json!({"memories": memories}))
}
