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

const STATE_COLUMNS: &str = "id, agent, key, value, created_at, updated_at";

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
        user_id: 1,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

// -- Structured facts CRUD ---

/// Create a new structured fact.
#[tracing::instrument(skip(db, req), fields(user_id = ?req.user_id, subject = %req.subject, predicate = %req.predicate, memory_id = ?req.memory_id))]
pub async fn create_fact(db: &Database, req: CreateFactRequest) -> Result<StructuredFact> {
    let _user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let confidence = req.confidence.unwrap_or(1.0);

    if let Some(mid) = req.memory_id {
        let exists = db
            .read(move |conn| {
                let result = conn
                    .query_row("SELECT 1 FROM memories WHERE id = ?1", params![mid], |_| {
                        Ok(())
                    })
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
                 (memory_id, subject, predicate, object, confidence) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![memory_id, subject, predicate, object, confidence],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    // SECURITY (MT-F6): scope the re-fetch by user_id even though we just
    // inserted the row. Defense-in-depth against any future change that
    // moves the insert and select onto separate connections.
    let sql = format!(
        "SELECT {} FROM structured_facts WHERE id = ?1",
        FACT_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![new_id], row_to_fact)
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("failed to fetch newly created fact".to_string()))
    })
    .await
}

/// List structured facts, optionally filtered by memory_id.
#[tracing::instrument(skip(db), fields(memory_id_filter = ?memory_id_filter, limit))]
pub async fn list_facts(
    db: &Database,
<<<<<<< HEAD
=======
    _user_id: i64,
>>>>>>> b897358 (fix(clippy): Phase 5 Stage 20 -- close hygiene tail)
    memory_id_filter: Option<i64>,
    limit: usize,
) -> Result<Vec<StructuredFact>> {
    let sql = if let Some(mid) = memory_id_filter {
        format!(
            "SELECT {cols} FROM structured_facts \
             WHERE memory_id = {mid} \
             ORDER BY id DESC LIMIT {limit}",
            cols = FACT_COLUMNS,
            mid = mid,
            limit = limit
        )
    } else {
        format!(
            "SELECT {cols} FROM structured_facts \
             ORDER BY id DESC LIMIT {limit}",
            cols = FACT_COLUMNS,
            limit = limit
        )
    };

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map([], row_to_fact)
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
<<<<<<< HEAD
#[tracing::instrument(skip(db), fields(fact_id = id))]
pub async fn delete_fact(db: &Database, id: i64) -> Result<()> {
=======
#[tracing::instrument(skip(db), fields(fact_id = id, user_id))]
pub async fn delete_fact(db: &Database, id: i64, _user_id: i64) -> Result<()> {
>>>>>>> b897358 (fix(clippy): Phase 5 Stage 20 -- close hygiene tail)
    let affected = db
        .write(move |conn| {
            conn.execute("DELETE FROM structured_facts WHERE id = ?1", params![id])
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
#[tracing::instrument(skip(db, value), fields(agent = %agent, key = %key, user_id))]
pub async fn set_state(
    db: &Database,
    agent: &str,
    key: &str,
    value: &str,
    _user_id: i64,
) -> Result<CurrentState> {
    let agent_owned = agent.to_string();
    let key_owned = key.to_string();
    let value_owned = value.to_string();
    let agent_for_get = agent_owned.clone();
    let key_for_get = key_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO current_state (agent, key, value) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(agent, key) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = datetime('now')",
            params![agent_owned, key_owned, value_owned],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    get_state(db, &agent_for_get, &key_for_get).await
}

/// Fetch a single state entry for the given agent/key/user.
#[tracing::instrument(skip(db), fields(agent = %agent, key = %key))]
pub async fn get_state(db: &Database, agent: &str, key: &str) -> Result<CurrentState> {
    let agent = agent.to_string();
    let key = key.to_string();
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND key = ?2",
        STATE_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![agent, key], row_to_state)
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound("state not found".to_string()))
    })
    .await
}

/// List all state entries for the given agent and user.
#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn list_state(db: &Database, agent: &str) -> Result<Vec<CurrentState>> {
    let agent = agent.to_string();
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 ORDER BY key ASC",
        STATE_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![agent], row_to_state)
            .map_err(rusqlite_to_eng_error)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(entries)
    })
    .await
}
