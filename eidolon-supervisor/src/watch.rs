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

    // M-018: bound the positions map to prevent unbounded growth when many
    // files are watched. Default cap is 1024; operator can override.
    let max_tracked: usize = std::env::var("EIDOLON_SUPERVISOR_MAX_TRACKED_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(1024);
    let cap = NonZeroUsize::new(max_tracked).unwrap();
    let mut positions: LruCache<PathBuf, u64> = LruCache::new(cap);
    let mut retry_tracker = RetryTracker::new();

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
                let mut violations = Vec::new();

                violations.extend(checks::rule_match::check(&entry, &state.rules));
                violations.extend(retry_tracker.check(&entry, &state.rules));

                for violation in violations {
                    if is_cooled_down(&state, &violation.rule_id).await {
                        continue;
                    }

                    tracing::warn!(
                        rule = %violation.rule_id,
                        severity = ?violation.severity,
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

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        new_pos += line.len() as u64 + 1;

        if line.trim().is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            let obj = value.as_object();
            let is_tool = obj
                .map(|o| o.contains_key("tool_name") || o.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
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
