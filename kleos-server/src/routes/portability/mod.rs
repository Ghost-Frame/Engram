// Portability routes: export, import (auto-detect), state, preferences

use axum::{
    body::Body, extract::Path, http::header, response::Response, routing::get, Json, Router,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};
use kleos_lib::db::Database;

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export", get(export_handler))
        .route("/import", axum::routing::post(import_handler))
        // NOTE: /import/mem0 is in ingestion.rs to avoid duplicate routes
        .route(
            "/state",
            get(get_state_handler).delete(delete_state_handler),
        )
        .route(
            "/preferences",
            get(list_preferences_handler)
                .put(put_preferences_handler)
                .delete(delete_all_preferences_handler),
        )
        .route(
            "/preferences/{key}",
            get(get_preference_handler).delete(delete_preference_handler),
        )
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

// DOS-L2: stream export as NDJSON so large user datasets don't require
// buffering the entire response as a single JSON blob. One JSON object per
// line; clients can parse records as they arrive.
async fn export_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Response, AppError> {
    let data = kleos_lib::admin::export_user_data(&db, auth.user_id).await?;

    let mut lines: Vec<Result<axum::body::Bytes, std::convert::Infallible>> = Vec::new();

    lines.push(Ok(axum::body::Bytes::from(
        json!({
            "type": "header",
            "version": data.version,
            "exported_at": data.exported_at,
            "user_id": data.user_id,
        })
        .to_string()
            + "\n",
    )));

    for (type_name, records) in [
        ("memory", &data.memories),
        ("conversation", &data.conversations),
        ("episode", &data.episodes),
        ("entity", &data.entities),
        ("fact", &data.facts),
        ("preference", &data.preferences),
        ("skill", &data.skills),
    ] {
        for record in records {
            let mut v = record.clone();
            if let Value::Object(ref mut map) = v {
                map.insert("type".into(), Value::String(type_name.to_string()));
            }
            lines.push(Ok(axum::body::Bytes::from(v.to_string() + "\n")));
        }
    }

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(futures::stream::iter(lines)))
        .unwrap())
}

// ---------------------------------------------------------------------------
// Import (auto-detect format)
// ---------------------------------------------------------------------------

async fn import_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    // Auto-detect format based on shape
    if body.is_array() {
        let arr = body.as_array().ok_or_else(|| {
            AppError(kleos_lib::EngError::InvalidInput(
                "expected JSON array".into(),
            ))
        })?;
        return import_array(&db, auth.user_id, arr).await;
    }
    if let Some(obj) = body.as_object() {
        if obj.contains_key("memories") {
            // Kleos JSON export or generic format with memories key
            let version = obj.get("version").and_then(|v| v.as_str());
            if version.is_some() {
                return import_kleos_export(&db, auth.user_id, obj).await;
            }
            // mem0-style: has "memories" but no version
            if let Some(arr) = obj.get("memories").and_then(|v| v.as_array()) {
                return import_mem0_array(&db, auth.user_id, arr).await;
            }
        }
        if obj.contains_key("results") {
            if let Some(arr) = obj.get("results").and_then(|v| v.as_array()) {
                return import_mem0_array(&db, auth.user_id, arr).await;
            }
        }
        if obj.contains_key("documents") || obj.contains_key("data") {
            let items = obj
                .get("documents")
                .or_else(|| obj.get("data"))
                .and_then(|v| v.as_array());
            if let Some(arr) = items {
                return import_array(&db, auth.user_id, arr).await;
            }
        }
    }
    Err(AppError(kleos_lib::EngError::InvalidInput(
        "unrecognized import format".into(),
    )))
}

async fn import_kleos_export(
    db: &Arc<Database>,
    _user_id: i64,
    obj: &serde_json::Map<String, Value>,
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    if let Some(memories) = obj.get("memories").and_then(|v| v.as_array()) {
        for mem in memories {
            let content = mem
                .get("content")
                .or_else(|| mem.get("col_1"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string());
            let content = match content.filter(|c| !c.is_empty()) {
                Some(c) => c,
                None => {
                    skipped += 1;
                    continue;
                }
            };
            let category = mem
                .get("category")
                .or_else(|| mem.get("col_2"))
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string();
            let source = mem
                .get("source")
                .or_else(|| mem.get("col_3"))
                .and_then(|v| v.as_str())
                .unwrap_or("import")
                .to_string();
            let importance = mem
                .get("importance")
                .or_else(|| mem.get("col_4"))
                .and_then(|v| v.as_i64())
                .unwrap_or(5) as i32;
            let sync_id = Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let created_at = mem
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or(&now)
                .to_string();
            let updated_at = mem
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or(&now)
                .to_string();
            match db.write(move |conn| {
                conn.execute(
                    "INSERT INTO memories (content, category, source, importance, sync_id, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![content, category, source, importance, sync_id, created_at, updated_at],
                ).map_err(|e| kleos_lib::EngError::Internal(e.to_string()))
            }).await {
                Ok(_) => imported += 1,
                Err(e) => { tracing::warn!("import_kleos_memory_failed: {}", e); skipped += 1; }
            }
        }
    }
    Ok(Json(
        json!({ "imported": imported, "skipped": skipped, "format": "kleos" }),
    ))
}

async fn import_array(
    db: &Arc<Database>,
    _user_id: i64,
    arr: &[Value],
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for item in arr {
        let content = item
            .get("content")
            .or_else(|| item.get("text"))
            .or_else(|| item.get("memory"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let content = match content.filter(|c| !c.is_empty()) {
            Some(c) => c,
            None => {
                skipped += 1;
                continue;
            }
        };
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("general")
            .to_string();
        let source = item
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("import")
            .to_string();
        let importance = item.get("importance").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
        let sync_id = Uuid::new_v4().to_string();
        match db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, sync_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))",
                params![content, category, source, importance, sync_id],
            ).map_err(|e| kleos_lib::EngError::Internal(e.to_string()))
        }).await {
            Ok(_) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": imported, "skipped": skipped, "format": "array" }),
    ))
}

async fn import_mem0_array(
    db: &Arc<Database>,
    _user_id: i64,
    arr: &[Value],
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for mem in arr {
        let content = mem
            .get("memory")
            .or_else(|| mem.get("text"))
            .or_else(|| mem.get("content"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let content = match content.filter(|c| !c.is_empty()) {
            Some(c) => c,
            None => {
                skipped += 1;
                continue;
            }
        };
        let meta = mem.get("metadata").and_then(|m| m.as_object());
        let category = meta
            .and_then(|m| m.get("category"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("category").and_then(|v| v.as_str()))
            .unwrap_or("general")
            .to_string();
        let source = meta
            .and_then(|m| m.get("source"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("source").and_then(|v| v.as_str()))
            .unwrap_or("mem0-import")
            .to_string();
        let importance = meta
            .and_then(|m| m.get("importance"))
            .and_then(|v| v.as_i64())
            .unwrap_or(5) as i32;
        let sync_id = Uuid::new_v4().to_string();
        match db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, sync_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))",
                params![content, category, source, importance, sync_id],
            ).map_err(|e| kleos_lib::EngError::Internal(e.to_string()))
        }).await {
            Ok(_) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": imported, "skipped": skipped, "format": "mem0" }),
    ))
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

async fn get_state_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    // SECURITY (SEC-LOW-8): push user filter into SQL instead of fetching all
    // rows and filtering in memory. Avoids leaking timing information about
    // total state table size and reduces unnecessary data transfer.
    let prefix = format!("user:{}:", auth.user_id);
    let prefix_like = format!("{}%", prefix);
    let prefix_len = prefix.len();
    let user_state: serde_json::Map<String, Value> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT key, value FROM app_state WHERE key LIKE ?1 ORDER BY key")
                .map_err(|e| kleos_lib::EngError::Internal(e.to_string()))?;
            let rows = stmt
                .query_map(params![prefix_like], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| kleos_lib::EngError::Internal(e.to_string()))?;
            let mut result = serde_json::Map::new();
            for row in rows {
                let (k, v) = row.map_err(|e| kleos_lib::EngError::Internal(e.to_string()))?;
                let short_key = k[prefix_len..].to_string();
                result.insert(short_key, Value::String(v));
            }
            Ok(result)
        })
        .await
        .map_err(AppError)?;
    Ok(Json(json!({ "state": user_state })))
}

async fn delete_state_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let prefix = format!("user:{}:%", auth.user_id);
    let affected = db
        .write(move |conn| {
            conn.execute("DELETE FROM app_state WHERE key LIKE ?1", params![prefix])
                .map_err(|e| kleos_lib::EngError::Internal(e.to_string()))
        })
        .await
        .map_err(AppError)? as i64;
    Ok(Json(json!({ "deleted": affected })))
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

async fn list_preferences_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let prefs = kleos_lib::preferences::list_preferences(&db, auth.user_id).await?;
    Ok(Json(serde_json::to_value(prefs).map_err(|e| {
        AppError(kleos_lib::EngError::Internal(e.to_string()))
    })?))
}

async fn get_preference_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pref = kleos_lib::preferences::get_preference(&db, auth.user_id, &key).await?;
    Ok(Json(serde_json::to_value(pref).map_err(|e| {
        AppError(kleos_lib::EngError::Internal(e.to_string()))
    })?))
}

async fn put_preferences_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<serde_json::Map<String, Value>>,
) -> Result<Json<Value>, AppError> {
    let mut updated = 0i64;
    for (key, val) in &body {
        let v = val
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| val.to_string());
        kleos_lib::preferences::set_preference(&db, auth.user_id, key, &v).await?;
        updated += 1;
    }
    Ok(Json(json!({ "updated": updated })))
}

async fn delete_all_preferences_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let deleted = kleos_lib::preferences::delete_all_preferences(&db, auth.user_id).await?;
    Ok(Json(json!({ "deleted": deleted })))
}

async fn delete_preference_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::preferences::delete_preference(&db, auth.user_id, &key).await?;
    Ok(Json(json!({ "deleted": true, "key": key })))
}
