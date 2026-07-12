use redis::AsyncCommands;
use std::sync::Arc;
use tokio::time::{interval, Duration};

use crate::services::slot_manager::flush_slot;
use crate::AppState;

/// Background task that checks for inactive slots every 60 seconds.
pub async fn run_timeout_monitor(state: Arc<AppState>) {
    let mut ticker = interval(Duration::from_secs(60));
    loop {
        ticker.tick().await;
        if let Err(e) = clean_inactive_slots(&state.redis_conn).await {
            tracing::error!(error = %e, "Failed to clean inactive slots");
        }
    }
}

async fn clean_inactive_slots(
    conn: &redis::aio::MultiplexedConnection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut con = conn.clone();

    // Get all active slot UUIDs
    let uuids: Vec<String> = con.smembers("active_slots").await.unwrap_or_default();
    let now_ts = chrono::Utc::now().timestamp();

    for uuid in uuids {
        let meta_key = format!("slot:{}:meta", uuid);
        let last_updated_opt: Option<i64> = con.hget(&meta_key, "last_updated").await?;
        if let Some(last) = last_updated_opt {
            // 15 minutes = 900 seconds
            if now_ts - last > 900 {
                flush_slot(&mut con, &uuid).await?;
            }
        }
    }

    Ok(())
}