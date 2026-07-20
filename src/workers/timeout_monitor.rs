use redis::AsyncCommands;
use std::sync::Arc;
use tokio::time::{interval, Duration};

use crate::services::slot_manager::flush_slot_with_compilation;
use crate::AppState;

/// Background task that checks for inactive slots every 60 seconds.
pub async fn run_timeout_monitor(state: Arc<AppState>) {
    let mut ticker = interval(Duration::from_secs(60));
    loop {
        ticker.tick().await;
        if let Err(e) = clean_inactive_slots(&state).await {
            tracing::error!(error = %e, "Timeout monitor cycle encountered an error");
        }
    }
}

async fn clean_inactive_slots(
    state: &Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut con = state.redis_conn.clone();

    // Get all active slot UUIDs
    let uuids: Vec<String> = con.smembers("active_slots").await.unwrap_or_default();
    let now_ts = chrono::Utc::now().timestamp();

    tracing::debug!(
        active_slot_count = uuids.len(),
        "Timeout monitor checking for inactive slots"
    );

    for uuid in uuids {
        let meta_key = format!("slot:{}:meta", uuid);
        let last_updated_opt: Option<i64> = con.hget(&meta_key, "last_updated").await?;
        if let Some(last) = last_updated_opt {
            // 15 minutes = 900 seconds
            if now_ts - last > 900 {
                tracing::info!(
                    slot = %uuid,
                    idle_seconds = now_ts - last,
                    "Slot timed out – flushing"
                );
                // Per-slot error isolation: a single slot flush failure
                // should not prevent other slots from being processed.
                if let Err(e) = flush_slot_with_compilation(
                    &con,
                    &uuid,
                    Arc::clone(&state.config),
                )
                .await
                {
                    tracing::error!(
                        slot = %uuid,
                        error = %e,
                        "Failed to flush timed-out slot"
                    );
                }
            }
        }
    }

    Ok(())
}
