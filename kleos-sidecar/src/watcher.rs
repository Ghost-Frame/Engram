//! File watcher for Claude Code session JSONL files.
//! Monitors ~/.claude/projects/*/sessions/*.jsonl for changes,
//! parses new entries, and stores condensed summaries to Engram.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::SidecarState;

/// Tracks file read positions to only process new content.
type FilePositions = Arc<RwLock<HashMap<PathBuf, u64>>>;

/// Flush the in-memory position map to the checkpoint file every N successful parses.
const CHECKPOINT_FLUSH_EVERY: usize = 10;

/// Load checkpoint from disk. Missing or unreadable file is logged and treated as empty.
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

/// Atomically write the position map to disk via tempfile + rename.
pub fn flush_checkpoint(path: &Path, positions: &HashMap<PathBuf, u64>) {
    // Ensure parent directory exists.
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

/// Resolve the checkpoint path: env var > default.
pub fn checkpoint_path() -> PathBuf {
    if let Ok(p) = std::env::var("ENGRAM_SIDECAR_WATCHER_STATE_PATH") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kleos")
        .join("sidecar-watcher-state.json")
}

/// Start the file watcher in a background task.
/// Returns a JoinHandle that can be used to await completion.
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
        // Wait for directory to appear
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if watch_dir.exists() {
                tracing::info!(path = %watch_dir.display(), "watch directory appeared");
                break;
            }
        }
    }

    // Load persisted positions so restarts don't re-ingest already-processed lines.
    let cp_path = checkpoint_path();
    let initial = load_checkpoint(&cp_path);
    let positions: FilePositions = Arc::new(RwLock::new(initial));

    // Channel for debounced events
    let (tx, mut rx) = mpsc::channel(100);

    // Create debouncer (500ms debounce to batch rapid writes)
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Ok(events) = res {
                for event in events {
                    let _ = tx.blocking_send(event);
                }
            }
        },
    )?;

    debouncer
        .watcher()
        .watch(&watch_dir, RecursiveMode::Recursive)?;
    tracing::info!(path = %watch_dir.display(), "file watcher started");

    let mut parse_count: usize = 0;

    // Process events
    while let Some(event) = rx.recv().await {
        if event.kind != DebouncedEventKind::Any {
            continue;
        }

        let path = &event.path;

        // Only process .jsonl files
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Skip if file doesn't exist (deleted)
        if !path.exists() {
            continue;
        }

        tracing::debug!(path = %path.display(), "processing changed file");

        match process_file(path, &positions, &state).await {
            Ok(parsed) => {
                if parsed > 0 {
                    parse_count += parsed;
                    if parse_count >= CHECKPOINT_FLUSH_EVERY {
                        let map = positions.read().await;
                        flush_checkpoint(&cp_path, &map);
                        parse_count = 0;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to process file");
            }
        }
    }

    // Best-effort checkpoint flush on normal watcher exit.
    {
        let map = positions.read().await;
        flush_checkpoint(&cp_path, &map);
    }

    Ok(())
}

fn get_watch_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_SESSIONS_DIR") {
        return PathBuf::from(dir);
    }

    // Default: ~/.claude/projects
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Returns the number of successfully parsed observations (used by caller for checkpoint
/// flush cadence). Position is always updated even when no observations were extracted.
async fn process_file(
    path: &Path,
    positions: &FilePositions,
    state: &SidecarState,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let path_buf = path.to_path_buf();

    // Get last read position
    let last_pos = {
        let pos_map = positions.read().await;
        pos_map.get(&path_buf).copied().unwrap_or(0)
    };

    let file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // If file is smaller than last position, it was truncated - start from beginning
    let start_pos = if file_len < last_pos { 0 } else { last_pos };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_pos))?;

    let mut new_pos = start_pos;
    let mut observations = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read line");
                break;
            }
        };

        new_pos += line.len() as u64 + 1; // +1 for newline

        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON and extract relevant info
        if let Some(obs) = parse_jsonl_entry(&line) {
            observations.push(obs);
        }
    }

    // Update position
    {
        let mut pos_map = positions.write().await;
        pos_map.insert(path_buf, new_pos);
    }

    let parsed = observations.len();

    // Store observations
    if !observations.is_empty() {
        tracing::debug!(count = observations.len(), "storing observations from file");
        store_observations(observations, state).await;
    }

    Ok(parsed)
}

struct FileObservation {
    tool_name: String,
    content: String,
    importance: i32,
}

fn parse_jsonl_entry(line: &str) -> Option<FileObservation> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;

    // Claude Code JSONL format varies, handle common patterns
    let obj = json.as_object()?;

    // Skip user messages, focus on assistant tool use
    let msg_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Look for tool_use entries
    if msg_type == "tool_use" || obj.contains_key("tool_name") {
        let tool_name = obj
            .get("tool_name")
            .or_else(|| obj.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Skip common low-value tools
        if matches!(tool_name.as_str(), "Glob" | "Grep" | "LS") {
            return None;
        }

        // Extract content summary
        let input = obj.get("tool_input").or_else(|| obj.get("input"));

        let content = if let Some(input) = input {
            summarize_input(&tool_name, input)
        } else {
            format!("Tool: {}", tool_name)
        };

        // Importance based on tool type
        let importance = match tool_name.as_str() {
            "Edit" | "Write" => 4,
            "Bash" | "PowerShell" => 3,
            "Read" => 2,
            _ => 2,
        };

        return Some(FileObservation {
            tool_name,
            content,
            importance,
        });
    }

    None
}

fn summarize_input(tool_name: &str, input: &serde_json::Value) -> String {
    let obj = match input.as_object() {
        Some(o) => o,
        None => return format!("Tool: {}", tool_name),
    };

    // Extract key fields based on tool type
    match tool_name {
        "Read" | "Edit" | "Write" => {
            let path = obj
                .get("file_path")
                .or_else(|| obj.get("filePath"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("{}: {}", tool_name, path)
        }
        "Bash" | "PowerShell" => {
            let cmd = obj
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .take(100)
                .collect::<String>();
            format!("{}: {}", tool_name, cmd)
        }
        "Agent" => {
            let desc = obj
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            format!("Agent: {}", desc)
        }
        _ => {
            // Generic: just list keys
            let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).take(3).collect();
            format!("{}: {}", tool_name, keys.join(", "))
        }
    }
}

async fn store_observations(observations: Vec<FileObservation>, state: &SidecarState) {
    // R8 S-003: validate kleos_url once per batch so a misconfigured value
    // cannot redirect observations to an arbitrary host. Sidecar runs in a
    // trusted env today, but the CLI will expose --kleos-url so we harden
    // proactively.
    let url = format!("{}/memory/store", state.kleos_url);
    if let Err(e) = kleos_lib::net::validate_outbound_url(&url) {
        tracing::warn!(
            kleos_url = %state.kleos_url,
            error = %e,
            "file-watcher store: kleos_url failed outbound validation; dropping batch"
        );
        return;
    }
    for obs in observations {
        let req = serde_json::json!({
            "content": format!("[file-watcher] [{}] {}", obs.tool_name, obs.content),
            "category": "session",
            "source": state.source.clone(),
            "importance": obs.importance,
            "tags": ["file-watcher", obs.tool_name.clone()],
            "user_id": state.user_id,
        });

        let mut request = state.client.post(&url).json(&req);
        if let Some(ref api_key) = state.kleos_api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(tool = %obs.tool_name, "file-watcher observation stored");
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "file-watcher store failed");
            }
            Err(e) => {
                tracing::warn!(error = %e, "file-watcher store request failed");
            }
        }
    }
}
