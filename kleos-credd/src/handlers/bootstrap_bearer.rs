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
//!      [CRED:v3] engram-rust/<agent> memory row from Kleos, decrypts it with
//!      state.master_key, returns the bare per-agent bearer.
//!   5. Response includes a TTL hint so clients can cache without re-asking
//!      every call. Per-agent bearers themselves are rotated separately
//!      (Kleos-side); the TTL is the cache invalidation primitive.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use axum::{
    extract::{Query, State},
    Json,
};
use hkdf::Hkdf;
use kleos_cred::crypto::decrypt;
use kleos_cred::piv::{ecdh_agree, PivSlot};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::Signature;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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
        AuthInfo::BootstrapAgent { name, .. } => name.clone(),
    };

    // Privileged self-fetch: credd's own per-host bearer. Owner-only.
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

    // Authorization for non-self bearers: owner OR bootstrap-agent token
    // with `bootstrap/<slot>` scope (or `bootstrap/*` or `*`). DB-backed
    // resolve agents are rejected here -- they have category permissions
    // for /resolve/{text,proxy,raw}, not for the bootstrap endpoint.
    let authorized = auth.is_master() || auth.has_bootstrap_scope("bootstrap", &params.agent);
    if !authorized {
        warn!(
            caller = %caller_id,
            agent = %params.agent,
            "denied /bootstrap/kleos-bearer: no scope match"
        );
        return Err(
            CredError::PermissionDenied(format!("no bootstrap scope for agent={}", params.agent))
                .into(),
        );
    }

    // Fetch the [CRED:v3] engram-rust/<agent> memory from Kleos, decrypt it
    // with the cred master key, return the bare bearer.
    let kleos_url = std::env::var("KLEOS_URL")
        .or_else(|_| std::env::var("ENGRAM_URL"))
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

    let target_prefix = format!("[CRED:v3] engram-rust/{} = ", params.agent);
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

// ---------------------------------------------------------------------------
// Shared bearer-fetch helper
// ---------------------------------------------------------------------------

/// Fetch the bare per-agent Kleos bearer for `agent`.
/// Returns either the decrypted bearer string or, for the privileged
/// `credd-<host>` self-fetch, `bootstrap_master` itself. Does NOT enforce
/// any auth; the caller is responsible for that.
async fn resolve_agent_bearer(state: &AppState, agent: &str) -> Result<String, AppError> {
    let bootstrap_master = state
        .bootstrap_master
        .as_ref()
        .ok_or_else(|| {
            CredError::NotFound("no bootstrap key loaded (bootstrap.enc absent)".into())
        })?
        .clone();

    let hostname = read_hostname();
    let credd_slot = format!("credd-{}", hostname);
    if agent == credd_slot {
        return Ok(bootstrap_master.as_str().to_string());
    }

    let kleos_url = std::env::var("KLEOS_URL")
        .or_else(|_| std::env::var("ENGRAM_URL"))
        .map_err(|_| {
            error!("KLEOS_URL not set; cannot fetch bearer for agent={}", agent);
            CredError::InvalidInput("credd misconfigured: KLEOS_URL not set".into())
        })?;

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/list", kleos_url.trim_end_matches('/')))
        .header(
            "Authorization",
            format!("Bearer {}", bootstrap_master.as_str()),
        )
        .query(&[("category", "credential"), ("limit", "500")])
        .send()
        .await
        .map_err(|e| {
            error!("Kleos /list failed for agent={}: {}", agent, e);
            CredError::InvalidInput(format!("kleos unreachable: {}", e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        error!("Kleos /list returned {} for agent={}", status, agent);
        return Err(CredError::InvalidInput(format!("kleos /list error: {}", status)).into());
    }

    let list: KleosListResponse = resp.json().await.map_err(|e| {
        error!("Kleos /list parse error for agent={}: {}", agent, e);
        CredError::InvalidInput(format!("kleos response parse error: {}", e))
    })?;

    let target_prefix = format!("[CRED:v3] engram-rust/{} = ", agent);
    let entry = list
        .results
        .iter()
        .find(|m| m.content.starts_with(&target_prefix))
        .ok_or_else(|| {
            warn!(
                "no [CRED:v3] entry for agent={} (looked for prefix `{}`)",
                agent, target_prefix
            );
            CredError::NotFound(format!("agent bearer not found: {}", agent))
        })?;

    let hex_data = entry.content[target_prefix.len()..].trim();
    let ciphertext = hex::decode(hex_data).map_err(|e| {
        error!("hex decode failed for agent={}: {}", agent, e);
        CredError::Decryption("corrupt cred entry: hex decode failed".into())
    })?;

    let plaintext = decrypt(state.master_key.as_ref(), &ciphertext).map_err(|e| {
        error!("decrypt failed for agent={}: {}", agent, e);
        CredError::Decryption("corrupt cred entry: decrypt failed".into())
    })?;

    let value: serde_json::Value = serde_json::from_slice(&plaintext).map_err(|e| {
        error!("JSON parse failed for agent={}: {}", agent, e);
        CredError::InvalidInput("corrupt cred entry: JSON parse failed".into())
    })?;

    let bare_key = value
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("cred entry for agent={} has no `key` field", agent);
            CredError::InvalidInput("expected ApiKey-typed cred entry".into())
        })?
        .to_string();

    Ok(bare_key)
}

// ---------------------------------------------------------------------------
// ECDH bootstrap (Stage 2 of ECDH PIV port)
// ---------------------------------------------------------------------------

/// Magic byte string mixed into HKDF salt + AES-GCM AAD so an
/// ECDH-derived key from one protocol version cannot be replayed on
/// another. Bump the version suffix when the wire format changes.
const ECDH_PROTOCOL: &str = "ecdh-v1";
const ECDH_HKDF_SALT: &[u8] = b"credd-ecdh-v1";

#[derive(Deserialize)]
pub struct EcdhBearerRequest {
    /// Agent slot (e.g. "claude-code-alice-myhost"). Used for HKDF info field.
    pub agent: String,
    /// Client's ephemeral P-256 public key, hex-encoded SubjectPublicKeyInfo
    /// DER (the form `p256::PublicKey::to_public_key_der().as_bytes()`
    /// produces).
    pub ephemeral_pubkey: String,
    /// Client's ECDSA-over-SHA256 signature of `agent || ephemeral_pubkey`,
    /// hex-encoded raw r||s (64 bytes for P-256).
    pub signature: String,
    /// Protocol selector. Must be `"ecdh-v1"`.
    pub protocol: String,
}

#[derive(Serialize)]
pub struct EcdhBearerResponse {
    /// AES-256-GCM ciphertext of the bare bearer, hex-encoded.
    pub encrypted_bearer: String,
    /// 12-byte AES-GCM nonce, hex-encoded.
    pub nonce: String,
    pub expires_at: String,
    pub ttl_secs: u64,
    pub protocol: String,
}

/// POST /bootstrap/kleos-bearer with ECDH-v1 body. The 9A signature on
/// `agent || ephemeral_pubkey_hex` is the auth token. credd performs
/// ECDH between its 9D private key (on the YubiKey, via Python yubikit)
/// and the client's ephemeral public key, derives the bearer-encryption
/// key with HKDF-SHA256, AES-256-GCM-encrypts the bearer, and returns
/// ciphertext + nonce. The client computes the same shared secret
/// in software and decrypts.
pub async fn post_bootstrap_kleos_bearer_ecdh(
    State(state): State<AppState>,
    Json(req): Json<EcdhBearerRequest>,
) -> Result<Json<EcdhBearerResponse>, AppError> {
    if req.protocol != ECDH_PROTOCOL {
        return Err(CredError::InvalidInput(format!(
            "unsupported protocol `{}` (want `{}`)",
            req.protocol, ECDH_PROTOCOL
        ))
        .into());
    }

    // 9A pubkey must be loaded for signature verification.
    let v9a = state.piv_9a_pubkey.as_ref().ok_or_else(|| {
        warn!("ECDH bootstrap rejected: piv-9a-pubkey.pem not loaded");
        CredError::PermissionDenied("ECDH unavailable: PIV 9A pubkey not configured".into())
    })?;

    // Decode the wire signature (raw r||s, 64 bytes for P-256).
    let sig_bytes = hex::decode(&req.signature)
        .map_err(|e| CredError::InvalidInput(format!("signature hex: {}", e)))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| CredError::InvalidInput(format!("signature parse: {}", e)))?;

    // Reconstruct what the client signed: agent || ephemeral_pubkey hex.
    let signed_payload = format!("{}|{}", req.agent, req.ephemeral_pubkey);
    v9a.verify(signed_payload.as_bytes(), &signature)
        .map_err(|_| {
            warn!(agent = %req.agent, "ECDH bootstrap: 9A signature verify failed");
            CredError::PermissionDenied("invalid 9A signature".into())
        })?;

    // Reassemble peer public key as PEM for the Python ECDH subprocess.
    let peer_der = hex::decode(&req.ephemeral_pubkey)
        .map_err(|e| CredError::InvalidInput(format!("ephemeral_pubkey hex: {}", e)))?;
    let peer_pem = der_to_pem(&peer_der);

    // Run ECDH on the YubiKey.
    let shared = ecdh_agree(PivSlot::KeyManagement, &peer_pem).map_err(|e| {
        error!("ECDH agree failed: {}", e);
        CredError::YubiKey(format!("ECDH key agreement failed: {}", e))
    })?;

    // Resolve the bare bearer (same path as the GET handler uses).
    let bare_bearer = resolve_agent_bearer(&state, &req.agent).await?;

    // HKDF-SHA256 to derive a 32-byte AES-256 key bound to the agent slot.
    let hk = Hkdf::<Sha256>::new(Some(ECDH_HKDF_SALT), &shared);
    let mut bearer_key = [0u8; 32];
    hk.expand(req.agent.as_bytes(), &mut bearer_key)
        .map_err(|e| CredError::Encryption(format!("hkdf expand: {}", e)))?;

    // AES-256-GCM with a fresh random 12-byte nonce.
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&bearer_key));
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, bare_bearer.as_bytes())
        .map_err(|e| CredError::Encryption(format!("aes-gcm encrypt: {}", e)))?;

    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(DEFAULT_TTL_SECS as i64);
    Ok(Json(EcdhBearerResponse {
        encrypted_bearer: hex::encode(&ciphertext),
        nonce: hex::encode(nonce_bytes),
        expires_at: expires_at.to_rfc3339(),
        ttl_secs: DEFAULT_TTL_SECS,
        protocol: ECDH_PROTOCOL.to_string(),
    }))
}

/// Wrap raw SubjectPublicKeyInfo DER bytes into a PEM block so they can
/// be passed to Python yubikit's `load_pem_public_key`.
fn der_to_pem(der: &[u8]) -> String {
    let mut out = String::from("-----BEGIN PUBLIC KEY-----\n");
    let b64 = base64_encode(der);
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str("-----END PUBLIC KEY-----\n");
    out
}

/// Minimal base64 encoder so we do not pull a dep just for PEM wrap.
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(CHARS[(b0 >> 2) as usize] as char);
        out.push(CHARS[((b0 & 0x03) << 4 | b1 >> 4) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((b1 & 0x0f) << 2 | b2 >> 6) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
