use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Types of connections between brain patterns. Mirrors the eidolon
/// edge taxonomy: association (cosine similarity), temporal (co-occurrence
/// within a time window), contradiction (high sim + same category +
/// different content), and causal (NLP-scored cause-effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Association,
    Temporal,
    Contradiction,
    Causal,
}

impl fmt::Display for EdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeType::Association => write!(f, "association"),
            EdgeType::Temporal => write!(f, "temporal"),
            EdgeType::Contradiction => write!(f, "contradiction"),
            EdgeType::Causal => write!(f, "causal"),
        }
    }
}

impl EdgeType {
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "temporal" => EdgeType::Temporal,
            "contradiction" => EdgeType::Contradiction,
            "causal" => EdgeType::Causal,
            _ => EdgeType::Association,
        }
    }
}

/// A weighted, typed edge between two brain patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainEdge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub weight: f32,
    pub edge_type: EdgeType,
    pub user_id: i64,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Database CRUD
// ---------------------------------------------------------------------------

/// Insert a new edge. If the (source, target, type) triple already exists,
/// update the weight to the max of old and new.
pub async fn store_edge(
    db: &Database,
    source_id: i64,
    target_id: i64,
    weight: f32,
    edge_type: EdgeType,
    user_id: i64,
) -> Result<()> {
    let edge_type_str = edge_type.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO brain_edges (source_id, target_id, weight, edge_type, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(source_id, target_id, edge_type) \
             DO UPDATE SET weight = MAX(weight, excluded.weight)",
            rusqlite::params![
                source_id,
                target_id,
                weight as f64,
                edge_type_str,
                user_id
            ],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Get all edges originating from a given pattern.
pub async fn get_edges_from(db: &Database, source_id: i64, user_id: i64) -> Result<Vec<BrainEdge>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, source_id, target_id, weight, edge_type, user_id, created_at \
                 FROM brain_edges WHERE source_id = ?1 AND user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let edges = stmt
            .query_map(rusqlite::params![source_id, user_id], |row| {
                Ok(row_to_edge_raw(row))
            })
            .map_err(rusqlite_to_eng_error)?
            .map(|r| r.map_err(rusqlite_to_eng_error).and_then(|inner| inner))
            .collect::<Result<Vec<BrainEdge>>>()?;

        Ok(edges)
    })
    .await
}

/// Get all edges connected to a pattern (either direction).
pub async fn get_edges_for(db: &Database, pattern_id: i64, user_id: i64) -> Result<Vec<BrainEdge>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, source_id, target_id, weight, edge_type, user_id, created_at \
                 FROM brain_edges \
                 WHERE (source_id = ?1 OR target_id = ?1) AND user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let edges = stmt
            .query_map(rusqlite::params![pattern_id, user_id], |row| {
                Ok(row_to_edge_raw(row))
            })
            .map_err(rusqlite_to_eng_error)?
            .map(|r| r.map_err(rusqlite_to_eng_error).and_then(|inner| inner))
            .collect::<Result<Vec<BrainEdge>>>()?;

        Ok(edges)
    })
    .await
}

/// Strengthen an edge by adding a Hebbian boost. The weight is clamped
/// to [0, 1].
pub async fn strengthen_edge(
    db: &Database,
    source_id: i64,
    target_id: i64,
    edge_type: EdgeType,
    boost: f32,
    user_id: i64,
) -> Result<()> {
    let edge_type_str = edge_type.to_string();
    let affected = db
        .write(move |conn| {
            let n = conn
                .execute(
                    "UPDATE brain_edges \
                     SET weight = MIN(1.0, weight + ?1) \
                     WHERE source_id = ?2 AND target_id = ?3 \
                       AND edge_type = ?4 AND user_id = ?5",
                    rusqlite::params![
                        boost as f64,
                        source_id,
                        target_id,
                        edge_type_str,
                        user_id
                    ],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(n)
        })
        .await?;

    if affected == 0 {
        // Edge doesn't exist yet -- create it with the boost as initial weight
        store_edge(db, source_id, target_id, boost, edge_type, user_id).await?;
    }
    Ok(())
}

/// Decay all edge weights for a user by multiplying with the given rate.
/// Returns the number of affected edges.
pub async fn decay_edges(db: &Database, user_id: i64, rate: f32) -> Result<usize> {
    db.write(move |conn| {
        let n = conn
            .execute(
                "UPDATE brain_edges SET weight = weight * ?1 WHERE user_id = ?2",
                rusqlite::params![rate as f64, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(n)
    })
    .await
}

/// Remove edges whose weight has fallen below the threshold.
/// Returns the number of pruned edges.
pub async fn prune_edges(db: &Database, user_id: i64, threshold: f32) -> Result<usize> {
    db.write(move |conn| {
        let n = conn
            .execute(
                "DELETE FROM brain_edges WHERE user_id = ?1 AND weight < ?2",
                rusqlite::params![user_id, threshold as f64],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(n)
    })
    .await
}

/// Count edges for a user.
pub async fn count_edges(db: &Database, user_id: i64) -> Result<i64> {
    db.read(move |conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM brain_edges WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(count)
    })
    .await
}

/// Delete a specific edge.
#[allow(dead_code)]
pub async fn delete_edge(
    db: &Database,
    source_id: i64,
    target_id: i64,
    edge_type: EdgeType,
    user_id: i64,
) -> Result<()> {
    let edge_type_str = edge_type.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM brain_edges \
             WHERE source_id = ?1 AND target_id = ?2 \
               AND edge_type = ?3 AND user_id = ?4",
            rusqlite::params![source_id, target_id, edge_type_str, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_edge_raw(row: &rusqlite::Row<'_>) -> Result<BrainEdge> {
    let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
    let source_id: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
    let target_id: i64 = row.get(2).map_err(rusqlite_to_eng_error)?;
    let weight: f64 = row.get(3).map_err(rusqlite_to_eng_error)?;
    let edge_type_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let user_id: i64 = row.get(5).map_err(rusqlite_to_eng_error)?;
    let created_at: String = row.get(6).map_err(rusqlite_to_eng_error)?;

    Ok(BrainEdge {
        id,
        source_id,
        target_id,
        weight: weight as f32,
        edge_type: EdgeType::from_str_loose(&edge_type_str),
        user_id,
        created_at,
    })
}
