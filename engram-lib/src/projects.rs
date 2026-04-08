//! Projects -- project management with memory linking and scoped search.
//!
//! Ports: projects/db.ts, projects/types.ts, projects/routes.ts (logic)

use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

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

pub async fn create_project(db: &Database, name: &str, description: Option<&str>, status: &str, metadata: Option<&str>, user_id: i64) -> Result<(i64, String)> {
    let mut rows = db.conn.query(
        "INSERT INTO projects (name, description, status, metadata, user_id) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, created_at",
        libsql::params![name.to_string(), description.map(|s| s.to_string()), status.to_string(), metadata.map(|s| s.to_string()), user_id],
    ).await?;
    let row = rows.next().await?.ok_or_else(|| crate::EngError::Internal("insert project failed".into()))?;
    let id: i64 = row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let created_at: String = row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?;
    Ok((id, created_at))
}

pub async fn get_project(db: &Database, id: i64, user_id: i64) -> Result<Option<ProjectRow>> {
    let mut rows = db.conn.query(
        "SELECT p.id, p.name, p.description, p.status, p.metadata, p.user_id, p.created_at, p.updated_at, (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count FROM projects p WHERE p.id = ?1 AND p.user_id = ?2",
        libsql::params![id, user_id],
    ).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_to_project(&row)?)),
        None => Ok(None),
    }
}

pub async fn list_projects(db: &Database, user_id: i64, status: Option<&str>) -> Result<Vec<ProjectRow>> {
    let mut result = Vec::new();
    if let Some(s) = status {
        let mut rows = db.conn.query(
            "SELECT p.id, p.name, p.description, p.status, p.metadata, p.user_id, p.created_at, p.updated_at, (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count FROM projects p WHERE p.user_id = ?1 AND p.status = ?2 ORDER BY p.name COLLATE NOCASE",
            libsql::params![user_id, s.to_string()],
        ).await?;
        while let Some(row) = rows.next().await? { result.push(row_to_project(&row)?); }
    } else {
        let mut rows = db.conn.query(
            "SELECT p.id, p.name, p.description, p.status, p.metadata, p.user_id, p.created_at, p.updated_at, (SELECT COUNT(*) FROM memory_projects WHERE project_id = p.id) as memory_count FROM projects p WHERE p.user_id = ?1 ORDER BY p.status = 'active' DESC, p.name COLLATE NOCASE",
            libsql::params![user_id],
        ).await?;
        while let Some(row) = rows.next().await? { result.push(row_to_project(&row)?); }
    }
    Ok(result)
}

pub async fn update_project(db: &Database, id: i64, user_id: i64, name: Option<&str>, description: Option<&str>, status: Option<&str>, metadata: Option<&str>) -> Result<()> {
    db.conn.execute(
        "UPDATE projects SET name = COALESCE(?1, name), description = COALESCE(?2, description), status = COALESCE(?3, status), metadata = COALESCE(?4, metadata), updated_at = datetime('now') WHERE id = ?5 AND user_id = ?6",
        libsql::params![name.map(|s| s.to_string()), description.map(|s| s.to_string()), status.map(|s| s.to_string()), metadata.map(|s| s.to_string()), id, user_id],
    ).await?;
    Ok(())
}

pub async fn delete_project(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute("DELETE FROM projects WHERE id = ?1 AND user_id = ?2", libsql::params![id, user_id]).await?;
    Ok(())
}

pub async fn link_memory(db: &Database, memory_id: i64, project_id: i64) -> Result<()> {
    db.conn.execute("INSERT OR IGNORE INTO memory_projects (memory_id, project_id) VALUES (?1, ?2)", libsql::params![memory_id, project_id]).await?;
    Ok(())
}

pub async fn unlink_memory(db: &Database, memory_id: i64, project_id: i64) -> Result<()> {
    db.conn.execute("DELETE FROM memory_projects WHERE memory_id = ?1 AND project_id = ?2", libsql::params![memory_id, project_id]).await?;
    Ok(())
}

pub async fn get_project_memory_ids(db: &Database, project_id: i64, user_id: i64) -> Result<Vec<i64>> {
    let mut rows = db.conn.query(
        "SELECT mp.memory_id FROM memory_projects mp JOIN memories m ON m.id = mp.memory_id WHERE mp.project_id = ?1 AND m.user_id = ?2",
        libsql::params![project_id, user_id],
    ).await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<i64>(0).map_err(|e| crate::EngError::Internal(e.to_string()))?);
    }
    Ok(ids)
}

fn row_to_project(row: &libsql::Row) -> Result<ProjectRow> {
    Ok(ProjectRow {
        id: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        name: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        description: row.get(2).unwrap_or(None),
        status: row.get(3).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        metadata: row.get(4).unwrap_or(None),
        user_id: row.get(5).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        created_at: row.get(6).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        updated_at: row.get(7).unwrap_or(None),
        memory_count: row.get(8).unwrap_or(None),
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
