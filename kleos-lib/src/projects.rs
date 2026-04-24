//! Projects -- project management with memory linking and scoped search.
//!
//! Ports: projects/db.ts, projects/types.ts, projects/routes.ts (logic)

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub const VALID_PROJECT_STATUSES: &[&str] = &["active", "paused", "completed", "archived"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub metadata: Option<String>,
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub memory_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProjectBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[tracing::instrument(skip(db, description, metadata), fields(name = %name, status = %status, user_id))]
pub async fn create_project(
    db: &Database,
    name: &str,
    description: Option<&str>,
    status: &str,
    metadata: Option<&str>,
    _user_id: i64,
) -> Result<(i64, String)> {
    let name = name.to_string();
    let description = description.map(|s| s.to_string());
    let status = status.to_string();
    let metadata = metadata.map(|s| s.to_string());

    db.write(move |conn| {
        let mut stmt = conn
            .prepare(
                "INSERT INTO projects (name, description, status, metadata) \
                 VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
            )
            .map_err(rusqlite_to_eng_error)?;
        let (id, created_at) = stmt
            .query_row(
                rusqlite::params![name, description, status, metadata],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| EngError::Internal(e.to_string()))?;
        Ok((id, created_at))
    })
    .await
}

#[tracing::instrument(skip(db), fields(project_id = id, user_id))]
pub async fn get_project(db: &Database, id: i64, user_id: i64) -> Result<Option<ProjectRow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.name, p.description, p.status, p.metadata, \
                 p.created_at, p.updated_at, \
                 (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count \
                 FROM projects p WHERE p.id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(row_to_project(row, user_id)?)),
            None => Ok(None),
        }
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, status = ?status))]
pub async fn list_projects(
    db: &Database,
    user_id: i64,
    status: Option<&str>,
) -> Result<Vec<ProjectRow>> {
    let status = status.map(|s| s.to_string());

    db.read(move |conn| {
        let mut result = Vec::new();
        if let Some(ref s) = status {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.name, p.description, p.status, p.metadata, \
                     p.created_at, p.updated_at, \
                     (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count \
                     FROM projects p WHERE p.status = ?1 \
                     ORDER BY p.name COLLATE NOCASE",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![s])
                .map_err(rusqlite_to_eng_error)?;
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                result.push(row_to_project(row, user_id)?);
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.name, p.description, p.status, p.metadata, \
                     p.created_at, p.updated_at, \
                     (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count \
                     FROM projects p \
                     ORDER BY p.status = 'active' DESC, p.name COLLATE NOCASE",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt.query([]).map_err(rusqlite_to_eng_error)?;
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                result.push(row_to_project(row, user_id)?);
            }
        }
        Ok(result)
    })
    .await
}

#[tracing::instrument(skip(db, name, description, metadata), fields(project_id = id, user_id, status = ?status))]
pub async fn update_project(
    db: &Database,
    id: i64,
    _user_id: i64,
    name: Option<&str>,
    description: Option<&str>,
    status: Option<&str>,
    metadata: Option<&str>,
) -> Result<()> {
    let name = name.map(|s| s.to_string());
    let description = description.map(|s| s.to_string());
    let status = status.map(|s| s.to_string());
    let metadata = metadata.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "UPDATE projects SET \
             name = COALESCE(?1, name), \
             description = COALESCE(?2, description), \
             status = COALESCE(?3, status), \
             metadata = COALESCE(?4, metadata), \
             updated_at = datetime('now') \
             WHERE id = ?5",
            rusqlite::params![name, description, status, metadata, id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(project_id = id, user_id))]
pub async fn delete_project(db: &Database, id: i64, _user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(memory_id, project_id, user_id))]
pub async fn link_memory(
    db: &Database,
    memory_id: i64,
    project_id: i64,
    _user_id: i64,
) -> Result<()> {
    db.write(move |conn| {
        // Verify project exists in this tenant shard
        let project_exists: bool = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |_| Ok(true),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .unwrap_or(false);

        if !project_exists {
            return Err(EngError::NotFound(
                "project not found or not owned by user".to_string(),
            ));
        }

        // Verify memory exists
        let memory_exists: bool = conn
            .query_row(
                "SELECT 1 FROM memories WHERE id = ?1",
                rusqlite::params![memory_id],
                |_| Ok(true),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .unwrap_or(false);

        if !memory_exists {
            return Err(EngError::NotFound(
                "memory not found or not owned by user".to_string(),
            ));
        }

        conn.execute(
            "INSERT OR IGNORE INTO memory_projects (memory_id, project_id) VALUES (?1, ?2)",
            rusqlite::params![memory_id, project_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(memory_id, project_id, user_id))]
pub async fn unlink_memory(
    db: &Database,
    memory_id: i64,
    project_id: i64,
    _user_id: i64,
) -> Result<()> {
    db.write(move |conn| {
        // Verify project exists before unlinking
        let project_exists: bool = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |_| Ok(true),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .unwrap_or(false);

        if !project_exists {
            return Err(EngError::NotFound(
                "project not found or not owned by user".to_string(),
            ));
        }

        conn.execute(
            "DELETE FROM memory_projects WHERE memory_id = ?1 AND project_id = ?2",
            rusqlite::params![memory_id, project_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(project_id, user_id))]
pub async fn get_project_memory_ids(
    db: &Database,
    project_id: i64,
    _user_id: i64,
) -> Result<Vec<i64>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT mp.memory_id FROM memory_projects mp \
                 JOIN memories m ON m.id = mp.memory_id \
                 WHERE mp.project_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![project_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            ids.push(row.get::<_, i64>(0).map_err(rusqlite_to_eng_error)?);
        }
        Ok(ids)
    })
    .await
}

fn row_to_project(row: &rusqlite::Row<'_>, owner_user_id: i64) -> Result<ProjectRow> {
    Ok(ProjectRow {
        id: row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        name: row
            .get(1)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        description: row.get(2).unwrap_or(None),
        status: row
            .get(3)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        metadata: row.get(4).unwrap_or(None),
        user_id: owner_user_id,
        created_at: row
            .get(5)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        updated_at: row.get(6).unwrap_or(None),
        memory_count: row.get(7).unwrap_or(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_statuses() {
        assert!(VALID_PROJECT_STATUSES.contains(&"active"));
        assert!(VALID_PROJECT_STATUSES.contains(&"archived"));
        assert!(!VALID_PROJECT_STATUSES.contains(&"deleted"));
    }
}
