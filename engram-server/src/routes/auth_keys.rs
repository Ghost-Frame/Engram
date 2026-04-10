use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
    Json, Router,
};
use engram_lib::auth;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/keys", post(create_key).get(list_keys))
        .route("/keys/{id}", delete(revoke_key))
        .route("/keys/rotate", post(rotate_key))
        .route("/users", post(create_user).get(list_users))
        .route("/spaces", post(create_space).get(list_spaces))
        .route("/spaces/{id}", delete(delete_space))
}

// ---- Key Management ----

#[derive(Debug, Deserialize)]
struct CreateKeyBody {
    pub name: Option<String>,
    pub scopes: Option<String>,
    pub user_id: Option<i64>,
    #[allow(dead_code)]
    pub rate_limit: Option<i32>,
    #[allow(dead_code)]
    pub expires_at: Option<String>,
}

async fn create_key(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Only admin can create keys
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "Admin scope required".into(),
        )));
    }

    let target_user_id = body.user_id.unwrap_or(auth_ctx.user_id);
    let name = body.name.as_deref().unwrap_or("default");
    let scopes_str = body.scopes.as_deref().unwrap_or("read,write");
    let scopes: Vec<auth::Scope> = scopes_str
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let (api_key, raw_key) = auth::create_key(&state.db, target_user_id, name, scopes).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": raw_key,
            "id": api_key.id,
            "name": api_key.name,
            "scopes": scopes_str,
            "rate_limit": api_key.rate_limit,
            "user_id": target_user_id,
            "expires_at": body.expires_at,
            "message": "Save this key -- it cannot be retrieved again.",
        })),
    ))
}

async fn list_keys(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
    Ok(Json(json!({ "keys": keys })))
}

async fn revoke_key(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // Verify ownership: list user's keys and check if this id is among them
    let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
    let is_admin = auth_ctx.has_scope(&auth::Scope::Admin);
    let owns_key = keys.iter().any(|k| k.id == id);

    if !owns_key && !is_admin {
        return Err(AppError(engram_lib::EngError::Auth("Forbidden".into())));
    }

    auth::revoke_key(&state.db, id).await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct RotateKeyBody {
    pub key_id: i64,
    #[allow(dead_code)]
    pub expires_at: Option<String>,
}

async fn rotate_key(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<RotateKeyBody>,
) -> Result<Json<Value>, AppError> {
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    // Verify old key exists and belongs to user
    let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
    let old_key = keys
        .iter()
        .find(|k| k.id == body.key_id)
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Key not found".into())))?;

    // Create new key with same properties
    let new_name = format!("{} (rotated)", old_key.name);
    let (new_key, raw_key) = auth::create_key(
        &state.db,
        auth_ctx.user_id,
        &new_name,
        old_key.scopes.clone(),
    )
    .await?;

    // Set 24-hour grace period on old key
    let grace_expiry = (chrono::Utc::now() + chrono::Duration::hours(24))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    state
        .db
        .conn
        .execute(
            "UPDATE api_keys SET expires_at = ?1 WHERE id = ?2",
            libsql::params![grace_expiry.clone(), body.key_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    Ok(Json(json!({
        "new_key": raw_key,
        "new_key_id": new_key.id,
        "old_key_id": body.key_id,
        "old_key_expires": grace_expiry,
        "message": "Old key will expire in 24 hours. Update your clients to use the new key.",
    })))
}

// ---- User Management ----

#[derive(Debug, Deserialize)]
struct CreateUserBody {
    pub username: String,
    pub email: Option<String>,
    pub role: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateUserBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let valid_roles = ["admin", "writer", "reader"];
    let role = body
        .role
        .as_deref()
        .filter(|r| valid_roles.contains(r))
        .unwrap_or("writer");
    let is_admin = if role == "admin" { 1i64 } else { 0i64 };
    let username = body.username.trim();

    let mut rows = state
        .db
        .conn
        .query(
            "INSERT INTO users (username, email, role, is_admin) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
            libsql::params![
                username.to_string(),
                body.email.clone(),
                role.to_string(),
                is_admin
            ],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| {
            AppError(engram_lib::EngError::Internal(
                "user insert returned no row".into(),
            ))
        })?;

    let id: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    let created_at: String = row
        .get(1)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;

    // Create default space
    let _ = state
        .db
        .conn
        .execute(
            "INSERT INTO spaces (user_id, name) VALUES (?1, 'default')",
            libsql::params![id],
        )
        .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "username": username,
            "created_at": created_at,
        })),
    ))
}

async fn list_users(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let mut rows = state
        .db
        .conn
        .query(
            "SELECT id, username, email, role, is_admin, created_at FROM users ORDER BY id",
            libsql::params![],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let mut users = Vec::new();
    while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
        users.push(json!({
            "id": r.get::<i64>(0).unwrap_or(0),
            "username": r.get::<String>(1).unwrap_or_default(),
            "email": r.get::<Option<String>>(2).unwrap_or(None),
            "role": r.get::<String>(3).unwrap_or_default(),
            "is_admin": r.get::<i64>(4).unwrap_or(0) != 0,
            "created_at": r.get::<String>(5).unwrap_or_default(),
        }));
    }

    Ok(Json(json!({ "users": users })))
}

// ---- Space Management ----

#[derive(Debug, Deserialize)]
struct CreateSpaceBody {
    pub name: String,
    pub description: Option<String>,
}

async fn create_space(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateSpaceBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "name is required".into(),
        )));
    }

    let mut rows = state
        .db
        .conn
        .query(
            "INSERT INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3) RETURNING id, created_at",
            libsql::params![
                auth_ctx.user_id,
                name.to_string(),
                body.description.clone()
            ],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| {
            AppError(engram_lib::EngError::Internal(
                "space insert returned no row".into(),
            ))
        })?;

    let id: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    let created_at: String = row
        .get(1)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": name,
            "created_at": created_at,
        })),
    ))
}

async fn list_spaces(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT id, name, description, created_at FROM spaces WHERE user_id = ?1 ORDER BY id",
            libsql::params![auth_ctx.user_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let mut spaces = Vec::new();
    while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
        spaces.push(json!({
            "id": r.get::<i64>(0).unwrap_or(0),
            "name": r.get::<String>(1).unwrap_or_default(),
            "description": r.get::<Option<String>>(2).unwrap_or(None),
            "created_at": r.get::<String>(3).unwrap_or_default(),
        }));
    }

    Ok(Json(json!({ "spaces": spaces })))
}

async fn delete_space(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // Verify ownership
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT user_id, name FROM spaces WHERE id = ?1",
            libsql::params![id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Not found".into())))?;

    let owner: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    let name: String = row
        .get(1)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;

    if owner != auth_ctx.user_id && !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth("Forbidden".into())));
    }

    if name == "default" {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Cannot delete default space".into(),
        )));
    }

    state
        .db
        .conn
        .execute("DELETE FROM spaces WHERE id = ?1", libsql::params![id])
        .await
        .map_err(engram_lib::EngError::Database)?;

    Ok(Json(json!({ "deleted": true, "id": id })))
}
