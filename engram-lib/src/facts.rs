use crate::db::Database;
use crate::{EngError, Result};
use libsql::params;
use serde::{Deserialize, Serialize};

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

fn row_to_fact(row: &libsql::Row) -> Result<StructuredFact> {
    Ok(StructuredFact {
        id: row.get::<i64>(0)?,
        memory_id: row.get::<Option<i64>>(1)?,
        subject: row.get::<String>(2)?,
        predicate: row.get::<String>(3)?,
        object: row.get::<String>(4)?,
        confidence: row.get::<f64>(5)?,
        user_id: row.get::<i64>(6)?,
        created_at: row.get::<String>(7)?,
    })
}

fn row_to_state(row: &libsql::Row) -> Result<CurrentState> {
    Ok(CurrentState {
        id: row.get::<i64>(0)?,
        agent: row.get::<String>(1)?,
        key: row.get::<String>(2)?,
        value: row.get::<String>(3)?,
        user_id: row.get::<i64>(4)?,
        created_at: row.get::<String>(5)?,
        updated_at: row.get::<String>(6)?,
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
        let mut rows = db
            .conn
            .query(
                "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                params![mid, user_id],
            )
            .await?;
        if rows.next().await?.is_none() {
            return Err(EngError::NotFound(format!(
                "memory {} not found for user",
                mid
            )));
        }
    }

    db.conn
        .execute(
            "INSERT INTO structured_facts \
             (memory_id, subject, predicate, object, confidence, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                req.memory_id,
                req.subject,
                req.predicate,
                req.object,
                confidence,
                user_id
            ],
        )
        .await?;

    let mut id_rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let new_id: i64 = if let Some(row) = id_rows.next().await? {
        row.get(0)?
    } else {
        return Err(EngError::Internal(
            "failed to get last insert id for structured_fact".to_string(),
        ));
    };

    // SECURITY (MT-F6): scope the re-fetch by user_id even though we just
    // inserted the row. Defense-in-depth against any future change that
    // moves the insert and select onto separate connections.
    let sql = format!(
        "SELECT {} FROM structured_facts WHERE id = ?1 AND user_id = ?2",
        FACT_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![new_id, user_id]).await?;
    if let Some(row) = rows.next().await? {
        row_to_fact(&row)
    } else {
        Err(EngError::Internal(
            "failed to fetch newly created fact".to_string(),
        ))
    }
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

    let mut rows = db.conn.query(&sql, params![user_id]).await?;
    let mut facts = Vec::new();
    while let Some(row) = rows.next().await? {
        facts.push(row_to_fact(&row)?);
    }
    Ok(facts)
}

/// Hard-delete a structured fact by id (tenant-scoped).
pub async fn delete_fact(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM structured_facts WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
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
    db.conn
        .execute(
            "INSERT INTO current_state (agent, key, value, user_id) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(agent, key, user_id) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = datetime('now')",
            params![agent, key, value, user_id],
        )
        .await?;

    get_state(db, agent, key, user_id).await
}

/// Fetch a single state entry for the given agent/key/user.
pub async fn get_state(
    db: &Database,
    agent: &str,
    key: &str,
    user_id: i64,
) -> Result<CurrentState> {
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND key = ?2 AND user_id = ?3",
        STATE_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![agent, key, user_id]).await?;

    if let Some(row) = rows.next().await? {
        row_to_state(&row)
    } else {
        Err(EngError::NotFound(format!(
            "state {}/{} not found for user {}",
            agent, key, user_id
        )))
    }
}

/// List all state entries for the given agent and user.
pub async fn list_state(db: &Database, agent: &str, user_id: i64) -> Result<Vec<CurrentState>> {
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND user_id = ?2 ORDER BY key ASC",
        STATE_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![agent, user_id]).await?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        entries.push(row_to_state(&row)?);
    }
    Ok(entries)
}
