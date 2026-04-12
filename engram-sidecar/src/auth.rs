//! Sidecar shared-secret authentication.
//!
//! The sidecar holds a tenant-scoped `Database` handle. Without auth any
//! process on the network can read or write that tenant's memories -- this
//! is the single highest-blast-radius finding in the round-6 audit.
//! Middleware below enforces `Authorization: Bearer <token>` on every
//! non-health route using a constant-time compare.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::SidecarState;

const OPEN_PATHS: &[&str] = &["/health"];

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

pub async fn require_token(
    State(state): State<SidecarState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if OPEN_PATHS.contains(&path) {
        return next.run(request).await;
    }

    let Some(expected) = state.token.as_deref() else {
        return unauthorized();
    };

    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if presented.is_empty() {
        return unauthorized();
    }

    let presented_bytes = presented.as_bytes();
    let expected_bytes = expected.as_bytes();
    if presented_bytes.len() != expected_bytes.len()
        || presented_bytes.ct_eq(expected_bytes).unwrap_u8() != 1
    {
        return unauthorized();
    }

    next.run(request).await
}

/// Generate a fresh 32-byte token encoded as lowercase hex.
#[allow(dead_code)]
pub fn generate_token() -> String {
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let mut out = String::with_capacity(64);
    for byte in raw {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_is_64_hex_chars() {
        let tok = generate_token();
        assert_eq!(tok.len(), 64);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_tokens_differ() {
        assert_ne!(generate_token(), generate_token());
    }
}
