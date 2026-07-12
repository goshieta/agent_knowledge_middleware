use axum::{
    extract::{Extension, Json},
    response::IntoResponse,
};
use serde_json::json;
use std::sync::Arc;

use crate::{
    models::IngestLogRequest,
    services::slot_manager::process_log,
};

pub async fn handle_logs(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<IngestLogRequest>,
) -> impl IntoResponse {
    match process_log(&state.redis_conn, payload).await {
        Ok(slot_id) => {
            let body = json!({"status": "success", "slot_id": slot_id});
            (axum::http::StatusCode::OK, Json(body))
        }
        Err(e) => {
            let body = json!({"status": "error", "message": e.to_string()});
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(body))
        }
    }
}

// AppState is defined in main.rs and re-exported here for convenience.
use crate::AppState;