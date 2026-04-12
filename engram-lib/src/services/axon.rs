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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub retain_hours: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: i64,
    pub agent: String,
    pub channel: String,
    pub filter_type: Option<String>,
    pub webhook_url: Option<String>,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub agent: String,
    pub channel: String,
    pub last_event_id: i64,
    pub updated_at: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub agent: String,
    pub channel: String,
    pub filter_type: Option<String>,
    pub webhook_url: Option<String>,
}

fn row_to_event(row: &libsql::Row) -> Result<Event> {
    let payload_str: String = row.get(4)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    let source: String = row.get(2)?;
    Ok(Event {
        id: row.get(0)?,
        channel: row.get(1)?,
        source: Some(source.clone()),
        agent: Some(source),
        action: row.get(3)?,
        payload,
        created_at: row.get(5)?,
        user_id: row.get(6)?,
    })
}

#[cfg(feature = "db_pool")]
fn row_to_event_rusqlite(row: &rusqlite::Row<'_>) -> Result<Event> {
    let payload_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    let source: String = row.get(2).map_err(rusqlite_to_eng_error)?;
    Ok(Event {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        channel: row.get(1).map_err(rusqlite_to_eng_error)?,
        source: Some(source.clone()),
        agent: Some(source),
        action: row.get(3).map_err(rusqlite_to_eng_error)?,
        payload,
        created_at: row.get(5).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(6).map_err(rusqlite_to_eng_error)?,
    })
}

#[cfg(feature = "db_pool")]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

const EVENT_COLUMNS: &str = "id, channel, source, type, payload, created_at, user_id";

fn resolve_source(req: &PublishEventRequest) -> String {
    req.source
        .clone()
        .or_else(|| req.agent.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

pub async fn publish_event(db: &Database, req: PublishEventRequest) -> Result<Event> {
    let payload = req
        .payload
        .clone()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;
    let source = resolve_source(&req);
    let channel = req.channel.clone();
    let action = req.action.clone();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let c = channel.clone();
        let s = source.clone();
        let a = action.clone();
        let p = payload_str.clone();
        let id = db
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO axon_events (channel, source, type, payload, user_id)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![c, s, a, p, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(conn.last_insert_rowid())
            })
            .await?;
        return get_event(db, id, user_id).await;
    }

    let conn = &db.conn;
    conn.execute(
        "INSERT INTO axon_events (channel, source, type, payload, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        libsql::params![channel, source, action, payload_str, user_id],
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
    let sql = format!("SELECT {EVENT_COLUMNS} FROM axon_events WHERE id = ?1 AND user_id = ?2");

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
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
    let mut rows = conn.query(&sql, libsql::params![id, user_id]).await?;
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
    let mut sql = format!("SELECT {EVENT_COLUMNS} FROM axon_events WHERE user_id = ?1");
    let mut param_idx = 2usize;
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];

    if let Some(c) = channel {
        sql.push_str(&format!(" AND channel = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(c.to_string()));
        param_idx += 1;
    }
    if let Some(a) = action {
        sql.push_str(&format!(" AND type = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = source {
        sql.push_str(&format!(" AND source = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
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

pub async fn list_channels(db: &Database, _user_id: i64) -> Result<Vec<Channel>> {
    let sql = "SELECT id, name, description, retain_hours, created_at
               FROM axon_channels ORDER BY name ASC";

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt.query([]).map_err(rusqlite_to_eng_error)?;
                let mut results = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    results.push(Channel {
                        id: row.get(0).map_err(rusqlite_to_eng_error)?,
                        name: row.get(1).map_err(rusqlite_to_eng_error)?,
                        description: row.get(2).map_err(rusqlite_to_eng_error)?,
                        retain_hours: row.get(3).map_err(rusqlite_to_eng_error)?,
                        created_at: row.get(4).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(results)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(sql, ()).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(Channel {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            retain_hours: row.get(3)?,
            created_at: row.get(4)?,
        });
    }
    Ok(results)
}

pub async fn ensure_channel(
    db: &Database,
    name: String,
    description: Option<String>,
) -> Result<()> {
    let sql = "INSERT INTO axon_channels (name, description)
               VALUES (?1, ?2)
               ON CONFLICT(name) DO NOTHING";

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .write(move |conn| {
                conn.execute(sql, rusqlite::params![name, description])
                    .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(sql, libsql::params![name, description])
        .await?;
    Ok(())
}

pub async fn upsert_subscription(
    db: &Database,
    req: SubscribeRequest,
    user_id: i64,
) -> Result<Subscription> {
    let sql = "INSERT INTO axon_subscriptions (agent, channel, filter_type, webhook_url, user_id)
               VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(agent, channel) DO UPDATE SET
                   filter_type = excluded.filter_type,
                   webhook_url = excluded.webhook_url";

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let a = req.agent.clone();
        let c = req.channel.clone();
        let ft = req.filter_type.clone();
        let wh = req.webhook_url.clone();
        db.write(move |conn| {
            conn.execute(sql, rusqlite::params![a, c, ft, wh, user_id])
                .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;
        return get_subscription(db, &req.agent, &req.channel, user_id).await;
    }

    let conn = &db.conn;
    conn.execute(
        sql,
        libsql::params![
            req.agent.clone(),
            req.channel.clone(),
            req.filter_type.clone(),
            req.webhook_url.clone(),
            user_id
        ],
    )
    .await?;
    get_subscription(db, &req.agent, &req.channel, user_id).await
}

pub async fn get_subscription(
    db: &Database,
    agent: &str,
    channel: &str,
    user_id: i64,
) -> Result<Subscription> {
    let sql = "SELECT id, agent, channel, filter_type, webhook_url, user_id, created_at
               FROM axon_subscriptions
               WHERE agent = ?1 AND channel = ?2 AND user_id = ?3";
    let agent_s = agent.to_string();
    let channel_s = channel.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![agent_s, channel_s, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound("subscription".into()))?;
                Ok(Subscription {
                    id: row.get(0).map_err(rusqlite_to_eng_error)?,
                    agent: row.get(1).map_err(rusqlite_to_eng_error)?,
                    channel: row.get(2).map_err(rusqlite_to_eng_error)?,
                    filter_type: row.get(3).map_err(rusqlite_to_eng_error)?,
                    webhook_url: row.get(4).map_err(rusqlite_to_eng_error)?,
                    user_id: row.get(5).map_err(rusqlite_to_eng_error)?,
                    created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
                })
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(sql, libsql::params![agent_s, channel_s, user_id])
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound("subscription".into()))?;
    Ok(Subscription {
        id: row.get(0)?,
        agent: row.get(1)?,
        channel: row.get(2)?,
        filter_type: row.get(3)?,
        webhook_url: row.get(4)?,
        user_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub async fn delete_subscription(
    db: &Database,
    agent: &str,
    channel: &str,
    user_id: i64,
) -> Result<bool> {
    let sql = "DELETE FROM axon_subscriptions WHERE agent = ?1 AND channel = ?2 AND user_id = ?3";
    let a = agent.to_string();
    let c = channel.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let n = db
            .write(move |conn| {
                conn.execute(sql, rusqlite::params![a, c, user_id])
                    .map_err(rusqlite_to_eng_error)
            })
            .await?;
        return Ok(n > 0);
    }

    let conn = &db.conn;
    let n = conn.execute(sql, libsql::params![a, c, user_id]).await?;
    Ok(n > 0)
}

pub async fn list_subscriptions_for_agent(
    db: &Database,
    agent: &str,
    user_id: i64,
) -> Result<Vec<Subscription>> {
    let sql = "SELECT id, agent, channel, filter_type, webhook_url, user_id, created_at
               FROM axon_subscriptions
               WHERE agent = ?1 AND user_id = ?2
               ORDER BY channel ASC";
    let a = agent.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![a, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let mut results = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    results.push(Subscription {
                        id: row.get(0).map_err(rusqlite_to_eng_error)?,
                        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
                        channel: row.get(2).map_err(rusqlite_to_eng_error)?,
                        filter_type: row.get(3).map_err(rusqlite_to_eng_error)?,
                        webhook_url: row.get(4).map_err(rusqlite_to_eng_error)?,
                        user_id: row.get(5).map_err(rusqlite_to_eng_error)?,
                        created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(results)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(sql, libsql::params![a, user_id]).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(Subscription {
            id: row.get(0)?,
            agent: row.get(1)?,
            channel: row.get(2)?,
            filter_type: row.get(3)?,
            webhook_url: row.get(4)?,
            user_id: row.get(5)?,
            created_at: row.get(6)?,
        });
    }
    Ok(results)
}

pub async fn get_cursor(db: &Database, agent: &str, channel: &str, user_id: i64) -> Result<Cursor> {
    let sql = "SELECT agent, channel, last_event_id, updated_at, user_id
               FROM axon_cursors
               WHERE agent = ?1 AND channel = ?2 AND user_id = ?3";
    let a = agent.to_string();
    let c = channel.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![a.clone(), c.clone(), user_id])
                    .map_err(rusqlite_to_eng_error)?;
                match rows.next().map_err(rusqlite_to_eng_error)? {
                    Some(row) => Ok(Cursor {
                        agent: row.get(0).map_err(rusqlite_to_eng_error)?,
                        channel: row.get(1).map_err(rusqlite_to_eng_error)?,
                        last_event_id: row.get(2).map_err(rusqlite_to_eng_error)?,
                        updated_at: row.get(3).map_err(rusqlite_to_eng_error)?,
                        user_id: row.get(4).map_err(rusqlite_to_eng_error)?,
                    }),
                    None => Ok(Cursor {
                        agent: a,
                        channel: c,
                        last_event_id: 0,
                        updated_at: String::new(),
                        user_id,
                    }),
                }
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(sql, libsql::params![a.clone(), c.clone(), user_id])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Cursor {
            agent: row.get(0)?,
            channel: row.get(1)?,
            last_event_id: row.get(2)?,
            updated_at: row.get(3)?,
            user_id: row.get(4)?,
        }),
        None => Ok(Cursor {
            agent: a,
            channel: c,
            last_event_id: 0,
            updated_at: String::new(),
            user_id,
        }),
    }
}

async fn upsert_cursor(
    db: &Database,
    agent: &str,
    channel: &str,
    last_event_id: i64,
    user_id: i64,
) -> Result<()> {
    let sql = "INSERT INTO axon_cursors (agent, channel, last_event_id, updated_at, user_id)
               VALUES (?1, ?2, ?3, datetime('now'), ?4)
               ON CONFLICT(agent, channel) DO UPDATE SET
                   last_event_id = excluded.last_event_id,
                   updated_at = excluded.updated_at,
                   user_id = excluded.user_id";
    let a = agent.to_string();
    let c = channel.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .write(move |conn| {
                conn.execute(sql, rusqlite::params![a, c, last_event_id, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(sql, libsql::params![a, c, last_event_id, user_id])
        .await?;
    Ok(())
}

pub async fn consume(
    db: &Database,
    agent: &str,
    channel: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<Event>> {
    let cursor = get_cursor(db, agent, channel, user_id).await?;
    let last = cursor.last_event_id;
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM axon_events
         WHERE channel = ?1 AND user_id = ?2 AND id > ?3
         ORDER BY id ASC LIMIT ?4"
    );
    let channel_s = channel.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let c = channel_s.clone();
        let events: Vec<Event> = db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![c, user_id, last, limit as i64])
                    .map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    out.push(row_to_event_rusqlite(row)?);
                }
                Ok(out)
            })
            .await?;
        if let Some(max_id) = events.iter().map(|e| e.id).max() {
            upsert_cursor(db, agent, channel, max_id, user_id).await?;
        }
        return Ok(events);
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(
            &sql,
            libsql::params![channel_s, user_id, last, limit as i64],
        )
        .await?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await? {
        events.push(row_to_event(&row)?);
    }
    if let Some(max_id) = events.iter().map(|e| e.id).max() {
        upsert_cursor(db, agent, channel, max_id, user_id).await?;
    }
    Ok(events)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<AxonStats> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let stats = if let Some(uid) = user_id {
                    conn.query_row(
                        "SELECT COUNT(*), COUNT(DISTINCT channel), COUNT(DISTINCT source)
                         FROM axon_events WHERE user_id = ?1",
                        rusqlite::params![uid],
                        |row| {
                            Ok(AxonStats {
                                total_events: row.get(0)?,
                                channels: row.get(1)?,
                                sources: row.get(2)?,
                            })
                        },
                    )
                    .map_err(rusqlite_to_eng_error)?
                } else {
                    conn.query_row(
                        "SELECT COUNT(*), COUNT(DISTINCT channel), COUNT(DISTINCT source)
                         FROM axon_events",
                        [],
                        |row| {
                            Ok(AxonStats {
                                total_events: row.get(0)?,
                                channels: row.get(1)?,
                                sources: row.get(2)?,
                            })
                        },
                    )
                    .map_err(rusqlite_to_eng_error)?
                };
                Ok(stats)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT COUNT(*), COUNT(DISTINCT channel), COUNT(DISTINCT source)
             FROM axon_events WHERE user_id = ?1",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query(
            "SELECT COUNT(*), COUNT(DISTINCT channel), COUNT(DISTINCT source)
             FROM axon_events",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    #[tokio::test]
    async fn publish_and_get_event() {
        let db = setup().await;
        let ev = publish_event(
            &db,
            PublishEventRequest {
                channel: "test".into(),
                action: "ping".into(),
                payload: Some(serde_json::json!({"k": "v"})),
                source: Some("agent-a".into()),
                agent: None,
                user_id: Some(1),
            },
        )
        .await
        .expect("publish");
        assert_eq!(ev.channel, "test");
        assert_eq!(ev.action, "ping");
        assert_eq!(ev.source.as_deref(), Some("agent-a"));
        let fetched = get_event(&db, ev.id, 1).await.expect("get");
        assert_eq!(fetched.id, ev.id);
    }

    #[tokio::test]
    async fn consume_advances_cursor() {
        let db = setup().await;
        for i in 0..3 {
            publish_event(
                &db,
                PublishEventRequest {
                    channel: "cons".into(),
                    action: format!("act-{i}"),
                    payload: None,
                    source: Some("src".into()),
                    agent: None,
                    user_id: Some(1),
                },
            )
            .await
            .expect("publish");
        }
        let first = consume(&db, "agent-x", "cons", 10, 1).await.expect("c1");
        assert_eq!(first.len(), 3);
        let second = consume(&db, "agent-x", "cons", 10, 1).await.expect("c2");
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn consume_is_scoped_by_user() {
        let db = setup().await;
        publish_event(
            &db,
            PublishEventRequest {
                channel: "s".into(),
                action: "a".into(),
                payload: None,
                source: Some("x".into()),
                agent: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let other = consume(&db, "who", "s", 10, 2).await.unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn subscription_upsert_is_idempotent() {
        let db = setup().await;
        upsert_subscription(
            &db,
            SubscribeRequest {
                agent: "a1".into(),
                channel: "c1".into(),
                filter_type: Some("ping".into()),
                webhook_url: None,
            },
            1,
        )
        .await
        .unwrap();
        let s2 = upsert_subscription(
            &db,
            SubscribeRequest {
                agent: "a1".into(),
                channel: "c1".into(),
                filter_type: Some("pong".into()),
                webhook_url: None,
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(s2.filter_type.as_deref(), Some("pong"));
        let all = list_subscriptions_for_agent(&db, "a1", 1).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn list_channels_returns_seeded() {
        let db = setup().await;
        let channels = list_channels(&db, 1).await.unwrap();
        assert!(channels.iter().any(|c| c.name == "system"));
    }
}
