use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Router};
use tracing_subscriber::{fmt, EnvFilter};
use tokio::net::TcpListener;

mod api;
mod config;
mod models;
mod services;
mod workers;

/// Shared application state passed to handlers.
pub struct AppState {
    pub redis_conn: redis::aio::MultiplexedConnection,
    pub config: config::Config,
}

#[tokio::main]
async fn main() {
    // Initialize tracing subscriber for logs
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Load configuration and connect to Redis
    let cfg = config::Config::from_env();
    let redis_conn = cfg.create_redis_connection().await;

    let state = Arc::new(AppState {
        redis_conn,
        config: cfg,
    });

    // Spawn the timeout monitor worker
    {
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            workers::timeout_monitor::run_timeout_monitor(state_clone).await;
        });
    }

    // Build API router
    let app = Router::new()
        .nest("/", api::api_router())
        .fallback(get(|| async { "AI Proxy is running" }))
        .layer(axum::extract::Extension(state));

    // Run the server
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!(%addr, "Starting server");
    let listener = TcpListener::bind(addr).await.expect("Failed to bind");
    axum::serve(listener, app).await.expect("Server failed");
}