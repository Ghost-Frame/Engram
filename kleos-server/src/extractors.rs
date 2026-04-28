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
/// - sharding disabled returns Err so callers can map that to 503 Service
///   Unavailable just like the request extractor.
///
/// M-R3-007: GUI handlers needed this so /gui/memory/* writes land in the
/// same shard as /memory/*. The user_id==1 carve-out that previously
/// returned the monolith was removed during the monolith->tenant
/// migration; user_id=1 now lives in tenants/1/ like every other user.
/// The monolith remains mounted only for system-scoped tables (users,
/// api_keys, audit_log, agents, app_state).
pub async fn resolve_db_for_user(
    state: &AppState,
    user_id: i64,
) -> Result<Arc<Database>, EngError> {
    let registry = state.tenant_registry.as_ref().ok_or_else(|| {
        EngError::Internal("tenant sharding disabled; non-system users are unsupported".into())
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
/// Every authenticated user, including user_id=1 (the operator), routes
/// through their per-tenant shard via `tenant_registry`. The previous
/// user_id==1 -> monolith carve-out was removed during the monolith
/// migration: Master is tenant_1 like every other user, and the monolith
/// only retains system-scoped tables (users, api_keys, audit_log, agents,
/// app_state) accessed directly via `state.db`.
///
/// When tenant sharding is disabled (`state.tenant_registry` is `None`),
/// every authenticated user receives **503 Service Unavailable**. The
/// previous silent monolith fallback was a default-config BOLA: helpers
/// in projects/webhooks/broca rely on shard isolation and dropped their
/// user_id predicates in Phase 5; failing closed surfaces the misconfig
/// instead of leaking other tenants' data.
pub struct ResolvedDb(pub Arc<Database>);

impl FromRequestParts<AppState> for ResolvedDb {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let auth = parts.extensions.get::<AuthContext>().cloned();
        let registry = state.tenant_registry.clone();

        async move {
            let auth = auth.ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "Authentication required." })),
                )
            })?;

            let registry = registry.ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "tenant sharding disabled; tenant-scoped routes are unavailable. \
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
