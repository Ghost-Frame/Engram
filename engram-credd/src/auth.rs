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

use engram_cred::agent_keys::{parse_agent_key, validate_agent_key, AgentKey};
use engram_cred::crypto::hash_key;
use hex;
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Pre-auth rate limit: 10 failed attempts per 60-second window.
const PREAUTH_LIMIT: u32 = 10;

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
    /// Agent key authentication - scoped access.
    Agent { user_id: i64, key: AgentKey },
}

impl AuthInfo {
    pub fn user_id(&self) -> i64 {
        match self {
            Self::Master { user_id } => *user_id,
            Self::Agent { user_id, .. } => *user_id,
        }
    }

    pub fn is_master(&self) -> bool {
        matches!(self, Self::Master { .. })
    }

    pub fn agent_name(&self) -> Option<&str> {
        match self {
            Self::Master { .. } => None,
            Self::Agent { key, .. } => Some(&key.name),
        }
    }

    pub fn can_access_category(&self, category: &str) -> bool {
        match self {
            Self::Master { .. } => true,
            Self::Agent { key, .. } => key.can_access(category),
        }
    }

    pub fn can_access_raw(&self) -> bool {
        match self {
            Self::Master { .. } => true,
            Self::Agent { key, .. } => key.can_access_raw(),
        }
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
    } else {
        // Try as agent key
        let key_bytes = parse_agent_key(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
        let agent_key = validate_agent_key(&state.db, &key_bytes)
            .await
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        AuthInfo::Agent {
            user_id: agent_key.user_id,
            key: agent_key,
        }
    };

    request.extensions_mut().insert(auth);
    Ok(next.run(request).await)
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
