use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::{extractors::Auth, state::AppState};
use kleos_lib::auth::Scope;
use kleos_lib::jobs;

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(get_health))
        .route("/health/live", get(get_live))
        .route("/health/ready", get(get_ready))
        .route("/live", get(get_live))
        .route("/ready", get(get_ready))
        .route("/metrics", get(get_metrics))
        // S7-26: health probes must respond within 1s or they are useless.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(1),
        ))
        // S7-27: health endpoints carry no body; 1 KB is generous.
        .layer(DefaultBodyLimit::max(1024))
}

async fn get_health(State(state): State<AppState>) -> Json<Value> {
    let (mut memories, mut entities, mut episodes, mut pending, mut static_count, mut versioned) =
        (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);

    if let Some(ref registry) = state.tenant_registry {
        if let Ok(tenants) = registry.list() {
            for row in &tenants {
                if row.status != kleos_lib::tenant::TenantStatus::Active {
                    continue;
                }
                let handle = match registry.get(&row.user_id).await {
                    Ok(Some(h)) => h,
                    _ => continue,
                };
                let db = handle.database();
                let counts = db
                    .read(|conn| {
                        conn.query_row(
                            "SELECT
                                SUM(CASE WHEN is_forgotten = 0 AND is_archived = 0 THEN 1 ELSE 0 END),
                                (SELECT COUNT(*) FROM entities),
                                (SELECT COUNT(*) FROM episodes),
                                SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
                                SUM(CASE WHEN is_static = 1 AND is_forgotten = 0 THEN 1 ELSE 0 END),
                                SUM(CASE WHEN version > 1 AND is_forgotten = 0 THEN 1 ELSE 0 END)
                             FROM memories",
                            [],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, i64>(2)?,
                                    row.get::<_, i64>(3)?,
                                    row.get::<_, i64>(4)?,
                                    row.get::<_, i64>(5)?,
                                ))
                            },
                        )
                        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
                    })
                    .await
                    .unwrap_or((0, 0, 0, 0, 0, 0));

                memories += counts.0;
                entities += counts.1;
                episodes += counts.2;
                pending += counts.3;
                static_count += counts.4;
                versioned += counts.5;
            }
        }
    }

    let llm_configured = state.brain.is_some();
    let embedding_model = state
        .config
        .embedding_model_dir
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");

    Json(json!({
        "status": "ok",
        "service": "kleos",
        "version": env!("CARGO_PKG_VERSION"),
        "memories": memories,
        "entities": entities,
        "episodes": episodes,
        "pending": pending,
        "static": static_count,
        "versioned": versioned,
        "llm_configured": llm_configured,
        "embedding_model": embedding_model,
    }))
}

async fn get_live() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "kleos",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn get_ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    // Required checks must all pass for 200. Optional components (embedder,
    // reranker, LLM) report their state but do not block readiness, because
    // a server deployment may legitimately run without them.
    //
    // Returns 503 when any required check fails, with a `failing` array so
    // operators can see at a glance what is wrong.

    // DB ping: required. Run a trivial query to verify the connection pool.
    let db_ok = state
        .db
        .read(|conn| {
            conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .is_ok();

    // Optional components: surface state but do not fail readiness.
    let embedder_loaded = state.embedder.read().await.is_some();
    let reranker_loaded = state.reranker.read().await.is_some();
    let llm_configured = state.brain.is_some();

    let mut failing: Vec<&'static str> = Vec::new();
    if !db_ok {
        failing.push("database");
    }

    let all_ok = failing.is_empty();
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let checks = json!({
        "database": if db_ok { "ok" } else { "unavailable" },
        "embedder": if embedder_loaded { "loaded" } else { "disabled" },
        "reranker": if reranker_loaded { "loaded" } else { "disabled" },
        "llm": if llm_configured { "configured" } else { "disabled" },
    });

    (
        status,
        Json(json!({
            "status": if all_ok { "ready" } else { "not_ready" },
            "service": "kleos",
            "version": env!("CARGO_PKG_VERSION"),
            "checks": checks,
            "failing": failing,
        })),
    )
}

async fn get_metrics(State(state): State<AppState>, Auth(auth): Auth) -> Response<Body> {
    // SECURITY: /metrics exposes global counts. Restrict to admin-scoped callers so
    // a leaked read/write key can neither enumerate tenant sizes nor observe fleet
    // activity.
    if !auth.has_scope(&Scope::Admin) {
        return (StatusCode::FORBIDDEN, "admin scope required for metrics\n").into_response();
    }
    let mut lines = Vec::new();

    // Memory counts
    let mem_count: i64 = state
        .db
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE is_forgotten = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap_or(0);

    let emb_count: i64 = state
        .db
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL AND is_forgotten = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap_or(0);

    lines.push("# HELP kleos_memories_total Total non-forgotten memories".to_string());
    lines.push("# TYPE kleos_memories_total gauge".to_string());
    lines.push(format!("kleos_memories_total {}", mem_count));
    lines.push(String::new());

    lines.push("# HELP kleos_embedded_total Memories with embeddings".to_string());
    lines.push("# TYPE kleos_embedded_total gauge".to_string());
    lines.push(format!("kleos_embedded_total {}", emb_count));
    lines.push(String::new());

    // Job stats
    if let Ok(stats) = jobs::get_job_stats(&state.db).await {
        lines.push("# HELP kleos_jobs_total Jobs by status".to_string());
        lines.push("# TYPE kleos_jobs_total gauge".to_string());
        lines.push(format!(
            "kleos_jobs_total{{status=\"pending\"}} {}",
            stats.pending
        ));
        lines.push(format!(
            "kleos_jobs_total{{status=\"running\"}} {}",
            stats.running
        ));
        lines.push(format!(
            "kleos_jobs_total{{status=\"completed\"}} {}",
            stats.completed
        ));
        lines.push(format!(
            "kleos_jobs_total{{status=\"failed\"}} {}",
            stats.failed
        ));
        lines.push(String::new());
    }

    let body = lines.join("\n");
    // R8-R-001: response builder uses static header bytes so .unwrap() is
    // infallible today, but fall back to a minimal 200 response rather
    // than panicking if that ever changes.
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
