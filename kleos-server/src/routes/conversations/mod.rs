use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::conversations::{
    self, BulkInsertRequest, CreateConversationRequest, SearchMessagesRequest,
    UpdateConversationRequest, UpsertConversationRequest,
};

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
    let conv = conversations::create_conversation(&db, body, auth.user_id).await?;
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
        conversations::list_conversations_by_agent(&db, auth.user_id, agent, limit).await?
    } else {
        conversations::list_conversations(&db, auth.user_id, limit).await?
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
    let conv = conversations::get_conversation_for_user(&db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let messages = conversations::list_messages(&db, id, auth.user_id, limit, offset).await?;
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
    let conv = conversations::update_conversation(&db, id, auth.user_id, body).await?;
    Ok(Json(json!(conv)))
}

// DELETE /conversations/{id}
async fn remove(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    conversations::delete_conversation(&db, id, auth.user_id).await?;
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
    conversations::get_conversation_for_user(&db, id, auth.user_id).await?;
    match body {
        MessageBody::Single(req) => {
            let msg =
                conversations::add_message(&db, &state.credd, id, auth.user_id, req).await?;
            Ok((StatusCode::CREATED, Json(json!(msg))))
        }
        MessageBody::Batch(reqs) => {
            let mut msgs = Vec::new();
            for req in reqs {
                msgs.push(
                    conversations::add_message(&db, &state.credd, id, auth.user_id, req)
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
    conversations::get_conversation_for_user(&db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let messages = conversations::list_messages(&db, id, auth.user_id, limit, offset).await?;
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
    let conv = conversations::bulk_insert_conversation(&db, &state.credd, body, auth.user_id)
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
        conversations::upsert_conversation(&db, &state.credd, body, auth.user_id).await?;
    Ok(Json(json!(conv)))
}

// POST /messages/search
async fn search_msgs(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchMessagesRequest>,
) -> Result<Json<Value>, AppError> {
    let results = conversations::search_messages(&db, body, auth.user_id).await?;
    Ok(Json(json!({ "messages": results })))
}
