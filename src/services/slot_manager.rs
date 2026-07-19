use std::sync::Arc;

use redis::{AsyncCommands, Script};
use chrono::Utc;

use crate::config::Config;
use crate::models::{AiProcessedResult, TimelineEntry};

/// Key for tracking recent slot assignments to detect rapid context shifts.
const CONTEXT_SHIFT_KEY: &str = "context_shift:recent_logs";
/// Window (seconds) for counting rapid log ingestion to the same slot.
const CONTEXT_SHIFT_WINDOW_SECS: i64 = 180; // 3 minutes
/// Threshold count to trigger immediate flush of other slots.
const CONTEXT_SHIFT_THRESHOLD: usize = 3;

/// Lua script that atomically finds-or-creates a slot and appends a timeline entry.
///
/// KEYS[1] = "active_slots" set
/// KEYS[2] = CONTEXT_SHIFT_KEY list
/// ARGV[1] = topic to match / create
/// ARGV[2] = source string
/// ARGV[3] = summary (JSON-escaped timeline entry content)
/// ARGV[4] = entry_json (full TimelineEntry as JSON string)
/// ARGV[5] = now_ts (current Unix timestamp)
///
/// Returns: { slot_id, is_new, matched_topics_json }
const ATOMIC_PROCESS_LOG_SCRIPT: &str = r#"
local active_slots_key = KEYS[1]
local context_shift_key = KEYS[2]
local topic = ARGV[1]
local source = ARGV[2]
local summary = ARGV[3]
local entry_json = ARGV[4]
local now_ts = tonumber(ARGV[5])
local context_shift_window = tonumber(ARGV[6])
local context_shift_threshold = tonumber(ARGV[7])

-- Gather all active slot UUIDs and their topics in one pass
local uuids = redis.call('SMEMBERS', active_slots_key)
local matched_uuid = nil
local all_topics = {}

for _, uuid in ipairs(uuids) do
    local meta_key = 'slot:' .. uuid .. ':meta'
    local existing_topic = redis.call('HGET', meta_key, 'topic')
    if existing_topic then
        table.insert(all_topics, existing_topic)
        -- Match: exact match or existing topic contains the new topic
        if existing_topic == topic or string.find(existing_topic, topic, 1, true) then
            matched_uuid = uuid
        end
    end
end

local slot_id
local is_new = 0

if matched_uuid then
    slot_id = matched_uuid
else
    -- Create new slot atomically
    slot_id = redis.call('UUID')
    if not slot_id then
        -- Fallback: use Redis internal time + random for unique ID
        local seed = redis.call('TIME')
        slot_id = string.format('%s-%s', seed[1], seed[2])
    end
    redis.call('SADD', active_slots_key, slot_id)

    local meta_key = 'slot:' .. slot_id .. ':meta'
    redis.call('HSET', meta_key, 'topic', topic, 'focused_file', 'None', 'last_updated', now_ts)
    is_new = 1
end

-- Append timeline entry
local timeline_key = 'slot:' .. slot_id .. ':timeline'
redis.call('RPUSH', timeline_key, entry_json)

-- Update last_updated on the matched slot
local meta_key = 'slot:' .. slot_id .. ':meta'
redis.call('HSET', meta_key, 'last_updated', now_ts)

-- Context-shift detection: push entry and trim old ones
local shift_entry = slot_id .. '|' .. now_ts
redis.call('RPUSH', context_shift_key, shift_entry)

-- Trim old entries from context_shift list
local cutoff = now_ts - context_shift_window
while true do
    local front = redis.call('LINDEX', context_shift_key, 0)
    if not front then break end
    local parts = {}
    for part in string.gmatch(front, '[^|]+') do
        table.insert(parts, part)
    end
    if #parts >= 2 then
        local ts = tonumber(parts[2])
        if ts and ts < cutoff then
            redis.call('LPOP', context_shift_key)
        else
            break
        end
    else
        break
    end
end

-- Count recent entries for this slot
local recent = redis.call('LRANGE', context_shift_key, 0, -1)
local count_for_slot = 0
for _, entry in ipairs(recent) do
    if string.sub(entry, 1, #slot_id + 1) == slot_id .. '|' then
        count_for_slot = count_for_slot + 1
    end
end

-- Determine which slots to flush (context shift)
local flush_candidates = {}
if count_for_slot >= context_shift_threshold then
    for _, other_uuid in ipairs(uuids) do
        if other_uuid ~= slot_id then
            local other_meta_key = 'slot:' .. other_uuid .. ':meta'
            local last_updated = redis.call('HGET', other_meta_key, 'last_updated')
            if last_updated then
                local last = tonumber(last_updated)
                if last and (now_ts - last) > context_shift_window then
                    table.insert(flush_candidates, other_uuid)
                end
            end
        end
    end
end

-- Return results as a flat array: slot_id, is_new, then flush_candidates...
local result = {slot_id, tostring(is_new)}
for _, fc in ipairs(flush_candidates) do
    table.insert(result, fc)
end
-- Append all existing topics
table.insert(result, cjson.encode(all_topics))

return result
"#;

/// Fetch all existing topic strings from active slots.
pub async fn get_existing_topics(
    conn: &redis::aio::MultiplexedConnection,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut con = conn.clone();
    let uuids: Vec<String> = con.smembers("active_slots").await.unwrap_or_default();
    let mut topics = Vec::with_capacity(uuids.len());
    for uuid in &uuids {
        let meta_key = format!("slot:{}:meta", uuid);
        if let Some(topic) = con.hget::<_, _, Option<String>>(&meta_key, "topic").await? {
            topics.push(topic);
        }
    }
    Ok(topics)
}

/// Process an AI-processed log: atomically find or create a slot by topic and store the summary.
///
/// Uses a Redis Lua script to ensure atomicity of the read-check-then-act sequence,
/// preventing duplicate slot creation under concurrent requests.
pub async fn process_log(
    conn: &redis::aio::MultiplexedConnection,
    source: &str,
    processed: AiProcessedResult,
    config: Arc<Config>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut con = conn.clone();
    let now_ts = Utc::now().timestamp();

    let entry = TimelineEntry {
        timestamp: Utc::now(),
        source: source.to_string(),
        content: processed.summary.clone(),
    };
    let entry_json = serde_json::to_string(&entry)?;

    let script = Script::new(ATOMIC_PROCESS_LOG_SCRIPT);
    let result: Vec<String> = script
        .key("active_slots")
        .key(CONTEXT_SHIFT_KEY)
        .arg(&processed.topic)
        .arg(source)
        .arg(&processed.summary)
        .arg(&entry_json)
        .arg(now_ts)
        .arg(CONTEXT_SHIFT_WINDOW_SECS)
        .arg(CONTEXT_SHIFT_THRESHOLD)
        .invoke_async(&mut con)
        .await?;

    if result.is_empty() {
        return Err("Atomic process_log script returned no results".into());
    }

    let slot_id = result[0].clone();
    // result[1] is is_new ("0" or "1")
    // result[2..n-1] are flush_candidate UUIDs (if any)
    // result[last] is JSON-encoded all_topics array (we don't need it here)

    let flush_candidates: Vec<String> = if result.len() > 2 {
        // Last element is the JSON-encoded topics list; exclude it
        let candidate_count = result.len() - 3; // slot_id, is_new, topics_json
        result[2..2 + candidate_count].to_vec()
    } else {
        vec![]
    };

    // Process context-shift flushes outside the Lua script
    // (Lua scripts cannot make HTTP calls for memory compilation)
    if !flush_candidates.is_empty() {
        for other_uuid in &flush_candidates {
            tracing::info!(
                slot = %other_uuid,
                new_slot = %slot_id,
                "Context shift detected – flushing inactive slot immediately"
            );
            // Clone con for each spawn to avoid lifetime issues
            let mut flush_con = conn.clone();
            let flush_config = Arc::clone(&config);
            let uuid = other_uuid.clone();
            tokio::spawn(async move {
                if let Err(e) = flush_slot_with_compilation(&mut flush_con, &uuid, flush_config)
                    .await
                {
                    tracing::error!(
                        slot = %uuid,
                        error = %e,
                        "Failed to flush slot during context shift"
                    );
                }
            });
        }
    }

    Ok(slot_id)
}

/// Immediately flush a slot: delete its Redis data and remove from active set.
/// Also triggers long-term memory compilation if a config is provided.
pub async fn flush_slot(
    con: &mut redis::aio::MultiplexedConnection,
    uuid: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(slot = %uuid, "Flushing slot data");

    let meta_key = format!("slot:{}:meta", uuid);
    let timeline_key = format!("slot:{}:timeline", uuid);

    con.del::<_, ()>(&[&meta_key, &timeline_key]).await?;
    con.srem::<_, _, ()>("active_slots", uuid).await?;

    Ok(())
}

/// Flush a slot and compile its timeline into long-term memory.
/// This variant is used when we have access to the full AppState.
pub async fn flush_slot_with_compilation(
    con: &mut redis::aio::MultiplexedConnection,
    uuid: &str,
    config: Arc<Config>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Fetch the timeline and topic before deleting
    let timeline_key = format!("slot:{}:timeline", uuid);
    let meta_key = format!("slot:{}:meta", uuid);

    let topic: Option<String> = con.hget(&meta_key, "topic").await?;
    let raw_entries: Vec<String> = con.lrange(&timeline_key, 0, -1).await.unwrap_or_default();

    let timeline_entries: Vec<TimelineEntry> = raw_entries
        .iter()
        .filter_map(|s| serde_json::from_str::<TimelineEntry>(s).ok())
        .collect();

    // Flush the slot from Redis
    flush_slot(con, uuid).await?;

    // Trigger long-term memory compilation in the background
    if let Some(topic) = topic {
        let slot_id = uuid.to_string();
        tokio::spawn(async move {
            if let Err(e) = crate::services::memory_compiler::compile_and_store(
                config,
                &slot_id,
                &topic,
                &timeline_entries,
            )
            .await
            {
                tracing::error!(
                    slot_id = %slot_id,
                    error = %e,
                    "Failed to compile slot into long-term memory"
                );
            }
        });
    }

    Ok(())
}
