mod source;
mod tables;
mod target;
mod validate;
mod vectors;

use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "kleos-migrate")]
#[command(about = "ETL tool to copy an encrypted SQLCipher monolith into a per-tenant shard")]
struct Args {
    /// Absolute path to source monolith .db file
    #[arg(long)]
    source: PathBuf,

    /// Name of env var holding SQLCipher raw hex key (default: ENGRAM_DB_KEY).
    /// If the env var is unset or empty, source is opened as plaintext.
    #[arg(long, default_value = "ENGRAM_DB_KEY")]
    source_key_env: String,

    /// Tenant shard output directory (will contain kleos.db and hnsw/memories.lance/)
    #[arg(long)]
    target: PathBuf,

    /// Only copy rows where user_id = FILTER_USER_ID
    #[arg(long)]
    filter_user_id: i64,

    /// Allow writing into a non-empty target directory
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard =
        kleos_lib::observability::init_tracing("kleos-migrate", "kleos_migrate=info");

    let args = Args::parse();

    info!("kleos-migrate starting");
    info!("Source:         {:?}", args.source);
    info!("Source key env: {}", args.source_key_env);
    info!("Target:         {:?}", args.target);
    info!("Filter user_id: {}", args.filter_user_id);
    info!("Force:          {}", args.force);

    // Safety check: refuse to overwrite a non-empty target unless --force.
    if args.target.exists() && !args.force {
        let mut entries = std::fs::read_dir(&args.target)?;
        if let Some(_first) = entries.next() {
            return Err(anyhow!(
                "target directory {:?} is not empty; use --force to allow overwriting",
                args.target
            ));
        }
    }

    // Phase 1: open source.
    info!("Phase 1: Opening source database...");
    let source = source::open(&args.source, Some(args.source_key_env.as_str()))?;

    // Phase 2: open / initialize target.
    info!("Phase 2: Opening target tenant shard...");
    let target = target::open(&args.target).await?;

    // Phase 3: copy relational tables.
    info!("Phase 3: Copying relational tables...");
    let counts = tables::copy_all(&source, &target, args.filter_user_id).await?;

    // Phase 4: extract and write vectors.
    info!("Phase 4: Extracting vectors to LanceDB...");
    let lance = vectors::open_lance(&args.target).await?;
    vectors::extract_and_insert(&source, &lance, args.filter_user_id).await?;

    // Phase 5: validate.
    info!("Phase 5: Validating...");
    validate::run(&source, &target, args.filter_user_id).await?;

    // Print per-table summary.
    println!("\n=== Migration summary ===");
    println!("{:<40} {:>10}", "Table", "Rows copied");
    println!("{}", "-".repeat(52));
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|(t, _)| t.as_str());
    for (table, n) in &sorted {
        println!("{:<40} {:>10}", table, n);
    }
    let total: usize = counts.values().sum();
    println!("{}", "-".repeat(52));
    println!("{:<40} {:>10}", "TOTAL", total);

    info!("kleos-migrate complete");
    Ok(())
}
