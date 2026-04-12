use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalChain {
    pub id: i64,
    pub root_memory_id: Option<i64>,
    pub description: Option<String>,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
    pub links: Vec<CausalLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalLink {
    pub id: i64,
    pub chain_id: i64,
    pub cause_memory_id: i64,
    pub effect_memory_id: i64,
    pub strength: f64,
    pub order_index: i32,
    pub created_at: String,
}

/// Create a causal chain.
pub async fn create_chain(
    db: &Database,
    root_memory_id: Option<i64>,
    description: Option<&str>,
    user_id: i64,
) -> Result<CausalChain> {
    let description_owned = description.map(|s| s.to_string());
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO causal_chains (root_memory_id, description, user_id) VALUES (?1, ?2, ?3)",
                params![root_memory_id, description_owned, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(CausalChain {
        id,
        root_memory_id,
        description: description.map(|s| s.to_string()),
        confidence: 1.0,
        user_id,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        links: Vec::new(),
    })
}

/// Add a causal link to a chain.
pub async fn add_link(
    db: &Database,
    chain_id: i64,
    cause_memory_id: i64,
    effect_memory_id: i64,
    strength: f64,
    order_index: i32,
    user_id: i64,
) -> Result<CausalLink> {
    // Verify chain belongs to caller
    let chain_exists = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM causal_chains WHERE id = ?1 AND user_id = ?2")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let found = stmt
                .query_map(params![chain_id, user_id], |_row| Ok(()))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
                .next()
                .is_some();
            Ok(found)
        })
        .await?;

    if !chain_exists {
        return Err(EngError::NotFound(format!(
            "causal chain {} not found",
            chain_id
        )));
    }

    // Verify both memories belong to caller
    let count: i64 = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND user_id = ?3 AND is_forgotten = 0",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let c: i64 = stmt
                .query_row(params![cause_memory_id, effect_memory_id, user_id], |row| {
                    row.get(0)
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(c)
        })
        .await?;

    if count != 2 {
        return Err(EngError::NotFound(
            "one or more memories not found or not owned".into(),
        ));
    }

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO causal_links (chain_id, cause_memory_id, effect_memory_id, strength, order_index) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chain_id, cause_memory_id, effect_memory_id, strength, order_index],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(CausalLink {
        id,
        chain_id,
        cause_memory_id,
        effect_memory_id,
        strength,
        order_index,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// Get a causal chain with all its links.
pub async fn get_chain(db: &Database, chain_id: i64, user_id: i64) -> Result<CausalChain> {
    let mut chain = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, root_memory_id, description, confidence, user_id, created_at \
                     FROM causal_chains WHERE id = ?1 AND user_id = ?2",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query_map(params![chain_id, user_id], |row| {
                    Ok(CausalChain {
                        id: row.get(0)?,
                        root_memory_id: row.get(1)?,
                        description: row.get(2)?,
                        confidence: row.get(3)?,
                        user_id: row.get(4)?,
                        created_at: row.get(5)?,
                        links: Vec::new(),
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.next()
                .transpose()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
                .ok_or_else(|| EngError::NotFound(format!("causal chain {} not found", chain_id)))
        })
        .await?;

    // Fetch links
    let links = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, chain_id, cause_memory_id, effect_memory_id, strength, order_index, created_at \
                     FROM causal_links WHERE chain_id = ?1 ORDER BY order_index",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![chain_id], |row| {
                    Ok(CausalLink {
                        id: row.get(0)?,
                        chain_id: row.get(1)?,
                        cause_memory_id: row.get(2)?,
                        effect_memory_id: row.get(3)?,
                        strength: row.get(4)?,
                        order_index: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    chain.links = links;
    Ok(chain)
}

/// List causal chains for a user.
pub async fn list_chains(db: &Database, user_id: i64, limit: usize) -> Result<Vec<CausalChain>> {
    let ids = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM causal_chains WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![user_id, limit as i64], |row| row.get::<_, i64>(0))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let mut chains = Vec::new();
    for id in ids {
        chains.push(get_chain(db, id, user_id).await?);
    }

    Ok(chains)
}
