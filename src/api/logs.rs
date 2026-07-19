use axum::{
    extract::{Extension, Json},
    response::IntoResponse,
};
use serde_json::json;
use std::sync::Arc;

use crate::{
    models::IngestLogRequest,
    services::{ai_processor, slot_manager},
};

pub async fn handle_logs(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<IngestLogRequest>,
) -> impl IntoResponse {
    // Step 0: Fetch existing topics from active slots
    let existing_topics = match slot_manager::get_existing_topics(&state.redis_conn).await {
        Ok(topics) => topics,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch existing topics");
            let body = json!({"status": "error", "message": e.to_string()});
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(body));
        }
    };

    // Step 1: Call AI to extract topic (informed by existing topics) and summarize
    let processed = match ai_processor::process_raw_content(
        &state.config.ai_base_url,
        state.config.ai_api_key.as_deref(),
        &state.config.ai_model,
        &payload.source,
        &payload.content,
        &existing_topics,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "AI processing failed");
            let body = json!({"status": "error", "message": format!("AI processing failed: {}", e)});
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(body));
        }
    };

    // Step 2: Route to slot manager
    match slot_manager::process_log(&state.redis_conn, &payload.source, processed, Arc::clone(&state.config)).await {
        Ok(slot_id) => {
            let body = json!({"status": "success", "slot_id": slot_id});
            (axum::http::StatusCode::OK, Json(body))
        }
        Err(e) => {
            tracing::error!(error = %e, "Slot processing failed");
            let body = json!({"status": "error", "message": e.to_string()});
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(body))
        }
    }
}

use crate::AppState;