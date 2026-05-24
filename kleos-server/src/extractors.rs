use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use kleos_lib::auth::AuthContext;
use kleos_lib::db::Database;
use kleos_lib::EngError;
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

/// Resolve the per-tenant `Database` for an arbitrary `user_id` outside of
/// a request-extraction context (cookie-auth GUI handlers, background
/// jobs, etc.). Mirrors the routing rules of [`ResolvedDb`]:
/// - sharding enabled returns the per-tenant shard for the given user.
/// - sharding disabled returns the shared monolith DB. Single-DB (shared)
///   mode is a first-class deployment: every data table carries `user_id`
///   and the query layer applies a `WHERE user_id = ?` predicate, so the
///   monolith isolates users by row just as shards isolate them by file.
///
/// M-R3-007: GUI handlers needed this so /gui/memory/* writes land in the
/// same shard as /memory/* when sharding is enabled.
pub async fn resolve_db_for_user(
    state: &AppState,
    user_id: i64,
) -> Result<Arc<Database>, EngError> {
    let Some(registry) = state.tenant_registry.as_ref() else {
        // Sharding disabled: serve the shared monolith. Row-level user_id
        // scoping provides isolation in this mode.
        return Ok(state.db.clone());
    };
    let handle = registry
        .get_or_create(&user_id.to_string())
        .await
        .map_err(|e| EngError::Internal(format!("tenant registry error: {}", e)))?;
    Ok(handle.database())
}

pub struct Auth(pub AuthContext);

impl<S: Send + Sync> FromRequestParts<S> for Auth {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(Auth)
            .ok_or((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Authentication required. Provide Bearer token." })),
            ));
        std::future::ready(result)
    }
}

/// Extractor that resolves the correct `Database` for the authenticated tenant.
///
/// When tenant sharding is enabled, every authenticated user (including the
/// operator, user_id=1) routes through their per-tenant shard via
/// `tenant_registry`; isolation is by file. When sharding is disabled
/// (`state.tenant_registry` is `None`), the extractor returns the shared
/// monolith DB: single-DB (shared) mode is a first-class deployment where
/// every data table carries `user_id` and the query layer applies a
/// `WHERE user_id = ?` predicate, so users are isolated by row instead of by
/// file. (The Phase-5 design dropped those predicates and made this path 503
/// fail-closed to avoid a BOLA; restoring the predicates makes shared mode
/// safe and usable again.)
pub struct ResolvedDb(pub Arc<Database>);

impl FromRequestParts<AppState> for ResolvedDb {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let auth = parts.extensions.get::<AuthContext>().cloned();
        let registry = state.tenant_registry.clone();
        let monolith_db = state.db.clone();

        async move {
            let auth = auth.ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "Authentication required." })),
                )
            })?;

            // Sharding disabled: serve the shared monolith. Row-level user_id
            // scoping (restored across the data layer) isolates users in this
            // mode, so this is a correct first-class path, not a fallback hole.
            let Some(registry) = registry else {
                return Ok(ResolvedDb(monolith_db));
            };

            let handle = registry
                .get_or_create(&auth.user_id.to_string())
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": format!("tenant registry error: {}", e) })),
                    )
                })?;

            Ok(ResolvedDb(handle.database()))
        }
    }
}
