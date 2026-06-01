use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use kleos_lib::agents;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{fs, path::PathBuf, sync::OnceLock};
use subtle::ConstantTimeEq;

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};

mod types;
use types::{ExecutionsQuery, LinkKeyBody, RegisterBody, RevokeBody, VerifyBody};

type HmacSha256 = Hmac<Sha256>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", post(register_agent).get(list_agents))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/revoke", post(revoke_agent))
        .route("/agents/{id}/passport", get(get_passport))
        .route("/agents/{id}/link-key", post(link_key))
        .route("/agents/{id}/executions", get(get_executions))
        .route("/verify", post(verify))
}

async fn register_agent(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<RegisterBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if body.name.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "name (string) required".into(),
        )));
    }

    // Rely on UNIQUE(user_id, name) constraint to prevent duplicates
    // atomically instead of a check-then-insert race (TOCTOU).
    let result = match agents::insert_agent(
        &db,
        auth.effective_user_id(),
        &body.name,
        body.category.as_deref(),
        body.description.as_deref(),
        body.code_hash.as_deref(),
    )
    .await
    {
        Ok(r) => r,
        Err(kleos_lib::EngError::DatabaseMessage(msg))
            if msg.contains("UNIQUE constraint failed") =>
        {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "Agent '{}' already registered",
                body.name
            ))));
        }
        Err(e) => return Err(AppError(e)),
    };

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "agent_id": result.id,
            "name": body.name,
            "trust_score": result.trust_score,
            "created_at": result.created_at,
        })),
    ))
}

async fn list_agents(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let agents = agents::list_agents(&db, auth.effective_user_id()).await?;
    Ok(Json(json!({ "agents": agents })))
}

async fn get_agent(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.effective_user_id())
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    // Omit code_hash from response (matches TS behavior)
    Ok(Json(json!({
        "id": agent.id,
        "user_id": agent.user_id,
        "name": agent.name,
        "category": agent.category,
        "description": agent.description,
        "trust_score": agent.trust_score,
        "total_ops": agent.total_ops,
        "successful_ops": agent.successful_ops,
        "failed_ops": agent.failed_ops,
        "guard_allows": agent.guard_allows,
        "guard_warns": agent.guard_warns,
        "guard_blocks": agent.guard_blocks,
        "is_active": agent.is_active,
        "last_seen_at": agent.last_seen_at,
        "revoked_at": agent.revoked_at,
        "revoke_reason": agent.revoke_reason,
        "created_at": agent.created_at,
    })))
}

async fn revoke_agent(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<RevokeBody>,
) -> Result<Json<Value>, AppError> {
    let reason = body.reason.as_deref().unwrap_or("revoked");
    agents::revoke_agent(&db, id, auth.effective_user_id(), reason).await?;
    Ok(Json(json!({ "revoked": true, "agent_id": id })))
}

async fn get_passport(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.effective_user_id())
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    if !agent.is_active {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Agent is revoked".into(),
        )));
    }

    let issued_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let payload = json!({
        "agent_id": agent.id,
        "user_id": auth.effective_user_id(),
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
    });
    let signature = sign_value(&payload)?;
    Ok(Json(json!({
        "agent_id": agent.id,
        "user_id": auth.effective_user_id(),
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
        "signature": signature,
    })))
}

async fn link_key(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<LinkKeyBody>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.effective_user_id())
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    // api_keys lives in the system DB, not the tenant shard.
    agents::link_key_to_agent(&state.db, agent.id, body.key_id, auth.effective_user_id()).await?;
    Ok(Json(
        json!({ "linked": true, "agent_id": id, "key_id": body.key_id }),
    ))
}

async fn get_executions(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<ExecutionsQuery>,
) -> Result<Json<Value>, AppError> {
    // Verify agent belongs to user
    let agent = agents::get_agent_by_id(&db, id, auth.effective_user_id())
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    let limit = params.limit.unwrap_or(50).min(1000);
    let executions =
        agents::get_agent_executions(&db, agent.id, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "agent_id": id, "executions": executions })))
}

// --- Verify DTOs ---

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyExecutionInput {
    agent_identity_pem: String,
    actions_log: Vec<ActionEntry>,
    signature_hex: String,
}

#[derive(Deserialize, Serialize)]
struct ActionEntry {
    ts: i64,
    kind: String,
    target: String,
    result: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyMessageInput {
    agent_identity_pem: String,
    message: serde_json::Value,
    signature_hex: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyToolManifestInput {
    agent_identity_pem: String,
    declared_tools: Vec<String>,
    signature_hex: String,
}

// --- Verify handler ---

// Supported kinds:
//   - passport: HMAC-based server-signed credential check
//   - execution: Ed25519 signature over canonicalized action log
//   - message: Ed25519 signature over canonicalized JSON message
//   - tool_manifest: Ed25519 signature over sorted tool list; persists to DB
async fn verify(
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<VerifyBody>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    if let Some(passport) = body.passport {
        let result = verify_signed_value(&passport).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.0.to_string() })),
            )
        })?;
        return Ok(Json(json!({ "type": "passport", "valid": result })));
    }

    if let Some(raw) = body.execution {
        let input: VerifyExecutionInput = serde_json::from_value(raw).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid execution payload: {e}") })),
            )
        })?;
        let verified = verify_execution(&input).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.0.to_string() })),
            )
        })?;
        return Ok(Json(json!({ "type": "execution", "verified": verified })));
    }

    if let Some(raw) = body.message {
        let input: VerifyMessageInput = serde_json::from_value(raw).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid message payload: {e}") })),
            )
        })?;
        let verified = verify_message(&input).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.0.to_string() })),
            )
        })?;
        return Ok(Json(json!({ "type": "message", "verified": verified })));
    }

    if let Some(raw) = body.tool_manifest {
        let input: VerifyToolManifestInput = serde_json::from_value(raw).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid tool_manifest payload: {e}") })),
            )
        })?;
        let (verified, manifest_hash, first_seen) =
            verify_tool_manifest(&input, &db).await.map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.0.to_string() })),
                )
            })?;
        return Ok(Json(json!({
            "type": "tool_manifest",
            "verified": verified,
            "manifest_hash": manifest_hash,
            "first_seen": first_seen,
        })));
    }

    Err((
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "Provide 'passport', 'execution', 'message', or 'tool_manifest' to verify",
        })),
    ))
}

// --- Verify helpers ---

fn verify_execution(input: &VerifyExecutionInput) -> Result<bool, AppError> {
    // Canonicalize: serialize each action entry with sorted keys, then
    // wrap in an array. We use serde_json's default key order (struct field
    // declaration order) which is deterministic for known structs.
    let canonical = serde_json::to_string(&input.actions_log)
        .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;

    let ok = kleos_lib::auth_piv::verify_signature(
        kleos_lib::auth_piv::SignatureAlgo::Ed25519,
        &input.agent_identity_pem,
        canonical.as_bytes(),
        &input.signature_hex,
    );
    Ok(ok.is_ok())
}

fn verify_message(input: &VerifyMessageInput) -> Result<bool, AppError> {
    // Canonicalize: sort object keys recursively, then serialize with no
    // extra whitespace.
    let canonical = canonical_json(&input.message)
        .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;

    let ok = kleos_lib::auth_piv::verify_signature(
        kleos_lib::auth_piv::SignatureAlgo::Ed25519,
        &input.agent_identity_pem,
        canonical.as_bytes(),
        &input.signature_hex,
    );
    Ok(ok.is_ok())
}

async fn verify_tool_manifest(
    input: &VerifyToolManifestInput,
    db: &std::sync::Arc<kleos_lib::db::Database>,
) -> Result<(bool, String, bool), AppError> {
    // Sort the declared tools list for deterministic canonical form.
    let mut sorted_tools = input.declared_tools.clone();
    sorted_tools.sort();

    let canonical = serde_json::to_string(&sorted_tools)
        .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;

    let verified = kleos_lib::auth_piv::verify_signature(
        kleos_lib::auth_piv::SignatureAlgo::Ed25519,
        &input.agent_identity_pem,
        canonical.as_bytes(),
        &input.signature_hex,
    )
    .is_ok();

    // Compute SHA-256 manifest hash.
    let manifest_hash = kleos_lib::artifacts::sha256_hex(canonical.as_bytes());

    if !verified {
        return Ok((false, manifest_hash, false));
    }

    // Look up agent_identity_id from PEM, then persist manifest.
    let pem = input.agent_identity_pem.clone();
    let hash = manifest_hash.clone();
    let tools_json = canonical;

    let first_seen = db
        .write(move |conn| {
            // Find identity_keys row by pubkey_pem.
            let identity_id: Option<i64> = match conn.query_row(
                "SELECT id FROM identity_keys WHERE pubkey_pem = ?1 LIMIT 1",
                rusqlite::params![pem],
                |row| row.get(0),
            ) {
                Ok(id) => Some(id),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(kleos_lib::EngError::Database(e)),
            };

            let Some(identity_id) = identity_id else {
                // Unknown key -- still return verified=true but no DB record.
                return Ok(false);
            };

            // INSERT OR IGNORE; check affected rows to determine first_seen.
            let rows = conn.execute(
                "INSERT OR IGNORE INTO tool_manifests \
                     (agent_identity_id, manifest_hash, declared_tools_json) \
                     VALUES (?1, ?2, ?3)",
                rusqlite::params![identity_id, hash, tools_json],
            )?;

            Ok(rows > 0)
        })
        .await
        .map_err(AppError)?;

    Ok((true, manifest_hash, first_seen))
}

/// Recursively sort object keys in a JSON value and re-serialize without
/// extra whitespace, producing a stable canonical form for signature
/// verification.
fn canonical_json(value: &serde_json::Value) -> serde_json::Result<String> {
    let sorted = sort_json_keys(value);
    serde_json::to_string(&sorted)
}

fn sort_json_keys(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k.clone(), sort_json_keys(&map[k]));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_keys).collect())
        }
        other => other.clone(),
    }
}

fn signing_secret() -> Result<&'static str, AppError> {
    static SECRET: OnceLock<String> = OnceLock::new();
    let secret = SECRET.get_or_init(load_or_create_signing_secret);
    if secret.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::Internal(
            "signing secret is empty".into(),
        )));
    }
    Ok(secret.as_str())
}

fn load_or_create_signing_secret() -> String {
    // SECURITY (SEC-MED-5): use OsRng for 256-bit signing secret instead of
    // UUID v4 which has only ~122 bits and fixed version/variant bits.
    let generated = {
        use rand::rngs::OsRng;
        use rand::TryRngCore;
        let mut raw = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut raw)
            .expect("OS CSPRNG must be available");
        hex::encode(raw)
    };

    // L-R3-003: refuse to fall back to ./kleos-signing-secret.txt when
    // dirs::data_dir() resolves to None. Writing the secret into CWD made
    // it readable by any local user who could reach the working directory
    // (e.g. /tmp during ad-hoc testing). If we cannot resolve a real
    // user-data dir, run with an in-memory secret and surface a loud
    // warning so the operator notices.
    let path = match signing_secret_path() {
        Some(p) => p,
        None => {
            tracing::error!(
                "no env ENGRAM_SIGNING_SECRET_FILE and dirs::data_dir() returned None; \
                 running with an in-memory signing secret (regenerates on every restart). \
                 Set ENGRAM_SIGNING_SECRET_FILE to an absolute path to persist."
            );
            return generated;
        }
    };

    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }

    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!(
                path = %parent.display(),
                error = %e,
                "failed to create signing secret parent dir; secret will not persist across restarts"
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let write_result = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(generated.as_bytes())
            });
        if let Err(e) = write_result {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to persist signing secret; new secret will be regenerated on next restart"
            );
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = fs::write(&path, &generated) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to persist signing secret; new secret will be regenerated on next restart"
            );
        }
    }
    generated
}

fn signing_secret_path() -> Option<PathBuf> {
    if let Ok(path) = kleos_lib::kleos_env("SIGNING_SECRET_FILE") {
        if !path.trim().is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    // L-R3-003: no CWD fallback. If neither env var nor data_dir resolves,
    // the caller runs with an in-memory secret instead of leaking it into
    // an unpredictable on-disk location.
    dirs::data_dir().map(|d| d.join("kleos").join("signing-secret"))
}

fn sign_value(payload: &Value) -> Result<String, AppError> {
    let secret = signing_secret()?;
    let bytes = serde_json::to_vec(payload)
        .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(
        |e: hmac::digest::InvalidLength| AppError(kleos_lib::EngError::Internal(e.to_string())),
    )?;
    mac.update(&bytes);
    let digest = mac.finalize().into_bytes();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

fn verify_signed_value(value: &Value) -> Result<bool, AppError> {
    let Some(signature) = value.get("signature").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    let mut unsigned = value.clone();
    if let Some(obj) = unsigned.as_object_mut() {
        obj.remove("signature");
    }
    let computed = sign_value(&unsigned)?;
    Ok(computed.as_bytes().ct_eq(signature.as_bytes()).into())
}
