use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::brain_absorber::absorb_activity_to_brain;
use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::conversations::{
    self, BulkInsertRequest, CreateConversationRequest, SearchMessagesRequest,
    UpdateConversationRequest, UpsertConversationRequest,
};
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;

mod types;
use types::{GetConversationParams, ListConversationsParams, ListMessagesParams, MessageBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", post(create).get(list))
        .route(
            "/conversations/{id}",
            get(get_one).patch(update).delete(remove),
        )
        .route("/conversations/{id}/messages", post(add_msg).get(list_msgs))
        .route("/conversations/{id}/memorize", post(memorize))
        .route("/conversations/bulk", post(bulk_insert))
        .route("/conversations/upsert", post(upsert))
        .route("/messages/search", post(search_msgs))
}

// POST /conversations
async fn create(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let conv = conversations::create_conversation(&db, body, auth.effective_user_id()).await?;
    Ok((StatusCode::CREATED, Json(json!(conv))))
}

// GET /conversations
async fn list(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListConversationsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).min(100);
    let convs = if let Some(ref agent) = params.agent {
        conversations::list_conversations_by_agent(&db, auth.effective_user_id(), agent, limit)
            .await?
    } else {
        conversations::list_conversations(&db, auth.effective_user_id(), limit).await?
    };
    Ok(Json(json!({ "conversations": convs })))
}

// GET /conversations/{id}
async fn get_one(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Query(params): Query<GetConversationParams>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::get_conversation_for_user(&db, id, auth.effective_user_id()).await?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let messages =
        conversations::list_messages(&db, id, auth.effective_user_id(), limit, offset).await?;
    Ok(Json(json!({
        "id": conv.id, "agent": conv.agent, "session_id": conv.session_id,
        "title": conv.title, "metadata": conv.metadata,
        "started_at": conv.started_at, "updated_at": conv.updated_at,
        "messages": messages,
    })))
}

// PATCH /conversations/{id}
async fn update(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<UpdateConversationRequest>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::update_conversation(&db, id, auth.effective_user_id(), body).await?;
    Ok(Json(json!(conv)))
}

// DELETE /conversations/{id}
async fn remove(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    conversations::delete_conversation(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

// POST /conversations/{id}/messages
// Hybrid: add_message needs state.credd to resolve secret references.
async fn add_msg(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<MessageBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Verify conversation belongs to user
    conversations::get_conversation_for_user(&db, id, auth.effective_user_id()).await?;
    match body {
        MessageBody::Single(req) => {
            let msg =
                conversations::add_message(&db, &state.credd, id, auth.effective_user_id(), req)
                    .await?;
            Ok((StatusCode::CREATED, Json(json!(msg))))
        }
        MessageBody::Batch(reqs) => {
            let mut msgs = Vec::new();
            for req in reqs {
                msgs.push(
                    conversations::add_message(
                        &db,
                        &state.credd,
                        id,
                        auth.effective_user_id(),
                        req,
                    )
                    .await?,
                );
            }
            Ok((StatusCode::CREATED, Json(json!({ "messages": msgs }))))
        }
    }
}

// GET /conversations/{id}/messages
async fn list_msgs(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Query(params): Query<ListMessagesParams>,
) -> Result<Json<Value>, AppError> {
    // Verify conversation ownership before accessing messages
    conversations::get_conversation_for_user(&db, id, auth.effective_user_id()).await?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let messages =
        conversations::list_messages(&db, id, auth.effective_user_id(), limit, offset).await?;
    Ok(Json(json!({ "messages": messages })))
}

// POST /conversations/bulk
// Hybrid: needs state.credd for credd.resolve_text on message bodies.
async fn bulk_insert(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<BulkInsertRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let conv =
        conversations::bulk_insert_conversation(&db, &state.credd, body, auth.effective_user_id())
            .await?;
    Ok((StatusCode::CREATED, Json(json!(conv))))
}

// POST /conversations/upsert
// Hybrid: needs state.credd for credd.resolve_text on message bodies.
async fn upsert(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<UpsertConversationRequest>,
) -> Result<Json<Value>, AppError> {
    let conv =
        conversations::upsert_conversation(&db, &state.credd, body, auth.effective_user_id())
            .await?;
    Ok(Json(json!(conv)))
}

// POST /messages/search
async fn search_msgs(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchMessagesRequest>,
) -> Result<Json<Value>, AppError> {
    let results = conversations::search_messages(&db, body, auth.effective_user_id()).await?;
    Ok(Json(json!({ "messages": results })))
}

// POST /conversations/{id}/memorize
// Stores the conversation transcript as a searchable memory and absorbs it
// into the Hopfield brain. Idempotent: calling it multiple times creates
// multiple memory versions (handled by the dedup/boost logic in store).
async fn memorize(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::get_conversation_for_user(&db, id, auth.user_id).await?;
    let messages = conversations::list_messages(&db, id, auth.user_id, 1000, 0).await?;

    if messages.is_empty() {
        return Ok(Json(json!({ "memorized": false, "reason": "no messages" })));
    }

    // Format as a readable transcript.
    let header = match &conv.title {
        Some(t) => format!("[Conversation with {}: {}]\n", conv.agent, t),
        None => format!("[Conversation with {}]\n", conv.agent),
    };
    let body: String = messages
        .iter()
        .map(|m| format!("[{}]: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let transcript = format!("{}{}", header, body);

    let embedder = state.current_embedder().await;
    let req = StoreRequest {
        content: transcript.clone(),
        category: "conversation".to_string(),
        source: conv.agent.clone(),
        importance: 5,
        user_id: Some(auth.user_id),
        ..StoreRequest::default()
    };

    let result = if let Some(ref e) = embedder {
        memory::store_with_chunks(&db, e.as_ref(), req).await?
    } else {
        memory::store(&db, req, None, false).await?
    };

    // Brain absorb: fire-and-forget.
    if let Some(brain) = state.brain.clone() {
        let embedder = state.embedder.clone();
        let memory_id = result.id;
        let user_id = auth.user_id;
        let agent = conv.agent.clone();
        if let Ok(permit) = state.brain_absorb_sem.clone().acquire_owned().await {
            let shutdown = state.shutdown_token.clone();
            let mut bg = state.background_tasks.lock().await;
            bg.spawn(async move {
                let _permit = permit;
                tokio::select! {
                    _ = shutdown.cancelled() => {}
                    _ = absorb_activity_to_brain(
                        brain, embedder, user_id, memory_id, transcript,
                        "conversation".to_string(), 5.0, agent,
                    ) => {}
                }
            });
        }
    }

    Ok(Json(json!({
        "memorized": true,
        "memory_id": result.id,
        "message_count": messages.len(),
        "duplicate": result.duplicate_of.is_some(),
    })))
}
