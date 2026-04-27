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
/// - `user_id == 1` returns the monolith fallback.
/// - sharding enabled returns the per-tenant shard.
/// - sharding disabled + non-system user returns Err so callers can map
///   that to 503 Service Unavailable just like the request extractor.
///
/// M-R3-007: GUI handlers needed this so /gui/memory/* writes land in the
/// same shard as /memory/*.
pub async fn resolve_db_for_user(
    state: &AppState,
    user_id: i64,
) -> Result<Arc<Database>, EngError> {
    if user_id == 1 {
        return Ok(Arc::clone(&state.db));
    }
    let registry = state
        .tenant_registry
        .as_ref()
        .ok_or_else(|| {
            EngError::Internal(
                "tenant sharding disabled; non-system users are unsupported".into(),
            )
        })?;
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
/// Behavior (post C-R3-004):
/// - `auth.user_id == 1` always returns the monolith `state.db`. This is the
///   system/admin user whose pre-existing data lives on the monolith.
/// - When tenant sharding is enabled (`state.tenant_registry` is `Some`),
///   non-system users get their per-tenant handle.
/// - When tenant sharding is disabled (`state.tenant_registry` is `None`),
///   non-system users receive **503 Service Unavailable**. The previous
///   silent fallback to the monolith was a default-config BOLA: helpers in
///   projects/webhooks/broca rely on shard isolation and have no `user_id`
///   predicate, so falling back to the monolith leaks every other tenant's
///   data. Failing closed surfaces the misconfig instead of leaking.
pub struct ResolvedDb(pub Arc<Database>);

impl FromRequestParts<AppState> for ResolvedDb {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let auth = parts.extensions.get::<AuthContext>().cloned();
        let registry = state.tenant_registry.clone();
        let fallback_db = Arc::clone(&state.db);

        async move {
            let auth = auth.ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "Authentication required." })),
                )
            })?;

            // System user always uses the monolith (legacy data).
            if auth.user_id == 1 {
                return Ok(ResolvedDb(fallback_db));
            }

            // Non-system users require tenant sharding to be enabled.
            // Failing closed (503) prevents the silent monolith fallback that
            // would otherwise allow cross-tenant reads/writes through helpers
            // that dropped their user_id predicates in Phase 5.
            let registry = registry.ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "tenant sharding disabled; non-system users are unsupported. \
                                  Enable ENGRAM_TENANT_SHARDING (default ON) and restart."
                    })),
                )
            })?;

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
