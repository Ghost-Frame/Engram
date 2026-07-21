// ============================================================================
// CONTEXT DOMAIN -- Database query helpers for context assembly
// ============================================================================

use crate::db::Database;
use crate::memory::types::Memory;
use crate::memory::{row_to_memory, MEMORY_COLUMNS};
use crate::validation::MAX_STATIC_CONTEXT_MEMORIES;
use crate::Result;

/// One revision in a memory's version chain, surfaced when context assembly
/// wants to show how a fact evolved.
#[derive(Debug, Clone)]
pub struct VersionChainEntry {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub version: i32,
    pub is_latest: bool,
    pub created_at: String,
}

/// A memory reached through a `memory_links` edge, with the link similarity that
/// justified surfacing it alongside the anchor memory.
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

/// A current-state key/value pair (the agent's tracked working state), with how
/// many times it has been updated.
#[derive(Debug, Clone)]
pub struct StateEntry {
    pub key: String,
    pub value: String,
    pub updated_count: i32,
}

/// A learned user preference in one domain, with a strength score used to rank
/// which preferences are worth injecting.
#[derive(Debug, Clone)]
pub struct PreferenceEntry {
    pub domain: String,
    pub preference: String,
    pub strength: f64,
}

/// A subject-verb-object fact extracted from a memory, with optional quantity,
/// unit, and validity window, for injecting precise facts rather than prose.
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

/// A short summary of an episode (a grouped run of related memories), used to
/// give context assembly a temporal anchor.
#[derive(Debug, Clone)]
pub struct EpisodeSummary {
    pub id: i64,
    pub summary: Option<String>,
    pub started_at: Option<String>,
}

/// Load the user's static (pinned) memories, highest-importance first, for the
/// static-facts block of assembled context. Withholds review-gate pending rows.
#[tracing::instrument(skip(db))]
pub async fn get_static_memories(db: &Database, user_id: i64) -> Result<Vec<Memory>> {
    // status != 'pending' is the review-gate predicate. Static facts are ranked
    // by importance and injected verbatim, so without it the gate would withhold
    // a high-importance memory from search and then inject it here anyway --
    // defeating the gate on exactly the memories it exists to hold back.
    // Finding [19]: bound the injection set. Static memories are injected
    // verbatim into every context assembly, so an unbounded set lets one
    // user's static corpus dominate assembly cost. Top-N by importance keeps
    // the highest-value facts; the downstream token budget trims further.
    let sql = format!(
        "SELECT {} FROM memories WHERE user_id = ?1 AND is_static = 1 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 AND status != 'pending' ORDER BY importance DESC LIMIT {}",
        MEMORY_COLUMNS, MAX_STATIC_CONTEXT_MEMORIES,
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![user_id])?;
        // 6.9 capacity hint: static-memory sets are typically small per-user.
        let mut memories = Vec::with_capacity(16);
        while let Some(row) = rows.next()? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories)
    })
    .await
}

/// Hydrate a single owned memory by id (without its embedding blob) for
/// injection. Withholds review-gate pending rows even when the id is supplied.
#[tracing::instrument(skip(db))]
pub async fn get_memory_without_embedding(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<Option<Memory>> {
    // status != 'pending' is the review-gate predicate: this hydrates a memory
    // for injection, so a pending row must not surface here even when a caller
    // hands us its id.
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND status != 'pending'",
        MEMORY_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![id, user_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_memory(row, user_id)?)),
            None => Ok(None),
        }
    })
    .await
}

/// Load the full version chain for an owned memory (all revisions sharing a
/// root), ordered oldest-first, for showing how a fact evolved in context.
#[tracing::instrument(skip(db))]
pub async fn get_version_chain(
    db: &Database,
    root_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    // The owner predicate (?2) keeps single-DB (shared) mode from returning
    // another user's version chain; a no-op in a single-owner shard.
    // status != 'pending' is the review-gate predicate: a chain sibling can be a
    // freshly stored, still-unapproved revision, and its content is injected.
    let sql = "SELECT id, content, category, version, is_latest, created_at \
               FROM memories \
               WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2 \
                 AND status != 'pending' \
               ORDER BY version ASC";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![root_id, user_id])?;
        let mut chain = Vec::with_capacity(8);
        while let Some(row) = rows.next()? {
            chain.push(VersionChainEntry {
                id: row.get::<_, i64>(0)?,
                content: row.get::<_, String>(1)?,
                category: row.get::<_, String>(2)?,
                version: row.get::<_, i32>(3)?,
                is_latest: row.get::<_, i32>(4)? != 0,
                created_at: row.get::<_, String>(5)?,
            });
        }
        Ok(chain)
    })
    .await
}

/// Fetch the summary of one owned episode by id, for anchoring injected
/// memories to the episode they belong to.
#[tracing::instrument(skip(db))]
pub async fn get_episode_summary(
    db: &Database,
    ep_id: i64,
    user_id: i64,
) -> Result<Option<EpisodeSummary>> {
    let sql = "SELECT id, summary, started_at FROM episodes WHERE id = ?1 AND user_id = ?2";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![ep_id, user_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(EpisodeSummary {
                id: row.get::<_, i64>(0)?,
                summary: row.get::<_, Option<String>>(1)?,
                started_at: row.get::<_, Option<String>>(2)?,
            })),
            None => Ok(None),
        }
    })
    .await
}

/// Load the top owned memories linked to `mem_id` by similarity edges, for the
/// associative block of context. Withholds review-gate pending neighbours.
#[tracing::instrument(skip(db))]
pub async fn get_links(db: &Database, mem_id: i64, user_id: i64) -> Result<Vec<LinkedMemory>> {
    // The joined memory is scoped to the owner (?2) so single-DB mode never
    // returns a link into another user's memory; a no-op in a single-owner shard.
    // m.status != 'pending' is the review-gate predicate: the JOIN reaches a
    // memory the caller never selected, so an unapproved neighbour would other-
    // wise ride into the prompt on the coat-tails of an approved one.
    let sql = "SELECT m.id, m.content, m.category, ml.similarity, m.is_forgotten, m.model, m.source \
               FROM memory_links ml \
               JOIN memories m ON (m.id = CASE WHEN ml.source_id = ?1 THEN ml.target_id ELSE ml.source_id END) \
               WHERE (ml.source_id = ?1 OR ml.target_id = ?1) \
                 AND m.user_id = ?2 \
                 AND m.is_latest = 1 AND m.is_consolidated = 0 \
                 AND m.status != 'pending' \
               ORDER BY ml.similarity DESC LIMIT 10";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![mem_id, user_id])?;
        let mut linked = Vec::with_capacity(10);
        while let Some(row) = rows.next()? {
            linked.push(LinkedMemory {
                id: row.get::<_, i64>(0)?,
                content: row.get::<_, String>(1)?,
                category: row.get::<_, String>(2)?,
                similarity: row.get::<_, f64>(3)?,
                is_forgotten: row.get::<_, i32>(4)? != 0,
                model: row.get::<_, Option<String>>(5)?,
                source: row.get::<_, Option<String>>(6)?,
            });
        }
        Ok(linked)
    })
    .await
}

/// Load the user's most recent non-static memories (newest first, capped at
/// `limit`) for the temporal context block. Withholds review-gate pending rows.
#[tracing::instrument(skip(db))]
pub async fn get_recent_dynamic(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Memory>> {
    // status != 'pending' is the review-gate predicate. This is the temporal
    // context block: a memory stored seconds ago is the most likely thing to be
    // awaiting review, so this path is the gate's likeliest leak without it.
    let sql = format!(
        "SELECT {} FROM memories \
         WHERE user_id = ?1 AND is_static = 0 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 \
           AND status != 'pending' \
         ORDER BY created_at DESC LIMIT ?2",
        MEMORY_COLUMNS,
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![user_id, limit as i64])?;
        let mut memories = Vec::with_capacity(limit);
        while let Some(row) = rows.next()? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories)
    })
    .await
}

/// Retrieve the most recent current_state entries for the given user.
/// The WHERE user_id = ?1 predicate enforces single-DB isolation: in shared
/// mode each user sees only their own state entries.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_current_state(db: &Database, user_id: i64) -> Result<Vec<StateEntry>> {
    let sql = "SELECT key, value, updated_count FROM current_state \
               WHERE user_id = ?1 \
               ORDER BY updated_at DESC LIMIT 30";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![user_id])?;
        let mut entries = Vec::with_capacity(30);
        while let Some(row) = rows.next()? {
            entries.push(StateEntry {
                key: row.get::<_, String>(0)?,
                value: row.get::<_, String>(1)?,
                updated_count: row.get::<_, i32>(2)?,
            });
        }
        Ok(entries)
    })
    .await
}

/// Load the user's strongest learned preferences (strength >= 1.5), for the
/// preferences block of assembled context.
#[tracing::instrument(skip(db))]
pub async fn get_user_preferences(db: &Database, user_id: i64) -> Result<Vec<PreferenceEntry>> {
    let sql = "SELECT domain, preference, strength FROM user_preferences \
               WHERE user_id = ?1 AND strength >= 1.5 ORDER BY strength DESC LIMIT 15";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params![user_id])?;
        let mut prefs = Vec::with_capacity(15);
        while let Some(row) = rows.next()? {
            prefs.push(PreferenceEntry {
                domain: row.get::<_, String>(0)?,
                preference: row.get::<_, String>(1)?,
                strength: row.get::<_, f64>(2)?,
            });
        }
        Ok(prefs)
    })
    .await
}

/// Load currently-valid structured facts extracted from the given memories, for
/// injecting precise SVO facts. `mem_ids` must already be owner-scoped by the
/// caller: the query filters by `memory_id`, not `user_id`.
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
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut facts = Vec::with_capacity(mem_ids_len);
        while let Some(row) = rows.next()? {
            facts.push(StructuredFact {
                subject: row.get::<_, String>(0)?,
                verb: row.get::<_, String>(1)?,
                object: row.get::<_, Option<String>>(2)?,
                quantity: row.get::<_, Option<f64>>(3)?,
                unit: row.get::<_, Option<String>>(4)?,
                date_ref: row.get::<_, Option<String>>(5)?,
                date_approx: row.get::<_, Option<String>>(6)?,
                valid_at: row.get::<_, Option<String>>(7)?,
                invalid_at: row.get::<_, Option<String>>(8)?,
            });
        }
        Ok(facts)
    })
    .await
}

/// Increment the access counter and refresh the last-accessed timestamp for each
/// memory that was surfaced into context. Best-effort: failures are logged only.
#[tracing::instrument(skip(db, ids), fields(id_count = ids.len()))]
pub async fn track_access(db: &Database, ids: &[i64]) {
    for &id in ids {
        if let Err(e) = db
            .write(move |conn| {
                conn.execute(
                    "UPDATE memories SET access_count = access_count + 1, last_accessed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                    rusqlite::params![id],
                )
                ?;
                Ok(())
            })
            .await
        {
            tracing::warn!("Failed to track access for memory {}: {}", id, e);
        }
    }
}
