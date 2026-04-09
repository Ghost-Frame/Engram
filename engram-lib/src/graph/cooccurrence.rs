use super::types::{Entity, GraphEdge, LinkType};
use crate::db::Database;
use crate::Result;
use tracing::info;

const ENTITY_COLUMNS: &str =
    "id, name, entity_type, description, aliases, user_id, space_id, \
     confidence, occurrence_count, first_seen_at, last_seen_at, created_at";

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

/// Build GraphEdge objects from the entity_cooccurrence table.
/// Each edge weight is the normalized cooccurrence count (count / max_count).
/// `window_size` is accepted for API compatibility but unused -- cooccurrences
/// are pre-computed per-memory rather than via a sliding text window.
pub async fn build_cooccurrence_edges(
    db: &Database,
    _window_size: usize,
) -> Result<Vec<GraphEdge>> {
    let conn = db.connection();

    // Find max count for normalization
    let mut max_row = conn
        .query(
            "SELECT MAX(count) FROM entity_cooccurrences",
            libsql::params![],
        )
        .await?;
    let max_count: f64 = match max_row.next().await? {
        Some(row) => row.get::<f64>(0).unwrap_or(1.0).max(1.0),
        None => return Ok(vec![]),
    };

    let mut rows = conn
        .query(
            "SELECT entity_a_id, entity_b_id, count FROM entity_cooccurrences ORDER BY count DESC",
            libsql::params![],
        )
        .await?;

    let mut edges = Vec::new();
    while let Some(row) = rows.next().await? {
        let a: i64 = row.get(0)?;
        let b: i64 = row.get(1)?;
        let count: f64 = row.get::<f64>(2).unwrap_or(1.0);
        edges.push(GraphEdge {
            source: format!("e{}", a),
            target: format!("e{}", b),
            link_type: LinkType::Mentions,
            weight: (count / max_count) as f32,
        });
    }

    Ok(edges)
}

/// Record a pairwise co-occurrence between two entities.
/// The pair is stored in canonical order (smaller id first) so that
/// (A, B) and (B, A) map to the same row.
pub async fn record_cooccurrence(
    db: &Database,
    entity_a: i64,
    entity_b: i64,
) -> Result<()> {
    let (lo, hi) = if entity_a <= entity_b {
        (entity_a, entity_b)
    } else {
        (entity_b, entity_a)
    };
    db.connection()
        .execute(
            "INSERT INTO entity_cooccurrences (entity_a_id, entity_b_id, count)
             VALUES (?1, ?2, 1)
             ON CONFLICT(entity_a_id, entity_b_id) DO UPDATE SET count = count + 1",
            libsql::params![lo, hi],
        )
        .await?;
    Ok(())
}

/// Rebuild all cooccurrence counts for a user from scratch.
/// Deletes existing pairs for user's entities, then reprocesses every memory
/// that has 2+ entities. Returns the total number of pairs upserted.
pub async fn rebuild_cooccurrences(db: &Database, user_id: i64) -> Result<i64> {
    let conn = db.connection();

    // Delete existing cooccurrences where both entities belong to this user
    conn.execute(
        "DELETE FROM entity_cooccurrences \
         WHERE entity_a_id IN (SELECT id FROM entities WHERE user_id = ?1) \
           AND entity_b_id IN (SELECT id FROM entities WHERE user_id = ?1)",
        libsql::params![user_id],
    )
    .await?;

    // Get all memories that have 2+ entities for this user
    let mut mem_rows = conn
        .query(
            "SELECT me.memory_id \
             FROM memory_entities me \
             JOIN entities e ON e.id = me.entity_id \
             WHERE e.user_id = ?1 \
             GROUP BY me.memory_id \
             HAVING COUNT(*) >= 2",
            libsql::params![user_id],
        )
        .await?;

    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = mem_rows.next().await? {
        memory_ids.push(row.get(0)?);
    }

    let mut total_pairs: i64 = 0;

    for memory_id in memory_ids {
        // Get all entity IDs linked to this memory (scoped to user)
        let mut ent_rows = conn
            .query(
                "SELECT me.entity_id \
                 FROM memory_entities me \
                 JOIN entities e ON e.id = me.entity_id \
                 WHERE me.memory_id = ?1 AND e.user_id = ?2",
                libsql::params![memory_id, user_id],
            )
            .await?;

        let mut entity_ids: Vec<i64> = Vec::new();
        while let Some(row) = ent_rows.next().await? {
            entity_ids.push(row.get(0)?);
        }

        // Upsert pairwise cooccurrences
        for i in 0..entity_ids.len() {
            for j in (i + 1)..entity_ids.len() {
                record_cooccurrence(db, entity_ids[i], entity_ids[j]).await?;
                total_pairs += 1;
            }
        }
    }

    info!(user_id, total_pairs, "cooccurrences_rebuilt");
    Ok(total_pairs)
}

/// Return entities that co-occur most frequently with the given entity,
/// scoped to `user_id`. Results ordered by cooccurrence count descending.
pub async fn get_cooccurring_entities(
    db: &Database,
    entity_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Entity>> {
    let conn = db.connection();
    let qualified_cols = ENTITY_COLUMNS
        .split(", ")
        .map(|c| format!("e.{}", c))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT {cols} \
         FROM entity_cooccurrences ec \
         JOIN entities e ON e.id = CASE \
             WHEN ec.entity_a_id = ?1 THEN ec.entity_b_id \
             ELSE ec.entity_a_id \
         END \
         WHERE (ec.entity_a_id = ?1 OR ec.entity_b_id = ?1) \
           AND e.user_id = ?2 \
         ORDER BY ec.count DESC \
         LIMIT ?3",
        cols = qualified_cols
    );

    let mut rows = conn
        .query(&query, libsql::params![entity_id, user_id, limit as i64])
        .await?;

    let mut entities = Vec::new();
    while let Some(row) = rows.next().await? {
        entities.push(row_to_entity(&row)?);
    }
    Ok(entities)
}
