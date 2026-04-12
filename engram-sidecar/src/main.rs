mod auth;
mod routes;
mod session;

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
}

#[derive(Clone)]
pub struct SidecarState {
    pub client: reqwest::Client,
    pub engram_url: String,
    pub engram_api_key: Option<String>,
    pub llm: Option<Arc<LocalModelClient>>,
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

    // Probe LLM quickly (non-blocking network check).
    let llm: Option<Arc<LocalModelClient>> = {
        let llm_config = OllamaConfig::from_env();
        let client = LocalModelClient::new(llm_config);
        if client.probe().await {
            tracing::info!("local LLM client ready for sidecar");
            Some(Arc::new(client))
        } else {
            tracing::warn!(
                "local LLM unavailable for sidecar. Observations stored without enrichment."
            );
            None
        }
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
            // Print token once to stderr so the launching process can capture it.
            eprintln!("SIDECAR_TOKEN={}", generated);
            Some(generated)
        }
    };
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
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

    let app = routes::router(state);
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    axum::serve(listener, app).await.expect("server error");
}
