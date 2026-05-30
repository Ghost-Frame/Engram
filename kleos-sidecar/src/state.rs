use kleos_lib::llm::local::LocalModelClient;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::session::SessionManager;
use crate::syntheos::SyntheosClient;

#[derive(Clone)]
pub struct SidecarState {
    pub client: reqwest::Client,
    pub kleos_url: String,
    pub kleos_api_key: Option<String>,
    /// Tiered request signer (PIV > ed25519 > none). When present, auth headers
    /// are signed rather than sent as a plain bearer token.
    pub signer: Option<Arc<kleos_lib::auth_piv::RequestSigner>>,
    pub llm: Option<Arc<LocalModelClient>>,
    pub sessions: Arc<RwLock<SessionManager>>,
    pub source: String,
    pub user_id: i64,
    pub token: Option<String>,
    pub compress_enabled: bool,
    pub compress_model: Option<String>,
    pub gate_model: Option<String>,
    pub batch_size: usize,
    pub batch_interval_ms: u64,
    pub max_pending_per_session: usize,
    pub retain_every_n: usize,
    pub overlap_turns: usize,
    pub retain_roles: Vec<String>,
    pub retain_tool_calls: bool,
    pub compress_passthrough_bytes: usize,
    pub compress_max_input_bytes: usize,
    pub compress_timeout_ms: u64,
    pub syntheos: Arc<SyntheosClient>,
}
