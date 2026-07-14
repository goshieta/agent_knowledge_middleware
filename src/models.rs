use serde::{Deserialize, Serialize};
use chrono::{Utc, DateTime};

/// Request payload for ingesting a log entry.
/// Only `source` and raw `content` are accepted; topic and summary are
/// derived by the AI processor.
#[derive(Debug, Deserialize, Serialize)]
pub struct IngestLogRequest {
    pub source: String, // e.g., "ocr", "memos", "voice"
    pub content: String, // raw, unprocessed data
}

/// Result of AI processing: extracted topic and summarized content.
#[derive(Debug, Deserialize, Serialize)]
pub struct AiProcessedResult {
    pub topic: String,
    pub summary: String,
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