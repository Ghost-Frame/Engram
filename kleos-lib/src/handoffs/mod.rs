use crate::{EngError, Result};
use deadpool_sqlite::{Config as PoolManagerConfig, Hook, HookError, Pool, PoolConfig, Runtime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub id: i64,
    pub created_at: String,
    pub project: String,
    pub branch: Option<String>,
    pub directory: Option<String>,
    pub agent: String,
    #[serde(rename = "type")]
    pub handoff_type: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreParams {
    pub project: String,
    pub branch: Option<String>,
    pub directory: Option<String>,
    pub agent: Option<String>,
    #[serde(rename = "type")]
    pub handoff_type: Option<String>,
    pub content: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HandoffFilters {
    pub project: Option<String>,
    pub agent: Option<String>,
    #[serde(rename = "type")]
    pub handoff_type: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub host: Option<String>,
    pub since: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: i64,
    pub created_at: String,
    pub project: String,
    pub agent: String,
    #[serde(rename = "type")]
    pub handoff_type: String,
    pub model: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStats {
    pub name: String,
    pub count: i64,
    pub latest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStats {
    pub name: String,
    pub count: i64,
    pub latest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStats {
    pub name: String,
    pub count: i64,
    pub latest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeStats {
    pub name: String,
    pub count: i64,
    pub latest: String,
    pub total_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffStats {
    pub total: i64,
    pub total_content_bytes: i64,
    pub date_range: Option<(String, String)>,
    pub by_project: Vec<ProjectStats>,
    pub by_agent: Vec<AgentStats>,
    pub by_host: Vec<HostStats>,
    pub by_type: Vec<TypeStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreResult {
    pub id: Option<i64>,
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResult {
    pub deleted: i64,
    pub remaining: i64,
}

pub struct HandoffsDb {
    writer: Pool,
    reader: Pool,
}

impl HandoffsDb {
    pub async fn open(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            EngError::Internal(format!("failed to create handoffs data dir: {}", e))
        })?;

        let db_path = format!("{}/handoffs.db", data_dir);

        let writer = build_pool(&db_path, 1)?;
        let reader = build_pool(&db_path, 2)?;

        let db = Self { writer, reader };
        db.setup_schema().await?;

        info!("handoffs db opened: {}", db_path);
        Ok(db)
    }

    pub async fn store(&self, params: StoreParams) -> Result<StoreResult> {
        let handoff_type = params.handoff_type.clone().unwrap_or_else(|| "manual".to_string());
        let agent = params.agent.clone().unwrap_or_else(|| "unknown".to_string());
        let content_hash = compute_content_hash(&params.content, &handoff_type);
        let metadata_str = match &params.metadata {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };

        let ht = handoff_type.clone();
        let hash = content_hash.clone();
        let project = params.project.clone();

        if ht == "mechanical" {
            let hash2 = hash.clone();
            let project2 = project.clone();
            let conn = self.reader.get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;
            let exists: bool = conn
                .interact(move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM handoffs WHERE content_hash = ?1 AND project = ?2 AND type = 'mechanical'",
                        rusqlite::params![hash2, project2],
                        |row| row.get(0),
                    )?;
                    Ok::<bool, rusqlite::Error>(count > 0)
                })
                .await
                .map_err(|e| EngError::Internal(format!("handoffs reader interact failed: {e}")))?
                .map_err(|e: rusqlite::Error| EngError::Database(e))?;

            if exists {
                return Ok(StoreResult { id: None, skipped: true });
            }
        }

        let conn = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
        })?;

        let branch = params.branch.clone();
        let directory = params.directory.clone();
        let content = params.content.clone();
        let session_id = params.session_id.clone();
        let model = params.model.clone();
        let host = params.host.clone();
        let project2 = project.clone();

        let new_id: i64 = conn
            .interact(move |conn| {
                conn.execute(
                    "INSERT INTO handoffs (project, branch, directory, agent, type, content, metadata, session_id, model, host, content_hash)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        project2,
                        branch,
                        directory,
                        agent,
                        ht,
                        content,
                        metadata_str,
                        session_id,
                        model,
                        host,
                        hash,
                    ],
                )?;
                Ok::<i64, rusqlite::Error>(conn.last_insert_rowid())
            })
            .await
            .map_err(|e| EngError::Internal(format!("handoffs writer interact failed: {e}")))?
            .map_err(|e: rusqlite::Error| EngError::Database(e))?;

        let writer_clone = self.writer.clone();
        let project3 = project.clone();
        tokio::spawn(async move {
            let conn = match writer_clone.get().await {
                Ok(c) => c,
                Err(e) => {
                    error!("handoffs auto_gc writer acquire failed: {}", e);
                    return;
                }
            };
            let total: i64 = match conn
                .interact(|c| c.query_row("SELECT COUNT(*) FROM handoffs", [], |r| r.get(0)))
                .await
            {
                Ok(Ok(n)) => n,
                _ => return,
            };

            if total > 500 {
                let gc_conn = match writer_clone.get().await {
                    Ok(c) => c,
                    Err(e) => {
                        error!("handoffs auto_gc gc conn acquire failed: {}", e);
                        return;
                    }
                };
                if let Err(e) = gc_conn
                    .interact(move |c| run_tiered_gc(c, &project3))
                    .await
                {
                    error!("handoffs auto_gc failed: {}", e);
                }
            }
        });

        Ok(StoreResult { id: Some(new_id), skipped: false })
    }

    pub async fn list(&self, filters: HandoffFilters) -> Result<Vec<Handoff>> {
        let limit = filters.limit.unwrap_or(20);
        let conn = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
        })?;

        conn.interact(move |conn| {
            let mut conditions: Vec<String> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref p) = filters.project {
                conditions.push(format!("project = ?{}", params.len() + 1));
                params.push(Box::new(p.clone()));
            }
            if let Some(ref a) = filters.agent {
                conditions.push(format!("agent = ?{}", params.len() + 1));
                params.push(Box::new(a.clone()));
            }
            if let Some(ref t) = filters.handoff_type {
                conditions.push(format!("type = ?{}", params.len() + 1));
                params.push(Box::new(t.clone()));
            }
            if let Some(ref m) = filters.model {
                conditions.push(format!("model = ?{}", params.len() + 1));
                params.push(Box::new(m.clone()));
            }
            if let Some(ref s) = filters.session_id {
                conditions.push(format!("session_id = ?{}", params.len() + 1));
                params.push(Box::new(s.clone()));
            }
            if let Some(ref h) = filters.host {
                conditions.push(format!("host = ?{}", params.len() + 1));
                params.push(Box::new(h.clone()));
            }
            if let Some(ref since) = filters.since {
                conditions.push(format!("created_at >= ?{}", params.len() + 1));
                params.push(Box::new(since.clone()));
            }

            let where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            };

            let sql = format!(
                "SELECT id, created_at, project, branch, directory, agent, type, content, metadata, session_id, model, host, content_hash
                 FROM handoffs {} ORDER BY created_at DESC LIMIT {}",
                where_clause, limit
            );

            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_refs.as_slice(), |row| {
                let metadata_str: Option<String> = row.get(8)?;
                let metadata = metadata_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
                Ok(Handoff {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    project: row.get(2)?,
                    branch: row.get(3)?,
                    directory: row.get(4)?,
                    agent: row.get(5)?,
                    handoff_type: row.get(6)?,
                    content: row.get(7)?,
                    metadata,
                    session_id: row.get(9)?,
                    model: row.get(10)?,
                    host: row.get(11)?,
                    content_hash: row.get(12)?,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<Vec<Handoff>, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs list interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn get_latest(&self, filters: HandoffFilters) -> Result<Option<Handoff>> {
        let has_project = filters.project.is_some();
        let mut f = filters;
        f.limit = Some(1);

        let results = self.list(f.clone()).await?;
        if !results.is_empty() {
            return Ok(results.into_iter().next());
        }

        if has_project {
            let mut fallback = f;
            fallback.project = None;
            let results = self.list(fallback).await?;
            return Ok(results.into_iter().next());
        }

        Ok(None)
    }

    pub async fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SearchResult>> {
        let conn = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
        })?;

        let query = query.to_string();
        let project = project.map(|s| s.to_string());

        conn.interact(move |conn| {
            let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(ref p) = project {
                (
                    format!(
                        "SELECT h.id, h.created_at, h.project, h.agent, h.type, h.model,
                                snippet(handoffs_fts, 0, '>>>', '<<<', '...', 48)
                         FROM handoffs_fts fts
                         JOIN handoffs h ON h.id = fts.rowid
                         WHERE handoffs_fts MATCH ?1 AND h.project = ?2
                         ORDER BY rank
                         LIMIT {}", limit
                    ),
                    vec![Box::new(query) as Box<dyn rusqlite::types::ToSql>, Box::new(p.clone())],
                )
            } else {
                (
                    format!(
                        "SELECT h.id, h.created_at, h.project, h.agent, h.type, h.model,
                                snippet(handoffs_fts, 0, '>>>', '<<<', '...', 48)
                         FROM handoffs_fts fts
                         JOIN handoffs h ON h.id = fts.rowid
                         WHERE handoffs_fts MATCH ?1
                         ORDER BY rank
                         LIMIT {}", limit
                    ),
                    vec![Box::new(query) as Box<dyn rusqlite::types::ToSql>],
                )
            };

            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_refs.as_slice(), |row| {
                Ok(SearchResult {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    project: row.get(2)?,
                    agent: row.get(3)?,
                    handoff_type: row.get(4)?,
                    model: row.get(5)?,
                    snippet: row.get(6)?,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<Vec<SearchResult>, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs search interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn stats(&self) -> Result<HandoffStats> {
        let conn = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
        })?;

        conn.interact(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM handoffs",
                [],
                |r| r.get(0),
            )?;

            let total_content_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM handoffs",
                [],
                |r| r.get(0),
            )?;

            let date_range: Option<(String, String)> = {
                let result: rusqlite::Result<(Option<String>, Option<String>)> = conn.query_row(
                    "SELECT MIN(created_at), MAX(created_at) FROM handoffs",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                );
                result.ok().and_then(|(min, max)| match (min, max) {
                    (Some(mn), Some(mx)) if !mn.is_empty() => Some((mn, mx)),
                    _ => None,
                })
            };

            let by_project = {
                let mut stmt = conn.prepare(
                    "SELECT project, COUNT(*) as cnt, MAX(created_at) as latest
                     FROM handoffs GROUP BY project ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(ProjectStats {
                        name: r.get(0)?,
                        count: r.get(1)?,
                        latest: r.get(2)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let by_agent = {
                let mut stmt = conn.prepare(
                    "SELECT agent, COUNT(*) as cnt, MAX(created_at) as latest
                     FROM handoffs GROUP BY agent ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(AgentStats {
                        name: r.get(0)?,
                        count: r.get(1)?,
                        latest: r.get(2)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let by_host = {
                let mut stmt = conn.prepare(
                    "SELECT COALESCE(host, 'unknown'), COUNT(*) as cnt, MAX(created_at) as latest
                     FROM handoffs GROUP BY host ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(HostStats {
                        name: r.get(0)?,
                        count: r.get(1)?,
                        latest: r.get(2)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let by_type = {
                let mut stmt = conn.prepare(
                    "SELECT type, COUNT(*) as cnt, MAX(created_at) as latest, COALESCE(SUM(LENGTH(content)), 0) as total_bytes
                     FROM handoffs GROUP BY type ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(TypeStats {
                        name: r.get(0)?,
                        count: r.get(1)?,
                        latest: r.get(2)?,
                        total_bytes: r.get(3)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            Ok::<HandoffStats, rusqlite::Error>(HandoffStats {
                total,
                total_content_bytes,
                date_range,
                by_project,
                by_agent,
                by_host,
                by_type,
            })
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs stats interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn gc(&self, tiered: bool, keep: Option<i64>) -> Result<GcResult> {
        let conn = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
        })?;

        conn.interact(move |conn| {
            let before: i64 = conn.query_row("SELECT COUNT(*) FROM handoffs", [], |r| r.get(0))?;

            if let Some(n) = keep {
                let projects: Vec<String> = {
                    let mut stmt = conn.prepare("SELECT DISTINCT project FROM handoffs")?;
                    let rows = stmt.query_map([], |r| r.get(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                for project in projects {
                    conn.execute(
                        "DELETE FROM handoffs WHERE project = ?1 AND id NOT IN (
                             SELECT id FROM handoffs WHERE project = ?1 ORDER BY created_at DESC LIMIT ?2
                         )",
                        rusqlite::params![project, n],
                    )?;
                }
            } else if tiered {
                conn.execute(
                    "DELETE FROM handoffs WHERE type = 'mechanical' AND created_at < datetime('now', '-7 days')",
                    [],
                )?;
                conn.execute(
                    "DELETE FROM handoffs WHERE type IN ('manual', 'auto') AND created_at < datetime('now', '-90 days')",
                    [],
                )?;

                let projects: Vec<String> = {
                    let mut stmt = conn.prepare("SELECT DISTINCT project FROM handoffs")?;
                    let rows = stmt.query_map([], |r| r.get(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                for project in projects {
                    conn.execute(
                        "DELETE FROM handoffs WHERE project = ?1 AND id NOT IN (
                             SELECT id FROM handoffs WHERE project = ?1 ORDER BY created_at DESC LIMIT 50
                         )",
                        rusqlite::params![project],
                    )?;
                }
            }

            conn.execute("VACUUM", [])?;

            let after: i64 = conn.query_row("SELECT COUNT(*) FROM handoffs", [], |r| r.get(0))?;

            Ok::<GcResult, rusqlite::Error>(GcResult {
                deleted: before - after,
                remaining: after,
            })
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs gc interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn delete(&self, id: i64) -> Result<bool> {
        let conn = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
        })?;

        conn.interact(move |conn| {
            let affected = conn.execute(
                "DELETE FROM handoffs WHERE id = ?1",
                rusqlite::params![id],
            )?;
            Ok::<bool, rusqlite::Error>(affected > 0)
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs delete interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    async fn setup_schema(&self) -> Result<()> {
        let conn = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire handoffs writer for schema: {e}"))
        })?;

        conn.interact(|conn| conn.execute_batch(SCHEMA_SQL))
            .await
            .map_err(|e| EngError::Internal(format!("handoffs schema interact failed: {e}")))?
            .map_err(|e: rusqlite::Error| EngError::Database(e))
    }
}

fn build_pool(db_path: &str, max_size: usize) -> Result<Pool> {
    let mut config = PoolManagerConfig::new(db_path);
    config.pool = Some(PoolConfig::new(max_size));
    let db_path_owned = db_path.to_string();

    config
        .builder(Runtime::Tokio1)
        .map_err(|e| {
            EngError::Internal(format!("failed to configure handoffs pool for {db_path}: {e}"))
        })?
        .post_create(Hook::async_fn(move |conn, _| {
            let db_path = db_path_owned.clone();
            Box::pin(async move {
                conn.interact(move |conn: &mut deadpool_sqlite::rusqlite::Connection| {
                    apply_pragmas(conn)
                })
                .await
                .map_err(|e| {
                    HookError::message(format!(
                        "failed to initialize handoffs connection {}: {e}",
                        db_path
                    ))
                })?
                .map_err(HookError::Backend)?;
                Ok(())
            })
        }))
        .build()
        .map_err(|e| {
            EngError::Internal(format!("failed to build handoffs pool for {db_path}: {e}"))
        })
}

fn apply_pragmas(conn: &mut deadpool_sqlite::rusqlite::Connection) -> deadpool_sqlite::rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update(None, "cache_size", -8192_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

fn compute_content_hash(content: &str, handoff_type: &str) -> String {
    let to_hash = if handoff_type == "mechanical" {
        strip_mechanical_timestamps(content)
    } else {
        content.to_string()
    };
    let hash = Sha256::digest(to_hash.as_bytes());
    format!("{:x}", hash)[..16].to_string()
}

fn strip_mechanical_timestamps(content: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut skip_section = false;
    for line in content.lines() {
        if line.starts_with("Generated:") {
            continue;
        }
        if line.contains("Recently Modified Files") {
            skip_section = true;
            continue;
        }
        if skip_section {
            if line.starts_with("## ") || line.starts_with("# ") {
                skip_section = false;
            } else {
                continue;
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn run_tiered_gc(conn: &mut rusqlite::Connection, project: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM handoffs WHERE type = 'mechanical' AND created_at < datetime('now', '-7 days')",
        [],
    )?;
    conn.execute(
        "DELETE FROM handoffs WHERE type IN ('manual', 'auto') AND created_at < datetime('now', '-90 days')",
        [],
    )?;
    conn.execute(
        "DELETE FROM handoffs WHERE project = ?1 AND id NOT IN (
             SELECT id FROM handoffs WHERE project = ?1 ORDER BY created_at DESC LIMIT 50
         )",
        rusqlite::params![project],
    )?;
    Ok(())
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS handoffs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
    project TEXT NOT NULL,
    branch TEXT,
    directory TEXT,
    agent TEXT DEFAULT 'unknown',
    type TEXT DEFAULT 'manual',
    content TEXT NOT NULL,
    metadata TEXT,
    session_id TEXT,
    model TEXT,
    host TEXT,
    content_hash TEXT
);

CREATE INDEX IF NOT EXISTS idx_handoffs_project ON handoffs(project, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_created ON handoffs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_hash ON handoffs(content_hash);
CREATE INDEX IF NOT EXISTS idx_handoffs_agent ON handoffs(agent, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_type ON handoffs(type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_session ON handoffs(session_id);
CREATE INDEX IF NOT EXISTS idx_handoffs_model ON handoffs(model, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_restore ON handoffs(project, type, agent, created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS handoffs_fts USING fts5(
    content, content='handoffs', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS handoffs_fts_ai AFTER INSERT ON handoffs BEGIN
    INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS handoffs_fts_ad AFTER DELETE ON handoffs BEGIN
    INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER IF NOT EXISTS handoffs_fts_au AFTER UPDATE OF content ON handoffs BEGIN
    INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
END;
";
