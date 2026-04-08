use super::types::GraphEdge;
use crate::db::Database;
use crate::Result;

pub async fn build_cooccurrence_edges(
    db: &Database,
    window_size: usize,
) -> Result<Vec<GraphEdge>> {
    todo!()
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
