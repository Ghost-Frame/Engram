use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRow {
    pub id: i64,
    pub title: Option<String>,
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub summary: Option<String>,
    pub user_id: i64,
    pub memory_count: i64,
    pub duration_seconds: Option<i64>,
    pub decay_score: Option<f64>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateEpisodeRequest {
    pub title: Option<String>,
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateEpisodeRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssignMemoriesRequest {
    pub memory_ids: Vec<i64>,
}

pub async fn create_episode(
    db: &Database,
    req: CreateEpisodeRequest,
    user_id: i64,
) -> Result<EpisodeRow> {
    let mut rows = db
        .conn
        .query(
            "INSERT INTO episodes (title, session_id, agent, summary, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at",
            libsql::params![
                req.title,
                req.session_id,
                req.agent,
                req.summary,
                user_id
            ],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("failed to create episode".into()))?;
    row_to_episode(&row)
}

pub async fn list_episodes(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE user_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2",
            libsql::params![user_id, limit as i64],
        )
        .await?;
    collect_episodes(&mut rows).await
}

pub async fn list_episodes_by_time_range(
    db: &Database,
    user_id: i64,
    after: &str,
    before: &str,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE user_id = ?1 AND started_at >= ?2 AND started_at <= ?3
             ORDER BY started_at DESC
             LIMIT ?4",
            libsql::params![user_id, after.to_string(), before.to_string(), limit as i64],
        )
        .await?;
    collect_episodes(&mut rows).await
}

pub async fn search_episodes_fts(
    db: &Database,
    query: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    let like = format!("%{}%", query);
    let mut rows = db
        .conn
        .query(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE user_id = ?1 AND (title LIKE ?2 OR summary LIKE ?2)
             ORDER BY started_at DESC
             LIMIT ?3",
            libsql::params![user_id, like, limit as i64],
        )
        .await?;
    collect_episodes(&mut rows).await
}

pub async fn get_episode_for_user(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<EpisodeRow> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::NotFound(format!("episode {}", id)))?;
    row_to_episode(&row)
}

pub async fn get_episode_memories(
    db: &Database,
    episode_id: i64,
    user_id: i64,
) -> Result<Vec<serde_json::Value>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, content, category, source, importance, created_at
             FROM memories
             WHERE episode_id = ?1 AND user_id = ?2 AND is_forgotten = 0
             ORDER BY created_at DESC",
            libsql::params![episode_id, user_id],
        )
        .await?;
    let mut memories = Vec::new();
    while let Some(row) = rows.next().await? {
        memories.push(serde_json::json!({
            "id": row.get::<i64>(0)?,
            "content": row.get::<String>(1)?,
            "category": row.get::<String>(2)?,
            "source": row.get::<String>(3)?,
            "importance": row.get::<i32>(4)?,
            "created_at": row.get::<String>(5)?,
        }));
    }
    Ok(memories)
}

pub async fn update_episode_for_user(
    db: &Database,
    id: i64,
    user_id: i64,
    req: &UpdateEpisodeRequest,
) -> Result<()> {
    db.conn
        .execute(
            "UPDATE episodes
             SET title = COALESCE(?1, title),
                 summary = COALESCE(?2, summary),
                 ended_at = COALESCE(?3, ended_at)
             WHERE id = ?4 AND user_id = ?5",
            libsql::params![
                req.title.clone(),
                req.summary.clone(),
                req.ended_at.clone(),
                id,
                user_id
            ],
        )
        .await?;
    Ok(())
}

pub async fn assign_memories_to_episode(
    db: &Database,
    episode_id: i64,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<i64> {
    let mut assigned = 0_i64;
    for memory_id in memory_ids {
        assigned += db
            .conn
            .execute(
                "UPDATE memories SET episode_id = ?1 WHERE id = ?2 AND user_id = ?3",
                libsql::params![episode_id, *memory_id, user_id],
            )
            .await? as i64;
    }
    db.conn
        .execute(
            "UPDATE episodes
             SET memory_count = (
                 SELECT COUNT(*) FROM memories WHERE episode_id = ?1 AND user_id = ?2
             )
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![episode_id, user_id],
        )
        .await?;
    Ok(assigned)
}

pub async fn finalize_episode(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<EpisodeRow> {
    db.conn
        .execute(
            "UPDATE episodes
             SET ended_at = COALESCE(ended_at, datetime('now')),
                 memory_count = (
                     SELECT COUNT(*) FROM memories WHERE episode_id = ?1 AND user_id = ?2
                 )
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    get_episode_for_user(db, id, user_id).await
}

async fn collect_episodes(rows: &mut libsql::Rows) -> Result<Vec<EpisodeRow>> {
    let mut episodes = Vec::new();
    while let Some(row) = rows.next().await? {
        episodes.push(row_to_episode(&row)?);
    }
    Ok(episodes)
}

fn row_to_episode(row: &libsql::Row) -> Result<EpisodeRow> {
    Ok(EpisodeRow {
        id: row.get(0)?,
        title: row.get(1)?,
        session_id: row.get(2)?,
        agent: row.get(3)?,
        summary: row.get(4)?,
        user_id: row.get(5)?,
        memory_count: row.get::<i64>(6).unwrap_or(0),
        duration_seconds: row.get(7)?,
        decay_score: row.get(8)?,
        started_at: row.get(9)?,
        ended_at: row.get(10)?,
        created_at: row.get(11)?,
    })
}
