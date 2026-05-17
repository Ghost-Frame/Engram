use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::ledger::Ledger;
use crate::tailer;
use crate::writer::KleosWriter;

pub async fn run(config: Config, ledger: Ledger, writer: KleosWriter, dry_run: bool) {
    // In dry-run mode, use an in-memory ledger so we don't advance offsets
    let ledger = if dry_run {
        tracing::info!("dry-run: using ephemeral in-memory ledger (offsets won't persist)");
        Ledger::open(Path::new(":memory:")).expect("in-memory ledger")
    } else {
        ledger
    };

    let config = Arc::new(config);
    let ledger = Arc::new(ledger);
    let writer = Arc::new(Mutex::new(writer));

    let mut file_count = 0;
    let mut files = Vec::new();
    collect_jsonl(&config.watch_dir, &mut files);

    for path in &files {
        file_count += 1;
        tailer::tail_file(
            path.clone(),
            Arc::clone(&config),
            Arc::clone(&ledger),
            Arc::clone(&writer),
            dry_run,
        )
        .await;
    }

    tracing::info!(files = file_count, dry_run, "one-shot pass complete");
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}
