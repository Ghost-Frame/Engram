use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// -- Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredFact {
    pub id: i64,
    pub memory_id: Option<i64>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFactRequest {
    pub memory_id: Option<i64>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: Option<f64>,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentState {
    pub id: i64,
    pub agent: String,
    pub key: String,
    pub value: String,
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

// -- Constants ---

const FACT_COLUMNS: &str =
    "id, memory_id, subject, predicate, object, confidence, user_id, created_at";

const STATE_COLUMNS: &str = "id, agent, key, value, user_id, created_at, updated_at";

// -- Helpers ---

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<StructuredFact> {
    Ok(StructuredFact {
        id: row.get(0)?,
        memory_id: row.get(1)?,
        subject: row.get(2)?,
        predicate: row.get(3)?,
        object: row.get(4)?,
        confidence: row.get(5)?,
        user_id: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn row_to_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<CurrentState> {
    Ok(CurrentState {
        id: row.get(0)?,
        agent: row.get(1)?,
        key: row.get(2)?,
        value: row.get(3)?,
        user_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

// -- Structured facts CRUD ---

/// Create a new structured fact.
pub async fn create_fact(db: &Database, req: CreateFactRequest) -> Result<StructuredFact> {
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let confidence = req.confidence.unwrap_or(1.0);

    if let Some(mid) = req.memory_id {
        let exists = db
            .read(move |conn| {
                let result = conn
                    .query_row(
                        "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                        params![mid, user_id],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(rusqlite_to_eng_error)?;
                Ok(result.is_some())
            })
            .await?;
        if !exists {
            return Err(EngError::NotFound(format!(
                "memory {} not found for user",
                mid
            )));
        }
    }

    let memory_id = req.memory_id;
    let subject = req.subject.clone();
    let predicate = req.predicate.clone();
    let object = req.object.clone();

    let new_id: i64 = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO structured_facts \
                 (memory_id, subject, predicate, object, confidence, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![memory_id, subject, predicate, object, confidence, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    // SECURITY (MT-F6): scope the re-fetch by user_id even though we just
    // inserted the row. Defense-in-depth against any future change that
    // moves the insert and select onto separate connections.
    let sql = format!(
        "SELECT {} FROM structured_facts WHERE id = ?1 AND user_id = ?2",
        FACT_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![new_id, user_id], |row| row_to_fact(row))
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("failed to fetch newly created fact".to_string()))
    })
    .await
}

/// List structured facts for a user, optionally filtered by memory_id.
pub async fn list_facts(
    db: &Database,
    user_id: i64,
    memory_id_filter: Option<i64>,
    limit: usize,
) -> Result<Vec<StructuredFact>> {
    let sql = if let Some(mid) = memory_id_filter {
        format!(
            "SELECT {cols} FROM structured_facts \
             WHERE user_id = ?1 AND memory_id = {mid} \
             ORDER BY id DESC LIMIT {limit}",
            cols = FACT_COLUMNS,
            mid = mid,
            limit = limit
        )
    } else {
        format!(
            "SELECT {cols} FROM structured_facts \
             WHERE user_id = ?1 \
             ORDER BY id DESC LIMIT {limit}",
            cols = FACT_COLUMNS,
            limit = limit
        )
    };

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![user_id], |row| row_to_fact(row))
            .map_err(rusqlite_to_eng_error)?;
        let mut facts = Vec::new();
        for row in rows {
            facts.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(facts)
    })
    .await
}

/// Hard-delete a structured fact by id (tenant-scoped).
pub async fn delete_fact(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM structured_facts WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "structured_fact {} not found",
            id
        )));
    }
    Ok(())
}

// -- Current state (per-agent key-value) ---

/// Upsert a state entry for the given agent/key/user combination.
pub async fn set_state(
    db: &Database,
    agent: &str,
    key: &str,
    value: &str,
    user_id: i64,
) -> Result<CurrentState> {
    let agent_owned = agent.to_string();
    let key_owned = key.to_string();
    let value_owned = value.to_string();
    let agent_for_get = agent_owned.clone();
    let key_for_get = key_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO current_state (agent, key, value, user_id) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(agent, key, user_id) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = datetime('now')",
            params![agent_owned, key_owned, value_owned, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    get_state(db, &agent_for_get, &key_for_get, user_id).await
}

/// Fetch a single state entry for the given agent/key/user.
pub async fn get_state(
    db: &Database,
    agent: &str,
    key: &str,
    user_id: i64,
) -> Result<CurrentState> {
    let agent = agent.to_string();
    let key = key.to_string();
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND key = ?2 AND user_id = ?3",
        STATE_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![agent, key, user_id], |row| row_to_state(row))
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| {
                EngError::NotFound(format!("state not found for user {}", user_id))
            })
    })
    .await
}

/// List all state entries for the given agent and user.
pub async fn list_state(db: &Database, agent: &str, user_id: i64) -> Result<Vec<CurrentState>> {
    let agent = agent.to_string();
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND user_id = ?2 ORDER BY key ASC",
        STATE_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![agent, user_id], |row| row_to_state(row))
            .map_err(rusqlite_to_eng_error)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(entries)
    })
    .await
}
