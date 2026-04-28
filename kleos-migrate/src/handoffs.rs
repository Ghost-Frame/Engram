//! Handoffs ETL: copy rows from a legacy standalone handoffs.db into a
//! tenant shard's handoffs table (tenant schema v43).
//!
//! The legacy DB at `data_dir/handoffs.db` is plaintext SQLite (NOT
//! SQLCipher) and has the schema in `kleos_lib::handoffs::SCHEMA_SQL`
//! (pre-refactor) including the runtime-applied `user_id` column. The
//! target shard's handoffs table was created by the v43 migration and
//! has the same column set, so we copy by named columns and let the
//! v43 AFTER-INSERT trigger populate `handoffs_fts`.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::path::Path;
use tracing::{info, warn};

use crate::target::{self, TargetDb};

/// Columns we expect in both the legacy `handoffs.db` and the v43 tenant
/// `handoffs` table. `id` is excluded so AUTOINCREMENT generates fresh
/// rowids on the target.
const HANDOFFS_COLUMNS: &[&str] = &[
    "user_id",
    "created_at",
    "project",
    "branch",
    "directory",
    "agent",
    "type",
    "content",
    "metadata",
    "session_id",
    "model",
    "host",
    "content_hash",
];

/// Copy filtered handoff rows from the legacy plaintext `handoffs.db` into
/// the target tenant's `handoffs` table. Returns the number of rows
/// inserted.
pub fn copy(handoffs_source: &Path, target: &TargetDb, filter_user_id: i64) -> Result<usize> {
    let src = Connection::open(handoffs_source)
        .with_context(|| format!("open handoffs source {:?}", handoffs_source))?;

    let src_cols = source_columns(&src)?;
    if !src_cols.iter().any(|c| c == "user_id") {
        warn!(
            "legacy handoffs source {:?} has no user_id column; nothing to migrate for user_id={}",
            handoffs_source, filter_user_id
        );
        return Ok(0);
    }

    let usable: Vec<&str> = HANDOFFS_COLUMNS
        .iter()
        .copied()
        .filter(|c| src_cols.iter().any(|s| s == c))
        .collect();
    if usable.is_empty() {
        return Err(anyhow!(
            "no overlap between legacy handoffs schema {:?} and v43 column list",
            src_cols
        ));
    }

    let col_list = usable
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=usable.len())
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(", ");

    let dst = target::raw_conn(target)?;
    dst.execute_batch("PRAGMA foreign_keys = OFF;")?;

    let select_sql = format!("SELECT {} FROM handoffs WHERE user_id = ?1", col_list);
    let insert_sql = format!(
        "INSERT INTO handoffs ({}) VALUES ({})",
        col_list, placeholders
    );

    let mut select = src.prepare(&select_sql)?;
    let mut rows = select.query(rusqlite::params![filter_user_id])?;
    let mut insert = dst.prepare(&insert_sql)?;

    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let mut values: Vec<rusqlite::types::Value> = Vec::with_capacity(usable.len());
        for i in 0..usable.len() {
            values.push(row.get(i)?);
        }
        insert.execute(rusqlite::params_from_iter(values.iter()))?;
        count += 1;
    }

    info!(
        "copied {} handoffs row(s) from {:?} for user_id={}",
        count, handoffs_source, filter_user_id
    );
    Ok(count)
}

/// Pre-flight count for `--dry-run`. Returns 0 with a warning if the
/// source lacks a user_id column.
pub fn dry_run_count(handoffs_source: &Path, filter_user_id: i64) -> Result<i64> {
    let src = Connection::open(handoffs_source)
        .with_context(|| format!("open handoffs source {:?}", handoffs_source))?;

    let src_cols = source_columns(&src)?;
    if !src_cols.iter().any(|c| c == "user_id") {
        warn!(
            "legacy handoffs source {:?} has no user_id column; reporting 0",
            handoffs_source
        );
        return Ok(0);
    }

    let count: i64 = src.query_row(
        "SELECT COUNT(*) FROM handoffs WHERE user_id = ?1",
        rusqlite::params![filter_user_id],
        |r| r.get(0),
    )?;
    Ok(count)
}

fn source_columns(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(\"handoffs\")")?;
    let mut rows = stmt.query([])?;
    let mut cols = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        cols.push(name);
    }
    if cols.is_empty() {
        return Err(anyhow!("handoffs table missing in source DB"));
    }
    Ok(cols)
}
