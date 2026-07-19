use std::sync::Arc;
use tokio::time::{interval, Duration};

use crate::config::Config;

/// Background task that runs the memory janitor cycle once per week (every 604800 seconds).
pub async fn run_janitor_worker(config: Arc<Config>) {
    // Run once at startup after a short delay, then weekly
    tokio::time::sleep(Duration::from_secs(30)).await;

    let mut ticker = interval(Duration::from_secs(604800)); // 7 days

    loop {
        tracing::info!("Starting scheduled memory janitor cycle");
        if let Err(e) = crate::services::memory_janitor::run_janitor_cycle(Arc::clone(&config))
            .await
        {
            tracing::error!(error = %e, "Memory janitor cycle failed");
        }
        ticker.tick().await;
    }
}