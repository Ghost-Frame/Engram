use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use crate::source::{self, SourceDb};
use crate::target::TargetDb;

/// Tables to skip during migration
const SKIP_TABLES: &[&str] = &[
    "rate_limits",         // ephemeral
    "schema_version",      // target gets fresh one
    "schema_versions",     // target gets fresh one
    "vector_sync_pending", // stale after migration
    "app_state",           // fresh bootstrap on target
    "sqlite_sequence",     // auto-managed
    // FTS virtual tables (rebuilt via triggers)
    "memories_fts",
    "memories_fts_data",
    "memories_fts_idx",
    "memories_fts_docsize",
    "memories_fts_content",
    "memories_fts_config",
    "episodes_fts",
    "episodes_fts_data",
    "episodes_fts_idx",
    "episodes_fts_docsize",
    "episodes_fts_content",
    "episodes_fts_config",
    "messages_fts",
    "messages_fts_data",
    "messages_fts_idx",
    "messages_fts_docsize",
    "messages_fts_content",
    "messages_fts_config",
    "skills_fts",
    "skills_fts_data",
    "skills_fts_idx",
    "skills_fts_docsize",
    "skills_fts_content",
    "skills_fts_config",
    "artifacts_fts",
    "artifacts_fts_data",
    "artifacts_fts_idx",
    "artifacts_fts_docsize",
    "artifacts_fts_content",
    "artifacts_fts_config",
    // libsql vector index internals
    "libsql_vector_meta_shadow",
];

/// Columns to skip (vector/embedding columns -- will be extracted to LanceDB)
const SKIP_COLUMNS: &[&str] = &["embedding", "embedding_vec_1024", "embedding_vec_1536"];

/// FK-ordered tables (copy these first in order)
const FK_ORDERED_TABLES: &[&str] = &[
    "users",
    "spaces",
    "memories",
    "episodes",
    "conversations",
    "agents",
    "projects",
];

/// Copy all tables from source to target
pub async fn copy_all(
    source: &SourceDb,
    target: &TargetDb,
    override_user_id: Option<i64>,
) -> Result<()> {
    let all_tables = source::get_tables(source).await?;

    // Disable FK checks during migration (legacy data may have orphaned refs)
    {
        let conn = target.conn.lock().await;
        conn.execute("PRAGMA foreign_keys = OFF", [])?;
    }

    // Build copy order: FK-ordered tables first, then alphabetical
    let mut copy_order: Vec<String> = Vec::new();

    for table in FK_ORDERED_TABLES {
        if all_tables.contains(&table.to_string()) {
            copy_order.push(table.to_string());
        }
    }

    for table in &all_tables {
        if !copy_order.contains(table) && !should_skip(table) {
            copy_order.push(table.clone());
        }
    }

    info!("Copying {} tables...", copy_order.len());

    for table in &copy_order {
        copy_table(source, target, table, override_user_id).await?;
    }

    // Re-enable FK checks and verify the copied data is still referentially
    // consistent. PRAGMA foreign_key_check surfaces orphans that survived the
    // OFF window; failing loud here prevents a stamped-but-broken target from
    // reaching production.
    {
        let conn = target.conn.lock().await;
        conn.execute("PRAGMA foreign_keys = ON", [])?;

        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        let mut violations: Vec<String> = Vec::new();
        while let Some(row) = rows.next()? {
            let table: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
            let rowid: Option<i64> = row.get(1).ok();
            let parent: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
            let fkid: Option<i64> = row.get(3).ok();
            violations.push(format!(
                "child={} rowid={:?} parent={} fkid={:?}",
                table, rowid, parent, fkid
            ));
            if violations.len() >= 10 {
                break;
            }
        }
        if !violations.is_empty() {
            anyhow::bail!(
                "foreign_key_check reported {} violation(s) after copy (showing up to 10): {}",
                violations.len(),
                violations.join("; ")
            );
        }
    }

    Ok(())
}

fn should_skip(table: &str) -> bool {
    SKIP_TABLES.contains(&table)
        || table.contains("_shadow")
        || table.contains("_idx")
        || table.ends_with("_fts")
}

/// Get target table columns
fn get_target_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\")", table))?;
    let mut rows = stmt.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        columns.push(name);
    }
    Ok(columns)
}

/// Get required columns (NOT NULL without default, excluding PRIMARY KEY which auto-fills)
fn get_required_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\")", table))?;
    let mut rows = stmt.query([])?;
    let mut required = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        let notnull: i32 = row.get(3)?;
        let dflt_value: Option<String> = row.get(4)?;
        let pk: i32 = row.get(5)?;
        // Required = NOT NULL + no default + not primary key (pk handles itself)
        if notnull == 1 && dflt_value.is_none() && pk == 0 {
            required.push(name);
        }
    }
    Ok(required)
}

/// Copy a single table from source to target
async fn copy_table(
    source: &SourceDb,
    target: &TargetDb,
    table: &str,
    override_user_id: Option<i64>,
) -> Result<()> {
    // Get source columns
    let source_cols = source::get_columns(source, table).await?;

    // Get target columns and required columns
    let (target_cols, required_cols) = {
        let conn = target.conn.lock().await;
        let tc = match get_target_columns(&conn, table) {
            Ok(cols) => cols,
            Err(_) => {
                info!("Skipping {} (table not in target schema)", table);
                return Ok(());
            }
        };
        let rc = get_required_columns(&conn, table).unwrap_or_default();
        (tc, rc)
    };

    if target_cols.is_empty() {
        info!("Skipping {} (table not in target schema)", table);
        return Ok(());
    }

    // Intersect: only copy columns that exist in BOTH source and target
    // Also filter out vector columns
    let mut cols: Vec<String> = source_cols
        .into_iter()
        .filter(|c| !SKIP_COLUMNS.contains(&c.as_str()))
        .filter(|c| target_cols.contains(c))
        .collect();

    if cols.is_empty() {
        info!("Skipping {} (no common columns)", table);
        return Ok(());
    }

    // If --override-user-id is set and the target has a user_id column we
    // want to force the override on every row regardless of source value.
    // Two cases:
    //   - source lacks user_id: inject it as an appended column and use the
    //     override as the value (source row never touches that column).
    //   - source has user_id: leave cols alone, just replace the value in
    //     each row with the override at the existing position.
    let source_col_count = cols.len();
    let target_has_user_id = target_cols.iter().any(|c| c == "user_id");
    let inject_user_id = override_user_id.is_some()
        && target_has_user_id
        && !cols.iter().any(|c| c == "user_id");
    if inject_user_id {
        cols.push("user_id".to_string());
    }

    // Check if all required target columns are present in source
    // (user_id is satisfied either by source or by the injected override)
    let missing_required: Vec<&String> = required_cols
        .iter()
        .filter(|c| !cols.contains(c))
        .collect();

    if !missing_required.is_empty() {
        info!(
            "Skipping {} (missing required columns: {:?})",
            table, missing_required
        );
        return Ok(());
    }

    // Count rows (using MAX(rowid) as proxy)
    let count_query = format!("SELECT COUNT(*) FROM \"{}\"", table);
    let stmt = source.conn.prepare(&count_query).await?;
    let mut rows = stmt.query(()).await?;
    let total: i64 = if let Some(row) = rows.next().await? {
        row.get(0).unwrap_or(0)
    } else {
        0
    };

    if total == 0 {
        info!("Skipping {} (empty)", table);
        return Ok(());
    }

    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
            )?
            .progress_chars("#>-"),
    );
    pb.set_message(table.to_string());

    // Build column lists for INSERT and SELECT separately.
    // INSERT covers every column we write (including any injected user_id).
    // SELECT only covers source columns, since the injected column value
    // comes from the --override-user-id flag, not the source row.
    let insert_col_list = cols
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");
    let select_col_list = cols[..source_col_count]
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!("SELECT {} FROM \"{}\"", select_col_list, table);
    let stmt = source.conn.prepare(&query).await?;
    let mut rows = stmt.query(()).await?;

    // Position of user_id in the source row (and INSERT cols when not
    // injecting) so we can replace the value in-place. When we inject a
    // user_id column, no source position exists.
    let user_id_source_idx = if inject_user_id {
        None
    } else if override_user_id.is_some() && target_has_user_id {
        cols.iter().position(|c| c == "user_id")
    } else {
        None
    };

    let conn = target.conn.lock().await;

    // Prepare insert statement (use INSERT OR REPLACE to handle seed data conflicts)
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{}", i)).collect();
    let insert_sql = format!(
        "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
        table,
        insert_col_list,
        placeholders.join(", ")
    );

    let mut insert_stmt = conn.prepare_cached(&insert_sql)?;
    let mut batch_count = 0;
    const BATCH_SIZE: usize = 5000;

    conn.execute("BEGIN TRANSACTION", [])?;

    while let Some(row) = rows.next().await? {
        // Convert libsql row to rusqlite values. We read `source_col_count`
        // columns from the source row, then append the override user_id if
        // we're injecting it.
        let mut values: Vec<rusqlite::types::Value> = Vec::new();

        for i in 0..source_col_count {
            let value = convert_value(&row, i as i32)?;
            values.push(value);
        }

        if let Some(override_uid) = override_user_id {
            if inject_user_id {
                values.push(rusqlite::types::Value::Integer(override_uid));
            } else if let Some(idx) = user_id_source_idx {
                values[idx] = rusqlite::types::Value::Integer(override_uid);
            }
        }

        let params: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        insert_stmt.execute(params.as_slice())?;

        batch_count += 1;
        pb.inc(1);

        if batch_count >= BATCH_SIZE {
            conn.execute("COMMIT", [])?;
            conn.execute("BEGIN TRANSACTION", [])?;
            batch_count = 0;
        }
    }

    conn.execute("COMMIT", [])?;
    pb.finish_with_message(format!("{} done", table));

    Ok(())
}

/// Convert libsql value to rusqlite value
fn convert_value(row: &libsql::Row, idx: i32) -> Result<rusqlite::types::Value> {
    // Get the value as a libsql::Value enum to avoid panics on unknown types
    let value: libsql::Value = row.get(idx)?;

    Ok(match value {
        libsql::Value::Null => rusqlite::types::Value::Null,
        libsql::Value::Integer(v) => rusqlite::types::Value::Integer(v),
        libsql::Value::Real(v) => rusqlite::types::Value::Real(v),
        libsql::Value::Text(v) => rusqlite::types::Value::Text(v),
        libsql::Value::Blob(v) => rusqlite::types::Value::Blob(v),
    })
}
