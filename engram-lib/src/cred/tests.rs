use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::cred::{find_secret_patterns, CreddClient, ProxyRequest};
use crate::db::Database;

#[derive(Clone)]
struct MockCreddState {
    expected_auth: String,
    documents: Arc<HashMap<(String, String), Value>>,
}

#[derive(Clone, Default)]
struct UpstreamState {
    seen_authorization: Arc<Mutex<Option<String>>>,
}

#[tokio::test]
async fn resolve_text_substitutes_placeholders_and_logs_audit() {
    let db = Database::connect_memory().await.expect("db");
    let (base_url, _handle) = spawn_mock_credd(HashMap::from([
        (
            ("foo".to_string(), "bar".to_string()),
            json!({
                "service": "foo",
                "key": "bar",
                "type": "ApiKey",
                "value": { "type": "api_key", "key": "alpha-secret" }
            }),
        ),
        (
            ("zip".to_string(), "zap".to_string()),
            json!({
                "service": "zip",
                "key": "zap",
                "type": "Note",
                "value": { "type": "note", "content": "omega-secret" }
            }),
        ),
    ]))
    .await;
    let client = CreddClient::for_testing(base_url, "test-agent-key", false, 60);

    let resolved = client
        .resolve_text(
            &db,
            1,
            "cred-test",
            "a={{secret:foo/bar}} b={{secret:zip/zap}}",
        )
        .await
        .expect("resolve text");

    assert_eq!(resolved, "a=alpha-secret b=omega-secret");

    let mut rows = db
        .conn
        .query(
            "SELECT service, action, payload FROM broca_actions WHERE service = 'cred' ORDER BY id ASC",
            (),
        )
        .await
        .expect("query audit rows");

    let mut payloads = Vec::new();
    while let Some(row) = rows.next().await.expect("next row") {
        let service: String = row.get(0).expect("service");
        let action: String = row.get(1).expect("action");
        let payload: String = row.get(2).expect("payload");
        assert_eq!(service, "cred");
        assert_eq!(action, "resolve");
        assert!(!payload.contains("alpha-secret"));
        assert!(!payload.contains("omega-secret"));
        payloads.push(payload);
    }

    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains("\"svc\":\"foo\""));
    assert!(payloads[1].contains("\"key\":\"zap\""));
}

#[tokio::test]
async fn raw_fetch_is_compile_or_runtime_gated() {
    let db = Database::connect_memory().await.expect("db");
    let (base_url, _handle) = spawn_mock_credd(HashMap::from([(
        ("foo".to_string(), "bar".to_string()),
        json!({
            "service": "foo",
            "key": "bar",
            "type": "ApiKey",
            "value": { "type": "api_key", "key": "alpha-secret" }
        }),
    )]))
    .await;
    let client = CreddClient::for_testing(base_url, "test-agent-key", false, 60);

    let err = client
        .get_raw(&db, 1, "cred-test", "foo", "bar")
        .await
        .expect_err("raw should be gated");

    let message = err.to_string();
    assert!(message.contains("disabled"));
}

#[tokio::test]
async fn proxy_forwards_secret_in_auth_header() {
    let db = Database::connect_memory().await.expect("db");
    let (credd_url, _credd_handle) = spawn_mock_credd(HashMap::from([(
        ("foo".to_string(), "bar".to_string()),
        json!({
            "service": "foo",
            "key": "bar",
            "type": "ApiKey",
            "value": { "type": "api_key", "key": "alpha-secret" }
        }),
    )]))
    .await;
    let upstream_state = UpstreamState::default();
    let (upstream_url, _upstream_handle) = spawn_upstream(upstream_state.clone()).await;
    let client = CreddClient::for_testing(credd_url, "test-agent-key", false, 60);

    let response = client
        .proxy(
            &db,
            1,
            "cred-test",
            "foo",
            "bar",
            &ProxyRequest {
                url: format!("{}/upstream", upstream_url),
                method: Some("POST".to_string()),
                headers: Some(HashMap::from([(
                    "Content-Type".to_string(),
                    "text/plain".to_string(),
                )])),
                body: Some("hello".to_string()),
                auth_header: None,
                auth_scheme: Some("Bearer".to_string()),
            },
        )
        .await
        .expect("proxy request");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, "proxied");
    assert_eq!(
        upstream_state
            .seen_authorization
            .lock()
            .await
            .clone()
            .unwrap_or_default(),
        "Bearer alpha-secret"
    );
}

#[test]
fn pattern_parser_handles_raw_and_nested_keys() {
    let patterns =
        find_secret_patterns("{{secret:svc/key}} {{secret-raw:security/blocklist/0}} trailing");

    assert_eq!(patterns.len(), 2);
    assert_eq!(patterns[0].service, "svc");
    assert_eq!(patterns[0].key, "key");
    assert_eq!(patterns[1].service, "security");
    assert_eq!(patterns[1].key, "blocklist/0");
}

async fn spawn_mock_credd(
    documents: HashMap<(String, String), Value>,
) -> (String, tokio::task::JoinHandle<()>) {
    let state = MockCreddState {
        expected_auth: "Bearer test-agent-key".to_string(),
        documents: Arc::new(documents),
    };

    let app = Router::new()
        .route("/secret/{service}/{*key}", get(mock_secret))
        .route("/secrets", get(mock_list))
        .with_state(state);

    spawn_router(app).await
}

async fn spawn_upstream(state: UpstreamState) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/upstream", post(mock_upstream))
        .with_state(state);

    spawn_router(app).await
}

async fn spawn_router(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test app");
    });
    (format!("http://{}", addr), handle)
}

async fn mock_secret(
    State(state): State<MockCreddState>,
    Path((service, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    if headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(state.expected_auth.as_str())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let key = key.trim_start_matches('/').to_string();
    let Some(document) = state.documents.get(&(service, key)).cloned() else {
        return Err(StatusCode::NOT_FOUND);
    };
    Ok(Json(document))
}

async fn mock_list(
    State(state): State<MockCreddState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    if headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(state.expected_auth.as_str())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let secrets: Vec<Value> = state
        .documents
        .keys()
        .map(|(service, key)| json!({ "service": service, "key": key }))
        .collect();
    Ok(Json(json!({ "secrets": secrets })))
}

async fn mock_upstream(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
) -> (StatusCode, String) {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    *state.seen_authorization.lock().await = value;
    (StatusCode::OK, "proxied".to_string())
}
