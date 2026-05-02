use clap::Parser;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "stdio")]
    transport: String,
    #[cfg(feature = "http")]
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: String,
}

#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard = kleos_lib::observability::init_tracing("engram-mcp", "kleos_mcp=info");

    let args = Args::parse();
    let app = kleos_mcp::App::from_env()
        .await
        .expect("failed to initialize engram-mcp");

    match args.transport.as_str() {
        "stdio" => kleos_mcp::transport::stdio::serve(app)
            .await
            .expect("stdio transport failed"),
        #[cfg(feature = "http")]
        "http" => kleos_mcp::transport::http::serve(app, &args.listen)
            .await
            .expect("http transport failed"),
        other => panic!("unknown transport: {other}"),
    }
}
