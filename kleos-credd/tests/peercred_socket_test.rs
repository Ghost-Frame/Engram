//! End-to-end test for SO_PEERCRED injection over the Unix socket listener.
//!
//! Starts `listen_unix` with a minimal router whose single handler returns
//! HTTP 200 only when it can extract a `PeerIdentity` extension. Connects
//! as the same OS user, asserts the response is 200 and that the observed
//! uid matches `nix::unistd::geteuid()`.

#![cfg(unix)]

use std::sync::{Arc, Mutex};

use axum::{extract::Extension, http::StatusCode, response::IntoResponse, routing::get, Router};
use kleos_credd::peercred::PeerIdentity;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

// ---------------------------------------------------------------------------
// Shared state for the handler to report back the observed uid.
// ---------------------------------------------------------------------------

/// Shared between server handler and the test assertion.
#[derive(Clone, Default)]
struct ObservedUid(Arc<Mutex<Option<u32>>>);

// ---------------------------------------------------------------------------
// Handler: returns 200 if PeerIdentity extension is present, 417 otherwise.
// Stores the observed uid into shared state for the test to assert.
// ---------------------------------------------------------------------------

async fn peer_check_handler(
    Extension(peer): Extension<PeerIdentity>,
    Extension(observed): Extension<ObservedUid>,
) -> impl IntoResponse {
    let mut guard = observed.0.lock().unwrap();
    *guard = Some(peer.uid);
    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peercred_extension_is_injected_and_carries_current_uid() {
    // Unique socket path per test run to avoid collision with other tests.
    let dir = tempfile::tempdir().expect("tempdir");
    let sock_path = dir.path().join("credd-test.sock");
    let sock_str = sock_path.to_str().unwrap().to_string();

    let observed = ObservedUid::default();
    let observed_clone = observed.clone();

    // Build a minimal router that only has the peer-check endpoint.
    // No auth middleware -- this is a focused unit test of the extension.
    let app: Router = Router::new()
        .route("/peer", get(peer_check_handler))
        .layer(axum::Extension(observed_clone));

    // Spawn the Unix listener in the background.
    let sock_for_task = sock_str.clone();
    let _server = tokio::spawn(async move {
        kleos_credd::listener::listen_unix_test_only(&sock_for_task, app)
            .await
            .expect("listener failed");
    });

    // Give the listener a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Connect over the Unix socket and send a minimal HTTP/1.1 GET.
    let mut stream = UnixStream::connect(&sock_str)
        .await
        .expect("connect to test socket");

    let request = "GET /peer HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    // Read the response (we only need the status line).
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");

    // Assert HTTP 200.
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected HTTP 200, got: {}",
        response.lines().next().unwrap_or("<empty>")
    );

    // Assert the handler observed the correct uid.
    let uid_seen = observed
        .0
        .lock()
        .unwrap()
        .expect("handler did not run -- PeerIdentity extension was missing");

    let expected_uid = nix::unistd::geteuid().as_raw();
    assert_eq!(
        uid_seen, expected_uid,
        "PeerIdentity.uid {uid_seen} != geteuid() {expected_uid}"
    );
}
