//! Mint short-lived, identity-signed Kleos bearers for local agents.
//!
//! `POST /phylax/kleos/token` is reachable only over the credd Unix socket and
//! only by a process running as the same effective user as the daemon
//! (kernel-verified SO_PEERCRED). It mints an `mcp_token` bearer with the host's
//! soft Ed25519 identity, registers the token with the Kleos server (required --
//! the server rejects bearers whose `jti` is not registered), then returns it.
//! No static long-lived Kleos API key needs to live on disk.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use kleos_credd::auth::IsUnixSocket;
use kleos_credd::peercred::PeerIdentity;
use kleos_lib::auth_piv::RequestSigner;
use kleos_lib::mcp_token::{self, McpTokenError};

use crate::state::PhylaxState;

/// Socket-minted tokens are hard-capped at read,write; admin is never issued here.
const MINT_SCOPE_CAP: &str = "read,write";

/// Default token lifetime when `CREDD_KLEOS_TOKEN_TTL_SECS` is unset (5 minutes).
const DEFAULT_TTL_SECS: u64 = 300;

/// Request body for `POST /phylax/kleos/token`. Scopes default to read,write.
#[derive(Debug, Default, Deserialize)]
pub struct TokenRequest {
    /// Requested scopes (CSV). Capped at read,write; omit for the default.
    pub scopes: Option<String>,
}

/// Response body: the bearer plus its lifetime in seconds.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// The `kleos.` bearer string to present as `Authorization: Bearer <token>`.
    pub token: String,
    /// Seconds until the token expires.
    pub expires_in: u64,
}

/// Mint a Kleos mcp_token bearer signed by `signing_key`. `kid` is the minting
/// key fingerprint, `uid` the Kleos user id. Enforces the read,write cap and a
/// fixed TTL (no renewal window beyond ttl). Returns the bearer string only.
pub fn mint_token_with_key(
    signing_key: &SigningKey,
    kid: &str,
    uid: i64,
    ttl_secs: u64,
    scopes: &str,
) -> Result<String, McpTokenError> {
    let requested = mcp_token::parse_scopes_strict(scopes)?;
    let cap = mcp_token::parse_scopes_strict(MINT_SCOPE_CAP)?;
    mcp_token::scopes_within_cap(&requested, &cap)?;
    let (token, _payload) =
        mcp_token::mint(signing_key, kid, uid, None, scopes, ttl_secs, ttl_secs)?;
    Ok(token)
}

/// Pure local-access gate: Unix-socket-only plus kernel-verified same-owner UID.
///
/// Returns the authorized peer, or the HTTP status to reject with. Kept free of
/// application state so the security decision is unit-testable in isolation.
fn authorize_local(is_unix: bool, peer: Option<PeerIdentity>) -> Result<PeerIdentity, StatusCode> {
    // Never serve this endpoint over TCP -- it must stay socket-local.
    if !is_unix {
        return Err(StatusCode::FORBIDDEN);
    }
    // The kernel must have vouched for a peer, and it must be our own user.
    let peer = peer.ok_or(StatusCode::FORBIDDEN)?;
    if !peer.is_local_owner() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(peer)
}

/// Resolve the Kleos server base URL the same way credd's vault fallback does.
fn kleos_base_url() -> String {
    std::env::var("KLEOS_URL")
        .or_else(|_| std::env::var("ENGRAM_URL"))
        .unwrap_or_else(|_| "http://localhost:4200".into())
}

/// Register a freshly minted token with the Kleos server so its `jti` is
/// accepted by the bearer-validation path (the server rejects unregistered
/// tokens). Signs `POST /mcp-tokens` with the same soft identity that minted the
/// token, mirroring the credd vault-fallback signing idiom. The signed canonical
/// envelope binds the exact body bytes, so we sign and send the identical buffer.
async fn register_with_kleos(
    signer: &RequestSigner,
    token: &str,
    scopes: &str,
    ttl_secs: u64,
) -> Result<(), StatusCode> {
    let body = serde_json::json!({
        "token": token,
        "name": "phylax-broker",
        "scopes": scopes,
        "ttl_secs": ttl_secs,
    });
    let body_bytes = serde_json::to_vec(&body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let url = format!("{}/mcp-tokens", kleos_base_url().trim_end_matches('/'));
    let http = reqwest::Client::new();
    let req = http
        .post(&url)
        .header("content-type", "application/json")
        .body(body_bytes.clone());

    // Sign exactly the bytes we send -- the envelope covers method/path/body.
    let signed = signer
        .sign_request("POST", "/mcp-tokens", "", &body_bytes)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to sign /mcp-tokens registration");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let req = signed.apply_headers(req);

    let resp = req.send().await.map_err(|e| {
        tracing::error!(error = %e, "Kleos /mcp-tokens registration unreachable");
        StatusCode::BAD_GATEWAY
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        tracing::error!(%status, detail, "Kleos rejected token registration");
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(())
}

/// `POST /phylax/kleos/token` -- mint and register a short-lived Kleos bearer.
///
/// Authenticated purely by SO_PEERCRED over the Unix socket: no bearer is
/// required to call this (it exists to hand one out). The bearer is signed by
/// the host's enrolled soft Ed25519 identity and capped at read,write.
pub async fn mint_kleos_token(
    State(state): State<PhylaxState>,
    unix: Option<Extension<IsUnixSocket>>,
    peer: Option<Extension<PeerIdentity>>,
    body: Option<Json<TokenRequest>>,
) -> Result<Json<TokenResponse>, StatusCode> {
    // 1 + 2. Transport + identity gate (Unix socket only, same-owner UID).
    let is_unix = matches!(unix, Some(Extension(IsUnixSocket(true))));
    let peer = authorize_local(is_unix, peer.map(|Extension(p)| p))?;

    // 3. Require a soft Ed25519 signer; PIV keys cannot export a secret to mint.
    let signer = state
        .kleos_signer
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let sk = signer
        .soft_signing_key()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let kid = signer.fingerprint().to_string();

    // 4. Resolve TTL + scopes (scopes are capped read,write inside the helper).
    let ttl_secs: u64 = std::env::var("CREDD_KLEOS_TOKEN_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    let scopes = body
        .and_then(|Json(b)| b.scopes)
        .unwrap_or_else(|| MINT_SCOPE_CAP.to_string());

    // 5. Mint locally with the soft key. The server derives ownership from the
    //    verified signing identity and ignores this uid, so any value works
    //    (this is what makes the broker user-agnostic on multi-user instances).
    let token =
        mint_token_with_key(sk, &kid, 1, ttl_secs, &scopes).map_err(|_| StatusCode::BAD_REQUEST)?;

    // 6. Register so the server accepts the bearer (rejects unregistered jtis).
    register_with_kleos(signer.as_ref(), &token, &scopes, ttl_secs).await?;

    tracing::info!(
        uid = peer.uid,
        pid = peer.pid,
        scopes = %scopes,
        "minted Kleos bearer via SO_PEERCRED broker"
    );
    Ok(Json(TokenResponse {
        token,
        expires_in: ttl_secs,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TCP-borne request (no Unix marker) must always be refused.
    #[test]
    fn tcp_request_is_forbidden() {
        let peer = Some(PeerIdentity {
            uid: nix::unistd::geteuid().as_raw(),
            pid: 1,
        });
        assert_eq!(authorize_local(false, peer), Err(StatusCode::FORBIDDEN));
    }

    /// A Unix request with no captured peer credentials must be refused.
    #[test]
    fn unix_without_peer_is_forbidden() {
        assert_eq!(authorize_local(true, None), Err(StatusCode::FORBIDDEN));
    }

    /// A Unix request from a different UID must be refused.
    #[test]
    fn unix_other_uid_is_forbidden() {
        let other = PeerIdentity {
            uid: u32::MAX,
            pid: 1,
        };
        assert_eq!(
            authorize_local(true, Some(other)),
            Err(StatusCode::FORBIDDEN)
        );
    }

    /// A Unix request from our own UID is authorized.
    #[test]
    fn unix_same_uid_is_authorized() {
        let me = PeerIdentity {
            uid: nix::unistd::geteuid().as_raw(),
            pid: 42,
        };
        assert_eq!(authorize_local(true, Some(me)), Ok(me));
    }
}
