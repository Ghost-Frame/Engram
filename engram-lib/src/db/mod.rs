pub mod migrations;
pub mod schema;

use crate::config::Config;
use crate::vector::{LanceIndex, VectorIndex};
use crate::Result;
use libsql::{Builder, Connection, Database as LibsqlDatabase};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub struct Database {
    db: LibsqlDatabase,
    pub conn: Connection,
    pub vector_index: Option<Arc<dyn VectorIndex>>,
}

impl Database {
    /// Connect to a libsql database file, configure pragmas, enable WAL, create schema.
    pub async fn connect(db_path: &str) -> Result<Self> {
        let mut config = Config::from_env();
        config.db_path = db_path.to_string();
        Self::connect_with_config(&config).await
    }

    pub async fn connect_with_config(config: &Config) -> Result<Self> {
        let db_path = &config.db_path;
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

        let vector_index = if config.use_lance_index {
            let lance_path = config
                .lance_index_path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&config.data_dir).join("lance").to_string_lossy().into_owned());

            match LanceIndex::open(&lance_path, config.vector_dimensions).await {
                Ok(index) => {
                    info!("LanceDB vector index connected: {}", lance_path);
                    Some(Arc::new(index) as Arc<dyn VectorIndex>)
                }
                Err(e) => {
                    warn!("LanceDB vector index unavailable, falling back to libsql vectors: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { db, conn, vector_index })
    }

    /// Connect to an in-memory database for testing.
    /// Skips WAL and mmap PRAGMAs that are invalid for in-memory databases.
    pub async fn connect_memory() -> Result<Self> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;

        conn.execute("PRAGMA foreign_keys = ON", ()).await?;

        migrations::run_migrations(&conn).await?;

        Ok(Self { db, conn, vector_index: None })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get a new connection from the database (for concurrent operations).
    pub fn new_connection(&self) -> Result<Connection> {
        Ok(self.db.connect()?)
    }
}
