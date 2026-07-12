use axum::{routing::post, Router};

mod logs;

pub fn api_router() -> Router {
    Router::new().nest("/api", Router::new().route("/logs", post(logs::handle_logs)))
}