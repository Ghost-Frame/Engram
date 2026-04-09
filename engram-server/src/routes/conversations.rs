use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::conversations::{
    self, CreateConversationRequest, UpdateConversationRequest, AddMessageRequest,
    BulkInsertRequest, UpsertConversationRequest, SearchMessagesRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", post(create).get(list))
        .route("/conversations/{id}", get(get_one).patch(update).delete(remove))
        .route("/conversations/{id}/messages", post(add_msg).get(list_msgs))
        .route("/conversations/bulk", post(bulk_insert))
        .route("/conversations/upsert", post(upsert))
        .route("/messages/search", post(search_msgs))
}

// POST /conversations
async fn create(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let conv = conversations::create_conversation(&state.db, body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(conv))))
}

// GET /conversations
#[derive(Debug, Deserialize)]
struct ListConversationsParams {
    limit: Option<usize>,
    agent: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListConversationsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).min(100);
    let convs = if let Some(ref agent) = params.agent {
        conversations::list_conversations_by_agent(&state.db, auth.user_id, agent, limit).await?
    } else {
        conversations::list_conversations(&state.db, auth.user_id, limit).await?
    };
    Ok(Json(json!({ "conversations": convs })))
}

// GET /conversations/{id}
#[derive(Debug, Deserialize)]
struct GetConversationParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn get_one(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<GetConversationParams>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::get_conversation_for_user(&state.db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let messages = conversations::list_messages(&state.db, id, limit, offset).await?;
    Ok(Json(json!({
        "id": conv.id, "agent": conv.agent, "session_id": conv.session_id,
        "title": conv.title, "metadata": conv.metadata,
        "started_at": conv.started_at, "updated_at": conv.updated_at,
        "messages": messages,
    })))
}

// PATCH /conversations/{id}
async fn update(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateConversationRequest>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::update_conversation(&state.db, id, auth.user_id, body).await?;
    Ok(Json(json!(conv)))
}

// DELETE /conversations/{id}
async fn remove(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    conversations::delete_conversation(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

// POST /conversations/{id}/messages
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageBody {
    Single(AddMessageRequest),
    Batch(Vec<AddMessageRequest>),
}

async fn add_msg(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<MessageBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Verify conversation belongs to user
    conversations::get_conversation_for_user(&state.db, id, auth.user_id).await?;
    match body {
        MessageBody::Single(req) => {
            let msg = conversations::add_message(&state.db, id, req).await?;
            Ok((StatusCode::CREATED, Json(json!(msg))))
        }
        MessageBody::Batch(reqs) => {
            let mut msgs = Vec::new();
            for req in reqs {
                msgs.push(conversations::add_message(&state.db, id, req).await?);
            }
            Ok((StatusCode::CREATED, Json(json!({ "messages": msgs }))))
        }
    }
}

// GET /conversations/{id}/messages
#[derive(Debug, Deserialize)]
struct ListMessagesParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn list_msgs(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<ListMessagesParams>,
) -> Result<Json<Value>, AppError> {
    // Verify conversation ownership before accessing messages
    conversations::get_conversation_for_user(&state.db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let messages = conversations::list_messages(&state.db, id, limit, offset).await?;
    Ok(Json(json!({ "messages": messages })))
}

// POST /conversations/bulk
async fn bulk_insert(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<BulkInsertRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let conv = conversations::bulk_insert_conversation(&state.db, body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(conv))))
}

// POST /conversations/upsert
async fn upsert(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UpsertConversationRequest>,
) -> Result<Json<Value>, AppError> {
    let conv = conversations::upsert_conversation(&state.db, body, auth.user_id).await?;
    Ok(Json(json!(conv)))
}

// POST /messages/search
async fn search_msgs(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchMessagesRequest>,
) -> Result<Json<Value>, AppError> {
    let results = conversations::search_messages(&state.db, body, auth.user_id).await?;
    Ok(Json(json!({ "messages": results })))
}
