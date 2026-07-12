use std::env;

use redis::{aio::MultiplexedConnection, Client};

/// Application configuration loaded from environment variables.
pub struct Config {
    /// Redis connection URL (e.g., redis://127.0.0.1:6379)
    pub redis_url: String,
}

impl Config {
    /// Load configuration from the environment.
    pub fn from_env() -> Self {
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        Config { redis_url }
    }

    /// Create a multiplexed Redis connection that can be shared across async tasks.
    pub async fn create_redis_connection(&self) -> MultiplexedConnection {
        let client = Client::open(self.redis_url.clone()).expect("Invalid REDIS_URL");
        client
            .get_multiplexed_async_connection()
            .await
            .expect("Failed to connect to Redis")
    }
}