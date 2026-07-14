use serde::{Deserialize, Serialize};
use crate::models::AiProcessedResult;

/// OpenAI-compatible chat completion request.
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

/// OpenAI-compatible chat completion response.
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: String,
}

fn build_system_prompt(existing_topics: &[String]) -> String {
    let topics_list = if existing_topics.is_empty() {
        "(none — no existing topics yet)".to_string()
    } else {
        existing_topics
            .iter()
            .enumerate()
            .map(|(i, t)| format!("  {}. \"{}\"", i + 1, t))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are a context analyzer for a personal AI knowledge management system.
This system acts as middleware that processes raw sensor inputs (OCR, voice transcripts,
memos, etc.) and routes them into a "Hot Memory" slot system backed by Redis.
Each slot represents an ongoing context/topic in the user's life.

## Your Task

Analyze the given raw input and produce a JSON object with exactly two fields:
"topic" and "summary".

## Topic Selection Rules

Below is the list of EXISTING active topics (slots). Your job is to choose the
**best matching existing topic** if the input clearly belongs to one of them.
Only create a NEW topic if the input does NOT fit any existing topic.

Existing topics:
{}

When deciding:
- Same project/task with slightly different phrasing → REUSE the exact existing topic string
  (e.g., if existing is "英文読解 ソクモン 開発", and input is about ソクモン debugging,
  output exactly "英文読解 ソクモン 開発")
- Completely different context → create a new concise label (1-5 words, Japanese or English)
- Topic should capture WHAT the user is working on, not how the data was captured

## Summary Rules (CRITICAL — this data feeds a personal knowledge base)

The summary must preserve ALL information that could be valuable for future
reference. This is NOT a casual TL;DR. Extract and structure the following
with high fidelity:

1. **User identity & attributes**: name, role, affiliations, preferences, skills
   mentioned, accounts, devices used
2. **Current activity & state**: what the user is doing right now, where they are,
   what application/file they are working with, their physical/mental state
3. **Thoughts & decisions**: what the user is thinking, decisions being made,
   questions being asked, reasoning processes, opinions expressed
4. **Discoveries & learnings**: new information acquired, insights gained,
   problems solved, answers found, skills learned
5. **Concrete work content**: specific code being written, documents being edited,
   data being analyzed, URLs visited, commands run, error messages encountered,
   specific numbers/figures/metrics
6. **Action items & intentions**: things the user plans to do, deadlines,
   commitments, reminders mentioned
7. **Relationships & context**: people mentioned, how topics connect, timeline
   of events, cause and effect

Be thorough. If the input is long and rich, your summary should be proportionally
detailed. Do NOT drop specifics for the sake of brevity. A 2-page document
deserves a substantial summary. Omit only obvious noise and OCR artifacts.

## Output Format

Respond ONLY with valid JSON (no markdown fences, no extra text):
{{"topic": "<chosen or new topic>", "summary": "<structured summary>"}}"#,
        topics_list
    )
}

/// Call the OpenAI-compatible API to extract a topic and summarize the raw content.
pub async fn process_raw_content(
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    source: &str,
    raw_content: &str,
    existing_topics: &[String],
) -> Result<AiProcessedResult, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let user_message = format!(
        "Source: {}\n\nRaw content:\n{}",
        source, raw_content
    );

    let system_prompt = build_system_prompt(existing_topics);

    let request_body = ChatCompletionRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
            },
            ChatMessage {
                role: "user".to_string(),
                content: user_message,
            },
        ],
        temperature: 0.1,
        response_format: Some(ResponseFormat {
            format_type: "json_object".to_string(),
        }),
    };

    let mut req = client.post(&url).json(&request_body);

    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("AI API error ({}): {}", status, body).into());
    }

    let completion: ChatCompletionResponse = resp.json().await?;

    let raw_json = completion
        .choices
        .first()
        .ok_or("AI API returned no choices")?
        .message
        .content
        .trim()
        .to_string();

    // Parse the JSON response from the model
    let result: AiProcessedResult = serde_json::from_str(&raw_json)
        .map_err(|e| format!("Failed to parse AI response as JSON: {}. Raw: {}", e, raw_json))?;

    Ok(result)
}