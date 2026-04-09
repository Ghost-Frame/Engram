use crate::db::Database;
use crate::{EngError, Result};
use libsql::params;
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
    let conn = db.connection();
    conn.execute(
        "INSERT INTO causal_chains (root_memory_id, description, user_id) VALUES (?1, ?2, ?3)",
        params![root_memory_id, description, user_id],
    ).await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = if let Some(row) = rows.next().await? { row.get(0)? } else { 0 };

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
    // Verify the chain belongs to this user
    let conn = db.connection();
    let mut check_rows = conn.query(
        "SELECT id FROM causal_chains WHERE id = ?1 AND user_id = ?2",
        params![chain_id, user_id],
    ).await?;
    if check_rows.next().await?.is_none() {
        return Err(crate::EngError::NotFound(format!("causal chain {} not found", chain_id)));
    }
    conn.execute(
        "INSERT INTO causal_links (chain_id, cause_memory_id, effect_memory_id, strength, order_index) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![chain_id, cause_memory_id, effect_memory_id, strength, order_index],
    ).await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = if let Some(row) = rows.next().await? { row.get(0)? } else { 0 };

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
    let conn = db.connection();

    let mut rows = conn.query(
        "SELECT id, root_memory_id, description, confidence, user_id, created_at \
         FROM causal_chains WHERE id = ?1 AND user_id = ?2",
        params![chain_id, user_id],
    ).await?;

    let row = rows.next().await?
        .ok_or_else(|| EngError::NotFound(format!("causal chain {} not found", chain_id)))?;

    let mut chain = CausalChain {
        id: row.get(0)?,
        root_memory_id: row.get(1)?,
        description: row.get(2)?,
        confidence: row.get(3)?,
        user_id: row.get(4)?,
        created_at: row.get(5)?,
        links: Vec::new(),
    };

    // Fetch links
    let mut link_rows = conn.query(
        "SELECT id, chain_id, cause_memory_id, effect_memory_id, strength, order_index, created_at \
         FROM causal_links WHERE chain_id = ?1 ORDER BY order_index",
        params![chain_id],
    ).await?;

    while let Some(lr) = link_rows.next().await? {
        chain.links.push(CausalLink {
            id: lr.get(0)?,
            chain_id: lr.get(1)?,
            cause_memory_id: lr.get(2)?,
            effect_memory_id: lr.get(3)?,
            strength: lr.get(4)?,
            order_index: lr.get(5)?,
            created_at: lr.get(6)?,
        });
    }

    Ok(chain)
}

/// List causal chains for a user.
pub async fn list_chains(db: &Database, user_id: i64, limit: usize) -> Result<Vec<CausalChain>> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT id FROM causal_chains WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
        params![user_id, limit as i64],
    ).await?;

    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<i64>(0)?);
    }

    let mut chains = Vec::new();
    for id in ids {
        chains.push(get_chain(db, id, user_id).await?);
    }

    Ok(chains)
}
