use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};

#[allow(dead_code)]
pub async fn audit_middleware(request: Request, next: Next) -> Response {
    // TODO: log mutations via engram-lib::audit
    next.run(request).await
}
