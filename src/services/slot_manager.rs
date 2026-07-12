use redis::AsyncCommands;
use uuid::Uuid;
use chrono::Utc;

use crate::{
    models::{IngestLogRequest, SlotMeta, TimelineEntry},
};

/// Key for tracking recent slot assignments to detect rapid context shifts.
const CONTEXT_SHIFT_KEY: &str = "context_shift:recent_logs";
/// Window (seconds) for counting rapid log ingestion to the same slot.
const CONTEXT_SHIFT_WINDOW_SECS: i64 = 180; // 3 minutes
/// Threshold count to trigger immediate flush of other slots.
const CONTEXT_SHIFT_THRESHOLD: usize = 3;

/// Process an incoming log: find or create a slot and store the log.
pub async fn process_log(
    conn: &redis::aio::MultiplexedConnection,
    req: IngestLogRequest,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut con = conn.clone();
    let now_ts = Utc::now().timestamp();

    // Load all active slot UUIDs
    let uuids: Vec<String> = con.smembers("active_slots").await.unwrap_or_default();

    // Try to find a matching slot
    let mut matched_uuid: Option<String> = None;
    for uuid in &uuids {
        let meta_key = format!("slot:{}:meta", uuid);
        let topic_opt: Option<String> = con.hget(&meta_key, "topic").await?;
        if let Some(topic) = topic_opt {
            if topic == req.topic_hint || topic.contains(&req.topic_hint) {
                matched_uuid = Some(uuid.clone());
                break;
            }
        }
    }

    let slot_id = if let Some(ref uuid) = matched_uuid {
        // Matching slot found – update it
        let timeline_key = format!("slot:{}:timeline", uuid);
        let entry = TimelineEntry {
            timestamp: Utc::now(),
            source: req.source.clone(),
            content: req.content.clone(),
        };
        let entry_json = serde_json::to_string(&entry)?;
        con.rpush::<_, _, ()>(&timeline_key, entry_json).await?;

        // Update meta
        let focused = req.focused_file.unwrap_or_else(|| "None".to_string());
        let meta_key = format!("slot:{}:meta", uuid);
        con.hset::<_, _, _, ()>(&meta_key, "last_updated", now_ts).await?;
        con.hset::<_, _, _, ()>(&meta_key, "focused_file", focused).await?;
        uuid.clone()
    } else {
        // No matching slot – create a new one
        let new_uuid = Uuid::new_v4().to_string();
        con.sadd::<_, _, ()>("active_slots", &new_uuid).await?;

        let meta_key = format!("slot:{}:meta", new_uuid);
        let meta = SlotMeta {
            topic: req.topic_hint.clone(),
            focused_file: req.focused_file.unwrap_or_else(|| "None".to_string()),
            last_updated: Utc::now(),
        };
        let last_ts = meta.last_updated.timestamp();
        con.hset::<_, _, _, ()>(&meta_key, "topic", &meta.topic).await?;
        con.hset::<_, _, _, ()>(&meta_key, "focused_file", &meta.focused_file).await?;
        con.hset::<_, _, _, ()>(&meta_key, "last_updated", last_ts).await?;

        // Create timeline with first entry
        let timeline_key = format!("slot:{}:timeline", new_uuid);
        let entry = TimelineEntry {
            timestamp: Utc::now(),
            source: req.source,
            content: req.content,
        };
        let entry_json = serde_json::to_string(&entry)?;
        con.rpush::<_, _, ()>(&timeline_key, entry_json).await?;
        new_uuid
    };

    // ── Context-shift detection (Section 4.4) ──────────────────────
    // Track this assignment in a time-windowed list.
    let shift_entry = format!("{}|{}", slot_id, now_ts);
    con.rpush::<_, _, ()>(CONTEXT_SHIFT_KEY, &shift_entry).await?;

    // Trim list to only keep entries within the 3-minute window.
    // We remove elements older than now_ts - CONTEXT_SHIFT_WINDOW_SECS.
    // A simple approach: pop from left while the timestamp is too old.
    trim_context_shift_list(&mut con, now_ts).await?;

    // Count how many times this slot appears in the recent window.
    let recent: Vec<String> = con.lrange(CONTEXT_SHIFT_KEY, 0, -1).await.unwrap_or_default();
    let count_for_slot = recent
        .iter()
        .filter(|entry| entry.starts_with(&format!("{}|", slot_id)))
        .count();

    if count_for_slot >= CONTEXT_SHIFT_THRESHOLD {
        // Rapid ingestion detected for this slot – flush all OTHER active slots
        // that appear stale (their last activity is outside this window).
        for other_uuid in &uuids {
            if *other_uuid == slot_id {
                continue;
            }
            let meta_key = format!("slot:{}:meta", other_uuid);
            let last_updated_opt: Option<i64> =
                con.hget(&meta_key, "last_updated").await.unwrap_or(None);
            if let Some(last) = last_updated_opt {
                // If the other slot's last update is outside the recent window,
                // consider it abandoned and flush it.
                if now_ts - last > CONTEXT_SHIFT_WINDOW_SECS {
                    tracing::info!(
                        slot = %other_uuid,
                        new_slot = %slot_id,
                        "Context shift detected – flushing inactive slot immediately"
                    );
                    flush_slot(&mut con, other_uuid).await?;
                }
            }
        }
    }

    Ok(slot_id)
}

/// Remove entries from the context-shift list that are older than the window.
async fn trim_context_shift_list(
    con: &mut redis::aio::MultiplexedConnection,
    now_ts: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cutoff = now_ts - CONTEXT_SHIFT_WINDOW_SECS;
    loop {
        let front: Option<String> = con.lindex(CONTEXT_SHIFT_KEY, 0).await?;
        match front {
            Some(entry) => {
                // Format: "uuid|timestamp"
                if let Some(ts_str) = entry.split('|').nth(1) {
                    if let Ok(ts) = ts_str.parse::<i64>() {
                        if ts < cutoff {
                            con.lpop::<_, Option<String>>(CONTEXT_SHIFT_KEY, None).await?;
                            continue;
                        }
                    }
                }
                break;
            }
            None => break,
        }
    }
    Ok(())
}

/// Immediately flush a slot: delete its Redis data and remove from active set.
/// This is called both by the timeout monitor and the context-shift detector.
pub async fn flush_slot(
    con: &mut redis::aio::MultiplexedConnection,
    uuid: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(slot = %uuid, "Flushing slot data");

    let meta_key = format!("slot:{}:meta", uuid);
    let timeline_key = format!("slot:{}:timeline", uuid);

    // Delete timeline and meta
    con.del::<_, ()>(&[meta_key, timeline_key]).await?;

    // Remove from active set
    con.srem::<_, _, ()>("active_slots", uuid).await?;

    Ok(())
}