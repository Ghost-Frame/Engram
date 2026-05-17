use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::extractor::Extractor;
use crate::ledger::Ledger;
use crate::writer::KleosWriter;

/// Parse project and session_id from a path like:
/// ~/.claude/projects/<proj>/sessions/<session-id>.jsonl
pub fn parse_session_path(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let mut ancestors = path.ancestors();
    ancestors.next(); // the file itself
    let sessions_dir = ancestors.next()?; // sessions/
    if sessions_dir.file_name()?.to_str()? != "sessions" {
        // Might be directly under the project dir -- we still proceed
    }
    let project_dir = ancestors.next()?; // <project-hash>/
    let project = project_dir.file_name()?.to_str()?.to_string();
    Some((project, stem))
}

pub async fn tail_file(
    path: PathBuf,
    _config: Arc<Config>,
    ledger: Arc<Ledger>,
    writer: Arc<Mutex<KleosWriter>>,
    dry_run: bool,
) {
    let path_str = path.to_string_lossy().to_string();
    let (project, session_id) = match parse_session_path(&path) {
        Some(ps) => ps,
        None => {
            tracing::warn!(path = %path_str, "could not parse project/session from path");
            return;
        }
    };

    let mut extractor = Extractor::new();
    let offset = ledger.get_offset(&path_str);

    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %path_str, error = %e, "failed to open file");
            return;
        }
    };

    let mut reader = BufReader::new(file);
    if offset > 0 {
        if let Err(e) = reader.seek(SeekFrom::Start(offset as u64)) {
            tracing::warn!(path = %path_str, error = %e, "failed to seek");
            return;
        }
    }

    let mut current_offset = offset;
    let mut line = String::new();
    let mut memories_this_pass = 0;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(n) => {
                current_offset += n as i64;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    let content = extract_content(&parsed);
                    if let Some(text) = content {
                        if let Some(candidate) = extractor.extract(&text, &session_id, &project) {
                            if dry_run {
                                let preview: String = candidate.content.chars().take(120).collect();
                                tracing::info!(
                                    category = %candidate.category,
                                    importance = candidate.importance,
                                    tags = ?candidate.tags,
                                    session = %candidate.session_id,
                                    "[DRY-RUN] would store: {preview}"
                                );
                                memories_this_pass += 1;
                            } else {
                                let mut w = writer.lock().await;
                                if w.store(candidate).await {
                                    ledger.increment_memories(&session_id);
                                    memories_this_pass += 1;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(path = %path_str, error = %e, "read error");
                break;
            }
        }
    }

    ledger.set_offset(&path_str, current_offset, &project, &session_id);

    if memories_this_pass > 0 {
        tracing::info!(
            path = %path_str,
            memories = memories_this_pass,
            offset = current_offset,
            "tail pass complete"
        );
    }
}

/// Extract readable content from a JSONL line.
/// Claude Code session files have varied formats -- try common shapes.
fn extract_content(value: &serde_json::Value) -> Option<String> {
    // Shape 1: {"type": "assistant", "message": {"content": [{"text": "..."}]}}
    if let Some(msg) = value.get("message") {
        if let Some(content) = msg.get("content") {
            if let Some(arr) = content.as_array() {
                let texts: Vec<&str> = arr
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            block.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !texts.is_empty() {
                    return Some(texts.join("\n"));
                }
            }
            if let Some(text) = content.as_str() {
                return Some(text.to_string());
            }
        }
    }

    // Shape 2: {"role": "assistant", "content": "..."}
    if let Some(content) = value.get("content") {
        if let Some(text) = content.as_str() {
            if text.len() > 20 {
                return Some(text.to_string());
            }
        }
    }

    // Shape 3: {"type": "user", "message": {"content": "..."}}
    if value.get("type").and_then(|t| t.as_str()) == Some("user") {
        if let Some(msg) = value.get("message") {
            if let Some(content) = msg.get("content") {
                if let Some(text) = content.as_str() {
                    return Some(text.to_string());
                }
            }
        }
    }

    None
}
