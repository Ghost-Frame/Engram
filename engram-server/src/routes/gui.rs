use axum::{routing::get, Router};
use axum::response::Html;

pub fn router() -> Router {
    Router::new().route("/", get(index))
}

async fn index() -> Html<&'static str> {
    Html("<html><body><h1>Engram</h1><p>GUI not yet implemented.</p></body></html>")
}
