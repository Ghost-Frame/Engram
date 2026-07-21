//! GUI cookie authentication for read-only requests.
//!
//! An `EventSource` (SSE) cannot send an `Authorization` header, so the realtime
//! stream authenticates via the HMAC-signed, SameSite=Strict GUI cookie. These
//! tests pin that behavior: a valid cookie authenticates a safe GET, but a
//! cookie alone can never perform a write (defense in depth on top of
//! SameSite=Strict), and no cookie is still rejected.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use kleos_lib::auth_piv::{ReplayGuard, SessionManager};
use kleos_lib::config::Config;
use kleos_lib::cred::CreddClient;
use kleos_lib::db::Database;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use kleos_server::server::build_router;
use kleos_server::state::AppState;

use common::{body_json, bootstrap_admin_key};

/// Build a monolith test app with the GUI enabled so the cookie-login flow works.
async fn gui_app() -> axum::Router {
    std::env::set_var("ENGRAM_OPEN_ACCESS", "0");
    std::env::set_var("ENGRAM_BOOTSTRAP_SECRET", "test-bootstrap-secret");
    std::env::set_var("CREDD_AGENT_KEY", "test-agent-key");

    // gui_enabled set in the initializer to satisfy clippy::field_reassign_with_default.
    let mut config = Config {
        gui_enabled: true, // KLEOS_GUI_PASSWORD equivalent
        ..Config::default()
    };
    let dir = std::env::temp_dir().join(format!("kleos-gui-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    config.data_dir = dir.to_string_lossy().into_owned();
    std::env::set_var("CREDD_URL", &config.eidolon.credd.url);

    let db = Arc::new(Database::connect_memory().await.expect("in-memory db"));
    let credd = Arc::new(CreddClient::from_config(&config));
    let state = AppState {
        db,
        encryption_key: None,
        config: Arc::new(config),
        credd,
        embedder: Arc::new(RwLock::new(None)),
        reranker: Arc::new(RwLock::new(None)),
        brain: None,
        llm: None,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        eidolon_config: None,
        approval_notify: None,
        pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        safe_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        dreamer_stats: kleos_server::dreamer::new_stats_handle(),
        last_request_time: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        tenant_registry: None,
        handoffs_gc_sem: Arc::new(tokio::sync::Semaphore::new(8)),
        shutdown_token: CancellationToken::new(),
        background_tasks: Arc::new(Mutex::new(JoinSet::new())),
        fact_extract_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        brain_absorb_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        // Detached audit channel: no worker in tests, events are dropped.
        audit_tx: tokio::sync::mpsc::channel(64).0,
        ingest_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        replay_guard: Arc::new(ReplayGuard::new()),
        session_manager: Arc::new(SessionManager::new([0u8; 32])),
        axon_broadcast: {
            let (tx, _) = tokio::sync::broadcast::channel(64);
            tx
        },
        artifact_encryption: Arc::new(
            kleos_lib::artifacts_crypto::ArtifactEncryption::new("").expect("disabled encryption"),
        ),
    };
    build_router(state)
}

/// Log in through /gui/auth with an API key and return the session cookie pair
/// ("name=value") for use as a Cookie header.
async fn gui_login_cookie(app: &axum::Router, api_key: &str) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/gui/auth")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("api_key={api_key}")))
                .unwrap(),
        )
        .await
        .expect("gui/auth request");
    assert!(
        res.status().is_success() || res.status().is_redirection(),
        "gui/auth should succeed, got {}",
        res.status()
    );
    let set_cookie = res
        .headers()
        .get("set-cookie")
        .expect("gui/auth must set a cookie")
        .to_str()
        .unwrap();
    // Keep just the "name=value" portion before the first attribute.
    set_cookie.split(';').next().unwrap().to_string()
}

/// Log in and return (combined Cookie header with both the session and CSRF
/// cookies, csrf_token_value) for exercising cookie-authenticated writes.
async fn gui_login_full(app: &axum::Router, api_key: &str) -> (String, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/gui/auth")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("api_key={api_key}")))
                .unwrap(),
        )
        .await
        .expect("gui/auth request");
    assert!(res.status().is_success(), "gui/auth should succeed");

    let mut session = None;
    let mut csrf_kv = None;
    for hv in res.headers().get_all("set-cookie") {
        let kv = hv.to_str().unwrap().split(';').next().unwrap().to_string();
        if kv.starts_with("engram_auth=") {
            session = Some(kv);
        } else if kv.starts_with("kleos_csrf=") {
            csrf_kv = Some(kv);
        }
    }
    let session = session.expect("session cookie set");
    let csrf_kv = csrf_kv.expect("csrf cookie set");
    let csrf_token = csrf_kv.strip_prefix("kleos_csrf=").unwrap().to_string();
    (format!("{session}; {csrf_kv}"), csrf_token)
}

#[tokio::test]
/// A valid GUI session cookie authenticates a safe GET.
async fn gui_cookie_authenticates_safe_get() {
    let app = gui_app().await;
    let admin = bootstrap_admin_key(&app).await;
    let cookie = gui_login_cookie(&app, &admin).await;

    // GET with ONLY the cookie (no Authorization header) authenticates.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a valid GUI cookie must authenticate a safe GET (this is what lets SSE connect)"
    );
}

#[tokio::test]
/// A cookie alone (no CSRF token) must not authorize a write.
async fn gui_cookie_cannot_write() {
    let app = gui_app().await;
    let admin = bootstrap_admin_key(&app).await;
    let cookie = gui_login_cookie(&app, &admin).await;

    // A mutating request with ONLY the cookie is rejected: cookie auth is
    // read-only, writes must carry a Bearer token.
    let (status, _body) = {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/projects")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        json!({ "name": "x", "status": "active" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let s = res.status();
        (s, body_json(res).await)
    };
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a cookie alone (no CSRF token) must not authorize a write"
    );
}

#[tokio::test]
/// Cookie + matching CSRF token authorizes a write.
async fn gui_cookie_with_valid_csrf_can_write() {
    let app = gui_app().await;
    let admin = bootstrap_admin_key(&app).await;
    let (cookie, csrf) = gui_login_full(&app, &admin).await;

    // Cookie + matching X-CSRF-Token authorizes a write, so the SPA no longer
    // needs a raw API key in localStorage.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects")
                .header("Cookie", &cookie)
                .header("X-CSRF-Token", &csrf)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({ "name": "csrf-ok", "status": "active" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "cookie + valid CSRF must authorize a write, got {}",
        res.status()
    );
}

#[tokio::test]
/// Cookie + WRONG CSRF token is rejected.
async fn gui_cookie_write_rejected_with_wrong_csrf() {
    let app = gui_app().await;
    let admin = bootstrap_admin_key(&app).await;
    let (cookie, _csrf) = gui_login_full(&app, &admin).await;

    // A forged / mismatched CSRF token must not authorize a write even with a
    // valid session cookie.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects")
                .header("Cookie", &cookie)
                .header(
                    "X-CSRF-Token",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({ "name": "csrf-bad", "status": "active" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a wrong CSRF token must not authorize a write"
    );
}

#[tokio::test]
/// Requests with neither bearer nor cookie are rejected.
async fn no_auth_is_rejected() {
    let app = gui_app().await;
    let _admin = bootstrap_admin_key(&app).await;

    let res = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
