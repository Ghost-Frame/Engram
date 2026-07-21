use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
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
    pub created_at: String,
}

/// Input payload for creating a new structured fact (subject/predicate/object triple).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFactRequest {
    pub memory_id: Option<i64>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: Option<f64>,
}

/// A single agent/key-value entry in the current_state table, scoped to a user.
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

const FACT_COLUMNS: &str = "id, memory_id, subject, predicate, object, confidence, created_at";

/// Column list for SELECT queries on current_state. Includes user_id so that
/// row_to_state can read it from position 4.
const STATE_COLUMNS: &str = "id, agent, key, value, user_id, created_at, updated_at";

// -- Helpers ---

/// Map a structured_facts SELECT row (FACT_COLUMNS order) to a StructuredFact.
fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<StructuredFact> {
    Ok(StructuredFact {
        id: row.get(0)?,
        memory_id: row.get(1)?,
        subject: row.get(2)?,
        predicate: row.get(3)?,
        object: row.get(4)?,
        confidence: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Map a current_state SELECT row (STATE_COLUMNS order) to a CurrentState.
/// Column positions must match STATE_COLUMNS exactly.
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

/// Create a new structured fact scoped to the given user.
/// The `user_id` is written into the `user_id` column and is used to
/// scope the post-insert re-fetch so multi-tenant single-DB installs
/// cannot observe another user's newly inserted rows.
#[tracing::instrument(skip(db, req), fields(subject = %req.subject, predicate = %req.predicate, memory_id = ?req.memory_id, user_id))]
pub async fn create_fact(
    db: &Database,
    req: CreateFactRequest,
    user_id: i64,
) -> Result<StructuredFact> {
    let confidence = req.confidence.unwrap_or(1.0);
    // Write-side guard (deep-sweep F7 residual): confidence is a probability.
    // NaN/Inf/out-of-range values previously landed in the column and every
    // consumer needed its own read-side guard; reject at the boundary instead.
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(EngError::InvalidInput(
            "confidence must be a finite value between 0.0 and 1.0".to_string(),
        ));
    }

    if let Some(mid) = req.memory_id {
        let exists = db
            .read(move |conn| {
                let result = conn
                    .query_row(
                        "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                        params![mid, user_id],
                        |_| Ok(()),
                    )
                    .optional()?;
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
            )?;
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
        conn.query_row(&sql, params![new_id, user_id], row_to_fact)
            .optional()?
            .ok_or_else(|| EngError::Internal("failed to fetch newly created fact".to_string()))
    })
    .await
}

/// List structured facts for a user, optionally filtered by memory_id.
/// The `user_id` predicate enforces single-DB isolation so users sharing
/// one monolith DB cannot observe each other's facts.
#[tracing::instrument(skip(db), fields(memory_id_filter = ?memory_id_filter, limit, user_id))]
pub async fn list_facts(
    db: &Database,
    memory_id_filter: Option<i64>,
    limit: usize,
    user_id: i64,
) -> Result<Vec<StructuredFact>> {
    let sql = if let Some(mid) = memory_id_filter {
        format!(
            "SELECT {cols} FROM structured_facts \
             WHERE memory_id = {mid} AND user_id = ?1 \
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
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user_id], row_to_fact)?;
        let mut facts = Vec::new();
        for row in rows {
            facts.push(row?);
        }
        Ok(facts)
    })
    .await
}

// -- Current state (per-agent key-value) ---

/// Upsert a state entry for the given agent/key/user combination.
/// The user_id is included in the UNIQUE constraint (agent, key, user_id) so
/// each user maintains independent state for the same key.
#[tracing::instrument(skip(db, value), fields(agent = %agent, key = %key, user_id))]
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
        )?;
        Ok(())
    })
    .await?;

    get_state(db, &agent_for_get, &key_for_get, user_id).await
}

/// Fetch a single state entry for the given agent/key/user.
/// The WHERE predicate includes user_id to enforce single-DB isolation.
#[tracing::instrument(skip(db), fields(agent = %agent, key = %key, user_id))]
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
        conn.query_row(&sql, params![agent, key, user_id], row_to_state)
            .optional()?
            .ok_or_else(|| EngError::NotFound("state not found".to_string()))
    })
    .await
}

/// List all state entries for the given agent and user.
/// The WHERE predicate includes user_id to enforce single-DB isolation.
#[tracing::instrument(skip(db), fields(agent = %agent, user_id))]
pub async fn list_state(db: &Database, agent: &str, user_id: i64) -> Result<Vec<CurrentState>> {
    let agent = agent.to_string();
    let sql = format!(
        "SELECT {} FROM current_state WHERE agent = ?1 AND user_id = ?2 ORDER BY key ASC",
        STATE_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![agent, user_id], row_to_state)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    })
    .await
}
