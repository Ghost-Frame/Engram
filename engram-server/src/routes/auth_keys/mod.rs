use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
    Json, Router,
};
use engram_lib::auth;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{CreateKeyBody, CreateSpaceBody, CreateUserBody, RotateKeyBody};

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

    // SECURITY (SEC-MED-3): verify target user_id exists before minting a key.
    if target_user_id != auth_ctx.user_id {
        let uid = target_user_id;
        let exists: bool = state
            .db
            .read(move |conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
                    params![uid],
                    |row| row.get(0),
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
            })
            .await?;
        if !exists {
            return Err(AppError(engram_lib::EngError::NotFound(format!(
                "user {} not found",
                target_user_id
            ))));
        }
    }

    let name = body.name.as_deref().unwrap_or("default");
    let scopes_str = body.scopes.as_deref().unwrap_or("read,write");
    let scopes: Vec<auth::Scope> = scopes_str
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    // SECURITY (SEC-MED-4): scope escalation cap -- caller cannot grant
    // scopes they do not themselves possess.
    for scope in &scopes {
        if !auth_ctx.has_scope(scope) {
            return Err(AppError(engram_lib::EngError::Auth(format!(
                "cannot grant scope '{}' that caller does not hold",
                scope
            ))));
        }
    }

    // Absolute expires_at takes precedence; otherwise derive from ttl_secs.
    let final_expires_at = body.expires_at.clone().or_else(|| {
        body.ttl_secs.filter(|s| *s > 0).map(|s| {
            (chrono::Utc::now() + chrono::Duration::seconds(s))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
    });

    let (api_key, raw_key) = auth::create_key_with_expiry(
        &state.db,
        target_user_id,
        name,
        scopes,
        body.rate_limit,
        final_expires_at.clone(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": raw_key,
            "id": api_key.id,
            "name": api_key.name,
            "scopes": scopes_str,
            "rate_limit": api_key.rate_limit,
            "user_id": target_user_id,
            "expires_at": final_expires_at,
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
    // SECURITY (SEC-HIGH-5): resolve the key owner before revoking so an
    // admin can revoke any key but a non-admin can only revoke their own.
    // The inner SQL is also scoped by user_id as defense-in-depth; see
    // `auth::revoke_key`.
    let mut target_user = auth_ctx.user_id;
    if auth_ctx.has_scope(&auth::Scope::Admin) {
        // Look up the key's owner by id so admin revocations resolve
        // against the correct tenant row.
        let owner: Option<i64> = state
            .db
            .read(move |conn| {
                conn.query_row(
                    "SELECT user_id FROM api_keys WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
            })
            .await?;

        match owner {
            Some(uid) => target_user = uid,
            None => {
                return Err(AppError(engram_lib::EngError::NotFound(
                    "key not found".into(),
                )));
            }
        }
    } else {
        let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
        if !keys.iter().any(|k| k.id == id) {
            return Err(AppError(engram_lib::EngError::Auth("Forbidden".into())));
        }
    }

    auth::revoke_key(&state.db, target_user, id).await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
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
        Some(old_key.rate_limit as i64),
    )
    .await?;

    // Grace period on old key -- caller override takes precedence over
    // `auth_key_rotation_grace_hours` config default. Clamped to >= 1h so
    // the returned key always has a non-trivial overlap window.
    //
    // SECURITY (SEC-HIGH-5): the UPDATE is scoped to the caller's user_id
    // as defense-in-depth. Ownership was already verified above via
    // list_keys, but constraining the SQL means an attacker who bypassed
    // the in-memory check still cannot touch another tenant's keys.
    let grace_hours = body
        .grace_hours
        .unwrap_or(state.config.auth_key_rotation_grace_hours)
        .max(1);
    let grace_expiry = (chrono::Utc::now() + chrono::Duration::hours(grace_hours))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let key_id = body.key_id;
    let user_id = auth_ctx.user_id;
    let grace_expiry_clone = grace_expiry.clone();
    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE api_keys SET expires_at = ?1 WHERE id = ?2 AND user_id = ?3",
                params![grace_expiry_clone, key_id, user_id],
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    Ok(Json(json!({
        "new_key": raw_key,
        "new_key_id": new_key.id,
        "old_key_id": body.key_id,
        "old_key_expires": grace_expiry,
        "grace_hours": grace_hours,
        "message": format!(
            "Old key will expire in {} hour(s). Update your clients to use the new key.",
            grace_hours
        ),
    })))
}

// ---- User Management ----

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
    let username = body.username.trim().to_string();
    let email = body.email.clone();
    let role = role.to_string();

    let (id, created_at) = state
        .db
        .write(move |conn| {
            conn.query_row(
                "INSERT INTO users (username, email, role, is_admin) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
                params![username, email, role, is_admin],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    // Create default space (best-effort).
    if let Err(e) = state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO spaces (user_id, name) VALUES (?1, 'default')",
                params![id],
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
    {
        tracing::warn!(
            user_id = id,
            error = %e,
            "failed to create default space after user provisioning",
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "username": body.username.trim(),
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

    let users: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, username, email, role, is_admin, created_at FROM users ORDER BY id",
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;

            let mut result = Vec::new();
            for row in rows {
                let (id, username, email, role, is_admin, created_at) =
                    row.map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
                result.push(json!({
                    "id": id,
                    "username": username,
                    "email": email,
                    "role": role,
                    "is_admin": is_admin != 0,
                    "created_at": created_at,
                }));
            }
            Ok(result)
        })
        .await?;

    Ok(Json(json!({ "users": users })))
}

// ---- Space Management ----

async fn create_space(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateSpaceBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "name is required".into(),
        )));
    }

    let user_id = auth_ctx.user_id;
    let description = body.description.clone();
    let name_clone = name.clone();

    let (id, created_at) = state
        .db
        .write(move |conn| {
            conn.query_row(
                "INSERT INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3) RETURNING id, created_at",
                params![user_id, name_clone, description],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

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
    let user_id = auth_ctx.user_id;

    let spaces: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, description, created_at FROM spaces WHERE user_id = ?1 ORDER BY id",
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params![user_id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;

            let mut result = Vec::new();
            for row in rows {
                let (id, name, description, created_at) =
                    row.map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
                result.push(json!({
                    "id": id,
                    "name": name,
                    "description": description,
                    "created_at": created_at,
                }));
            }
            Ok(result)
        })
        .await?;

    Ok(Json(json!({ "spaces": spaces })))
}

async fn delete_space(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // Verify ownership
    let row: Option<(i64, String)> = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, name FROM spaces WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let (owner, name) =
        row.ok_or_else(|| AppError(engram_lib::EngError::NotFound("Not found".into())))?;

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
        .write(move |conn| {
            conn.execute("DELETE FROM spaces WHERE id = ?1", params![id])
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    Ok(Json(json!({ "deleted": true, "id": id })))
}
