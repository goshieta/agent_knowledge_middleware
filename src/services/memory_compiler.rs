use std::sync::Arc;

use crate::config::Config;
use crate::models::{CompiledMemory, EmbeddingRequest, EmbeddingResponse, QdrantPayload, TimelineEntry, Triple};

/// System prompt for the memory compiler LLM.
const COMPILER_SYSTEM_PROMPT: &str = r#"# 役割
あなたは自律型パーソナルAIの「記憶編纂システム」です。ユーザーの断片的な行動ログ（時系列テキスト、スクショOCR、日常メモ）を分析し、未来のAIエージェントが瞬時に文脈を理解できるよう、「客観的な要約（Summary）」と「知識グラフ（トリプル）」へ厳密に構造化・圧縮しなさい。

# 要約（Summary）の生成ルール
以下の4つの要素を必ず含め、客観的な3人称（「ユーザーは〜」）で、1文〜最大3文の簡潔なMarkdownテキストとして出力しなさい。
1. 【文脈/目的】ユーザーが何を目的として、どのコンテクスト（開発、勉強、ゲーム、生活手続き等）で動いていたか。
2. 【行動/事象】ログの中で発生した決定的な出来事（エラー、特定の計算、座標のメモ、文章の作成など）。
3. 【知見/データ】ログから得られた具体的な数値、座標、ソースコードの修正点、数式の作り方などの「コアデータ」。
4. 【状態】最終的にそのタスクがどうなったか（解決した、中断した、課題が残っているなど）。
※「〜と思われる」「〜かもしれない」といった曖昧な推測は一切禁止。ログにある事実のみを記述すること。

# 知識グラフ（トリプル）の抽出ルール
入力ログから重要な実体（Entity）を抽出し、以下の定義に厳密に沿ってJSON配列として出力しなさい。

## エンティティ・タイプ（Labels）
- User: ユーザー自身（固定で "GoshiEta" とする）
- Context: プロジェクト名、科目名、ゲームの目的、生活のタスク名など（例: "英文読解 ソクモン", "大学化学", "Minecraft"）
- Item: 使用したツール、プログラミング言語、化学の反応名、ゲーム内のアイテム名など（例: "Next.js", "酸化還元反応", "加速レール"）
- Artifact: 具体的な成果物、ファイルパス、具体的な数値データ、座標など（例: "src/main.rs", "X:125, Z:-340", "問い合わせメール文面"）

## リレーション・タイプ（Relations）
- "ENGAGED_IN": User が Context に取り組んでいるとき
- "TOUCHED": 今回の出来事が Item に触れた、あるいは使用したとき
- "PRODUCED": 今回の出来事によって Artifact が生成・変更・確定されたとき

# 出力フォーマット
出力は必ず以下のJSON形式のみとし、余計な挨拶や解説のテキストは一切含めてはならない。

```json
{
  "summary": "（生成ルールに沿った要約）",
  "domain": "development | study | game | life | other",
  "triples": [
    {
      "source": "実体Aの名称",
      "source_type": "User | Context | Item | Artifact",
      "relation": "ENGAGED_IN | TOUCHED | PRODUCED",
      "target": "実体Bの名称",
      "target_type": "User | Context | Item | Artifact"
    }
  ]
}
```"#;

/// Call the AI to compile a flushed slot's timeline into a structured long-term memory.
pub async fn compile_slot_memory(
    config: &Config,
    slot_id: &str,
    topic: &str,
    timeline_entries: &[TimelineEntry],
) -> Result<CompiledMemory, Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(
        slot_id = %slot_id,
        topic = %topic,
        entry_count = timeline_entries.len(),
        "Starting memory compilation via LLM"
    );

    // Build the user message from timeline entries
    let mut log_text = String::new();
    for entry in timeline_entries {
        log_text.push_str(&format!(
            "[{}] (source: {}) {}\n",
            entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
            entry.source,
            entry.content
        ));
    }

    let user_message = format!(
        "Slot ID: {}\nTopic: {}\n\nTimeline logs:\n{}",
        slot_id, topic, log_text
    );

    let client = reqwest::Client::new();
    let url = format!(
        "{}/chat/completions",
        config.ai_base_url.trim_end_matches('/')
    );

    let request_body = serde_json::json!({
        "model": config.ai_model,
        "messages": [
            {"role": "system", "content": COMPILER_SYSTEM_PROMPT},
            {"role": "user", "content": user_message}
        ],
        "temperature": 0.1,
        "response_format": {"type": "json_object"}
    });

    let mut req = client.post(&url).json(&request_body);
    if let Some(ref key) = config.ai_api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Compiler AI API error ({}): {}", status, body).into());
    }

    let completion: serde_json::Value = resp.json().await?;
    let raw_json = completion["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("Compiler AI returned no content")?
        .trim()
        .to_string();

    let compiled: CompiledMemory = serde_json::from_str(&raw_json)
        .map_err(|e| format!("Failed to parse compiler AI response: {}. Raw: {}", e, raw_json))?;

    tracing::info!(
        slot_id = %slot_id,
        domain = %compiled.domain,
        triple_count = compiled.triples.len(),
        summary_len = compiled.summary.len(),
        "Memory compilation via LLM complete"
    );

    Ok(compiled)
}

/// Generate an embedding vector for the given text using the embedding API.
pub async fn generate_embedding(
    config: &Config,
    text: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let text_preview = crate::models::truncate_str(text, 100);
    tracing::info!(
        text_len = text.len(),
        text_preview = %text_preview,
        "Generating embedding"
    );

    let client = reqwest::Client::new();
    let url = format!(
        "{}/embeddings",
        config.embedding_api_url.trim_end_matches('/')
    );

    let request_body = EmbeddingRequest {
        model: config.embedding_model.clone(),
        prompt: text.to_string(),
        stream: Some(false),
    };

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", &config.embedding_api_password)
        .json(&request_body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Embedding API error ({}): {}", status, body).into());
    }

    let emb_resp: EmbeddingResponse = resp.json().await?;
    tracing::info!(
        embedding_dim = emb_resp.embedding.len(),
        "Embedding generated successfully"
    );
    Ok(emb_resp.embedding)
}

/// Upsert a compiled memory into Qdrant.
pub async fn upsert_to_qdrant(
    config: &Config,
    slot_id: &str,
    timestamp: i64,
    context_name: &str,
    compiled: &CompiledMemory,
    embedding: Vec<f32>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let base_url = config.qdrant_url.trim_end_matches('/');

    // Ensure the collection exists (idempotent)
    let collection_url = format!("{}/collections/user_memories", base_url);
    let create_body = serde_json::json!({
        "vectors": {
            "size": embedding.len(),
            "distance": "Cosine"
        }
    });

    // Try to create the collection; log if it fails for reasons other than "already exists"
    match client.put(&collection_url).json(&create_body).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(
                    status = %status,
                    body = %body,
                    "Qdrant collection creation returned non-success (may already exist)"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Failed to ensure Qdrant collection exists (will attempt upsert anyway)"
            );
        }
    }

    // Upsert the point
    let upsert_url = format!("{}/collections/user_memories/points", base_url);
    let payload = QdrantPayload {
        summary: compiled.summary.clone(),
        timestamp,
        context_name: context_name.to_string(),
        domain: compiled.domain.clone(),
        slot_id: slot_id.to_string(),
    };

    let point_id = uuid::Uuid::new_v4().to_string();
    let upsert_body = serde_json::json!({
        "points": [{
            "id": point_id,
            "vector": embedding,
            "payload": payload
        }]
    });

    let resp = client.put(&upsert_url).json(&upsert_body).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Qdrant upsert error ({}): {}", status, body).into());
    }

    tracing::info!(
        slot_id = %slot_id,
        qdrant_point_id = %point_id,
        domain = %compiled.domain,
        "Upserted compiled memory to Qdrant"
    );

    Ok(())
}

/// Write the compiled memory triples into Neo4j.
pub async fn write_to_neo4j(
    config: &Config,
    slot_id: &str,
    timestamp: i64,
    compiled: &CompiledMemory,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(
        slot_id = %slot_id,
        triple_count = compiled.triples.len(),
        "Writing compiled memory to Neo4j"
    );

    let uri = &config.neo4j_uri;
    let user = &config.neo4j_user;
    let password = &config.neo4j_password;

    let graph = neo4rs::Graph::new(uri, user, password).await?;

    // 1. Create the MemoryChunk node
    let create_chunk = neo4rs::query(
        r#"
        MERGE (m:MemoryChunk {slot_id: $slot_id})
        SET m.timestamp = $timestamp, m.summary = $summary, m.domain = $domain
        "#,
    )
    .param("slot_id", slot_id.to_string())
    .param("timestamp", timestamp)
    .param("summary", compiled.summary.clone())
    .param("domain", compiled.domain.clone());

    graph.run(create_chunk).await?;

    // 2. Process each triple
    let mut success_count = 0;
    let mut fail_count = 0;
    for triple in &compiled.triples {
        let cypher = build_triple_cypher(triple, slot_id);
        match graph.run(cypher).await {
            Ok(_) => success_count += 1,
            Err(e) => {
                fail_count += 1;
                tracing::warn!(
                    slot_id = %slot_id,
                    triple = ?triple,
                    error = %e,
                    "Failed to write triple to Neo4j"
                );
            }
        }
    }

    tracing::info!(
        slot_id = %slot_id,
        success_count = success_count,
        fail_count = fail_count,
        "Wrote compiled memory to Neo4j"
    );

    Ok(())
}

/// Build a Cypher query for a single triple, dynamically using the correct labels.
fn build_triple_cypher(triple: &Triple, slot_id: &str) -> neo4rs::Query {
    let source_label = map_label(&triple.source_type);
    let target_label = map_label(&triple.target_type);
    let relation = &triple.relation;

    let cypher = format!(
        r#"
        MERGE (s:{} {{name: $source_name}})
        MERGE (t:{} {{name: $target_name}})
        MERGE (s)-[:{}]->(t)
        WITH s, t
        MATCH (m:MemoryChunk {{slot_id: $slot_id}})
        MERGE (m)-[:BELONGS_TO]->(s)
        "#,
        source_label, target_label, relation
    );

    neo4rs::query(&cypher)
        .param("source_name", triple.source.clone())
        .param("target_name", triple.target.clone())
        .param("slot_id", slot_id.to_string())
}

/// Map a source_type/target_type string to the corresponding Neo4j label.
fn map_label(type_str: &str) -> &str {
    match type_str {
        "User" => "User",
        "Context" => "Context",
        "Item" => "Item",
        "Artifact" => "Artifact",
        _ => "Item", // fallback
    }
}

/// Full pipeline: compile a flushed slot into long-term memory.
pub async fn compile_and_store(
    config: Arc<Config>,
    slot_id: &str,
    topic: &str,
    timeline_entries: &[TimelineEntry],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(
        slot_id = %slot_id,
        topic = %topic,
        entry_count = timeline_entries.len(),
        "Starting compile_and_store pipeline"
    );

    if timeline_entries.is_empty() {
        tracing::info!(slot_id = %slot_id, "Skipping compilation for empty slot");
        return Ok(());
    }

    let timestamp = timeline_entries
        .first()
        .map(|e| e.timestamp.timestamp())
        .unwrap_or_else(|| chrono::Utc::now().timestamp());

    // Step 1: Compile via LLM
    let compiled = compile_slot_memory(&config, slot_id, topic, timeline_entries).await?;

    // Extract context_name from the first Context triple, or fall back to topic
    let context_name = compiled
        .triples
        .iter()
        .find(|t| t.source_type == "Context")
        .map(|t| t.source.clone())
        .unwrap_or_else(|| topic.to_string());

    // Step 2: Generate embedding
    let embedding = match generate_embedding(&config, &compiled.summary).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(slot_id = %slot_id, error = %e, "Failed to generate embedding");
            return Err(e);
        }
    };

    // Step 3: Write to Qdrant
    if let Err(e) = upsert_to_qdrant(&config, slot_id, timestamp, &context_name, &compiled, embedding).await {
        tracing::error!(slot_id = %slot_id, error = %e, "Failed to upsert to Qdrant");
        return Err(e);
    }

    // Step 4: Write to Neo4j
    if let Err(e) = write_to_neo4j(&config, slot_id, timestamp, &compiled).await {
        tracing::error!(slot_id = %slot_id, error = %e, "Failed to write to Neo4j");
        return Err(e);
    }

    tracing::info!(
        slot_id = %slot_id,
        "compile_and_store pipeline completed successfully"
    );

    Ok(())
}
