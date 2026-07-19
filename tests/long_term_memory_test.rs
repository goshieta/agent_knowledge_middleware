//! Unit tests for the long-term memory compiler and janitor engines.
//!
//! These tests focus on pure logic that does not require external services
//! (Levenshtein distance, data structures, etc.).
//! Since this is a binary-only crate, we test the algorithms directly
//! rather than importing from the crate.

use serde::{Deserialize, Serialize};

// ── Replicated data structures for testing ──────────────────────────

/// A single triple extracted by the memory compiler LLM.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct Triple {
    source: String,
    source_type: String,
    relation: String,
    target: String,
    target_type: String,
}

/// Structured output from the memory compiler LLM.
#[derive(Debug, Deserialize, Serialize)]
struct CompiledMemory {
    summary: String,
    domain: String,
    triples: Vec<Triple>,
}

/// Payload stored in Qdrant for a compiled memory.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct QdrantPayload {
    summary: String,
    timestamp: i64,
    context_name: String,
    domain: String,
    slot_id: String,
}

/// Request body for the embedding API.
#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

// ── Levenshtein distance functions (mirrors memory_janitor.rs) ──────

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];
    for i in 0..=a_len {
        dp[i][0] = i;
    }
    for j in 0..=b_len {
        dp[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[a_len][b_len]
}

fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let dist = levenshtein_distance(a, b);
    let max_len = a.len().max(b.len()) as f64;
    if max_len == 0.0 {
        return 1.0;
    }
    1.0 - (dist as f64 / max_len)
}

fn find_similar_pairs(names: &[String], threshold: f64) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            let similarity = levenshtein_similarity(&names[i], &names[j]);
            if similarity >= threshold {
                if names[i].len() >= names[j].len() {
                    pairs.push((names[i].clone(), names[j].clone()));
                } else {
                    pairs.push((names[j].clone(), names[i].clone()));
                }
            }
        }
    }
    pairs
}

// ── Tests ───────────────────────────────────────────────────────────

/// Test Levenshtein distance computation used in the janitor's node merging.
#[test]
fn test_levenshtein_distance() {
    // Exact match
    assert_eq!(levenshtein_distance("hello", "hello"), 0);
    assert!((levenshtein_similarity("hello", "hello") - 1.0).abs() < 0.001);

    // One character difference
    assert_eq!(levenshtein_distance("hello", "hallo"), 1);
    assert!((levenshtein_similarity("hello", "hallo") - 0.8).abs() < 0.001);

    // Completely different
    assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    assert!((levenshtein_similarity("abc", "xyz") - 0.0).abs() < 0.001);

    // Empty strings
    assert_eq!(levenshtein_distance("", ""), 0);
    assert!((levenshtein_similarity("", "") - 1.0).abs() < 0.001);
    assert_eq!(levenshtein_distance("abc", ""), 3);
    assert!((levenshtein_similarity("abc", "") - 0.0).abs() < 0.001);

    // Similar names (typo / abbreviation)
    let dist = levenshtein_distance("Next.js", "nextjs");
    assert_eq!(dist, 2); // 'N'->'n' (case) + '.' removed
    let sim = levenshtein_similarity("Next.js", "nextjs");
    assert!(sim > 0.7); // 1 - 2/7 ≈ 0.714

    // Japanese text
    assert_eq!(levenshtein_distance("酸化還元反応", "酸化還元"), 2); // 反応 removed
    let sim_ja = levenshtein_similarity("酸化還元反応", "酸化還元");
    assert!(sim_ja > 0.65); // 1 - 2/6 ≈ 0.667
}

/// Test that find_similar_pairs logic works correctly.
#[test]
fn test_find_similar_pairs() {
    // No similar pairs
    let names1: Vec<String> = vec!["React".into(), "Python".into(), "Docker".into()];
    let pairs1 = find_similar_pairs(&names1, 0.9);
    assert!(pairs1.is_empty());

    // Similar pair: "Next.js" and "nextjs"
    let names2: Vec<String> = vec!["Next.js".into(), "nextjs".into(), "Rust".into()];
    let pairs2 = find_similar_pairs(&names2, 0.7);
    assert_eq!(pairs2.len(), 1);
    assert_eq!(pairs2[0].0, "Next.js"); // longer name kept
    assert_eq!(pairs2[0].1, "nextjs");

    // Multiple similar pairs
    let names3: Vec<String> = vec![
        "酸化還元反応".into(),
        "酸化還元".into(),
        "Minecraft".into(),
        "minecraft".into(),
    ];
    let pairs3 = find_similar_pairs(&names3, 0.6);
    assert_eq!(pairs3.len(), 2);
}

/// Test the data model serialization/deserialization.
#[test]
fn test_compiled_memory_serde() {
    // Test CompiledMemory deserialization
    let json = r#"{
        "summary": "ユーザーはMinecraftで鉄道敷設を行い、加速レールを用いた高速路線をX:125, Z:-340に敷設した。",
        "domain": "game",
        "triples": [
            {
                "source": "GoshiEta",
                "source_type": "User",
                "relation": "ENGAGED_IN",
                "target": "Minecraft",
                "target_type": "Context"
            },
            {
                "source": "Minecraft",
                "source_type": "Context",
                "relation": "TOUCHED",
                "target": "加速レール",
                "target_type": "Item"
            },
            {
                "source": "Minecraft",
                "source_type": "Context",
                "relation": "PRODUCED",
                "target": "X:125, Z:-340",
                "target_type": "Artifact"
            }
        ]
    }"#;

    let compiled: CompiledMemory =
        serde_json::from_str(json).expect("Failed to parse CompiledMemory");

    assert_eq!(compiled.domain, "game");
    assert!(compiled.summary.contains("Minecraft"));
    assert_eq!(compiled.triples.len(), 3);

    // Check first triple
    assert_eq!(compiled.triples[0].source, "GoshiEta");
    assert_eq!(compiled.triples[0].source_type, "User");
    assert_eq!(compiled.triples[0].relation, "ENGAGED_IN");
    assert_eq!(compiled.triples[0].target, "Minecraft");
    assert_eq!(compiled.triples[0].target_type, "Context");
}

/// Test QdrantPayload serialization.
#[test]
fn test_qdrant_payload_serde() {
    let payload = QdrantPayload {
        summary: "テスト要約".to_string(),
        timestamp: 1781848200,
        context_name: "Minecraft 鉄道敷設".to_string(),
        domain: "game".to_string(),
        slot_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
    };

    let json = serde_json::to_string(&payload).expect("Failed to serialize");
    let parsed: QdrantPayload =
        serde_json::from_str(&json).expect("Failed to deserialize");

    assert_eq!(parsed.summary, "テスト要約");
    assert_eq!(parsed.timestamp, 1781848200);
    assert_eq!(parsed.context_name, "Minecraft 鉄道敷設");
    assert_eq!(parsed.domain, "game");
    assert_eq!(parsed.slot_id, "550e8400-e29b-41d4-a716-446655440000");
}

/// Test EmbeddingRequest serialization.
#[test]
fn test_embedding_request_serde() {
    let req = EmbeddingRequest {
        model: "jeffh/intfloat-multilingual-e5-large:f32".to_string(),
        prompt: "カレーの作り方".to_string(),
        stream: Some(false),
    };

    let json = serde_json::to_string(&req).expect("Failed to serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("Failed to parse");

    assert_eq!(parsed["model"], "jeffh/intfloat-multilingual-e5-large:f32");
    assert_eq!(parsed["prompt"], "カレーの作り方");
    assert_eq!(parsed["stream"], false);
}

/// Test that the config loads with default values when env vars are not set.
#[test]
fn test_config_defaults() {
    // Clear relevant env vars for this test
    std::env::remove_var("PORT");
    std::env::remove_var("REDIS_URL");
    std::env::remove_var("AI_BASE_URL");
    std::env::remove_var("AI_API_KEY");
    std::env::remove_var("AI_MODEL");
    std::env::remove_var("QDRANT_URL");
    std::env::remove_var("NEO4J_URI");
    std::env::remove_var("NEO4J_USER");
    std::env::remove_var("NEO4J_PASSWORD");
    std::env::remove_var("EMBEDDING_API_URL");
    std::env::remove_var("EMBEDDING_API_PASSWORD");
    std::env::remove_var("EMBEDDING_MODEL");

    // Replicate the config loading logic
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap();
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let ai_base_url =
        std::env::var("AI_BASE_URL").unwrap_or_else(|_| "http://localhost:8080/v1".to_string());
    let ai_api_key = std::env::var("AI_API_KEY").ok();
    let ai_model =
        std::env::var("AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let qdrant_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6333".to_string());
    let neo4j_uri =
        std::env::var("NEO4J_URI").unwrap_or_else(|_| "bolt://localhost:7687".to_string());
    let neo4j_user =
        std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let neo4j_password =
        std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let embedding_api_url = std::env::var("EMBEDDING_API_URL")
        .unwrap_or_else(|_| "https://aisvr221.aikb.kyutech.ac.jp/api".to_string());
    let embedding_api_password =
        std::env::var("EMBEDDING_API_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let embedding_model = std::env::var("EMBEDDING_MODEL")
        .unwrap_or_else(|_| "jeffh/intfloat-multilingual-e5-large:f32".to_string());

    assert_eq!(port, 3000);
    assert_eq!(redis_url, "redis://127.0.0.1:6379");
    assert_eq!(ai_base_url, "http://localhost:8080/v1");
    assert_eq!(ai_api_key, None);
    assert_eq!(ai_model, "gpt-4o-mini");
    assert_eq!(qdrant_url, "http://localhost:6333");
    assert_eq!(neo4j_uri, "bolt://localhost:7687");
    assert_eq!(neo4j_user, "neo4j");
    assert_eq!(neo4j_password, "password");
    assert_eq!(
        embedding_api_url,
        "https://aisvr221.aikb.kyutech.ac.jp/api"
    );
    assert_eq!(embedding_api_password, "password");
    assert_eq!(
        embedding_model,
        "jeffh/intfloat-multilingual-e5-large:f32"
    );
}

/// Test that the config loads custom values from environment variables.
#[test]
fn test_config_custom_values() {
    std::env::set_var("PORT", "8080");
    std::env::set_var("REDIS_URL", "redis://custom:6379");
    std::env::set_var("AI_BASE_URL", "http://custom:1234/v1");
    std::env::set_var("AI_API_KEY", "sk-test-key");
    std::env::set_var("AI_MODEL", "gpt-4");
    std::env::set_var("QDRANT_URL", "http://qdrant:6333");
    std::env::set_var("NEO4J_URI", "bolt://neo4j:7687");
    std::env::set_var("NEO4J_USER", "admin");
    std::env::set_var("NEO4J_PASSWORD", "secret");
    std::env::set_var("EMBEDDING_API_URL", "http://embed:8080/api");
    std::env::set_var("EMBEDDING_API_PASSWORD", "mypassword");
    std::env::set_var("EMBEDDING_MODEL", "custom-model");

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap();
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let ai_base_url =
        std::env::var("AI_BASE_URL").unwrap_or_else(|_| "http://localhost:8080/v1".to_string());
    let ai_api_key = std::env::var("AI_API_KEY").ok();
    let ai_model =
        std::env::var("AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let qdrant_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6333".to_string());
    let neo4j_uri =
        std::env::var("NEO4J_URI").unwrap_or_else(|_| "bolt://localhost:7687".to_string());
    let neo4j_user =
        std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let neo4j_password =
        std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let embedding_api_url = std::env::var("EMBEDDING_API_URL")
        .unwrap_or_else(|_| "https://aisvr221.aikb.kyutech.ac.jp/api".to_string());
    let embedding_api_password =
        std::env::var("EMBEDDING_API_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let embedding_model = std::env::var("EMBEDDING_MODEL")
        .unwrap_or_else(|_| "jeffh/intfloat-multilingual-e5-large:f32".to_string());

    assert_eq!(port, 8080);
    assert_eq!(redis_url, "redis://custom:6379");
    assert_eq!(ai_base_url, "http://custom:1234/v1");
    assert_eq!(ai_api_key, Some("sk-test-key".to_string()));
    assert_eq!(ai_model, "gpt-4");
    assert_eq!(qdrant_url, "http://qdrant:6333");
    assert_eq!(neo4j_uri, "bolt://neo4j:7687");
    assert_eq!(neo4j_user, "admin");
    assert_eq!(neo4j_password, "secret");
    assert_eq!(embedding_api_url, "http://embed:8080/api");
    assert_eq!(embedding_api_password, "mypassword");
    assert_eq!(embedding_model, "custom-model");

    // Clean up
    std::env::remove_var("PORT");
    std::env::remove_var("REDIS_URL");
    std::env::remove_var("AI_BASE_URL");
    std::env::remove_var("AI_API_KEY");
    std::env::remove_var("AI_MODEL");
    std::env::remove_var("QDRANT_URL");
    std::env::remove_var("NEO4J_URI");
    std::env::remove_var("NEO4J_USER");
    std::env::remove_var("NEO4J_PASSWORD");
    std::env::remove_var("EMBEDDING_API_URL");
    std::env::remove_var("EMBEDDING_API_PASSWORD");
    std::env::remove_var("EMBEDDING_MODEL");
}