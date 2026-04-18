use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use kleos_lib::facts::list_facts;
use kleos_lib::graph::{
    builder::build_graph_data,
    communities::{detect_communities, get_community_members, get_community_stats},
    cooccurrence::{get_cooccurring_entities, rebuild_cooccurrences},
    entities::{
        delete_relationship, link_memory_entity, search_entity_memories, unlink_memory_entity,
        update_entity,
    },
    pagerank::update_pagerank_scores,
    search::{graph_search, neighborhood_filtered},
    types::{CreateEntityRequest, CreateRelationshipRequest, GraphBuildOptions},
};
use kleos_lib::validation::{
    MAX_ENTITY_RELATIONSHIPS, MAX_GRAPH_BUILD_NODES, MAX_GRAPH_NEIGHBORHOOD_DEPTH,
    MAX_MEMORY_ENTITY_FANOUT,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{
    DeleteRelationshipBody, EntitySearchBody, FactsQuery, GraphQuery, GraphSearchBody, ListQuery,
    NeighborhoodQuery, RelationshipQuery, UpdateEntityBody,
};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Entity CRUD
        .route(
            "/entities",
            post(create_entity_handler).get(list_entities_handler),
        )
        .route(
            "/entities/{id}",
            get(get_entity_handler)
                .put(update_entity_handler)
                .delete(delete_entity_handler),
        )
        .route(
            "/entities/{id}/relationships",
            get(entity_relationships_handler).delete(delete_relationship_handler),
        )
        .route("/entities/{id}/memories", get(entity_memories_handler))
        .route("/entities/{id}/search", post(entity_search_handler))
        .route(
            "/entities/{id}/memories/{mid}",
            put(link_entity_memory_handler).delete(unlink_entity_memory_handler),
        )
        .route(
            "/entities/{id}/cooccurrences",
            get(entity_cooccurrences_handler),
        )
        // Relationships
        .route("/entity-relationships", post(create_relationship_handler))
        // Graph operations
        .route("/graph", get(graph_handler))
        .route("/graph/raw", get(graph_raw_handler))
        .route("/graph/view", get(graph_view_handler))
        .route("/graph/build", post(build_graph_handler))
        .route("/graph/search", post(graph_search_handler))
        .route("/graph/neighborhood/{id}", get(neighborhood_handler))
        // Communities
        .route("/communities", get(communities_handler))
        .route("/communities/{id}", get(community_detail_handler))
        .route("/graph/communities", post(detect_communities_handler))
        .route(
            "/graph/communities/{id}/members",
            get(community_members_handler),
        )
        .route("/graph/communities/stats", get(community_stats_handler))
        // PageRank
        .route("/graph/pagerank", post(pagerank_handler))
        // Cooccurrence
        .route(
            "/graph/cooccurrences/rebuild",
            post(rebuild_cooccurrences_handler),
        )
        // Memory entity extraction
        .route("/memory/{id}/entities", get(memory_entities_handler))
        .route("/facts", get(facts_handler))
}

// ---------------------------------------------------------------------------
// POST /entities
// ---------------------------------------------------------------------------

async fn create_entity_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateEntityRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);

    let entity_type = req.entity_type.as_deref().unwrap_or("unknown").to_string();
    let description = req.description.clone();
    let aliases_json = req
        .aliases
        .as_ref()
        .and_then(|a| serde_json::to_string(a).ok());
    let space_id = req.space_id;
    let user_id = auth.user_id;
    let name = req.name.clone();

    // INSERT ... RETURNING avoids the cross-connection last_insert_rowid() race
    // that could hand a caller another tenant's row under concurrency.
    let entity = state
        .db
        .write(move |conn| {
            conn.query_row(
                "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 RETURNING id, name, entity_type, description, aliases, user_id, space_id, \
                 confidence, occurrence_count, first_seen_at, last_seen_at, created_at",
                params![name, entity_type, description, aliases_json, user_id, space_id],
                row_to_entity_json,
            )
            .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    Ok((StatusCode::CREATED, Json(entity)))
}

// ---------------------------------------------------------------------------
// GET /entities
// ---------------------------------------------------------------------------

async fn list_entities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).min(1000);
    let offset = params.offset.unwrap_or(0);
    let user_id = auth.user_id;

    let results = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, entity_type, description, aliases, user_id, space_id, \
                     confidence, occurrence_count, first_seen_at, last_seen_at, created_at \
                     FROM entities WHERE user_id = ?1 \
                     ORDER BY occurrence_count DESC \
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(kleos_lib::EngError::Database)?;

            let rows = stmt
                .query_map(params![user_id, limit, offset], row_to_entity_json)
                .map_err(kleos_lib::EngError::Database)?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    Ok(Json(json!({ "entities": results })))
}

// ---------------------------------------------------------------------------
// GET /entities/{id}
// ---------------------------------------------------------------------------

async fn get_entity_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    let entity = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT id, name, entity_type, description, aliases, user_id, space_id, \
                 confidence, occurrence_count, first_seen_at, last_seen_at, created_at \
                 FROM entities WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                row_to_entity_json,
            )
            .optional()
            .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    match entity {
        Some(e) => Ok(Json(e)),
        None => Err(AppError(kleos_lib::EngError::NotFound(format!(
            "entity {} not found",
            id
        )))),
    }
}

// ---------------------------------------------------------------------------
// PUT /entities/{id}
// ---------------------------------------------------------------------------

async fn update_entity_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateEntityBody>,
) -> Result<Json<Value>, AppError> {
    let metadata = body
        .metadata
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok());
    let entity = update_entity(
        &state.db,
        id,
        auth.user_id,
        body.name.as_deref(),
        body.entity_type.as_deref(),
        body.description.as_deref(),
        metadata.as_deref(),
    )
    .await
    .map_err(AppError)?;

    Ok(Json(json!(entity)))
}

// ---------------------------------------------------------------------------
// DELETE /entities/{id}
// ---------------------------------------------------------------------------

async fn delete_entity_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    state
        .db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM entities WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )
            .map_err(kleos_lib::EngError::Database)?;
            Ok(())
        })
        .await?;

    Ok(Json(json!({ "deleted": true, "id": id })))
}

// ---------------------------------------------------------------------------
// GET /entities/{id}/relationships
// ---------------------------------------------------------------------------

async fn entity_relationships_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<RelationshipQuery>,
) -> Result<Json<Value>, AppError> {
    // SECURITY/DoS: cap the fan-out so a hot entity cannot return an unbounded
    // result set and starve server memory.
    let user_id = auth.user_id;

    let relationships = state
        .db
        .read(move |conn| {
            if let Some(relationship_type) = params.relationship_type {
                let mut stmt = conn
                    .prepare(
                        "SELECT er.id, er.source_entity_id, er.target_entity_id, er.relationship_type, \
                         er.strength, er.evidence_count, er.created_at \
                         FROM entity_relationships er \
                         WHERE (er.source_entity_id = ?1 OR er.target_entity_id = ?1) \
                           AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?2) \
                           AND er.relationship_type = ?3 \
                         ORDER BY er.strength DESC, er.id DESC \
                         LIMIT ?4",
                    )
                    .map_err(kleos_lib::EngError::Database)?;

                let rows = stmt
                    .query_map(
                        params![id, user_id, relationship_type, MAX_ENTITY_RELATIONSHIPS as i64],
                        row_to_relationship_json,
                    )
                    .map_err(kleos_lib::EngError::Database)?;

                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(kleos_lib::EngError::Database)
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT er.id, er.source_entity_id, er.target_entity_id, er.relationship_type, \
                         er.strength, er.evidence_count, er.created_at \
                         FROM entity_relationships er \
                         WHERE (er.source_entity_id = ?1 OR er.target_entity_id = ?1) \
                         AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?2) \
                         ORDER BY er.strength DESC, er.id DESC \
                         LIMIT ?3",
                    )
                    .map_err(kleos_lib::EngError::Database)?;

                let rows = stmt
                    .query_map(
                        params![id, user_id, MAX_ENTITY_RELATIONSHIPS as i64],
                        row_to_relationship_json,
                    )
                    .map_err(kleos_lib::EngError::Database)?;

                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(kleos_lib::EngError::Database)
            }
        })
        .await?;

    Ok(Json(json!({ "relationships": relationships })))
}

// ---------------------------------------------------------------------------
// DELETE /entities/{id}/relationships
// ---------------------------------------------------------------------------

async fn delete_relationship_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<DeleteRelationshipBody>,
) -> Result<Json<Value>, AppError> {
    delete_relationship(
        &state.db,
        id,
        body.target_entity_id,
        auth.user_id,
        body.relationship_type.as_deref(),
    )
    .await
    .map_err(AppError)?;

    Ok(Json(json!({
        "deleted": true,
        "source_entity_id": id,
        "target_entity_id": body.target_entity_id,
        "relationship_type": body.relationship_type,
    })))
}

// ---------------------------------------------------------------------------
// GET /entities/{id}/memories
// ---------------------------------------------------------------------------

async fn entity_memories_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    let memory_ids = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT me.memory_id FROM memory_entities me \
                     JOIN entities e ON e.id = me.entity_id \
                     WHERE me.entity_id = ?1 AND e.user_id = ?2",
                )
                .map_err(kleos_lib::EngError::Database)?;

            let rows = stmt
                .query_map(params![id, user_id], |row| row.get::<_, i64>(0))
                .map_err(kleos_lib::EngError::Database)?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    Ok(Json(json!({ "memory_ids": memory_ids })))
}

// ---------------------------------------------------------------------------
// POST /entities/{id}/search
// ---------------------------------------------------------------------------

async fn entity_search_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<EntitySearchBody>,
) -> Result<Json<Value>, AppError> {
    let memories = search_entity_memories(
        &state.db,
        id,
        auth.user_id,
        &body.query,
        body.limit.unwrap_or(20).min(1000),
    )
    .await
    .map_err(AppError)?;

    Ok(Json(json!({ "memories": memories })))
}

// ---------------------------------------------------------------------------
// PUT /entities/{id}/memories/{mid}
// ---------------------------------------------------------------------------

async fn link_entity_memory_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((entity_id, memory_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    link_memory_entity(&state.db, memory_id, entity_id, auth.user_id, 1.0)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({
        "linked": true,
        "entity_id": entity_id,
        "memory_id": memory_id,
    })))
}

// ---------------------------------------------------------------------------
// DELETE /entities/{id}/memories/{mid}
// ---------------------------------------------------------------------------

async fn unlink_entity_memory_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((entity_id, memory_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    unlink_memory_entity(&state.db, memory_id, entity_id, auth.user_id)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({
        "deleted": true,
        "entity_id": entity_id,
        "memory_id": memory_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /entity-relationships
// ---------------------------------------------------------------------------

async fn create_relationship_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(req): Json<CreateRelationshipRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.user_id;
    let source_id = req.source_entity_id;
    let target_id = req.target_entity_id;

    // Verify both entities belong to the authenticated user
    let count: i64 = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE id IN (?1, ?2) AND user_id = ?3",
                params![source_id, target_id, user_id],
                |row| row.get(0),
            )
            .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    if count < 2 {
        return Err(AppError(kleos_lib::EngError::NotFound(
            "one or both entities not found".into(),
        )));
    }

    let rel_type = req
        .relationship_type
        .as_deref()
        .unwrap_or("related")
        .to_string();
    let strength = req.strength.unwrap_or(1.0);

    // INSERT ... RETURNING avoids the cross-connection last_insert_rowid()
    // race that could otherwise leak another tenant's relationship row.
    let relationship = state
        .db
        .write(move |conn| {
            conn.query_row(
                "INSERT INTO entity_relationships \
                 (source_entity_id, target_entity_id, relationship_type, strength) \
                 VALUES (?1, ?2, ?3, ?4) \
                 RETURNING id, source_entity_id, target_entity_id, relationship_type, \
                 strength, evidence_count, created_at",
                params![source_id, target_id, rel_type, strength],
                row_to_relationship_json,
            )
            .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    Ok((StatusCode::CREATED, Json(relationship)))
}

// ---------------------------------------------------------------------------
// GET /graph  (accepts ?limit=N or ?max=N for GUI compat, ?depth= is accepted but unused)
// ---------------------------------------------------------------------------

async fn graph_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<GraphQuery>,
) -> Result<Json<Value>, AppError> {
    let cap = params.max.or(params.limit).unwrap_or(500).min(5000) as usize;
    let opts = GraphBuildOptions {
        user_id: auth.user_id,
        limit: Some(cap),
    };
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    let node_count = result.nodes.len();
    let edge_count = result.edges.len();
    Ok(Json(json!({
        "nodes": result.nodes,
        "edges": result.edges,
        "node_count": node_count,
        "edge_count": edge_count,
    })))
}

// ---------------------------------------------------------------------------
// GET /graph/raw
// ---------------------------------------------------------------------------

async fn graph_raw_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = GraphBuildOptions {
        user_id: auth.user_id,
        limit: Some(params.limit.unwrap_or(500).min(5000) as usize),
    };
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    Ok(Json(json!({
        "nodes": result.nodes,
        "edges": result.edges,
        "raw": true,
    })))
}

// ---------------------------------------------------------------------------
// GET /graph/view
// ---------------------------------------------------------------------------

async fn graph_view_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = GraphBuildOptions {
        user_id: auth.user_id,
        limit: Some(params.limit.unwrap_or(500).min(5000) as usize),
    };
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    Ok(Json(json!({
        "nodes": result.nodes,
        "edges": result.edges,
        "view": "force",
    })))
}

// ---------------------------------------------------------------------------
// POST /graph/build
// ---------------------------------------------------------------------------

async fn build_graph_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut opts): Json<GraphBuildOptions>,
) -> Result<Json<Value>, AppError> {
    opts.user_id = auth.user_id;
    // SECURITY/DoS: clamp caller-supplied node cap so a single request cannot
    // force the server to materialize an arbitrarily large graph.
    opts.limit = Some(match opts.limit {
        Some(0) => {
            return Err(AppError::from(kleos_lib::EngError::InvalidInput(
                "limit must be >= 1".into(),
            )))
        }
        Some(n) => n.min(MAX_GRAPH_BUILD_NODES),
        None => MAX_GRAPH_BUILD_NODES,
    });
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    Ok(Json(json!(result)))
}

// ---------------------------------------------------------------------------
// POST /graph/search
// ---------------------------------------------------------------------------

async fn graph_search_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GraphSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20).min(1000);
    let nodes = graph_search(&state.db, &body.query, limit, auth.user_id).await?;
    Ok(Json(json!({ "nodes": nodes })))
}

// ---------------------------------------------------------------------------
// GET /graph/neighborhood/{id}
// ---------------------------------------------------------------------------

async fn neighborhood_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
    Query(params): Query<NeighborhoodQuery>,
) -> Result<Json<Value>, AppError> {
    // SECURITY/DoS: neighborhood expansion is super-linear in depth. Cap the
    // caller-supplied depth so a single request cannot amplify into a full
    // graph traversal.
    let depth = params
        .depth
        .unwrap_or(2)
        .clamp(1, MAX_GRAPH_NEIGHBORHOOD_DEPTH);

    let link_types: Option<Vec<String>> = params.link_types.map(|lt| {
        lt.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    let (nodes, edges, hops) =
        neighborhood_filtered(&state.db, &id, depth, auth.user_id, link_types.as_deref()).await?;
    Ok(Json(
        json!({ "nodes": nodes, "edges": edges, "hops": hops }),
    ))
}

// ---------------------------------------------------------------------------
// GET /memory/{id}/entities
// ---------------------------------------------------------------------------

async fn memory_entities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // SECURITY/DoS: cap entity fan-out per memory to avoid unbounded result sets.
    let user_id = auth.user_id;

    let entities = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT e.id, e.name, e.entity_type, me.salience \
                     FROM memory_entities me \
                     JOIN entities e ON e.id = me.entity_id \
                     JOIN memories m ON m.id = me.memory_id \
                     WHERE me.memory_id = ?1 AND m.user_id = ?2 \
                     ORDER BY me.salience DESC \
                     LIMIT ?3",
                )
                .map_err(kleos_lib::EngError::Database)?;

            let rows = stmt
                .query_map(params![id, user_id, MAX_MEMORY_ENTITY_FANOUT], |row| {
                    let eid: i64 = row.get(0)?;
                    let name: String = row.get(1)?;
                    let entity_type: String = row.get(2)?;
                    let salience: f64 = row.get(3)?;
                    Ok(json!({
                        "id": eid,
                        "name": name,
                        "entity_type": entity_type,
                        "salience": salience,
                    }))
                })
                .map_err(kleos_lib::EngError::Database)?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(kleos_lib::EngError::Database)
        })
        .await?;

    Ok(Json(json!({ "entities": entities })))
}

// ---------------------------------------------------------------------------
// GET /communities
// ---------------------------------------------------------------------------

async fn communities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    // Fetch community -> memory_id mapping for the GUI graph visualization.
    // The GUI needs {id, top_memories: [memId, ...]} to map graph nodes to communities.
    let communities: Vec<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT community_id, id FROM memories \
                     WHERE user_id = ?1 AND community_id IS NOT NULL \
                       AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                     ORDER BY community_id, importance DESC",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    let cid: i64 = row.get(0)?;
                    let mid: i64 = row.get(1)?;
                    Ok((cid, mid))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let mut comm_map: std::collections::BTreeMap<i64, Vec<i64>> =
                std::collections::BTreeMap::new();
            for row in rows {
                let (cid, mid) =
                    row.map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                comm_map.entry(cid).or_default().push(mid);
            }

            let result: Vec<Value> = comm_map
                .into_iter()
                .map(|(cid, mids)| json!({"id": cid, "top_memories": mids}))
                .collect();
            Ok(result)
        })
        .await
        .map_err(AppError)?;

    let count = communities.len();
    Ok(Json(json!({ "communities": communities, "count": count })))
}

// ---------------------------------------------------------------------------
// GET /communities/{id}
// ---------------------------------------------------------------------------

async fn community_detail_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let stats = get_community_stats(&state.db, auth.user_id)
        .await
        .map_err(AppError)?;
    let members = get_community_members(&state.db, id, auth.user_id, 50)
        .await
        .map_err(AppError)?;
    let community = stats.into_iter().find(|item| item.community_id == id);

    Ok(Json(json!({
        "community": community,
        "members": members,
    })))
}

// ---------------------------------------------------------------------------
// POST /graph/communities
// ---------------------------------------------------------------------------

async fn detect_communities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let result = detect_communities(&state.db, auth.user_id, 25)
        .await
        .map_err(AppError)?;
    Ok(Json(json!(result)))
}

// ---------------------------------------------------------------------------
// GET /graph/communities/{id}/members
// ---------------------------------------------------------------------------

async fn community_members_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).min(1000) as usize;
    let members = get_community_members(&state.db, id, auth.user_id, limit)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "members": members })))
}

// ---------------------------------------------------------------------------
// GET /graph/communities/stats
// ---------------------------------------------------------------------------

async fn community_stats_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_community_stats(&state.db, auth.user_id)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "stats": stats })))
}

// ---------------------------------------------------------------------------
// POST /graph/pagerank
// ---------------------------------------------------------------------------

async fn pagerank_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let result = update_pagerank_scores(&state.db, auth.user_id)
        .await
        .map_err(AppError)?;
    Ok(Json(json!(result)))
}

// ---------------------------------------------------------------------------
// POST /graph/cooccurrences/rebuild
// ---------------------------------------------------------------------------

async fn rebuild_cooccurrences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let count = rebuild_cooccurrences(&state.db, auth.user_id)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "rebuilt": count })))
}

// ---------------------------------------------------------------------------
// GET /entities/{id}/cooccurrences
// ---------------------------------------------------------------------------

async fn entity_cooccurrences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(1000) as usize;
    let entities = get_cooccurring_entities(&state.db, id, auth.user_id, limit)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "cooccurrences": entities })))
}

// ---------------------------------------------------------------------------
// GET /facts
// ---------------------------------------------------------------------------

async fn facts_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<FactsQuery>,
) -> Result<Json<Value>, AppError> {
    let facts = list_facts(
        &state.db,
        auth.user_id,
        params.memory_id,
        params.limit.unwrap_or(50).min(1000),
    )
    .await
    .map_err(AppError)?;
    Ok(Json(json!({ "facts": facts })))
}

// ---------------------------------------------------------------------------
// Helpers -- row mapping
// ---------------------------------------------------------------------------

fn row_to_entity_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let entity_type: String = row.get(2)?;
    let description: Option<String> = row.get(3)?;
    let aliases_raw: Option<String> = row.get(4)?;
    let user_id: i64 = row.get(5)?;
    let space_id: Option<i64> = row.get(6)?;
    let confidence: f64 = row.get(7)?;
    let occurrence_count: i32 = row.get(8)?;
    let first_seen_at: String = row.get(9)?;
    let last_seen_at: String = row.get(10)?;
    let created_at: String = row.get(11)?;

    let aliases: Value = aliases_raw
        .as_ref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Array(vec![]));

    Ok(json!({
        "id": id,
        "name": name,
        "entity_type": entity_type,
        "description": description,
        "aliases": aliases,
        "user_id": user_id,
        "space_id": space_id,
        "confidence": confidence,
        "occurrence_count": occurrence_count,
        "first_seen_at": first_seen_at,
        "last_seen_at": last_seen_at,
        "created_at": created_at,
    }))
}

fn row_to_relationship_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let id: i64 = row.get(0)?;
    let source_entity_id: i64 = row.get(1)?;
    let target_entity_id: i64 = row.get(2)?;
    let relationship_type: String = row.get(3)?;
    let strength: f64 = row.get(4)?;
    let evidence_count: i32 = row.get(5)?;
    let created_at: String = row.get(6)?;

    Ok(json!({
        "id": id,
        "source_entity_id": source_entity_id,
        "target_entity_id": target_entity_id,
        "relationship_type": relationship_type,
        "strength": strength,
        "evidence_count": evidence_count,
        "created_at": created_at,
    }))
}
