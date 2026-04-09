use super::cooccurrence::record_cooccurrence;
use super::types::{
    CreateEntityRequest, CreateRelationshipRequest, Entity, EntityRelationship,
};
use crate::db::Database;
use crate::{EngError, Result};

const ENTITY_COLUMNS: &str = "id, name, entity_type, description, aliases, user_id, space_id, confidence, occurrence_count, first_seen_at, last_seen_at, created_at";

fn row_to_entity(row: &libsql::Row) -> Result<Entity> {
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
    let conn = db.connection();

    let entity_type = req.entity_type.unwrap_or_else(|| "general".to_string());
    let user_id = req.user_id.unwrap_or(0);
    let aliases_json = match req.aliases {
        Some(ref v) => Some(serde_json::to_string(v)?),
        None => None,
    };

    conn.execute(
        "INSERT INTO entities \
         (name, entity_type, description, aliases, user_id, space_id, confidence, occurrence_count, \
          first_seen_at, last_seen_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1.0, 1, datetime('now'), datetime('now'), datetime('now')) \
         ON CONFLICT(name, entity_type, user_id) DO UPDATE SET \
           occurrence_count = occurrence_count + 1, \
           last_seen_at = datetime('now')",
        libsql::params![
            req.name.clone(),
            entity_type.clone(),
            req.description.clone(),
            aliases_json,
            user_id,
            req.space_id,
        ],
    )
    .await?;

    // Fetch the row that was just upserted
    let entity = find_entity_by_name_type(db, &req.name, &entity_type, user_id)
        .await?
        .ok_or_else(|| EngError::Internal("entity upsert succeeded but fetch returned nothing".to_string()))?;

    Ok(entity)
}

/// Internal helper: look up an entity by (name, entity_type, user_id).
async fn find_entity_by_name_type(
    db: &Database,
    name: &str,
    entity_type: &str,
    user_id: i64,
) -> Result<Option<Entity>> {
    let conn = db.connection();
    let query = format!(
        "SELECT {} FROM entities WHERE name = ?1 AND entity_type = ?2 AND user_id = ?3 LIMIT 1",
        ENTITY_COLUMNS
    );

    let mut rows = conn
        .query(&query, libsql::params![name, entity_type, user_id])
        .await?;

    match rows.next().await? {
        Some(row) => Ok(Some(row_to_entity(&row)?)),
        None => Ok(None),
    }
}

pub async fn get_entity(db: &Database, id: i64) -> Result<Entity> {
    let conn = db.connection();
    let query = format!(
        "SELECT {} FROM entities WHERE id = ?1 LIMIT 1",
        ENTITY_COLUMNS
    );

    let mut rows = conn.query(&query, libsql::params![id]).await?;

    match rows.next().await? {
        Some(row) => row_to_entity(&row),
        None => Err(EngError::NotFound(format!("entity {}", id))),
    }
}

/// List entities for a user, ordered by occurrence_count descending.
pub async fn list_entities(
    db: &Database,
    user_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<Entity>> {
    let conn = db.connection();
    let query = format!(
        "SELECT {} FROM entities WHERE user_id = ?1 \
         ORDER BY occurrence_count DESC \
         LIMIT ?2 OFFSET ?3",
        ENTITY_COLUMNS
    );

    let mut rows = conn
        .query(
            &query,
            libsql::params![user_id, limit as i64, offset as i64],
        )
        .await?;

    let mut entities = Vec::new();
    while let Some(row) = rows.next().await? {
        entities.push(row_to_entity(&row)?);
    }
    Ok(entities)
}

/// Find an entity by name (case-sensitive) for a given user.
pub async fn find_entity_by_name(
    db: &Database,
    name: &str,
    user_id: i64,
) -> Result<Option<Entity>> {
    let conn = db.connection();
    let query = format!(
        "SELECT {} FROM entities WHERE name = ?1 AND user_id = ?2 LIMIT 1",
        ENTITY_COLUMNS
    );

    let mut rows = conn.query(&query, libsql::params![name, user_id]).await?;

    match rows.next().await? {
        Some(row) => Ok(Some(row_to_entity(&row)?)),
        None => Ok(None),
    }
}

pub async fn delete_entity(db: &Database, id: i64) -> Result<()> {
    let conn = db.connection();
    conn.execute("DELETE FROM entities WHERE id = ?1", libsql::params![id])
        .await?;
    Ok(())
}

// -- Entity Relationships --

/// Upsert a relationship between two entities. On conflict, increments
/// evidence_count and keeps the higher strength value.
pub async fn create_relationship(
    db: &Database,
    req: CreateRelationshipRequest,
) -> Result<EntityRelationship> {
    let conn = db.connection();

    let relationship_type = req
        .relationship_type
        .unwrap_or_else(|| "related".to_string());
    let strength = req.strength.unwrap_or(0.5);

    conn.execute(
        "INSERT INTO entity_relationships \
         (source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at) \
         VALUES (?1, ?2, ?3, ?4, 1, datetime('now')) \
         ON CONFLICT(source_entity_id, target_entity_id, relationship_type) DO UPDATE SET \
           evidence_count = evidence_count + 1, \
           strength = max(strength, excluded.strength)",
        libsql::params![
            req.source_entity_id,
            req.target_entity_id,
            relationship_type,
            strength,
        ],
    )
    .await?;

    // Fetch the upserted row
    let mut rows = conn
        .query(
            "SELECT id, source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at \
             FROM entity_relationships \
             WHERE source_entity_id = ?1 AND target_entity_id = ?2 \
             ORDER BY id DESC LIMIT 1",
            libsql::params![req.source_entity_id, req.target_entity_id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Ok(EntityRelationship {
            id: row.get(0)?,
            source_entity_id: row.get(1)?,
            target_entity_id: row.get(2)?,
            relationship_type: row.get(3)?,
            strength: row.get(4)?,
            evidence_count: row.get(5)?,
            created_at: row.get(6)?,
        }),
        None => Err(EngError::Internal(
            "relationship upsert succeeded but fetch returned nothing".to_string(),
        )),
    }
}

/// Return all relationships where the entity appears as source or target.
pub async fn get_entity_relationships(
    db: &Database,
    entity_id: i64,
) -> Result<Vec<EntityRelationship>> {
    let conn = db.connection();

    let mut rows = conn
        .query(
            "SELECT id, source_entity_id, target_entity_id, relationship_type, strength, evidence_count, created_at \
             FROM entity_relationships \
             WHERE source_entity_id = ?1 OR target_entity_id = ?1 \
             ORDER BY strength DESC",
            libsql::params![entity_id],
        )
        .await?;

    let mut rels = Vec::new();
    while let Some(row) = rows.next().await? {
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
}

// -- Memory-Entity linking --

/// Link a memory to an entity with a salience score. Silently ignores duplicates.
pub async fn link_memory_entity(
    db: &Database,
    memory_id: i64,
    entity_id: i64,
    salience: f64,
) -> Result<()> {
    let conn = db.connection();
    conn.execute(
        "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, salience, created_at) \
         VALUES (?1, ?2, ?3, datetime('now'))",
        libsql::params![memory_id, entity_id, salience],
    )
    .await?;
    Ok(())
}

/// Return all entities linked to the given memory.
pub async fn get_memory_entities(db: &Database, memory_id: i64) -> Result<Vec<Entity>> {
    let conn = db.connection();
    let query = format!(
        "SELECT e.{cols} \
         FROM entities e \
         JOIN memory_entities me ON me.entity_id = e.id \
         WHERE me.memory_id = ?1 \
         ORDER BY me.salience DESC",
        cols = ENTITY_COLUMNS
            .split(", ")
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", e.")
    );

    let mut rows = conn.query(&query, libsql::params![memory_id]).await?;

    let mut entities = Vec::new();
    while let Some(row) = rows.next().await? {
        entities.push(row_to_entity(&row)?);
    }
    Ok(entities)
}

/// Return the IDs of all memories linked to the given entity.
pub async fn get_entity_memories(db: &Database, entity_id: i64) -> Result<Vec<i64>> {
    let conn = db.connection();

    let mut rows = conn
        .query(
            "SELECT memory_id FROM memory_entities WHERE entity_id = ?1",
            libsql::params![entity_id],
        )
        .await?;

    let mut memory_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        memory_ids.push(id);
    }
    Ok(memory_ids)
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
    has_alpha && word.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase())
}

/// Strip common leading/trailing punctuation from a word slice without allocating.
fn strip_punctuation(s: &str) -> &str {
    let punct = |c: char| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '`');
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
        link_memory_entity(db, memory_id, entity.id, 1.0).await?;
        entities.push(entity);
    }

    // Record pairwise co-occurrences for all entity pairs found in this memory
    for a in 0..entities.len() {
        for b in (a + 1)..entities.len() {
            let _ = record_cooccurrence(db, entities[a].id, entities[b].id).await;
        }
    }

    Ok(entities)
}
