//! Integration tests for the AI Proxy server.
//!
//! These tests require a running Redis instance and an OpenAI-compatible
//! AI server. Set environment variables accordingly:
//!   REDIS_URL, AI_BASE_URL, AI_API_KEY (optional), AI_MODEL

#[tokio::test]
async fn test_ingest_log_endpoint() {
    // Spawn the binary
    let mut child = tokio::process::Command::new("cargo")
        .arg("run")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn server");

    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Send request with new simplified format (source + content only)
    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:3000/api/logs")
        .json(&serde_json::json!({
            "source": "ocr",
            "content": "スクリーンショットから抽出したテキスト: 英文読解の課題、ソクモン開発の続き"
        }))
        .send()
        .await
        .expect("Request failed");

    assert!(resp.status().is_success());

    let body: serde_json::Value = resp.json().await.expect("Failed to parse response");
    assert_eq!(body["status"], "success");
    assert!(body["slot_id"].is_string());

    // Shut down the server process
    let _ = child.kill().await;
}