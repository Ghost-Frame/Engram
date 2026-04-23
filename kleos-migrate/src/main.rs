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

    /// Path to an existing source LanceDB directory (or the `.lance` table
    /// inside it). When set, vectors are read from this LanceDB and filtered
    /// by user_id, not from `memories.embedding_vec_1024`. Use when the
    /// monolith stores embeddings in LanceDB rather than the SQL blob column.
    #[arg(long)]
    source_lance: Option<PathBuf>,

    /// Allow writing into a non-empty target directory
    #[arg(long, default_value_t = false)]
    force: bool,

    /// Report per-table source-filtered counts and exit without writing.
    /// Does not touch the target directory. Useful as a pre-cutover dry run.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard = kleos_lib::observability::init_tracing("kleos-migrate", "kleos_migrate=info");

    let args = Args::parse();

    info!("kleos-migrate starting");
    info!("Source:         {:?}", args.source);
    info!("Source key env: {}", args.source_key_env);
    info!("Target:         {:?}", args.target);
    info!("Filter user_id: {}", args.filter_user_id);
    info!("Force:          {}", args.force);
    info!("Dry run:        {}", args.dry_run);

    // Phase 1: open source (always, also in dry-run).
    info!("Phase 1: Opening source database...");
    let source = source::open(&args.source, Some(args.source_key_env.as_str()))?;

    if args.dry_run {
        return dry_run_report(&source, args.filter_user_id, args.source_lance.as_deref()).await;
    }

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

    // Phase 2: open / initialize target.
    info!("Phase 2: Opening target tenant shard...");
    let target = target::open(&args.target).await?;

    // Phase 3: copy relational tables.
    info!("Phase 3: Copying relational tables...");
    let counts = tables::copy_all(&source, &target, args.filter_user_id).await?;

    // Phase 4: extract and write vectors.
    info!("Phase 4: Extracting vectors to LanceDB...");
    let lance = vectors::open_lance(&args.target).await?;
    let vector_stats = if let Some(src_lance) = args.source_lance.as_deref() {
        vectors::extract_from_source_lance(src_lance, &lance, args.filter_user_id).await?
    } else {
        vectors::extract_and_insert(&source, &lance, args.filter_user_id).await?
    };

    // Phase 5: validate.
    info!("Phase 5: Validating...");
    validate::run(&source, &target, args.filter_user_id, vector_stats).await?;

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

/// Read-only pre-flight: report per-table source-filtered row counts and
/// total embedding rows without touching the target directory or LanceDB.
/// The target arg is accepted but ignored in dry-run mode.
async fn dry_run_report(
    source: &source::SourceDb,
    filter_user_id: i64,
    source_lance: Option<&std::path::Path>,
) -> Result<()> {
    info!("DRY RUN: reporting source-side counts only; target untouched");

    let tables = source::get_tables(source)?;
    println!("\n=== Dry run: source-filtered counts ===");
    println!("{:<40} {:>10}", "Table", "Rows (filtered)");
    println!("{}", "-".repeat(52));
    let mut total = 0i64;
    let mut rows: Vec<(String, i64)> = Vec::new();
    for table in &tables {
        let cols = source::get_columns(source, table)?;
        let has_user_id = cols.iter().any(|c| c == "user_id");
        let count: i64 = if has_user_id {
            source.conn.query_row(
                &format!("SELECT COUNT(*) FROM \"{}\" WHERE user_id = ?1", table),
                rusqlite::params![filter_user_id],
                |r| r.get(0),
            )?
        } else {
            source
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", table), [], |r| {
                    r.get(0)
                })?
        };
        rows.push((table.clone(), count));
        total += count;
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (table, count) in &rows {
        println!("{:<40} {:>10}", table, count);
    }
    println!("{}", "-".repeat(52));
    println!("{:<40} {:>10}", "TOTAL (non-vector rows)", total);

    // Embedding preview: count eligible source rows with a vector blob.
    let cols = source::get_columns(source, "memories")?;
    let has_user_id = cols.iter().any(|c| c == "user_id");
    let has_vec = cols.iter().any(|c| c == "embedding_vec_1024");
    if has_vec {
        let vec_count: i64 = if has_user_id {
            source.conn.query_row(
                "SELECT COUNT(*) FROM memories \
                 WHERE embedding_vec_1024 IS NOT NULL AND user_id = ?1",
                rusqlite::params![filter_user_id],
                |r| r.get(0),
            )?
        } else {
            source.conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE embedding_vec_1024 IS NOT NULL",
                [],
                |r| r.get(0),
            )?
        };
        println!("{:<40} {:>10}", "Embeddings (eligible)", vec_count);
    } else {
        println!("{:<40} {:>10}", "Embeddings (eligible, SQL)", "n/a");
    }

    // If a source LanceDB was given, count filtered rows there too.
    if let Some(src_lance) = source_lance {
        match vectors::dry_run_source_lance_count(src_lance, filter_user_id).await {
            Ok(count) => {
                println!("{:<40} {:>10}", "Embeddings (source LanceDB)", count);
            }
            Err(e) => {
                println!(
                    "{:<40} {:>10}",
                    format!("Embeddings (source LanceDB): ERROR {}", e),
                    ""
                );
            }
        }
    }

    info!("dry run complete");
    Ok(())
}
