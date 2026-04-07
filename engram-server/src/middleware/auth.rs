use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};

pub async fn auth_middleware(request: Request, next: Next) -> Response {
    // TODO: validate bearer token against engram-lib::auth
    next.run(request).await
}
