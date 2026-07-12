use serde::{Deserialize, Serialize};
use chrono::{Utc, DateTime};


/// Request payload for ingesting a log entry.
#[derive(Debug, Deserialize, Serialize)]
pub struct IngestLogRequest {
    pub source: String,          // e.g., "ocr", "memos"
    pub topic_hint: String,
    pub focused_file: Option<String>,
    pub content: String,
}

/// Representation of a timeline entry stored in Redis (as JSON).
#[derive(Debug, Deserialize, Serialize)]
pub struct TimelineEntry {
    #[serde(with = "chrono::serde::ts_seconds")]
    pub timestamp: DateTime<Utc>,
    pub source: String,
    pub content: String,
}

/// Metadata for a slot (stored as a Redis hash).
#[derive(Debug, Deserialize, Serialize)]
pub struct SlotMeta {
    pub topic: String,
    #[serde(default = "default_focused_file")]
    pub focused_file: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub last_updated: DateTime<Utc>,
}

fn default_focused_file() -> String {
    "None".to_string()
}