use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Instant;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::auth::{create_key, AuthContext, Scope};
use kleos_lib::cred::ProxyResponse;
use kleos_lib::graph::{communities, cooccurrence};

mod types;
use types::{
    AdminCredProxyBody, AdminCredResolveBody, AdminPageRankQuery, BootstrapBody, ColdStorageParams,
    DeprovisionBody, GcBody, MaintenanceBody, MigrateDownBody, PitrPrepareBody, ProvisionBody,
    ReembedBody, ResetBody, VectorRebuildIndexBody, VectorSyncReplayBody,
};

fn require_admin(auth: &AuthContext) -> Result<(), AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }
    Ok(())
}

fn to_json<T: serde::Serialize>(v: T) -> Result<Json<Value>, AppError> {
    serde_json::to_value(v)
        .map(Json)
        .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))
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
        .route("/admin/reset", post(reset_user))
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
        // ANN index rebuild
        .route(
            "/admin/vector/rebuild-index",
            post(admin_vector_rebuild_index),
        )
        // Point-in-time recovery
        .route("/admin/pitr/snapshots", get(admin_pitr_snapshots))
        .route("/admin/pitr/prepare-restore", post(admin_pitr_prepare))
        // Migrations
        .route("/admin/migrations", get(admin_migration_status))
        .route("/admin/migrations/down", post(admin_migrate_down))
        // Brain instincts
        .route(
            "/admin/brain/instincts/reapply",
            post(admin_reapply_instincts),
        )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn count_rows(state: &AppState, sql: &str) -> Result<i64, AppError> {
    let sql = sql.to_string();
    state
        .db
        .read(move |conn| {
            conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(AppError)
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

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
    let mut throttle = bootstrap_throttle()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    let mut throttle = bootstrap_throttle()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    // SECURITY (SEC-LOW-1): pure constant-time comparison without a length
    // short-circuit. The prior `len != len` guard leaked secret length via
    // timing. When lengths differ, ct_eq returns 0 anyway, so this is safe.
    use subtle::ConstantTimeEq;
    if supplied.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1 {
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
        .write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO app_state (key, value, updated_at) \
                 VALUES ('bootstrap_claimed', datetime('now'), datetime('now'))",
                [],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(AppError)?;

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

    // SECURITY (MT-F15): user_id=1 is the reserved operator sentinel.
    // Insert the row explicitly so later AUTOINCREMENT-driven user
    // creation never collides with it and so every tenant-scoped query
    // has a real FK target. INSERT OR IGNORE is idempotent.
    state
        .db
        .write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, username, role, is_admin) \
                 VALUES (1, 'operator', 'admin', 1)",
                [],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(AppError)?;

    let scopes = vec![Scope::Read, Scope::Write, Scope::Admin];
    let (key, raw_key) = create_key(&state.db, 1, "admin", scopes, None).await?;

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
    let rows = kleos_lib::admin::list_state(&state.db).await?;
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
        kleos_lib::admin::upsert_state(&state.db, key, &v).await?;
        updated += 1;
    }
    Ok(Json(json!({ "updated": updated })))
}

// ---------------------------------------------------------------------------
// GC
// ---------------------------------------------------------------------------

async fn admin_gc(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<GcBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let uid = body.and_then(|b| b.user_id);
    let result = kleos_lib::admin::gc(&state.db, uid).await?;
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
    let result = kleos_lib::admin::compact(&state.db).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Re-embed
// ---------------------------------------------------------------------------

async fn admin_reembed(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<ReembedBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let uid = body.and_then(|b| b.user_id);
    let cleared = kleos_lib::admin::reembed_all(&state.db, uid).await?;
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
    let indexed = kleos_lib::admin::rebuild_fts(&state.db).await?;
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
    let memories = kleos_lib::admin::get_memories_without_facts(&state.db, 500).await?;
    let processed = memories.len() as i64;
    let mut facts_created = 0i32;
    for (memory_id, content, user_id) in memories {
        if let Ok(stats) = kleos_lib::intelligence::extraction::fast_extract_facts(
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
    let result = kleos_lib::admin::get_schema(&state.db).await?;
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
        "ready": state.embedder.read().await.is_some(),
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
    let result = kleos_lib::admin::scale_report(&state.db).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Cold storage stats
// ---------------------------------------------------------------------------

async fn cold_storage_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ColdStorageParams>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::cold_storage_stats(&state.db, params.days).await?;
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
            "ready": state.embedder.read().await.is_some(),
            "model": state.config.embedding_model,
        },
        "reranker": {
            "ready": state.reranker.read().await.is_some(),
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
    let stats = kleos_lib::jobs::get_job_stats(&state.db).await?;
    to_json(stats)
}

async fn admin_cred_resolve(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<AdminCredResolveBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    // SECURITY (SEC-LOW-4): use key_id for audit trail instead of
    // auth.key.name which is user-controlled and could be misleading.
    let agent = &format!("key:{}", auth.key.id);

    if let Some(text) = body.text.as_deref() {
        let resolved = state
            .credd
            .resolve_text_with_options(&state.db, auth.user_id, agent, text, body.raw)
            .await?;
        return Ok(Json(json!({ "text": resolved })));
    }

    let service = body.service.as_deref().ok_or_else(|| {
        AppError(kleos_lib::EngError::InvalidInput(
            "service is required".into(),
        ))
    })?;
    let key = body
        .key
        .as_deref()
        .ok_or_else(|| AppError(kleos_lib::EngError::InvalidInput("key is required".into())))?;

    let value = if body.raw {
        state
            .credd
            .get_raw(&state.db, agent, service, key)
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

async fn admin_cred_proxy(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<AdminCredProxyBody>,
) -> Result<Json<ProxyResponse>, AppError> {
    require_admin(&auth)?;
    let agent = format!("key:{}", auth.key.id);
    let response = state
        .credd
        .proxy(
            &state.db,
            auth.user_id,
            &agent,
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
    let result = kleos_lib::admin::get_maintenance(&state.db).await?;
    to_json(result)
}

async fn post_maintenance_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<MaintenanceBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result =
        kleos_lib::admin::set_maintenance(&state.db, body.enabled, body.message.as_deref()).await?;
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
    let result = kleos_lib::admin::get_sla(&state.db).await?;
    to_json(result)
}

async fn admin_sla_reset(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let ts = chrono::Utc::now().to_rfc3339();
    kleos_lib::admin::upsert_state(&state.db, "sla_reset_at", &ts).await?;
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
    let quotas = kleos_lib::quota::list_quotas(&state.db).await?;
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
    Json(body): Json<kleos_lib::quota::TenantQuota>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    kleos_lib::quota::upsert_quota(&state.db, &body).await?;
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
    let rows = kleos_lib::admin::get_usage(&state.db).await?;
    to_json(rows)
}

async fn admin_tenants(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = kleos_lib::admin::get_tenants(&state.db).await?;
    to_json(rows)
}

// ---------------------------------------------------------------------------
// Provision / Deprovision
// ---------------------------------------------------------------------------

async fn provision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ProvisionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::provision_tenant(
        &state.db,
        &body.username,
        body.email.as_deref(),
        &body.role,
    )
    .await?;
    let json_result = to_json(result)?;
    Ok((StatusCode::CREATED, json_result))
}

async fn deprovision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<DeprovisionBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let removed = kleos_lib::admin::deprovision_tenant(&state.db, body.user_id).await?;
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
    let result = kleos_lib::admin::checkpoint(&state.db).await?;
    Ok(Json(result))
}

async fn backup_verify_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::verify_backup(&state.db).await?;
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
        "kleos-backup-{}-{}.db",
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    ));
    let tmp = tmp_path.to_string_lossy().to_string();
    // SQLite's VACUUM INTO requires a string literal; embedding the UUID
    // filename keeps the statement immune to user-controlled input.
    if tmp.contains('\'') {
        return Err(AppError(kleos_lib::EngError::Internal(
            "backup path contains a single quote".into(),
        )));
    }
    let vacuum_sql = format!("VACUUM INTO '{}'", tmp);
    state
        .db
        .write(move |conn| {
            conn.execute(&vacuum_sql, [])
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(AppError)?;
    let bytes = tokio::fs::read(&tmp)
        .await
        .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;
    if let Err(e) = tokio::fs::remove_file(&tmp).await {
        tracing::warn!(path = %tmp, error = %e, "failed to remove temporary backup file");
    }
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"kleos-backup.db\"",
            ),
        ],
        bytes,
    ))
}

// ---------------------------------------------------------------------------
// Point-in-time recovery
// ---------------------------------------------------------------------------

/// Name of the jailed restore directory under `data_dir`. PITR prepared files
/// land here and nowhere else.
const PITR_RESTORE_SUBDIR: &str = "pitr-restore";
/// Hard cap on caller-supplied filenames. Matches common filesystem NAME_MAX.
const PITR_DEST_MAX_LEN: usize = 200;

/// SECURITY (SEC-CRIT-1): Validate an admin-supplied `dest_path` for POST
/// `/admin/pitr/prepare-restore` and resolve it to an absolute path inside a
/// jailed directory under `data_dir`. Treats the caller input as a filename
/// only (no directory components) and refuses to overwrite an existing file.
///
/// Rejects: empty, NUL, absolute paths, any `/` or `\\`, any `..`, leading `.`
/// or `-`, anything above `PITR_DEST_MAX_LEN`, and characters outside
/// `[A-Za-z0-9._-]`. Canonicalises the jail and asserts the joined candidate
/// stays inside it (defence against a pre-existing symlink in the jail root).
fn sanitize_pitr_dest(data_dir: &str, raw: &str) -> Result<std::path::PathBuf, AppError> {
    let invalid = |msg: &str| -> AppError {
        AppError(kleos_lib::EngError::InvalidInput(format!(
            "dest_path {msg}; must be a bare filename restricted to \
             [A-Za-z0-9._-], <= {PITR_DEST_MAX_LEN} chars, and must not exist"
        )))
    };

    if raw.is_empty() {
        return Err(invalid("is empty"));
    }
    if raw.len() > PITR_DEST_MAX_LEN {
        return Err(invalid("too long"));
    }
    if raw.as_bytes().contains(&0) {
        return Err(invalid("contains NUL"));
    }
    if raw.contains('/') || raw.contains('\\') {
        return Err(invalid("contains path separator"));
    }
    if raw == "." || raw == ".." || raw.contains("..") {
        return Err(invalid("contains traversal"));
    }
    if raw.starts_with('.') || raw.starts_with('-') {
        return Err(invalid("must not start with '.' or '-'"));
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(invalid("contains disallowed characters"));
    }

    let data_dir_path = std::path::PathBuf::from(data_dir);
    if !data_dir_path.is_absolute() {
        return Err(AppError(kleos_lib::EngError::Internal(
            "data_dir is not absolute; refusing to resolve PITR restore path".into(),
        )));
    }
    let jail = data_dir_path.join(PITR_RESTORE_SUBDIR);
    std::fs::create_dir_all(&jail).map_err(|e| {
        AppError(kleos_lib::EngError::Internal(format!(
            "failed to create PITR restore dir {}: {e}",
            jail.display()
        )))
    })?;
    let jail_canon = std::fs::canonicalize(&jail).map_err(|e| {
        AppError(kleos_lib::EngError::Internal(format!(
            "failed to canonicalize PITR restore dir {}: {e}",
            jail.display()
        )))
    })?;

    let candidate = jail_canon.join(raw);
    if !candidate.starts_with(&jail_canon) {
        return Err(invalid("resolves outside restore jail"));
    }
    if candidate.symlink_metadata().is_ok() {
        return Err(invalid("target already exists"));
    }

    Ok(candidate)
}

async fn admin_pitr_snapshots(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let dir =
        crate::background::resolve_backup_dir(&state.config.data_dir, &state.config.backup_dir);
    let snapshots = kleos_lib::db::pitr::list_snapshots(&dir);
    Ok(Json(json!({ "snapshots": snapshots })))
}

async fn admin_pitr_prepare(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<PitrPrepareBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let target = chrono::DateTime::parse_from_rfc3339(&body.target)
        .map_err(|e| {
            AppError(kleos_lib::EngError::InvalidInput(format!(
                "target must be RFC3339: {e}"
            )))
        })?
        .with_timezone(&chrono::Utc);
    let dir =
        crate::background::resolve_backup_dir(&state.config.data_dir, &state.config.backup_dir);
    // SECURITY (SEC-CRIT-1): sandbox dest_path into data_dir/pitr-restore.
    // Previously any absolute path was accepted, letting an admin token write
    // DB snapshots anywhere the process could reach.
    let dest = sanitize_pitr_dest(&state.config.data_dir, &body.dest_path)?;
    let prepared = kleos_lib::db::pitr::prepare_restore(&dir, target, &dest).await?;
    Ok(Json(json!(prepared)))
}

// ---------------------------------------------------------------------------
// Export (user-scoped, any authenticated user)
// ---------------------------------------------------------------------------

async fn export_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // SECURITY (SEC-MED-1): gate admin-path export behind admin scope.
    // User-facing export lives in the portability module.
    require_admin(&auth)?;
    let result = kleos_lib::admin::export_user_data(&state.db, auth.user_id).await?;
    to_json(result)
}

// ---------------------------------------------------------------------------
// Reset (user's own data only)
// ---------------------------------------------------------------------------

async fn reset_user(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ResetBody>,
) -> Result<Json<Value>, AppError> {
    // SECURITY (SEC-MED-2): destructive reset must require admin scope.
    require_admin(&auth)?;
    // Require explicit confirmation phrase to prevent accidental data loss.
    if body.confirm.as_deref() != Some("WIPE_ALL_MEMORIES") {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "/admin/reset requires {\"confirm\":\"WIPE_ALL_MEMORIES\"} body".into(),
        )));
    }
    let uid = auth.user_id;
    // Tables that are wiped by user_id (single-tenant: uid == the only tenant)
    let uid_tables = &[
        "DELETE FROM conversations WHERE user_id = ?1",
        "DELETE FROM user_preferences WHERE user_id = ?1",
    ];
    // Tables wiped whole-monolith (no user_id column, or single-tenant equivalent)
    let global_tables = &[
        "DELETE FROM episodes",
        "DELETE FROM memories",
        "DELETE FROM structured_facts WHERE memory_id IN (SELECT id FROM memories)",
    ];
    let mut total = 0i64;
    for sql in uid_tables {
        let sql_owned = sql.to_string();
        total += state
            .db
            .write(move |conn| {
                conn.execute(&sql_owned, params![uid])
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
            })
            .await
            .map_err(AppError)? as i64;
    }
    for sql in global_tables {
        let sql_owned = sql.to_string();
        total += state
            .db
            .write(move |conn| {
                conn.execute(&sql_owned, [])
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
            })
            .await
            .map_err(AppError)? as i64;
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

async fn admin_vector_sync_replay(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<VectorSyncReplayBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let limit = body.and_then(|Json(b)| b.limit).unwrap_or(200).min(5000);
    let report = kleos_lib::memory::replay_vector_sync_pending(&state.db, limit).await?;
    to_json(report)
}

// ---------------------------------------------------------------------------
// Rebuild ANN index (IVF_HNSW_PQ)
// ---------------------------------------------------------------------------

async fn admin_vector_rebuild_index(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<VectorRebuildIndexBody>>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let replace = body.and_then(|Json(b)| b.replace).unwrap_or(false);

    let Some(vector_index) = state.db.vector_index.clone() else {
        return Ok(Json(json!({
            "rebuilt": false,
            "row_count": 0usize,
            "reason": "vector index not configured",
        })));
    };

    let row_count = vector_index.count().await.unwrap_or(0);
    let rebuilt = vector_index.rebuild_index(replace).await?;
    Ok(Json(json!({
        "rebuilt": rebuilt,
        "row_count": row_count,
        "min_rows_for_index": kleos_lib::vector::lance::MIN_ROWS_FOR_INDEX,
    })))
}

// ---------------------------------------------------------------------------
// Safe mode exit
// ---------------------------------------------------------------------------

async fn post_safe_mode_exit(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    kleos_lib::admin::clear_crash_window(&state.db).await?;
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
                kleos_lib::graph::pagerank::compute_pagerank_for_user(&state.db, uid).await?;
            let count = scores.len();
            kleos_lib::graph::pagerank::persist_pagerank(&state.db, &scores).await?;
            Ok(Json(json!({
                "success": true,
                "users_updated": 1,
                "memories_updated": count,
            })))
        }
        None => {
            let users_updated = kleos_lib::graph::pagerank::rebuild_all_users(&state.db).await?;
            Ok(Json(json!({
                "success": true,
                "users_updated": users_updated,
            })))
        }
    }
}

// ---------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------

/// GET /admin/migrations -- return current migration status (version, pending, revertible).
async fn admin_migration_status(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let status = kleos_lib::db::migrations::migration_status(&state.db).await?;
    to_json(status)
}

/// POST /admin/migrations/down -- roll the schema back to target_version.
/// When dry_run is true, returns the plan without executing.
async fn admin_migrate_down(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<MigrateDownBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let plan =
        kleos_lib::db::migrations::migrate_down(&state.db, body.target_version, body.dry_run)
            .await?;
    to_json(plan)
}

async fn admin_reapply_instincts(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(kleos_lib::EngError::Internal(
            "brain backend not available".to_string(),
        ))
    })?;
    let resp = brain.reapply_instincts().await?;
    Ok(Json(json!({
        "ok": resp.ok,
        "data": resp.data,
    })))
}

#[cfg(test)]
mod pitr_sandbox_tests {
    use super::*;
    use std::fs;

    fn unique_data_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pitr-sandbox-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_empty() {
        let dd = unique_data_dir();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    #[test]
    fn rejects_absolute() {
        let dd = unique_data_dir();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "/etc/passwd").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    #[test]
    fn rejects_traversal() {
        let dd = unique_data_dir();
        for bad in ["..", "../x", "a/../b", "..hidden", "a..b"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(
                matches!(err.0, kleos_lib::EngError::InvalidInput(_)),
                "expected InvalidInput for {bad}"
            );
        }
    }

    #[test]
    fn rejects_separators() {
        let dd = unique_data_dir();
        for bad in ["a/b", "a\\b", "foo/"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    #[test]
    fn rejects_unicode_and_exotic() {
        let dd = unique_data_dir();
        for bad in ["rm -rf", "a\0b", "name with space", "héllo"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    #[test]
    fn rejects_leading_dash_or_dot() {
        let dd = unique_data_dir();
        for bad in ["-rf", ".env", ".ssh"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    #[test]
    fn rejects_non_absolute_data_dir() {
        let err = sanitize_pitr_dest("relative/dir", "restore.db").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::Internal(_)));
    }

    #[test]
    fn accepts_plain_filename_and_creates_jail() {
        let dd = unique_data_dir();
        let out = sanitize_pitr_dest(dd.to_str().unwrap(), "restore.db").unwrap();
        let jail = dd.join(PITR_RESTORE_SUBDIR);
        assert!(jail.is_dir(), "jail dir must be created");
        assert!(out.starts_with(std::fs::canonicalize(&jail).unwrap()));
        assert_eq!(out.file_name().unwrap(), "restore.db");
    }

    #[test]
    fn rejects_overwrite_existing() {
        let dd = unique_data_dir();
        let jail = dd.join(PITR_RESTORE_SUBDIR);
        fs::create_dir_all(&jail).unwrap();
        fs::write(jail.join("already.db"), b"x").unwrap();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "already.db").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    #[test]
    fn rejects_too_long_name() {
        let dd = unique_data_dir();
        let long_name = "a".repeat(PITR_DEST_MAX_LEN + 1);
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), &long_name).unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }
}
