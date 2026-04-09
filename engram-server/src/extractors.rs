use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use engram_lib::auth::AuthContext;
use serde_json::json;

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
                Json(json!({ "error": "Authentication required. Provide Bearer engram_* token." })),
            ));
        std::future::ready(result)
    }
}
