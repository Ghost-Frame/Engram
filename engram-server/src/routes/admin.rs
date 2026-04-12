use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Instant;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::auth::{create_key, AuthContext, Scope};
use engram_lib::cred::{ProxyRequest, ProxyResponse};
use engram_lib::graph::{communities, cooccurrence};

fn require_admin(auth: &AuthContext) -> Result<(), AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }
    Ok(())
}

fn to_json<T: serde::Serialize>(v: T) -> Result<Json<Value>, AppError> {
    serde_json::to_value(v)
        .map(Json)
        .map_err(|e| AppError(engram_lib::EngError::Serialization(e)))
}

pub fn router() -> Router<AppState> {
    Router::new()
        // Existing
        .route("/bootstrap", post(bootstrap))
        .route("/stats", get(get_stats))
        // Settings
        .route("/admin/settings", get(get_settings).put(put_settings))
        // Operations
        .route("/admin/gc", post(admin_gc))
        .route("/admin/compact", post(admin_compact))
        .route("/admin/reembed", post(admin_reembed))
        .route("/admin/rebuild-fts", post(admin_rebuild_fts))
        .route("/admin/refresh-cache", post(refresh_cache))
        .route("/admin/backfill-facts", post(backfill_facts))
        // Info
        .route("/admin/schema", get(admin_schema))
        .route("/admin/embedding-info", get(embedding_info))
        .route("/admin/scale-report", get(scale_report_handler))
        .route("/admin/cold-storage", get(cold_storage_handler))
        .route("/admin/providers", get(admin_providers))
        .route("/admin/tasks", get(admin_tasks))
        .route("/admin/cred/resolve", post(admin_cred_resolve))
        .route("/admin/cred/proxy", post(admin_cred_proxy))
        // Maintenance + SLA
        .route(
            "/admin/maintenance",
            get(get_maintenance_handler).post(post_maintenance_handler),
        )
        .route("/admin/sla", get(admin_sla))
        .route("/admin/sla/reset", post(admin_sla_reset))
        // Quotas
        .route("/admin/quotas", get(get_quotas).put(put_quotas))
        // Usage + Tenants
        .route("/admin/usage", get(admin_usage))
        .route("/admin/tenants", get(admin_tenants))
        .route("/tenants/provision", post(provision_tenant))
        .route("/tenants/deprovision", post(deprovision_tenant))
        // Data management
        .route("/admin/export", get(export_handler))
        .route("/reset", post(reset_user))
        .route("/backup", get(backup_handler))
        .route("/backup/verify", post(backup_verify_handler))
        .route("/checkpoint", post(checkpoint_handler))
        // Safe mode
        .route("/admin/safe-mode/exit", post(post_safe_mode_exit))
        // Graph operations
        .route(
            "/admin/detect-communities",
            post(detect_communities_handler),
        )
        .route(
            "/admin/rebuild-cooccurrences",
            post(rebuild_cooccurrences_handler),
        )
        // PageRank
        .route("/admin/pagerank/rebuild", post(admin_pagerank_rebuild))
        // Vector sync replay
        .route("/admin/vector-sync/replay", post(admin_vector_sync_replay))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn count_rows(state: &AppState, sql: &str) -> Result<i64, AppError> {
    let mut rows = state
        .db
        .conn
        .query(sql, ())
        .await
        .map_err(engram_lib::EngError::Database)?;
    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| engram_lib::EngError::Internal("missing stats row".into()))?;
    row.get(0)
        .map_err(engram_lib::EngError::Database)
        .map_err(AppError)
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, Default)]
struct BootstrapBody {
    #[serde(default)]
    secret: Option<String>,
}

/// SECURITY (SEC-HIGH-6): bootstrap has no upstream rate limiter because
/// it bypasses auth entirely. Without a cooldown an attacker can brute-
/// force `ENGRAM_BOOTSTRAP_SECRET` at wire speed. This sliding-window
/// counter is kept in-process and deliberately global rather than per-IP:
/// bootstrap is a one-shot operation, so throttling the whole endpoint is
/// sufficient and avoids having to rearchitect the app to pass
/// `ConnectInfo<SocketAddr>` through to handlers. Legitimate retries after
/// the cooldown window are still possible.
const BOOTSTRAP_FAILURE_LIMIT: u32 = 5;
const BOOTSTRAP_WINDOW_SECS: u64 = 60;

struct BootstrapThrottle {
    failures: u32,
    window_start: Instant,
}

fn bootstrap_throttle() -> &'static Mutex<BootstrapThrottle> {
    static STATE: std::sync::OnceLock<Mutex<BootstrapThrottle>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(BootstrapThrottle {
            failures: 0,
            window_start: Instant::now(),
        })
    })
}

/// Returns Err((status, body)) if currently locked out.
fn check_bootstrap_cooldown() -> Result<(), (StatusCode, Json<Value>)> {
    let mut throttle = bootstrap_throttle().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if now.duration_since(throttle.window_start).as_secs() >= BOOTSTRAP_WINDOW_SECS {
        throttle.window_start = now;
        throttle.failures = 0;
    }
    if throttle.failures >= BOOTSTRAP_FAILURE_LIMIT {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "bootstrap rate-limited; wait before retrying",
            })),
        ));
    }
    Ok(())
}

fn record_bootstrap_failure() {
    let mut throttle = bootstrap_throttle().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if now.duration_since(throttle.window_start).as_secs() >= BOOTSTRAP_WINDOW_SECS {
        throttle.window_start = now;
        throttle.failures = 0;
    }
    throttle.failures = throttle.failures.saturating_add(1);
}

async fn bootstrap(
    State(state): State<AppState>,
    body: Option<Json<BootstrapBody>>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // SECURITY (SEC-HIGH-6): before any env lookup, check the global
    // bootstrap cooldown so attackers cannot brute-force the secret.
    if let Err(resp) = check_bootstrap_cooldown() {
        return Ok(resp);
    }

    // SECURITY: previously POST /bootstrap was unauthenticated with only a
    // "no active keys exist" guard. On fresh deployments an attacker could
    // race the legitimate admin to obtain the first admin key. We now require
    // a pre-shared ENGRAM_BOOTSTRAP_SECRET, fed either via an Authorization
    // header or the request body. If the env var is unset, bootstrap is
    // disabled entirely and must be performed out-of-band.
    let Ok(expected) = std::env::var("ENGRAM_BOOTSTRAP_SECRET") else {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "bootstrap disabled: set ENGRAM_BOOTSTRAP_SECRET to enable"
            })),
        ));
    };
    if expected.is_empty() {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "bootstrap disabled: ENGRAM_BOOTSTRAP_SECRET is empty" })),
        ));
    }

    let supplied = body
        .as_ref()
        .and_then(|Json(b)| b.secret.as_deref())
        .map(|s| s.to_string())
        .unwrap_or_default();
    use subtle::ConstantTimeEq;
    if supplied.len() != expected.len()
        || supplied.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1
    {
        // SECURITY (SEC-HIGH-6): record failure toward the sliding-window
        // cooldown so repeated wrong secrets lock the endpoint.
        record_bootstrap_failure();
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid bootstrap secret" })),
        ));
    }

    // SECURITY: atomically claim the bootstrap slot via an INSERT OR IGNORE on
    // a unique row in app_state. SQLite reports the number of modified rows,
    // which is 1 if we won the claim and 0 if another concurrent request beat
    // us to it. Collapsing the prior COUNT + INSERT race means two requests
    // arriving in the same microsecond cannot both mint an admin key.
    let changes = state
        .db
        .conn
        .execute(
            "INSERT OR IGNORE INTO app_state (key, value, updated_at) \
             VALUES ('bootstrap_claimed', datetime('now'), datetime('now'))",
            (),
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    if changes == 0 {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "bootstrap already complete" })),
        ));
    }

    // Belt-and-suspenders: if any prior build already minted keys without the
    // sentinel being set, keep refusing so we don't produce a second admin.
    let existing_count =
        count_rows(&state, "SELECT COUNT(*) FROM api_keys WHERE is_active = 1").await?;
    if existing_count > 0 {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "bootstrap already complete" })),
        ));
    }

    let scopes = vec![Scope::Read, Scope::Write, Scope::Admin];
    let (key, raw_key) = create_key(&state.db, 1, "admin", scopes).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": raw_key.clone(),
            "api_key": raw_key,
            "name": key.name,
            "scopes": key.scopes,
            "user_id": key.user_id,
            "message": "Bootstrap complete. Store this key -- it will not be shown again."
        })),
    ))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    Ok(Json(json!({
        "memories": count_rows(&state, "SELECT COUNT(*) FROM memories").await?,
        "tasks": count_rows(&state, "SELECT COUNT(*) FROM tasks").await?,
        "events": count_rows(&state, "SELECT COUNT(*) FROM events").await?,
        "actions": count_rows(&state, "SELECT COUNT(*) FROM action_log").await?,
        "agents": count_rows(&state, "SELECT COUNT(*) FROM agents").await?,
        "api_keys": count_rows(&state, "SELECT COUNT(*) FROM api_keys WHERE is_active = 1").await?,
    })))
}

// ---------------------------------------------------------------------------
// Settings (app_state key-value)
// ---------------------------------------------------------------------------

async fn get_settings(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = engram_lib::admin::list_state(&state.db).await?;
    let map: serde_json::Map<String, Value> = rows
        .into_iter()
        .map(|r| (r.key, Value::String(r.value)))
        .collect();
    Ok(Json(Value::Object(map)))
}

async fn put_settings(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<serde_json::Map<String, Value>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let mut updated = 0usize;
    for (key, val) in &body {
        let v = val
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| val.to_string());
        engram_lib::admin::upsert_state(&state.db, key, &v).await?;
        updated += 1;
    }
    Ok(Json(json!({ "updated": updated })))
}

// ---------------------------------------------------------------------------
// GC
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct GcBody {
    user_id: Option<i64>,
}

async fn admin_gc(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<GcBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let uid = body.and_then(|b| b.user_id);
    let result = engram_lib::admin::gc(&state.db, uid).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Compact
// ---------------------------------------------------------------------------

async fn admin_compact(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::compact(&state.db).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Re-embed
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReembedBody {
    user_id: Option<i64>,
}

async fn admin_reembed(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<ReembedBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let uid = body.and_then(|b| b.user_id);
    let cleared = engram_lib::admin::reembed_all(&state.db, uid).await?;
    Ok(Json(json!({ "cleared": cleared })))
}

// ---------------------------------------------------------------------------
// Rebuild FTS
// ---------------------------------------------------------------------------

async fn admin_rebuild_fts(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let indexed = engram_lib::admin::rebuild_fts(&state.db).await?;
    Ok(Json(json!({ "indexed": indexed })))
}

// ---------------------------------------------------------------------------
// Refresh cache (no-op signal)
// ---------------------------------------------------------------------------

async fn refresh_cache(
    State(_state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    Ok(Json(
        json!({ "status": "ok", "message": "cache refresh signaled" }),
    ))
}

// ---------------------------------------------------------------------------
// Backfill facts
// ---------------------------------------------------------------------------

async fn backfill_facts(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let memories = engram_lib::admin::get_memories_without_facts(&state.db, 500).await?;
    let processed = memories.len() as i64;
    let mut facts_created = 0i32;
    for (memory_id, content, user_id) in memories {
        if let Ok(stats) = engram_lib::intelligence::extraction::fast_extract_facts(
            &state.db, &content, memory_id, user_id, None,
        )
        .await
        {
            facts_created += stats.facts;
        }
    }
    Ok(Json(
        json!({ "processed": processed, "facts_created": facts_created }),
    ))
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

async fn admin_schema(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::get_schema(&state.db).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Embedding info
// ---------------------------------------------------------------------------

async fn embedding_info(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    Ok(Json(json!({
        "model": state.config.embedding_model,
        "dimensions": state.config.embedding_dim,
        "ready": state.embedder.is_some(),
    })))
}

// ---------------------------------------------------------------------------
// Scale report
// ---------------------------------------------------------------------------

async fn scale_report_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::scale_report(&state.db).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Cold storage stats
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ColdStorageParams {
    #[serde(default = "default_cold_days")]
    days: i64,
}
fn default_cold_days() -> i64 {
    90
}

async fn cold_storage_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ColdStorageParams>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::cold_storage_stats(&state.db, params.days).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

async fn admin_providers(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    Ok(Json(json!({
        "embedding": {
            "ready": state.embedder.is_some(),
            "model": state.config.embedding_model,
        },
        "reranker": {
            "ready": state.reranker.is_some(),
        },
        "llm": {
            "ready": state.llm.is_some(),
        },
        "brain": {
            "ready": state.brain.is_some(),
        },
    })))
}

// ---------------------------------------------------------------------------
// Tasks (job queue stats)
// ---------------------------------------------------------------------------

async fn admin_tasks(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let stats = engram_lib::jobs::get_job_stats(&state.db.conn).await?;
    to_json(stats)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AdminCredResolveBody {
    text: Option<String>,
    service: Option<String>,
    key: Option<String>,
    raw: bool,
}

async fn admin_cred_resolve(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<AdminCredResolveBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let agent = auth.key.name.as_str();

    if let Some(text) = body.text.as_deref() {
        let resolved = state
            .credd
            .resolve_text_with_options(&state.db, auth.user_id, agent, text, body.raw)
            .await?;
        return Ok(Json(json!({ "text": resolved })));
    }

    let service = body
        .service
        .as_deref()
        .ok_or_else(|| AppError(engram_lib::EngError::InvalidInput("service is required".into())))?;
    let key = body
        .key
        .as_deref()
        .ok_or_else(|| AppError(engram_lib::EngError::InvalidInput("key is required".into())))?;

    let value = if body.raw {
        state
            .credd
            .get_raw(&state.db, auth.user_id, agent, service, key)
            .await?
    } else {
        state
            .credd
            .resolve_text(
                &state.db,
                auth.user_id,
                agent,
                &format!("{{{{secret:{service}/{key}}}}}"),
            )
            .await?
    };

    Ok(Json(json!({
        "service": service,
        "key": key,
        "value": value,
        "raw": body.raw,
    })))
}

#[derive(Deserialize)]
struct AdminCredProxyBody {
    service: String,
    key: String,
    request: ProxyRequest,
}

async fn admin_cred_proxy(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<AdminCredProxyBody>,
) -> Result<Json<ProxyResponse>, AppError> {
    require_admin(&auth)?;
    let response = state
        .credd
        .proxy(
            &state.db,
            auth.user_id,
            auth.key.name.as_str(),
            &body.service,
            &body.key,
            &body.request,
        )
        .await?;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

async fn get_maintenance_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::get_maintenance(&state.db).await?;
    to_json(result)
}

#[derive(Deserialize)]
struct MaintenanceBody {
    enabled: bool,
    message: Option<String>,
}

async fn post_maintenance_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<MaintenanceBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result =
        engram_lib::admin::set_maintenance(&state.db, body.enabled, body.message.as_deref())
            .await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// SLA
// ---------------------------------------------------------------------------

async fn admin_sla(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::get_sla(&state.db).await?;
    to_json(result)
}

async fn admin_sla_reset(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let ts = chrono::Utc::now().to_rfc3339();
    engram_lib::admin::upsert_state(&state.db, "sla_reset_at", &ts).await?;
    Ok(Json(json!({ "status": "ok", "reset_at": ts })))
}

// ---------------------------------------------------------------------------
// Quotas
// ---------------------------------------------------------------------------

async fn get_quotas(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let quotas = engram_lib::quota::list_quotas(&state.db).await?;
    let result: Vec<Value> = quotas
        .into_iter()
        .map(|(q, username)| {
            json!({
                "user_id": q.user_id,
                "username": username,
                "max_memories": q.max_memories,
                "max_conversations": q.max_conversations,
                "max_api_keys": q.max_api_keys,
                "max_spaces": q.max_spaces,
                "max_memory_size_bytes": q.max_memory_size_bytes,
                "rate_limit_override": q.rate_limit_override,
            })
        })
        .collect();
    Ok(Json(json!(result)))
}

async fn put_quotas(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<engram_lib::quota::TenantQuota>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    engram_lib::quota::upsert_quota(&state.db, &body).await?;
    Ok(Json(json!({ "status": "ok", "user_id": body.user_id })))
}

// ---------------------------------------------------------------------------
// Usage + Tenants
// ---------------------------------------------------------------------------

async fn admin_usage(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = engram_lib::admin::get_usage(&state.db).await?;
    to_json(rows)
}

async fn admin_tenants(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = engram_lib::admin::get_tenants(&state.db).await?;
    to_json(rows)
}

// ---------------------------------------------------------------------------
// Provision / Deprovision
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProvisionBody {
    username: String,
    email: Option<String>,
    #[serde(default = "default_role")]
    role: String,
}
fn default_role() -> String {
    "user".to_string()
}

async fn provision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ProvisionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::provision_tenant(
        &state.db,
        &body.username,
        body.email.as_deref(),
        &body.role,
    )
    .await?;
    let json_result = to_json(result)?;
    Ok((StatusCode::CREATED, json_result))
}

#[derive(Deserialize)]
struct DeprovisionBody {
    user_id: i64,
}

async fn deprovision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<DeprovisionBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let removed = engram_lib::admin::deprovision_tenant(&state.db, body.user_id).await?;
    Ok(Json(json!({ "removed": removed, "user_id": body.user_id })))
}

// ---------------------------------------------------------------------------
// Checkpoint / Backup verify
// ---------------------------------------------------------------------------

async fn checkpoint_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::checkpoint(&state.db).await?;
    Ok(Json(result))
}

async fn backup_verify_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = engram_lib::admin::verify_backup(&state.db).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Backup download
// ---------------------------------------------------------------------------

async fn backup_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&auth)?;
    // SECURITY/TOCTOU: use a UUID-bearing filename inside the OS temp dir so
    // two admin requests landing in the same second cannot collide on the
    // same path, and a predictable path cannot be pre-created by a local
    // unprivileged user to redirect VACUUM INTO.
    let tmp_path = std::env::temp_dir().join(format!(
        "engram-backup-{}-{}.db",
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    ));
    let tmp = tmp_path.to_string_lossy().to_string();
    // SQLite's VACUUM INTO requires a string literal; embedding the UUID
    // filename keeps the statement immune to user-controlled input.
    if tmp.contains('\'') {
        return Err(AppError(engram_lib::EngError::Internal(
            "backup path contains a single quote".into(),
        )));
    }
    state
        .db
        .conn
        .execute(&format!("VACUUM INTO '{}'", tmp), ())
        .await
        .map_err(engram_lib::EngError::Database)?;
    let bytes = tokio::fs::read(&tmp)
        .await
        .map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?;
    let _ = tokio::fs::remove_file(&tmp).await;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"engram-backup.db\"",
            ),
        ],
        bytes,
    ))
}

// ---------------------------------------------------------------------------
// Export (user-scoped, any authenticated user)
// ---------------------------------------------------------------------------

async fn export_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let result = engram_lib::admin::export_user_data(&state.db, auth.user_id).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Reset (user's own data only)
// ---------------------------------------------------------------------------

async fn reset_user(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let uid = auth.user_id;
    let tables = &[
        "DELETE FROM memories WHERE user_id = ?1",
        "DELETE FROM conversations WHERE user_id = ?1",
        "DELETE FROM episodes WHERE user_id = ?1",
        "DELETE FROM user_preferences WHERE user_id = ?1",
        "DELETE FROM structured_facts WHERE memory_id IN (SELECT id FROM memories WHERE user_id = ?1)",
    ];
    let mut total = 0i64;
    for sql in tables {
        total += state
            .db
            .conn
            .execute(sql, libsql::params![uid])
            .await
            .map_err(engram_lib::EngError::Database)? as i64;
    }
    Ok(Json(json!({ "deleted_rows": total, "user_id": uid })))
}

// ---------------------------------------------------------------------------
// Communities + Cooccurrences
// ---------------------------------------------------------------------------

async fn detect_communities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = communities::detect_communities(&state.db, auth.user_id, 100).await?;
    to_json(result)
}

async fn rebuild_cooccurrences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let pairs = cooccurrence::rebuild_cooccurrences(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "rebuilt_pairs": pairs })))
}

// ---------------------------------------------------------------------------
// PageRank rebuild
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AdminPageRankQuery {
    user_id: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct VectorSyncReplayBody {
    limit: Option<usize>,
}

async fn admin_vector_sync_replay(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<VectorSyncReplayBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let limit = body.and_then(|Json(b)| b.limit).unwrap_or(200).min(5000);
    let report = engram_lib::memory::replay_vector_sync_pending(&state.db, limit).await?;
    to_json(report)
}

// ---------------------------------------------------------------------------
// Safe mode exit
// ---------------------------------------------------------------------------

async fn post_safe_mode_exit(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    engram_lib::admin::clear_crash_window(&state.db).await?;
    state.safe_mode.store(false, Ordering::Relaxed);
    tracing::info!(user_id = auth.user_id, "safe mode exited");
    Ok(Json(json!({ "safe_mode": false })))
}

async fn admin_pagerank_rebuild(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<AdminPageRankQuery>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    match params.user_id {
        Some(uid) => {
            let scores =
                engram_lib::graph::pagerank::compute_pagerank_for_user(&state.db, uid).await?;
            let count = scores.len();
            engram_lib::graph::pagerank::persist_pagerank(&state.db, uid, &scores).await?;
            Ok(Json(json!({
                "success": true,
                "users_updated": 1,
                "memories_updated": count,
            })))
        }
        None => {
            let users_updated = engram_lib::graph::pagerank::rebuild_all_users(&state.db).await?;
            Ok(Json(json!({
                "success": true,
                "users_updated": users_updated,
            })))
        }
    }
}
