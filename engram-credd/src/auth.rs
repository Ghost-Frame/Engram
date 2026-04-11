//! Two-tier authentication for credd.
//!
//! - Master key: full access to all secrets and operations
//! - Agent keys: scoped access based on permissions

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use engram_cred::agent_keys::{parse_agent_key, validate_agent_key, AgentKey};
use engram_cred::crypto::hash_key;

use crate::state::AppState;

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
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer_token(&request).ok_or(StatusCode::UNAUTHORIZED)?;

    // Check if it's the master key (hash comparison)
    let master_hash = hash_key(state.master_key.as_ref());
    let token_hash = hash_key(token.as_bytes());

    let auth = if master_hash == token_hash {
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
