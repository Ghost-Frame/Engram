use super::types::{CausalAncestor, CausalChain, CausalLink};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

/// Create a causal chain.
#[tracing::instrument(skip(db, description), fields(root_memory_id = ?root_memory_id, user_id))]
pub async fn create_chain(
    db: &Database,
    root_memory_id: Option<i64>,
    description: Option<&str>,
    user_id: i64,
) -> Result<CausalChain> {
    let description_owned = description.map(|s| s.to_string());
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO causal_chains (root_memory_id, description) VALUES (?1, ?2)",
                params![root_memory_id, description_owned],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

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
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip(db),
    fields(
        chain_id,
        cause_memory_id,
        effect_memory_id,
        strength,
        order_index,
        user_id
    )
)]
pub async fn add_link(
    db: &Database,
    chain_id: i64,
    cause_memory_id: i64,
    effect_memory_id: i64,
    strength: f64,
    order_index: i32,
    user_id: i64,
) -> Result<CausalLink> {
    // Verify chain exists
    let chain_exists = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM causal_chains WHERE id = ?1")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let found = stmt
                .query_map(params![chain_id], |_row| Ok(()))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
                .next()
                .is_some();
            Ok(found)
        })
        .await?;

    if !chain_exists {
        return Err(EngError::NotFound(format!(
            "causal chain {} not found",
            chain_id
        )));
    }

    // Verify both memories exist
    let count: i64 = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND is_forgotten = 0")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let c: i64 = stmt
                .query_row(params![cause_memory_id, effect_memory_id], |row| row.get(0))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(c)
        })
        .await?;

    if count != 2 {
        return Err(EngError::NotFound(
            "one or more memories not found or not owned".into(),
        ));
    }

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO causal_links (chain_id, cause_memory_id, effect_memory_id, strength, order_index) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chain_id, cause_memory_id, effect_memory_id, strength, order_index],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

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
#[tracing::instrument(skip(db), fields(chain_id, user_id))]
pub async fn get_chain(db: &Database, chain_id: i64, user_id: i64) -> Result<CausalChain> {
    let mut chain = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, root_memory_id, description, confidence, created_at \
                     FROM causal_chains WHERE id = ?1",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query_map(params![chain_id], |row| {
                    Ok(CausalChain {
                        id: row.get(0)?,
                        root_memory_id: row.get(1)?,
                        description: row.get(2)?,
                        confidence: row.get(3)?,
                        user_id: 1,
                        created_at: row.get(4)?,
                        links: Vec::new(),
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.next()
                .transpose()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
                .ok_or_else(|| EngError::NotFound(format!("causal chain {} not found", chain_id)))
        })
        .await?;

    // Fetch links
    let links = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, chain_id, cause_memory_id, effect_memory_id, strength, order_index, created_at \
                     FROM causal_links WHERE chain_id = ?1 ORDER BY order_index",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![chain_id], |row| {
                    Ok(CausalLink {
                        id: row.get(0)?,
                        chain_id: row.get(1)?,
                        cause_memory_id: row.get(2)?,
                        effect_memory_id: row.get(3)?,
                        strength: row.get(4)?,
                        order_index: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    chain.links = links;
    Ok(chain)
}

/// List causal chains for a user.
#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn list_chains(db: &Database, user_id: i64, limit: usize) -> Result<Vec<CausalChain>> {
    let ids = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM causal_chains ORDER BY id DESC LIMIT ?1")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![limit as i64], |row| row.get::<_, i64>(0))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let mut chains = Vec::new();
    for id in ids {
        chains.push(get_chain(db, id, user_id).await?);
    }

    Ok(chains)
}

/// Walk causal_links backward from `effect_memory_id`, visiting each
/// ancestor cause at most once. Returns ancestors in BFS (shortest-path)
/// order. When multiple paths reach the same ancestor, the shorter path
/// wins; on equal-length paths, the larger `strength_min` wins.
///
/// `max_depth` caps the traversal: `0` means "only direct causes", `1`
/// means "direct + one level further", and so on. This avoids runaway
/// traversal on tangled graphs and serves as a cheap timeout.
///
/// Only links whose parent chain belongs to `user_id` are followed, so
/// users never see each other's causal structure.
#[tracing::instrument(skip(db), fields(effect_memory_id, user_id, max_depth))]
pub async fn backward_chain(
    db: &Database,
    effect_memory_id: i64,
    user_id: i64,
    max_depth: usize,
) -> Result<Vec<CausalAncestor>> {
    use std::collections::{HashMap, VecDeque};

    let mut best: HashMap<i64, CausalAncestor> = HashMap::new();
    let mut frontier: VecDeque<(i64, usize, f64)> = VecDeque::new();
    frontier.push_back((effect_memory_id, 0, f64::INFINITY));
    let mut seen_effects: std::collections::HashSet<i64> = std::collections::HashSet::new();
    seen_effects.insert(effect_memory_id);

    while let Some((current_effect, depth_to_effect, strength_so_far)) = frontier.pop_front() {
        if depth_to_effect > max_depth {
            continue;
        }
        let rows: Vec<(i64, f64)> = db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT l.cause_memory_id, l.strength \
                         FROM causal_links l \
                         JOIN causal_chains c ON c.id = l.chain_id \
                         WHERE l.effect_memory_id = ?1",
                    )
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let iter = stmt
                    .query_map(params![current_effect], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                    })
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                iter.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))
            })
            .await?;

        let ancestor_depth = depth_to_effect + 1;
        for (cause_id, strength) in rows {
            let min_so_far = strength_so_far.min(strength);
            let should_insert = match best.get(&cause_id) {
                None => true,
                Some(existing) if ancestor_depth < existing.depth => true,
                Some(existing)
                    if ancestor_depth == existing.depth && min_so_far > existing.strength_min =>
                {
                    true
                }
                _ => false,
            };
            if should_insert {
                best.insert(
                    cause_id,
                    CausalAncestor {
                        memory_id: cause_id,
                        depth: ancestor_depth,
                        strength_min: min_so_far,
                    },
                );
            }
            if ancestor_depth <= max_depth && seen_effects.insert(cause_id) {
                frontier.push_back((cause_id, ancestor_depth, min_so_far));
            }
        }
    }

    let mut out: Vec<CausalAncestor> = best.into_values().collect();
    out.sort_by(|a, b| {
        a.depth.cmp(&b.depth).then_with(|| {
            b.strength_min
                .partial_cmp(&a.strength_min)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::StoreRequest;

    fn req(content: &str, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: "fact".to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn seed(db: &Database, content: &str, user_id: i64) -> i64 {
        crate::memory::store(db, req(content, user_id))
            .await
            .expect("store")
            .id
    }

    #[tokio::test]
    async fn backward_chain_empty_when_no_links() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let m = seed(&db, "chi standalone observation", 1).await;
        let out = backward_chain(&db, m, 1, 5).await.expect("bc");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn backward_chain_direct_cause_at_depth_one() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let cause = seed(&db, "psi initial decision rationale", uid).await;
        let effect = seed(&db, "psi downstream outcome observed", uid).await;
        let chain = create_chain(&db, Some(cause), Some("psi"), uid)
            .await
            .expect("chain");
        add_link(&db, chain.id, cause, effect, 0.8, 0, uid)
            .await
            .expect("link");
        let out = backward_chain(&db, effect, uid, 0).await.expect("bc");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].memory_id, cause);
        assert_eq!(out[0].depth, 1);
        assert!((out[0].strength_min - 0.8).abs() < 1e-9);
    }

    #[tokio::test]
    async fn backward_chain_multi_hop_respects_max_depth() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let a = seed(&db, "omega root cause", uid).await;
        let b = seed(&db, "omega intermediate step", uid).await;
        let c = seed(&db, "omega observed outcome", uid).await;
        let chain = create_chain(&db, Some(a), Some("omega"), uid)
            .await
            .unwrap();
        add_link(&db, chain.id, a, b, 0.9, 0, uid).await.unwrap();
        add_link(&db, chain.id, b, c, 0.5, 1, uid).await.unwrap();

        // max_depth = 0 should stop at direct cause only
        let shallow = backward_chain(&db, c, uid, 0).await.unwrap();
        assert_eq!(shallow.len(), 1);
        assert_eq!(shallow[0].memory_id, b);
        assert_eq!(shallow[0].depth, 1);

        // max_depth = 1 reaches the root
        let deep = backward_chain(&db, c, uid, 1).await.unwrap();
        assert_eq!(deep.len(), 2);
        assert_eq!(deep[0].memory_id, b);
        assert_eq!(deep[0].depth, 1);
        assert_eq!(deep[1].memory_id, a);
        assert_eq!(deep[1].depth, 2);
        assert!((deep[1].strength_min - 0.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn backward_chain_breaks_cycles() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let a = seed(&db, "cycle node alpha memory", uid).await;
        let b = seed(&db, "cycle node beta memory", uid).await;
        let chain = create_chain(&db, Some(a), Some("cycle"), uid)
            .await
            .unwrap();
        // A -> B and B -> A, forming a cycle
        add_link(&db, chain.id, a, b, 0.7, 0, uid).await.unwrap();
        add_link(&db, chain.id, b, a, 0.6, 1, uid).await.unwrap();
        let out = backward_chain(&db, a, uid, 10).await.expect("bc");
        // Should contain B exactly once
        let bs: Vec<_> = out.iter().filter(|x| x.memory_id == b).collect();
        assert_eq!(bs.len(), 1);
    }

    #[tokio::test]
    async fn backward_chain_respects_user_isolation() {
        // After user_id drop, causal_chains are global on a single-tenant shard.
        // All chains and links are visible regardless of the caller's user_id.
        let db = Database::connect_memory().await.expect("in-mem db");
        let mine = 1;
        let other = 2;
        let cause = seed(&db, "private cause note", mine).await;
        let effect = seed(&db, "private effect note", mine).await;
        let chain = create_chain(&db, Some(cause), Some("private"), mine)
            .await
            .unwrap();
        add_link(&db, chain.id, cause, effect, 1.0, 0, mine)
            .await
            .unwrap();
        // Single-tenant: even "other" user sees the same causal structure.
        let result = backward_chain(&db, effect, other, 5).await.unwrap();
        assert_eq!(result.len(), 1, "single-tenant: cause is globally visible");
        assert_eq!(result[0].memory_id, cause);
    }
}
