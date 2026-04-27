use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use kleos_lib::agents;
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
        auth.user_id,
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
    let agents = agents::list_agents(&db, auth.user_id).await?;
    Ok(Json(json!({ "agents": agents })))
}

async fn get_agent(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.user_id)
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
    agents::revoke_agent(&db, id, auth.user_id, reason).await?;
    Ok(Json(json!({ "revoked": true, "agent_id": id })))
}

async fn get_passport(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.user_id)
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
        "user_id": auth.user_id,
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
    });
    let signature = sign_value(&payload)?;
    Ok(Json(json!({
        "agent_id": agent.id,
        "user_id": auth.user_id,
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
        "signature": signature,
    })))
}

async fn link_key(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<LinkKeyBody>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    agents::link_key_to_agent(&db, agent.id, body.key_id, auth.user_id).await?;
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
    let agent = agents::get_agent_by_id(&db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Agent not found".into())))?;

    let limit = params.limit.unwrap_or(50).min(1000);
    let executions = agents::get_agent_executions(&db, agent.id, auth.user_id, limit).await?;
    Ok(Json(json!({ "agent_id": id, "executions": executions })))
}

// M-R3-001: previously this returned 200 OK with valid:false + an error
// string for execution/message/tool_manifest, which made callers branch on
// "valid" and treat the request as a legitimate negative result. Now we
// return 501 NOT_IMPLEMENTED so callers can distinguish "the server cannot
// answer" from "the server says invalid".
async fn verify(
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

    let kind = if body.execution.is_some() {
        Some("execution")
    } else if body.message.is_some() {
        Some("message")
    } else if body.tool_manifest.is_some() {
        Some("tool_manifest")
    } else {
        None
    };

    if let Some(kind) = kind {
        return Err((
            axum::http::StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "type": kind,
                "error": format!("verification of '{}' is not implemented", kind),
            })),
        ));
    }

    Err((
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "Provide 'passport', 'execution', 'message', or 'tool_manifest' to verify",
        })),
    ))
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
        use rand::Rng;
        let mut raw = [0u8; 32];
        rand::rng().fill(&mut raw);
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
    if let Ok(path) = std::env::var("ENGRAM_SIGNING_SECRET_FILE") {
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
