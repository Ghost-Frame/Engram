use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

// --- Request / response types ---

/// Query parameters for the list endpoint.
#[derive(Debug, Deserialize)]
struct ListQuery {
    /// When true, include disabled configs in the response.
    #[serde(default)]
    all: Option<bool>,
}

/// Returns the default HTTP method for new dispatch configs.
fn default_method() -> String {
    "POST".to_string()
}

/// HTTP methods a dispatch config may use. Unknown methods are rejected at
/// write time so a typo cannot persist a config the dispatcher can never
/// execute (the outbound call would otherwise fail only at dispatch time).
const ALLOWED_DISPATCH_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Validate a dispatch config method against the allowlist and normalize it
/// to uppercase so stored values compare exactly at dispatch time.
fn validate_method(method: &str) -> Result<String, AppError> {
    let normalized = method.trim().to_uppercase();
    if !ALLOWED_DISPATCH_METHODS.contains(&normalized.as_str()) {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "invalid method '{}'; must be one of {:?}",
            method, ALLOWED_DISPATCH_METHODS
        ))));
    }
    Ok(normalized)
}

/// Require a schema value to be a JSON object before persisting it. Full
/// JSON-Schema meta validation would pull in a new dependency for an
/// admin-gated write path; the object-shape check catches the practical
/// corruption (arrays or scalars stored where the dispatcher expects a
/// schema object).
fn require_schema_object(value: &serde_json::Value, field: &str) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "{field} must be a JSON object"
        ))));
    }
    Ok(())
}

/// Returns the default target type for new dispatch configs.
fn default_target_type() -> String {
    "internal".to_string()
}

/// Returns true: new dispatch configs are enabled by default.
fn default_enabled() -> bool {
    true
}

/// Request body for creating a new dispatch config.
#[derive(Debug, Deserialize)]
struct CreateConfigBody {
    /// Unique skill name that this config maps to.
    skill_name: String,
    /// Human-readable description of what the skill does.
    description: Option<String>,
    /// The endpoint path or URL the dispatcher will call.
    endpoint: String,
    /// HTTP method (defaults to "POST").
    #[serde(default = "default_method")]
    method: String,
    /// Target type: "internal" or "external" (defaults to "internal").
    #[serde(default = "default_target_type")]
    target_type: String,
    /// JSON Schema describing accepted input parameters.
    #[serde(default)]
    params_schema: Option<Value>,
    /// Hints for extracting structured output from the response.
    #[serde(default)]
    output_hints: Option<Value>,
    /// Whether the config is active (defaults to true).
    #[serde(default = "default_enabled")]
    enabled: bool,
}

/// Request body for updating an existing dispatch config.
#[derive(Debug, Deserialize)]
struct UpdateConfigBody {
    /// Updated description.
    description: Option<String>,
    /// Updated endpoint.
    endpoint: Option<String>,
    /// Updated HTTP method.
    method: Option<String>,
    /// Updated target type.
    target_type: Option<String>,
    /// Updated params schema.
    params_schema: Option<Value>,
    /// Updated output hints.
    output_hints: Option<Value>,
    /// Updated enabled flag.
    enabled: Option<bool>,
}

// --- Router ---

/// Register all `/dispatch/configs` routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dispatch/configs", get(list_configs).post(create_config))
        .route(
            "/dispatch/configs/{skill_name}",
            get(get_config).put(update_config).delete(delete_config),
        )
}

// --- Helpers ---

/// Parse a TEXT column that contains a JSON string into a `Value`.
/// Falls back to `Value::Null` on parse failure rather than erroring.
fn parse_json_column(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

// --- Handlers ---

/// List all dispatch configs.
///
/// By default only enabled configs are returned. Pass `?all=true` to include
/// disabled ones.
#[tracing::instrument(skip_all)]
async fn list_configs(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let include_all = query.all.unwrap_or(false);

    let configs: Vec<Value> = state
        .db
        .read(move |conn| {
            let sql = if include_all {
                "SELECT id, skill_name, description, enabled, target_type, endpoint, method, \
                 params_schema, output_hints, created_at, updated_at \
                 FROM skill_dispatch_configs \
                 ORDER BY skill_name ASC"
            } else {
                "SELECT id, skill_name, description, enabled, target_type, endpoint, method, \
                 params_schema, output_hints, created_at, updated_at \
                 FROM skill_dispatch_configs \
                 WHERE enabled = 1 \
                 ORDER BY skill_name ASC"
            };

            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| {
                let id: i64 = r.get(0)?;
                let skill_name: String = r.get(1)?;
                let description: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                let enabled: bool = r.get::<_, Option<i64>>(3)?.unwrap_or(1) != 0;
                let target_type: String = r.get::<_, Option<String>>(4)?.unwrap_or_default();
                let endpoint: String = r.get::<_, Option<String>>(5)?.unwrap_or_default();
                let method: String = r.get::<_, Option<String>>(6)?.unwrap_or_default();
                let params_schema_raw: String = r
                    .get::<_, Option<String>>(7)?
                    .unwrap_or_else(|| "{}".to_string());
                let output_hints_raw: String = r
                    .get::<_, Option<String>>(8)?
                    .unwrap_or_else(|| "{}".to_string());
                let created_at: String = r.get::<_, Option<String>>(9)?.unwrap_or_default();
                let updated_at: String = r.get::<_, Option<String>>(10)?.unwrap_or_default();
                Ok((
                    id,
                    skill_name,
                    description,
                    enabled,
                    target_type,
                    endpoint,
                    method,
                    params_schema_raw,
                    output_hints_raw,
                    created_at,
                    updated_at,
                ))
            })?;

            let mut configs = Vec::new();
            for row in rows {
                let (
                    id,
                    skill_name,
                    description,
                    enabled,
                    target_type,
                    endpoint,
                    method,
                    params_schema_raw,
                    output_hints_raw,
                    created_at,
                    updated_at,
                ) = row?;
                configs.push(json!({
                    "id": id,
                    "skill_name": skill_name,
                    "description": description,
                    "enabled": enabled,
                    "target_type": target_type,
                    "endpoint": endpoint,
                    "method": method,
                    "params_schema": parse_json_column(&params_schema_raw),
                    "output_hints": parse_json_column(&output_hints_raw),
                    "created_at": created_at,
                    "updated_at": updated_at,
                }));
            }
            Ok(configs)
        })
        .await?;

    let count = configs.len();
    Ok(Json(json!({ "configs": configs, "count": count })))
}

/// Get a single dispatch config by skill name.
///
/// Returns 404 if no config exists for that skill name.
#[tracing::instrument(skip_all, fields(skill_name = %skill_name))]
async fn get_config(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(skill_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let config: Option<Value> = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, skill_name, description, enabled, target_type, endpoint, method, \
                 params_schema, output_hints, created_at, updated_at \
                 FROM skill_dispatch_configs \
                 WHERE skill_name = ?1",
            )?;

            let mut rows = stmt.query_map(params![skill_name], |r| {
                let id: i64 = r.get(0)?;
                let skill_name: String = r.get(1)?;
                let description: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                let enabled: bool = r.get::<_, Option<i64>>(3)?.unwrap_or(1) != 0;
                let target_type: String = r.get::<_, Option<String>>(4)?.unwrap_or_default();
                let endpoint: String = r.get::<_, Option<String>>(5)?.unwrap_or_default();
                let method: String = r.get::<_, Option<String>>(6)?.unwrap_or_default();
                let params_schema_raw: String = r
                    .get::<_, Option<String>>(7)?
                    .unwrap_or_else(|| "{}".to_string());
                let output_hints_raw: String = r
                    .get::<_, Option<String>>(8)?
                    .unwrap_or_else(|| "{}".to_string());
                let created_at: String = r.get::<_, Option<String>>(9)?.unwrap_or_default();
                let updated_at: String = r.get::<_, Option<String>>(10)?.unwrap_or_default();
                Ok((
                    id,
                    skill_name,
                    description,
                    enabled,
                    target_type,
                    endpoint,
                    method,
                    params_schema_raw,
                    output_hints_raw,
                    created_at,
                    updated_at,
                ))
            })?;

            if let Some(row) = rows.next() {
                let (
                    id,
                    skill_name,
                    description,
                    enabled,
                    target_type,
                    endpoint,
                    method,
                    params_schema_raw,
                    output_hints_raw,
                    created_at,
                    updated_at,
                ) = row?;
                Ok(Some(json!({
                    "id": id,
                    "skill_name": skill_name,
                    "description": description,
                    "enabled": enabled,
                    "target_type": target_type,
                    "endpoint": endpoint,
                    "method": method,
                    "params_schema": parse_json_column(&params_schema_raw),
                    "output_hints": parse_json_column(&output_hints_raw),
                    "created_at": created_at,
                    "updated_at": updated_at,
                })))
            } else {
                Ok(None)
            }
        })
        .await?;

    match config {
        Some(c) => Ok(Json(c)),
        None => Err(AppError(kleos_lib::EngError::NotFound(
            "dispatch config not found".into(),
        ))),
    }
}

/// Create a new dispatch config. Requires admin scope.
///
/// Returns 201 with `{ "id": N, "skill_name": "..." }` on success.
#[tracing::instrument(skip_all)]
async fn create_config(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateConfigBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !auth.has_scope(&kleos_lib::auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }

    let skill_name = body.skill_name.trim().to_string();
    if skill_name.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "skill_name is required".into(),
        )));
    }

    let endpoint = body.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "endpoint is required".into(),
        )));
    }

    let description = body.description.unwrap_or_default();
    let method = validate_method(&body.method)?;
    let target_type = body.target_type;
    let params_schema = body.params_schema.unwrap_or(json!({}));
    require_schema_object(&params_schema, "params_schema")?;
    let params_schema_str = serde_json::to_string(&params_schema)
        .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
    let output_hints_str = serde_json::to_string(&body.output_hints.unwrap_or(json!({})))
        .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
    let enabled = body.enabled;

    // Clone skill_name so we can use it in both the closure and the response.
    let skill_name_for_resp = skill_name.clone();

    let row_id = state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO skill_dispatch_configs \
                 (skill_name, description, enabled, target_type, endpoint, method, params_schema, output_hints) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    skill_name,
                    description,
                    enabled as i64,
                    target_type,
                    endpoint,
                    method,
                    params_schema_str,
                    output_hints_str,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": row_id, "skill_name": skill_name_for_resp })),
    ))
}

/// Update an existing dispatch config by skill name. Requires admin scope.
///
/// Partial updates are supported: only provided fields are changed.
/// Returns 404 if no config exists for that skill name.
#[tracing::instrument(skip_all, fields(skill_name = %skill_name))]
async fn update_config(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(skill_name): Path<String>,
    Json(body): Json<UpdateConfigBody>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&kleos_lib::auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }

    // Build a dynamic SET clause from whichever fields were provided.
    // We collect into Vec<rusqlite::types::Value> because that type implements ToSql
    // and is safe to pass to params_from_iter.
    let mut set_parts: Vec<String> = Vec::new();
    let mut bound_values: Vec<rusqlite::types::Value> = Vec::new();

    let mut idx = 1usize;

    if let Some(desc) = body.description {
        set_parts.push(format!("description = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(desc));
        idx += 1;
    }
    if let Some(ep) = body.endpoint {
        set_parts.push(format!("endpoint = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(ep));
        idx += 1;
    }
    if let Some(m) = body.method {
        let m = validate_method(&m)?;
        set_parts.push(format!("method = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(m));
        idx += 1;
    }
    if let Some(tt) = body.target_type {
        set_parts.push(format!("target_type = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(tt));
        idx += 1;
    }
    if let Some(ps) = body.params_schema {
        require_schema_object(&ps, "params_schema")?;
        let s = serde_json::to_string(&ps)
            .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
        set_parts.push(format!("params_schema = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(s));
        idx += 1;
    }
    if let Some(oh) = body.output_hints {
        let s = serde_json::to_string(&oh)
            .map_err(|e| AppError(kleos_lib::EngError::Serialization(e)))?;
        set_parts.push(format!("output_hints = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Text(s));
        idx += 1;
    }
    if let Some(en) = body.enabled {
        set_parts.push(format!("enabled = ?{}", idx));
        bound_values.push(rusqlite::types::Value::Integer(en as i64));
        idx += 1;
    }

    if set_parts.is_empty() {
        // Nothing to update is valid -- return success immediately.
        return Ok(Json(json!({ "updated": true })));
    }

    // Always bump updated_at.
    set_parts.push("updated_at = datetime('now')".to_string());

    let sql = format!(
        "UPDATE skill_dispatch_configs SET {} WHERE skill_name = ?{}",
        set_parts.join(", "),
        idx
    );
    bound_values.push(rusqlite::types::Value::Text(skill_name.clone()));

    let rows_affected = state
        .db
        .write(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let n = stmt.execute(rusqlite::params_from_iter(bound_values))?;
            Ok(n)
        })
        .await?;

    if rows_affected == 0 {
        return Err(AppError(kleos_lib::EngError::NotFound(format!(
            "dispatch config '{}' not found",
            skill_name
        ))));
    }

    Ok(Json(json!({ "updated": true })))
}

/// Delete a dispatch config by skill name. Requires admin scope.
///
/// Returns 404 if no config exists for that skill name.
#[tracing::instrument(skip_all, fields(skill_name = %skill_name))]
async fn delete_config(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(skill_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&kleos_lib::auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }

    let skill_name_for_err = skill_name.clone();
    let rows_affected = state
        .db
        .write(move |conn| {
            let n = conn.execute(
                "DELETE FROM skill_dispatch_configs WHERE skill_name = ?1",
                params![skill_name],
            )?;
            Ok(n)
        })
        .await?;

    if rows_affected == 0 {
        return Err(AppError(kleos_lib::EngError::NotFound(format!(
            "dispatch config '{}' not found",
            skill_name_for_err
        ))));
    }

    Ok(Json(json!({ "deleted": true })))
}
