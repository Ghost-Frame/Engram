//! File watcher for Claude Code session JSONL files.
//! Monitors ~/.claude/projects/*/sessions/*.jsonl for changes,
//! extracts assistant text turns, feeds them through the LLM quality gate,
//! and stores only curated memories to Kleos.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::gate::{GateResult, MemoryGate, PendingTurn};
use crate::SidecarState;

type FilePositions = Arc<RwLock<HashMap<PathBuf, u64>>>;

const CHECKPOINT_FLUSH_EVERY: usize = 10;

/// How long to wait after the last file event before flushing the pending batch
/// through the LLM gate. Gives rapid successive writes time to accumulate.
const BATCH_IDLE_SECS: u64 = 5;

/// Flush when pending turns exceed this count regardless of idle time.
const BATCH_MAX_PENDING: usize = 20;

fn load_checkpoint(path: &Path) -> HashMap<PathBuf, u64> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<HashMap<PathBuf, u64>>(&text) {
            Ok(map) => {
                tracing::debug!(path = %path.display(), entries = map.len(), "loaded watcher checkpoint");
                map
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "watcher checkpoint corrupt, starting empty");
                HashMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cannot read watcher checkpoint, starting empty");
            HashMap::new()
        }
    }
}

pub fn flush_checkpoint(path: &Path, positions: &HashMap<PathBuf, u64>) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "watcher checkpoint: could not create parent dir");
            return;
        }
    }

    let tmp = path.with_extension("tmp");
    match serde_json::to_string(positions) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&tmp, &json) {
                tracing::warn!(error = %e, "watcher checkpoint: write tmp failed");
                return;
            }
            if let Err(e) = std::fs::rename(&tmp, path) {
                tracing::warn!(error = %e, "watcher checkpoint: rename failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "watcher checkpoint: serialize failed");
        }
    }
}

pub fn checkpoint_path() -> PathBuf {
    if let Ok(p) = std::env::var("ENGRAM_SIDECAR_WATCHER_STATE_PATH") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kleos")
        .join("sidecar-watcher-state.json")
}

pub fn start(state: SidecarState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_watcher(state).await {
            tracing::error!(error = %e, "file watcher failed");
        }
    })
}

async fn run_watcher(state: SidecarState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let watch_dir = get_watch_dir();

    if !watch_dir.exists() {
        tracing::warn!(path = %watch_dir.display(), "watch directory does not exist, waiting...");
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if watch_dir.exists() {
                tracing::info!(path = %watch_dir.display(), "watch directory appeared");
                break;
            }
        }
    }

    let gate = match state.llm.as_ref() {
        Some(llm) => Arc::new(MemoryGate::new(
            Arc::clone(llm),
            state.gate_model.clone().or_else(|| state.compress_model.clone()),
        )),
        None => {
            tracing::warn!("watcher: no LLM available, gate disabled -- watcher will not store memories");
            return Ok(());
        }
    };

    let cp_path = checkpoint_path();
    let initial = load_checkpoint(&cp_path);
    let positions: FilePositions = Arc::new(RwLock::new(initial));

    let (tx, mut rx) = mpsc::channel(100);

    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Ok(events) = res {
                for event in events {
                    let _ = tx.blocking_send(event.clone());
                }
            }
        },
    )?;

    debouncer
        .watcher()
        .watch(&watch_dir, RecursiveMode::Recursive)?;
    tracing::info!(path = %watch_dir.display(), "file watcher started (LLM gate enabled)");

    let mut parse_count: usize = 0;
    let mut pending_turns: Vec<PendingTurn> = Vec::new();
    let mut batch_started: Option<Instant> = None;

    loop {
        let timeout = Duration::from_secs(BATCH_IDLE_SECS);
        let event = tokio::time::timeout(timeout, rx.recv()).await;

        match event {
            Ok(Some(event)) => {
                if event.kind != DebouncedEventKind::Any {
                    continue;
                }

                let path = &event.path;
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if !path.exists() {
                    continue;
                }

                tracing::debug!(path = %path.display(), "processing changed jsonl file");

                match extract_turns_from_file(path, &positions).await {
                    Ok(turns) => {
                        let count = turns.len();
                        if count > 0 {
                            if batch_started.is_none() {
                                batch_started = Some(Instant::now());
                            }
                            pending_turns.extend(turns);
                            parse_count += count;
                            if parse_count >= CHECKPOINT_FLUSH_EVERY {
                                let map = positions.read().await;
                                flush_checkpoint(&cp_path, &map);
                                parse_count = 0;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to extract turns");
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {} // timeout, handled below
        }

        // Flush when: batch is large enough OR batch has been pending long enough
        let should_flush = if pending_turns.is_empty() {
            false
        } else if pending_turns.len() >= BATCH_MAX_PENDING {
            true
        } else if let Some(started) = batch_started {
            started.elapsed() >= Duration::from_secs(BATCH_IDLE_SECS)
        } else {
            false
        };

        if should_flush {
            let batch = std::mem::take(&mut pending_turns);
            batch_started = None;
            let batch_len = batch.len();
            tracing::info!(turns = batch_len, "flushing batch through LLM gate");

            let results = gate.evaluate_batch(batch).await;
            let stored = store_gate_results(&results, &state).await;

            tracing::info!(
                evaluated = batch_len,
                stored,
                skipped = batch_len - stored,
                "gate batch complete"
            );
        }
    }

    // Final flush on exit
    if !pending_turns.is_empty() {
        let batch = std::mem::take(&mut pending_turns);
        let results = gate.evaluate_batch(batch).await;
        store_gate_results(&results, &state).await;
    }

    let map = positions.read().await;
    flush_checkpoint(&cp_path, &map);

    Ok(())
}

fn get_watch_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_SESSIONS_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Parse project and session_id from a path like:
/// ~/.claude/projects/<proj-hash>/sessions/<session-id>.jsonl
fn parse_session_path(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let mut ancestors = path.ancestors();
    ancestors.next(); // the file itself
    let _sessions_dir = ancestors.next()?; // sessions/
    let project_dir = ancestors.next()?; // <project-hash>/
    let project = project_dir.file_name()?.to_str()?.to_string();
    Some((project, stem))
}

async fn extract_turns_from_file(
    path: &Path,
    positions: &FilePositions,
) -> Result<Vec<PendingTurn>, Box<dyn std::error::Error + Send + Sync>> {
    let path_buf = path.to_path_buf();

    let (project, session_id) = parse_session_path(path)
        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

    let last_pos = {
        let pos_map = positions.read().await;
        pos_map.get(&path_buf).copied().unwrap_or(0)
    };

    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let start_pos = if file_len < last_pos { 0 } else { last_pos };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_pos))?;

    let mut new_pos = start_pos;
    let mut turns = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read line");
                break;
            }
        };

        new_pos += line.len() as u64 + 1;

        if line.trim().is_empty() {
            continue;
        }

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(text) = extract_assistant_text(&parsed) {
                if text.len() > 50 {
                    turns.push(PendingTurn {
                        text,
                        session_id: session_id.clone(),
                        project: project.clone(),
                    });
                }
            }
        }
    }

    {
        let mut pos_map = positions.write().await;
        pos_map.insert(path_buf, new_pos);
    }

    Ok(turns)
}

/// Extract assistant text content from a JSONL line.
/// Only takes assistant messages -- user messages and tool_use entries are skipped.
fn extract_assistant_text(value: &serde_json::Value) -> Option<String> {
    let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

    // Only process assistant messages
    if msg_type != "assistant" {
        if let Some(role) = value.get("role").and_then(|r| r.as_str()) {
            if role != "assistant" {
                return None;
            }
        } else if msg_type != "" {
            return None;
        }
    }

    // Shape 1: {"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}]}}
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
                if text.len() > 50 {
                    return Some(text.to_string());
                }
            }
        }
    }

    // Shape 2: {"role": "assistant", "content": "..."}
    if let Some(content) = value.get("content") {
        if let Some(text) = content.as_str() {
            if text.len() > 50 {
                return Some(text.to_string());
            }
        }
    }

    None
}

/// Store gate-approved results to Kleos. Returns count stored.
async fn store_gate_results(results: &[GateResult], state: &SidecarState) -> usize {
    let url = format!("{}/store", state.kleos_url);
    if let Err(e) = kleos_lib::net::validate_outbound_url(&url) {
        tracing::warn!(
            kleos_url = %state.kleos_url,
            error = %e,
            "watcher store: kleos_url failed outbound validation; dropping batch"
        );
        return 0;
    }

    let mut stored = 0;

    for result in results {
        if !result.verdict.store {
            continue;
        }

        let category = result
            .verdict
            .category
            .as_deref()
            .unwrap_or("session");

        let importance = result.verdict.importance.unwrap_or(3);

        let content = result
            .verdict
            .summary
            .as_deref()
            .unwrap_or(&result.original_text);

        let req = serde_json::json!({
            "content": content,
            "category": category,
            "source": format!("sidecar-gate:{}", result.project),
            "importance": importance,
            "tags": ["sidecar-gate", &result.project, &result.session_id],
            "user_id": state.user_id,
        });

        let mut request = state.client.post(&url).json(&req);
        if let Some(ref api_key) = state.kleos_api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(
                    category,
                    importance,
                    session = %result.session_id,
                    "gate-approved memory stored"
                );
                stored += 1;
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "watcher store failed");
            }
            Err(e) => {
                tracing::warn!(error = %e, "watcher store request failed");
            }
        }
    }

    stored
}
