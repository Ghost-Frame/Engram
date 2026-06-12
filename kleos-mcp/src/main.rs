use clap::Parser;

/// CLI flags for the kleos-mcp binary.
#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "stdio")]
    transport: String,
    #[cfg(feature = "http")]
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: String,
}

/// Binary entry point: parse args, build the App, run the chosen transport.
#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard =
        kleos_lib::observability::init_tracing("kleos-mcp", "kleos_mcp=info,warn,yubikey=off");

    let args = Args::parse();
    let app = match kleos_mcp::App::from_env() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kleos-mcp failed to start: {e}");
            std::process::exit(1);
        }
    };

    let result = match args.transport.as_str() {
        "stdio" => kleos_mcp::transport::stdio::serve(app).await,
        #[cfg(feature = "http")]
        "http" => kleos_mcp::transport::http::serve(app, &args.listen).await,
        other => {
            eprintln!("kleos-mcp: unknown transport '{other}' (expected 'stdio' or 'http')");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("kleos-mcp: {} transport failed: {e}", args.transport);
        std::process::exit(1);
    }
}
