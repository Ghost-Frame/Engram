use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use kleos_lib::spaces::{self, InstanceAccess};
use kleos_lib::{audit, auth};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{CreateGrantBody, CreateKeyBody, CreateSpaceBody, ListGrantsQuery, RotateKeyBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/keys", post(create_key).get(list_keys))
        .route("/keys/{id}", delete(revoke_key))
        .route("/keys/rotate", post(rotate_key))
        // Caller identity + scopes, so the GUI can gate admin-only surfaces.
        .route("/me", get(whoami))
        // /users routes moved to routes::users module
        .route("/spaces", post(create_space).get(list_spaces))
        .route("/spaces/{id}", delete(delete_space))
        // Instance-level access grants (Space Sharing, whole-instance model).
        .route(
            "/instance-grants",
            post(create_instance_grant).get(list_instance_grants),
        )
        .route(
            "/instance-grants/{owner}/{grantee}",
            delete(revoke_instance_grant),
        )
        // Admin-wide overviews for the Spaces and Sharing console. A dedicated
        // /sharing/* namespace avoids shadowing the GUI's /admin/spaces browser
        // route and any /spaces/{id} or /instance-grants/{..} param routes.
        .route("/sharing/grants", get(list_all_instance_grants))
        .route("/sharing/spaces", get(list_all_spaces))
}

// ---- Caller identity ----

/// GET /me -- report the authenticated caller's identity and scopes so the GUI
/// can gate admin-only surfaces (e.g. the Spaces and Sharing page). This always
/// reflects the REAL caller, never an act-as target.
async fn whoami(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    let scopes: Vec<String> = auth_ctx.key.scopes.iter().map(|s| s.to_string()).collect();
    let is_admin = auth_ctx.has_scope(&auth::Scope::Admin);
    let uid = auth_ctx.user_id;
    let username: Option<String> = state
        .db
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT username FROM users WHERE id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .optional()?)
        })
        .await?;
    Ok(Json(json!({
        "user_id": auth_ctx.user_id,
        "username": username,
        "scopes": scopes,
        "is_admin": is_admin,
    })))
}

// ---- Key Management ----

async fn create_key(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Only admin can create keys
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
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
                Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
                    params![uid],
                    |row| row.get(0),
                )?)
            })
            .await?;
        if !exists {
            return Err(AppError(kleos_lib::EngError::NotFound(format!(
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
            return Err(AppError(kleos_lib::EngError::Auth(format!(
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
                Ok(conn
                    .query_row(
                        "SELECT user_id FROM api_keys WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()?)
            })
            .await?;

        match owner {
            Some(uid) => target_user = uid,
            None => {
                return Err(AppError(kleos_lib::EngError::NotFound(
                    "key not found".into(),
                )));
            }
        }
    } else {
        let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
        if !keys.iter().any(|k| k.id == id) {
            return Err(AppError(kleos_lib::EngError::Auth("Forbidden".into())));
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
        return Err(AppError(kleos_lib::EngError::Auth("Admin required".into())));
    }

    // Verify old key exists and belongs to user
    let keys = auth::list_keys(&state.db, auth_ctx.user_id).await?;
    let old_key = keys
        .iter()
        .find(|k| k.id == body.key_id)
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Key not found".into())))?;

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
            Ok(conn.execute(
                "UPDATE api_keys SET expires_at = ?1 WHERE id = ?2 AND user_id = ?3",
                params![grace_expiry_clone, key_id, user_id],
            )?)
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

// ---- Space Management ----

async fn create_space(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateSpaceBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "name is required".into(),
        )));
    }

    let user_id = auth_ctx.user_id;
    let description = body.description.clone();
    let name_clone = name.clone();

    let (id, created_at) = state
        .db
        .write(move |conn| {
            Ok(conn.query_row(
                "INSERT INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3) RETURNING id, created_at",
                params![user_id, name_clone, description],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )?)
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
                ?;

            let rows = stmt
                .query_map(params![user_id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                ?;

            let mut result = Vec::new();
            for row in rows {
                let (id, name, description, created_at) =
                    row?;
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
            Ok(conn
                .query_row(
                    "SELECT user_id, name FROM spaces WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?)
        })
        .await?;

    let (owner, name) =
        row.ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Not found".into())))?;

    if owner != auth_ctx.user_id && !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth("Forbidden".into())));
    }

    if name == "default" {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Cannot delete default space".into(),
        )));
    }

    state
        .db
        .write(move |conn| Ok(conn.execute("DELETE FROM spaces WHERE id = ?1", params![id])?))
        .await?;

    Ok(Json(json!({ "deleted": true, "id": id })))
}

// ---- Instance Access Grants (Space Sharing) ----

/// Create or update a grant that delegates access to an owner's entire shard.
///
/// SD2: only the instance owner or an admin may manage grants on a shard. A
/// non-admin caller may therefore only grant access to their OWN shard.
async fn create_instance_grant(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<CreateGrantBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let owner = body.owner_user_id;
    let grantee = body.grantee_user_id;
    let access: InstanceAccess = body.access.parse().map_err(AppError)?;

    // SD2: owner-or-admin gate. Mirrors the `delete_space` ownership check.
    if owner != auth_ctx.user_id && !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "only the instance owner or an admin may manage grants".into(),
        )));
    }

    // The grantee must be a real, active user, and the owner must exist, so a
    // typo'd or disabled account can never be handed delegated access.
    let (owner_exists, grantee_active): (bool, bool) = state
        .db
        .read(move |conn| {
            let owner_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
                params![owner],
                |r| r.get(0),
            )?;
            let grantee_active: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1 AND is_active = 1)",
                params![grantee],
                |r| r.get(0),
            )?;
            Ok((owner_exists, grantee_active))
        })
        .await?;
    if !owner_exists {
        return Err(AppError(kleos_lib::EngError::NotFound(format!(
            "owner user {owner} not found"
        ))));
    }
    if !grantee_active {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "grantee user {grantee} not found or inactive"
        ))));
    }

    // Storage upserts on (owner, grantee) and rejects a self-grant.
    spaces::grant_instance_access(&state.db, owner, grantee, access, auth_ctx.user_id).await?;

    // SD5: record the grant in the audit trail (who granted what to whom).
    let _ = audit::log_mutation(
        &state.db,
        "instance_grant.create",
        "instance_grant",
        &owner.to_string(),
        Some(&auth_ctx.user_id.to_string()),
        Some(auth_ctx.user_id),
        None,
        Some(json!({ "owner": owner, "grantee": grantee, "access": access.as_str() })),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "owner_user_id": owner,
            "grantee_user_id": grantee,
            "access": access.as_str(),
        })),
    ))
}

/// List the grants an owner has issued. Defaults to the caller's own shard;
/// querying another owner requires Admin (or being that owner).
async fn list_instance_grants(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Query(q): Query<ListGrantsQuery>,
) -> Result<Json<Value>, AppError> {
    let owner = q.owner.unwrap_or(auth_ctx.user_id);

    if owner != auth_ctx.user_id && !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "only the instance owner or an admin may list grants".into(),
        )));
    }

    let grants = spaces::list_grants_for_owner(&state.db, owner).await?;
    let grants_json: Vec<Value> = grants
        .iter()
        .map(|g| {
            json!({
                "owner_user_id": g.owner_user_id,
                "grantee_user_id": g.grantee_user_id,
                "access": g.access.as_str(),
                "granted_by": g.granted_by,
                "created_at": g.created_at,
            })
        })
        .collect();

    Ok(Json(
        json!({ "grants": grants_json, "count": grants_json.len() }),
    ))
}

/// Revoke a grant. Idempotent. SD2: owner-or-admin gate.
async fn revoke_instance_grant(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path((owner, grantee)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    if owner != auth_ctx.user_id && !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "only the instance owner or an admin may revoke grants".into(),
        )));
    }

    spaces::revoke_instance_access(&state.db, owner, grantee).await?;

    // SD5: record the revoke in the audit trail.
    let _ = audit::log_mutation(
        &state.db,
        "instance_grant.revoke",
        "instance_grant",
        &owner.to_string(),
        Some(&auth_ctx.user_id.to_string()),
        Some(auth_ctx.user_id),
        Some(json!({ "owner": owner, "grantee": grantee })),
        None,
    )
    .await;

    Ok(Json(json!({
        "revoked": true,
        "owner_user_id": owner,
        "grantee_user_id": grantee,
    })))
}

/// GET /admin/instance-grants -- every instance grant across all owners, with
/// usernames resolved, for the admin Spaces and Sharing overview. Admin only.
async fn list_all_instance_grants(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "admin scope required".into(),
        )));
    }
    let grants: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT g.owner_user_id, ou.username, g.grantee_user_id, gu.username,
                        g.access, g.granted_by, bu.username, g.created_at
                 FROM instance_grants g
                 LEFT JOIN users ou ON ou.id = g.owner_user_id
                 LEFT JOIN users gu ON gu.id = g.grantee_user_id
                 LEFT JOIN users bu ON bu.id = g.granted_by
                 ORDER BY ou.username, gu.username",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "owner_user_id": r.get::<_, i64>(0)?,
                        "owner_username": r.get::<_, Option<String>>(1)?,
                        "grantee_user_id": r.get::<_, i64>(2)?,
                        "grantee_username": r.get::<_, Option<String>>(3)?,
                        "access": r.get::<_, String>(4)?,
                        "granted_by": r.get::<_, i64>(5)?,
                        "granted_by_username": r.get::<_, Option<String>>(6)?,
                        "created_at": r.get::<_, String>(7)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<Value>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(json!({ "grants": grants, "count": grants.len() })))
}

/// GET /admin/spaces -- every named space across all users, with the owner
/// username resolved, for the admin Spaces and Sharing overview. Admin only.
async fn list_all_spaces(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth_ctx.has_scope(&auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "admin scope required".into(),
        )));
    }
    let spaces: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT s.id, s.user_id, u.username, s.name, s.description, s.created_at
                 FROM spaces s
                 LEFT JOIN users u ON u.id = s.user_id
                 ORDER BY u.username, s.name",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "owner_user_id": r.get::<_, i64>(1)?,
                        "owner_username": r.get::<_, Option<String>>(2)?,
                        "name": r.get::<_, String>(3)?,
                        "description": r.get::<_, Option<String>>(4)?,
                        "created_at": r.get::<_, String>(5)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<Value>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(json!({ "spaces": spaces, "count": spaces.len() })))
}
