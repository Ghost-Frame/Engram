//! Integration tests for MCP direct-auth token (kleos. prefix bearer).
//!
//! Tests the full auth middleware path: token verification, revocation,
//! scope enforcement, backward compatibility with existing API key bearer.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{bootstrap_admin_key, send, test_app};
use ed25519_dalek::SigningKey;
use kleos_lib::auth_piv::RequestSigner;
use kleos_lib::mcp_token;
use serde_json::json;

/// Enroll an Ed25519 soft key and return the signer.
async fn enroll_soft_key(app: &axum::Router) -> RequestSigner {
    let signer = RequestSigner::from_key_bytes([42u8; 32], "test-host", "test-agent", "test-model");
    let sig_hex = signer
        .sign_enrollment_proof()
        .expect("sign enrollment proof");

    let body = json!({
        "algo": "ed25519",
        "tier": "soft",
        "pubkey_pem": signer.pubkey_pem(),
        "host_label": "test-host",
        "sig_hex": sig_hex,
    });

    let request = Request::builder()
        .method("POST")
        .uri("/identity-keys/enroll")
        .header("Content-Type", "application/json")
        .header("X-Bootstrap-Secret", "test-bootstrap-secret")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, resp) = send(app, request).await;
    assert!(status.is_success(), "enrollment failed: {status}: {resp}");
    signer
}

/// Build a signed request using the RequestSigner (for PIV-envelope auth).
fn signed_request(
    signer: &RequestSigner,
    method: &str,
    path: &str,
    body_bytes: &[u8],
) -> Request<Body> {
    let signed = signer
        .sign_request(method, path, "", body_bytes)
        .expect("sign");
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("X-Kleos-Sig", &signed.sig_hex)
        .header("X-Kleos-Algo", signed.algo.as_str())
        .header("X-Kleos-Identity", &signed.identity_hash)
        .header("X-Kleos-Ts", signed.ts_ms.to_string())
        .header("X-Kleos-Nonce", &signed.nonce)
        .header("X-Kleos-Key-Fp", &signed.key_fp)
        .header("X-Kleos-Host", &signed.host_label)
        .header("X-Kleos-Agent", &signed.agent_label)
        .header("X-Kleos-Model", &signed.model_label);
    if !body_bytes.is_empty() {
        builder = builder.header("Content-Type", "application/json");
    }
    builder.body(Body::from(body_bytes.to_vec())).unwrap()
}

/// Mint an MCP token using the raw Ed25519 key (matching the enrolled signer).
/// Uses the signer's actual fingerprint as kid for DB lookup compatibility.
fn mint_token(
    signer: &RequestSigner,
    scopes: &str,
    ttl: u64,
) -> (String, mcp_token::McpTokenPayload) {
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let kid = signer.fingerprint().to_string();
    mcp_token::mint(
        &sk,
        &kid,
        1,
        None,
        scopes,
        ttl,
        mcp_token::DEFAULT_MAX_TTL_SECS,
    )
    .unwrap()
}

/// Register a minted token via POST /mcp-tokens (PIV-envelope auth).
async fn register_token(
    app: &axum::Router,
    signer: &RequestSigner,
    token: &str,
    name: &str,
    scopes: &str,
    ttl: u64,
) -> (StatusCode, serde_json::Value) {
    let body = json!({
        "token": token,
        "name": name,
        "scopes": scopes,
        "ttl_secs": ttl,
    });
    let body_bytes = body.to_string().into_bytes();
    let req = signed_request(signer, "POST", "/mcp-tokens", &body_bytes);
    send(app, req).await
}

// --- Tests ---

/// Full flow: enroll key -> mint token -> register -> use as bearer.
#[tokio::test]
async fn mcp_token_full_flow() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Mint and register a token.
    let (token, _payload) = mint_token(&signer, "read,write", 3600);
    let (status, body) =
        register_token(&app, &signer, &token, "test-token", "read,write", 3600).await;
    assert_eq!(status, StatusCode::CREATED, "register failed: {body}");

    // Use the token as a Bearer to access a protected route.
    let req = Request::builder()
        .method("GET")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert!(
        status.is_success(),
        "mcp token auth should succeed on protected route"
    );
}

/// A token whose `payload.uid` disagrees with the enrolled key owner is
/// accepted AS the key owner: the verified signing identity is the sole
/// authority, not the client-supplied uid. This is what lets a keyless minter
/// (the SO_PEERCRED broker, kleos-cli) mint without knowing its server-side
/// user id, so the flow works for any user on a multi-user/sharded instance.
#[tokio::test]
async fn mcp_token_uid_mismatch_binds_to_key_owner() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Mint with a bogus uid that does NOT match the enrolled key owner (1).
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let kid = signer.fingerprint().to_string();
    let (token, _payload) = mcp_token::mint(
        &sk,
        &kid,
        99_999,
        None,
        "read,write",
        3600,
        mcp_token::DEFAULT_MAX_TTL_SECS,
    )
    .unwrap();

    // Registration accepts the mismatch and stamps the real key owner.
    let (status, body) =
        register_token(&app, &signer, &token, "mismatch-uid", "read,write", 3600).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "registration must accept a uid mismatch (key owner is authority): {body}"
    );

    // The bearer authenticates on a protected route, acting as the key owner.
    let req = Request::builder()
        .method("GET")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert!(
        status.is_success(),
        "a token with a mismatched uid must authenticate as the verified key owner"
    );
}

/// A revoked token must be rejected with 401.
#[tokio::test]
async fn mcp_token_revoked_returns_401() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    let (token, payload) = mint_token(&signer, "read", 3600);
    let (status, _) = register_token(&app, &signer, &token, "revoke-me", "read", 3600).await;
    assert_eq!(status, StatusCode::CREATED);

    // Revoke via DELETE /mcp-tokens/:jti (signed request).
    let req = signed_request(
        &signer,
        "DELETE",
        &format!("/mcp-tokens/{}", payload.jti),
        b"",
    );
    let (status, body) = send(&app, req).await;
    assert!(status.is_success(), "revoke should succeed: {body}");

    // Token should now be rejected on a protected route.
    let req = Request::builder()
        .method("GET")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked token should be rejected"
    );
}

/// An expired token must be rejected with 401 (expiry check before DB hit).
#[tokio::test]
async fn mcp_token_expired_returns_401() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Mint with 0 TTL (already expired).
    let (token, _) = mint_token(&signer, "read", 0);
    // Register may succeed (server stores it), but auth should fail.
    let _ = register_token(&app, &signer, &token, "expired", "read", 0).await;

    let req = Request::builder()
        .method("GET")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "expired token should be rejected"
    );
}

/// A minted-but-not-registered token must be rejected (revocation table miss).
#[tokio::test]
async fn mcp_token_unregistered_returns_401() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Mint but don't register.
    let (token, _) = mint_token(&signer, "read", 3600);

    let req = Request::builder()
        .method("GET")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "unregistered token should be rejected"
    );
}

/// Existing API key bearer auth must still work (backward compat).
#[tokio::test]
async fn existing_api_key_still_works() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    let req = Request::builder()
        .method("GET")
        .uri("/keys")
        .header("Authorization", format!("Bearer {}", admin_key))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert!(
        status.is_success(),
        "existing API key bearer should still work"
    );
}

/// Bearer auth (not PIV envelope) cannot register MCP tokens.
#[tokio::test]
async fn api_key_bearer_cannot_register_mcp_token() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    let body = json!({
        "token": "kleos.fake.fake",
        "name": "should-fail",
        "scopes": "read",
        "ttl_secs": 3600,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/mcp-tokens")
        .header("Authorization", format!("Bearer {}", admin_key))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _body) = send(&app, req).await;
    // Should fail because auth_ctx.identity is None for bearer auth.
    assert!(
        status.as_u16() >= 400,
        "bearer auth should not be able to register MCP tokens: {status}"
    );
}

/// Wildcard scope must be rejected at mint time.
#[tokio::test]
async fn wildcard_scope_rejected() {
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let result = mcp_token::mint(
        &sk,
        "test-kid",
        1,
        None,
        "*",
        3600,
        mcp_token::DEFAULT_MAX_TTL_SECS,
    );
    assert!(result.is_err(), "mint should reject wildcard scope");
}

/// List and info routes return correct data.
#[tokio::test]
async fn list_and_info_routes() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    let (token, payload) = mint_token(&signer, "read,write", 3600);
    let (status, _) = register_token(&app, &signer, &token, "list-test", "read,write", 3600).await;
    assert_eq!(status, StatusCode::CREATED);

    // List tokens.
    let req = signed_request(&signer, "GET", "/mcp-tokens", b"");
    let (status, body) = send(&app, req).await;
    assert!(status.is_success(), "list should succeed: {body}");
    assert_eq!(body["count"].as_u64(), Some(1));

    // Get single token info.
    let req = signed_request(&signer, "GET", &format!("/mcp-tokens/{}", payload.jti), b"");
    let (status, body) = send(&app, req).await;
    assert!(status.is_success(), "info should succeed: {body}");
    assert_eq!(body["name"], "list-test");
}

/// Revoke-all revokes all tokens for the user.
#[tokio::test]
async fn revoke_all_tokens() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Register two tokens.
    let (t1, _) = mint_token(&signer, "read", 3600);
    let (t2, _) = mint_token(&signer, "read,write", 3600);
    register_token(&app, &signer, &t1, "tok-1", "read", 3600).await;
    register_token(&app, &signer, &t2, "tok-2", "read,write", 3600).await;

    // Revoke all.
    let req = signed_request(&signer, "DELETE", "/mcp-tokens", b"");
    let (status, body) = send(&app, req).await;
    assert!(status.is_success(), "revoke-all should succeed: {body}");
    assert_eq!(body["revoked_count"].as_u64(), Some(2));

    // Both tokens should be rejected on a protected route.
    for t in [&t1, &t2] {
        let req = Request::builder()
            .method("GET")
            .uri("/mcp-tokens")
            .header("Authorization", format!("Bearer {}", t))
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
