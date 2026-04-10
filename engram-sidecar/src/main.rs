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
}

#[derive(Clone)]
pub struct SidecarState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub llm: Option<Arc<LocalModelClient>>,
    pub session: Arc<RwLock<session::Session>>,
    pub source: String,
    pub user_id: i64,
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
    let db = Database::connect_with_config(&config)
        .await
        .expect("failed to connect to database");

    let embedder: Option<Arc<dyn EmbeddingProvider>> = match OnnxProvider::new(&config).await {
        Ok(provider) => Some(Arc::new(provider)),
        Err(e) => {
            tracing::warn!(
                "embedding provider unavailable: {}. Observations will be stored without embeddings.",
                e
            );
            None
        }
    };

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

    let state = SidecarState {
        db: Arc::new(db),
        config: Arc::new(config),
        embedder,
        llm,
        session: Arc::new(RwLock::new(session::Session::new(session_id))),
        source: cli.source,
        user_id: cli.user_id,
    };

    let app = routes::router(state);
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    axum::serve(listener, app).await.expect("server error");
}
