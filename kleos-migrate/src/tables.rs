use anyhow::Result;
use std::collections::HashMap;
use tracing::info;

use crate::source::{self, SourceDb};
use crate::target::{self, TargetDb};

/// Columns containing raw vector blobs -- extracted to LanceDB separately.
const SKIP_COLUMNS: &[&str] = &["embedding", "embedding_vec_1024", "embedding_vec_1536"];

/// FK-stable ordering: copy parents before children.
const FK_ORDERED_TABLES: &[&str] = &[
    "users",
    "spaces",
    "memories",
    "episodes",
    "conversations",
    "agents",
    "projects",
];

/// Copy all eligible tables from source into target, filtered by `filter_user_id`.
/// Returns a map of table name -> rows inserted.
pub async fn copy_all(
    source: &SourceDb,
    target: &TargetDb,
    filter_user_id: i64,
) -> Result<HashMap<String, usize>> {
    let conn = target::raw_conn(target)?;

    // Disable FK checks -- migrating a subset of rows will temporarily break refs.
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

    let all_tables = source::get_tables(source)?;

    // Build copy order: FK_ORDERED_TABLES first (if present), then remaining.
    let mut copy_order: Vec<String> = Vec::new();
    for &t in FK_ORDERED_TABLES {
        if all_tables.contains(&t.to_string()) {
            copy_order.push(t.to_string());
        }
    }
    for t in &all_tables {
        if !copy_order.contains(t) {
            copy_order.push(t.clone());
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();

    for table in &copy_order {
        if source::should_skip(table) {
            continue;
        }

        // Skip tables absent from the target schema.
        if !target::table_exists(&conn, table)? {
            info!("skip table {}: not in target schema", table);
            continue;
        }

        let n = copy_table(source, &conn, table, filter_user_id)?;
        info!("copied {} rows into {}", n, table);
        counts.insert(table.clone(), n);
    }

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(counts)
}

fn copy_table(
    source: &SourceDb,
    conn: &rusqlite::Connection,
    table: &str,
    filter_user_id: i64,
) -> Result<usize> {
    // Compute column intersection (source cols that exist in target, minus vector cols).
    let source_cols = source::get_columns(source, table)?;
    let target_cols = target::get_target_columns(conn, table)?;

    if target_cols.is_empty() {
        info!("skip table {}: empty target schema (no columns)", table);
        return Ok(0);
    }

    let intersection: Vec<String> = source_cols
        .iter()
        .filter(|c| !SKIP_COLUMNS.contains(&c.as_str()))
        .filter(|c| target_cols.contains(c))
        .cloned()
        .collect();

    if intersection.is_empty() {
        info!("skip table {}: no common columns", table);
        return Ok(0);
    }

    // Apply the user_id filter on the source whenever the source has the
    // column, even if the target schema (Phase 5.1.2 tenant shards) no
    // longer carries it. The column itself is not part of `intersection`
    // when target lacks it, so the SELECT/INSERT column lists stay correct;
    // we just need the WHERE clause to scope source rows to the operator.
    let has_user_id = source_cols.iter().any(|c| c == "user_id");

    // Build SELECT from source.
    let select_cols = intersection
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");

    let select_sql = if has_user_id {
        format!(
            "SELECT {} FROM \"{}\" WHERE user_id = ?1",
            select_cols, table
        )
    } else {
        format!("SELECT {} FROM \"{}\"", select_cols, table)
    };

    // Build INSERT into target.
    let insert_cols = intersection
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=intersection.len())
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
        table, insert_cols, placeholders
    );

    let mut src_stmt = source.conn.prepare(&select_sql)?;

    let mut rows = if has_user_id {
        src_stmt.query(rusqlite::params![filter_user_id])?
    } else {
        src_stmt.query([])?
    };

    let tx = conn.unchecked_transaction()?;
    let mut insert_stmt = conn.prepare_cached(&insert_sql)?;
    let mut count = 0usize;

    while let Some(row) = rows.next()? {
        let mut values: Vec<rusqlite::types::Value> = Vec::with_capacity(intersection.len());
        for i in 0..intersection.len() {
            values.push(row.get::<_, rusqlite::types::Value>(i)?);
        }
        let params: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        insert_stmt.execute(params.as_slice())?;
        count += 1;
    }

    drop(insert_stmt);
    tx.commit()?;

    Ok(count)
}
