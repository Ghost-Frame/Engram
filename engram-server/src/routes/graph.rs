use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use engram_lib::facts::list_facts;
use engram_lib::graph::{
    builder::build_graph_data,
    communities::{detect_communities, get_community_members, get_community_stats},
    cooccurrence::{get_cooccurring_entities, rebuild_cooccurrences},
    entities::{
        delete_relationship, link_memory_entity, search_entity_memories, unlink_memory_entity,
        update_entity,
    },
    pagerank::update_pagerank_scores,
    search::{graph_search, neighborhood},
    types::{CreateEntityRequest, CreateRelationshipRequest, GraphBuildOptions},
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

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

    let conn = state.db.connection();
    let entity_type = req.entity_type.as_deref().unwrap_or("unknown");
    let description = req.description.as_deref();
    let aliases_json = req
        .aliases
        .as_ref()
        .and_then(|a| serde_json::to_string(a).ok());
    let space_id = req.space_id;
    let user_id = req.user_id.unwrap_or(1);

    conn.execute(
        "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        libsql::params![
            req.name.clone(),
            entity_type.to_string(),
            description.map(|s| s.to_string()),
            aliases_json.clone(),
            user_id,
            space_id
        ],
    )
    .await
    .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    let mut rows = conn
        .query(
            "SELECT id, name, entity_type, description, aliases, user_id, space_id, \
             confidence, occurrence_count, first_seen_at, last_seen_at, created_at \
             FROM entities WHERE rowid = last_insert_rowid()",
            (),
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        let entity = row_to_entity_json(&row)?;
        Ok((StatusCode::CREATED, Json(entity)))
    } else {
        Err(AppError(engram_lib::EngError::Internal(
            "failed to fetch created entity".into(),
        )))
    }
}

// ---------------------------------------------------------------------------
// GET /entities
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpdateEntityBody {
    pub name: Option<String>,
    pub entity_type: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EntitySearchBody {
    pub query: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RelationshipQuery {
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteRelationshipBody {
    pub target_entity_id: i64,
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FactsQuery {
    pub limit: Option<usize>,
    pub memory_id: Option<i64>,
}

async fn list_entities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    let user_id = auth.user_id;

    let conn = state.db.connection();
    let mut rows = conn
        .query(
            "SELECT id, name, entity_type, description, aliases, user_id, space_id, \
             confidence, occurrence_count, first_seen_at, last_seen_at, created_at \
             FROM entities WHERE user_id = ?1 \
             ORDER BY occurrence_count DESC \
             LIMIT ?2 OFFSET ?3",
            libsql::params![user_id, limit, offset],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    let mut results: Vec<Value> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        results.push(row_to_entity_json(&row)?);
    }

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
    let conn = state.db.connection();
    let mut rows = conn
        .query(
            "SELECT id, name, entity_type, description, aliases, user_id, space_id, \
             confidence, occurrence_count, first_seen_at, last_seen_at, created_at \
             FROM entities WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        Ok(Json(row_to_entity_json(&row)?))
    } else {
        Err(AppError(engram_lib::EngError::NotFound(format!(
            "entity {} not found",
            id
        ))))
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
    let conn = state.db.connection();
    conn.execute(
        "DELETE FROM entities WHERE id = ?1 AND user_id = ?2",
        libsql::params![id, auth.user_id],
    )
    .await
    .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
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
    let conn = state.db.connection();
    let mut rows = if let Some(relationship_type) = params.relationship_type {
        conn.query(
            "SELECT er.id, er.source_entity_id, er.target_entity_id, er.relationship_type, \
             er.strength, er.evidence_count, er.created_at \
             FROM entity_relationships er \
             WHERE (er.source_entity_id = ?1 OR er.target_entity_id = ?1) \
               AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?2) \
               AND er.relationship_type = ?3",
            libsql::params![id, auth.user_id, relationship_type],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    } else {
        conn.query(
            "SELECT er.id, er.source_entity_id, er.target_entity_id, er.relationship_type, \
             er.strength, er.evidence_count, er.created_at \
             FROM entity_relationships er \
             WHERE (er.source_entity_id = ?1 OR er.target_entity_id = ?1) \
             AND EXISTS (SELECT 1 FROM entities WHERE id = ?1 AND user_id = ?2)",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    };

    let mut relationships: Vec<Value> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        relationships.push(row_to_relationship_json(&row)?);
    }

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
    let conn = state.db.connection();
    let mut rows = conn
        .query(
            "SELECT me.memory_id FROM memory_entities me \
             JOIN entities e ON e.id = me.entity_id \
             WHERE me.entity_id = ?1 AND e.user_id = ?2",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        let mid: i64 = row
            .get(0)
            .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
        memory_ids.push(mid);
    }

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
        body.limit.unwrap_or(20),
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
    let conn = state.db.connection();
    // Verify both entities belong to the authenticated user
    let mut check = conn
        .query(
            "SELECT COUNT(*) FROM entities WHERE id IN (?1, ?2) AND user_id = ?3",
            libsql::params![req.source_entity_id, req.target_entity_id, auth.user_id],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let count: i64 = check
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);
    if count < 2 {
        return Err(AppError(engram_lib::EngError::NotFound(
            "one or both entities not found".into(),
        )));
    }
    let rel_type = req.relationship_type.as_deref().unwrap_or("related");
    let strength = req.strength.unwrap_or(1.0);

    conn.execute(
        "INSERT INTO entity_relationships \
         (source_entity_id, target_entity_id, relationship_type, strength) \
         VALUES (?1, ?2, ?3, ?4)",
        libsql::params![
            req.source_entity_id,
            req.target_entity_id,
            rel_type.to_string(),
            strength
        ],
    )
    .await
    .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    let mut rows = conn
        .query(
            "SELECT id, source_entity_id, target_entity_id, relationship_type, \
             strength, evidence_count, created_at \
             FROM entity_relationships WHERE rowid = last_insert_rowid()",
            (),
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        Ok((StatusCode::CREATED, Json(row_to_relationship_json(&row)?)))
    } else {
        Err(AppError(engram_lib::EngError::Internal(
            "failed to fetch created relationship".into(),
        )))
    }
}

// ---------------------------------------------------------------------------
// GET /graph
// ---------------------------------------------------------------------------

async fn graph_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = GraphBuildOptions {
        user_id: auth.user_id,
        limit: Some(params.limit.unwrap_or(500) as usize),
    };
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    Ok(Json(json!(result)))
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
        limit: Some(params.limit.unwrap_or(500) as usize),
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
        limit: Some(params.limit.unwrap_or(500) as usize),
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
    let result = build_graph_data(&state.db, &opts).await.map_err(AppError)?;
    Ok(Json(json!(result)))
}

// ---------------------------------------------------------------------------
// POST /graph/search
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GraphSearchBody {
    pub query: String,
    pub limit: Option<usize>,
}

async fn graph_search_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GraphSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20);
    let nodes = graph_search(&state.db, &body.query, limit, auth.user_id).await?;
    Ok(Json(json!({ "nodes": nodes })))
}

// ---------------------------------------------------------------------------
// GET /graph/neighborhood/{id}
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct NeighborhoodQuery {
    pub depth: Option<u32>,
}

async fn neighborhood_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
    Query(params): Query<NeighborhoodQuery>,
) -> Result<Json<Value>, AppError> {
    let depth = params.depth.unwrap_or(2);
    let (nodes, edges) = neighborhood(&state.db, &id, depth, auth.user_id).await?;
    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

// ---------------------------------------------------------------------------
// GET /memory/{id}/entities
// ---------------------------------------------------------------------------

async fn memory_entities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let conn = state.db.connection();
    let mut rows = conn
        .query(
            "SELECT e.id, e.name, e.entity_type, me.salience \
             FROM memory_entities me \
             JOIN entities e ON e.id = me.entity_id \
             JOIN memories m ON m.id = me.memory_id \
             WHERE me.memory_id = ?1 AND m.user_id = ?2",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

    let mut entities: Vec<Value> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?
    {
        let eid: i64 = row
            .get(0)
            .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
        let name: String = row
            .get(1)
            .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
        let entity_type: String = row
            .get(2)
            .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
        let salience: f64 = row
            .get(3)
            .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
        entities.push(json!({
            "id": eid,
            "name": name,
            "entity_type": entity_type,
            "salience": salience,
        }));
    }

    Ok(Json(json!({ "entities": entities })))
}

// ---------------------------------------------------------------------------
// GET /communities
// ---------------------------------------------------------------------------

async fn communities_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_community_stats(&state.db, auth.user_id)
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "communities": stats })))
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
    let limit = params.limit.unwrap_or(50) as usize;
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
    let limit = params.limit.unwrap_or(20) as usize;
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
        params.limit.unwrap_or(50),
    )
    .await
    .map_err(AppError)?;
    Ok(Json(json!({ "facts": facts })))
}

// ---------------------------------------------------------------------------
// Helpers -- row mapping
// ---------------------------------------------------------------------------

fn row_to_entity_json(row: &libsql::Row) -> Result<Value, AppError> {
    let id: i64 = row
        .get(0)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let name: String = row
        .get(1)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let entity_type: String = row
        .get(2)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let description: Option<String> = row
        .get(3)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let aliases_raw: Option<String> = row
        .get(4)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let user_id: i64 = row
        .get(5)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let space_id: Option<i64> = row
        .get(6)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let confidence: f64 = row
        .get(7)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let occurrence_count: i32 = row
        .get(8)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let first_seen_at: String = row
        .get(9)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let last_seen_at: String = row
        .get(10)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let created_at: String = row
        .get(11)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

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

fn row_to_relationship_json(row: &libsql::Row) -> Result<Value, AppError> {
    let id: i64 = row
        .get(0)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let source_entity_id: i64 = row
        .get(1)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let target_entity_id: i64 = row
        .get(2)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let relationship_type: String = row
        .get(3)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let strength: f64 = row
        .get(4)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let evidence_count: i32 = row
        .get(5)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;
    let created_at: String = row
        .get(6)
        .map_err(|e| AppError(engram_lib::EngError::Database(e)))?;

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
