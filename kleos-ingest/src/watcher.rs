use notify::{Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};

use crate::config::Config;
use crate::ledger::Ledger;
use crate::summarizer;
use crate::tailer;
use crate::writer::KleosWriter;

pub async fn run(config: Config, ledger: Ledger, writer: KleosWriter) {
    let config = Arc::new(config);
    let ledger = Arc::new(ledger);
    let writer = Arc::new(Mutex::new(writer));

    let (tx, mut rx) = mpsc::channel::<PathBuf>(256);

    // File watcher
    let watch_dir = config.watch_dir.clone();
    let tx_clone = tx.clone();
    std::thread::spawn(move || {
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            for path in event.paths {
                                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                    let _ = tx_clone.blocking_send(path);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
            NotifyConfig::default(),
        ).expect("failed to create file watcher");

        if !watch_dir.exists() {
            std::fs::create_dir_all(&watch_dir).expect("failed to create watch directory");
        }
        watcher.watch(&watch_dir, RecursiveMode::Recursive)
            .expect("failed to watch directory");

        tracing::info!(dir = %watch_dir.display(), "file watcher started");

        // Keep the watcher alive
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    });

    // Track last activity per file for idle detection
    let mut last_activity: HashMap<PathBuf, Instant> = HashMap::new();

    // Idle check ticker
    let mut idle_interval = tokio::time::interval(Duration::from_secs(60));

    // Signal handler
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM");

    loop {
        tokio::select! {
            Some(path) = rx.recv() => {
                last_activity.insert(path.clone(), Instant::now());
                let config = Arc::clone(&config);
                let ledger = Arc::clone(&ledger);
                let writer = Arc::clone(&writer);
                tokio::spawn(async move {
                    tailer::tail_file(path, config, ledger, writer).await;
                });
            }
            _ = idle_interval.tick() => {
                let idle_threshold = Duration::from_secs(config.summary_idle_secs);
                let now = Instant::now();
                let mut to_summarize = Vec::new();
                for (path, last) in &last_activity {
                    if now.duration_since(*last) > idle_threshold {
                        let path_str = path.to_string_lossy().to_string();
                        if !ledger.is_summarized(&path_str) {
                            if let Some((project, session_id)) = tailer::parse_session_path(path) {
                                to_summarize.push((path.clone(), project, session_id));
                            }
                        }
                    }
                }
                for (path, project, session_id) in to_summarize {
                    let path_str = path.to_string_lossy().to_string();
                    summarizer::summarize_session(
                        &session_id, &project, &config, &ledger, Arc::clone(&writer)
                    ).await;
                    ledger.mark_summarized(&path_str);
                    last_activity.remove(&path);
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
                break;
            }
        }
    }
}
