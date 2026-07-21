//! Identity key management endpoints.
//!
//! Handles enrollment, listing, revocation, and invite generation for
//! cryptographic signing keys (PIV YubiKey, software Ed25519, and
//! eventually FIDO2 security keys).

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

/// Registers all identity key management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/identity-keys/enroll", post(enroll_handler))
        .route(
            "/identity-keys/enroll/challenge",
            post(enroll_challenge_handler),
        )
        .route("/identity-keys", get(list_handler))
        .route("/identity-keys/mine", get(list_mine_handler))
        .route("/identity-keys/{id}/revoke", post(revoke_handler))
}

/// Lifetime of an enrollment challenge nonce in seconds.
const ENROLL_CHALLENGE_TTL_SECS: i64 = 300;

/// Issues a single-use enrollment challenge nonce bound to the caller.
/// The nonce must be included in the proof-of-possession signature of a
/// subsequent POST /identity-keys/enroll, which prevents a captured
/// enrollment proof from being replayed by another principal.
async fn enroll_challenge_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    // 32 random bytes hex-encoded; collision-free for PRIMARY KEY purposes.
    let mut nonce_bytes = [0u8; 32];
    {
        use rand::rngs::OsRng;
        use rand::TryRngCore;
        OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|e| AppError(kleos_lib::EngError::Internal(format!("CSPRNG: {e}"))))?;
    }
    let nonce = hex::encode(nonce_bytes);

    let user_id = auth.user_id;
    let nonce_for_insert = nonce.clone();
    let expires_at = state
        .db
        .write(move |conn| {
            // Opportunistic purge so abandoned challenges never accumulate.
            conn.execute(
                "DELETE FROM enrollment_challenges WHERE expires_at <= datetime('now')",
                [],
            )?;
            Ok(conn.query_row(
                "INSERT INTO enrollment_challenges (nonce, user_id, expires_at)
                 VALUES (?1, ?2, datetime('now', ?3))
                 RETURNING expires_at",
                params![
                    nonce_for_insert,
                    user_id,
                    format!("+{} seconds", ENROLL_CHALLENGE_TTL_SECS)
                ],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await?;

    Ok(Json(json!({ "nonce": nonce, "expires_at": expires_at })))
}

/// Enrolls a new signing key for the authenticated user after verifying
/// a proof-of-possession signature over the key material.
async fn enroll_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<EnrollBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let algo = auth_piv::SignatureAlgo::from_header(&body.algo).map_err(AppError)?;

    if !["piv", "soft"].contains(&body.tier.as_str()) {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "tier must be 'piv' or 'soft'".into(),
        )));
    }

    // The very first key (bootstrap, secret-gated in the auth middleware)
    // may use the legacy nonce-less proof: no other principals exist yet,
    // so there is no replay or squatting target. Every later enrollment
    // must present a server-issued single-use nonce bound to the caller,
    // making captured proofs worthless to anyone else.
    let key_count: i64 = state
        .db
        .read(
            |conn| Ok(conn.query_row("SELECT COUNT(*) FROM identity_keys", [], |row| row.get(0))?),
        )
        .await?;

    let proof_msg = if key_count == 0 {
        format!(
            "KLEOS-ENROLL:{}:{}:{}:{}",
            body.algo, body.tier, body.host_label, body.pubkey_pem
        )
    } else {
        let nonce = body.nonce.clone().ok_or_else(|| {
            AppError(kleos_lib::EngError::InvalidInput(
                "enrollment nonce required: request one via POST /identity-keys/enroll/challenge"
                    .into(),
            ))
        })?;

        // Atomic consume: the DELETE only succeeds for an unexpired nonce
        // issued to this caller, and removing the row makes it single-use.
        let caller_id = auth.user_id;
        let nonce_for_consume = nonce.clone();
        let consumed = state
            .db
            .write(move |conn| {
                Ok(conn.execute(
                    "DELETE FROM enrollment_challenges
                     WHERE nonce = ?1 AND user_id = ?2 AND expires_at > datetime('now')",
                    params![nonce_for_consume, caller_id],
                )? > 0)
            })
            .await?;
        if !consumed {
            return Err(AppError(kleos_lib::EngError::Auth(
                "invalid or expired enrollment challenge".into(),
            )));
        }

        format!(
            "KLEOS-ENROLL:{}:{}:{}:{}:{}",
            body.algo, body.tier, body.host_label, body.pubkey_pem, nonce
        )
    };
    auth_piv::verify_signature(algo, &body.pubkey_pem, proof_msg.as_bytes(), &body.sig_hex)?;

    let pubkey_der = pem_to_der(&body.pubkey_pem)?;
    let fingerprint = hex::encode(Sha256::digest(&pubkey_der));

    let user_id = auth.user_id;
    // SECURITY (C3): the new key inherits the caller's own scopes, so a caller
    // cannot mint a key more privileged than itself (mirrors the api-key
    // SEC-MED-4 escalation cap). Stored comma-separated to match the
    // api_keys.scopes / v53 identity_keys.scopes column format.
    let scopes_csv = kleos_lib::auth::scopes_to_string(&auth.key.scopes);
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
                "INSERT INTO identity_keys (user_id, tier, algo, pubkey_pem, pubkey_fingerprint, host_label, label, serial, scopes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![user_id, tier, algo_str, pubkey_pem, fpr, host, label, serial, scopes_csv],
            )
            ?;

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

/// Lists all identity keys across all users. Admin-only. Optionally
/// filters to active-only keys (the default).
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
            ?;

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
                ?
                .collect::<std::result::Result<Vec<_>, _>>()
                ?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

/// Lists identity keys belonging to the currently authenticated user.
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
                ?;

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
                ?
                .collect::<std::result::Result<Vec<_>, _>>()
                ?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

/// Revokes an identity key by ID. Admins can revoke any key; regular
/// users can only revoke their own.
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
            ?;

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

// Finding [44]: the enrollment-invite endpoint was removed. Invite tokens were
// minted and stored (hash-only) but no enrollment path ever consumed them, so
// the feature was dead surface: every issued token was unusable and the table
// only accumulated rows. Enrollment continues through the bootstrap and
// challenge-nonce flows above. If invite-based onboarding returns, it needs a
// deliberate design pass (token consumption inside enroll, expiry checks, and
// rate limits), not a resurrection of this handler. The enrollment_invites
// table is dropped by migration v101.

/// Extracts the raw DER bytes from a PEM-encoded public key string.
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
        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(format!("bad PEM: {e}"))))
}
