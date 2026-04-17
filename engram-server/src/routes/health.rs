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
use engram_lib::auth::Scope;
use engram_lib::jobs;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(get_health))
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
    // Single query to get all dashboard counts. Avoids 6 serial DB round-trips.
    let counts = state
        .db
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
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap_or((0, 0, 0, 0, 0, 0));

    let (memories, entities, episodes, pending, static_count, versioned) = counts;

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
        "service": "engram",
        "version": "0.1.0",
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
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let mut checks: Vec<(&str, bool)> = Vec::new();

    // DB ping: run a trivial query to verify the connection pool is alive.
    let db_ok = state
        .db
        .read(|conn| {
            conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .is_ok();
    checks.push(("database", db_ok));

    // Embedder loaded
    let embedder_ok = state.embedder.read().await.is_some();
    checks.push(("embedder", embedder_ok));

    // Reranker loaded
    let reranker_ok = state.reranker.read().await.is_some();
    checks.push(("reranker", reranker_ok));

    let all_ok = checks.iter().all(|(_, ok)| *ok);
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let details: serde_json::Map<String, Value> = checks
        .iter()
        .map(|(name, ok)| {
            (
                name.to_string(),
                json!(if *ok { "ok" } else { "unavailable" }),
            )
        })
        .collect();

    (
        status,
        Json(json!({
            "status": if all_ok { "ready" } else { "not_ready" },
            "service": "engram",
            "version": "0.1.0",
            "checks": details,
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
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
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
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap_or(0);

    lines.push("# HELP engram_memories_total Total non-forgotten memories".to_string());
    lines.push("# TYPE engram_memories_total gauge".to_string());
    lines.push(format!("engram_memories_total {}", mem_count));
    lines.push(String::new());

    lines.push("# HELP engram_embedded_total Memories with embeddings".to_string());
    lines.push("# TYPE engram_embedded_total gauge".to_string());
    lines.push(format!("engram_embedded_total {}", emb_count));
    lines.push(String::new());

    // Job stats
    if let Ok(stats) = jobs::get_job_stats(&state.db).await {
        lines.push("# HELP engram_jobs_total Jobs by status".to_string());
        lines.push("# TYPE engram_jobs_total gauge".to_string());
        lines.push(format!(
            "engram_jobs_total{{status=\"pending\"}} {}",
            stats.pending
        ));
        lines.push(format!(
            "engram_jobs_total{{status=\"running\"}} {}",
            stats.running
        ));
        lines.push(format!(
            "engram_jobs_total{{status=\"completed\"}} {}",
            stats.completed
        ));
        lines.push(format!(
            "engram_jobs_total{{status=\"failed\"}} {}",
            stats.failed
        ));
        lines.push(String::new());
    }

    let body = lines.join("\n");
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(Body::from(body))
        .unwrap()
}
