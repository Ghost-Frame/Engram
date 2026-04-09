use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::{EngError, Result};

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

pub async fn publish_event(db: &Database, req: PublishEventRequest) -> Result<Event> {
    let conn = &db.conn;

    let payload = req.payload.unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req.user_id.unwrap_or(1);

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
    let id_row = rows.next().await?.ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    get_event(db, id).await
}

pub async fn get_event(db: &Database, id: i64) -> Result<Event> {
    let conn = &db.conn;
    let mut rows = conn.query(
        "SELECT id, channel, payload, action, source, agent, user_id, created_at
         FROM events WHERE id = ?1",
        libsql::params![id],
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
) -> Result<Vec<Event>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, channel, payload, action, source, agent, user_id, created_at
         FROM events WHERE 1=1",
    );

    let mut param_idx = 1usize;
    let mut params_vec: Vec<libsql::Value> = Vec::new();

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

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_event(&row)?);
    }
    Ok(results)
}

pub async fn list_channels(db: &Database) -> Result<Vec<String>> {
    let conn = &db.conn;
    let mut rows = conn
        .query("SELECT DISTINCT channel FROM events ORDER BY channel ASC", ())
        .await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        let channel: String = row.get(0)?;
        results.push(channel);
    }
    Ok(results)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<AxonStats> {
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
