use axum::{
    body::Body, extract::State, http::header, response::Response, routing::get, Json, Router,
};
use serde_json::{json, Value};

use crate::state::AppState;
use engram_lib::jobs;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(get_health))
        .route("/live", get(get_live))
        .route("/ready", get(get_ready))
        .route("/metrics", get(get_metrics))
}

async fn get_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_live() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_ready() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_metrics(State(state): State<AppState>) -> Response<Body> {
    let mut lines = Vec::new();

    // Memory counts
    let mem_count: i64 = async {
        let mut rows = state
            .db
            .conn
            .query("SELECT COUNT(*) FROM memories WHERE is_forgotten = 0", ())
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        row.get::<i64>(0).ok()
    }
    .await
    .unwrap_or(0);

    let emb_count: i64 = async {
        let mut rows = state
            .db
            .conn
            .query(
                "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL AND is_forgotten = 0",
                (),
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        row.get::<i64>(0).ok()
    }
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
    if let Ok(stats) = jobs::get_job_stats(&state.db.conn).await {
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
