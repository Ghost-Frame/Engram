mod source;
mod tables;
mod target;
mod validate;
mod vectors;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "engram-migrate")]
#[command(about = "ETL tool to migrate Engram from libsql to rusqlite + LanceDB")]
struct Args {
    /// Path to source libsql database file
    #[arg(long)]
    source: PathBuf,

    /// Directory for target rusqlite db + lance/ subdirectory
    #[arg(long)]
    target: PathBuf,

    /// Report table counts and schema diffs without writing
    #[arg(long, default_value_t = false)]
    dry_run: bool,

    /// Copy relational data only, skip embedding extraction
    #[arg(long, default_value_t = false)]
    skip_vectors: bool,

    /// Force all rows to this user_id (for migrating a single-tenant source
    /// DB into a tenant shard owned by a different user). Affects both the
    /// memories.user_id column and the user_id field of lance vectors.
    #[arg(long)]
    override_user_id: Option<i64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard =
        kleos_lib::observability::init_tracing("engram-migrate", "kleos_migrate=info");

    let args = Args::parse();

    info!("engram-migrate starting");
    info!("Source: {:?}", args.source);
    info!("Target: {:?}", args.target);
    info!("Dry run: {}", args.dry_run);
    info!("Skip vectors: {}", args.skip_vectors);

    // Phase 1: Initialize
    info!("Phase 1: Initializing...");
    let source_db = source::open(&args.source).await?;

    if args.dry_run {
        info!("Dry run mode -- analyzing source database...");
        let table_info = source::analyze(&source_db).await?;
        for (table, count) in &table_info {
            info!("  {}: {} rows", table, count);
        }
        return Ok(());
    }

    let target_db = target::create(&args.target).await?;
    let lance_db = if !args.skip_vectors {
        Some(vectors::open_lance(&args.target).await?)
    } else {
        None
    };

    // Phase 2: Copy relational data
    info!("Phase 2: Copying relational data...");
    tables::copy_all(&source_db, &target_db, args.override_user_id).await?;

    // Phase 3: Extract vectors
    if let Some(ref lance) = lance_db {
        info!("Phase 3: Extracting vectors to LanceDB...");
        vectors::extract_and_insert(&source_db, lance, args.override_user_id).await?;
    } else {
        info!("Phase 3: Skipping vector extraction (--skip-vectors)");
    }

    // Phase 4: Rebuild FTS indexes
    info!("Phase 4: Rebuilding FTS indexes...");
    target::rebuild_fts(&target_db).await?;

    // Phase 5: Validate
    info!("Phase 5: Validating migration...");
    validate::run(&source_db, &target_db, lance_db.as_ref()).await?;

    // Phase 6: Stamp metadata
    info!("Phase 6: Stamping migration metadata...");
    target::stamp_metadata(&target_db, &args.source, &args.target).await?;

    info!("Migration complete!");
    Ok(())
}
