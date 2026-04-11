// ============================================================================
// CONTEXT DOMAIN -- Database query helpers for context assembly
// ============================================================================

use crate::db::Database;
use crate::memory::types::Memory;
use crate::memory::{row_to_memory, MEMORY_COLUMNS};
use crate::Result;

#[derive(Debug, Clone)]
pub struct VersionChainEntry {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub version: i32,
    pub is_latest: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct LinkedMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub similarity: f64,
    pub is_forgotten: bool,
    pub model: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StateEntry {
    pub key: String,
    pub value: String,
    pub updated_count: i32,
}

#[derive(Debug, Clone)]
pub struct PreferenceEntry {
    pub domain: String,
    pub preference: String,
    pub strength: f64,
}

#[derive(Debug, Clone)]
pub struct StructuredFact {
    pub subject: String,
    pub verb: String,
    pub object: Option<String>,
    pub quantity: Option<f64>,
    pub unit: Option<String>,
    pub date_ref: Option<String>,
    pub date_approx: Option<String>,
    pub valid_at: Option<String>,
    pub invalid_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EpisodeSummary {
    pub id: i64,
    pub summary: Option<String>,
    pub started_at: Option<String>,
}

pub async fn get_static_memories(db: &Database, user_id: i64) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories WHERE user_id = ?1 AND is_static = 1 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 ORDER BY importance DESC",
        MEMORY_COLUMNS,
    );
    let mut rows = db.conn.query(&sql, libsql::params![user_id]).await?;
    let mut memories = Vec::new();
    while let Some(row) = rows.next().await? {
        memories.push(row_to_memory(&row)?);
    }
    Ok(memories)
}

pub async fn get_memory_without_embedding(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<Option<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2",
        MEMORY_COLUMNS
    );
    let mut rows = db.conn.query(&sql, libsql::params![id, user_id]).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_to_memory(&row)?)),
        None => Ok(None),
    }
}

pub async fn get_version_chain(
    db: &Database,
    root_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let sql = "SELECT id, content, category, version, is_latest, created_at                FROM memories                WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2                ORDER BY version ASC";
    let mut rows = db
        .conn
        .query(sql, libsql::params![root_id, user_id])
        .await?;
    let mut chain = Vec::new();
    while let Some(row) = rows.next().await? {
        chain.push(VersionChainEntry {
            id: row.get::<i64>(0)?,
            content: row.get::<String>(1)?,
            category: row.get::<String>(2)?,
            version: row.get::<i32>(3)?,
            is_latest: row.get::<i32>(4)? != 0,
            created_at: row.get::<String>(5)?,
        });
    }
    Ok(chain)
}

pub async fn get_episode_summary(
    db: &Database,
    ep_id: i64,
    user_id: i64,
) -> Result<Option<EpisodeSummary>> {
    let sql = "SELECT id, summary, started_at FROM episodes WHERE id = ?1 AND user_id = ?2";
    let mut rows = db.conn.query(sql, libsql::params![ep_id, user_id]).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(EpisodeSummary {
            id: row.get::<i64>(0)?,
            summary: row.get::<Option<String>>(1)?,
            started_at: row.get::<Option<String>>(2)?,
        })),
        None => Ok(None),
    }
}

pub async fn get_links(db: &Database, mem_id: i64, user_id: i64) -> Result<Vec<LinkedMemory>> {
    let sql = "SELECT m.id, m.content, m.category, ml.weight, m.is_forgotten, m.model, m.source          FROM memory_links ml          JOIN memories m ON (m.id = CASE WHEN ml.source_id = ?1 THEN ml.target_id ELSE ml.source_id END)          WHERE (ml.source_id = ?1 OR ml.target_id = ?1)            AND m.user_id = ?2 AND m.is_latest = 1 AND m.is_consolidated = 0          ORDER BY ml.weight DESC LIMIT 10";
    let mut rows = db.conn.query(sql, libsql::params![mem_id, user_id]).await?;
    let mut linked = Vec::new();
    while let Some(row) = rows.next().await? {
        linked.push(LinkedMemory {
            id: row.get::<i64>(0)?,
            content: row.get::<String>(1)?,
            category: row.get::<String>(2)?,
            similarity: row.get::<f64>(3)?,
            is_forgotten: row.get::<i32>(4)? != 0,
            model: row.get::<Option<String>>(5)?,
            source: row.get::<Option<String>>(6)?,
        });
    }
    Ok(linked)
}

pub async fn get_recent_dynamic(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories          WHERE user_id = ?1 AND is_static = 0 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0          ORDER BY created_at DESC LIMIT ?2",
        MEMORY_COLUMNS,
    );
    let mut rows = db
        .conn
        .query(&sql, libsql::params![user_id, limit as i64])
        .await?;
    let mut memories = Vec::new();
    while let Some(row) = rows.next().await? {
        memories.push(row_to_memory(&row)?);
    }
    Ok(memories)
}

pub async fn get_current_state(db: &Database, user_id: i64) -> Result<Vec<StateEntry>> {
    let sql = "SELECT key, value, updated_count FROM current_state                WHERE user_id = ?1 ORDER BY updated_at DESC LIMIT 30";
    let mut rows = db.conn.query(sql, libsql::params![user_id]).await?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        entries.push(StateEntry {
            key: row.get::<String>(0)?,
            value: row.get::<String>(1)?,
            updated_count: row.get::<i32>(2)?,
        });
    }
    Ok(entries)
}

pub async fn get_user_preferences(db: &Database, user_id: i64) -> Result<Vec<PreferenceEntry>> {
    let sql = "SELECT domain, preference, strength FROM user_preferences                WHERE user_id = ?1 AND strength >= 1.5 ORDER BY strength DESC LIMIT 15";
    let mut rows = db.conn.query(sql, libsql::params![user_id]).await?;
    let mut prefs = Vec::new();
    while let Some(row) = rows.next().await? {
        prefs.push(PreferenceEntry {
            domain: row.get::<String>(0)?,
            preference: row.get::<String>(1)?,
            strength: row.get::<f64>(2)?,
        });
    }
    Ok(prefs)
}

pub async fn get_structured_facts(
    db: &Database,
    mem_ids: &[i64],
    user_id: i64,
) -> Result<Vec<StructuredFact>> {
    if mem_ids.is_empty() {
        return Ok(vec![]);
    }
    // SECURITY: user_id is always scoped; memory IDs are i64 so format! is safe.
    let placeholders: Vec<String> = mem_ids.iter().map(|id| id.to_string()).collect();
    let sql = format!(
        "SELECT subject, verb, object, quantity, unit, date_ref, date_approx, valid_at, invalid_at          FROM structured_facts WHERE user_id = ?1 AND memory_id IN ({}) AND invalid_at IS NULL          ORDER BY valid_at DESC NULLS LAST, date_approx DESC NULLS LAST",
        placeholders.join(",")
    );
    let mut rows = db.conn.query(&sql, libsql::params![user_id]).await?;
    let mut facts = Vec::new();
    while let Some(row) = rows.next().await? {
        facts.push(StructuredFact {
            subject: row.get::<String>(0)?,
            verb: row.get::<String>(1)?,
            object: row.get::<Option<String>>(2)?,
            quantity: row.get::<Option<f64>>(3)?,
            unit: row.get::<Option<String>>(4)?,
            date_ref: row.get::<Option<String>>(5)?,
            date_approx: row.get::<Option<String>>(6)?,
            valid_at: row.get::<Option<String>>(7)?,
            invalid_at: row.get::<Option<String>>(8)?,
        });
    }
    Ok(facts)
}

pub async fn track_access(db: &Database, ids: &[i64], user_id: i64) {
    for &id in ids {
        if let Err(e) = db
            .conn
            .execute(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2",
                libsql::params![id, user_id],
            )
            .await
        {
            tracing::warn!("Failed to track access for memory {}: {}", id, e);
        }
    }
}
