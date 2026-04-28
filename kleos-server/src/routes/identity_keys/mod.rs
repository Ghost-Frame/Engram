use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::auth::Scope;
use kleos_lib::auth_piv;

mod types;
use types::{EnrollBody, ListParams, RevokeBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/identity-keys/enroll", post(enroll_handler))
        .route("/identity-keys", get(list_handler))
        .route("/identity-keys/mine", get(list_mine_handler))
        .route("/identity-keys/{id}/revoke", post(revoke_handler))
}

async fn enroll_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<EnrollBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let algo =
        auth_piv::SignatureAlgo::from_header(&body.algo).map_err(|e| AppError(e))?;

    if !["piv", "soft"].contains(&body.tier.as_str()) {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "tier must be 'piv' or 'soft'".into(),
        )));
    }

    let proof_msg = format!(
        "KLEOS-ENROLL:{}:{}:{}:{}",
        body.algo, body.tier, body.host_label, body.pubkey_pem
    );
    auth_piv::verify_signature(algo, &body.pubkey_pem, proof_msg.as_bytes(), &body.sig_hex)?;

    let pubkey_der = pem_to_der(&body.pubkey_pem)?;
    let fingerprint = hex::encode(Sha256::digest(&pubkey_der));

    let user_id = auth.user_id;
    let tier = body.tier.clone();
    let algo_str = algo.as_str().to_string();
    let pubkey_pem = body.pubkey_pem.clone();
    let fpr = fingerprint.clone();
    let host = body.host_label.clone();
    let label = body.label.clone();
    let serial = body.serial.clone();

    let id: i64 = state.db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO identity_keys (user_id, tier, algo, pubkey_pem, pubkey_fingerprint, host_label, label, serial)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![user_id, tier, algo_str, pubkey_pem, fpr, host, label, serial],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "pubkey_fingerprint": fingerprint,
            "host_label": body.host_label,
            "tier": body.tier,
        })),
    ))
}

async fn list_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for listing all identity keys".into(),
        )));
    }

    let active_only = params.active_only.unwrap_or(true);

    let keys = state.db
        .read(move |conn| {
            let mut stmt = if active_only {
                conn.prepare(
                    "SELECT id, user_id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys WHERE is_active = 1 ORDER BY enrolled_at DESC",
                )
            } else {
                conn.prepare(
                    "SELECT id, user_id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys ORDER BY enrolled_at DESC",
                )
            }
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "user_id": row.get::<_, i64>(1)?,
                        "tier": row.get::<_, String>(2)?,
                        "algo": row.get::<_, String>(3)?,
                        "pubkey_fingerprint": row.get::<_, String>(4)?,
                        "host_label": row.get::<_, String>(5)?,
                        "label": row.get::<_, Option<String>>(6)?,
                        "serial": row.get::<_, Option<String>>(7)?,
                        "enrolled_at": row.get::<_, String>(8)?,
                        "last_seen_at": row.get::<_, Option<String>>(9)?,
                        "is_active": row.get::<_, bool>(10)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

async fn list_mine_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    let keys = state.db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys WHERE user_id = ?1 ORDER BY enrolled_at DESC",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "tier": row.get::<_, String>(1)?,
                        "algo": row.get::<_, String>(2)?,
                        "pubkey_fingerprint": row.get::<_, String>(3)?,
                        "host_label": row.get::<_, String>(4)?,
                        "label": row.get::<_, Option<String>>(5)?,
                        "serial": row.get::<_, Option<String>>(6)?,
                        "enrolled_at": row.get::<_, String>(7)?,
                        "last_seen_at": row.get::<_, Option<String>>(8)?,
                        "is_active": row.get::<_, bool>(9)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

async fn revoke_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(key_id): Path<i64>,
    Json(body): Json<RevokeBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;
    let is_admin = auth.has_scope(&Scope::Admin);
    let reason = body.reason;

    let revoked = state.db
        .write(move |conn| {
            let affected = if is_admin {
                conn.execute(
                    "UPDATE identity_keys SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?2
                     WHERE id = ?1 AND is_active = 1",
                    params![key_id, reason],
                )
            } else {
                conn.execute(
                    "UPDATE identity_keys SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?3
                     WHERE id = ?1 AND is_active = 1 AND user_id = ?2",
                    params![key_id, user_id, reason],
                )
            }
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(affected > 0)
        })
        .await?;

    if revoked {
        Ok(Json(json!({ "revoked": true, "id": key_id })))
    } else {
        Err(AppError(kleos_lib::EngError::NotFound(
            "identity key not found or already revoked".into(),
        )))
    }
}

fn pem_to_der(pem: &str) -> Result<Vec<u8>, AppError> {
    let begin = "-----BEGIN PUBLIC KEY-----";
    let end = "-----END PUBLIC KEY-----";
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with(begin))
        .skip(1)
        .take_while(|l| !l.starts_with(end))
        .collect::<Vec<_>>()
        .join("");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| {
            AppError(kleos_lib::EngError::InvalidInput(format!(
                "bad PEM: {e}"
            )))
        })
}
