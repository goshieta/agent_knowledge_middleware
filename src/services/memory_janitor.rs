use std::sync::Arc;

use crate::config::Config;
use crate::models::CompiledMemory;

/// System prompt for the memory compaction LLM (monthly episode summarization).
const COMPACTION_SYSTEM_PROMPT: &str = r#"# 役割
あなたは自律型パーソナルAIの「記憶圧縮システム」です。同一文脈における1ヶ月分の細かな活動記録（MemoryChunk群）を、1つの洗練された「月間エピソード要約」に圧縮しなさい。

# 圧縮ルール
1. 複数の細かな要約を読み込み、その月の活動全体を俯瞰した1つの要約（3〜5文）にまとめる。
2. 重要な成果物、学び、決定事項は必ず残す。
3. 細かすぎる日々の雑多な操作（タイポ修正、ログ追加など）は抽象化して「開発を継続した」などとまとめる。
4. 客観的な3人称（「ユーザーは〜」）で記述する。
5. ドメイン（domain）は元のチャンクから最も適切なものを選ぶ。

# 出力フォーマット
出力は必ず以下のJSON形式のみとし、余計な挨拶や解説のテキストは一切含めてはならない。

```json
{
  "summary": "（圧縮された月間要約）",
  "domain": "development | study | game | life | other"
}
```"#;

/// Run the full janitor cycle: merge similar nodes, compact old memories, and clean orphans.
pub async fn run_janitor_cycle(
    config: Arc<Config>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("Starting memory janitor cycle");

    // 3.1 Merge similar nodes
    if let Err(e) = merge_similar_nodes(&config).await {
        tracing::error!(error = %e, "Failed to merge similar nodes");
    }

    // 3.2 Compact old memories
    if let Err(e) = compact_old_memories(&config).await {
        tracing::error!(error = %e, "Failed to compact old memories");
    }

    // 3.3 Clean orphan data
    if let Err(e) = clean_orphans(&config).await {
        tracing::error!(error = %e, "Failed to clean orphans");
    }

    tracing::info!("Memory janitor cycle complete");
    Ok(())
}

/// 3.1 Merge similar nodes: find nodes with similar names and merge them.
async fn merge_similar_nodes(
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let graph = neo4rs::Graph::new(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await?;

    // Fetch all Context, Item, Artifact nodes with their names
    for label in &["Context", "Item", "Artifact"] {
        let query_str = format!("MATCH (n:{}) RETURN n.name AS name ORDER BY n.name", label);
        let query = neo4rs::query(&query_str);

        let mut result = graph.execute(query).await?;
        let mut names: Vec<String> = Vec::new();
        while let Ok(Some(row)) = result.next().await {
            if let Ok(name) = row.get::<String>("name") {
                names.push(name);
            }
        }

        // Find similar pairs using Levenshtein distance
        let similar_pairs = find_similar_pairs(&names, 0.9);

        for (keep, remove) in similar_pairs {
            tracing::info!(
                label = %label,
                keep = %keep,
                remove = %remove,
                "Merging similar nodes"
            );

            // Merge: move all relationships from `remove` to `keep`, then delete `remove`.
            // We use a two-step approach: fetch all relationships, re-create them from
            // the target node, then delete the source node.
            // 1. Get all relationships from source
            // 2. Re-create them from target
            // 3. Delete source
            let fetch_rels = format!(
                "MATCH (source:{} {{name: $remove}})-[r]->(x) RETURN type(r) AS rel_type, x.name AS target_name, labels(x) AS target_labels",
                label
            );
            let fetch_query = neo4rs::query(&fetch_rels).param("remove", remove.clone());

            let mut rels_to_recreate: Vec<(String, String, String)> = Vec::new();
            let mut result = graph.execute(fetch_query).await?;
            while let Ok(Some(row)) = result.next().await {
                let rel_type: String = row.get("rel_type").unwrap_or_default();
                let target_name: String = row.get("target_name").unwrap_or_default();
                let target_labels: Vec<String> = row.get("target_labels").unwrap_or_default();
                let target_label = target_labels.first().cloned().unwrap_or_else(|| "Item".to_string());
                if !target_name.is_empty() {
                    rels_to_recreate.push((rel_type, target_name, target_label));
                }
            }

            // Also fetch incoming relationships
            let fetch_incoming = format!(
                "MATCH (y)-[r]->(source:{} {{name: $remove}}) RETURN type(r) AS rel_type, y.name AS source_name, labels(y) AS source_labels",
                label
            );
            let fetch_in_query = neo4rs::query(&fetch_incoming).param("remove", remove.clone());

            let mut incoming_rels: Vec<(String, String, String)> = Vec::new();
            let mut result = graph.execute(fetch_in_query).await?;
            while let Ok(Some(row)) = result.next().await {
                let rel_type: String = row.get("rel_type").unwrap_or_default();
                let source_name: String = row.get("source_name").unwrap_or_default();
                let source_labels: Vec<String> = row.get("source_labels").unwrap_or_default();
                let source_label = source_labels.first().cloned().unwrap_or_else(|| "Item".to_string());
                if !source_name.is_empty() {
                    incoming_rels.push((rel_type, source_name, source_label));
                }
            }

            // Re-create outgoing relationships from target
            for (rel_type, target_name, target_label) in &rels_to_recreate {
                let create_rel = format!(
                    "MATCH (t:{} {{name: $keep}}) MATCH (x:{} {{name: $target_name}}) MERGE (t)-[:{}]->(x)",
                    label, target_label, rel_type
                );
                let q = neo4rs::query(&create_rel)
                    .param("keep", keep.clone())
                    .param("target_name", target_name.clone());
                let _ = graph.run(q).await;
            }

            // Re-create incoming relationships to target
            for (rel_type, source_name, source_label) in &incoming_rels {
                let create_rel = format!(
                    "MATCH (y:{} {{name: $source_name}}) MATCH (t:{} {{name: $keep}}) MERGE (y)-[:{}]->(t)",
                    source_label, label, rel_type
                );
                let q = neo4rs::query(&create_rel)
                    .param("keep", keep.clone())
                    .param("source_name", source_name.clone());
                let _ = graph.run(q).await;
            }

            // Delete the old node
            let delete_q = format!(
                "MATCH (source:{} {{name: $remove}}) DETACH DELETE source",
                label
            );
            let q = neo4rs::query(&delete_q).param("remove", remove.clone());
            let _ = graph.run(q).await;
        }
    }

    Ok(())
}

/// Find pairs of strings with similarity >= threshold using Levenshtein distance.
fn find_similar_pairs(names: &[String], threshold: f64) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            let similarity = levenshtein_similarity(&names[i], &names[j]);
            if similarity >= threshold {
                // Keep the longer name (assumed to be more canonical)
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

/// Compute normalized Levenshtein similarity between two strings (0.0 to 1.0).
fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let dist = levenshtein_distance(a, b);
    let max_len = a.len().max(b.len()) as f64;
    if max_len == 0.0 {
        return 1.0;
    }
    1.0 - (dist as f64 / max_len)
}

/// Compute Levenshtein (edit) distance between two strings.
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

/// 3.2 Compact old memories: merge MemoryChunks older than 30 days into monthly episodes.
async fn compact_old_memories(
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let graph = neo4rs::Graph::new(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await?;

    let thirty_days_ago = chrono::Utc::now().timestamp() - 30 * 24 * 3600;

    // Find old MemoryChunks grouped by domain
    let query = neo4rs::query(
        r#"
        MATCH (m:MemoryChunk)
        WHERE m.timestamp < $cutoff
        RETURN m.slot_id AS slot_id, m.summary AS summary, m.domain AS domain, m.timestamp AS timestamp
        ORDER BY m.domain, m.timestamp
        "#,
    )
    .param("cutoff", thirty_days_ago);

    let mut result = graph.execute(query).await?;

    // Group by domain
    let mut domain_groups: std::collections::HashMap<String, Vec<(String, String, i64)>> =
        std::collections::HashMap::new();

    while let Ok(Some(row)) = result.next().await {
        let slot_id: String = row.get("slot_id").unwrap_or_default();
        let summary: String = row.get("summary").unwrap_or_default();
        let domain: String = row.get("domain").unwrap_or_default();
        let timestamp: i64 = row.get("timestamp").unwrap_or(0);

        if !slot_id.is_empty() {
            domain_groups
                .entry(domain)
                .or_default()
                .push((slot_id, summary, timestamp));
        }
    }

    for (domain, chunks) in domain_groups {
        if chunks.len() < 2 {
            continue; // Need at least 2 chunks to compact
        }

        tracing::info!(
            domain = %domain,
            chunk_count = chunks.len(),
            "Compacting old memories"
        );

        // Build the prompt for the LLM
        let mut summaries_text = String::new();
        for (i, (_, summary, _)) in chunks.iter().enumerate() {
            summaries_text.push_str(&format!("{}. {}\n", i + 1, summary));
        }

        let user_message = format!(
            "ドメイン: {}\n\n圧縮対象の記憶:\n{}",
            domain, summaries_text
        );

        // Call LLM for compaction
        let client = reqwest::Client::new();
        let url = format!(
            "{}/chat/completions",
            config.ai_base_url.trim_end_matches('/')
        );

        let request_body = serde_json::json!({
            "model": config.ai_model,
            "messages": [
                {"role": "system", "content": COMPACTION_SYSTEM_PROMPT},
                {"role": "user", "content": user_message}
            ],
            "temperature": 0.1,
            "response_format": {"type": "json_object"}
        });

        let mut req = client.post(&url).json(&request_body);
        if let Some(ref key) = config.ai_api_key {
            req = req.bearer_auth(key);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "Failed to call compaction LLM");
                continue;
            }
        };

        if !resp.status().is_success() {
            tracing::error!(status = %resp.status(), "Compaction LLM returned error");
            continue;
        }

        let completion: serde_json::Value = match resp.json().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to parse compaction LLM response");
                continue;
            }
        };

        let raw_json = match completion["choices"][0]["message"]["content"].as_str() {
            Some(s) => s.trim().to_string(),
            None => continue,
        };

        let compiled: CompiledMemory = match serde_json::from_str(&raw_json) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, raw = %raw_json, "Failed to parse compaction result");
                continue;
            }
        };

        // Create a new MonthlyMemoryChunk node
        let new_slot_id = uuid::Uuid::new_v4().to_string();
        let now_ts = chrono::Utc::now().timestamp();

        let create_monthly = neo4rs::query(
            r#"
            CREATE (m:MonthlyMemoryChunk {
                slot_id: $slot_id,
                timestamp: $timestamp,
                summary: $summary,
                domain: $domain
            })
            "#,
        )
        .param("slot_id", new_slot_id.clone())
        .param("timestamp", now_ts)
        .param("summary", compiled.summary.clone())
        .param("domain", domain.clone());

        if let Err(e) = graph.run(create_monthly).await {
            tracing::error!(error = %e, "Failed to create MonthlyMemoryChunk");
            continue;
        }

        // Delete the old MemoryChunks
        for (slot_id, _, _) in &chunks {
            let delete_q = neo4rs::query(
                "MATCH (m:MemoryChunk {slot_id: $slot_id}) DETACH DELETE m",
            )
            .param("slot_id", slot_id.clone());
            let _ = graph.run(delete_q).await;
        }

        // Also clean up from Qdrant: delete old points by slot_id
        let qdrant_client = reqwest::Client::new();
        let base_url = config.qdrant_url.trim_end_matches('/');

        for (slot_id, _, _) in &chunks {
            let delete_url = format!(
                "{}/collections/user_memories/points/delete",
                base_url
            );
            let delete_body = serde_json::json!({
                "filter": {
                    "must": [{
                        "key": "slot_id",
                        "match": {"value": slot_id}
                    }]
                }
            });
            let _ = qdrant_client
                .post(&delete_url)
                .json(&delete_body)
                .send()
                .await;
        }

        // Generate embedding for the new monthly summary and upsert to Qdrant
        let embedding = match crate::services::memory_compiler::generate_embedding(
            config,
            &compiled.summary,
        )
        .await
        {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(error = %e, "Failed to generate embedding for monthly chunk");
                continue;
            }
        };

        let upsert_url = format!("{}/collections/user_memories/points", base_url);
        let payload = crate::models::QdrantPayload {
            summary: compiled.summary.clone(),
            timestamp: now_ts,
            context_name: domain.clone(),
            domain: compiled.domain.clone(),
            slot_id: new_slot_id.clone(),
        };

        let point_id = uuid::Uuid::new_v4().to_string();
        let upsert_body = serde_json::json!({
            "points": [{
                "id": point_id,
                "vector": embedding,
                "payload": payload
            }]
        });

        let _ = qdrant_client.put(&upsert_url).json(&upsert_body).send().await;

        tracing::info!(
            domain = %domain,
            old_chunks = chunks.len(),
            new_slot_id = %new_slot_id,
            "Compacted old memories into monthly episode"
        );
    }

    Ok(())
}

/// 3.3 Clean orphan data: remove Qdrant points without Neo4j MemoryChunk, and vice versa.
async fn clean_orphans(
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let graph = neo4rs::Graph::new(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await?;

    // Check 1: Get all slot_ids from Neo4j MemoryChunks
    let query = neo4rs::query("MATCH (m:MemoryChunk) RETURN m.slot_id AS slot_id");
    let mut result = graph.execute(query).await?;
    let mut neo4j_slot_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Ok(Some(row)) = result.next().await {
        if let Ok(slot_id) = row.get::<String>("slot_id") {
            neo4j_slot_ids.insert(slot_id);
        }
    }

    // Also get MonthlyMemoryChunk slot_ids
    let query2 = neo4rs::query("MATCH (m:MonthlyMemoryChunk) RETURN m.slot_id AS slot_id");
    let mut result2 = graph.execute(query2).await?;
    while let Ok(Some(row)) = result2.next().await {
        if let Ok(slot_id) = row.get::<String>("slot_id") {
            neo4j_slot_ids.insert(slot_id);
        }
    }

    // Check 1: Scan Qdrant and remove points not in Neo4j
    let qdrant_client = reqwest::Client::new();
    let base_url = config.qdrant_url.trim_end_matches('/');

    // Scroll through all points in Qdrant
    let scroll_url = format!(
        "{}/collections/user_memories/points/scroll",
        base_url
    );
    let scroll_body = serde_json::json!({
        "limit": 100,
        "with_payload": true
    });

    if let Ok(resp) = qdrant_client.post(&scroll_url).json(&scroll_body).send().await {
        if resp.status().is_success() {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(points) = data["result"]["points"].as_array() {
                    for point in points {
                        if let Some(slot_id) = point["payload"]["slot_id"].as_str() {
                            if !neo4j_slot_ids.contains(slot_id) {
                                // Delete orphan point from Qdrant
                                if let Some(point_id) = point["id"].as_str() {
                                    let delete_url = format!(
                                        "{}/collections/user_memories/points/delete",
                                        base_url
                                    );
                                    let delete_body = serde_json::json!({
                                        "points": [point_id]
                                    });
                                    let _ = qdrant_client
                                        .post(&delete_url)
                                        .json(&delete_body)
                                        .send()
                                        .await;
                                    tracing::info!(
                                        slot_id = %slot_id,
                                        qdrant_point_id = %point_id,
                                        "Removed orphan Qdrant point"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Check 2: Remove orphan Item/Artifact nodes in Neo4j (not connected to any Context or MemoryChunk)
    for label in &["Item", "Artifact"] {
        let orphan_query = format!(
            r#"
            MATCH (n:{})
            WHERE NOT (n)<-[:BELONGS_TO]-() AND NOT (n)-[:BELONGS_TO]->()
            AND NOT (n)<-[:TOUCHED]-() AND NOT (n)-[:TOUCHED]->()
            AND NOT (n)<-[:PRODUCED]-() AND NOT (n)-[:PRODUCED]->()
            AND NOT (n)<-[:ENGAGED_IN]-() AND NOT (n)-[:ENGAGED_IN]->()
            DELETE n
            "#,
            label
        );
        let q = neo4rs::query(&orphan_query);
        let _ = graph.run(q).await;
    }

    tracing::info!("Orphan cleanup complete");
    Ok(())
}