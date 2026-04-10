// Ingestion routes -- ported from ingestion/routes.ts

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use engram_lib::ingestion::{
    self,
    types::{FormatMeta, IngestMode, IngestOptions, SupportedFormat},
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/import/bulk", post(import_bulk))
        .route("/import/json", post(import_json))
        .route("/import/mem0", post(import_mem0))
        .route("/import/supermemory", post(import_supermemory))
        .route("/ingest", post(ingest_text))
        .route("/add", post(add_conversation))
        .route("/derive", post(derive))
}

#[derive(Debug, Deserialize)]
struct ImportBulkBody {
    text: Option<String>,
    url: Option<String>,
    format: Option<String>,
    mode: Option<String>,
    source: Option<String>,
    category: Option<String>,
    project_id: Option<i64>,
    episode_id: Option<i64>,
}

async fn import_bulk(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ImportBulkBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if body.text.is_none() && body.url.is_none() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Provide text or url parameter".to_string(),
        )));
    }
    let input = if let Some(ref text) = body.text {
        if text.trim().is_empty() {
            return Err(AppError(engram_lib::EngError::InvalidInput(
                "text must be a non-empty string".to_string(),
            )));
        }
        text.clone()
    } else {
        return Err(AppError(engram_lib::EngError::NotImplemented(
            "URL fetching not yet implemented".to_string(),
        )));
    };
    let format: Option<SupportedFormat> = body.format.as_ref().and_then(|f| f.parse().ok());
    let mode = body
        .mode
        .as_ref()
        .map(|m| {
            if m == "extract" {
                IngestMode::Extract
            } else {
                IngestMode::Raw
            }
        })
        .unwrap_or(IngestMode::Extract);
    let options = IngestOptions {
        mode,
        format,
        source: body.source.unwrap_or_else(|| "import".to_string()),
        category: body.category.unwrap_or_else(|| "general".to_string()),
        user_id: auth.user_id,
        space_id: None,
        project_id: body.project_id,
        episode_id: body.episode_id,
        entity_ids: None,
        chunker_options: None,
    };
    let meta = FormatMeta {
        extension: None,
        mime: None,
    };
    let result = ingestion::ingest(&state.db, &input, options, Some(&meta)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "job_id": result.job_id, "status": result.status,
            "total_documents": result.total_documents, "total_chunks": result.total_chunks,
            "total_memories": result.total_memories, "errors": result.errors,
            "duration_ms": result.duration_ms,
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct ImportJsonBody {
    version: Option<String>,
    memories: Option<Vec<ImportMemory>>,
}

#[derive(Debug, Deserialize)]
struct ImportMemory {
    content: Option<String>,
    category: Option<String>,
    source: Option<String>,
    session_id: Option<String>,
    importance: Option<i32>,
    tags: Option<serde_json::Value>,
    confidence: Option<f64>,
    is_static: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

async fn import_json(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ImportJsonBody>,
) -> Result<Json<Value>, AppError> {
    if body.version.is_none() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Invalid export format: missing version field".to_string(),
        )));
    }
    let memories = body.memories.unwrap_or_default();
    if memories.is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Invalid export format: missing memories array".to_string(),
        )));
    }
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for m in &memories {
        let content = match &m.content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let tags_str = match &m.tags {
            Some(serde_json::Value::Array(arr)) => {
                Some(serde_json::to_string(arr).unwrap_or_default())
            }
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let sync_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let created_at = m.created_at.as_deref().unwrap_or(&now);
        let updated_at = m.updated_at.as_deref().unwrap_or(&now);
        match state.db.conn.execute(
            "INSERT INTO memories (content, category, source, session_id, importance, tags, confidence, is_static, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            libsql::params![
                content,
                m.category.clone().unwrap_or_else(|| "general".to_string()),
                m.source.clone().unwrap_or_else(|| "import".to_string()),
                m.session_id.clone(), m.importance.unwrap_or(5), tags_str.clone(),
                m.confidence.unwrap_or(1.0),
                if m.is_static.unwrap_or(false) { 1 } else { 0 },
                auth.user_id, sync_id, created_at.to_string(), updated_at.to_string()
            ],
        ).await {
            Ok(_) => imported += 1,
            Err(e) => { tracing::warn!("import_memory_failed: {}", e); skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": { "memories": imported, "skipped": skipped } }),
    ))
}

async fn import_mem0(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let memories = body
        .get("memories")
        .or_else(|| body.get("results"))
        .cloned()
        .unwrap_or(body.clone());
    let arr = memories.as_array().ok_or_else(|| {
        AppError(engram_lib::EngError::InvalidInput(
            "Expected array of mem0 memories".to_string(),
        ))
    })?;
    let mut imported = 0i64;
    for mem in arr {
        let content = mem
            .get("memory")
            .or_else(|| mem.get("text"))
            .or_else(|| mem.get("content"))
            .and_then(|v| v.as_str());
        let content = match content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => continue,
        };
        let meta_obj = mem.get("metadata").and_then(|m| m.as_object());
        let category = meta_obj
            .and_then(|m| m.get("category"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("category").and_then(|v| v.as_str()))
            .unwrap_or("general");
        let source = meta_obj
            .and_then(|m| m.get("source"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("source").and_then(|v| v.as_str()))
            .unwrap_or("mem0-import");
        let importance = meta_obj
            .and_then(|m| m.get("importance"))
            .and_then(|v| v.as_i64())
            .unwrap_or(5) as i32;
        let tags = meta_obj
            .and_then(|m| m.get("tags"))
            .cloned()
            .unwrap_or_else(|| json!(["mem0-import"]));
        let tags_str = serde_json::to_string(&tags).unwrap_or_default();
        let sync_id = Uuid::new_v4().to_string();
        let _ = state.db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, tags, confidence, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, ?7, datetime('now'), datetime('now'))",
            libsql::params![content, category.to_string(), source.to_string(), importance, tags_str, auth.user_id, sync_id],
        ).await.map(|_| imported += 1);
    }
    Ok(Json(json!({ "imported": imported, "source": "mem0" })))
}

async fn import_supermemory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let items = body
        .get("documents")
        .or_else(|| body.get("memories"))
        .or_else(|| body.get("data"))
        .cloned()
        .or_else(|| {
            if body.is_array() {
                Some(body.clone())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            AppError(engram_lib::EngError::InvalidInput(
                "Expected documents/memories array".to_string(),
            ))
        })?;
    let arr = items.as_array().ok_or_else(|| {
        AppError(engram_lib::EngError::InvalidInput(
            "Expected array".to_string(),
        ))
    })?;
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for item in arr {
        let content = item
            .get("content")
            .or_else(|| item.get("text"))
            .or_else(|| item.get("description"))
            .or_else(|| item.get("raw"))
            .and_then(|v| v.as_str());
        let content = match content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| match item_type.to_lowercase().as_str() {
                "note" => "general",
                "tweet" | "page" | "bookmark" => "discovery",
                "document" => "task",
                "conversation" => "state",
                _ => "general",
            });
        let mut tags: Vec<String> = vec!["supermemory-import".to_string()];
        if let Some(spaces) = item.get("spaces").and_then(|v| v.as_array()) {
            for s in spaces {
                if let Some(sv) = s.as_str() {
                    tags.push(sv.to_lowercase());
                }
            }
        } else if let Some(space) = item.get("space").and_then(|v| v.as_str()) {
            tags.push(space.to_lowercase());
        }
        if let Some(item_tags) = item.get("tags").and_then(|v| v.as_array()) {
            for t in item_tags {
                if let Some(tv) = t.as_str() {
                    tags.push(tv.to_lowercase());
                }
            }
        }
        if !item_type.is_empty() {
            tags.push(item_type.to_lowercase());
        }
        tags.dedup();
        let tags_str = serde_json::to_string(&tags).unwrap_or_default();
        let importance = item
            .get("importance")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                item.get("metadata")
                    .and_then(|m| m.get("importance"))
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(5) as i32;
        let source = item
            .get("source")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.get("metadata")
                    .and_then(|m| m.get("source"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("supermemory-import");
        let sync_id = Uuid::new_v4().to_string();
        match state.db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, tags, confidence, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, ?7, datetime('now'), datetime('now'))",
            libsql::params![content, category.to_string(), source.to_string(), importance, tags_str, auth.user_id, sync_id],
        ).await {
            Ok(_) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": imported, "skipped": skipped, "source": "supermemory" }),
    ))
}

#[derive(Debug, Deserialize)]
struct IngestBody {
    url: Option<String>,
    text: Option<String>,
    title: Option<String>,
    source: Option<String>,
    entity_ids: Option<Vec<i64>>,
    #[allow(dead_code)]
    project_ids: Option<Vec<i64>>,
    episode_id: Option<i64>,
}

async fn ingest_text(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<IngestBody>,
) -> Result<Json<Value>, AppError> {
    if body.url.is_none() && body.text.is_none() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Provide url or text parameter".to_string(),
        )));
    }
    let raw_text;
    let ingest_source;
    let title;
    if let Some(ref text) = body.text {
        if text.trim().is_empty() {
            return Err(AppError(engram_lib::EngError::InvalidInput(
                "text must be a non-empty string".to_string(),
            )));
        }
        raw_text = text.trim().to_string();
        title = body.title.unwrap_or_else(|| {
            let t: String = raw_text.chars().take(60).collect();
            t.replace(char::from(10u8), " ")
        });
        ingest_source = body.source.unwrap_or_else(|| "text".to_string());
    } else {
        return Err(AppError(engram_lib::EngError::Internal(
            "URL fetching not yet implemented in Rust port".to_string(),
        )));
    }
    let options = IngestOptions {
        mode: IngestMode::Raw,
        format: None,
        source: ingest_source.clone(),
        category: "general".to_string(),
        user_id: auth.user_id,
        space_id: None,
        project_id: None,
        episode_id: body.episode_id,
        entity_ids: body.entity_ids.clone(),
        chunker_options: None,
    };
    let result = ingestion::ingest(&state.db, &raw_text, options, None).await?;
    Ok(Json(json!({
        "ingested": result.total_memories, "source": ingest_source, "title": title,
        "chunks_processed": result.total_chunks, "errors": result.errors,
    })))
}

async fn add_conversation(
    _state: State<AppState>,
    Auth(_auth): Auth,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Err(AppError(engram_lib::EngError::NotImplemented(
        "/add requires LLM-based fact extraction (not yet implemented)".to_string(),
    )))
}

async fn derive(
    _state: State<AppState>,
    Auth(_auth): Auth,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Err(AppError(engram_lib::EngError::NotImplemented(
        "/derive requires LLM-based inference (not yet implemented)".to_string(),
    )))
}
