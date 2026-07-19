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

// ── Long-term memory models ──────────────────────────────────────────

/// A single triple extracted by the memory compiler LLM.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Triple {
    pub source: String,
    pub source_type: String, // "User" | "Context" | "Item" | "Artifact"
    pub relation: String,    // "ENGAGED_IN" | "TOUCHED" | "PRODUCED"
    pub target: String,
    pub target_type: String, // "User" | "Context" | "Item" | "Artifact"
}

/// Structured output from the memory compiler LLM.
#[derive(Debug, Deserialize, Serialize)]
pub struct CompiledMemory {
    pub summary: String,
    pub domain: String, // "development" | "study" | "game" | "life" | "other"
    pub triples: Vec<Triple>,
}

/// Payload stored in Qdrant for a compiled memory.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QdrantPayload {
    pub summary: String,
    pub timestamp: i64,
    pub context_name: String,
    pub domain: String,
    pub slot_id: String,
}

/// A single embedding vector entry returned from the embedding API.
#[derive(Debug, Deserialize)]
pub struct EmbeddingData {
    pub embedding: Vec<f32>,
}

/// Response from the embedding API.
#[derive(Debug, Deserialize)]
pub struct EmbeddingResponse {
    pub data: Vec<EmbeddingData>,
}

/// Request body for the embedding API.
#[derive(Debug, Serialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}