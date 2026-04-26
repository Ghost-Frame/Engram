use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use kleos_lib::auth::AuthContext;
use kleos_lib::db::Database;
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

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
/// When tenant sharding is enabled (`state.tenant_registry` is `Some`), this
/// looks up (or lazily creates) the per-tenant handle and returns its async
/// database pool. Otherwise it falls back to the monolithic `state.db`.
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

            // Users whose data lives in the main DB bypass tenant sharding.
            // user_id=1 is the system/admin user with pre-existing data.
            if auth.user_id == 1 {
                return Ok(ResolvedDb(fallback_db));
            }

            if let Some(registry) = registry {
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
            } else {
                Ok(ResolvedDb(fallback_db))
            }
        }
    }
}
