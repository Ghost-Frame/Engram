use std::collections::HashMap;
use std::sync::OnceLock;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::auth::Scope;
use kleos_lib::grounding::{BackendType, GroundingClient, SessionConfig};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{CreateSessionBody, ExecuteBody, QualityQuery, ToolsQuery};

/// Per-tenant grounding clients. Each tenant's sessions are isolated inside their
/// own `GroundingClient`, so list/get/destroy cannot cross tenant boundaries.
type TenantMap = HashMap<i64, RwLock<GroundingClient>>;

fn tenants() -> &'static RwLock<TenantMap> {
    static TENANTS: OnceLock<RwLock<TenantMap>> = OnceLock::new();
    TENANTS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Run an action against the caller's per-tenant `GroundingClient`, creating one
/// on demand. Returns the closure's result.
async fn with_tenant_client<F, R>(user_id: i64, f: F) -> R
where
    F: for<'a> FnOnce(&'a mut GroundingClient) -> R,
{
    // Fast path: read lock + existing entry.
    {
        let guard = tenants().read().await;
        if let Some(lock) = guard.get(&user_id) {
            let mut client = lock.write().await;
            return f(&mut client);
        }
    }
    // Slow path: create entry under write lock, then operate.
    let mut guard = tenants().write().await;
    let lock = guard
        .entry(user_id)
        .or_insert_with(|| RwLock::new(GroundingClient::new()));
    let mut client = lock.write().await;
    f(&mut client)
}

/// Read-only variant that takes the caller's client read-locked.
async fn with_tenant_client_read<F, R>(user_id: i64, f: F) -> R
where
    F: for<'a> FnOnce(&'a GroundingClient) -> R,
{
    {
        let guard = tenants().read().await;
        if let Some(lock) = guard.get(&user_id) {
            let client = lock.read().await;
            return f(&client);
        }
    }
    let mut guard = tenants().write().await;
    let lock = guard
        .entry(user_id)
        .or_insert_with(|| RwLock::new(GroundingClient::new()));
    let client = lock.read().await;
    f(&client)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/grounding/sessions",
            post(create_session).get(list_sessions),
        )
        .route(
            "/grounding/sessions/{id}",
            get(get_session).delete(destroy_session),
        )
        .route("/grounding/tools", get(list_tools))
        .route("/grounding/execute", post(execute_tool))
        .route("/grounding/quality", get(get_quality))
        .route("/grounding/providers", get(list_providers))
}

async fn create_session(
    Auth(auth): Auth,
    Json(body): Json<CreateSessionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let backend = match body.backend.as_deref() {
        Some("mcp") => BackendType::Mcp,
        Some("web") => BackendType::Web,
        Some("gui") => BackendType::Gui,
        Some("system") => BackendType::System,
        _ => BackendType::Shell,
    };

    let name = body
        .name
        .unwrap_or_else(|| format!("session-{}", chrono::Utc::now().timestamp_millis()));

    let config = SessionConfig {
        name,
        backend,
        timeout_ms: body.timeout_ms,
        max_retries: body.max_retries,
        metadata: body.metadata,
    };

    let session = with_tenant_client(auth.user_id, |client| client.create_session(&config)).await;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(session).unwrap_or(json!({}))),
    ))
}

async fn list_sessions(Auth(auth): Auth) -> Result<Json<Value>, AppError> {
    let (session_values, count) = with_tenant_client_read(auth.user_id, |client| {
        let sessions = client.list_sessions();
        let values: Vec<Value> = sessions
            .iter()
            .map(|s| serde_json::to_value(s).unwrap_or(json!({})))
            .collect();
        let count = values.len();
        (values, count)
    })
    .await;
    Ok(Json(json!({ "sessions": session_values, "count": count })))
}

async fn get_session(Auth(auth): Auth, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    let session_json = with_tenant_client_read(auth.user_id, |client| {
        client
            .list_sessions()
            .iter()
            .find(|s| s.id == id)
            .map(|s| serde_json::to_value(s).unwrap_or(json!({})))
    })
    .await
    .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Session not found".into())))?;
    Ok(Json(session_json))
}

async fn destroy_session(
    Auth(auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    // Only destroy if the session exists under this tenant, otherwise 404.
    let existed = with_tenant_client(auth.user_id, |client| {
        let present = client.list_sessions().iter().any(|s| s.id == id);
        if present {
            client.destroy_session(&id);
        }
        present
    })
    .await;
    if !existed {
        return Err(AppError(kleos_lib::EngError::NotFound(
            "Session not found".into(),
        )));
    }
    Ok(Json(json!({ "destroyed": true, "id": id })))
}

async fn list_tools(
    Auth(auth): Auth,
    Query(_params): Query<ToolsQuery>,
) -> Result<Json<Value>, AppError> {
    let (tools_json, count) = with_tenant_client_read(auth.user_id, |client| {
        let tools = client.get_all_tools();
        let values: Vec<Value> = tools
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or(json!({})))
            .collect();
        let count = values.len();
        (values, count)
    })
    .await;
    Ok(Json(json!({ "tools": tools_json, "count": count })))
}

async fn execute_tool(
    Auth(auth): Auth,
    Json(body): Json<ExecuteBody>,
) -> Result<Json<Value>, AppError> {
    // SECURITY: grounding execution exposes shell-backed tools (shell_exec, file
    // read/write, etc). Restrict to admin-scoped callers so a leaked read/write
    // key cannot become RCE on the API host.
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(kleos_lib::EngError::Auth(
            "admin scope required for grounding execution".into(),
        )));
    }
    let tool_name = body.tool.trim();
    if tool_name.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "tool is required".into(),
        )));
    }

    let args = body.args.unwrap_or(json!({}));

    // Acquire per-tenant client read lock, then drive the async execute.
    let result = {
        // Ensure a client exists for this tenant.
        {
            let guard = tenants().read().await;
            if guard.get(&auth.user_id).is_none() {
                drop(guard);
                let mut w = tenants().write().await;
                w.entry(auth.user_id)
                    .or_insert_with(|| RwLock::new(GroundingClient::new()));
            }
        }
        let guard = tenants().read().await;
        let lock = guard.get(&auth.user_id).ok_or_else(|| {
            AppError(kleos_lib::EngError::Internal(
                "tenant grounding client not initialized".into(),
            ))
        })?;
        let client = lock.read().await;
        client.execute_tool(tool_name, &args, body.timeout_ms).await
    };
    Ok(Json(serde_json::to_value(result).unwrap_or(json!({}))))
}

// H-R3-004: tool_quality_records is operator-wide telemetry (tool_key /
// tool_name fields can encode per-tenant invocation history). Gating the
// surface behind admin scope keeps the data inside the operator boundary
// instead of leaking enumeration to every auth+read user.
async fn get_quality(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<QualityQuery>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&kleos_lib::auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for tool quality telemetry".into(),
        )));
    }
    let limit = params.limit.unwrap_or(50).min(200);
    let degraded_only = params.degraded.as_deref() == Some("true");

    let qm = kleos_lib::grounding::ToolQualityManager::new(None);

    if degraded_only {
        let tools = qm
            .get_degraded_tools(&state.db)
            .await
            .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;
        let records: Vec<Value> = tools
            .iter()
            .map(|(name, score)| json!({ "tool_name": name, "quality_score": score }))
            .collect();
        let count = records.len();
        Ok(Json(json!({ "records": records, "count": count })))
    } else {
        let records = qm
            .get_all_records(&state.db, limit as i64)
            .await
            .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;
        let count = records.len();
        Ok(Json(json!({ "records": records, "count": count })))
    }
}

async fn list_providers(Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    // Currently only shell provider is registered
    Ok(Json(json!({
        "providers": [{ "name": "shell", "type": "shell", "status": "connected" }],
        "count": 1,
    })))
}
