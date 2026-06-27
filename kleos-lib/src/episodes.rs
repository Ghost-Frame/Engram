use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
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

#[tracing::instrument(skip(db, req), fields(has_title = req.title.is_some()))]
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
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    db.read(move |conn| {
        conn.query_row(
            "SELECT id, title, session_id, agent, summary, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |row| row_to_episode(row, user_id),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::Internal("failed to create episode".into())
            }
            other => EngError::Database(other),
        })
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_episodes(db: &Database, user_id: i64, limit: usize) -> Result<Vec<EpisodeRow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, session_id, agent, summary, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
                 FROM episodes
                 WHERE user_id = ?1
                 ORDER BY started_at DESC
                 LIMIT ?2",
            )
            ?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                row_to_episode(row, user_id).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::other(e.to_string())),
                    )
                })
            })
            ?;

        collect_episodes(rows)
    })
    .await
}

#[tracing::instrument(skip(db, after, before))]
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
                "SELECT id, title, session_id, agent, summary, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
                 FROM episodes
                 WHERE user_id = ?4 AND started_at >= ?1 AND started_at <= ?2
                 ORDER BY started_at DESC
                 LIMIT ?3",
            )
            ?;

        let rows = stmt
            .query_map(params![after, before, limit as i64, user_id], |row| {
                row_to_episode(row, user_id).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::other(e.to_string())),
                    )
                })
            })
            ?;

        collect_episodes(rows)
    })
    .await
}

#[tracing::instrument(skip(db, query), fields(query_len = query.len()))]
pub async fn search_episodes_fts(
    db: &Database,
    user_id: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<EpisodeRow>> {
    // SECURITY: bound the query size before tokenisation, mirroring memory `fts_search`,
    // so a pathologically large input cannot drive CPU-heavy FTS expression building.
    if query.len() > crate::validation::MAX_FTS_QUERY_LEN {
        return Err(EngError::InvalidInput(format!(
            "query exceeds maximum length of {} bytes",
            crate::validation::MAX_FTS_QUERY_LEN
        )));
    }
    // Build a bounded OR-of-tokens FTS5 MATCH expression (each token alphanumeric-only and
    // quoted, stopwords dropped) using the same builder memory search uses. This replaces the
    // prior `title/summary LIKE %query%` scan, which bypassed the `episodes_fts` index, did no
    // BM25 ranking or stemming, and fed raw user input straight into a LIKE pattern. Returns
    // empty (no error) when no usable token remains, matching `fts_search`.
    //
    // Behavior change: results are now ordered by BM25 relevance (best match first) instead of
    // the prior `started_at DESC` recency order -- the correct ranking for a search endpoint.
    let match_expr = crate::memory::fts::fts_or_match_query(query);
    if match_expr.is_empty() {
        return Ok(vec![]);
    }

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.title, e.session_id, e.agent, e.summary, e.memory_count, \
                    e.duration_seconds, e.decay_score, e.started_at, e.ended_at, e.created_at \
             FROM episodes_fts f \
             JOIN episodes e ON e.id = f.rowid \
             WHERE episodes_fts MATCH ?1 AND e.user_id = ?2 \
             ORDER BY bm25(episodes_fts) \
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![match_expr, user_id, limit as i64], |row| {
            row_to_episode(row, user_id).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Null,
                    Box::new(std::io::Error::other(e.to_string())),
                )
            })
        })?;

        collect_episodes(rows)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_episode_for_user(db: &Database, id: i64, user_id: i64) -> Result<EpisodeRow> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, title, session_id, agent, summary, memory_count, duration_seconds, decay_score, started_at, ended_at, created_at
             FROM episodes
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            |row| row_to_episode(row, user_id),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::NotFound(format!("episode {}", id))
            }
            other => EngError::Database(other),
        })
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_episode_memories(
    db: &Database,
    episode_id: i64,
    user_id: i64,
) -> Result<Vec<serde_json::Value>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, content, category, source, importance, created_at
                 FROM memories
                 WHERE episode_id = ?1 AND user_id = ?2 AND is_forgotten = 0 \
                   AND is_latest = 1 AND is_archived = 0
                 ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map(params![episode_id, user_id], |row| {
            let id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let category: String = row.get(2)?;
            let source: Option<String> = row.get(3)?;
            let importance: i32 = row.get(4)?;
            let created_at: String = row.get(5)?;
            Ok((id, content, category, source, importance, created_at))
        })?;

        let mut memories = Vec::new();
        for row in rows {
            let (id, content, category, source, importance, created_at) = row?;
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

#[tracing::instrument(skip(db, req))]
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
        )?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, memory_ids), fields(memory_count = memory_ids.len()))]
pub async fn assign_memories_to_episode(
    db: &Database,
    episode_id: i64,
    user_id: i64,
    memory_ids: &[i64],
) -> Result<i64> {
    let memory_ids = memory_ids.to_vec();

    db.write(move |conn| {
        // The episode must belong to the caller before any memory is linked.
        let owns_episode: i64 = conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE id = ?1 AND user_id = ?2",
            params![episode_id, user_id],
            |row| row.get(0),
        )?;
        if owns_episode == 0 {
            return Err(EngError::NotFound(format!("episode {}", episode_id)));
        }

        let mut assigned = 0_i64;
        for memory_id in &memory_ids {
            let count = conn.execute(
                "UPDATE memories SET episode_id = ?1 \
                     WHERE id = ?2 AND user_id = ?3 AND is_latest = 1 AND is_archived = 0",
                params![episode_id, *memory_id, user_id],
            )?;
            assigned += count as i64;
        }

        conn.execute(
            "UPDATE episodes
             SET memory_count = (
                 SELECT COUNT(*) FROM memories WHERE episode_id = ?1
             )
             WHERE id = ?1 AND user_id = ?2",
            params![episode_id, user_id],
        )?;

        Ok(assigned)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn finalize_episode(db: &Database, id: i64, user_id: i64) -> Result<EpisodeRow> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE episodes
             SET ended_at = COALESCE(ended_at, datetime('now')),
                 memory_count = (
                     SELECT COUNT(*) FROM memories WHERE episode_id = ?1
                 )
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
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
        episodes.push(row?);
    }
    Ok(episodes)
}

// The SELECT column lists omit user_id; every read scopes by it in the WHERE
// clause, so the row belongs to `owner_user_id` and the field is set from it.
fn row_to_episode(row: &rusqlite::Row<'_>, owner_user_id: i64) -> rusqlite::Result<EpisodeRow> {
    Ok(EpisodeRow {
        id: row.get(0)?,
        title: row.get(1)?,
        session_id: row.get(2)?,
        agent: row.get(3)?,
        summary: row.get(4)?,
        user_id: owner_user_id,
        memory_count: row.get::<_, i64>(5).unwrap_or(0),
        duration_seconds: row.get(6)?,
        decay_score: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}
