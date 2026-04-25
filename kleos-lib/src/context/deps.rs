// ============================================================================
// CONTEXT DOMAIN -- Database query helpers for context assembly
// ============================================================================

use crate::db::Database;
use crate::memory::types::Memory;
use crate::memory::{row_to_memory, MEMORY_COLUMNS};
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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

#[tracing::instrument(skip(db))]
pub async fn get_static_memories(db: &Database, user_id: i64) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories WHERE is_static = 1 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 ORDER BY importance DESC",
        MEMORY_COLUMNS,
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![])
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: static-memory sets are typically small per-user.
        let mut memories = Vec::with_capacity(16);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_memory_without_embedding(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<Option<Memory>> {
    let sql = format!("SELECT {} FROM memories WHERE id = ?1", MEMORY_COLUMNS);
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(row_to_memory(row, user_id)?)),
            None => Ok(None),
        }
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_version_chain(
    db: &Database,
    root_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let sql = "SELECT id, content, category, version, is_latest, created_at \
               FROM memories \
               WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2 \
               ORDER BY version ASC";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![root_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut chain = Vec::with_capacity(8);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            chain.push(VersionChainEntry {
                id: row.get::<_, i64>(0).map_err(rusqlite_to_eng_error)?,
                content: row.get::<_, String>(1).map_err(rusqlite_to_eng_error)?,
                category: row.get::<_, String>(2).map_err(rusqlite_to_eng_error)?,
                version: row.get::<_, i32>(3).map_err(rusqlite_to_eng_error)?,
                is_latest: row.get::<_, i32>(4).map_err(rusqlite_to_eng_error)? != 0,
                created_at: row.get::<_, String>(5).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(chain)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_episode_summary(
    db: &Database,
    ep_id: i64,
    user_id: i64,
) -> Result<Option<EpisodeSummary>> {
    let sql = "SELECT id, summary, started_at FROM episodes WHERE id = ?1 AND user_id = ?2";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![ep_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(EpisodeSummary {
                id: row.get::<_, i64>(0).map_err(rusqlite_to_eng_error)?,
                summary: row
                    .get::<_, Option<String>>(1)
                    .map_err(rusqlite_to_eng_error)?,
                started_at: row
                    .get::<_, Option<String>>(2)
                    .map_err(rusqlite_to_eng_error)?,
            })),
            None => Ok(None),
        }
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_links(db: &Database, mem_id: i64, user_id: i64) -> Result<Vec<LinkedMemory>> {
    let sql = "SELECT m.id, m.content, m.category, ml.similarity, m.is_forgotten, m.model, m.source \
               FROM memory_links ml \
               JOIN memories m ON (m.id = CASE WHEN ml.source_id = ?1 THEN ml.target_id ELSE ml.source_id END) \
               WHERE (ml.source_id = ?1 OR ml.target_id = ?1) \
                 AND m.user_id = ?2 AND m.is_latest = 1 AND m.is_consolidated = 0 \
               ORDER BY ml.similarity DESC LIMIT 10";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![mem_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut linked = Vec::with_capacity(10);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            linked.push(LinkedMemory {
                id: row.get::<_, i64>(0).map_err(rusqlite_to_eng_error)?,
                content: row.get::<_, String>(1).map_err(rusqlite_to_eng_error)?,
                category: row.get::<_, String>(2).map_err(rusqlite_to_eng_error)?,
                similarity: row.get::<_, f64>(3).map_err(rusqlite_to_eng_error)?,
                is_forgotten: row.get::<_, i32>(4).map_err(rusqlite_to_eng_error)? != 0,
                model: row
                    .get::<_, Option<String>>(5)
                    .map_err(rusqlite_to_eng_error)?,
                source: row
                    .get::<_, Option<String>>(6)
                    .map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(linked)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_recent_dynamic(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories \
         WHERE is_static = 0 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 \
         ORDER BY created_at DESC LIMIT ?1",
        MEMORY_COLUMNS,
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![limit as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut memories = Vec::with_capacity(limit);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_current_state(db: &Database, _user_id: i64) -> Result<Vec<StateEntry>> {
    let sql = "SELECT key, value, updated_count FROM current_state \
               ORDER BY updated_at DESC LIMIT 30";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![])
            .map_err(rusqlite_to_eng_error)?;
        let mut entries = Vec::with_capacity(30);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            entries.push(StateEntry {
                key: row.get::<_, String>(0).map_err(rusqlite_to_eng_error)?,
                value: row.get::<_, String>(1).map_err(rusqlite_to_eng_error)?,
                updated_count: row.get::<_, i32>(2).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(entries)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_user_preferences(db: &Database, user_id: i64) -> Result<Vec<PreferenceEntry>> {
    let sql = "SELECT domain, preference, strength FROM user_preferences \
               WHERE strength >= 1.5 ORDER BY strength DESC LIMIT 15";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![])
            .map_err(rusqlite_to_eng_error)?;
        let mut prefs = Vec::with_capacity(15);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            prefs.push(PreferenceEntry {
                domain: row.get::<_, String>(0).map_err(rusqlite_to_eng_error)?,
                preference: row.get::<_, String>(1).map_err(rusqlite_to_eng_error)?,
                strength: row.get::<_, f64>(2).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(prefs)
    })
    .await
}

#[tracing::instrument(skip(db, mem_ids), fields(mem_id_count = mem_ids.len()))]
pub async fn get_structured_facts(
    db: &Database,
    mem_ids: &[i64],
    user_id: i64,
) -> Result<Vec<StructuredFact>> {
    if mem_ids.is_empty() {
        return Ok(vec![]);
    }
    // SECURITY: user_id is always scoped; memory IDs are i64 so format! is safe.
    let mem_ids_len = mem_ids.len();
    let mut placeholders: Vec<String> = Vec::with_capacity(mem_ids_len);
    placeholders.extend(mem_ids.iter().map(|id| id.to_string()));
    let sql = format!(
        "SELECT subject, verb, object, quantity, unit, date_ref, date_approx, valid_at, invalid_at \
         FROM structured_facts WHERE memory_id IN ({}) AND invalid_at IS NULL \
         ORDER BY valid_at DESC NULLS LAST, date_approx DESC NULLS LAST",
        placeholders.join(",")
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query([])
            .map_err(rusqlite_to_eng_error)?;
        let mut facts = Vec::with_capacity(mem_ids_len);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            facts.push(StructuredFact {
                subject: row.get::<_, String>(0).map_err(rusqlite_to_eng_error)?,
                verb: row.get::<_, String>(1).map_err(rusqlite_to_eng_error)?,
                object: row
                    .get::<_, Option<String>>(2)
                    .map_err(rusqlite_to_eng_error)?,
                quantity: row
                    .get::<_, Option<f64>>(3)
                    .map_err(rusqlite_to_eng_error)?,
                unit: row
                    .get::<_, Option<String>>(4)
                    .map_err(rusqlite_to_eng_error)?,
                date_ref: row
                    .get::<_, Option<String>>(5)
                    .map_err(rusqlite_to_eng_error)?,
                date_approx: row
                    .get::<_, Option<String>>(6)
                    .map_err(rusqlite_to_eng_error)?,
                valid_at: row
                    .get::<_, Option<String>>(7)
                    .map_err(rusqlite_to_eng_error)?,
                invalid_at: row
                    .get::<_, Option<String>>(8)
                    .map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(facts)
    })
    .await
}

#[tracing::instrument(skip(db, ids), fields(id_count = ids.len()))]
pub async fn track_access(db: &Database, ids: &[i64], _user_id: i64) {
    for &id in ids {
        if let Err(e) = db
            .write(move |conn| {
                conn.execute(
                    "UPDATE memories SET access_count = access_count + 1, last_accessed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                    rusqlite::params![id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await
        {
            tracing::warn!("Failed to track access for memory {}: {}", id, e);
        }
    }
}
