pub mod migrations;
pub mod schema;

use crate::Result;
use libsql::{Builder, Connection, Database as LibsqlDatabase};
use tracing::info;

pub struct Database {
    db: LibsqlDatabase,
    pub conn: Connection,
}

impl Database {
    /// Connect to a libsql database file, configure pragmas, enable WAL, create schema.
    pub async fn connect(db_path: &str) -> Result<Self> {
        let db = Builder::new_local(db_path).build().await?;
        let conn = db.connect()?;

        // SQLite pragmas for performance
        conn.execute("PRAGMA journal_mode = WAL", ()).await?;
        conn.execute("PRAGMA synchronous = NORMAL", ()).await?;
        conn.execute("PRAGMA cache_size = -64000", ()).await?; // 64MB
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        conn.execute("PRAGMA busy_timeout = 5000", ()).await?;
        conn.execute("PRAGMA temp_store = MEMORY", ()).await?;
        conn.execute("PRAGMA mmap_size = 268435456", ()).await?; // 256MB

        // Run schema migrations (idempotent)
        migrations::run_migrations(&conn).await?;

        info!("database connected: {}", db_path);

        Ok(Self { db, conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get a new connection from the database (for concurrent operations).
    pub fn new_connection(&self) -> Result<Connection> {
        Ok(self.db.connect()?)
    }
}
