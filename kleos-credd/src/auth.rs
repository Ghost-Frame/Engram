//! Two-tier authentication for credd.
//!
//! - Master key: full access to all secrets and operations
//! - Agent keys: scoped access based on permissions

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;

use hex;
use kleos_cred::agent_keys::{parse_agent_key, validate_agent_key, AgentKey};
use kleos_cred::crypto::hash_key;
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Pre-auth rate limit: 10 failed attempts per 60-second window.
const PREAUTH_LIMIT: u32 = 10;

/// Marker inserted by the Unix-socket listener middleware so downstream
/// middleware (rate limiter, auth) can identify connections that came over
/// the 0600 Unix socket. Such connections are inherently scoped to the
/// owning UID, so brute-force IP rate limiting is meaningless and skipped.
#[derive(Clone, Copy, Default, Debug)]
pub struct IsUnixSocket(pub bool);

/// Hash a socket address IP to an i64 key for the in-memory rate limiter.
fn ip_to_key(addr: &std::net::IpAddr) -> i64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    addr.hash(&mut hasher);
    hasher.finish() as i64
}

/// Pre-authentication rate-limiting middleware.
///
/// Uses the real TCP peer address (ConnectInfo) to prevent brute-force
/// token guessing. Runs BEFORE auth_middleware in the layer stack.
/// Skipped entirely for Unix-socket connections (0600 socket = single UID).
#[tracing::instrument(skip_all, fields(middleware = "credd.preauth_rate_limit"))]
pub async fn preauth_rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Skip rate limiting for health check
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // Skip for Unix-socket connections: filesystem ACL on 0600 socket is the
    // boundary, IP-based rate limiting cannot help.
    if request
        .extensions()
        .get::<IsUnixSocket>()
        .map(|m| m.0)
        .unwrap_or(false)
    {
        return next.run(request).await;
    }

    let key = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ip_to_key(&ci.0.ip()))
        .unwrap_or(0);

    match state.rate_limiter.check(key, PREAUTH_LIMIT) {
        Ok(_count) => next.run(request).await,
        Err(retry_after) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "rate limit exceeded",
                "retry_after": retry_after
            })),
        )
            .into_response(),
    }
}

/// Authentication result passed to handlers.
#[derive(Debug, Clone)]
pub enum AuthInfo {
    /// Master key authentication - full access.
    Master { user_id: i64 },
    /// DB-backed agent key authentication - scoped access via cred_agent_keys.
    /// Used by the three-tier resolve handlers.
    Agent { user_id: i64, key: AgentKey },
    /// File-backed bootstrap-agent token (~/.config/cred/agent-keys.json).
    /// Used only by the `/bootstrap/kleos-bearer` endpoint. Scopes are
    /// `bootstrap/<slot>` strings the handler matches against the request.
    BootstrapAgent { name: String, scopes: Vec<String> },
}

impl AuthInfo {
    pub fn user_id(&self) -> i64 {
        match self {
            Self::Master { user_id } => *user_id,
            Self::Agent { user_id, .. } => *user_id,
            // Bootstrap agents do not map to a Kleos user_id; they auth
            // credd itself, not Kleos. Use 0 as a sentinel for audit fields.
            Self::BootstrapAgent { .. } => 0,
        }
    }

    pub fn is_master(&self) -> bool {
        matches!(self, Self::Master { .. })
    }

    pub fn agent_name(&self) -> Option<&str> {
        match self {
            Self::Master { .. } => None,
            Self::Agent { key, .. } => Some(&key.name),
            Self::BootstrapAgent { name, .. } => Some(name.as_str()),
        }
    }

    pub fn can_access_category(&self, category: &str) -> bool {
        match self {
            Self::Master { .. } => true,
            Self::Agent { key, .. } => key.can_access(category),
            // Bootstrap agents have no DB-side category permissions; the
            // bootstrap-bearer handler does its own scope check.
            Self::BootstrapAgent { .. } => false,
        }
    }

    pub fn can_access_raw(&self) -> bool {
        match self {
            Self::Master { .. } => true,
            Self::Agent { key, .. } => key.can_access_raw(),
            Self::BootstrapAgent { .. } => false,
        }
    }

    /// True if any of the agent's scopes match `service/key` (exact, wildcard,
    /// or `*`). Only meaningful for `BootstrapAgent`; other variants return
    /// `false`. Master tier should be allowed by callers via `is_master()`
    /// before consulting this.
    pub fn has_bootstrap_scope(&self, service: &str, key: &str) -> bool {
        let scopes = match self {
            Self::BootstrapAgent { scopes, .. } => scopes,
            _ => return false,
        };
        let exact = format!("{}/{}", service, key);
        let wildcard = format!("{}/*", service);
        scopes
            .iter()
            .any(|s| s == &exact || s == &wildcard || s == "*")
    }
}

/// Extract bearer token from Authorization header.
fn extract_bearer_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Authentication middleware.
#[tracing::instrument(skip_all, fields(middleware = "credd.auth"))]
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth for health endpoint
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let token = extract_bearer_token(&request).ok_or(StatusCode::UNAUTHORIZED)?;

    // Check if it's the master key (try hex-decoded first, then raw)
    let master_hash = hash_key(state.master_key.as_ref());
    let token_bytes = hex::decode(token).unwrap_or_else(|_| token.as_bytes().to_vec());
    let token_hash = hash_key(&token_bytes);

    // SECURITY (SEC-INFO-10): constant-time comparison to prevent timing
    // oracle on the master key hash.
    let auth = if master_hash.len() == token_hash.len()
        && master_hash
            .as_bytes()
            .ct_eq(token_hash.as_bytes())
            .unwrap_u8()
            == 1
    {
        // Master key - assume user_id 1 (admin)
        AuthInfo::Master { user_id: 1 }
    } else if let Ok(key_bytes) = parse_agent_key(token) {
        // Try DB-backed agent key (used by three-tier resolve handlers).
        match validate_agent_key(&state.db, &key_bytes).await {
            Ok(agent_key) => AuthInfo::Agent {
                user_id: agent_key.user_id,
                key: agent_key,
            },
            Err(_) => check_bootstrap_agent(token, &state)?,
        }
    } else {
        // Token isn't valid hex for a DB-backed agent key, but the file-backed
        // bootstrap tokens are also 64-char hex so it could still match. If
        // not, that branch returns UNAUTHORIZED.
        check_bootstrap_agent(token, &state)?
    };

    request.extensions_mut().insert(auth);
    Ok(next.run(request).await)
}

/// Look the bearer up in the file-backed bootstrap-agent store. Returns
/// `BootstrapAgent` on hit, `UNAUTHORIZED` on miss.
fn check_bootstrap_agent(token: &str, state: &AppState) -> Result<AuthInfo, StatusCode> {
    let mut store = state
        .file_agent_keys
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let agent_id = store.validate(token).ok_or(StatusCode::UNAUTHORIZED)?;
    let scopes = store.scopes_for(&agent_id);
    store.touch(&agent_id);
    Ok(AuthInfo::BootstrapAgent {
        name: agent_id,
        scopes,
    })
}

/// Extractor for authentication info.
#[derive(Clone)]
pub struct Auth(pub AuthInfo);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Auth {
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<AuthInfo>()
            .cloned()
            .map(Auth)
            .ok_or(StatusCode::UNAUTHORIZED);
        std::future::ready(result)
    }
}
