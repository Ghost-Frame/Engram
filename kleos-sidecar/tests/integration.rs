// Integration tests for kleos-sidecar.
//
// Each test spins up:
//   - A mock upstream (axum router on a random OS-assigned port) that stands in for kleos-server.
//   - A sidecar router (using build_test_state) bound to a second random port.
//
// Tests drive the sidecar via plain reqwest calls, then inspect the mock
// upstream's recorded calls to verify correct behavior.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::Body,
    extract::State as AxumState,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::any,
    Router,
};
use kleos_sidecar::{build_test_state, routes, CodeContextMode, SidecarState};
use reqwest::Client;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Mock upstream helpers
// ---------------------------------------------------------------------------

/// Shared state for the mock upstream: counts of POST /batch calls + response status.
#[derive(Clone, Default)]
struct MockState {
    batch_calls: Arc<Mutex<Vec<serde_json::Value>>>,
    store_calls: Arc<Mutex<Vec<serde_json::Value>>>,
    /// Response to return for /batch. Default 200.
    batch_status: Arc<Mutex<u16>>,
    /// If Some(k), /batch accepts the first k ops, returns a failure result for
    /// the (k+1)th, and truncates the remaining ops (mirroring the real server's
    /// stop-on-first-failure behaviour).
    batch_fail_after: Arc<Mutex<Option<usize>>>,
}

/// Test-facing accessors and knobs for the mock upstream's recorded state.
impl MockState {
    /// Number of POST /batch calls the mock upstream has received so far.
    fn batch_call_count(&self) -> usize {
        self.batch_calls.lock().unwrap().len()
    }

    /// Set the HTTP status the mock returns for /batch.
    fn set_batch_status(&self, code: u16) {
        *self.batch_status.lock().unwrap() = code;
    }

    /// Configure /batch to accept the first `k` ops then fail (None = succeed).
    fn set_batch_fail_after(&self, k: Option<usize>) {
        *self.batch_fail_after.lock().unwrap() = k;
    }
}

/// Mock POST /batch handler: records the request body and applies the
/// configured status / fail-after behaviour.
async fn mock_batch(AxumState(ms): AxumState<MockState>, req: Request<Body>) -> impl IntoResponse {
    let status = *ms.batch_status.lock().unwrap();
    let fail_after = *ms.batch_fail_after.lock().unwrap();
    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();

    let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let op_count = parsed
        .get("ops")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(1);
    ms.batch_calls.lock().unwrap().push(parsed);

    if status != 200 {
        return StatusCode::from_u16(status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            .into_response();
    }

    // Build a per-op results array matching the request length (or the
    // real server's stop-on-first-failure truncation when fail_after is set).
    let mut results: Vec<serde_json::Value> = Vec::new();
    match fail_after {
        Some(k) if k < op_count => {
            for i in 0..k {
                results.push(serde_json::json!({"index": i, "op": "store", "success": true}));
            }
            results.push(serde_json::json!({
                "index": k,
                "op": "store",
                "success": false,
                "error": "simulated failure",
            }));
            // Ops beyond k are not attempted, so no result entry.
            let body = serde_json::json!({
                "results": results,
                "total": results.len(),
                "succeeded": k,
            });
            (StatusCode::MULTI_STATUS, axum::Json(body)).into_response()
        }
        _ => {
            for i in 0..op_count {
                results.push(serde_json::json!({"index": i, "op": "store", "success": true}));
            }
            let body = serde_json::json!({
                "results": results,
                "total": op_count,
                "succeeded": op_count,
            });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
    }
}

/// Mock POST /store handler: records the request body and returns a stub id.
async fn mock_store(AxumState(ms): AxumState<MockState>, req: Request<Body>) -> impl IntoResponse {
    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
        ms.store_calls.lock().unwrap().push(val);
    }
    (StatusCode::OK, axum::Json(serde_json::json!({"id": 1}))).into_response()
}

/// Mock health endpoint: always 200.
async fn mock_health(_req: Request<Body>) -> impl IntoResponse {
    StatusCode::OK
}

/// Catch-all for unexpected routes: 404.
async fn mock_catch_all(_req: Request<Body>) -> impl IntoResponse {
    StatusCode::NOT_FOUND
}

/// Build and bind a mock upstream. Returns (url, MockState, JoinHandle).
async fn spawn_mock_upstream() -> (String, MockState, tokio::task::JoinHandle<()>) {
    let ms = MockState {
        batch_status: Arc::new(Mutex::new(200)),
        ..Default::default()
    };
    let app = Router::new()
        .route("/batch", any(mock_batch))
        .route("/store", any(mock_store))
        .route("/memory/store", any(mock_store))
        .route("/health", any(mock_health))
        .fallback(any(mock_catch_all))
        .with_state(ms.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, ms, handle)
}

/// Spawn the sidecar router. Returns (base_url, state, JoinHandle).
async fn spawn_sidecar(
    upstream_url: String,
    token: Option<String>,
) -> (String, SidecarState, tokio::task::JoinHandle<()>) {
    // Tests bind mock upstreams on 127.0.0.1; allow private addresses through
    // validate_outbound_url for this controlled in-process loopback.
    std::env::set_var("KLEOS_NET_ALLOW_PRIVATE", "1");
    let state = build_test_state(upstream_url, token.clone());
    let app =
        routes::router(state.clone()).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, state, handle)
}

/// Bind the sidecar router for `state` on a random port and serve it in a
/// background task. Returns (base_url, state, join_handle).
async fn spawn_sidecar_with_state(
    state: SidecarState,
) -> (String, SidecarState, tokio::task::JoinHandle<()>) {
    std::env::set_var("KLEOS_NET_ALLOW_PRIVATE", "1");
    let app =
        routes::router(state.clone()).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let cloned_state = state.clone();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, cloned_state, handle)
}

/// Poll `cond` every 25ms until it holds or `max` elapses; returns whether it
/// held. Replaces fixed post-action sleeps, which raced the sidecar's async
/// flush tasks under loaded parallel test runners (nextest runs many test
/// processes at once, so scheduling latency varies widely).
async fn wait_until(max: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    while !cond() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    true
}

/// Bounded wait for a session's flush BOOKKEEPING: polls until the named
/// session's (stored_count, pending_count) match the expected pair or `max`
/// elapses. The mock upstream receives /batch before the sidecar records the
/// outcome, so the batch call count alone is too early an observable for
/// assertions about stored/pending state.
async fn wait_for_counts(
    state: &SidecarState,
    sess: &str,
    stored: usize,
    pending: usize,
    max: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        let got = {
            let sessions = state.sessions.read().await;
            sessions
                .list()
                .into_iter()
                .find(|s| s.id == sess)
                .map(|s| (s.stored_count, s.pending_count))
        };
        if got == Some((stored, pending)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// reqwest client with a 5s timeout and an optional bearer-token header.
fn client(token: Option<&str>) -> Client {
    let mut builder = Client::builder().timeout(Duration::from_secs(5));
    if let Some(t) = token {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", t).parse().unwrap(),
        );
        builder = builder.default_headers(headers);
    }
    builder.build().unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: happy path -- /session/start -> /observe -> automatic flush ->
// mock upstream sees the batch.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_happy_path_observe_and_flush() {
    let (upstream_url, ms, _upstream) = spawn_mock_upstream().await;
    let token = "test-token-happy";
    let (sidecar_url, state, _sidecar) = spawn_sidecar(upstream_url, Some(token.to_string())).await;
    let c = client(Some(token));

    // Start a named session.
    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-happy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);

    // Send enough observations to trigger a size-based flush (batch_size=5).
    for i in 0..5 {
        let r = c
            .post(format!("{}/observe", sidecar_url))
            .json(&serde_json::json!({
                "tool_name": "Read",
                "content": format!("reading file {}", i),
                "session_id": "sess-happy",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
    }

    // Mock upstream should receive at least one /batch call once the async
    // flush task runs (bounded wait instead of a fixed sleep).
    assert!(
        wait_until(Duration::from_secs(10), || ms.batch_call_count() >= 1).await,
        "expected at least 1 /batch call, got {}",
        ms.batch_call_count()
    );

    // Verify the batch contained ops.
    {
        let calls = ms.batch_calls.lock().unwrap();
        let ops = calls[0].get("ops").and_then(|v| v.as_array());
        assert!(ops.is_some(), "batch body should have 'ops' array");
    }

    // End the session -- should succeed.
    let r = c
        .post(format!("{}/end", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-happy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let _ = state; // keep alive
}

// ---------------------------------------------------------------------------
// Test 2: retry exhaustion -- mock upstream returns 500; after filling the
// pending queue to max, /observe returns 503.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retry_exhaustion_returns_503() {
    // Manual serve loop (no spawn_sidecar): set the loopback allowance so the
    // flush actually reaches the mock and exhausts on real HTTP 500s, rather
    // than failing earlier (and vacuously) on outbound-URL validation.
    std::env::set_var("KLEOS_NET_ALLOW_PRIVATE", "1");
    let (upstream_url, ms, _upstream) = spawn_mock_upstream().await;
    ms.set_batch_status(500);

    let token = "test-token-503";
    // Use max_pending=10 so the test is fast.
    let mut state = build_test_state(upstream_url, Some(token.to_string()));
    state.max_pending_per_session = 10;
    state.batch_size = 200; // high threshold so flush happens at the hard cap only
    state.retain_every_n = 100_000; // disable turn-based drains so queue fills to max_pending
    let app =
        routes::router(state.clone()).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sidecar_url = format!("http://{}", addr);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let c = client(Some(token));

    // Start session.
    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-full" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);

    // Fill the queue exactly to the limit (max_pending = 10).
    for i in 0..10 {
        let r = c
            .post(format!("{}/observe", sidecar_url))
            .json(&serde_json::json!({
                "tool_name": "Bash",
                "content": format!("cmd {}", i),
                "session_id": "sess-full",
            }))
            .send()
            .await
            .unwrap();
        // These might return 202 or they might trigger a failed flush and
        // restore. Either way, we don't assert on them -- we just fill the queue.
        let _ = r.status();
    }

    // One more should return 503.
    let r = c
        .post(format!("{}/observe", sidecar_url))
        .json(&serde_json::json!({
            "tool_name": "Bash",
            "content": "overflow",
            "session_id": "sess-full",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "expected 503 when pending queue is full"
    );
}

// ---------------------------------------------------------------------------
// Test 3: graceful shutdown flushes pending observations.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_graceful_shutdown_flushes_pending() {
    // This test wires its own serve loop (to control with_graceful_shutdown)
    // instead of going through spawn_sidecar, so it must set the loopback
    // allowance itself: without it validate_outbound_url rejects the mock
    // upstream and the shutdown flush silently sends nothing. Under threaded
    // cargo test, sibling tests' spawn_sidecar calls leaked this process-global
    // var and masked the dependency; per-test-process runners (nextest) do not.
    std::env::set_var("KLEOS_NET_ALLOW_PRIVATE", "1");
    let (upstream_url, ms, _upstream) = spawn_mock_upstream().await;
    let token = "test-token-shutdown";

    // Build state with high batch_size so observations queue up without auto-flush.
    let mut state = build_test_state(upstream_url.clone(), Some(token.to_string()));
    state.batch_size = 1000;
    let app =
        routes::router(state.clone()).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sidecar_url = format!("http://{}", addr);

    let state_for_shutdown = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
                // Flush all sessions with a 10s deadline, mirroring main.rs.
                let _ = tokio::time::timeout(
                    Duration::from_secs(10),
                    routes::flush_all_sessions(&state_for_shutdown),
                )
                .await;
            })
            .await
            .unwrap();
    });

    let c = client(Some(token));

    // Queue some observations.
    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-shutdown" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);

    for i in 0..3 {
        let r = c
            .post(format!("{}/observe", sidecar_url))
            .json(&serde_json::json!({
                "tool_name": "Write",
                "content": format!("writing {}", i),
                "session_id": "sess-shutdown",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
    }

    // No flush should have happened yet (batch_size = 1000).
    assert_eq!(ms.batch_call_count(), 0, "no flush before shutdown");

    // Trigger graceful shutdown.
    let _ = shutdown_tx.send(());

    // Bounded wait matching the 10s flush deadline above: a fixed 500ms sleep
    // raced the serve task's shutdown flush under parallel test load.
    assert!(
        wait_until(Duration::from_secs(10), || ms.batch_call_count() >= 1).await,
        "flush should have occurred during graceful shutdown"
    );
}

// ---------------------------------------------------------------------------
// Test 4: /session/start then /end lifecycle.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_start_and_end_lifecycle() {
    let (upstream_url, _ms, _upstream) = spawn_mock_upstream().await;
    let token = "test-token-lifecycle";
    let (sidecar_url, _state, _sidecar) =
        spawn_sidecar(upstream_url, Some(token.to_string())).await;
    let c = client(Some(token));

    // Start session.
    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-lifecycle" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["session_id"], "sess-lifecycle");

    // Starting same session again returns 409 Conflict.
    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-lifecycle" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);

    // List sessions -- should contain "sess-lifecycle".
    let r = c
        .get(format!("{}/sessions", sidecar_url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: serde_json::Value = r.json().await.unwrap();
    let sessions = body["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|s| s["id"] == "sess-lifecycle"));

    // End session.
    let r = c
        .post(format!("{}/end", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-lifecycle" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["ended"], true);

    // Ending again returns 410 Gone.
    let r = c
        .post(format!("{}/end", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-lifecycle" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::GONE);
}

// ---------------------------------------------------------------------------
// Test 5: local code injection survives an unavailable memory upstream.
// ---------------------------------------------------------------------------

/// Exact code retrieval remains useful when both Kleos search routes return 404.
#[tokio::test]
async fn test_code_context_injects_when_memory_recall_fails() {
    let (upstream_url, _ms, _upstream) = spawn_mock_upstream().await;
    let token = "test-token-code-context";
    let repository = tempfile::tempdir().unwrap();
    std::fs::create_dir(repository.path().join(".git")).unwrap();
    std::fs::create_dir(repository.path().join("src")).unwrap();
    std::fs::write(
        repository.path().join("src/lib.rs"),
        "/// Reconciles one invoice.\npub fn reconcile_invoice() -> bool { true }\n",
    )
    .unwrap();

    let mut state = build_test_state(upstream_url, Some(token.to_string()));
    state.code_context_mode = CodeContextMode::Inject;
    let (sidecar_url, _state, _sidecar) = spawn_sidecar_with_state(state).await;
    let response = client(Some(token))
        .post(format!("{}/recall", sidecar_url))
        .json(&serde_json::json!({
            "query": "Where is reconcile_invoice implemented?",
            "session_id": "sess-code-context",
            "cwd": repository.path(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code_mode"], "inject");
    assert!(body["memory_error"].is_string());
    assert!(body["context"]
        .as_str()
        .unwrap_or_default()
        .contains("pub fn reconcile_invoice"));
    assert_eq!(body["code_pack"]["snippets"].as_array().unwrap().len(), 1);

    let repeated = client(Some(token))
        .post(format!("{}/recall", sidecar_url))
        .json(&serde_json::json!({
            "query": "invoice reconciliation behavior",
            "session_id": "sess-code-context",
            "cwd": repository.path(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(repeated.status(), StatusCode::OK);
    let repeated_body: serde_json::Value = repeated.json().await.unwrap();
    assert!(repeated_body["context"]
        .as_str()
        .unwrap_or_default()
        .is_empty());
    assert!(repeated_body["code_pack"]["snippets"]
        .as_array()
        .unwrap()
        .is_empty());

    let exact_repeat = client(Some(token))
        .post(format!("{}/recall", sidecar_url))
        .json(&serde_json::json!({
            "query": "reconcile_invoice",
            "session_id": "sess-code-context",
            "cwd": repository.path(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(exact_repeat.status(), StatusCode::OK);
    let exact_body: serde_json::Value = exact_repeat.json().await.unwrap();
    assert!(exact_body["context"]
        .as_str()
        .unwrap_or_default()
        .contains("pub fn reconcile_invoice"));
}

// ---------------------------------------------------------------------------
// Test 6: idle session sweep expires stale sessions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_idle_session_sweep() {
    use kleos_sidecar::session::SessionManager;

    let mut mgr = SessionManager::new("default".to_string());
    mgr.start_session("fresh".to_string()).unwrap();
    mgr.start_session("stale".to_string()).unwrap();

    // Access "fresh" to update its last_activity.
    {
        let s = mgr.get_mut("fresh").unwrap();
        // Add an observation to update last_activity.
        use kleos_sidecar::session::Observation;
        s.add_observation(Observation {
            tool_name: "Read".to_string(),
            content: "recent".to_string(),
            role: "tool".to_string(),
            importance: 1,
            category: "d".to_string(),
            origin: None,
            timestamp: chrono::Utc::now(),
        });
    }

    // Expire with a zero-duration TTL -- everything except default and sessions
    // with pending should be expired. "fresh" has pending, so it survives.
    // "stale" has no pending and zero elapsed > zero TTL... well with Duration::ZERO
    // every elapsed > 0, so both non-default sessions with no pending get swept.
    // Use a very small TTL and sleep to make "stale" exceed it.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let removed = mgr.expire_idle(Duration::from_millis(1));
    // "stale" has no pending and is idle > 1ms -- should be removed.
    // "fresh" has pending -- should be kept.
    assert!(removed >= 1, "at least 'stale' should be expired");
    assert!(
        mgr.get("fresh").is_some(),
        "session with pending should survive sweep"
    );
    assert!(
        mgr.get("default").is_some(),
        "default session should never be swept"
    );
}

// ---------------------------------------------------------------------------
// Test 9: partial /batch success must requeue the failed suffix and count
// only confirmed successes in stored_count. Regression test for the bug where
// drain_pending() bumped stored_count eagerly and the Ok(Some(n)) branch in
// flush_pending silently discarded the failed observations.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_partial_batch_requeues_failed_and_counts_only_successes() {
    let (upstream_url, ms, _upstream) = spawn_mock_upstream().await;
    // Mock accepts the first 3 ops of each /batch call and fails the 4th,
    // matching the real server's stop-on-first-failure semantics.
    ms.set_batch_fail_after(Some(3));

    let token = "test-token-partial";
    let mut state = build_test_state(upstream_url, Some(token.to_string()));
    // Disable overlap so drain_with_overlap behaves like drain_pending -- this
    // test validates partial-batch requeue semantics, not retain/overlap.
    state.overlap_turns = 0;
    let (sidecar_url, state, _sidecar) = spawn_sidecar_with_state(state).await;
    let c = client(Some(token));

    let r = c
        .post(format!("{}/session/start", sidecar_url))
        .json(&serde_json::json!({ "session_id": "sess-partial" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);

    // Send 5 observations to trip the size-based flush (batch_size=5 in
    // build_test_state). The inline flush happens on the 5th /observe.
    for i in 0..5 {
        let r = c
            .post(format!("{}/observe", sidecar_url))
            .json(&serde_json::json!({
                "tool_name": "Read",
                "content": format!("obs {}", i),
                "session_id": "sess-partial",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
    }

    // Bounded wait for the flush bookkeeping (not just the /batch arrival),
    // then pin the exact call count.
    assert!(
        wait_for_counts(&state, "sess-partial", 3, 2, Duration::from_secs(10)).await,
        "first-flush bookkeeping did not land"
    );
    assert_eq!(ms.batch_call_count(), 1, "exactly one batch sent so far");

    {
        let sessions = state.sessions.read().await;
        let info = sessions
            .list()
            .into_iter()
            .find(|s| s.id == "sess-partial")
            .expect("session exists");
        assert_eq!(
            info.stored_count, 3,
            "stored_count must reflect only confirmed /batch successes"
        );
        assert_eq!(
            info.pending_count, 2,
            "the 2 failed observations must be requeued"
        );
        assert_eq!(info.observation_count, 5);
    }

    // Disable the failure mode. Sending 3 more obs pushes pending back to 5
    // (2 requeued + 3 new) and triggers another flush, which now succeeds in
    // full. Final state: all 5 observations from the original batch plus the
    // 3 new ones delivered, nothing left pending.
    ms.set_batch_fail_after(None);
    for i in 0..3 {
        let r = c
            .post(format!("{}/observe", sidecar_url))
            .json(&serde_json::json!({
                "tool_name": "Read",
                "content": format!("obs {}", 100 + i),
                "session_id": "sess-partial",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
    }
    // Same bookkeeping-based wait for the second flush.
    assert!(
        wait_for_counts(&state, "sess-partial", 8, 0, Duration::from_secs(10)).await,
        "second-flush bookkeeping did not land"
    );
    assert_eq!(ms.batch_call_count(), 2, "second batch should be flushed");

    {
        let sessions = state.sessions.read().await;
        let info = sessions
            .list()
            .into_iter()
            .find(|s| s.id == "sess-partial")
            .expect("session still exists");
        assert_eq!(
            info.pending_count, 0,
            "all observations eventually delivered"
        );
        assert_eq!(
            info.stored_count, 8,
            "3 from first batch + 5 from the second (2 requeued + 3 new)"
        );
        assert_eq!(info.observation_count, 8);
    }
}
