//! C-R3-004 cross-tenant isolation regression tests.
//!
//! These tests anchor the audit fix from
//! `~/Documents/specs/2026-04-27-kleos-audit-round3.md` (C-R3-004):
//!
//! - With tenant sharding ENABLED (the new default), two non-system users
//!   each get their own shard. Their projects, webhooks, and sync streams
//!   never cross.
//! - With tenant sharding DISABLED, the `ResolvedDb` extractor refuses
//!   non-system users with 503 Service Unavailable instead of silently
//!   falling back to the monolith.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use kleos_lib::config::Config;
use kleos_lib::cred::CreddClient;
use kleos_lib::db::Database;
use kleos_lib::tenant::{TenantConfig, TenantRegistry};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use kleos_server::server::build_router;
use kleos_server::state::AppState;

use common::{bootstrap_admin_key, get, post, seed_user};

/// Build a router with a real `TenantRegistry` rooted at a tempdir.
/// Returns `(router, state, tempdir)` -- the tempdir must outlive the test.
async fn test_app_with_sharding() -> (axum::Router, AppState, TempDir) {
    std::env::set_var("ENGRAM_OPEN_ACCESS", "0");
    std::env::set_var("ENGRAM_BOOTSTRAP_SECRET", "test-bootstrap-secret");
    std::env::set_var("CREDD_AGENT_KEY", "test-agent-key");

    let tmp = tempfile::tempdir().expect("tempdir");
    let config = Config::default();
    std::env::set_var("CREDD_URL", &config.eidolon.credd.url);

    let db = Arc::new(Database::connect_memory().await.expect("monolith db"));

    let registry = TenantRegistry::new(
        tmp.path().to_path_buf(),
        TenantConfig::default(),
        config.vector_dimensions,
    )
    .expect("tenant registry");

    let credd = Arc::new(CreddClient::from_config(&config));
    let state = AppState {
        db,
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
        tenant_registry: Some(Arc::new(registry)),
        handoffs_gc_sem: Arc::new(tokio::sync::Semaphore::new(8)),
        shutdown_token: CancellationToken::new(),
        background_tasks: Arc::new(Mutex::new(JoinSet::new())),
        fact_extract_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        brain_absorb_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        audit_log_sem: Arc::new(tokio::sync::Semaphore::new(64)),
        ingest_sem: Arc::new(tokio::sync::Semaphore::new(64)),
    };

    let router = build_router(state.clone());
    (router, state, tmp)
}

/// With sharding ENABLED, two non-system users cannot read or mutate each
/// other's projects. Each user lives in their own shard so list/get/delete
/// only see their own rows.
#[tokio::test]
async fn projects_isolated_between_tenants_with_sharding_on() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let (alice_uid, alice_key) = seed_user(&app, &admin_key, "alice").await;
    let (bob_uid, bob_key) = seed_user(&app, &admin_key, "bob").await;
    assert_ne!(alice_uid, bob_uid);

    // Alice creates a project.
    let (status, body) = post(
        &app,
        "/projects",
        &alice_key,
        json!({ "name": "alice-project", "status": "active" }),
    )
    .await;
    assert!(
        status.is_success(),
        "alice create project failed {status}: {body}"
    );
    let alice_project_id = body["id"].as_i64().expect("alice project id");

    // Bob creates a different project.
    let (status, body) = post(
        &app,
        "/projects",
        &bob_key,
        json!({ "name": "bob-project", "status": "active" }),
    )
    .await;
    assert!(
        status.is_success(),
        "bob create project failed {status}: {body}"
    );
    let bob_project_id = body["id"].as_i64().expect("bob project id");

    // Alice's list shows only her project.
    let (status, body) = get(&app, "/projects", &alice_key).await;
    assert!(status.is_success(), "alice list failed {status}: {body}");
    let names: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        vec!["alice-project"],
        "alice should only see her own projects, got {body}"
    );

    // Bob's list shows only his project.
    let (status, body) = get(&app, "/projects", &bob_key).await;
    assert!(status.is_success(), "bob list failed {status}: {body}");
    let names: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        vec!["bob-project"],
        "bob should only see his own projects, got {body}"
    );

    // Cross-tenant GET: per-shard auto-increment may collide on IDs (each
    // shard's first project is id=1), so we verify by NAME instead. If Bob
    // GETs Alice's project id, he must either get 404 (his shard has no row
    // at that id) or his own row -- never Alice's row.
    let (_, body) = get(&app, &format!("/projects/{}", alice_project_id), &bob_key).await;
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        assert_ne!(
            name, "alice-project",
            "bob fetched alice's project by id -- cross-tenant leak: {body}"
        );
    }

    // Symmetry: Alice fetching Bob's id must not return Bob's name.
    let (_, body) = get(&app, &format!("/projects/{}", bob_project_id), &alice_key).await;
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        assert_ne!(
            name, "bob-project",
            "alice fetched bob's project by id -- cross-tenant leak: {body}"
        );
    }
}

/// With sharding ENABLED, webhooks created by one tenant are invisible to
/// another tenant.
#[tokio::test]
async fn webhooks_isolated_between_tenants_with_sharding_on() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let (_, alice_key) = seed_user(&app, &admin_key, "alice-hook").await;
    let (_, bob_key) = seed_user(&app, &admin_key, "bob-hook").await;

    let (status, body) = post(
        &app,
        "/webhooks",
        &alice_key,
        json!({ "url": "https://example.com/alice", "events": ["*"] }),
    )
    .await;
    assert!(
        status.is_success(),
        "alice webhook create failed {status}: {body}"
    );

    let (status, body) = get(&app, "/webhooks", &bob_key).await;
    assert!(
        status.is_success(),
        "bob list webhooks failed {status}: {body}"
    );
    let count = body["count"].as_i64().unwrap_or(-1);
    assert_eq!(
        count, 0,
        "bob should see zero webhooks (alice's must not leak), got {body}"
    );
}

/// With sharding DISABLED, non-system users cannot reach any tenant-scoped
/// route. The fail-closed extractor returns 503 to surface the misconfig
/// instead of silently falling back to the monolith.
#[tokio::test]
async fn non_system_user_gets_503_when_sharding_disabled() {
    let (app, _state) = common::test_app().await; // tenant_registry: None
    let admin_key = bootstrap_admin_key(&app).await;
    let (_uid, user_key) = seed_user(&app, &admin_key, "no-shard-user").await;

    // /projects is ResolvedDb-backed; non-system user must fail closed.
    let (status, body) = get(&app, "/projects", &user_key).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "non-system user with sharding disabled must get 503, got {status}: {body}"
    );

    // /webhooks is also ResolvedDb-backed.
    let (status, body) = get(&app, "/webhooks", &user_key).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "/webhooks must also fail closed, got {status}: {body}"
    );

    // /sync/changes is also ResolvedDb-backed.
    let (status, body) = get(&app, "/sync/changes", &user_key).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "/sync/changes must also fail closed, got {status}: {body}"
    );
}

/// With sharding DISABLED, even user_id=1 (the operator) gets 503 on
/// tenant-scoped routes. The legacy carve-out that pinned user_id=1 to
/// the monolith was removed during the monolith->tenant migration; the
/// operator now lives in tenants/1/ like every other user, so disabling
/// sharding takes the operator offline together with everyone else.
/// System-only routes (admin/auth_keys/audit) keep working because they
/// bypass ResolvedDb and go straight to `state.db`.
#[tokio::test]
async fn system_user_503s_for_tenant_routes_when_sharding_disabled() {
    let (app, _state) = common::test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    let (status, _body) = get(&app, "/projects", &admin_key).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "user_id=1 must also fail closed when sharding is disabled (no monolith carve-out)"
    );
}

/// With sharding ENABLED, user_id=1 (Master) and user_id=2 see fully
/// isolated projects. Anchors the post-migration contract: the operator
/// is just another tenant.
#[tokio::test]
async fn user_one_isolated_from_user_two_with_sharding_on() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let admin_key = bootstrap_admin_key(&app).await;

    // user_id=1 is the bootstrap admin. user_id=2 is freshly seeded.
    let (uid_two, user_two_key) = seed_user(&app, &admin_key, "tenant-two").await;
    assert_ne!(uid_two, 1, "seeded user must not be the system user");

    // Master creates a project.
    let (status, _) = post(
        &app,
        "/projects",
        &admin_key,
        json!({"name": "master-only", "path": "/tmp/master"}),
    )
    .await;
    assert!(status.is_success(), "master project create");

    // Tenant 2 creates a project.
    let (status, _) = post(
        &app,
        "/projects",
        &user_two_key,
        json!({"name": "tenant-two-only", "path": "/tmp/two"}),
    )
    .await;
    assert!(status.is_success(), "tenant 2 project create");

    // Master lists -- should see only their own.
    let (_, body) = get(&app, "/projects", &admin_key).await;
    let names: Vec<String> = body["projects"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&"master-only".to_string()),
        "master sees own"
    );
    assert!(
        !names.contains(&"tenant-two-only".to_string()),
        "master must not see tenant 2: {body}"
    );

    // Tenant 2 lists -- should see only their own.
    let (_, body) = get(&app, "/projects", &user_two_key).await;
    let names: Vec<String> = body["projects"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&"tenant-two-only".to_string()),
        "tenant 2 sees own"
    );
    assert!(
        !names.contains(&"master-only".to_string()),
        "tenant 2 must not see master: {body}"
    );
}
