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

    // Initialize PIV/software key request signer.
    // Falls back to KLEOS_API_KEY env / config.api_key when no key is available.
    let signer = match kleos_lib::auth_piv::RequestSigner::from_env_or_file(
        &config.host,
        "kleos-ingest",
        "daemon",
    ) {
        Ok(Some(s)) => {
            tracing::info!(tier = %s.tier(), fingerprint = %s.fingerprint(), "PIV/software key auth initialized");
            Some(s)
        }
        Ok(None) => {
            if config.api_key.is_some() {
                tracing::info!("no PIV/software key found, using KLEOS_API_KEY");
            } else {
                tracing::warn!("no PIV/software key and no API key -- stores will fail");
            }
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to initialize PIV/software key signer, falling back to API key");
            None
        }
    };

    tracing::info!(
        watch_dir = %config.watch_dir.display(),
        kleos_url = %config.kleos_url,
        "kleos-ingest starting"
    );

    let ledger = ledger::Ledger::open(&config.ledger_path)
        .expect("failed to open ledger database");
    let writer = writer::KleosWriter::new(&config, signer);

    watcher::run(config, ledger, writer).await;
}
