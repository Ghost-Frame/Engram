mod auth;
mod routes;
mod session;

use clap::Parser;
use engram_lib::config::Config;
use engram_lib::db::Database;
use engram_lib::embeddings::onnx::OnnxProvider;
use engram_lib::embeddings::EmbeddingProvider;
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
    /// If unset and host is loopback, auth is skipped with a warning.
    /// If unset and host is non-loopback, the sidecar refuses to start.
    #[arg(long, env = "ENGRAM_SIDECAR_TOKEN")]
    token: Option<String>,
}

/// Shared embedder slot, populated asynchronously after the server starts.
pub type SharedEmbedder = Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>;

#[derive(Clone)]
pub struct SidecarState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: SharedEmbedder,
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
    let config = Config::from_env();
    let db = Database::connect_with_config(&config, None)
        .await
        .expect("failed to connect to database");

    // Embedder loads in the background -- the server starts immediately.
    let embedder: SharedEmbedder = Arc::new(RwLock::new(None));

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

    // SECURITY (SEC-CRIT-1 / MT-F18): refuse to start on a non-loopback
    // interface without a shared-secret token. On loopback, allow unauthed
    // startup with a loud warning to avoid breaking existing dev flows.
    let loopback = auth::is_loopback_host(&cli.host);
    let token = match cli
        .token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(t) => Some(t.to_string()),
        None => {
            if !loopback {
                eprintln!(
                    "error: sidecar refuses to bind {} without ENGRAM_SIDECAR_TOKEN (non-loopback)",
                    cli.host
                );
                std::process::exit(2);
            }
            tracing::warn!(
                host = %cli.host,
                "ENGRAM_SIDECAR_TOKEN not set; loopback-only so auth is skipped"
            );
            None
        }
    };
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
    }

    let state = SidecarState {
        db: Arc::new(db),
        config: Arc::new(config.clone()),
        embedder: embedder.clone(),
        llm,
        session: Arc::new(RwLock::new(session::Session::new(session_id))),
        source: cli.source,
        user_id: cli.user_id,
        token,
    };

    // Spawn background embedder initialization (non-blocking for server).
    let embedder_slot = embedder.clone();
    let embedder_config = config;
    tokio::spawn(async move {
        tracing::info!("loading embedding model in background...");
        match OnnxProvider::new(&embedder_config).await {
            Ok(provider) => {
                *embedder_slot.write().await = Some(Arc::new(provider));
                tracing::info!("embedding provider ready (loaded in background)");
            }
            Err(e) => {
                tracing::warn!(
                    "embedding provider unavailable: {}. Observations stored without embeddings.",
                    e
                );
            }
        }
    });

    let app = routes::router(state);
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    axum::serve(listener, app).await.expect("server error");
}
