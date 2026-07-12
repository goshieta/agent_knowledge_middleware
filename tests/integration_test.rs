//! Basic integration test that starts the server in a background task
//! and sends a POST /api/logs request.

#[tokio::test]
async fn test_ingest_log_endpoint() {
    // Spawn the binary (assumes `cargo run` works)
    let mut child = tokio::process::Command::new("cargo")
        .arg("run")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn server");

    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Send request
    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:3000/api/logs")
        .json(&serde_json::json!({
            "source": "ocr",
            "topic_hint": "test-topic",
            "focused_file": null,
            "content": "sample log entry"
        }))
        .send()
        .await
        .expect("Request failed");

    assert!(resp.status().is_success());

    // Shut down the server process
    let _ = child.kill().await;
}