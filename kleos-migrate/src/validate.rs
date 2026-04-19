use anyhow::{anyhow, Result};
use tracing::{error, info};

use crate::source::SourceDb;
use crate::target::TargetDb;
use crate::vectors::LanceDb;

/// Run validation checks after migration. Returns Err when any discrepancy is
/// detected so the caller aborts before stamping the target database as
/// complete. A stamped target that silently disagrees with its source is the
/// worst failure mode for a cut-over tool.
pub async fn run(source: &SourceDb, target: &TargetDb, lance: Option<&LanceDb>) -> Result<()> {
    info!("Running validation checks...");

    // Compare row counts for key tables
    let key_tables = [
        "users",
        "memories",
        "episodes",
        "messages",
        "conversations",
        "memory_links",
    ];

    let target_conn = target.conn.lock().await;
    let mut discrepancies: Vec<String> = Vec::new();

    for table in key_tables {
        let source_query = format!("SELECT COUNT(*) FROM \"{}\"", table);
        let source_count: i64 = {
            let stmt = source.conn.prepare(&source_query).await?;
            let mut rows = stmt.query(()).await?;
            if let Some(row) = rows.next().await? {
                row.get(0).unwrap_or(0)
            } else {
                0
            }
        };

        let target_count: i64 = target_conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", table), [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        if source_count != target_count {
            let msg = format!(
                "{}: source={} target={} (MISMATCH)",
                table, source_count, target_count
            );
            error!("{}", msg);
            discrepancies.push(msg);
        } else {
            info!("{}: {} rows OK", table, target_count);
        }
    }

    if let Some(lance) = lance {
        let source_vec_query = "SELECT COUNT(*) FROM memories WHERE embedding_vec_1024 IS NOT NULL";
        let stmt = source.conn.prepare(source_vec_query).await?;
        let mut rows = stmt.query(()).await?;
        let source_vec_count: i64 = if let Some(row) = rows.next().await? {
            row.get(0).unwrap_or(0)
        } else {
            0
        };

        let table = lance.db.open_table("memory_vectors").execute().await?;
        let lance_count = table.count_rows(None).await? as i64;

        if source_vec_count != lance_count {
            let msg = format!(
                "Vectors: source={} lance={} (MISMATCH)",
                source_vec_count, lance_count
            );
            error!("{}", msg);
            discrepancies.push(msg);
        } else {
            info!("Vectors: {} OK", lance_count);
        }
    }

    // Spot check: verify 10 random memories match. Any mismatch here is as
    // serious as a row-count mismatch and must fail the migration.
    info!("Spot-checking random memories...");
    let spot_check_query = "SELECT id, content FROM memories ORDER BY RANDOM() LIMIT 10";
    let stmt = source.conn.prepare(spot_check_query).await?;
    let mut rows = stmt.query(()).await?;

    let mut checked = 0;
    let mut matched = 0;
    let mut mismatched_ids: Vec<i64> = Vec::new();

    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let source_content: String = row.get(1)?;

        let target_content: Option<String> = target_conn
            .query_row("SELECT content FROM memories WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .ok();

        checked += 1;
        if Some(&source_content) == target_content.as_ref() {
            matched += 1;
        } else {
            error!("Memory {} content mismatch", id);
            mismatched_ids.push(id);
        }
    }

    info!("Spot check: {}/{} memories matched", matched, checked);
    if !mismatched_ids.is_empty() {
        discrepancies.push(format!(
            "Spot check: {}/{} memories content-mismatched (ids: {:?})",
            mismatched_ids.len(),
            checked,
            mismatched_ids
        ));
    }

    if !discrepancies.is_empty() {
        let summary = discrepancies.join("; ");
        return Err(anyhow!(
            "migration validation failed with {} discrepancy(ies): {}",
            discrepancies.len(),
            summary
        ));
    }

    info!("Validation passed!");
    Ok(())
}
