use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};

#[allow(dead_code)]
pub async fn rate_limit_middleware(request: Request, next: Next) -> Response {
    // TODO: implement token bucket rate limiting
    next.run(request).await
}
