use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
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
    AdminCredProxyBody, AdminCredResolveBody, AdminPageRankQuery, AsyncDeprovisionBody,
    BackfillEntitiesBody, BootstrapBody, ColdStorageParams, DeprovisionBody, GcBody,
    MaintenanceBody, MigrateDownBody, PitrPrepareBody, ProvisionBody, RecentDeprovisionQuery,
    ReembedBody, ResetBody, SetQuotaBody, SkipShardBody, VectorRebuildIndexBody,
    VectorSyncReplayBody,
};

/// Reject the request with a 403 unless the auth context carries the admin scope.
fn require_admin(auth: &AuthContext) -> Result<(), AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }
    Ok(())
}

/// Serialize an admin handler return value into the standard `Json<Value>` response wrapper.
fn to_json<T: serde::Serialize>(v: T) -> Result<Json<Value>, AppError> {
    serde_json::to_value(v)
        .map(Json)
        .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))
}

/// Extract the tenant registry or return a 501 error.
fn require_registry(state: &AppState) -> Result<&kleos_lib::tenant::TenantRegistry, AppError> {
    state.tenant_registry.as_deref().ok_or_else(|| {
        AppError(kleos_lib::EngError::NotImplemented(
            "tenant registry not configured".into(),
        ))
    })
}

/// Mount the admin router with every operator-only route, gated by `require_admin` at the handler level.
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
        .route("/admin/entities/backfill", post(backfill_entities))
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
        // E1 async deprovision
        .route("/admin/deprovisions/stuck", get(list_stuck_deprovisions))
        .route("/admin/deprovisions/recent", get(list_recent_deprovisions))
        .route(
            "/admin/deprovision/{id}/status",
            get(get_deprovision_status),
        )
        .route(
            "/admin/deprovision/{id}/force-retry",
            post(force_retry_deprovision),
        )
        .route(
            "/admin/deprovision/{id}/skip-shard",
            post(skip_shard_deprovision),
        )
        .route(
            "/admin/deprovision/{user_id}",
            post(deprovision_tenant_async),
        )
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
        // Vector health diagnostic
        .route("/admin/vector_health", get(admin_vector_health))
        // Chunk + embedding backfill (Phase 2 rollout)
        .route("/admin/backfill_chunks", post(admin_backfill_chunks))
        // Per-chunk LanceDB vector index rebuild from existing SQLite rows
        .route("/admin/vector/chunk-sync", post(admin_vector_chunk_sync))
        // Point-in-time recovery
        .route("/admin/pitr/snapshots", get(admin_pitr_snapshots))
        .route("/admin/pitr/prepare-restore", post(admin_pitr_prepare))
        // Migrations
        .route("/admin/migrations", get(admin_migration_status))
        .route("/admin/migrations/down", post(admin_migrate_down))
        // Monolith drain (move data from system DB to tenant shards)
        .route("/admin/monolith/status", get(admin_monolith_status))
        .route("/admin/monolith/drain", post(admin_monolith_drain))
        // Brain instincts
        .route(
            "/admin/brain/instincts/reapply",
            post(admin_reapply_instincts),
        )
        // E2 shard quota management
        .route(
            "/admin/quota/{user_id}",
            get(get_quota_status).put(set_quota),
        )
        .route("/admin/quota/{user_id}/recompute", post(recompute_quota))
}

// --- Helpers ---

async fn count_rows(state: &AppState, sql: &str) -> Result<i64, AppError> {
    let sql = sql.to_string();
    state
        .db
        .read(move |conn| Ok(conn.query_row(&sql, [], |row| row.get::<_, i64>(0))?))
        .await
        .map_err(AppError)
}

// --- Bootstrap ---

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

/// Sliding-window counter that backs off `POST /admin/bootstrap` after repeated failures.
struct BootstrapThrottle {
    failures: u32,
    window_start: Instant,
}

/// Process-wide singleton holding the bootstrap-throttle state.
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

/// Record one bootstrap failure against the throttle and trigger the back-off window when the threshold is hit.
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

/// POST /admin/bootstrap -- initial owner enrollment that seeds the database with an admin API key.
#[tracing::instrument(skip_all)]
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
    let Ok(expected) = kleos_lib::kleos_env("BOOTSTRAP_SECRET") else {
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
            Ok(conn.execute(
                "INSERT OR IGNORE INTO app_state (key, value, updated_at) \
                 VALUES ('bootstrap_claimed', datetime('now'), datetime('now'))",
                [],
            )?)
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
            Ok(conn.execute(
                "INSERT OR IGNORE INTO users (id, username, role, is_admin) \
                 VALUES (1, 'operator', 'admin', 1)",
                [],
            )?)
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

// --- Stats ---

#[tracing::instrument(skip_all)]
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

// --- Settings (app_state key-value) ---

#[tracing::instrument(skip_all)]
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

/// PUT /admin/settings -- upsert a settings row by key.
#[tracing::instrument(skip_all)]
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

// --- GC ---

#[tracing::instrument(skip_all)]
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

// --- Compact ---

#[tracing::instrument(skip_all)]
async fn admin_compact(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::compact(&state.db).await?;
    to_json(result)
}

// --- Re-embed ---

#[tracing::instrument(skip_all)]
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

// --- Rebuild FTS ---

#[tracing::instrument(skip_all)]
async fn admin_rebuild_fts(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let indexed = kleos_lib::admin::rebuild_fts(&state.db).await?;
    Ok(Json(json!({ "indexed": indexed })))
}

// --- Refresh cache (no-op signal) ---

#[tracing::instrument(skip_all)]
async fn refresh_cache(
    State(_state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    Ok(Json(
        json!({ "status": "ok", "message": "cache refresh signaled" }),
    ))
}

// --- Backfill facts ---

#[tracing::instrument(skip_all)]
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

// --- Backfill entities ---

/// One-shot admin handler to backfill entity extraction for historic memories.
///
/// Processes memories that have no rows in `memory_entities`, calling
/// `extract_and_link_entities` for each. Returns a summary with counts of
/// processed memories, entities created (including upserts), links created,
/// and elapsed time. Does not trigger automatically; invoke this endpoint
/// once after verifying forward-wiring is correct on new memories.
#[tracing::instrument(skip_all)]
async fn backfill_entities(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<BackfillEntitiesBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    let limit = match body.max_memories {
        Some(max) => max.min(body.batch_size),
        None => body.batch_size,
    };

    let memories = kleos_lib::admin::get_memories_without_entity_links(&state.db, limit).await?;

    let started = std::time::Instant::now();
    let processed = memories.len() as i64;
    let mut entities_created: i64 = 0;
    let mut links_created: i64 = 0;

    for (memory_id, content) in memories {
        // Use user_id = 1 as the tenant-shard owner for backfill; the field is
        // ignored internally (post migration v35) but must be supplied for the
        // function signature. If we ever need per-memory user_id we can add it
        // to the backfill query.
        match kleos_lib::graph::entities::extract_and_link_entities(
            &state.db, memory_id, &content, 1,
        )
        .await
        {
            Ok(entities) => {
                let count = entities.len() as i64;
                entities_created += count;
                links_created += count;
            }
            Err(e) => {
                tracing::warn!(memory_id, "entity backfill failed for memory: {}", e);
            }
        }
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    Ok(Json(json!({
        "processed": processed,
        "entities_created": entities_created,
        "links_created": links_created,
        "duration_ms": duration_ms,
    })))
}

// --- Schema ---

#[tracing::instrument(skip_all)]
async fn admin_schema(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::get_schema(&state.db).await?;
    to_json(result)
}

// --- Embedding info ---

#[tracing::instrument(skip_all)]
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

// --- Scale report ---

#[tracing::instrument(skip_all)]
async fn scale_report_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::scale_report(&state.db).await?;
    Ok(Json(result))
}

// --- Cold storage stats ---

#[tracing::instrument(skip_all)]
async fn cold_storage_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ColdStorageParams>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::cold_storage_stats(&state.db, params.days).await?;
    Ok(Json(result))
}

// --- Providers ---

#[tracing::instrument(skip_all)]
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

// --- Tasks (job queue stats) ---

#[tracing::instrument(skip_all)]
async fn admin_tasks(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let stats = kleos_lib::jobs::get_job_stats(&state.db).await?;
    to_json(stats)
}

/// POST /admin/cred/resolve -- ask credd to resolve a cred slot for a named agent.
#[tracing::instrument(skip_all)]
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

/// POST /admin/cred/proxy -- proxy a cred fetch through the server with operator-scoped audit.
#[tracing::instrument(skip_all)]
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

// --- Maintenance ---

#[tracing::instrument(skip_all)]
async fn get_maintenance_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::get_maintenance(&state.db).await?;
    to_json(result)
}

/// POST /admin/maintenance -- enter or exit maintenance mode.
#[tracing::instrument(skip_all)]
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

// --- SLA ---

#[tracing::instrument(skip_all)]
async fn admin_sla(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::get_sla(&state.db).await?;
    to_json(result)
}

/// POST /admin/sla/reset -- clear the rolling SLA metrics counters.
#[tracing::instrument(skip_all)]
async fn admin_sla_reset(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let ts = chrono::Utc::now().to_rfc3339();
    kleos_lib::admin::upsert_state(&state.db, "sla_reset_at", &ts).await?;
    Ok(Json(json!({ "status": "ok", "reset_at": ts })))
}

// --- Quotas ---

#[tracing::instrument(skip_all)]
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
    Ok(Json(json!({ "items": result, "count": result.len() })))
}

/// PUT /admin/quotas -- upsert per-user storage and rate-limit quotas.
#[tracing::instrument(skip_all)]
async fn put_quotas(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<kleos_lib::quota::TenantQuota>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    kleos_lib::quota::upsert_quota(&state.db, &body).await?;
    Ok(Json(json!({ "status": "ok", "user_id": body.user_id })))
}

// --- Usage + Tenants ---

#[tracing::instrument(skip_all)]
async fn admin_usage(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = kleos_lib::admin::get_usage(&state.db).await?;
    let count = rows.len();
    let items =
        serde_json::to_value(rows).map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
    Ok(Json(json!({ "items": items, "count": count })))
}

/// GET /admin/tenants -- list all tenants known to the server.
#[tracing::instrument(skip_all)]
async fn admin_tenants(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let rows = kleos_lib::admin::get_tenants(&state.db).await?;
    let count = rows.len();
    let items =
        serde_json::to_value(rows).map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
    Ok(Json(json!({ "items": items, "count": count })))
}

// --- Provision / Deprovision ---

#[tracing::instrument(skip_all)]
async fn provision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ProvisionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_admin(&auth)?;
    // E1: check tombstone hold before allowing re-provisioning of a deleted username.
    // Query deletions_log by target_username since the user_id doesn't exist yet.
    if let Some(ref registry) = state.tenant_registry {
        let hold_days: i64 = std::env::var("KLEOS_TOMBSTONE_HOLD_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(90);
        if hold_days > 0 {
            if let Some(hold_until) = registry
                .registry_db()
                .check_tombstone_hold_by_username(&body.username, hold_days)?
            {
                return Err(AppError(kleos_lib::EngError::Conflict(format!(
                    "username '{}' is under tombstone hold until {}",
                    body.username, hold_until
                ))));
            }
        }
    }
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

/// POST /tenants/deprovision -- legacy sync handler; routes to async E1 teardown
/// when tenant registry is present, falls back to monolith-only cleanup otherwise.
#[tracing::instrument(skip_all)]
async fn deprovision_tenant(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<DeprovisionBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    if let Some(ref registry) = state.tenant_registry {
        let dep_id = kleos_lib::tenant::teardown::begin_deprovision(
            registry,
            &state.db,
            body.user_id,
            auth.user_id,
            String::new(),
        )
        .await?;
        return Ok(Json(json!({
            "removed": true,
            "user_id": body.user_id,
            "deprovision_id": dep_id.as_str(),
            "async": true,
        })));
    }
    let removed = kleos_lib::admin::deprovision_tenant(&state.db, body.user_id).await?;
    Ok(Json(json!({ "removed": removed, "user_id": body.user_id })))
}

// --- Checkpoint / Backup verify ---

#[tracing::instrument(skip_all)]
async fn checkpoint_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::checkpoint(&state.db).await?;
    Ok(Json(result))
}

/// POST /admin/backup/verify -- run an integrity check over the most recent backup artifact.
#[tracing::instrument(skip_all)]
async fn backup_verify_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = kleos_lib::admin::verify_backup(&state.db).await?;
    to_json(result)
}

// --- Backup download ---

#[tracing::instrument(skip_all)]
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
        .write(move |conn| Ok(conn.execute(&vacuum_sql, [])?))
        .await
        .map_err(AppError)?;

    // M1: integrity-check the backup BEFORE streaming it. Without this,
    // operators could save a corrupted file and only discover it at restore
    // time. integrity_check runs SQLite's `PRAGMA integrity_check` against
    // the snapshot and returns early on any reported issue.
    match kleos_lib::db::backup::integrity_check(&tmp_path).await {
        Ok(messages) if !messages.is_empty() => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(AppError(kleos_lib::EngError::Internal(format!(
                "backup integrity check failed: {}",
                messages.join("; ")
            ))));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(AppError(kleos_lib::EngError::Internal(format!(
                "backup integrity check failed: {e}"
            ))));
        }
        Ok(_) => {}
    }

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

// --- Point-in-time recovery ---

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
    // L7: avoid TOCTOU between the existence check and the actual restore
    // write. Atomically claim the path via O_EXCL create. On EEXIST we map
    // to the same caller-visible "target already exists" error. On success
    // we keep the empty placeholder file -- the restore step owns the
    // canonical write to this path.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(invalid("target already exists"));
        }
        Err(e) => {
            return Err(invalid(&format!("failed to claim restore path: {e}")));
        }
    }

    Ok(candidate)
}

/// GET /admin/pitr/snapshots -- list available point-in-time recovery snapshots.
#[tracing::instrument(skip_all)]
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

/// POST /admin/pitr/prepare -- stage a PITR snapshot into a sandbox directory for inspection.
#[tracing::instrument(skip_all)]
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

// --- Export (user-scoped, any authenticated user) ---

#[tracing::instrument(skip_all)]
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

// --- Reset (user's own data only) ---

// C-R3-002 / H-R3-005: scope to ResolvedDb so the unfiltered DELETEs only
// hit the caller's shard, not the monolith. Each shard contains exactly one
// tenant's data; on the monolith path (user_id=1 / system) the operation
// only affects the caller's own rows because user_id=1 is the only resident
// of monolith memory tables in a properly-sharded deployment.
//
// Previous behavior was a global wipe disguised as per-user: the function
// name said reset_user, the response echoed user_id, but the SQL ran
// "DELETE FROM memories" with no predicate against the monolith. Operators
// reading the JSON saw user_id and assumed scope; they got cross-tenant
// destruction.
#[tracing::instrument(skip_all)]
async fn reset_user(
    Auth(auth): Auth,
    crate::extractors::ResolvedDb(db): crate::extractors::ResolvedDb,
    Json(body): Json<ResetBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    if body.confirm.as_deref() != Some("WIPE_ALL_MEMORIES") {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "/admin/reset requires {\"confirm\":\"WIPE_ALL_MEMORIES\"} body".into(),
        )));
    }
    let uid = auth.user_id;
    // structured_facts dangles off memories and is keyed by memory_id; run
    // it BEFORE DELETE FROM memories so the inner subquery still finds rows.
    let tables: &[&str] = &[
        "DELETE FROM structured_facts WHERE memory_id IN (SELECT id FROM memories)",
        "DELETE FROM conversations",
        "DELETE FROM user_preferences",
        "DELETE FROM episodes",
        "DELETE FROM memories",
    ];
    let mut total = 0i64;
    for sql in tables {
        let sql_owned = sql.to_string();
        total += db
            .write(move |conn| Ok(conn.execute(&sql_owned, [])?))
            .await
            .map_err(AppError)? as i64;
    }
    Ok(Json(json!({ "deleted_rows": total, "user_id": uid })))
}

// --- Communities + Cooccurrences ---

#[tracing::instrument(skip_all)]
async fn detect_communities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let result = communities::detect_communities(&state.db, auth.user_id, 100).await?;
    to_json(result)
}

/// POST /admin/cooccurrences/rebuild -- recompute the entity-cooccurrences index from scratch.
#[tracing::instrument(skip_all)]
async fn rebuild_cooccurrences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let pairs = cooccurrence::rebuild_cooccurrences(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "rebuilt_pairs": pairs })))
}

// --- PageRank rebuild ---

#[tracing::instrument(skip_all)]
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

// --- Rebuild ANN index (IVF_HNSW_PQ) ---

#[tracing::instrument(skip_all)]
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

// --- Vector health diagnostic ---

#[tracing::instrument(skip_all)]
async fn admin_vector_health(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    let registry = require_registry(&state)?;

    let tenants = registry.list().map_err(AppError)?;

    let mut total_lance: usize = 0;
    let mut total_chunk_lance: usize = 0;
    let mut total_active: i64 = 0;
    let mut total_chunks: i64 = 0;
    let mut total_pending: i64 = 0;
    let mut any_lance_index_built = false;
    let mut per_tenant = Vec::new();

    for row in &tenants {
        if row.status != kleos_lib::tenant::TenantStatus::Active {
            continue;
        }

        let handle = match registry.get(&row.user_id).await {
            Ok(Some(h)) => h,
            _ => continue,
        };

        let db = handle.database();

        let lance_count = if let Some(ref idx) = db.vector_index {
            idx.count().await.ok().unwrap_or(0)
        } else {
            0
        };

        let chunk_lance_count = if let Some(ref idx) = db.chunk_vector_index {
            idx.count().await.ok().unwrap_or(0)
        } else {
            0
        };

        if let Some(ref idx) = db.vector_index {
            if idx.rebuild_index(false).await.unwrap_or(false) {
                any_lance_index_built = true;
            }
        }

        let (active, chunks, pending) = db
            .read(|conn| {
                let active: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM memories WHERE is_forgotten = 0 AND is_latest = 1",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                let chunks: i64 = conn
                    .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
                    .unwrap_or(0);
                let pending: i64 = conn
                    .query_row("SELECT COUNT(*) FROM vector_sync_pending", [], |row| {
                        row.get(0)
                    })
                    .unwrap_or(0);
                Ok((active, chunks, pending))
            })
            .await
            .unwrap_or((0, 0, 0));

        if active > 0 || chunks > 0 || lance_count > 0 {
            per_tenant.push(json!({
                "tenant_id": row.tenant_id,
                "lance_row_count": lance_count,
                "chunk_lance_row_count": chunk_lance_count,
                "memories_active_count": active,
                "chunk_row_count": chunks,
                "vector_sync_pending_count": pending,
            }));
        }

        total_lance += lance_count;
        total_chunk_lance += chunk_lance_count;
        total_active += active;
        total_chunks += chunks;
        total_pending += pending;
    }

    Ok(Json(json!({
        "lance_row_count": total_lance,
        "chunk_lance_row_count": total_chunk_lance,
        "memories_active_count": total_active,
        "chunk_row_count": total_chunks,
        "lance_index_built": any_lance_index_built,
        "vector_sync_pending_count": total_pending,
        "per_tenant": per_tenant,
    })))
}

// --- Chunk + embedding backfill ---

#[tracing::instrument(skip_all)]
async fn admin_backfill_chunks(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    let embedder = state.current_embedder().await.ok_or_else(|| {
        AppError(kleos_lib::EngError::Internal(
            "no embedder configured; backfill requires an active embedding provider".into(),
        ))
    })?;

    let registry = require_registry(&state)?;

    let tenants = registry.list().map_err(AppError)?;

    let mut total_scanned = 0usize;
    let mut total_primary = 0usize;
    let mut total_chunks = 0usize;
    let mut total_failures = 0usize;
    let mut tenants_processed = 0usize;
    let mut per_tenant = Vec::new();

    for row in &tenants {
        if row.status != kleos_lib::tenant::TenantStatus::Active {
            continue;
        }

        let handle = match registry.get(&row.user_id).await {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(tenant = %row.tenant_id, error = %e, "backfill: failed to load tenant");
                total_failures += 1;
                continue;
            }
        };

        let db = handle.database();
        match kleos_lib::memory::backfill_missing_embeddings(&db, embedder.as_ref()).await {
            Ok(report) => {
                if report.scanned > 0 {
                    per_tenant.push(json!({
                        "tenant_id": row.tenant_id,
                        "scanned": report.scanned,
                        "primary_embeddings_filled": report.primary_embeddings_filled,
                        "chunk_rows_written": report.chunk_rows_written,
                        "failures": report.failures,
                    }));
                }
                total_scanned += report.scanned;
                total_primary += report.primary_embeddings_filled;
                total_chunks += report.chunk_rows_written;
                total_failures += report.failures;
            }
            Err(e) => {
                tracing::warn!(tenant = %row.tenant_id, error = %e, "backfill: tenant backfill failed");
                total_failures += 1;
            }
        }
        tenants_processed += 1;
    }

    Ok(Json(json!({
        "tenants_processed": tenants_processed,
        "scanned": total_scanned,
        "primary_embeddings_filled": total_primary,
        "chunk_rows_written": total_chunks,
        "failures": total_failures,
        "per_tenant": per_tenant,
    })))
}

// --- Per-chunk LanceDB vector index rebuild from existing SQLite rows ---

#[tracing::instrument(skip_all)]
async fn admin_vector_chunk_sync(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    let registry = require_registry(&state)?;

    let tenants = registry.list().map_err(AppError)?;
    let mut total = 0usize;
    let mut per_tenant = Vec::new();

    for row in &tenants {
        if row.status != kleos_lib::tenant::TenantStatus::Active {
            continue;
        }
        let handle = match registry.get(&row.user_id).await {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(tenant = %row.tenant_id, error = %e, "chunk-sync: failed to load tenant");
                continue;
            }
        };
        let db = handle.database();
        match kleos_lib::memory::build_lance_chunk_index_from_existing(&db).await {
            Ok(count) => {
                if count > 0 {
                    per_tenant.push(json!({
                        "tenant_id": row.tenant_id,
                        "rows_synced": count,
                    }));
                }
                total += count;
            }
            Err(e) => {
                tracing::warn!(tenant = %row.tenant_id, error = %e, "chunk-sync: rebuild failed");
                per_tenant.push(json!({
                    "tenant_id": row.tenant_id,
                    "error": e.to_string(),
                }));
            }
        }
    }

    Ok(Json(json!({
        "total_rows_synced": total,
        "per_tenant": per_tenant,
    })))
}

// --- Safe mode exit ---

#[tracing::instrument(skip_all)]
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

/// POST /admin/pagerank/rebuild -- recompute PageRank scores over the memory graph.
#[tracing::instrument(skip_all)]
async fn admin_pagerank_rebuild(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<AdminPageRankQuery>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    match params.user_id {
        Some(uid) => {
            let db = crate::extractors::resolve_db_for_user(&state, uid)
                .await
                .map_err(AppError)?;
            let scores = kleos_lib::graph::pagerank::compute_pagerank_for_user(&db, uid).await?;
            let count = scores.len();
            kleos_lib::graph::pagerank::persist_pagerank(&db, &scores).await?;
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

// --- Migrations ---

/// GET /admin/migrations -- return current migration status (version, pending, revertible).
#[tracing::instrument(skip_all)]
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
#[tracing::instrument(skip_all)]
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

/// POST /admin/instincts/reapply -- re-evaluate instinct rules across the existing corpus.
#[tracing::instrument(skip_all)]
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

// --- Monolith drain -- move data from system DB to tenant shards ---

#[tracing::instrument(skip_all)]
async fn admin_monolith_status(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    let has_table: bool = state
        .db
        .read(|conn| {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0)
                > 0;
            Ok(exists)
        })
        .await
        .unwrap_or(false);

    if !has_table {
        return Ok(Json(json!({
            "monolith_has_memories_table": false,
            "users": [],
        })));
    }

    let user_counts: Vec<(i64, i64, i64)> = state
        .db
        .read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT user_id, \
                            COUNT(*) AS total, \
                            SUM(CASE WHEN is_forgotten = 0 THEN 1 ELSE 0 END) AS active \
                     FROM memories GROUP BY user_id ORDER BY user_id",
            )?;
            let rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await
        .unwrap_or_default();

    let users: Vec<Value> = user_counts
        .iter()
        .map(|(uid, total, active)| {
            json!({
                "user_id": uid,
                "total": total,
                "active": active,
                "forgotten": total - active,
            })
        })
        .collect();

    let grand_total: i64 = user_counts.iter().map(|(_, t, _)| t).sum();
    let grand_active: i64 = user_counts.iter().map(|(_, _, a)| a).sum();

    Ok(Json(json!({
        "monolith_has_memories_table": true,
        "grand_total": grand_total,
        "grand_active": grand_active,
        "users": users,
    })))
}

const MONOLITH_DRAIN_COLUMNS: &str = "\
    content, category, source, session_id, importance, \
    embedding, embedding_vec_1024, version, is_latest, \
    parent_memory_id, root_memory_id, source_count, is_static, \
    is_forgotten, is_archived, is_fact, is_decomposed, \
    forget_after, forget_reason, model, recall_hits, recall_misses, \
    adaptive_score, pagerank_score, last_accessed_at, access_count, \
    tags, episode_id, decay_score, confidence, sync_id, status, \
    space_id, fsrs_stability, fsrs_difficulty, fsrs_storage_strength, \
    fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, \
    fsrs_last_review_at, is_superseded, is_consolidated, \
    valence, arousal, dominant_emotion, created_at, updated_at";

/// POST /admin/monolith/drain -- migrate per-user rows out of the monolith DB into tenant shards.
#[tracing::instrument(skip_all)]
async fn admin_monolith_drain(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;

    if state.tenant_registry.is_none() {
        return Err(AppError(kleos_lib::EngError::Internal(
            "tenant sharding disabled; drain requires tenant registry".into(),
        )));
    }

    let user_ids: Vec<i64> = state
        .db
        .read(|conn| {
            let mut stmt =
                conn.prepare("SELECT DISTINCT user_id FROM memories WHERE is_forgotten = 0")?;
            let rows: Vec<i64> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await
        .map_err(AppError)?;

    if user_ids.is_empty() {
        return Ok(Json(json!({
            "status": "nothing_to_drain",
            "message": "no active memories in monolith",
        })));
    }

    let col_select = format!(
        "SELECT {} FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
        MONOLITH_DRAIN_COLUMNS
    );
    let col_count = MONOLITH_DRAIN_COLUMNS.split(',').count();
    let placeholders: String = (1..=col_count)
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(", ");
    let col_insert = format!(
        "INSERT OR IGNORE INTO memories ({}) VALUES ({})",
        MONOLITH_DRAIN_COLUMNS, placeholders
    );

    let mut per_user = Vec::new();
    let mut total_drained = 0usize;
    let mut total_skipped = 0usize;
    let mut total_errors = 0usize;

    for uid in &user_ids {
        let uid_val = *uid;

        let tenant_db = match crate::extractors::resolve_db_for_user(&state, uid_val).await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(user_id = uid_val, error = %e, "drain: failed to resolve tenant");
                total_errors += 1;
                per_user.push(json!({
                    "user_id": uid_val,
                    "error": e.to_string(),
                }));
                continue;
            }
        };

        let existing_keys: std::collections::HashSet<(String, String)> = tenant_db
            .read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT content, created_at FROM memories \
                         WHERE is_forgotten = 0 AND is_latest = 1",
                )?;
                let keys: std::collections::HashSet<_> = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                Ok(keys)
            })
            .await
            .unwrap_or_default();

        let col_select_owned = col_select.clone();
        let rows: Vec<Vec<rusqlite::types::Value>> = state
            .db
            .read(move |conn| {
                let mut stmt = conn.prepare(&col_select_owned)?;
                let rows: Vec<Vec<rusqlite::types::Value>> = stmt
                    .query_map(rusqlite::params![uid_val], |row| {
                        let mut vals = Vec::with_capacity(col_count);
                        for i in 0..col_count {
                            vals.push(row.get_ref(i)?.into());
                        }
                        Ok(vals)
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                Ok(rows)
            })
            .await
            .map_err(AppError)?;

        let row_count = rows.len();
        let mut inserted = 0usize;
        let mut skipped = 0usize;

        let col_insert_owned = col_insert.clone();
        let insert_result = tenant_db
            .write(move |conn| {
                let tx = conn.savepoint()?;
                for row_vals in &rows {
                    let content = match &row_vals[0] {
                        rusqlite::types::Value::Text(s) => s.clone(),
                        _ => String::new(),
                    };
                    let created_at = match &row_vals[col_count - 2] {
                        rusqlite::types::Value::Text(s) => s.clone(),
                        _ => String::new(),
                    };

                    if existing_keys.contains(&(content, created_at)) {
                        skipped += 1;
                        continue;
                    }

                    let params: Vec<&dyn rusqlite::types::ToSql> = row_vals
                        .iter()
                        .map(|v| v as &dyn rusqlite::types::ToSql)
                        .collect();
                    match tx.execute(&col_insert_owned, params.as_slice()) {
                        Ok(_) => inserted += 1,
                        Err(e) => {
                            tracing::warn!(error = %e, "drain: insert failed");
                            skipped += 1;
                        }
                    }
                }
                tx.commit()?;
                Ok((inserted, skipped))
            })
            .await;

        match insert_result {
            Ok((ins, skip)) => {
                inserted = ins;
                skipped = skip;
            }
            Err(e) => {
                tracing::error!(user_id = uid_val, error = %e, "drain: batch insert failed");
                total_errors += 1;
                per_user.push(json!({
                    "user_id": uid_val,
                    "error": e.to_string(),
                }));
                continue;
            }
        }

        if inserted > 0 {
            let mark_result = state
                .db
                .write(move |conn| {
                    conn.execute(
                        "UPDATE memories SET is_forgotten = 1, \
                         forget_reason = 'drained to tenant shard' \
                         WHERE user_id = ?1 AND is_forgotten = 0",
                        rusqlite::params![uid_val],
                    )?;
                    Ok(())
                })
                .await;

            if let Err(e) = mark_result {
                tracing::error!(user_id = uid_val, error = %e, "drain: failed to mark monolith rows forgotten");
            }
        }

        per_user.push(json!({
            "user_id": uid_val,
            "monolith_rows": row_count,
            "inserted": inserted,
            "skipped_duplicate": skipped,
        }));

        total_drained += inserted;
        total_skipped += skipped;
    }

    Ok(Json(json!({
        "status": "drained",
        "total_inserted": total_drained,
        "total_skipped_duplicate": total_skipped,
        "total_errors": total_errors,
        "per_user": per_user,
    })))
}

// --- E1 Async Deprovision (cross-store teardown) ---

/// POST /admin/deprovision/{user_id} -- initiate async two-phase teardown.
#[tracing::instrument(skip_all)]
async fn deprovision_tenant_async(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(user_id): Path<i64>,
    Json(body): Json<AsyncDeprovisionBody>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let dep_id = kleos_lib::tenant::teardown::begin_deprovision(
        registry,
        &state.db,
        user_id,
        auth.user_id,
        body.reason,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "deprovision_id": dep_id.as_str(),
            "user_id": user_id,
        })),
    ))
}

/// GET /admin/deprovision/{id}/status -- get deprovision log status.
#[tracing::instrument(skip_all)]
async fn get_deprovision_status(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(dep_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let row = registry
        .registry_db()
        .get_deletion_log(&dep_id)?
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::NotFound(format!(
                "deprovision {dep_id} not found"
            )))
        })?;
    Ok(Json(json!({
        "deprovision_id": row.deprovision_id,
        "target_user_id": row.target_user_id,
        "target_username": row.target_username,
        "deleted_at": row.deleted_at,
        "reason": row.reason,
        "archive_path": row.archive_path,
        "shard_skipped": row.shard_skipped,
    })))
}

/// GET /admin/deprovisions/stuck -- list tenants in Stuck state.
#[tracing::instrument(skip_all)]
async fn list_stuck_deprovisions(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let rows = registry
        .registry_db()
        .list_by_status(kleos_lib::tenant::types::TenantStatus::Stuck)?;
    let items: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "tenant_id": r.tenant_id,
                "user_id": r.user_id,
                "status": r.status.as_str(),
            })
        })
        .collect();
    Ok(Json(json!({ "stuck": items, "count": items.len() })))
}

/// POST /admin/deprovision/{id}/force-retry -- re-enqueue a Stuck teardown.
///
/// Resets the tenant status back to Deleting and enqueues a new teardown job.
#[tracing::instrument(skip_all)]
async fn force_retry_deprovision(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(dep_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let log = registry
        .registry_db()
        .get_deletion_log(&dep_id)?
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::NotFound(format!(
                "deprovision {dep_id} not found"
            )))
        })?;
    let tenant_row = registry
        .registry_db()
        .get_by_user_id(&log.target_user_id.to_string())?;
    // Extract tenant_id before consuming tenant_row in the if-let to avoid borrow-after-move.
    let tenant_id = tenant_row
        .as_ref()
        .map(|r| r.tenant_id.clone())
        .unwrap_or_default();
    if let Some(row) = tenant_row {
        registry.registry_db().update_status(
            &row.tenant_id,
            kleos_lib::tenant::types::TenantStatus::Deleting,
        )?;
    }
    let payload = serde_json::json!({
        "deprovision_id": dep_id,
        "user_id": log.target_user_id,
        "tenant_id": tenant_id,
    })
    .to_string();
    kleos_lib::jobs::enqueue_job(&state.db, "deprovision_teardown", &payload, 5).await?;
    Ok(Json(json!({
        "re_enqueued": true,
        "deprovision_id": dep_id,
    })))
}

/// GET /admin/deprovisions/recent -- list recent deprovision log entries.
#[tracing::instrument(skip_all)]
async fn list_recent_deprovisions(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(q): Query<RecentDeprovisionQuery>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let rows = registry.registry_db().list_deletions_recent(q.limit)?;
    let items: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "deprovision_id": r.deprovision_id,
                "target_user_id": r.target_user_id,
                "target_username": r.target_username,
                "deleted_at": r.deleted_at,
                "reason": r.reason,
                "archive_path": r.archive_path,
                "shard_skipped": r.shard_skipped,
            })
        })
        .collect();
    Ok(Json(json!({ "deprovisions": items, "count": items.len() })))
}

/// POST /admin/deprovision/{id}/skip-shard -- skip shard removal for a Stuck teardown.
///
/// Marks the shard step as skipped in `deletions_log`, then runs the remaining
/// steps (monolith row deletion + tombstone) directly.
#[tracing::instrument(skip_all)]
async fn skip_shard_deprovision(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(dep_id): Path<String>,
    Json(body): Json<SkipShardBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let log = registry
        .registry_db()
        .get_deletion_log(&dep_id)?
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::NotFound(format!(
                "deprovision {dep_id} not found"
            )))
        })?;

    let note = body.note.as_deref().unwrap_or("admin skip-shard");
    registry
        .registry_db()
        .update_deletion_log_shard_skipped(&dep_id, note)?;

    let tenant_row = registry
        .registry_db()
        .get_by_user_id(&log.target_user_id.to_string())?;
    if let Some(row) = &tenant_row {
        kleos_lib::tenant::teardown::delete_monolith_rows(&state.db, log.target_user_id).await?;
        registry.registry_db().mark_tombstone(&row.tenant_id)?;
    }

    Ok(Json(json!({
        "skipped": true,
        "deprovision_id": dep_id,
    })))
}

// --- E2 shard quota management ---

/// GET /admin/quota/{user_id} -- return current quota limits and shadow usage.
async fn get_quota_status(
    State(state): State<AppState>,
    Auth(auth): Auth,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let row = registry.get_quota_row(&user_id)?;
    to_json(row)
}

/// PUT /admin/quota/{user_id} -- set quota limits for a tenant.
///
/// Writes limits to the registry and refreshes the ArcSwap on the loaded
/// handle (if resident) so the next write sees the new limits immediately.
async fn set_quota(
    State(state): State<AppState>,
    Auth(auth): Auth,
    axum::extract::Path(user_id): axum::extract::Path<String>,
    Json(body): Json<SetQuotaBody>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    registry
        .update_quota(
            &user_id,
            body.content_bytes,
            body.memory_count,
            body.disk_bytes,
        )
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// POST /admin/quota/{user_id}/recompute -- re-run the seed query to repair counters.
///
/// Overwrites tenant_state with a fresh scan of the memories table.
async fn recompute_quota(
    State(state): State<AppState>,
    Auth(auth): Auth,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let registry = require_registry(&state)?;
    let (bytes, count) = registry.recompute_usage(&user_id).await?;
    Ok(Json(
        json!({ "ok": true, "content_bytes": bytes, "memory_count": count }),
    ))
}

/// Unit tests for the PITR sandbox-path validator that gates `POST /admin/pitr/prepare`.
#[cfg(test)]
mod pitr_sandbox_tests {
    use super::*;
    use std::fs;

    /// Build a unique scratch directory path inside the temp dir for one PITR sandbox test.
    fn unique_data_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pitr-sandbox-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Sandbox name must be non-empty.
    #[test]
    fn rejects_empty() {
        let dd = unique_data_dir();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    /// Sandbox name must not be an absolute path.
    #[test]
    fn rejects_absolute() {
        let dd = unique_data_dir();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "/etc/passwd").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    /// Sandbox name must not contain `..` traversal segments.
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

    /// Sandbox name must not contain path separator characters.
    #[test]
    fn rejects_separators() {
        let dd = unique_data_dir();
        for bad in ["a/b", "a\\b", "foo/"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    /// Sandbox name must be plain ASCII alphanumerics, dashes, and underscores.
    #[test]
    fn rejects_unicode_and_exotic() {
        let dd = unique_data_dir();
        for bad in ["rm -rf", "a\0b", "name with space", "héllo"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    /// Sandbox name must not start with a dash or dot (avoids CLI ambiguity and hidden directories).
    #[test]
    fn rejects_leading_dash_or_dot() {
        let dd = unique_data_dir();
        for bad in ["-rf", ".env", ".ssh"] {
            let err = sanitize_pitr_dest(dd.to_str().unwrap(), bad).unwrap_err();
            assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
        }
    }

    /// The configured data directory must be absolute.
    #[test]
    fn rejects_non_absolute_data_dir() {
        let err = sanitize_pitr_dest("relative/dir", "restore.db").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::Internal(_)));
    }

    /// A valid name resolves into a fresh jail directory under the data dir.
    #[test]
    fn accepts_plain_filename_and_creates_jail() {
        let dd = unique_data_dir();
        let out = sanitize_pitr_dest(dd.to_str().unwrap(), "restore.db").unwrap();
        let jail = dd.join(PITR_RESTORE_SUBDIR);
        assert!(jail.is_dir(), "jail dir must be created");
        assert!(out.starts_with(std::fs::canonicalize(&jail).unwrap()));
        assert_eq!(out.file_name().unwrap(), "restore.db");
    }

    /// Refuse to prepare a sandbox whose target directory already exists.
    #[test]
    fn rejects_overwrite_existing() {
        let dd = unique_data_dir();
        let jail = dd.join(PITR_RESTORE_SUBDIR);
        fs::create_dir_all(&jail).unwrap();
        fs::write(jail.join("already.db"), b"x").unwrap();
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), "already.db").unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }

    /// Sandbox name must respect the configured length limit.
    #[test]
    fn rejects_too_long_name() {
        let dd = unique_data_dir();
        let long_name = "a".repeat(PITR_DEST_MAX_LEN + 1);
        let err = sanitize_pitr_dest(dd.to_str().unwrap(), &long_name).unwrap_err();
        assert!(matches!(err.0, kleos_lib::EngError::InvalidInput(_)));
    }
}
