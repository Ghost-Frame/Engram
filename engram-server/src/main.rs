mod middleware;
mod routes;
mod server;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "engram_server=debug,tower_http=debug".into()),
        )
        .init();

    if let Err(e) = server::run().await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
