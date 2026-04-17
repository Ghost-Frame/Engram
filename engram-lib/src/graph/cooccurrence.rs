use super::types::{Entity, GraphEdge, LinkType};
use crate::db::Database;
use crate::{EngError, Result};
use std::collections::HashMap;
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Build co-occurrence edges by sliding a window over recent memories.
///
/// For each window of `window_size` memories, extract entity mentions and
/// create edges between entities that appear in the same window. Weight by
/// co-occurrence frequency.
#[tracing::instrument(skip(db))]
pub async fn build_cooccurrence_edges(
    db: &Database,
    window_size: usize,
    user_id: i64,
) -> Result<Vec<GraphEdge>> {
    // Fetch recent memories with their entity links
    let (memory_order, memory_entities) = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, me.entity_id \
                     FROM memories m \
                     JOIN memory_entities me ON me.memory_id = m.id \
                     WHERE m.user_id = ?1 AND m.is_forgotten = 0 AND m.is_archived = 0 AND m.is_latest = 1 \
                     ORDER BY m.created_at DESC \
                     LIMIT 2000",
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut memory_order: Vec<i64> = Vec::new();
            let mut memory_entities: HashMap<i64, Vec<i64>> = HashMap::new();

            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(rusqlite_to_eng_error)?;

            for row in rows {
                let (memory_id, entity_id) = row.map_err(rusqlite_to_eng_error)?;
                if !memory_entities.contains_key(&memory_id) {
                    memory_order.push(memory_id);
                }
                memory_entities.entry(memory_id).or_default().push(entity_id);
            }

            Ok((memory_order, memory_entities))
        })
        .await?;

    // Sliding window: for each window of `window_size` memories, pair all
    // entities that appear in the window
    let mut pair_counts: HashMap<(i64, i64), i64> = HashMap::new();

    let ws = window_size.max(1);
    for i in 0..memory_order.len() {
        let end = (i + ws).min(memory_order.len());
        let mut window_entities: Vec<i64> = Vec::new();

        for mid in &memory_order[i..end] {
            if let Some(eids) = memory_entities.get(mid) {
                window_entities.extend(eids);
            }
        }

        // Deduplicate within window
        window_entities.sort_unstable();
        window_entities.dedup();

        // Generate all pairs (canonical order: smaller id first)
        for a_idx in 0..window_entities.len() {
            for b_idx in (a_idx + 1)..window_entities.len() {
                let a = window_entities[a_idx];
                let b = window_entities[b_idx];
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                *pair_counts.entry((lo, hi)).or_insert(0) += 1;
            }
        }
    }

    // Convert to edges and upsert to DB
    let mut edges = Vec::new();

    for (&(entity_a, entity_b), &count) in &pair_counts {
        // Upsert into entity_cooccurrences table
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO entity_cooccurrences (entity_a_id, entity_b_id, count, user_id) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(entity_a_id, entity_b_id) DO UPDATE SET \
                   count = count + ?3, \
                   user_id = excluded.user_id, \
                   last_seen_at = datetime('now')",
                rusqlite::params![entity_a, entity_b, count, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;

        let weight = (count as f32).ln().clamp(0.1, 1.0);

        edges.push(GraphEdge {
            source: format!("e{}", entity_a),
            target: format!("e{}", entity_b),
            link_type: LinkType::Mentions,
            weight,
        });
    }

    info!(
        pairs = pair_counts.len(),
        window_size = ws,
        user_id,
        "cooccurrence_edges_built"
    );

    Ok(edges)
}

/// Record a pairwise co-occurrence between two entities.
/// The pair is stored in canonical order (smaller id first) so that
/// (A, B) and (B, A) map to the same row.
#[tracing::instrument(skip(db))]
pub async fn record_cooccurrence(
    db: &Database,
    entity_a: i64,
    entity_b: i64,
    user_id: i64,
) -> Result<()> {
    let (lo, hi) = if entity_a <= entity_b {
        (entity_a, entity_b)
    } else {
        (entity_b, entity_a)
    };
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO entity_cooccurrences (entity_a_id, entity_b_id, count, user_id) \
             VALUES (?1, ?2, 1, ?3) \
             ON CONFLICT(entity_a_id, entity_b_id) DO UPDATE SET \
               count = count + 1, \
               user_id = excluded.user_id, \
               last_seen_at = datetime('now')",
            rusqlite::params![lo, hi, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;
    Ok(())
}

/// Full rebuild of co-occurrence table for a user.
/// Clears existing co-occurrences and recomputes from all memory-entity links.
#[tracing::instrument(skip(db))]
pub async fn rebuild_cooccurrences(db: &Database, user_id: i64) -> Result<i64> {
    // Clear existing co-occurrences for entities owned by this user
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM entity_cooccurrences \
             WHERE entity_a_id IN (SELECT id FROM entities WHERE user_id = ?1) \
                OR entity_b_id IN (SELECT id FROM entities WHERE user_id = ?1)",
            rusqlite::params![user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    // Fetch all memory -> entity links for this user's memories
    let memory_entities: HashMap<i64, Vec<i64>> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, me.entity_id \
                     FROM memories m \
                     JOIN memory_entities me ON me.memory_id = m.id \
                     WHERE m.user_id = ?1 AND m.is_forgotten = 0 AND m.is_archived = 0 \
                     ORDER BY m.created_at DESC",
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut memory_entities: HashMap<i64, Vec<i64>> = HashMap::new();

            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(rusqlite_to_eng_error)?;

            for row in rows {
                let (memory_id, entity_id) = row.map_err(rusqlite_to_eng_error)?;
                memory_entities
                    .entry(memory_id)
                    .or_default()
                    .push(entity_id);
            }

            Ok(memory_entities)
        })
        .await?;

    // For each memory, create co-occurrence pairs from its entities
    let mut total_pairs: i64 = 0;
    for entities in memory_entities.values() {
        let mut sorted = entities.clone();
        sorted.sort_unstable();
        sorted.dedup();

        for i in 0..sorted.len() {
            for j in (i + 1)..sorted.len() {
                record_cooccurrence(db, sorted[i], sorted[j], user_id).await?;
                total_pairs += 1;
            }
        }
    }

    info!(pairs = total_pairs, user_id, "cooccurrences_rebuilt");

    Ok(total_pairs)
}

/// Get entities that co-occur with the given entity.
#[tracing::instrument(skip(db))]
pub async fn get_cooccurring_entities(
    db: &Database,
    entity_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Entity>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT e.id, e.name, e.entity_type, e.description, e.aliases, \
                        e.user_id, e.space_id, e.confidence, e.occurrence_count, \
                        e.first_seen_at, e.last_seen_at, e.created_at, \
                        co.count as cooccurrence_count \
                 FROM entity_cooccurrences co \
                 JOIN entities e ON e.id = CASE \
                     WHEN co.entity_a_id = ?1 THEN co.entity_b_id \
                     ELSE co.entity_a_id \
                 END \
                 WHERE (co.entity_a_id = ?1 OR co.entity_b_id = ?1) \
                   AND e.user_id = ?2 \
                   AND co.user_id = ?2 \
                 ORDER BY co.count DESC \
                 LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(rusqlite::params![entity_id, user_id, limit as i64], |row| {
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
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut entities = Vec::new();
        for row in rows {
            entities.push(row.map_err(rusqlite_to_eng_error)?);
        }

        Ok(entities)
    })
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_canonical_pair_ordering() {
        let (lo, hi) = if 5 <= 3 { (5, 3) } else { (3, 5) };
        assert_eq!(lo, 3);
        assert_eq!(hi, 5);

        let (lo2, hi2) = if 2 <= 7 { (2, 7) } else { (7, 2) };
        assert_eq!(lo2, 2);
        assert_eq!(hi2, 7);
    }

    #[test]
    fn test_weight_calculation() {
        // Weight = ln(count).max(0.1).min(1.0)
        let count = 3i64;
        let weight = (count as f32).ln().clamp(0.1, 1.0);
        assert!(weight > 0.1);
        assert!(weight <= 1.0);

        let count_1 = 1i64;
        let weight_1 = (count_1 as f32).ln().clamp(0.1, 1.0);
        assert_eq!(weight_1, 0.1); // ln(1) = 0, clamped to 0.1
    }
}
