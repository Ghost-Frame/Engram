//! Integration tests for the auth middleware.
//!
//! Tests all four auth paths end-to-end: Bearer, Soft (Ed25519 signature),
//! Session (cached from prior signed auth), and PIV (YubiKey P256). The PIV
//! test is gated behind `cfg(feature = "piv")` + `KLEOS_TEST_PIV=1` so it
//! only runs on machines with a YubiKey inserted.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{body_json, bootstrap_admin_key, send, test_app};
use kleos_lib::auth_piv::RequestSigner;
use serde_json::json;
use tower::ServiceExt;

/// Enroll an Ed25519 soft key as the first identity key (bootstrap path).
///
/// Returns the `RequestSigner` for signing subsequent requests. Requires no
/// existing keys in the database and no `KLEOS_BOOTSTRAP_SECRET` env var.
async fn enroll_soft_key(app: &axum::Router) -> RequestSigner {
    let signer = RequestSigner::from_key_bytes([42u8; 32], "test-host", "test-agent", "test-model");
    let sig_hex = signer
        .sign_enrollment_proof()
        .expect("sign enrollment proof");

    // The middleware EnrollProof struct uses deny_unknown_fields, so only
    // send the five fields it expects.
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
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, resp_body) = send(app, request).await;
    assert!(
        status.is_success(),
        "enrollment failed: {status}: {resp_body}"
    );
    signer
}

/// Build a signed axum `Request` using `RequestSigner`.
///
/// Sets all `X-Kleos-*` signature headers required by the auth middleware
/// Path 2 handler. An optional JSON body may be supplied; pass `b""` for
/// GET requests.
fn signed_request(
    signer: &RequestSigner,
    method: &str,
    path: &str,
    body_bytes: &[u8],
) -> Request<Body> {
    let signed = signer
        .sign_request(method, path, "", body_bytes)
        .expect("sign request");
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

// ---- Bearer auth tests ----

/// Bearer token auth succeeds and allows access to protected routes.
#[tokio::test]
async fn bearer_auth_succeeds() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    let request = Request::builder()
        .method("GET")
        .uri("/keys")
        .header("Authorization", format!("Bearer {}", admin_key))
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert!(
        status.is_success(),
        "bearer auth should succeed for /keys: {status}"
    );
}

/// Invalid bearer token returns 401.
#[tokio::test]
async fn bearer_auth_invalid_token_returns_401() {
    let (app, _state) = test_app().await;
    let _ = bootstrap_admin_key(&app).await;

    let request = Request::builder()
        .method("GET")
        .uri("/keys")
        .header("Authorization", "Bearer totally-bogus-key")
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// No auth at all returns 401 on protected routes.
#[tokio::test]
async fn no_auth_returns_401() {
    let (app, _state) = test_app().await;
    let _ = bootstrap_admin_key(&app).await;

    let request = Request::builder()
        .method("GET")
        .uri("/keys")
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---- Soft key (Ed25519 signature) auth tests ----

/// Ed25519 soft key enrollment followed by a signed request succeeds.
///
/// The middleware must issue a session token in `x-kleos-session-issued`
/// on the first signed request (after auto-registering the identity row).
#[tokio::test]
async fn soft_key_signed_request_succeeds_and_issues_session() {
    let (app, _state) = test_app().await;
    // Do NOT bootstrap an admin key first -- enrollment requires zero existing keys.
    let signer = enroll_soft_key(&app).await;

    // Signed request to /stats (bootstrap identity gets admin scopes).
    let request = signed_request(&signer, "GET", "/stats", b"");
    let res = app.clone().oneshot(request).await.expect("oneshot request");
    let status = res.status();
    let session_header = res.headers().get("x-kleos-session-issued").cloned();
    let _body = body_json(res).await;

    assert!(
        status.is_success(),
        "signed request should succeed: {status}"
    );
    assert!(
        session_header.is_some(),
        "signed auth must issue a session token in x-kleos-session-issued"
    );
}

/// A signed request with a tampered signature is rejected with 401.
#[tokio::test]
async fn soft_key_bad_signature_returns_401() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Build a valid signed request, then swap in a bogus signature.
    let signed = signer.sign_request("GET", "/stats", "", b"").expect("sign");
    let bad_sig = "00".repeat(32); // 64 hex chars, wrong value
    let request = Request::builder()
        .method("GET")
        .uri("/stats")
        .header("X-Kleos-Sig", &bad_sig)
        .header("X-Kleos-Algo", signed.algo.as_str())
        .header("X-Kleos-Identity", &signed.identity_hash)
        .header("X-Kleos-Ts", signed.ts_ms.to_string())
        .header("X-Kleos-Nonce", &signed.nonce)
        .header("X-Kleos-Key-Fp", &signed.key_fp)
        .header("X-Kleos-Host", &signed.host_label)
        .header("X-Kleos-Agent", &signed.agent_label)
        .header("X-Kleos-Model", &signed.model_label)
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "bad signature should be rejected"
    );
}

// ---- Session auth tests ----

/// A session token received from a signed request can be reused directly.
///
/// This verifies that `X-Kleos-Session` (Path 1 in the auth middleware) works
/// end-to-end: sign once to get a token, then use that token without re-signing.
#[tokio::test]
async fn session_auth_from_signed_request_succeeds() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // First request: signed, gets a session token back.
    let request = signed_request(&signer, "GET", "/stats", b"");
    let res = app.clone().oneshot(request).await.expect("oneshot request");
    assert!(res.status().is_success());
    let session_token = res
        .headers()
        .get("x-kleos-session-issued")
        .expect("should issue session token")
        .to_str()
        .expect("valid utf8")
        .to_string();

    // Second request: session token only, no signature headers.
    let request2 = Request::builder()
        .method("GET")
        .uri("/stats")
        .header("X-Kleos-Session", &session_token)
        .body(Body::empty())
        .unwrap();
    let (status2, _body2) = send(&app, request2).await;
    assert!(
        status2.is_success(),
        "session auth should succeed: {status2}"
    );
}

/// Sliding window: an active session-token request gets a refreshed token
/// back via `x-kleos-session-issued`, and the old token is invalidated.
/// This is what keeps long-running agents from hitting the 15-minute TTL
/// boundary every quarter hour.
#[tokio::test]
async fn session_auth_refreshes_token_on_successful_request() {
    let (app, _state) = test_app().await;
    let signer = enroll_soft_key(&app).await;

    // Sign once to bootstrap a session token.
    let request = signed_request(&signer, "GET", "/stats", b"");
    let res = app.clone().oneshot(request).await.expect("oneshot request");
    assert!(res.status().is_success());
    let initial_token = res
        .headers()
        .get("x-kleos-session-issued")
        .expect("signed call must issue session token")
        .to_str()
        .expect("valid utf8")
        .to_string();

    // Call again with the session token. Server should refresh and emit
    // a new token in `x-kleos-session-issued`.
    let request2 = Request::builder()
        .method("GET")
        .uri("/stats")
        .header("X-Kleos-Session", &initial_token)
        .body(Body::empty())
        .unwrap();
    let res2 = app
        .clone()
        .oneshot(request2)
        .await
        .expect("oneshot session request");
    assert!(res2.status().is_success(), "session call should succeed");
    let refreshed_token = res2
        .headers()
        .get("x-kleos-session-issued")
        .expect("session call must refresh session token")
        .to_str()
        .expect("valid utf8")
        .to_string();
    assert_ne!(
        initial_token, refreshed_token,
        "refresh must mint a distinct token"
    );

    // The refreshed token works.
    let request3 = Request::builder()
        .method("GET")
        .uri("/stats")
        .header("X-Kleos-Session", &refreshed_token)
        .body(Body::empty())
        .unwrap();
    let (status3, _) = send(&app, request3).await;
    assert!(
        status3.is_success(),
        "refreshed token must verify: {status3}"
    );

    // The original token is still valid until its own expires_at fires --
    // refresh does not pre-emptively invalidate it, so concurrent in-flight
    // requests with the cached token cannot race into a 401.
    let request4 = Request::builder()
        .method("GET")
        .uri("/stats")
        .header("X-Kleos-Session", &initial_token)
        .body(Body::empty())
        .unwrap();
    let (status4, _) = send(&app, request4).await;
    assert!(
        status4.is_success(),
        "old token must remain valid until natural expiry: {status4}"
    );
}

/// An invalid (fabricated) session token returns 401.
#[tokio::test]
async fn session_auth_invalid_token_returns_401() {
    let (app, _state) = test_app().await;
    let _ = bootstrap_admin_key(&app).await;

    let request = Request::builder()
        .method("GET")
        .uri("/keys")
        .header("X-Kleos-Session", "totally-bogus-session-token")
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "bogus session should be rejected"
    );
}

// ---- Enrollment (Path 4) tests ----

/// Enrollment with valid proof-of-possession succeeds when no keys exist (bootstrap).
#[tokio::test]
async fn enrollment_bootstrap_succeeds_with_no_existing_keys() {
    let (app, _state) = test_app().await;
    let signer = RequestSigner::from_key_bytes([99u8; 32], "ci-host", "ci-agent", "ci-model");
    let sig_hex = signer.sign_enrollment_proof().expect("sign enrollment");

    let body = json!({
        "algo": "ed25519",
        "tier": "soft",
        "pubkey_pem": signer.pubkey_pem(),
        "host_label": "ci-host",
        "sig_hex": sig_hex,
    });

    let request = Request::builder()
        .method("POST")
        .uri("/identity-keys/enroll")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, resp_body) = send(&app, request).await;
    assert!(
        status.is_success(),
        "enrollment should succeed: {status}: {resp_body}"
    );
}

/// A second bootstrap enrollment attempt is rejected when keys already exist.
///
/// The middleware returns 401 and the enrollment handler is never reached.
#[tokio::test]
async fn enrollment_rejected_when_keys_exist() {
    let (app, _state) = test_app().await;
    // Enroll the first key via the bootstrap path.
    let _signer1 = enroll_soft_key(&app).await;

    // Attempt to enroll a second key via the same unauthenticated bootstrap path.
    let signer2 =
        RequestSigner::from_key_bytes([77u8; 32], "other-host", "other-agent", "other-model");
    let sig_hex = signer2.sign_enrollment_proof().expect("sign enrollment");

    let body = json!({
        "algo": "ed25519",
        "tier": "soft",
        "pubkey_pem": signer2.pubkey_pem(),
        "host_label": "other-host",
        "sig_hex": sig_hex,
    });

    let request = Request::builder()
        .method("POST")
        .uri("/identity-keys/enroll")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "second enrollment via bootstrap should be rejected"
    );
}

// ---- PIV (YubiKey) auth tests ----

/// End-to-end PIV auth: enroll a YubiKey P256 key, sign a request with
/// hardware, verify the tier resolves to "piv" in the identities table.
///
/// Requires a YubiKey with a cert in slot 9A and `PIV_PIN` set. Gated
/// behind `KLEOS_TEST_PIV=1` so it skips silently in CI or on machines
/// without hardware.
#[cfg(feature = "piv")]
#[tokio::test]
async fn piv_yubikey_auth_end_to_end() {
    if std::env::var("KLEOS_TEST_PIV").as_deref() != Ok("1") {
        eprintln!("skipping PIV test: set KLEOS_TEST_PIV=1 + PIV_PIN with a YubiKey inserted");
        return;
    }
    if std::env::var("PIV_PIN").is_err() {
        eprintln!("skipping PIV test: PIV_PIN must be set for YubiKey signing");
        return;
    }

    let (app, _state) = test_app().await;

    // Create a PIV signer from the inserted YubiKey's slot 9A certificate.
    let signer = RequestSigner::from_yubikey("test-piv-host", "test-piv-agent", "test-piv-model")
        .expect("YubiKey must be accessible with a cert in slot 9A");
    assert_eq!(signer.tier(), "piv", "YubiKey signer must report piv tier");
    assert_eq!(
        signer.algo().as_str(),
        "ecdsa-p256",
        "YubiKey slot 9A uses P256"
    );

    // Enroll the PIV key (bootstrap -- no existing keys).
    let sig_hex = signer
        .sign_enrollment_proof()
        .expect("PIV enrollment proof signing");
    let enroll_body = json!({
        "algo": "ecdsa-p256",
        "tier": "piv",
        "pubkey_pem": signer.pubkey_pem(),
        "host_label": "test-piv-host",
        "sig_hex": sig_hex,
    });
    let enroll_req = Request::builder()
        .method("POST")
        .uri("/identity-keys/enroll")
        .header("Content-Type", "application/json")
        .body(Body::from(enroll_body.to_string()))
        .unwrap();
    let (enroll_status, enroll_resp) = send(&app, enroll_req).await;
    assert!(
        enroll_status.is_success(),
        "PIV enrollment should succeed: {enroll_status}: {enroll_resp}"
    );

    // Make a signed request with the YubiKey -- this exercises the full
    // P256 signature path through the middleware.
    let signed = signer
        .sign_request("GET", "/identities", "", b"")
        .expect("PIV request signing");
    let request = Request::builder()
        .method("GET")
        .uri("/identities")
        .header("X-Kleos-Sig", &signed.sig_hex)
        .header("X-Kleos-Algo", signed.algo.as_str())
        .header("X-Kleos-Identity", &signed.identity_hash)
        .header("X-Kleos-Ts", signed.ts_ms.to_string())
        .header("X-Kleos-Nonce", &signed.nonce)
        .header("X-Kleos-Key-Fp", &signed.key_fp)
        .header("X-Kleos-Host", &signed.host_label)
        .header("X-Kleos-Agent", &signed.agent_label)
        .header("X-Kleos-Model", &signed.model_label)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(request).await.expect("oneshot");
    let status = res.status();
    let session_header = res.headers().get("x-kleos-session-issued").cloned();
    let body = body_json(res).await;

    assert!(
        status.is_success(),
        "PIV signed request should succeed: {status}: {body}"
    );
    assert!(
        session_header.is_some(),
        "PIV signed auth must issue a session token"
    );

    // Verify the auto-registered identity has tier = "piv".
    let identities = body["identities"]
        .as_array()
        .expect("identities array in response");
    let piv_identity = identities
        .iter()
        .find(|i| i["host_label"] == "test-piv-host")
        .expect("identity with test-piv-host should exist");
    assert_eq!(
        piv_identity["tier"], "piv",
        "identity tier must be 'piv' for YubiKey-enrolled key"
    );
    assert_eq!(piv_identity["algo"], "ecdsa-p256");
}
