use crate::ledger::Ledger;
use crate::writer::KleosWriter;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Extract a condensed transcript from the JSONL file.
/// Returns user prompts and short assistant text snippets, capped to ~3000 chars
/// to fit in Ollama's context window reasonably.
fn extract_transcript(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut parts: Vec<String> = Vec::new();
    let mut total_len = 0;
    const MAX_TRANSCRIPT: usize = 3000;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match msg_type {
            "user" => {
                // Only grab actual user text prompts, not tool_result arrays
                if let Some(content) = parsed.pointer("/message/content").and_then(|c| c.as_str()) {
                    if content.len() > 10 {
                        let snippet: String = content.chars().take(300).collect();
                        let entry = format!("USER: {}", snippet);
                        total_len += entry.len();
                        parts.push(entry);
                    }
                }
            }
            "assistant" => {
                // Extract text blocks from assistant content array
                if let Some(blocks) = parsed
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                if text.len() > 20 {
                                    let snippet: String = text.chars().take(200).collect();
                                    let entry = format!("ASSISTANT: {}", snippet);
                                    total_len += entry.len();
                                    parts.push(entry);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if total_len > MAX_TRANSCRIPT {
            break;
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\n"))
}

/// Call Ollama to summarize the transcript
async fn summarize_with_ollama(transcript: &str, project: &str) -> Option<String> {
    let ollama_url = std::env::var("KLEOS_INGEST_OLLAMA_URL")
        .or_else(|_| std::env::var("OLLAMA_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:11434/v1/chat/completions".to_string());

    let model = std::env::var("KLEOS_INGEST_SUMMARY_MODEL")
        .or_else(|_| std::env::var("OLLAMA_MODEL"))
        .unwrap_or_else(|_| "qwen2.5:3b".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .ok()?;

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a session summarizer. Given a transcript of a coding session, produce a concise summary (2-5 sentences) of what was accomplished, what decisions were made, and any unfinished work. Focus on outcomes and decisions, not process. Do not use markdown formatting. Be direct and factual."
            },
            {
                "role": "user",
                "content": format!("Summarize this coding session (project: {}):\n\n{}", project, transcript)
            }
        ],
        "temperature": 0.1,
        "max_tokens": 300,
        "stream": false
    });

    let resp = client
        .post(&ollama_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "ollama summarization failed");
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    json.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
}

pub async fn summarize_session(
    session_id: &str,
    project: &str,
    session_path: &Path,
    _ledger: &Ledger,
    writer: Arc<Mutex<KleosWriter>>,
) {
    tracing::info!(session = %session_id, project = %project, "generating session summary");

    // Use the watched file's actual path rather than reconstructing it: the
    // canonical layout is <watch_dir>/<project>/sessions/<id>.jsonl, and a
    // reconstruction that dropped the `sessions/` segment silently skipped
    // every summary (the file open failed).
    let transcript = match extract_transcript(session_path) {
        Some(t) => t,
        None => {
            tracing::debug!(session = %session_id, "no extractable content, skipping summary");
            return;
        }
    };

    // If transcript is too short, not worth summarizing
    if transcript.len() < 100 {
        tracing::debug!(session = %session_id, "transcript too short, skipping summary");
        return;
    }

    let summary = match summarize_with_ollama(&transcript, project).await {
        Some(s) => s,
        None => {
            tracing::warn!(session = %session_id, "ollama summarization failed, skipping");
            return;
        }
    };

    if summary.is_empty() {
        return;
    }

    let mut w = writer.lock().await;
    if w.store_summary(&summary, session_id, project).await {
        tracing::info!(session = %session_id, "session summary stored");
    }
}
