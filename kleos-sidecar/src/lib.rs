// Library surface used by integration tests (tests/ directory).
// main.rs declares the same modules; lib.rs re-declares them so they are
// accessible as `kleos_sidecar::...` from test code.

pub mod auth;
pub mod metrics;
pub mod routes;
pub mod session;
pub mod state;
pub mod syntheos;

pub use state::{CodeContextMode, SidecarState};

use std::sync::Arc;
use tokio::sync::RwLock;

/// Construct a SidecarState with sane test defaults. `kleos_url` is the
/// address of the mock upstream server used in integration tests.
pub fn build_test_state(kleos_url: String, token: Option<String>) -> SidecarState {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("test http client");

    let code_index = agent_forge::code_context::CodeIndex::open(
        std::env::temp_dir().join(format!("kleos-sidecar-test-{}.db", uuid::Uuid::new_v4())),
    )
    .ok()
    .map(Arc::new);
    let manager = session::SessionManager::new("test-default".to_string());
    let syntheos = Arc::new(syntheos::SyntheosClient::new_from_env(
        client.clone(),
        kleos_url.clone(),
        None,
    ));

    SidecarState {
        client,
        kleos_url,
        kleos_api_key: None,
        signer: None,
        code_index,
        code_context_mode: state::CodeContextMode::Shadow,
        code_max_tokens: 2_000,
        sessions: Arc::new(RwLock::new(manager)),
        source: "test".to_string(),
        user_id: 1,
        token,
        batch_size: 5,
        batch_interval_ms: 0, // disable time-based flush in tests
        max_pending_per_session: 100,
        retain_every_n: 5,
        overlap_turns: 2,
        retain_roles: vec![
            "user".to_string(),
            "assistant".to_string(),
            "tool".to_string(),
        ],
        retain_tool_calls: true,
        syntheos,
    }
}
