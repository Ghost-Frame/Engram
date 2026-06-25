use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lru::LruCache;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::alert;
use crate::checks;
use crate::checks::retry_loop::RetryTracker;

/// Maximum number of distinct files whose RetryTracker state we keep in memory.
/// Entries are evicted LRU once this cap is reached, which is safe because a
/// file that has been quiet long enough to be evicted is unlikely to be mid-loop.
const RETRY_TRACKERS_CAPACITY: usize = 512;

// Cap the in-memory map of session-file -> read offset. Without this, the
// supervisor's heap grows linearly with the number of distinct session JSONL
// files it has ever seen across the lifetime of the process.
const POSITIONS_CAPACITY: usize = 2048;

pub struct SupervisorState {
    pub kleos_url: String,
    pub api_key: Option<String>,
    pub rules: Vec<checks::Rule>,
    pub cooldowns: RwLock<HashMap<String, chrono::DateTime<chrono::Utc>>>,
    pub client: reqwest::Client,
}

pub async fn run(state: Arc<SupervisorState>, watch_dir: PathBuf) {
    if !watch_dir.exists() {
        tracing::warn!(path = %watch_dir.display(), "watch dir missing, waiting for it");
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if watch_dir.exists() {
                break;
            }
        }
    }

    let (tx, mut rx) = mpsc::channel(100);

    let mut debouncer = new_debouncer(Duration::from_millis(500), move |res| {
        if let Ok(events) = res {
            for event in events {
                let _ = tx.blocking_send(event);
            }
        }
    })
    .expect("failed to create file watcher");

    debouncer
        .watcher()
        .watch(&watch_dir, RecursiveMode::Recursive)
        .expect("failed to watch directory");

    tracing::info!(path = %watch_dir.display(), "watching for session changes");

    let max_tracked: usize = std::env::var("EIDOLON_SUPERVISOR_MAX_TRACKED_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(POSITIONS_CAPACITY);
    let cap = NonZeroUsize::new(max_tracked).expect("non-zero capacity");
    let mut positions: LruCache<PathBuf, u64> = LruCache::new(cap);

    // ESUP-1: compile rule regexes exactly once before the event loop so they
    // are not rebuilt for every log entry.
    let compiled_rules = checks::rule_match::compile_rules(&state.rules);

    // ESUP-2: one RetryTracker per watched file so that commands from different
    // agents/sessions do not interleave in a single global deque.  The LRU cap
    // prevents unbounded growth when many short-lived session files are seen.
    let retry_cap = NonZeroUsize::new(RETRY_TRACKERS_CAPACITY).expect("non-zero capacity");
    let mut retry_trackers: LruCache<PathBuf, RetryTracker> = LruCache::new(retry_cap);

    while let Some(event) = rx.recv().await {
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

        if let Ok(entries) = read_new_entries(path, &mut positions) {
            for entry in entries {
                let session_id = extract_session_id(&entry);
                let mut violations = Vec::new();

                violations.extend(checks::rule_match::check(&entry, &compiled_rules));

                // Obtain (or insert) the per-file RetryTracker, then check.
                // `get_or_insert_mut` gives a mutable reference so that
                // RetryTracker can update its internal command history.
                let tracker =
                    retry_trackers.get_or_insert_mut(path.to_path_buf(), RetryTracker::new);
                violations.extend(tracker.check(&entry, &state.rules));

                for mut violation in violations {
                    if is_cooled_down(&state, &violation.rule_id).await {
                        continue;
                    }
                    violation.session_id = session_id.clone();

                    tracing::warn!(
                        rule = %violation.rule_id,
                        severity = ?violation.severity,
                        session_id = ?violation.session_id,
                        message = %violation.message,
                        "violation detected"
                    );

                    alert::send_alert(&state, &violation).await;
                    set_cooldown(&state, &violation.rule_id, &state.rules).await;
                }
            }
        }
    }
}

/// Pull a Claude session id out of a JSONL entry. Claude Code writes
/// `sessionId` (camelCase) on every event; some older transcripts use
/// `session_id`. Returns None if neither is present or non-string.
fn extract_session_id(entry: &serde_json::Value) -> Option<String> {
    let obj = entry.as_object()?;
    obj.get("sessionId")
        .or_else(|| obj.get("session_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn read_new_entries(
    path: &Path,
    positions: &mut LruCache<PathBuf, u64>,
) -> Result<Vec<serde_json::Value>, std::io::Error> {
    let path_buf = path.to_path_buf();
    // M-018: use peek() to avoid promoting the entry to MRU when only reading
    // position; the actual put() at the end promotes it.
    let last_pos = positions.peek(&path_buf).copied().unwrap_or(0);

    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let start_pos = if file_len < last_pos { 0 } else { last_pos };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_pos))?;

    let mut new_pos = start_pos;
    let mut entries = Vec::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        // Read raw bytes rather than reader.lines(): a non-UTF-8 byte makes
        // lines() yield Err, and the previous `break` left new_pos un-advanced
        // so every later poll re-hit the same bad byte and silently missed all
        // subsequent events (a monitoring blind spot).
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break; // EOF
        }
        // A chunk that is not newline-terminated is a partial mid-write line.
        // Leave new_pos before it so the completed line is read (and parsed) on
        // the next poll instead of being skipped (TOCTOU drop of a real event).
        if buf.last() != Some(&b'\n') {
            break;
        }
        new_pos += n as u64;

        // Decode lossily so a bad byte is skipped past (offset advanced) rather
        // than stalling the watcher forever.
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            let obj = value.as_object();
            let is_tool = obj
                .map(|o| {
                    o.contains_key("tool_name")
                        || o.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                })
                .unwrap_or(false);

            if is_tool {
                entries.push(value);
            }
        }
    }

    positions.put(path_buf, new_pos);
    Ok(entries)
}

async fn is_cooled_down(state: &SupervisorState, rule_id: &str) -> bool {
    let cooldowns = state.cooldowns.read().await;
    if let Some(last_fired) = cooldowns.get(rule_id) {
        let now = chrono::Utc::now();
        now < *last_fired
    } else {
        false
    }
}

async fn set_cooldown(state: &SupervisorState, rule_id: &str, rules: &[checks::Rule]) {
    let cooldown_secs = rules
        .iter()
        .find(|r| r.id == rule_id)
        .map(|r| r.cooldown_secs)
        .unwrap_or(60);

    let until = chrono::Utc::now() + chrono::Duration::seconds(cooldown_secs as i64);
    let mut cooldowns = state.cooldowns.write().await;
    cooldowns.insert(rule_id.to_string(), until);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_lru_bounded() {
        let mut positions: LruCache<PathBuf, u64> =
            LruCache::new(NonZeroUsize::new(POSITIONS_CAPACITY).unwrap());
        for i in 0..3000 {
            positions.put(PathBuf::from(format!("/tmp/session-{i}.jsonl")), i as u64);
        }
        assert!(positions.len() <= POSITIONS_CAPACITY);
        assert_eq!(positions.cap().get(), POSITIONS_CAPACITY);
    }
}
