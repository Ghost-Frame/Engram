//! File watcher for Claude Code session JSONL files.
//! Monitors ~/.claude/projects/*/*.jsonl for changes,
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

/// Per-file byte offset map, persisted so the watcher resumes from where it
/// left off after a restart instead of re-extracting historic turns.
type FilePositions = Arc<RwLock<HashMap<PathBuf, u64>>>;

const CHECKPOINT_FLUSH_EVERY: usize = 10;

/// How long to wait after the last file event before flushing the pending batch
/// through the LLM gate. Gives rapid successive writes time to accumulate.
const BATCH_IDLE_SECS: u64 = 5;

/// Flush when pending turns exceed this count regardless of idle time.
const BATCH_MAX_PENDING: usize = 20;

/// Hard cap on how many turns a single file-extract pass will emit.
/// Defends against giant files (recovered state, fresh attach, etc.)
/// queueing hundreds of LLM calls in one batch.
///
/// Default tuned for modest GPU/CPU; faster hardware can raise it via
/// `KLEOS_SIDECAR_MAX_TURNS_PER_EXTRACT`, slower hardware can lower it.
const MAX_TURNS_PER_EXTRACT_DEFAULT: usize = 10;

/// Minimum delay between gate LLM calls. Spaces out GPU work so a
/// long batch can't pin the device at 100% for minutes on end.
///
/// Default tuned for modest GPU/CPU; override with
/// `KLEOS_SIDECAR_GATE_PACE_MS` (set to `0` to disable pacing).
const GATE_PACE_MS_DEFAULT: u64 = 1500;

/// Resolve the per-pass turn cap from `KLEOS_SIDECAR_MAX_TURNS_PER_EXTRACT`,
/// falling back to `MAX_TURNS_PER_EXTRACT_DEFAULT` when unset or unparseable.
fn max_turns_per_extract() -> usize {
    std::env::var("KLEOS_SIDECAR_MAX_TURNS_PER_EXTRACT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MAX_TURNS_PER_EXTRACT_DEFAULT)
}

/// Resolve the inter-call gate pacing from `KLEOS_SIDECAR_GATE_PACE_MS`,
/// falling back to `GATE_PACE_MS_DEFAULT` when unset or unparseable.
fn gate_pace_ms() -> u64 {
    std::env::var("KLEOS_SIDECAR_GATE_PACE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(GATE_PACE_MS_DEFAULT)
}

/// Read the watcher checkpoint JSON from `path`, returning an empty map if the
/// file is missing or corrupt so the watcher can keep running without history.
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

/// Atomically write the current per-file positions to `path` via temp-then-rename
/// so a crash mid-write cannot leave a partial checkpoint.
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

/// Resolve the on-disk path for the watcher checkpoint. Honours
/// `ENGRAM_SIDECAR_WATCHER_STATE_PATH` if set; otherwise defaults to
/// `~/.kleos/sidecar-watcher-state.json`.
pub fn checkpoint_path() -> PathBuf {
    if let Ok(p) = kleos_lib::kleos_env("SIDECAR_WATCHER_STATE_PATH") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kleos")
        .join("sidecar-watcher-state.json")
}

/// Spawn the watcher loop on the Tokio runtime. Returns the join handle so the
/// caller can await shutdown or detach.
pub fn start(state: SidecarState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_watcher(state).await {
            tracing::error!(error = %e, "file watcher failed");
        }
    })
}

/// Main watcher loop: subscribes to JSONL file changes via notify, batches new
/// turns, feeds them through the LLM gate, and stores curated memories to Kleos.
/// Returns Ok on graceful shutdown; propagates any fatal initialisation errors.
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
            state
                .gate_model
                .clone()
                .or_else(|| state.compress_model.clone()),
            gate_pace_ms(),
        )),
        None => {
            tracing::warn!(
                "watcher: no LLM available, gate disabled -- watcher will not store memories"
            );
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

/// Resolve the directory that contains Claude Code session JSONL files.
/// Honours `CLAUDE_SESSIONS_DIR`; otherwise defaults to `~/.claude/projects`.
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
/// ~/.claude/projects/<proj-hash>/<session-id>.jsonl
fn parse_session_path(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let project_dir = path.parent()?;
    let project = project_dir.file_name()?.to_str()?.to_string();
    Some((project, stem))
}

/// Read new assistant turns from `path` starting at the saved offset, advance
/// the offset, and return up to the env-resolved per-pass cap (see
/// `max_turns_per_extract`). First-seen files are checkpointed at EOF so
/// historic content is not replayed.
async fn extract_turns_from_file(
    path: &Path,
    positions: &FilePositions,
) -> Result<Vec<PendingTurn>, Box<dyn std::error::Error + Send + Sync>> {
    let path_buf = path.to_path_buf();

    let (project, session_id) =
        parse_session_path(path).unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

    let file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // First-time-seen files: skip to EOF. We do NOT replay history; only new
    // turns written after the sidecar starts get evaluated. This is the
    // critical guardrail against GPU-melting backfills.
    let last_pos = {
        let pos_map = positions.read().await;
        pos_map.get(&path_buf).copied()
    };
    let start_pos = match last_pos {
        Some(p) if p <= file_len => p,
        Some(_) => 0, // file was truncated, restart
        None => {
            // Unknown file: persist EOF as the starting point and skip parsing.
            let mut pos_map = positions.write().await;
            pos_map.insert(path_buf, file_len);
            return Ok(Vec::new());
        }
    };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_pos))?;

    let mut new_pos = start_pos;
    let mut turns = Vec::new();
    // Resolve the per-pass cap once per extract so env changes take effect on
    // the next file event without restarting the watcher.
    let max_turns = max_turns_per_extract();

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
                    if turns.len() >= max_turns {
                        // Don't advance past this line; we'll resume here next event.
                        break;
                    }
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
        } else if !msg_type.is_empty() {
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

        let category = result.verdict.category.as_deref().unwrap_or("session");

        let importance = result.verdict.importance.unwrap_or(3);

        // Store only the gate's distilled summary. Appending the raw assistant
        // turn (the prior behavior) dominated the FTS/embedding indexes with
        // uncurated narration, so recall matched conversational filler instead
        // of facts. A store=true verdict with no usable summary is a gate
        // contract violation, so skip it rather than fall back to the raw turn,
        // which would reintroduce exactly the noise the gate exists to remove.
        let content = match result.verdict.summary.as_deref() {
            Some(summary) if !summary.trim().is_empty() => summary.trim(),
            _ => {
                tracing::debug!(
                    session = %result.session_id,
                    "gate verdict store=true with empty summary; skipping to avoid raw-turn ingestion"
                );
                continue;
            }
        };

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
