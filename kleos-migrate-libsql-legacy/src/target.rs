use anyhow::Result;
use kleos_lib::db::schema_sql::{
    AUXILIARY_SCHEMA_STATEMENTS, CORE_SCHEMA_SQL, SYNTHEOS_SERVICES_SQL,
};
use rusqlite::Connection;
use std::path::Path;
use tokio::sync::Mutex;
use tracing::info;

pub struct TargetDb {
    pub conn: Mutex<Connection>,
}

/// Create target rusqlite database with schema
pub async fn create(target_dir: &Path) -> Result<TargetDb> {
    std::fs::create_dir_all(target_dir)?;
    let db_path = kleos_lib::config::resolve_db_path(&target_dir.join("kleos.db"));

    info!("Creating target database at {:?}", db_path);

    let conn = Connection::open(&db_path)?;

    // Enable WAL mode and pragmas
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-64000;
         PRAGMA busy_timeout=5000;",
    )?;

    // Run schema creation from engram-lib constants
    info!("Creating core schema...");
    conn.execute_batch(CORE_SCHEMA_SQL)?;

    info!("Creating auxiliary schema...");
    for stmt in AUXILIARY_SCHEMA_STATEMENTS {
        conn.execute_batch(stmt)?;
    }

    info!("Creating Syntheos services schema...");
    conn.execute_batch(SYNTHEOS_SERVICES_SQL)?;

    Ok(TargetDb {
        conn: Mutex::new(conn),
    })
}

/// Rebuild FTS indexes after data copy
pub async fn rebuild_fts(db: &TargetDb) -> Result<()> {
    let conn = db.conn.lock().await;

    let fts_tables = [
        "memories_fts",
        "episodes_fts",
        "messages_fts",
        "skills_fts",
        "artifacts_fts",
    ];

    for table in fts_tables {
        // Check if FTS table exists
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |row| row.get(0),
        )?;

        if exists {
            info!("Rebuilding FTS index: {}", table);
            let rebuild_sql = format!("INSERT INTO {}({}) VALUES('rebuild')", table, table);
            conn.execute(&rebuild_sql, [])?;
        }
    }

    Ok(())
}

/// Stamp migration metadata into app_state
pub async fn stamp_metadata(db: &TargetDb, source: &Path, target: &Path) -> Result<()> {
    let conn = db.conn.lock().await;

    let manifest = serde_json::json!({
        "source": source.to_string_lossy(),
        "target": target.to_string_lossy(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": "1.0"
    });

    conn.execute(
        "INSERT OR REPLACE INTO app_state (key, value, updated_at) VALUES (?, ?, datetime('now'))",
        rusqlite::params!["migration_manifest", manifest.to_string()],
    )?;

    info!("Migration metadata stamped");
    Ok(())
}
