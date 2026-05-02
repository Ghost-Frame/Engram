use clap::Parser;
use tracing_subscriber::EnvFilter;

mod config;
mod extractor;
mod ledger;
mod summarizer;
mod tailer;
mod watcher;
mod writer;

#[derive(Parser)]
#[command(name = "kleos-ingest", about = "Transcript ingest daemon for Kleos")]
struct Cli {
    #[arg(long, help = "Print ledger stats and exit")]
    status: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config = config::Config::from_env();

    if cli.status {
        match ledger::Ledger::open(&config.ledger_path) {
            Ok(ledger) => ledger.print_stats(),
            Err(e) => {
                eprintln!("Failed to open ledger: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let mut config = config;
    if config.api_key.is_none() {
        let slot = kleos_lib::cred::bootstrap::current_agent_slot();
        match kleos_lib::cred::bootstrap::resolve_api_key(&slot).await {
            Ok(key) => {
                tracing::info!(slot = %slot, "resolved kleos API key from credd");
                config.api_key = Some(key);
            }
            Err(e) => {
                tracing::error!(slot = %slot, error = %e, "failed to resolve kleos API key -- stores will fail");
            }
        }
    }

    tracing::info!(
        watch_dir = %config.watch_dir.display(),
        kleos_url = %config.kleos_url,
        "kleos-ingest starting"
    );

    let ledger = ledger::Ledger::open(&config.ledger_path)
        .expect("failed to open ledger database");
    let writer = writer::KleosWriter::new(&config);

    watcher::run(config, ledger, writer).await;
}
