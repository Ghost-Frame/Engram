//! GET /bootstrap/kleos-bearer?agent=<slot>
//!
//! Brokers per-agent Kleos bearers to local clients (kleos-cli, sidecar,
//! shell hooks) without putting any plaintext credential on disk.
//!
//! Flow:
//!   1. Client (with a scoped CREDD_AGENT_KEY) calls /bootstrap/kleos-bearer.
//!   2. credd authenticates: owner key or agent token with bootstrap/<slot>
//!      scope. (Stage 5 ships owner-only; Stage 6 adds scoped-agent support.)
//!   3. Special case: agent == "credd-<host>" returns bootstrap_master itself
//!      (credd's own Kleos bearer). Owner-only.
//!   4. Otherwise: credd uses bootstrap_master as a Kleos bearer to fetch the
//!      [CRED:v3] kleos/<agent> memory row from Kleos, decrypts it with
//!      state.master_key, returns the bare per-agent bearer.
//!   5. Response includes a TTL hint so clients can cache without re-asking
//!      every call. Per-agent bearers themselves are rotated separately
//!      (Kleos-side); the TTL is the cache invalidation primitive.

use axum::{
    extract::{Query, State},
    Json,
};
use kleos_cred::crypto::decrypt;
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::auth::{Auth, AuthInfo};
use crate::handlers::AppError;
use crate::state::AppState;
use kleos_cred::CredError;

/// How long the returned bearer should be cached by the client. Master may
/// override per Stage 14 follow-up (signed JWT with embedded `exp`).
const DEFAULT_TTL_SECS: u64 = 3600;

#[derive(Deserialize)]
pub struct BootstrapBearerParams {
    pub agent: String,
}

#[derive(Serialize)]
pub struct BootstrapBearerResponse {
    /// Bare per-agent Kleos bearer.
    pub key: String,
    /// RFC 3339 timestamp at which the client should refetch.
    pub expires_at: String,
    /// Cache duration in seconds (informational; `expires_at` is canonical).
    pub ttl_secs: u64,
}

/// Subset of the Kleos `/list` response we care about.
#[derive(Deserialize)]
struct KleosListResponse {
    results: Vec<KleosMemoryRow>,
}

#[derive(Deserialize)]
struct KleosMemoryRow {
    content: String,
}

/// GET /bootstrap/kleos-bearer?agent=<slot>
pub async fn get_bootstrap_kleos_bearer(
    Auth(auth): Auth,
    Query(params): Query<BootstrapBearerParams>,
    State(state): State<AppState>,
) -> Result<Json<BootstrapBearerResponse>, AppError> {
    // No bootstrap.enc loaded -> nothing to broker.
    let bootstrap_master = state
        .bootstrap_master
        .as_ref()
        .ok_or_else(|| {
            CredError::NotFound("no bootstrap key loaded (bootstrap.enc absent)".into())
        })?
        .clone();

    let hostname = read_hostname();
    let credd_slot = format!("credd-{}", hostname);
    let caller_id = match &auth {
        AuthInfo::Master { .. } => "owner".to_string(),
        AuthInfo::Agent { key, .. } => key.name.clone(),
    };

    // Privileged self-fetch: credd's own per-host bearer.
    if params.agent == credd_slot {
        if !auth.is_master() {
            warn!(
                caller = %caller_id,
                agent = %params.agent,
                "non-owner attempted credd self-bearer fetch"
            );
            return Err(CredError::PermissionDenied(
                "credd self-bearer requires owner auth".into(),
            )
            .into());
        }
        return Ok(Json(make_response(bootstrap_master.as_str())));
    }

    // Stage 5: owner-only for non-self bearers. Stage 6 will accept scoped
    // bootstrap-agent tokens via a new AuthInfo variant.
    if !auth.is_master() {
        warn!(
            caller = %caller_id,
            agent = %params.agent,
            "non-owner attempted /bootstrap/kleos-bearer (scoped agent support arrives in Stage 6)"
        );
        return Err(
            CredError::PermissionDenied(format!("no bootstrap scope for agent={}", params.agent))
                .into(),
        );
    }

    // Fetch the [CRED:v3] kleos/<agent> memory from Kleos, decrypt it
    // with the cred master key, return the bare bearer.
    let kleos_url = std::env::var("KLEOS_URL")
        .or_else(|_| std::env::var("KLEOS_URL"))
        .map_err(|_| {
            error!(
                "KLEOS_URL not set; cannot fetch bearer for agent={}",
                params.agent
            );
            CredError::InvalidInput("credd misconfigured: KLEOS_URL not set".into())
        })?;

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/list", kleos_url.trim_end_matches('/')))
        .header("Authorization", format!("Bearer {}", bootstrap_master.as_str()))
        .query(&[("category", "credential"), ("limit", "500")])
        .send()
        .await
        .map_err(|e| {
            error!(
                "Kleos /list failed for agent={}: {}",
                params.agent, e
            );
            CredError::InvalidInput(format!("kleos unreachable: {}", e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        error!(
            "Kleos /list returned {} for agent={}",
            status, params.agent
        );
        return Err(CredError::InvalidInput(format!("kleos /list error: {}", status)).into());
    }

    let list: KleosListResponse = resp.json().await.map_err(|e| {
        error!(
            "Kleos /list parse error for agent={}: {}",
            params.agent, e
        );
        CredError::InvalidInput(format!("kleos response parse error: {}", e))
    })?;

    let target_prefix = format!("[CRED:v3] kleos/{} = ", params.agent);
    let entry = list
        .results
        .iter()
        .find(|m| m.content.starts_with(&target_prefix))
        .ok_or_else(|| {
            warn!(
                "no [CRED:v3] entry for agent={} (looked for prefix `{}`)",
                params.agent, target_prefix
            );
            CredError::NotFound(format!("agent bearer not found: {}", params.agent))
        })?;

    let hex_data = entry.content[target_prefix.len()..].trim();
    let ciphertext = hex::decode(hex_data).map_err(|e| {
        error!(
            "hex decode failed for agent={}: {}",
            params.agent, e
        );
        CredError::Decryption("corrupt cred entry: hex decode failed".into())
    })?;

    let plaintext = decrypt(state.master_key.as_ref(), &ciphertext).map_err(|e| {
        error!(
            "decrypt failed for agent={}: {}",
            params.agent, e
        );
        CredError::Decryption("corrupt cred entry: decrypt failed".into())
    })?;

    let value: serde_json::Value = serde_json::from_slice(&plaintext).map_err(|e| {
        error!(
            "JSON parse failed for agent={}: {}",
            params.agent, e
        );
        CredError::InvalidInput("corrupt cred entry: JSON parse failed".into())
    })?;

    // Expect SecretData::ApiKey shape: {"type":"api_key","key":"...","endpoint":..,"notes":..}
    let bare_key = value
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!(
                "cred entry for agent={} has no `key` field",
                params.agent
            );
            CredError::InvalidInput("expected ApiKey-typed cred entry".into())
        })?
        .to_string();

    Ok(Json(make_response(&bare_key)))
}

/// Build the response with a TTL hint anchored to the current time.
fn make_response(key: &str) -> BootstrapBearerResponse {
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(DEFAULT_TTL_SECS as i64);
    BootstrapBearerResponse {
        key: key.to_string(),
        expires_at: expires_at.to_rfc3339(),
        ttl_secs: DEFAULT_TTL_SECS,
    }
}

/// Read the hostname for the credd-self-bearer slot match.
/// Linux: /etc/hostname. Otherwise the HOSTNAME env. Default "unknown".
fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into()))
}
