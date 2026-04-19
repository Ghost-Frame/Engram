// Library surface used by integration tests (tests/ directory).
// main.rs declares the same modules; lib.rs re-declares them so they are
// accessible as `kleos_sidecar::...` from test code.

pub mod auth;
pub mod metrics;
pub mod routes;
pub mod session;
pub mod state;
pub mod syntheos;
pub mod watcher;

pub use state::SidecarState;

use kleos_lib::llm::{local::LocalModelClient, OllamaConfig};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Construct a SidecarState with sane test defaults. `engram_url` is the
/// address of the mock upstream server used in integration tests.
pub fn build_test_state(engram_url: String, token: Option<String>) -> SidecarState {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("test http client");

    let llm = Arc::new(LocalModelClient::new(OllamaConfig::default()));
    let manager = session::SessionManager::new("test-default".to_string());
    let syntheos = Arc::new(syntheos::SyntheosClient::new_from_env(
        client.clone(),
        engram_url.clone(),
        None,
    ));

    SidecarState {
        client,
        engram_url,
        engram_api_key: None,
        llm,
        sessions: Arc::new(RwLock::new(manager)),
        source: "test".to_string(),
        user_id: 1,
        token,
        batch_size: 5,
        batch_interval_ms: 0, // disable time-based flush in tests
        max_pending_per_session: 100,
        compress_passthrough_bytes: 100,
        compress_max_input_bytes: 1000,
        compress_timeout_ms: 5000,
        syntheos,
    }
}
