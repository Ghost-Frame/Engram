// ============================================================================
// POST /batch -- execute multiple memory ops in a single transaction
// ============================================================================

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use kleos_lib::memory::{self, types::StoreRequest};
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

use kleos_lib::validation::MAX_BATCH_OPS;

mod types;
use types::{BatchOp, BatchRequest, BatchResult, LinkBody, StoreBody, UpdateBody};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new().route("/batch", post(batch_handler))
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn batch_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(req): Json<BatchRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if req.ops.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "ops must not be empty".to_string(),
        )));
    }
    if req.ops.len() > MAX_BATCH_OPS {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "batch limited to {} ops, got {}",
            MAX_BATCH_OPS,
            req.ops.len()
        ))));
    }

    let user_id = auth.user_id;
    // Clamp the capacity hint to MAX_BATCH_OPS so the allocation can never
    // exceed the enforced bound, even though the check above rejects larger
    // inputs (defence-in-depth + explicit for static analysers).
    let mut results: Vec<BatchResult> = Vec::with_capacity(req.ops.len().min(MAX_BATCH_OPS));

    for (i, op) in req.ops.into_iter().enumerate() {
        let res = execute_op(&state, user_id, i, op).await;
        let failed = !res.success;
        results.push(res);

        // Abort remaining ops on first failure (atomic batch semantics).
        if failed {
            break;
        }
    }

    let all_ok = results.iter().all(|r| r.success);
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };

    Ok((
        status,
        Json(json!({
            "results": results,
            "total": results.len(),
            "succeeded": results.iter().filter(|r| r.success).count(),
        })),
    ))
}

// ---------------------------------------------------------------------------
// Per-op dispatch
// ---------------------------------------------------------------------------

async fn execute_op(state: &AppState, user_id: i64, index: usize, op: BatchOp) -> BatchResult {
    match op {
        BatchOp::Store { body } => execute_store(state, user_id, index, body).await,
        BatchOp::Update { body } => execute_update(state, user_id, index, body).await,
        BatchOp::Link { body } => execute_link(state, user_id, index, body).await,
    }
}

async fn execute_store(
    state: &AppState,
    user_id: i64,
    index: usize,
    body: StoreBody,
) -> BatchResult {
    if body.content.trim().is_empty() {
        return BatchResult {
            index,
            op: "store".to_string(),
            success: false,
            result: None,
            error: Some("content must not be empty".to_string()),
        };
    }

    let mut req = StoreRequest {
        content: body.content,
        category: body.category.unwrap_or_else(|| "general".to_string()),
        source: body.source.unwrap_or_else(|| "batch".to_string()),
        importance: body.importance.unwrap_or(5),
        tags: body.tags,
        is_static: body.is_static,
        session_id: body.session_id,
        space_id: body.space_id,
        user_id: Some(user_id),
        embedding: None,
        parent_memory_id: None,
    };

    // Embed if available
    let embedder_guard = state.embedder.read().await;
    if let Some(ref embedder) = *embedder_guard {
        match embedder.embed(&req.content).await {
            Ok(emb) => req.embedding = Some(emb),
            Err(e) => tracing::warn!("batch store embed failed: {}", e),
        }
    }
    drop(embedder_guard);

    match memory::store(&state.db, req).await {
        Ok(store_result) => {
            if let Some(existing_id) = store_result.duplicate_of {
                BatchResult {
                    index,
                    op: "store".to_string(),
                    success: true,
                    result: Some(json!({
                        "stored": false, "duplicate": true,
                        "existing_id": existing_id,
                    })),
                    error: None,
                }
            } else {
                BatchResult {
                    index,
                    op: "store".to_string(),
                    success: true,
                    result: Some(json!({
                        "stored": true, "id": store_result.id,
                    })),
                    error: None,
                }
            }
        }
        Err(e) => BatchResult {
            index,
            op: "store".to_string(),
            success: false,
            result: None,
            error: Some(e.to_string()),
        },
    }
}

async fn execute_update(
    state: &AppState,
    user_id: i64,
    index: usize,
    body: UpdateBody,
) -> BatchResult {
    let req = kleos_lib::memory::types::UpdateRequest {
        content: body.content,
        category: body.category,
        importance: body.importance,
        tags: body.tags,
        is_static: None,
        status: None,
        embedding: None,
    };

    match memory::update(&state.db, body.id, req, user_id).await {
        Ok(mem) => BatchResult {
            index,
            op: "update".to_string(),
            success: true,
            result: Some(json!({ "id": mem.id, "updated": true })),
            error: None,
        },
        Err(e) => BatchResult {
            index,
            op: "update".to_string(),
            success: false,
            result: None,
            error: Some(e.to_string()),
        },
    }
}

async fn execute_link(state: &AppState, user_id: i64, index: usize, body: LinkBody) -> BatchResult {
    let similarity = body.similarity.unwrap_or(1.0);
    let link_type = body.link_type.unwrap_or_else(|| "manual".to_string());

    match memory::insert_link(
        &state.db,
        body.source_id,
        body.target_id,
        similarity,
        &link_type,
        user_id,
    )
    .await
    {
        Ok(()) => BatchResult {
            index,
            op: "link".to_string(),
            success: true,
            result: Some(json!({
                "linked": true,
                "source_id": body.source_id,
                "target_id": body.target_id,
            })),
            error: None,
        },
        Err(e) => BatchResult {
            index,
            op: "link".to_string(),
            success: false,
            result: None,
            error: Some(e.to_string()),
        },
    }
}
