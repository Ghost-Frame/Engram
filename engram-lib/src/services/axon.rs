use crate::db::Database;
#[cfg(feature = "db_pool")]
use crate::memory::{libsql_value_to_rusqlite_value, uses_pool_backend};
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub channel: String,
    pub action: String,
    pub payload: serde_json::Value,
    pub source: Option<String>,
    pub agent: Option<String>,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEventRequest {
    pub channel: String,
    pub action: String,
    pub payload: Option<serde_json::Value>,
    pub source: Option<String>,
    pub agent: Option<String>,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxonStats {
    pub total_events: i64,
    pub channels: i64,
    pub sources: i64,
}

fn row_to_event(row: &libsql::Row) -> Result<Event> {
    let payload_str: String = row.get(2)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    Ok(Event {
        id: row.get(0)?,
        channel: row.get(1)?,
        payload,
        action: row.get(3)?,
        source: row.get(4)?,
        agent: row.get(5)?,
        user_id: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(feature = "db_pool")]
fn row_to_event_rusqlite(row: &rusqlite::Row<'_>) -> Result<Event> {
    let payload_str: String = row.get(2).map_err(rusqlite_to_eng_error)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    Ok(Event {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        channel: row.get(1).map_err(rusqlite_to_eng_error)?,
        payload,
        action: row.get(3).map_err(rusqlite_to_eng_error)?,
        source: row.get(4).map_err(rusqlite_to_eng_error)?,
        agent: row.get(5).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(6).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(7).map_err(rusqlite_to_eng_error)?,
    })
}

#[cfg(feature = "db_pool")]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

pub async fn publish_event(db: &Database, req: PublishEventRequest) -> Result<Event> {
    let payload = req
        .payload
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req.user_id.unwrap_or(1);

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let channel = req.channel;
        let action = req.action;
        let source = req.source;
        let agent = req.agent;
        let id = db
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO events (channel, action, payload, source, agent, user_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![channel, action, payload_str, source, agent, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(conn.last_insert_rowid())
            })
            .await?;
        return get_event(db, id, user_id).await;
    }

    let conn = &db.conn;

    conn.execute(
        "INSERT INTO events (channel, action, payload, source, agent, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        libsql::params![
            req.channel,
            req.action,
            payload_str,
            req.source,
            req.agent,
            user_id,
        ],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    get_event(db, id, user_id).await
}

pub async fn get_event(db: &Database, id: i64, user_id: i64) -> Result<Event> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, channel, payload, action, source, agent, user_id, created_at
                         FROM events WHERE id = ?1 AND user_id = ?2",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![id, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound(format!("event {}", id)))?;
                row_to_event_rusqlite(row)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, channel, payload, action, source, agent, user_id, created_at
         FROM events WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("event {}", id)))?;

    row_to_event(&row)
}

pub async fn query_events(
    db: &Database,
    channel: Option<&str>,
    action: Option<&str>,
    source: Option<&str>,
    limit: usize,
    offset: usize,
    user_id: i64,
) -> Result<Vec<Event>> {
    let mut sql = String::from(
        "SELECT id, channel, payload, action, source, agent, user_id, created_at
         FROM events WHERE user_id = ?1",
    );

    let mut param_idx = 2usize;
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];

    if let Some(c) = channel {
        sql.push_str(&format!(" AND channel = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(c.to_string()));
        param_idx += 1;
    }
    if let Some(a) = action {
        sql.push_str(&format!(" AND action = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = source {
        sql.push_str(&format!(" AND source = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(libsql::Value::Integer(limit as i64));
    params_vec.push(libsql::Value::Integer(offset as i64));

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let params = rusqlite::params_from_iter(
                    params_vec.iter().map(libsql_value_to_rusqlite_value),
                );
                let mut rows = stmt.query(params).map_err(rusqlite_to_eng_error)?;
                let mut results = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    results.push(row_to_event_rusqlite(row)?);
                }
                Ok(results)
            })
            .await;
    }

    let conn = &db.conn;

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(params_vec))
        .await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_event(&row)?);
    }
    Ok(results)
}

pub async fn list_channels(db: &Database, user_id: i64) -> Result<Vec<String>> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT DISTINCT channel FROM events WHERE user_id = ?1 ORDER BY channel ASC")
                    .map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let mut results = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    results.push(row.get(0).map_err(rusqlite_to_eng_error)?);
                }
                Ok(results)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT DISTINCT channel FROM events WHERE user_id = ?1 ORDER BY channel ASC",
            libsql::params![user_id],
        )
        .await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        let channel: String = row.get(0)?;
        results.push(channel);
    }
    Ok(results)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<AxonStats> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let sql = if user_id.is_some() {
                    "SELECT
                        COUNT(*),
                        COUNT(DISTINCT channel),
                        COUNT(DISTINCT source)
                     FROM events
                     WHERE user_id = ?1"
                } else {
                    "SELECT
                        COUNT(*),
                        COUNT(DISTINCT channel),
                        COUNT(DISTINCT source)
                     FROM events"
                };

                let stats = if let Some(uid) = user_id {
                    conn.query_row(sql, rusqlite::params![uid], |row| {
                        Ok(AxonStats {
                            total_events: row.get(0)?,
                            channels: row.get(1)?,
                            sources: row.get(2)?,
                        })
                    })
                    .map_err(rusqlite_to_eng_error)?
                } else {
                    conn.query_row(sql, [], |row| {
                        Ok(AxonStats {
                            total_events: row.get(0)?,
                            channels: row.get(1)?,
                            sources: row.get(2)?,
                        })
                    })
                    .map_err(rusqlite_to_eng_error)?
                };

                Ok(stats)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT channel),
                COUNT(DISTINCT source)
             FROM events
             WHERE user_id = ?1",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT channel),
                COUNT(DISTINCT source)
             FROM events",
            (),
        )
        .await?
    };

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no axon stats row".into()))?;

    Ok(AxonStats {
        total_events: row.get(0)?,
        channels: row.get(1)?,
        sources: row.get(2)?,
    })
}
