use crate::config::Config;
use crate::ledger::Ledger;
use crate::writer::KleosWriter;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn summarize_session(
    session_id: &str,
    project: &str,
    config: &Config,
    ledger: &Ledger,
    writer: Arc<Mutex<KleosWriter>>,
) {
    tracing::info!(session = %session_id, project = %project, "generating session summary");

    let summary = format!(
        "Session summary (auto-generated)\n\
         Project: {}\n\
         Host: {}\n\
         Session: {}\n\
         Source: kleos-ingest rule-based summarizer",
        project, config.host, session_id
    );

    let mut w = writer.lock().await;
    if w.store_summary(&summary, session_id, project).await {
        tracing::info!(session = %session_id, "session summary stored");
    }

    // Try to invoke kleos-cli handoff dump as well
    drop(w);
    let handoff_result = tokio::process::Command::new("kleos-cli")
        .args([
            "handoff", "dump",
            "--handoff-type", "auto",
            "--content", &summary,
            "--session", session_id,
            "--project", project,
        ])
        .output()
        .await;

    match handoff_result {
        Ok(output) if output.status.success() => {
            tracing::info!(session = %session_id, "handoff dump created");
        }
        Ok(output) => {
            tracing::warn!(
                session = %session_id,
                stderr = %String::from_utf8_lossy(&output.stderr),
                "handoff dump failed"
            );
        }
        Err(e) => {
            tracing::warn!(session = %session_id, error = %e, "kleos-cli not available for handoff");
        }
    }
}
