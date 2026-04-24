use crate::db::Database;
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

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

fn row_to_event(row: &rusqlite::Row<'_>, owner_user_id: i64) -> Result<Event> {
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
        user_id: owner_user_id,
    })
}

const EVENT_COLUMNS: &str = "id, channel, source, type, payload, created_at";

fn resolve_source(req: &PublishEventRequest) -> String {
    req.source
        .clone()
        .or_else(|| req.agent.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tracing::instrument(skip(db, req), fields(channel = %req.channel, action = %req.action))]
pub async fn publish_event(db: &Database, req: PublishEventRequest) -> Result<Event> {
    let payload = req
        .payload
        .clone()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req.user_id.unwrap_or(1);
    let source = resolve_source(&req);
    let channel = req.channel.clone();
    let action = req.action.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO axon_events (channel, source, type, payload)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![channel, source, action, payload_str],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    get_event(db, id, user_id).await
}

#[tracing::instrument(skip(db), fields(event_id = id, user_id))]
pub async fn get_event(db: &Database, id: i64, user_id: i64) -> Result<Event> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM axon_events WHERE id = ?1");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("event {}", id)))?;
        row_to_event(row, user_id)
    })
    .await
}

#[tracing::instrument(skip(db), fields(channel = ?channel, action = ?action, source = ?source, limit, offset, user_id))]
pub async fn query_events(
    db: &Database,
    channel: Option<&str>,
    action: Option<&str>,
    source: Option<&str>,
    limit: usize,
    offset: usize,
    user_id: i64,
) -> Result<Vec<Event>> {
    let mut sql = format!("SELECT {EVENT_COLUMNS} FROM axon_events");
    let mut clauses: Vec<String> = Vec::new();
    let mut param_idx = 1usize;
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(c) = channel {
        clauses.push(format!("channel = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(c.to_string()));
        param_idx += 1;
    }
    if let Some(a) = action {
        clauses.push(format!("type = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = source {
        clauses.push(format!("source = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        param_idx += 1;
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }

    sql.push_str(&format!(
        " ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(rusqlite::types::Value::Integer(limit as i64));
    params_vec.push(rusqlite::types::Value::Integer(offset as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let params = rusqlite::params_from_iter(params_vec.iter().cloned());
        let mut rows = stmt.query(params).map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_event(row, user_id)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_channels(db: &Database, _user_id: i64) -> Result<Vec<Channel>> {
    let sql = "SELECT id, name, description, retain_hours, created_at
               FROM axon_channels ORDER BY name ASC";

    db.read(move |conn| {
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
    .await
}

#[tracing::instrument(skip(db, description), fields(name = %name))]
pub async fn ensure_channel(
    db: &Database,
    name: String,
    description: Option<String>,
) -> Result<()> {
    let sql = "INSERT INTO axon_channels (name, description)
               VALUES (?1, ?2)
               ON CONFLICT(name) DO NOTHING";

    db.write(move |conn| {
        conn.execute(sql, rusqlite::params![name, description])
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, req), fields(agent = %req.agent, channel = %req.channel, user_id))]
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
    get_subscription(db, &req.agent, &req.channel, user_id).await
}

#[tracing::instrument(skip(db), fields(agent = %agent, channel = %channel, user_id))]
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

    db.read(move |conn| {
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
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent, channel = %channel, user_id))]
pub async fn delete_subscription(
    db: &Database,
    agent: &str,
    channel: &str,
    user_id: i64,
) -> Result<bool> {
    let sql = "DELETE FROM axon_subscriptions WHERE agent = ?1 AND channel = ?2 AND user_id = ?3";
    let a = agent.to_string();
    let c = channel.to_string();

    let n = db
        .write(move |conn| {
            conn.execute(sql, rusqlite::params![a, c, user_id])
                .map_err(rusqlite_to_eng_error)
        })
        .await?;
    Ok(n > 0)
}

#[tracing::instrument(skip(db), fields(agent = %agent, user_id))]
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

    db.read(move |conn| {
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
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent, channel = %channel, user_id))]
pub async fn get_cursor(db: &Database, agent: &str, channel: &str, user_id: i64) -> Result<Cursor> {
    let sql = "SELECT agent, channel, last_event_id, updated_at, user_id
               FROM axon_cursors
               WHERE agent = ?1 AND channel = ?2 AND user_id = ?3";
    let a = agent.to_string();
    let c = channel.to_string();

    db.read(move |conn| {
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
    .await
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

    db.write(move |conn| {
        conn.execute(sql, rusqlite::params![a, c, last_event_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent, channel = %channel, limit, user_id))]
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
         WHERE channel = ?1 AND id > ?2
         ORDER BY id ASC LIMIT ?3"
    );
    let channel_s = channel.to_string();

    let events: Vec<Event> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![channel_s, last, limit as i64])
                .map_err(rusqlite_to_eng_error)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                out.push(row_to_event(row, user_id)?);
            }
            Ok(out)
        })
        .await?;
    if let Some(max_id) = events.iter().map(|e| e.id).max() {
        upsert_cursor(db, agent, channel, max_id, user_id).await?;
    }
    Ok(events)
}

#[tracing::instrument(skip(db), fields(user_id = ?_user_id))]
pub async fn get_stats(db: &Database, _user_id: Option<i64>) -> Result<AxonStats> {
    db.read(move |conn| {
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
        .map_err(rusqlite_to_eng_error)
    })
    .await
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

    /// Phase 5.8 dropped user_id from axon_events: tenant isolation is now at
    /// the database level. A shared in-memory DB no longer separates user 1
    /// from user 2 on axon_events. The tenant-aware form of this invariant
    /// lands in kleos-server/tests once Phase 4.2 wires the tenant-aware
    /// test harness.
    #[tokio::test]
    #[ignore]
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
