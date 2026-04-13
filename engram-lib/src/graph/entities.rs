use super::cooccurrence::record_cooccurrence;
use super::types::{
    CreateEntityRequest, CreateRelationshipRequest, Entity, EntityMemorySearchResult,
    EntityRelationship,
};
use crate::db::Database;
use crate::memory::fts::sanitize_fts_query;
use crate::{EngError, Result};

const ENTITY_COLUMNS: &str = "id, name, entity_type, description, aliases, user_id, space_id, confidence, occurrence_count, first_seen_at, last_seen_at, created_at";

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

fn row_to_entity(row: &rusqlite::Row<'_>) -> Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        name: row.get(1)?,
        entity_type: row.get(2)?,
        description: row.get(3)?,
        aliases: row.get(4)?,
        user_id: row.get(5)?,
        space_id: row.get(6)?,
        confidence: row.get(7)?,
        occurrence_count: row.get(8)?,
        first_seen_at: row.get(9)?,
        last_seen_at: row.get(10)?,
        created_at: row.get(11)?,
    })
}

// -- Entity CRUD --

/// Upsert an entity by (name, entity_type, user_id). On conflict, increments
/// occurrence_count and updates last_seen_at, then returns the stored entity.
pub async fn create_entity(db: &Database, req: CreateEntityRequest) -> Result<Entity> {
    let entity_type = req.entity_type.unwrap_or_else(|| "general".to_string());
    // SECURITY: fail closed when user_id is missing so an unauthenticated path
    // cannot silently create tenant-0 rows that other callers may trust.
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id is required to create an entity".into()))?;
    let aliases_json = match req.aliases {
        Some(ref v) => Some(serde_json::to_string(v)?),
        None => None,
    };

    let name_clone = req.name.clone();
    let entity_type_clone = entity_type.clone();
    let description = req.description.clone();
    let space_id = req.space_id;

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO entities \
             (name, entity_type, description, aliases, user_id, space_id, confidence, occurrence_count, \
              first_seen_at, last_seen_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1.0, 1, datetime('now'), datetime('now'), datetime('now')) \
             ON CONFLICT(name, entity_type, user_id) DO UPDATE SET \
               occurrence_count = occurrence_count + 1, \
               last_seen_at = datetime('now')",
            rusqlite::params![
                name_clone,
                entity_type_clone,
                description,
                aliases_json,
                user_id,
                space_id,
            ],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    // Fetch the row that was just upserted
    let entity = find_entity_by_name_type(db, &req.name, &entity_type, user_id)
        .await?
        .ok_or_else(|| {
            EngError::Internal("entity upsert succeeded but fetch returned nothing".to_string())
        })?;

    Ok(entity)
}

/// Internal helper: look up an entity by (name, entity_type, user_id).
async fn find_entity_by_name_type(
    db: &Database,
    name: &str,
    entity_type: &str,
    user_id: i64,
) -> Result<Option<Entity>> {
    let name = name.to_string();
    let entity_type = entity_type.to_string();
    let query = format!(
        "SELECT {} FROM entities WHERE name = ?1 AND entity_type = ?2 AND user_id = ?3 LIMIT 1",
        ENTITY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![name, entity_type, user_id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(row_to_entity(row)?)),
            None => Ok(None),
        }
    })
    .await
}

pub async fn get_entity(db: &Database, id: i64, user_id: i64) -> Result<Entity> {
    let query = format!(
        "SELECT {} FROM entities WHERE id = ?1 AND user_id = ?2 LIMIT 1",
        ENTITY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => row_to_entity(row),
            None => Err(EngError::NotFound(format!("entity {}", id))),
        }
    })
    .await
}

/// List entities for a user, ordered by occurrence_count descending.
pub async fn list_entities(
    db: &Database,
    user_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<Entity>> {
    let query = format!(
        "SELECT {} FROM entities WHERE user_id = ?1 \
         ORDER BY occurrence_count DESC \
         LIMIT ?2 OFFSET ?3",
        ENTITY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id, limit as i64, offset as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut entities = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            entities.push(row_to_entity(row)?);
        }
        Ok(entities)
    })
    .await
}

/// Find an entity by name (case-sensitive) for a given user.
pub async fn find_entity_by_name(
    db: &Database,
    name: &str,
    user_id: i64,
) -> Result<Option<Entity>> {
    let name = name.to_string();
    let query = format!(
        "SELECT {} FROM entities WHERE name = ?1 AND user_id = ?2 LIMIT 1",
        ENTITY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![name, user_id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(row_to_entity(row)?)),
            None => Ok(None),
        }
    })
    .await
}

pub async fn delete_entity(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        let affected = conn
            .execute(
                "DELETE FROM entities WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        if affected == 0 {
            return Err(EngError::NotFound(format!("entity {}", id)));
        }
        Ok(())
    })
    .await
}

pub async fn update_entity(
    db: &Database,
    id: i64,
    user_id: i64,
    name: Option<&str>,
    entity_type: Option<&str>,
    description: Option<&str>,
    metadata: Option<&str>,
) -> Result<Entity> {
    let mut sets = Vec::new();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1;

    if let Some(value) = name {
        sets.push(format!("name = ?{}", idx));
        params.push(value.to_string().into());
        idx += 1;
    }
    if let Some(value) = entity_type {
        sets.push(format!("entity_type = ?{}", idx));
        params.push(value.to_string().into());
        idx += 1;
    }
    if let Some(value) = description {
        sets.push(format!("description = ?{}", idx));
        params.push(value.to_string().into());
        idx += 1;
    }
    if let Some(value) = metadata {
        sets.push(format!("metadata = ?{}", idx));
        params.push(value.to_string().into());
        idx += 1;
    }

    if sets.is_empty() {
        return get_entity(db, id, user_id).await;
    }

    let sql = format!(
        "UPDATE entities SET {}, updated_at = datetime('now') WHERE id = ?{} AND user_id = ?{}",
        sets.join(", "),
        idx,
        idx + 1
    );
    params.push(id.into());
    params.push(user_id.into());

    db.write(move |conn| {
        let affected = conn
            .execute(&sql, rusqlite::params_from_iter(params.iter()))
            .map_err(rusqlite_to_eng_error)?;
        if affected == 0 {
            return Err(EngError::NotFound(format!("entity {}", id)));
        }
        Ok(())
    })
    .await?;

    get_entity(db, id, user_id).await
}

// -- Entity Relationships --

/// Upsert a relationship between two entities. On conflict, increments
/// evidence_count and keeps the higher strength value. Both source and target
/// must belong to `user_id`, otherwise returns NotFound.
pub async fn create_relationship(
    db: &Database,
    user_id: i64,
    req: CreateRelationshipRequest,
) -> Result<EntityRelationship> {
    // SECURITY: verify both endpoints belong to the caller before writing so a
    // malicious tenant cannot attach a target they don't own. Using a single
    // COUNT-of-two check prevents a TOCTOU gap between two separate lookups.
    let source_id = req.source_entity_id;
    let target_id = req.target_entity_id;

    let owned: i64 = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT COUNT(*) FROM entities WHERE id IN (?1, ?2) AND user_id = ?3")
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![source_id, target_id, user_id])
                .map_err(rusqlite_to_eng_error)?;
            match rows.next().map_err(rusqlite_to_eng_error)? {
                Some(row) => Ok(row.get(0)?),
                None => Ok(0i64),
            }
        })
        .await?;

    // When source == target the same row is counted once, so accept >= 1 in
    // that case and >= 2 otherwise.
    let required = if req.source_entity_id == req.target_entity_id {
        1
    } else {
        2
    };
    if owned < required {
        return Err(EngError::NotFound(
            "one or both entities not found for this user".into(),
        ));
    }

    let relationship_type = req
        .relationship_type
        .unwrap_or_else(|| "related".to_string());
    let strength = req.strength.unwrap_or(0.5);

    // INSERT ... ON CONFLICT ... RETURNING avoids the cross-connection
    // last_insert_rowid() race inherent to a follow-up SELECT.
    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO entity_relationships \
             (source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at) \
             VALUES (?1, ?2, ?3, ?4, 1, datetime('now')) \
             ON CONFLICT(source_entity_id, target_entity_id, relationship_type) DO UPDATE SET \
               evidence_count = evidence_count + 1, \
               strength = max(strength, excluded.strength) \
             RETURNING id, source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at",
            rusqlite::params![
                req.source_entity_id,
                req.target_entity_id,
                relationship_type,
                strength,
            ],
            |row| {
                Ok(EntityRelationship {
                    id: row.get(0)?,
                    source_entity_id: row.get(1)?,
                    target_entity_id: row.get(2)?,
                    relationship_type: row.get(3)?,
                    strength: row.get(4)?,
                    evidence_count: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Maximum number of relationships returned per entity. Hot entities
/// (for example a person mentioned in many memories) can accumulate
/// thousands of edges; without a cap the response would blow up memory
/// and client parsing.
pub const MAX_ENTITY_RELATIONSHIPS: i64 = 1000;

/// Return relationships where the entity appears as source or target,
/// ordered by strength, capped at [`MAX_ENTITY_RELATIONSHIPS`].
pub async fn get_entity_relationships(
    db: &Database,
    entity_id: i64,
    user_id: i64,
) -> Result<Vec<EntityRelationship>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at \
                 FROM entity_relationships \
                 WHERE (source_entity_id = ?1 OR target_entity_id = ?1) \
                   AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?2) \
                 ORDER BY strength DESC \
                 LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![entity_id, user_id, MAX_ENTITY_RELATIONSHIPS])
            .map_err(rusqlite_to_eng_error)?;
        let mut rels = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            rels.push(EntityRelationship {
                id: row.get(0)?,
                source_entity_id: row.get(1)?,
                target_entity_id: row.get(2)?,
                relationship_type: row.get(3)?,
                strength: row.get(4)?,
                evidence_count: row.get(5)?,
                created_at: row.get(6)?,
            });
        }
        Ok(rels)
    })
    .await
}

// -- Memory-Entity linking --

/// Link a memory to an entity with a salience score. Silently ignores duplicates.
pub async fn link_memory_entity(
    db: &Database,
    memory_id: i64,
    entity_id: i64,
    user_id: i64,
    salience: f64,
) -> Result<()> {
    let count: i64 = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT COUNT(*) \
                     FROM entities e \
                     JOIN memories m ON m.id = ?1 \
                     WHERE e.id = ?2 AND e.user_id = ?3 AND m.user_id = ?3",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![memory_id, entity_id, user_id])
                .map_err(rusqlite_to_eng_error)?;
            match rows.next().map_err(rusqlite_to_eng_error)? {
                Some(row) => Ok(row.get(0)?),
                None => Ok(0i64),
            }
        })
        .await?;

    if count == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} or entity {} not found",
            memory_id, entity_id
        )));
    }

    db.write(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, salience, created_at) \
             VALUES (?1, ?2, ?3, datetime('now'))",
            rusqlite::params![memory_id, entity_id, salience],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn unlink_memory_entity(
    db: &Database,
    memory_id: i64,
    entity_id: i64,
    user_id: i64,
) -> Result<()> {
    db.write(move |conn| {
        let affected = conn
            .execute(
                "DELETE FROM memory_entities \
                 WHERE memory_id = ?1 AND entity_id = ?2 \
                   AND EXISTS (SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?3) \
                   AND EXISTS (SELECT 1 FROM entities WHERE id = ?2 AND user_id = ?3)",
                rusqlite::params![memory_id, entity_id, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        if affected == 0 {
            return Err(EngError::NotFound(format!(
                "entity {} not linked to memory {}",
                entity_id, memory_id
            )));
        }
        Ok(())
    })
    .await
}

/// Return all entities linked to the given memory.
pub async fn get_memory_entities(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<Entity>> {
    let query = format!(
        "SELECT e.{cols} \
         FROM entities e \
         JOIN memory_entities me ON me.entity_id = e.id \
         JOIN memories m ON m.id = me.memory_id \
         WHERE me.memory_id = ?1 AND m.user_id = ?2 \
         ORDER BY me.salience DESC",
        cols = ENTITY_COLUMNS
            .split(", ")
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", e.")
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![memory_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut entities = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            entities.push(row_to_entity(row)?);
        }
        Ok(entities)
    })
    .await
}

/// Return the IDs of all memories linked to the given entity.
pub async fn get_entity_memories(db: &Database, entity_id: i64, user_id: i64) -> Result<Vec<i64>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT me.memory_id \
                 FROM memory_entities me \
                 JOIN memories m ON m.id = me.memory_id \
                 JOIN entities e ON e.id = me.entity_id \
                 WHERE me.entity_id = ?1 AND e.user_id = ?2 AND m.user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![entity_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut memory_ids = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let id: i64 = row.get(0)?;
            memory_ids.push(id);
        }
        Ok(memory_ids)
    })
    .await
}

pub async fn search_entity_memories(
    db: &Database,
    entity_id: i64,
    user_id: i64,
    query: &str,
    limit: i64,
) -> Result<Vec<EntityMemorySearchResult>> {
    // Sanitize query to prevent FTS5 syntax injection
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(vec![]);
    }

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.content, m.category, m.source, m.importance, m.created_at \
                 FROM memories m \
                 JOIN memory_entities me ON me.memory_id = m.id \
                 WHERE me.entity_id = ?1 AND m.user_id = ?2 AND m.is_forgotten = 0 \
                   AND m.is_archived = 0 AND m.is_latest = 1 \
                   AND m.id IN (SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?3) \
                 ORDER BY m.importance DESC, m.created_at DESC \
                 LIMIT ?4",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![entity_id, user_id, sanitized, limit])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(EntityMemorySearchResult {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                importance: row.get(4)?,
                created_at: row.get(5)?,
            });
        }
        Ok(results)
    })
    .await
}

pub async fn delete_relationship(
    db: &Database,
    entity_id: i64,
    target_entity_id: i64,
    user_id: i64,
    relationship_type: Option<&str>,
) -> Result<()> {
    let mut params: Vec<rusqlite::types::Value> =
        vec![entity_id.into(), target_entity_id.into(), user_id.into()];
    let sql = if let Some(value) = relationship_type {
        params.push(value.to_string().into());
        "DELETE FROM entity_relationships \
         WHERE source_entity_id = ?1 AND target_entity_id = ?2 \
           AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?3) \
           AND EXISTS (SELECT 1 FROM entities WHERE id = ?2 AND user_id = ?3) \
           AND relationship_type = ?4"
            .to_string()
    } else {
        "DELETE FROM entity_relationships \
         WHERE source_entity_id = ?1 AND target_entity_id = ?2 \
           AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?3) \
           AND EXISTS (SELECT 1 FROM entities WHERE id = ?2 AND user_id = ?3)"
            .to_string()
    };

    db.write(move |conn| {
        let affected = conn
            .execute(&sql, rusqlite::params_from_iter(params.iter()))
            .map_err(rusqlite_to_eng_error)?;
        if affected == 0 {
            return Err(EngError::NotFound(format!(
                "relationship {} -> {} not found",
                entity_id, target_entity_id
            )));
        }
        Ok(())
    })
    .await
}

// -- Entity Extraction (simple heuristic) --

/// Extract entities from free text using simple pattern rules.
///
/// Returns a deduplicated vec of (name, entity_type) pairs. Rules applied:
/// 1. Runs of 2+ consecutive capitalized words -> "person_or_place"
/// 2. Text inside double quotes -> "reference"
/// 3. Text inside backticks -> "code"
/// 4. All-uppercase words of 2+ chars (not a sentence start artifact) -> "acronym"
///
/// Deduplication is case-insensitive on the name.
pub fn extract_entities(content: &str) -> Vec<(String, String)> {
    let mut results: Vec<(String, String)> = Vec::new();
    // Track seen names (lowercased) for deduplication
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut add = |name: String, entity_type: &str| {
        let key = name.to_lowercase();
        if !name.is_empty() && seen.insert(key) {
            results.push((name, entity_type.to_string()));
        }
    };

    // -- Rule 2: quoted strings (double quotes) --
    // Do this before proper noun scan to avoid matching quoted text as proper nouns.
    {
        let mut rest = content;
        while let Some(start) = rest.find('"') {
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('"') {
                let s = rest[..end].trim().to_string();
                if !s.is_empty() {
                    add(s, "reference");
                }
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
    }

    // -- Rule 3: backtick-enclosed identifiers --
    {
        let mut rest = content;
        while let Some(start) = rest.find('`') {
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('`') {
                let s = rest[..end].trim().to_string();
                if !s.is_empty() {
                    add(s, "code");
                }
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
    }

    // -- Rules 1 & 4: scan whitespace-split tokens for proper nouns and acronyms --
    // A token is a word candidate; strip leading/trailing punctuation for classification.
    let tokens: Vec<&str> = content.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let raw = tokens[i];
        let word = strip_punctuation(raw);

        if word.len() >= 2 && is_all_caps(word) {
            // Rule 4: acronym
            add(word.to_string(), "acronym");
            i += 1;
            continue;
        }

        if is_capitalized(word) {
            // Rule 1: start of a capitalized run -- collect consecutive capitalized words
            let mut run: Vec<&str> = vec![word];
            let mut j = i + 1;
            while j < tokens.len() {
                let next_raw = tokens[j];
                let next_word = strip_punctuation(next_raw);
                if is_capitalized(next_word) && !is_all_caps(next_word) {
                    run.push(next_word);
                    j += 1;
                } else {
                    break;
                }
            }
            if run.len() >= 2 {
                let name = run.join(" ");
                add(name, "person_or_place");
                i = j;
                continue;
            }
        }

        i += 1;
    }

    results
}

/// Return true if the word starts with an uppercase letter (first char uppercase).
fn is_capitalized(word: &str) -> bool {
    word.chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
}

/// Return true if every alphabetic character in the word is uppercase and the
/// word contains at least one alphabetic character.
fn is_all_caps(word: &str) -> bool {
    let has_alpha = word.chars().any(|c| c.is_alphabetic());
    has_alpha
        && word
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_uppercase())
}

/// Strip common leading/trailing punctuation from a word slice without allocating.
fn strip_punctuation(s: &str) -> &str {
    let punct = |c: char| {
        matches!(
            c,
            '.' | ','
                | '!'
                | '?'
                | ';'
                | ':'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '\''
                | '"'
                | '`'
        )
    };
    s.trim_matches(punct)
}

// -- Combined extract + link --

/// Extract entities from content, upsert each into the DB, link them to the
/// given memory, and record pairwise co-occurrences. Returns the full entity
/// list found in the content.
pub async fn extract_and_link_entities(
    db: &Database,
    memory_id: i64,
    content: &str,
    user_id: i64,
) -> Result<Vec<Entity>> {
    let candidates = extract_entities(content);
    let mut entities: Vec<Entity> = Vec::with_capacity(candidates.len());

    for (name, entity_type) in &candidates {
        let req = CreateEntityRequest {
            name: name.clone(),
            entity_type: Some(entity_type.clone()),
            description: None,
            aliases: None,
            user_id: Some(user_id),
            space_id: None,
        };
        let entity = create_entity(db, req).await?;
        // Salience defaults to 1.0 for heuristic extraction
        link_memory_entity(db, memory_id, entity.id, user_id, 1.0).await?;
        entities.push(entity);
    }

    // Record pairwise co-occurrences for all entity pairs found in this memory
    for a in 0..entities.len() {
        for b in (a + 1)..entities.len() {
            let _ = record_cooccurrence(db, entities[a].id, entities[b].id, user_id).await;
        }
    }

    Ok(entities)
}
