use crate::auth::{require_admin, resolve_auth};
use crate::tools::{with_auth_props, ToolDef};
use crate::App;
use kleos_lib::admin;
use kleos_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef {
            name: "admin.reembed",
            description: "Clear stored embeddings.",
            input_schema: || with_auth_props(json!({"user_id":{"type":"integer"}}), &[]),
        },
        ToolDef {
            name: "admin.rebuild_fts",
            description: "Rebuild the FTS index.",
            input_schema: || with_auth_props(json!({}), &[]),
        },
        ToolDef {
            name: "admin.vector_sync_replay",
            description: "Replay pending vector sync ledger items.",
            input_schema: || with_auth_props(json!({"limit":{"type":"integer"}}), &[]),
        },
        ToolDef {
            name: "admin.backup",
            description: "Export logical backup data.",
            input_schema: || with_auth_props(json!({}), &[]),
        },
        ToolDef {
            name: "admin.checkpoint",
            description: "Run WAL checkpoint.",
            input_schema: || with_auth_props(json!({}), &[]),
        },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "admin.reembed"))]
pub async fn reembed(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_admin(&auth)?;
    Ok(
        json!({"cleared": admin::reembed_all(&app.db, args.get("user_id").and_then(Value::as_i64)).await?}),
    )
}

#[tracing::instrument(skip(app, args), fields(tool = "admin.rebuild_fts"))]
pub async fn rebuild_fts(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_admin(&auth)?;
    Ok(json!({"rows": admin::rebuild_fts(&app.db).await?}))
}

#[tracing::instrument(skip(app, args), fields(tool = "admin.vector_sync_replay"))]
pub async fn vector_sync_replay(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_admin(&auth)?;
    Ok(json!(memory_replay(app, args).await?))
}

async fn memory_replay(app: &App, args: Value) -> Result<Value> {
    Ok(json!(
        kleos_lib::memory::replay_vector_sync_pending(
            &app.db,
            args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
        )
        .await?
    ))
}

#[tracing::instrument(skip(app, args), fields(tool = "admin.backup"))]
pub async fn backup(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_admin(&auth)?;
    Ok(json!(admin::export_data(&app.db).await?))
}

#[tracing::instrument(skip(app, args), fields(tool = "admin.checkpoint"))]
pub async fn checkpoint(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    require_admin(&auth)?;
    admin::checkpoint(&app.db).await
}
