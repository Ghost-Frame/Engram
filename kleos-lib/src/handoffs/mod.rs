pub mod atoms;

pub use atoms::{
    extract, extract_heuristic, make_atom_id, Atom, AtomStatus, AtomType, BudgetPacker,
    ExtractedAtom,
};

use crate::db::Database;
use crate::{EngError, Result};
use deadpool_sqlite::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

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
    /// Tenant database hosting the handoffs table set (schema_v43).
    db: Arc<Database>,
    /// Semaphore throttling concurrent auto-GC spawns (M-005).
    gc_sem: Arc<Semaphore>,
}

impl HandoffsDb {
    /// Build a handoffs facade over a tenant database. The caller resolves
    /// the reserved "handoffs" tenant via the registry; schema_v43 runs
    /// automatically on tenant open.
    pub fn new(db: Arc<Database>, gc_sem: Arc<Semaphore>) -> Self {
        Self { db, gc_sem }
    }

    fn reader(&self) -> Pool {
        self.db.pools().reader().clone()
    }

    fn writer(&self) -> Pool {
        self.db.pools().writer().clone()
    }

    pub async fn store(&self, params: StoreParams, user_id: i64) -> Result<StoreResult> {
        let handoff_type = params
            .handoff_type
            .clone()
            .unwrap_or_else(|| "manual".to_string());
        let agent = params
            .agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
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
            let conn = self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;
            let exists: bool = conn
                .interact(move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM handoffs WHERE content_hash = ?1 AND project = ?2 AND type = 'mechanical' AND user_id = ?3",
                        rusqlite::params![hash2, project2, user_id],
                        |row| row.get(0),
                    )?;
                    Ok::<bool, rusqlite::Error>(count > 0)
                })
                .await
                .map_err(|e| EngError::Internal(format!("handoffs reader interact failed: {e}")))?
                .map_err(|e: rusqlite::Error| EngError::Database(e))?;

            if exists {
                return Ok(StoreResult {
                    id: None,
                    skipped: true,
                });
            }
        }

        let conn =
            self.writer().get().await.map_err(|e| {
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
                    "INSERT INTO handoffs (user_id, project, branch, directory, agent, type, content, metadata, session_id, model, host, content_hash)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        user_id,
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

        let writer_clone = self.writer();
        let project3 = project.clone();
        let gc_sem = Arc::clone(&self.gc_sem);
        tokio::spawn(async move {
            // Throttle concurrent auto-GC tasks (M-005).
            let _permit = match gc_sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    error!("handoffs auto_gc semaphore closed; skipping GC");
                    return;
                }
            };
            let conn = match writer_clone.get().await {
                Ok(c) => c,
                Err(e) => {
                    error!("handoffs auto_gc writer acquire failed: {}", e);
                    return;
                }
            };
            // Auto-GC scoped to this user only -- never cross-tenant.
            let total: i64 = match conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM handoffs WHERE user_id = ?1",
                        rusqlite::params![user_id],
                        |r| r.get(0),
                    )
                })
                .await
            {
                Ok(Ok(n)) => n,
                _ => return,
            };

            if total > 500 {
                // Reuse the same writer conn -- acquiring a second from the
                // single-writer pool while still holding the first deadlocks
                // the writer indefinitely.
                if let Err(e) = conn
                    .interact(move |c| run_tiered_gc(c, &project3, user_id))
                    .await
                {
                    error!("handoffs auto_gc failed: {}", e);
                }
            }
        });

        Ok(StoreResult {
            id: Some(new_id),
            skipped: false,
        })
    }

    /// Stores a handoff and then automatically extracts and persists context atoms.
    ///
    /// Wraps [`HandoffsDb::store`] with optional atom extraction. If
    /// `pre_extracted_atoms` is supplied those atoms are used directly; otherwise
    /// [`atoms::extract`] is called with the handoff content. Mechanical
    /// handoffs are stored as normal but atom extraction is skipped entirely
    /// because git-state dumps do not contain semantic atoms worth indexing.
    ///
    /// If atom extraction or storage fails a warning is logged but the original
    /// [`StoreResult`] is still returned -- atom failure is non-fatal.
    pub async fn store_with_atoms(
        &self,
        params: StoreParams,
        user_id: i64,
        pre_extracted_atoms: Option<Vec<atoms::ExtractedAtom>>,
        sidecar_url: Option<&str>,
    ) -> Result<StoreResult> {
        let result = self.store(params.clone(), user_id).await?;

        // Mechanical handoffs are git-state dumps; skip atom extraction.
        if params.handoff_type.as_deref() == Some("mechanical") {
            return Ok(result);
        }

        let handoff_id = match result.id {
            Some(id) => id,
            // Skipped (duplicate mechanical) -- nothing to attach atoms to.
            None => return Ok(result),
        };

        let extracted = match pre_extracted_atoms {
            Some(atoms) => atoms,
            None => atoms::extract(&params.content, sidecar_url).await,
        };

        if !extracted.is_empty() {
            let count = extracted.len();
            if let Err(e) = self
                .store_atoms(handoff_id, &params.project, &extracted, user_id)
                .await
            {
                warn!(
                    handoff_id,
                    error = %e,
                    "atom storage failed; handoff was saved but atoms were not indexed"
                );
            } else {
                info!(handoff_id, atom_count = count, "atoms indexed for handoff");
            }
        }

        Ok(result)
    }

    pub async fn list(&self, filters: HandoffFilters, user_id: i64) -> Result<Vec<Handoff>> {
        let limit = filters.limit.unwrap_or(20);
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        conn.interact(move |conn| {
            // Tenant scoping always first; subsequent filters are AND-joined.
            let mut conditions: Vec<String> = vec!["user_id = ?1".to_string()];
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(user_id) as Box<dyn rusqlite::types::ToSql>];

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

            let where_clause = format!("WHERE {}", conditions.join(" AND "));

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

    pub async fn get_latest(
        &self,
        filters: HandoffFilters,
        user_id: i64,
    ) -> Result<Option<Handoff>> {
        let has_project = filters.project.is_some();
        let mut f = filters;
        f.limit = Some(1);

        let results = self.list(f.clone(), user_id).await?;
        if !results.is_empty() {
            return Ok(results.into_iter().next());
        }

        if has_project {
            let mut fallback = f;
            fallback.project = None;
            let results = self.list(fallback, user_id).await?;
            return Ok(results.into_iter().next());
        }

        Ok(None)
    }

    pub async fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: i64,
        user_id: i64,
    ) -> Result<Vec<SearchResult>> {
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        let query = query.to_string();
        let project = project.map(|s| s.to_string());

        conn.interact(move |conn| {
            let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                if let Some(ref p) = project {
                    (
                        format!(
                            "SELECT h.id, h.created_at, h.project, h.agent, h.type, h.model,
                                snippet(handoffs_fts, 0, '>>>', '<<<', '...', 48)
                         FROM handoffs_fts fts
                         JOIN handoffs h ON h.id = fts.rowid
                         WHERE handoffs_fts MATCH ?1 AND h.project = ?2 AND h.user_id = ?3
                         ORDER BY rank
                         LIMIT {}",
                            limit
                        ),
                        vec![
                            Box::new(query) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(p.clone()),
                            Box::new(user_id),
                        ],
                    )
                } else {
                    (
                        format!(
                            "SELECT h.id, h.created_at, h.project, h.agent, h.type, h.model,
                                snippet(handoffs_fts, 0, '>>>', '<<<', '...', 48)
                         FROM handoffs_fts fts
                         JOIN handoffs h ON h.id = fts.rowid
                         WHERE handoffs_fts MATCH ?1 AND h.user_id = ?2
                         ORDER BY rank
                         LIMIT {}",
                            limit
                        ),
                        vec![
                            Box::new(query) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(user_id),
                        ],
                    )
                };

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
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

    pub async fn stats(&self, user_id: i64) -> Result<HandoffStats> {
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        conn.interact(move |conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM handoffs WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )?;

            let total_content_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM handoffs WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )?;

            let date_range: Option<(String, String)> = {
                let result: rusqlite::Result<(Option<String>, Option<String>)> = conn.query_row(
                    "SELECT MIN(created_at), MAX(created_at) FROM handoffs WHERE user_id = ?1",
                    rusqlite::params![user_id],
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
                     FROM handoffs WHERE user_id = ?1 GROUP BY project ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id], |r| {
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
                     FROM handoffs WHERE user_id = ?1 GROUP BY agent ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id], |r| {
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
                     FROM handoffs WHERE user_id = ?1 GROUP BY host ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id], |r| {
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
                     FROM handoffs WHERE user_id = ?1 GROUP BY type ORDER BY cnt DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id], |r| {
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

    pub async fn gc(&self, tiered: bool, keep: Option<i64>, user_id: i64) -> Result<GcResult> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        conn.interact(move |conn| {
            let before: i64 = conn.query_row(
                "SELECT COUNT(*) FROM handoffs WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )?;

            if let Some(n) = keep {
                let projects: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT project FROM handoffs WHERE user_id = ?1",
                    )?;
                    let rows = stmt.query_map(rusqlite::params![user_id], |r| r.get(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                for project in projects {
                    conn.execute(
                        "DELETE FROM handoffs WHERE user_id = ?3 AND project = ?1 AND id NOT IN (
                             SELECT id FROM handoffs WHERE user_id = ?3 AND project = ?1 ORDER BY created_at DESC LIMIT ?2
                         )",
                        rusqlite::params![project, n, user_id],
                    )?;
                }
            } else if tiered {
                conn.execute(
                    "DELETE FROM handoffs WHERE user_id = ?1 AND type = 'mechanical' AND created_at < datetime('now', '-7 days')",
                    rusqlite::params![user_id],
                )?;
                conn.execute(
                    "DELETE FROM handoffs WHERE user_id = ?1 AND type IN ('manual', 'auto') AND created_at < datetime('now', '-90 days')",
                    rusqlite::params![user_id],
                )?;

                let projects: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT project FROM handoffs WHERE user_id = ?1",
                    )?;
                    let rows = stmt.query_map(rusqlite::params![user_id], |r| r.get(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                for project in projects {
                    conn.execute(
                        "DELETE FROM handoffs WHERE user_id = ?2 AND project = ?1 AND id NOT IN (
                             SELECT id FROM handoffs WHERE user_id = ?2 AND project = ?1 ORDER BY created_at DESC LIMIT 50
                         )",
                        rusqlite::params![project, user_id],
                    )?;
                }
            }

            // VACUUM is global; safe because all rows are still scoped per
            // user; we only ever delete this user's rows above.
            conn.execute("VACUUM", [])?;

            let after: i64 = conn.query_row(
                "SELECT COUNT(*) FROM handoffs WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )?;

            Ok::<GcResult, rusqlite::Error>(GcResult {
                deleted: before - after,
                remaining: after,
            })
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs gc interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn delete(&self, id: i64, user_id: i64) -> Result<bool> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        conn.interact(move |conn| {
            let affected = conn.execute(
                "DELETE FROM handoffs WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?;
            Ok::<bool, rusqlite::Error>(affected > 0)
        })
        .await
        .map_err(|e| EngError::Internal(format!("handoffs delete interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    /// Persists a batch of extracted atoms for a handoff.
    ///
    /// For each atom the stable `atom_id` is derived via [`make_atom_id`]. If
    /// an atom with that id already exists for this user, it is updated
    /// (bump `seen_count`, refresh `last_seen_at`, increase salience by 0.1
    /// capped at 1.0). Otherwise a new row is inserted. All writes run inside
    /// a single transaction. Returns the `atom_id` strings for every atom in
    /// the input slice.
    pub async fn store_atoms(
        &self,
        handoff_id: i64,
        project: &str,
        extracted: &[atoms::ExtractedAtom],
        user_id: i64,
    ) -> Result<Vec<String>> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        // Clone everything needed to move into interact closure.
        let project = project.to_string();
        let extracted: Vec<atoms::ExtractedAtom> = extracted.to_vec();

        conn.interact(move |conn| {
            let tx = conn.transaction()?;

            let mut ids: Vec<String> = Vec::with_capacity(extracted.len());

            for ea in &extracted {
                let atom_id = make_atom_id(ea.atom_type.clone(), &ea.canonical_form);
                let decay_immune = ea.atom_type.is_decay_immune();

                // Check if the atom already exists for this user.
                let exists: bool = tx.query_row(
                    "SELECT COUNT(*) FROM handoff_atoms WHERE atom_id = ?1 AND user_id = ?2",
                    rusqlite::params![atom_id, user_id],
                    |row| row.get::<_, i64>(0),
                )? > 0;

                if exists {
                    tx.execute(
                        "UPDATE handoff_atoms
                         SET seen_count  = seen_count + 1,
                             last_seen_at = datetime('now'),
                             salience     = MIN(1.0, salience + 0.1)
                         WHERE atom_id = ?1 AND user_id = ?2",
                        rusqlite::params![atom_id, user_id],
                    )?;
                } else {
                    tx.execute(
                        "INSERT INTO handoff_atoms
                             (atom_id, handoff_id, user_id, project, atom_type,
                              content, canonical_form, salience, confidence,
                              status, decay_immune)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10)",
                        rusqlite::params![
                            atom_id,
                            handoff_id,
                            user_id,
                            project,
                            ea.atom_type.as_str(),
                            ea.content,
                            ea.canonical_form,
                            ea.confidence,
                            ea.confidence,
                            decay_immune,
                        ],
                    )?;
                }

                ids.push(atom_id);
            }

            tx.commit()?;
            Ok::<Vec<String>, rusqlite::Error>(ids)
        })
        .await
        .map_err(|e| EngError::Internal(format!("store_atoms interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    /// Lists atoms for a project, optionally filtered by type and status.
    ///
    /// Results are ordered by salience descending. When `status` is `None` the
    /// query defaults to active atoms only.
    pub async fn list_atoms(
        &self,
        project: &str,
        atom_type: Option<&str>,
        status: Option<&str>,
        limit: i64,
        user_id: i64,
    ) -> Result<Vec<atoms::Atom>> {
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        let project = project.to_string();
        let atom_type = atom_type.map(|s| s.to_string());
        let status = status.map(|s| s.to_string());

        conn.interact(move |conn| {
            let mut conditions: Vec<String> =
                vec!["user_id = ?1".to_string(), "project = ?2".to_string()];
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
                Box::new(user_id) as Box<dyn rusqlite::types::ToSql>,
                Box::new(project),
            ];

            if let Some(ref t) = atom_type {
                conditions.push(format!("atom_type = ?{}", params.len() + 1));
                params.push(Box::new(t.clone()));
            }

            // Default to 'active' when the caller does not specify a status.
            let status_val = status.unwrap_or_else(|| "active".to_string());
            conditions.push(format!("status = ?{}", params.len() + 1));
            params.push(Box::new(status_val));

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT id, atom_id, handoff_id, user_id, project, atom_type,
                        content, canonical_form, salience, confidence, status,
                        created_at, last_seen_at, seen_count, decay_immune,
                        superseded_by, metadata
                 FROM handoff_atoms
                 WHERE {}
                 ORDER BY salience DESC
                 LIMIT {}",
                where_clause, limit
            );

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_refs.as_slice(), |row| {
                let atom_type_str: String = row.get(5)?;
                let status_str: String = row.get(10)?;
                let metadata_str: Option<String> = row.get(16)?;
                let metadata = metadata_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());

                // Default to Entity if the stored value is somehow unknown.
                let atom_type =
                    atoms::AtomType::parse(&atom_type_str).unwrap_or(atoms::AtomType::Entity);

                let status = match status_str.as_str() {
                    "resolved" => atoms::AtomStatus::Resolved,
                    "superseded" => atoms::AtomStatus::Superseded,
                    "contested" => atoms::AtomStatus::Contested,
                    _ => atoms::AtomStatus::Active,
                };

                Ok(atoms::Atom {
                    id: Some(row.get(0)?),
                    atom_id: row.get(1)?,
                    handoff_id: row.get(2)?,
                    user_id: row.get(3)?,
                    project: row.get(4)?,
                    atom_type,
                    content: row.get(6)?,
                    canonical_form: row.get(7)?,
                    salience: row.get(8)?,
                    confidence: row.get(9)?,
                    status,
                    created_at: row.get(11)?,
                    last_seen_at: row.get(12)?,
                    seen_count: row.get(13)?,
                    decay_immune: row.get(14)?,
                    superseded_by: row.get(15)?,
                    metadata,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<Vec<atoms::Atom>, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| EngError::Internal(format!("list_atoms interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    /// Returns a packed context string for the project within a token budget.
    ///
    /// Fetches up to 200 active atoms, runs them through [`BudgetPacker`], and
    /// renders the result as a grouped markdown string via
    /// [`BudgetPacker::to_context_string`].
    pub async fn get_packed_context(
        &self,
        project: &str,
        max_tokens: usize,
        user_id: i64,
    ) -> Result<String> {
        let atom_list = self.list_atoms(project, None, None, 200, user_id).await?;
        let packer = atoms::BudgetPacker::new(max_tokens, 5);
        let packed = packer.pack(&atom_list);
        Ok(packer.to_context_string(&packed))
    }

    /// Marks an active atom as superseded by a newer atom.
    ///
    /// Only the owning user's active atom is updated. If the atom is already
    /// resolved, superseded, or belongs to another user this is a no-op.
    pub async fn supersede_atom(
        &self,
        old_atom_id: &str,
        new_atom_id: &str,
        user_id: i64,
    ) -> Result<()> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        let old_atom_id = old_atom_id.to_string();
        let new_atom_id = new_atom_id.to_string();

        conn.interact(move |conn| {
            conn.execute(
                "UPDATE handoff_atoms
                 SET status = 'superseded', superseded_by = ?2
                 WHERE atom_id = ?1 AND user_id = ?3 AND status = 'active'",
                rusqlite::params![old_atom_id, new_atom_id, user_id],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| EngError::Internal(format!("supersede_atom interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    /// Applies exponential salience decay to non-immune atoms in a project.
    ///
    /// Salience is updated to `MAX(0.01, salience * 0.9^sessions_elapsed)` for
    /// every non-immune active atom. Atoms whose salience drops below 0.05 are
    /// then transitioned to `resolved`. Returns the total number of rows
    /// affected across both statements.
    pub async fn apply_session_decay(
        &self,
        project: &str,
        sessions_elapsed: u32,
        user_id: i64,
    ) -> Result<u64> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        let project = project.to_string();

        conn.interact(move |conn| {
            // Compute the decay factor once and pass it as a parameter so that
            // SQLite does not have to evaluate a power function per row.
            let factor = 0.9_f64.powi(sessions_elapsed as i32);

            let decay_affected = conn.execute(
                "UPDATE handoff_atoms
                 SET salience = MAX(0.01, salience * ?1)
                 WHERE user_id = ?2
                   AND project = ?3
                   AND status  = 'active'
                   AND decay_immune = 0",
                rusqlite::params![factor, user_id, project],
            )? as u64;

            let resolve_affected = conn.execute(
                "UPDATE handoff_atoms
                 SET status = 'resolved'
                 WHERE user_id = ?1
                   AND project = ?2
                   AND status  = 'active'
                   AND decay_immune = 0
                   AND salience < 0.05",
                rusqlite::params![user_id, project],
            )? as u64;

            Ok::<u64, rusqlite::Error>(decay_affected + resolve_affected)
        })
        .await
        .map_err(|e| EngError::Internal(format!("apply_session_decay interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn link_atom_entity(
        &self,
        atom_id: &str,
        entity_id: i64,
        user_id: i64,
    ) -> Result<()> {
        let conn =
            self.writer().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs writer: {e}"))
            })?;

        let atom_id = atom_id.to_string();

        conn.interact(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO atom_entity_links (atom_id, entity_id, user_id)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![atom_id, entity_id, user_id],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| EngError::Internal(format!("link_atom_entity interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn get_atom_entities(&self, atom_id: &str, user_id: i64) -> Result<Vec<i64>> {
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        let atom_id = atom_id.to_string();

        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT entity_id FROM atom_entity_links WHERE atom_id = ?1 AND user_id = ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![atom_id, user_id], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()
        })
        .await
        .map_err(|e| EngError::Internal(format!("get_atom_entities interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }

    pub async fn get_entity_atoms(&self, entity_id: i64, user_id: i64) -> Result<Vec<atoms::Atom>> {
        let conn =
            self.reader().get().await.map_err(|e| {
                EngError::Internal(format!("failed to acquire handoffs reader: {e}"))
            })?;

        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.atom_id, a.handoff_id, a.user_id, a.project, a.atom_type,
                        a.content, a.canonical_form, a.salience, a.confidence, a.status,
                        a.created_at, a.last_seen_at, a.seen_count, a.decay_immune,
                        a.superseded_by, a.metadata
                 FROM handoff_atoms a
                 JOIN atom_entity_links l ON l.atom_id = a.atom_id AND l.user_id = a.user_id
                 WHERE l.entity_id = ?1 AND l.user_id = ?2 AND a.status = 'active'
                 ORDER BY a.salience DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![entity_id, user_id], |row| {
                let atom_type_str: String = row.get(5)?;
                let status_str: String = row.get(10)?;
                let metadata_str: Option<String> = row.get(16)?;
                let metadata = metadata_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());

                let atom_type =
                    atoms::AtomType::parse(&atom_type_str).unwrap_or(atoms::AtomType::Entity);

                let status = match status_str.as_str() {
                    "resolved" => atoms::AtomStatus::Resolved,
                    "superseded" => atoms::AtomStatus::Superseded,
                    "contested" => atoms::AtomStatus::Contested,
                    _ => atoms::AtomStatus::Active,
                };

                Ok(atoms::Atom {
                    id: Some(row.get(0)?),
                    atom_id: row.get(1)?,
                    handoff_id: row.get(2)?,
                    user_id: row.get(3)?,
                    project: row.get(4)?,
                    atom_type,
                    content: row.get(6)?,
                    canonical_form: row.get(7)?,
                    salience: row.get(8)?,
                    confidence: row.get(9)?,
                    status,
                    created_at: row.get(11)?,
                    last_seen_at: row.get(12)?,
                    seen_count: row.get(13)?,
                    decay_immune: row.get(14)?,
                    superseded_by: row.get(15)?,
                    metadata,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<Vec<atoms::Atom>, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| EngError::Internal(format!("get_entity_atoms interact failed: {e}")))?
        .map_err(|e: rusqlite::Error| EngError::Database(e))
    }
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

fn run_tiered_gc(
    conn: &mut rusqlite::Connection,
    project: &str,
    user_id: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM handoffs WHERE user_id = ?1 AND type = 'mechanical' AND created_at < datetime('now', '-7 days')",
        rusqlite::params![user_id],
    )?;
    conn.execute(
        "DELETE FROM handoffs WHERE user_id = ?1 AND type IN ('manual', 'auto') AND created_at < datetime('now', '-90 days')",
        rusqlite::params![user_id],
    )?;
    conn.execute(
        "DELETE FROM handoffs WHERE user_id = ?2 AND project = ?1 AND id NOT IN (
             SELECT id FROM handoffs WHERE user_id = ?2 AND project = ?1 ORDER BY created_at DESC LIMIT 50
         )",
        rusqlite::params![project, user_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_params(project: &str, content: &str) -> StoreParams {
        StoreParams {
            project: project.to_string(),
            branch: None,
            directory: None,
            agent: Some("test-agent".to_string()),
            handoff_type: Some("manual".to_string()),
            content: content.to_string(),
            session_id: Some("session-x".to_string()),
            model: None,
            host: None,
            metadata: None,
        }
    }

    /// Build a handoffs DB backed by an in-memory tenant shard. The
    /// shared-cache URI keeps reader and writer pools talking to the same
    /// SQLite instance for the duration of the test.
    async fn fresh_db() -> HandoffsDb {
        let tenant = Arc::new(
            crate::db::Database::open_tenant_memory()
                .await
                .expect("open tenant memory db"),
        );
        let gc_sem = Arc::new(Semaphore::new(8));
        HandoffsDb::new(tenant, gc_sem)
    }

    #[tokio::test]
    async fn store_scopes_to_user() {
        let db = fresh_db().await;
        let a = db
            .store(store_params("proj-a", "alice secret"), 7)
            .await
            .expect("store a")
            .id
            .expect("id");
        let b = db
            .store(store_params("proj-b", "bob secret"), 99)
            .await
            .expect("store b")
            .id
            .expect("id");

        let alice = db.list(HandoffFilters::default(), 7).await.expect("list a");
        let bob = db
            .list(HandoffFilters::default(), 99)
            .await
            .expect("list b");

        assert_eq!(alice.len(), 1, "alice sees only her handoff");
        assert_eq!(bob.len(), 1, "bob sees only his handoff");
        assert_eq!(alice[0].id, a);
        assert_eq!(bob[0].id, b);
        assert!(alice.iter().all(|h| h.id != b));
        assert!(bob.iter().all(|h| h.id != a));
    }

    #[tokio::test]
    async fn delete_refuses_cross_user() {
        let db = fresh_db().await;
        let a_id = db
            .store(store_params("proj-a", "alice"), 7)
            .await
            .expect("store")
            .id
            .expect("id");

        // Bob tries to delete alice's handoff by id; expect no-op.
        let removed = db.delete(a_id, 99).await.expect("delete");
        assert!(!removed, "cross-user delete must return false");

        // Alice still sees her handoff.
        let alice = db.list(HandoffFilters::default(), 7).await.expect("list");
        assert_eq!(alice.len(), 1);

        // Alice can delete her own.
        let removed = db.delete(a_id, 7).await.expect("delete own");
        assert!(removed);
    }

    #[tokio::test]
    async fn search_scopes_to_user() {
        let db = fresh_db().await;
        db.store(store_params("proj-a", "alice has a uniquemarker phrase"), 7)
            .await
            .expect("store a");
        db.store(store_params("proj-b", "bob has a uniquemarker phrase"), 99)
            .await
            .expect("store b");

        let alice_hits = db
            .search("uniquemarker", None, 10, 7)
            .await
            .expect("search a");
        let bob_hits = db
            .search("uniquemarker", None, 10, 99)
            .await
            .expect("search b");

        assert_eq!(alice_hits.len(), 1, "alice search returns one row");
        assert_eq!(bob_hits.len(), 1, "bob search returns one row");
        assert_ne!(alice_hits[0].id, bob_hits[0].id);
    }

    #[tokio::test]
    async fn gc_scopes_to_user() {
        let db = fresh_db().await;
        for i in 0..5 {
            db.store(store_params("proj-a", &format!("alice {i}")), 7)
                .await
                .expect("store a");
        }
        for i in 0..3 {
            db.store(store_params("proj-b", &format!("bob {i}")), 99)
                .await
                .expect("store b");
        }

        // Bob runs gc with keep=1; expect his rows pruned, alice untouched.
        let result = db.gc(false, Some(1), 99).await.expect("gc bob");
        assert_eq!(result.remaining, 1, "bob now has 1 handoff");
        assert_eq!(result.deleted, 2);

        let alice = db.list(HandoffFilters::default(), 7).await.expect("list a");
        assert_eq!(alice.len(), 5, "alice handoffs untouched by bob's gc");
    }

    #[tokio::test]
    async fn stats_scopes_to_user() {
        let db = fresh_db().await;
        db.store(store_params("proj-a", "alice"), 7)
            .await
            .expect("a");
        db.store(store_params("proj-a", "alice 2"), 7)
            .await
            .expect("a2");
        db.store(store_params("proj-b", "bob"), 99)
            .await
            .expect("b");

        let alice = db.stats(7).await.expect("stats a");
        assert_eq!(alice.total, 2);

        let bob = db.stats(99).await.expect("stats b");
        assert_eq!(bob.total, 1);
    }
}
