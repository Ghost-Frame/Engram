// Portability routes: export, import (auto-detect), state, preferences

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export", get(export_handler))
        .route("/import", axum::routing::post(import_handler))
        // NOTE: /import/mem0 is in ingestion.rs to avoid duplicate routes
        .route("/state", get(get_state_handler).delete(delete_state_handler))
        .route("/preferences", get(list_preferences_handler).put(put_preferences_handler).delete(delete_all_preferences_handler))
        .route("/preferences/{key}", get(get_preference_handler).delete(delete_preference_handler))
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

async fn export_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let data = engram_lib::admin::export_user_data(&state.db, auth.user_id).await?;
    Ok(Json(serde_json::to_value(data).map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?))
}

// ---------------------------------------------------------------------------
// Import (auto-detect format)
// ---------------------------------------------------------------------------

async fn import_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    // Auto-detect format based on shape
    if body.is_array() {
        return import_array(&state, auth.user_id, body.as_array().unwrap()).await;
    }
    if let Some(obj) = body.as_object() {
        if obj.contains_key("memories") {
            // Engram JSON export or generic format with memories key
            let version = obj.get("version").and_then(|v| v.as_str());
            if version.is_some() {
                return import_engram_export(&state, auth.user_id, obj).await;
            }
            // mem0-style: has "memories" but no version
            if let Some(arr) = obj.get("memories").and_then(|v| v.as_array()) {
                return import_mem0_array(&state, auth.user_id, arr).await;
            }
        }
        if obj.contains_key("results") {
            if let Some(arr) = obj.get("results").and_then(|v| v.as_array()) {
                return import_mem0_array(&state, auth.user_id, arr).await;
            }
        }
        if obj.contains_key("documents") || obj.contains_key("data") {
            let items = obj.get("documents").or_else(|| obj.get("data"))
                .and_then(|v| v.as_array());
            if let Some(arr) = items {
                return import_array(&state, auth.user_id, arr).await;
            }
        }
    }
    Err(AppError(engram_lib::EngError::InvalidInput("unrecognized import format".into())))
}

async fn import_engram_export(
    state: &AppState,
    user_id: i64,
    obj: &serde_json::Map<String, Value>,
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    if let Some(memories) = obj.get("memories").and_then(|v| v.as_array()) {
        for mem in memories {
            let content = mem.get("content")
                .or_else(|| mem.get("col_1"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string());
            let content = match content.filter(|c| !c.is_empty()) {
                Some(c) => c,
                None => { skipped += 1; continue; }
            };
            let category = mem.get("category").or_else(|| mem.get("col_2"))
                .and_then(|v| v.as_str()).unwrap_or("general").to_string();
            let source = mem.get("source").or_else(|| mem.get("col_3"))
                .and_then(|v| v.as_str()).unwrap_or("import").to_string();
            let importance = mem.get("importance").or_else(|| mem.get("col_4"))
                .and_then(|v| v.as_i64()).unwrap_or(5) as i32;
            let sync_id = Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let created_at = mem.get("created_at").and_then(|v| v.as_str()).unwrap_or(&now).to_string();
            let updated_at = mem.get("updated_at").and_then(|v| v.as_str()).unwrap_or(&now).to_string();
            match state.db.conn.execute(
                "INSERT INTO memories (content, category, source, importance, user_id, sync_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                libsql::params![content, category, source, importance, user_id, sync_id, created_at, updated_at],
            ).await {
                Ok(_) => imported += 1,
                Err(e) => { tracing::warn!("import_engram_memory_failed: {}", e); skipped += 1; }
            }
        }
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped, "format": "engram" })))
}

async fn import_array(
    state: &AppState,
    user_id: i64,
    arr: &[Value],
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for item in arr {
        let content = item.get("content").or_else(|| item.get("text")).or_else(|| item.get("memory"))
            .and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        let content = match content.filter(|c| !c.is_empty()) {
            Some(c) => c,
            None => { skipped += 1; continue; }
        };
        let category = item.get("category").and_then(|v| v.as_str()).unwrap_or("general").to_string();
        let source = item.get("source").and_then(|v| v.as_str()).unwrap_or("import").to_string();
        let importance = item.get("importance").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
        let sync_id = Uuid::new_v4().to_string();
        match state.db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, user_id, sync_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))",
            libsql::params![content, category, source, importance, user_id, sync_id],
        ).await {
            Ok(_) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped, "format": "array" })))
}

async fn import_mem0_array(
    state: &AppState,
    user_id: i64,
    arr: &[Value],
) -> Result<Json<Value>, AppError> {
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for mem in arr {
        let content = mem.get("memory").or_else(|| mem.get("text")).or_else(|| mem.get("content"))
            .and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        let content = match content.filter(|c| !c.is_empty()) {
            Some(c) => c,
            None => { skipped += 1; continue; }
        };
        let meta = mem.get("metadata").and_then(|m| m.as_object());
        let category = meta.and_then(|m| m.get("category")).and_then(|v| v.as_str())
            .or_else(|| mem.get("category").and_then(|v| v.as_str())).unwrap_or("general").to_string();
        let source = meta.and_then(|m| m.get("source")).and_then(|v| v.as_str())
            .or_else(|| mem.get("source").and_then(|v| v.as_str())).unwrap_or("mem0-import").to_string();
        let importance = meta.and_then(|m| m.get("importance")).and_then(|v| v.as_i64())
            .unwrap_or(5) as i32;
        let sync_id = Uuid::new_v4().to_string();
        match state.db.conn.execute(
            "INSERT INTO memories (content, category, source, importance, user_id, sync_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))",
            libsql::params![content, category, source, importance, user_id, sync_id],
        ).await {
            Ok(_) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped, "format": "mem0" })))
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

async fn get_state_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let all = engram_lib::admin::list_state(&state.db).await?;
    let prefix = format!("user:{}:", auth.user_id);
    let user_state: serde_json::Map<String, Value> = all
        .into_iter()
        .filter(|r| r.key.starts_with(&prefix))
        .map(|r| {
            let k = r.key[prefix.len()..].to_string();
            (k, Value::String(r.value))
        })
        .collect();
    Ok(Json(json!({ "state": user_state })))
}

async fn delete_state_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let prefix = format!("user:{}:%", auth.user_id);
    let affected = state.db.conn.execute(
        "DELETE FROM app_state WHERE key LIKE ?1",
        libsql::params![prefix],
    ).await.map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))? as i64;
    Ok(Json(json!({ "deleted": affected })))
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

async fn list_preferences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let prefs = engram_lib::preferences::list_preferences(&state.db, auth.user_id).await?;
    Ok(Json(serde_json::to_value(prefs).map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?))
}

async fn get_preference_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pref = engram_lib::preferences::get_preference(&state.db, auth.user_id, &key).await?;
    Ok(Json(serde_json::to_value(pref).map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?))
}

async fn put_preferences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<serde_json::Map<String, Value>>,
) -> Result<Json<Value>, AppError> {
    let mut updated = 0i64;
    for (key, val) in &body {
        let v = val.as_str().map(|s| s.to_string()).unwrap_or_else(|| val.to_string());
        engram_lib::preferences::set_preference(&state.db, auth.user_id, key, &v).await?;
        updated += 1;
    }
    Ok(Json(json!({ "updated": updated })))
}

async fn delete_all_preferences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let deleted = engram_lib::preferences::delete_all_preferences(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "deleted": deleted })))
}

async fn delete_preference_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    engram_lib::preferences::delete_preference(&state.db, auth.user_id, &key).await?;
    Ok(Json(json!({ "deleted": true, "key": key })))
}
