use agent_forge::code_context::CodeIndex;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::session::SessionManager;
use crate::syntheos::SyntheosClient;

/// Controls whether local code retrieval is disabled, measured, or injected.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CodeContextMode {
    /// Do not refresh or query the code index from sidecar hooks.
    Off,
    /// Query and measure relevant code without adding it to agent context.
    #[default]
    Shadow,
    /// Add high-confidence code packs to prompt context.
    Inject,
}

/// String rendering helpers for logging and health responses.
impl CodeContextMode {
    /// Return the stable configuration spelling for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::Inject => "inject",
        }
    }
}

/// Shared dependencies and policy used by all sidecar routes.
#[derive(Clone)]
pub struct SidecarState {
    pub client: reqwest::Client,
    pub kleos_url: String,
    pub kleos_api_key: Option<String>,
    /// Tiered request signer (PIV > ed25519 > none). When present, auth headers
    /// are signed rather than sent as a plain bearer token.
    pub signer: Option<Arc<kleos_lib::auth_piv::RequestSigner>>,
    /// Optional local Agent-Forge index; absence makes code retrieval fail open.
    pub code_index: Option<Arc<CodeIndex>>,
    /// Whether code retrieval is disabled, measured, or injected.
    pub code_context_mode: CodeContextMode,
    /// Maximum approximate code tokens selected for one prompt.
    pub code_max_tokens: usize,
    pub sessions: Arc<RwLock<SessionManager>>,
    pub source: String,
    pub user_id: i64,
    pub token: Option<String>,
    pub batch_size: usize,
    pub batch_interval_ms: u64,
    pub max_pending_per_session: usize,
    pub retain_every_n: usize,
    pub overlap_turns: usize,
    pub retain_roles: Vec<String>,
    pub retain_tool_calls: bool,
    pub syntheos: Arc<SyntheosClient>,
}
