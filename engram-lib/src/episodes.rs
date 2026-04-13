use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO episodes (title, session_id, agent, summary, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![req.title, req.session_id, req.agent, req.summary, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    db.read(move |conn| {
        conn.query_row(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE id = ?1",
            params![id],
            row_to_episode,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::Internal("failed to create episode".into())
            }
            other => rusqlite_to_eng_error(other),
        })
    })
    .await
}

pub async fn list_episodes(db: &Database, user_id: i64, limit: usize) -> Result<Vec<EpisodeRow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
                 FROM episodes
                 WHERE user_id = ?1
                 ORDER BY started_at DESC
                 LIMIT ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                row_to_episode(row).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::other(e.to_string())),
                    )
                })
            })
            .map_err(rusqlite_to_eng_error)?;

        collect_episodes(rows)
    })
    .await
}

pub async fn list_episodes_by_time_range(
    db: &Database,
    user_id: i64,
    after: &str,
    before: &str,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    let after = after.to_string();
    let before = before.to_string();

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
                 FROM episodes
                 WHERE user_id = ?1 AND started_at >= ?2 AND started_at <= ?3
                 ORDER BY started_at DESC
                 LIMIT ?4",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![user_id, after, before, limit as i64], |row| {
                row_to_episode(row).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::other(e.to_string())),
                    )
                })
            })
            .map_err(rusqlite_to_eng_error)?;

        collect_episodes(rows)
    })
    .await
}

pub async fn search_episodes_fts(
    db: &Database,
    query: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    let like = format!("%{}%", query);

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
                 FROM episodes
                 WHERE user_id = ?1 AND (title LIKE ?2 OR summary LIKE ?2)
                 ORDER BY started_at DESC
                 LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![user_id, like, limit as i64], |row| {
                row_to_episode(row).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::other(e.to_string())),
                    )
                })
            })
            .map_err(rusqlite_to_eng_error)?;

        collect_episodes(rows)
    })
    .await
}

pub async fn get_episode_for_user(db: &Database, id: i64, user_id: i64) -> Result<EpisodeRow> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, title, session_id, agent, summary, user_id, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            row_to_episode,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::NotFound(format!("episode {}", id))
            }
            other => rusqlite_to_eng_error(other),
        })
    })
    .await
}

pub async fn get_episode_memories(
    db: &Database,
    episode_id: i64,
    user_id: i64,
) -> Result<Vec<serde_json::Value>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, source, importance, created_at
                 FROM memories
                 WHERE episode_id = ?1 AND user_id = ?2 AND is_forgotten = 0
                 ORDER BY created_at DESC",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(params![episode_id, user_id], |row| {
                let id: i64 = row.get(0)?;
                let content: String = row.get(1)?;
                let category: String = row.get(2)?;
                let source: Option<String> = row.get(3)?;
                let importance: i32 = row.get(4)?;
                let created_at: String = row.get(5)?;
                Ok((id, content, category, source, importance, created_at))
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut memories = Vec::new();
        for row in rows {
            let (id, content, category, source, importance, created_at) =
                row.map_err(rusqlite_to_eng_error)?;
            memories.push(serde_json::json!({
                "id": id,
                "content": content,
                "category": category,
                "source": source,
                "importance": importance,
                "created_at": created_at,
            }));
        }
        Ok(memories)
    })
    .await
}

pub async fn update_episode_for_user(
    db: &Database,
    id: i64,
    user_id: i64,
    req: &UpdateEpisodeRequest,
) -> Result<()> {
    let title = req.title.clone();
    let summary = req.summary.clone();
    let ended_at = req.ended_at.clone();

    db.write(move |conn| {
        conn.execute(
            "UPDATE episodes
             SET title = COALESCE(?1, title),
                 summary = COALESCE(?2, summary),
                 ended_at = COALESCE(?3, ended_at)
             WHERE id = ?4 AND user_id = ?5",
            params![title, summary, ended_at, id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn assign_memories_to_episode(
    db: &Database,
    episode_id: i64,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<i64> {
    let memory_ids = memory_ids.to_vec();

    db.write(move |conn| {
        let mut assigned = 0_i64;
        for memory_id in &memory_ids {
            let count = conn
                .execute(
                    "UPDATE memories SET episode_id = ?1 WHERE id = ?2 AND user_id = ?3",
                    params![episode_id, *memory_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            assigned += count as i64;
        }

        conn.execute(
            "UPDATE episodes
             SET memory_count = (
                 SELECT COUNT(*) FROM memories WHERE episode_id = ?1 AND user_id = ?2
             )
             WHERE id = ?1 AND user_id = ?2",
            params![episode_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(assigned)
    })
    .await
}

pub async fn finalize_episode(db: &Database, id: i64, user_id: i64) -> Result<EpisodeRow> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE episodes
             SET ended_at = COALESCE(ended_at, datetime('now')),
                 memory_count = (
                     SELECT COUNT(*) FROM memories WHERE episode_id = ?1 AND user_id = ?2
                 )
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    get_episode_for_user(db, id, user_id).await
}

fn collect_episodes<I>(rows: I) -> Result<Vec<EpisodeRow>>
where
    I: Iterator<Item = rusqlite::Result<EpisodeRow>>,
{
    let mut episodes = Vec::new();
    for row in rows {
        episodes.push(row.map_err(rusqlite_to_eng_error)?);
    }
    Ok(episodes)
}

fn row_to_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<EpisodeRow> {
    Ok(EpisodeRow {
        id: row.get(0)?,
        title: row.get(1)?,
        session_id: row.get(2)?,
        agent: row.get(3)?,
        summary: row.get(4)?,
        user_id: row.get(5)?,
        memory_count: row.get::<_, i64>(6).unwrap_or(0),
        duration_seconds: row.get(7)?,
        decay_score: row.get(8)?,
        started_at: row.get(9)?,
        ended_at: row.get(10)?,
        created_at: row.get(11)?,
    })
}
