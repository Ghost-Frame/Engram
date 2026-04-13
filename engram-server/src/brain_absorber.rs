/// Brain absorption helpers for the server layer.
///
/// Brain absorption requires Arc<dyn BrainBackend> and EmbeddingProvider,
/// both of which are only available via AppState. This module contains the
/// server-side logic that ports Eidolon's absorber.rs concepts into Engram.
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::RwLock;

use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::services::brain::{AbsorbMemoryData, BrainBackend};

/// Absorb a single activity event into the brain.
///
/// This is called fire-and-forget from the activity route after process_activity
/// succeeds. Never fails the caller -- all errors are logged as warnings.
///
/// - `memory_id`: The id of the just-stored memory row (used as brain memory id)
/// - `content`: Pre-formatted content string
/// - `category`: "task" for task.* actions, "activity" for others
/// - `importance`: 6 for completed, 7 for blocked/error, 4 otherwise
/// - `source`: The agent name from the activity report
pub async fn absorb_activity_to_brain(
    brain: Arc<dyn BrainBackend>,
    embedder: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    memory_id: i64,
    content: String,
    category: String,
    importance: f64,
    source: String,
) {
    if !brain.is_ready() {
        tracing::debug!("brain_absorber: brain not ready, skipping absorption");
        return;
    }

    let embedder_guard = embedder.read().await;
    let embedder_ref = match embedder_guard.as_ref() {
        Some(e) => e.clone(),
        None => {
            tracing::warn!("brain_absorber: embedder not ready, skipping absorption for memory {}", memory_id);
            return;
        }
    };
    drop(embedder_guard);

    let memory = AbsorbMemoryData {
        id: memory_id,
        content,
        category,
        source,
        importance,
        created_at: Utc::now().to_rfc3339(),
        tags: None,
    };

    match brain.absorb(embedder_ref.as_ref(), memory).await {
        Ok(()) => tracing::debug!("brain_absorber: absorbed activity memory id={}", memory_id),
        Err(e) => tracing::warn!("brain_absorber: brain absorb failed for memory {}: {}", memory_id, e),
    }
}

/// Absorb session completion data into the brain.
///
/// This is a portable adaptation of Eidolon's absorb_session function.
/// Callers are expected to extract the relevant data from session state
/// before calling this. All parameters are pre-processed strings.
///
/// - `session_short_id`: Short human-readable session identifier for logging
/// - `task`: Task description (will be truncated to 100 chars)
/// - `outcome`: One of "succeeded", "failed", "killed", "timed_out", "unknown"
/// - `agent`: Agent name
/// - `corrections`: Number of corrections applied during session
/// - `user_label`: Optional user label for category namespacing (e.g. "zan")
/// - `issue_lines`: Lines from output that represent blocked/gate issues (max 5 used)
/// - `discovery_lines`: Lines from output matching discovery keywords (max 10 used)
#[allow(clippy::too_many_arguments)]
pub async fn absorb_session_to_brain(
    brain: Arc<dyn BrainBackend>,
    embedder: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    session_short_id: String,
    task: String,
    outcome: &str,
    agent: String,
    corrections: u32,
    user_label: Option<String>,
    issue_lines: Vec<String>,
    discovery_lines: Vec<String>,
) {
    if !brain.is_ready() {
        tracing::debug!("brain_absorber: brain not ready, skipping session absorption");
        return;
    }

    let importance: f64 = if outcome == "succeeded" { 6.0 } else { 7.0 };
    let task_excerpt: String = task.chars().take(100).collect();

    let summary = format!(
        "Engram session ({}) for task \"{}\": {}. Agent: {}. Corrections: {}.",
        session_short_id, task_excerpt, outcome, agent, corrections,
    );

    let category_prefix = match user_label.as_deref() {
        Some(u) => format!("user:{}/", u),
        None => "system/".to_string(),
    };

    // 1. Absorb session summary
    absorb_one(
        &brain,
        &embedder,
        &summary,
        &format!("{}task", category_prefix),
        importance,
        "engram-server",
    )
    .await;

    // 2. Absorb gate blocks as strong correction signals (importance 8)
    for line in issue_lines.iter().take(5) {
        let block_content = format!("Gate blocked action in session {}: {}", session_short_id, line);
        absorb_one(
            &brain,
            &embedder,
            &block_content,
            &format!("{}issue", category_prefix),
            8.0,
            "engram-server",
        )
        .await;
    }

    // 3. Extract and absorb key discoveries (importance 5)
    let used_discoveries: Vec<&String> = discovery_lines.iter().take(10).collect();
    if !used_discoveries.is_empty() {
        let task_excerpt_short: String = task.chars().take(80).collect();
        let discoveries = used_discoveries
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let discovery_content = format!(
            "Session {} discoveries for task \"{}\": {}",
            session_short_id, task_excerpt_short, discoveries,
        );
        absorb_one(
            &brain,
            &embedder,
            &discovery_content,
            &format!("{}discovery", category_prefix),
            5.0,
            "engram-server",
        )
        .await;
        tracing::info!(
            "brain_absorber: absorbed {} discovery lines for session {}",
            used_discoveries.len(),
            session_short_id
        );
    }
}

/// Internal helper: absorb a single content string into the brain.
/// Best-effort -- logs warning on failure, never panics.
async fn absorb_one(
    brain: &Arc<dyn BrainBackend>,
    embedder: &Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    content: &str,
    category: &str,
    importance: f64,
    source: &str,
) {
    let embedder_guard = embedder.read().await;
    let embedder_ref = match embedder_guard.as_ref() {
        Some(e) => e.clone(),
        None => {
            tracing::warn!("brain_absorber: embedder not ready, skipping absorption of {:?}", &content[..content.len().min(60)]);
            return;
        }
    };
    drop(embedder_guard);

    // Generate a stable-ish id from content hash to avoid duplicate tracking
    let id = stable_id(content);

    let memory = AbsorbMemoryData {
        id,
        content: content.to_string(),
        category: category.to_string(),
        source: source.to_string(),
        importance,
        created_at: Utc::now().to_rfc3339(),
        tags: None,
    };

    match brain.absorb(embedder_ref.as_ref(), memory).await {
        Ok(()) => tracing::debug!("brain_absorber: absorbed id={} category={}", id, category),
        Err(e) => tracing::warn!("brain_absorber: brain absorb failed (category={}): {}", category, e),
    }
}

/// Generate a pseudo-unique i64 id from string content using a fast hash.
/// Using a simple FNV-1a-style fold to avoid pulling in uuid/rand here.
fn stable_id(content: &str) -> i64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Fold to positive i64
    (hash & 0x7fff_ffff_ffff_ffff) as i64
}
