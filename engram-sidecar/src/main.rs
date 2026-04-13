mod auth;
mod routes;
mod session;
mod watcher;

use clap::Parser;
use engram_lib::llm::local::{LocalModelClient, OllamaConfig};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "engram-sidecar",
    about = "Engram memory sidecar for agent sessions"
)]
struct Cli {
    #[arg(short, long, default_value = "7711", env = "ENGRAM_SIDECAR_PORT")]
    port: u16,

    #[arg(long, default_value = "127.0.0.1", env = "ENGRAM_SIDECAR_HOST")]
    host: String,

    #[arg(long)]
    session_id: Option<String>,

    #[arg(long, default_value = "sidecar", env = "ENGRAM_SIDECAR_SOURCE")]
    source: String,

    #[arg(long, default_value = "1", env = "ENGRAM_SIDECAR_USER_ID")]
    user_id: i64,

    /// Shared-secret token clients must send as `Authorization: Bearer <token>`.
    /// If unset, a fresh token is generated at startup.
    #[arg(long, env = "ENGRAM_SIDECAR_TOKEN")]
    token: Option<String>,

    /// Engram server URL for memory storage/retrieval.
    #[arg(long, env = "ENGRAM_URL")]
    engram_url: String,

    /// API key for authenticating with the Engram server.
    #[arg(long, env = "ENGRAM_API_KEY")]
    engram_api_key: Option<String>,

    /// Enable file watcher for Claude Code session JSONL files.
    /// Watches ~/.claude/projects/ (or CLAUDE_SESSIONS_DIR) for changes.
    #[arg(long, env = "ENGRAM_SIDECAR_WATCH")]
    watch: bool,

    /// Directory to watch for session files (default: ~/.claude/projects).
    #[arg(long, env = "CLAUDE_SESSIONS_DIR")]
    watch_dir: Option<String>,
}

#[derive(Clone)]
pub struct SidecarState {
    pub client: reqwest::Client,
    pub engram_url: String,
    pub engram_api_key: Option<String>,
    pub llm: Arc<LocalModelClient>,
    pub session: Arc<RwLock<session::Session>>,
    pub source: String,
    pub user_id: i64,
    pub token: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "engram_sidecar=debug".into()),
        )
        .init();

    let cli = Cli::parse();

    // HTTP client for Engram server API calls
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to create HTTP client");

    // Create LLM client and probe once. Even if probe fails, we keep the
    // client so it can be re-probed later when Ollama becomes available.
    let llm: Arc<LocalModelClient> = {
        let llm_config = OllamaConfig::from_env();
        let client = LocalModelClient::new(llm_config);
        if client.probe().await {
            tracing::info!("local LLM client ready for sidecar");
        } else {
            tracing::warn!(
                "local LLM unavailable at startup -- will re-probe on first compress request"
            );
        }
        Arc::new(client)
    };

    let session_id = cli
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    tracing::info!(session_id = %session_id, "starting sidecar session");

    let token = match cli
        .token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(t) => Some(t.to_string()),
        None => {
            let generated = auth::generate_token();
            // SECURITY: log that a token was generated but do NOT log the value.
            // Use --token flag or ENGRAM_SIDECAR_TOKEN env to set explicitly.
            tracing::warn!(
                host = %cli.host,
                "ENGRAM_SIDECAR_TOKEN not set; generated one-time sidecar token (printed to stderr)"
            );
            // SECURITY (SEC-LOW-5): print token once to stderr so the launching
            // process can capture it. Only the first 8 hex chars are shown in the
            // log line; the full value is on a separate machine-parseable line.
            // Ensure stderr is NOT forwarded to persistent log files.
            eprintln!("SIDECAR_TOKEN={}", generated);
            tracing::debug!(token_prefix = &generated[..8.min(generated.len())], "sidecar token generated (see stderr for full value)");
            Some(generated)
        }
    };
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
    } else {
        tracing::info!(
            host = %cli.host,
            "no ENGRAM_SIDECAR_TOKEN set; running without auth (localhost-only)"
        );
    }

    let state = SidecarState {
        client,
        engram_url: cli.engram_url,
        engram_api_key: cli.engram_api_key,
        llm,
        session: Arc::new(RwLock::new(session::Session::new(session_id))),
        source: cli.source,
        user_id: cli.user_id,
        token,
    };

    // Start file watcher in background (if enabled)
    if cli.watch {
        if let Some(ref dir) = cli.watch_dir {
            std::env::set_var("CLAUDE_SESSIONS_DIR", dir);
        }
        let _watcher_handle = watcher::start(state.clone());
    }

    let app = routes::router(state);
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    axum::serve(listener, app).await.expect("server error");
}
