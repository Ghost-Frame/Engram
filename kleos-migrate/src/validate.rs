use anyhow::{anyhow, Result};
use tracing::info;

use crate::source::{self, SourceDb, SKIP_TABLES};
use crate::target::{self, TargetDb};
use crate::vectors::VectorStats;

/// Validate that row counts in the target match the source (filtered by user_id).
pub async fn run(
    source: &SourceDb,
    target: &TargetDb,
    filter_user_id: i64,
    vector_stats: VectorStats,
) -> Result<()> {
    info!("Running validation checks...");

    let conn = target::raw_conn(target)?;
    let all_source_tables = source::get_tables(source)?;

    let mut mismatches: Vec<String> = Vec::new();
    let mut verified = 0usize;

    for table in &all_source_tables {
        // Only validate tables that exist in both sides.
        if SKIP_TABLES.contains(&table.as_str()) || source::should_skip(table) {
            continue;
        }
        if !target::table_exists(&conn, table)? {
            continue;
        }

        let source_cols = source::get_columns(source, table)?;
        let has_user_id = source_cols.iter().any(|c| c == "user_id");

        let source_count: i64 = if has_user_id {
            source.conn.query_row(
                &format!("SELECT COUNT(*) FROM \"{}\" WHERE user_id = ?1", table),
                rusqlite::params![filter_user_id],
                |row| row.get(0),
            )?
        } else {
            source.conn.query_row(
                &format!("SELECT COUNT(*) FROM \"{}\"", table),
                [],
                |row| row.get(0),
            )?
        };

        let target_count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM \"{}\"", table),
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if source_count != target_count {
            let msg = format!(
                "{}: source={} target={} (MISMATCH)",
                table, source_count, target_count
            );
            tracing::error!("{}", msg);
            mismatches.push(msg);
        } else {
            info!("{}: {} rows OK", table, target_count);
        }
        verified += 1;
    }

    if !mismatches.is_empty() {
        return Err(anyhow!(
            "count mismatch: {}",
            mismatches.join("; ")
        ));
    }

    // Catch the silent-drop case: source had eligible embeddings, target has
    // zero vectors. A mismatch here means the blob decode filtered out every
    // row and we'd ship a tenant shard with no vector search coverage.
    if vector_stats.source_eligible > 0 && vector_stats.inserted == 0 {
        return Err(anyhow!(
            "vector extraction produced zero rows despite {} eligible source embeddings: \
             likely every blob failed the 4096-byte decode check",
            vector_stats.source_eligible
        ));
    }
    if vector_stats.source_eligible > 0 {
        info!(
            "vectors: {} / {} copied into LanceDB",
            vector_stats.inserted, vector_stats.source_eligible
        );
    }

    info!("validation ok: {} tables verified", verified);
    Ok(())
}
