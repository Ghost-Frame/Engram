//! User management endpoints for multi-user Kleos instances.
//!
//! All routes require admin scope. The owner account (user_id=1) is
//! protected from deactivation since it bootstraps the entire instance.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::auth::Scope;

mod types;
use types::{CreateUserBody, ListUsersParams};

/// Registers the /users routes under the authenticated API router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", post(create_user))
        .route("/users", get(list_users))
        .route("/users/{id}", delete(deactivate_user))
}

/// Creates a new user account. Rejects duplicate usernames with 409.
async fn create_user(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<CreateUserBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Only admins can create user accounts.
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required to create users".into(),
        )));
    }

    let username = body.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "username cannot be empty".into(),
        )));
    }

    let email = body.email.clone();
    let role = body.role.clone().unwrap_or_else(|| "user".into());
    let role_clone = role.clone();

    let result = state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO users (username, email, role) VALUES (?1, ?2, ?3)",
                params![username, email, role_clone],
            )
            .map_err(|e| {
                // SQLite UNIQUE constraint on username triggers this.
                if e.to_string().contains("UNIQUE constraint") {
                    kleos_lib::EngError::InvalidInput(format!(
                        "username '{}' already exists",
                        username
                    ))
                } else {
                    kleos_lib::EngError::Database(e)
                }
            })?;

            let id = conn.last_insert_rowid();

            // Read back the full row so the response reflects server defaults.
            Ok(conn.query_row(
                "SELECT id, username, email, role, is_active, created_at
                 FROM users WHERE id = ?1",
                params![id],
                |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "username": row.get::<_, String>(1)?,
                        "email": row.get::<_, Option<String>>(2)?,
                        "role": row.get::<_, String>(3)?,
                        "is_active": row.get::<_, bool>(4)?,
                        "created_at": row.get::<_, String>(5)?,
                    }))
                },
            )?)
        })
        .await;

    match result {
        Ok(user) => Ok((StatusCode::CREATED, Json(user))),
        Err(kleos_lib::EngError::InvalidInput(msg)) if msg.contains("already exists") => {
            // 409 Conflict for duplicate username.
            Err(AppError(kleos_lib::EngError::Conflict(msg)))
        }
        Err(e) => Err(AppError(e)),
    }
}

/// Lists user accounts. Excludes deactivated users unless
/// `?include_inactive=true` is passed.
async fn list_users(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(params): Query<ListUsersParams>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required to list users".into(),
        )));
    }

    let include_inactive = params.include_inactive.unwrap_or(false);

    let users = state
        .db
        .read(move |conn| {
            let sql = if include_inactive {
                "SELECT id, username, email, role, is_active, created_at
                 FROM users ORDER BY id"
            } else {
                "SELECT id, username, email, role, is_active, created_at
                 FROM users WHERE is_active = 1 ORDER BY id"
            };

            let mut stmt = conn.prepare(sql)?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "username": row.get::<_, String>(1)?,
                        "email": row.get::<_, Option<String>>(2)?,
                        "role": row.get::<_, String>(3)?,
                        "is_active": row.get::<_, bool>(4)?,
                        "created_at": row.get::<_, String>(5)?,
                    }))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "users": users, "count": users.len() })))
}

/// Soft-deletes a user by setting is_active=0. Refuses to deactivate
/// user_id=1 (the owner bootstrapped at instance creation).
async fn deactivate_user(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required to deactivate users".into(),
        )));
    }

    // The owner account is the trust root -- deactivating it would lock
    // everyone out of the admin API. The caller is authenticated and holds
    // admin scope; the action itself is forbidden, so this is 403 not 401.
    if user_id == 1 {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "cannot deactivate the owner account (user_id=1)".into(),
        )));
    }

    let deactivated = state
        .db
        .transaction(move |tx| {
            let affected = tx.execute(
                "UPDATE users SET is_active = 0 WHERE id = ?1 AND is_active = 1",
                params![user_id],
            )?;
            if affected == 0 {
                return Ok(false);
            }

            // Cascade-revoke every credential the user holds so deactivation
            // takes effect immediately. Without this, api keys, identity
            // keys, identities, and mcp tokens stay live and keep granting
            // access to the deactivated account.
            tx.execute(
                "UPDATE api_keys SET is_active = 0 WHERE user_id = ?1 AND is_active = 1",
                params![user_id],
            )?;
            tx.execute(
                "UPDATE identity_keys
                 SET is_active = 0, revoked_at = datetime('now'), revoke_reason = 'user deactivated'
                 WHERE user_id = ?1 AND is_active = 1",
                params![user_id],
            )?;
            // identities has no user_id column; ownership flows through the
            // parent identity_keys row.
            tx.execute(
                "UPDATE identities SET is_active = 0
                 WHERE is_active = 1 AND identity_key_id IN
                       (SELECT id FROM identity_keys WHERE user_id = ?1)",
                params![user_id],
            )?;
            tx.execute(
                "UPDATE mcp_tokens
                 SET is_active = 0, revoked_at = datetime('now')
                 WHERE user_id = ?1 AND is_active = 1",
                params![user_id],
            )?;
            // Drop any outstanding enrollment challenge nonces so a nonce
            // issued before deactivation cannot be redeemed if the account is
            // later reactivated within the challenge TTL.
            tx.execute(
                "DELETE FROM enrollment_challenges WHERE user_id = ?1",
                params![user_id],
            )?;

            Ok(true)
        })
        .await?;

    if deactivated {
        Ok(Json(json!({ "deactivated": true, "id": user_id })))
    } else {
        Err(AppError(kleos_lib::EngError::NotFound(
            "user not found or already deactivated".into(),
        )))
    }
}
