use std::env;

use redis::{aio::MultiplexedConnection, Client};

/// Application configuration loaded from environment variables.
pub struct Config {
    /// Server listen port (default: 3000)
    pub port: u16,
    /// Redis connection URL (e.g., redis://127.0.0.1:6379)
    pub redis_url: String,
    /// OpenAI-compatible API base URL (e.g., http://localhost:8080/v1)
    pub ai_base_url: String,
    /// API key for the AI service (optional, sent as Bearer token)
    pub ai_api_key: Option<String>,
    /// Model name to use (e.g., "gpt-4o-mini", "llama3")
    pub ai_model: String,
}

impl Config {
    /// Load configuration from the environment.
    pub fn from_env() -> Self {
        let redis_url = env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let ai_base_url = env::var("AI_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080/v1".to_string());
        let ai_api_key = env::var("AI_API_KEY").ok();
        let ai_model = env::var("AI_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let port = env::var("PORT")
            .unwrap_or_else(|_| "3000".to_string())
            .parse::<u16>()
            .expect("PORT must be a valid u16 port number");

        Config {
            port,
            redis_url,
            ai_base_url,
            ai_api_key,
            ai_model,
        }
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