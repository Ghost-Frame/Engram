//! Background dreamer: runs the intelligence consolidation pipeline and the
//! brain dream cycle on a configurable interval for every active user.
//!
//! Ported parity from the standalone eidolon-daemon, minus the ceremonial
//! idle gating. Runs unconditionally each tick so that merge/prune/discover
//! observably fire on a changing corpus.

use kleos_lib::config::Config;
use kleos_lib::db::Database;
use kleos_lib::intelligence::growth;
use kleos_lib::intelligence::scheduler::default_pipeline;
use kleos_lib::intelligence::types::GrowthReflectRequest;
use kleos_lib::services::brain::BrainBackend;
use kleos_lib::EngError;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Probability (0.0..1.0) that a dream cycle also triggers a growth reflection
/// for each user. Matches eidolon-daemon's 20% probabilistic hook.
const GROWTH_REFLECT_CHANCE: f64 = 0.2;
/// Number of recent memory contents to feed growth::reflect as context.
const GROWTH_CONTEXT_SIZE: usize = 20;

#[derive(Debug, Clone, Default, Serialize)]
pub struct DreamerStats {
    pub running: bool,
    pub cycles_total: u64,
    pub cycles_skipped_busy: u64,
    pub cycles_failed: u64,
    pub last_cycle_started_at: Option<String>,
    pub last_cycle_duration_ms: u64,
    pub last_users_processed: usize,
    pub last_pipeline_ok: usize,
    pub last_pipeline_failed: usize,
    pub last_brain_result: Option<Value>,
    pub last_pipeline_report: Option<Value>,
    pub totals: DreamerTotals,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DreamerTotals {
    pub pipeline_ok: u64,
    pub pipeline_failed: u64,
    pub brain_cycles: u64,
    pub brain_errors: u64,
    pub evolution_trainings: u64,
    pub growth_reflections: u64,
    pub growth_observations_stored: u64,
}

pub type DreamerStatsHandle = Arc<RwLock<DreamerStats>>;

pub fn new_stats_handle() -> DreamerStatsHandle {
    Arc::new(RwLock::new(DreamerStats::default()))
}

async fn active_user_ids(db: &Database) -> Result<Vec<i64>, EngError> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM users ORDER BY id")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub fn start_dreamer_task(
    db: Arc<Database>,
    config: Arc<Config>,
    brain: Option<Arc<dyn BrainBackend>>,
    stats: DreamerStatsHandle,
    last_request_time: Arc<AtomicU64>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let interval_secs = config.dream_interval_secs.max(30);
    let interval = Duration::from_secs(interval_secs);
    let idle_threshold = config.dream_idle_threshold_secs;

    let handle = tokio::spawn(async move {
        info!(interval_secs, idle_threshold, "dreamer task started");
        {
            let mut s = stats.write().await;
            s.running = true;
        }

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("dreamer task shutting down");
                    let mut s = stats.write().await;
                    s.running = false;
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    if !is_idle(&last_request_time, idle_threshold) {
                        let last = last_request_time.load(Ordering::Relaxed);
                        let now = now_secs();
                        let busy_for = now.saturating_sub(last);
                        info!(
                            idle_threshold,
                            seconds_since_last_request = busy_for,
                            "dreamer: skipping tick -- server still busy"
                        );
                        let mut s = stats.write().await;
                        s.cycles_skipped_busy += 1;
                        continue;
                    }
                    run_cycle(&db, brain.as_ref(), &stats).await;
                }
            }
        }
    });

    (token, handle)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_idle(last_request_time: &AtomicU64, threshold_secs: u64) -> bool {
    if threshold_secs == 0 {
        return true;
    }
    let last = last_request_time.load(Ordering::Relaxed);
    if last == 0 {
        // Never received a request -- treat as idle.
        return true;
    }
    now_secs().saturating_sub(last) >= threshold_secs
}

async fn run_cycle(
    db: &Arc<Database>,
    brain: Option<&Arc<dyn BrainBackend>>,
    stats: &DreamerStatsHandle,
) {
    let cycle_start = Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339();

    let users = match active_user_ids(db).await {
        Ok(u) => u,
        Err(e) => {
            warn!(error = %e, "dreamer: failed to enumerate users");
            let mut s = stats.write().await;
            s.cycles_failed += 1;
            return;
        }
    };

    let mut total_ok = 0usize;
    let mut total_failed = 0usize;
    let mut last_report: Option<Value> = None;

    for user_id in &users {
        match default_pipeline().run(db, *user_id).await {
            Ok(report) => {
                total_ok += report.ok_count;
                total_failed += report.failed_count;
                info!(
                    user_id = *user_id,
                    ok = report.ok_count,
                    failed = report.failed_count,
                    skipped = report.skipped_count,
                    duration_ms = report.total_duration_ms,
                    "dreamer: intelligence pipeline complete"
                );
                last_report = Some(serde_json::to_value(&report).unwrap_or(Value::Null));
            }
            Err(e) => {
                total_failed += 1;
                warn!(user_id = *user_id, error = %e, "dreamer: pipeline failed");
            }
        }
    }

    let mut brain_result: Option<Value> = None;
    let mut brain_ok = false;
    let mut evolution_ran = false;
    if let Some(b) = brain {
        if b.is_ready() {
            match b.dream_cycle().await {
                Ok(resp) => {
                    brain_ok = resp.ok;
                    info!(data = ?resp.data, "dreamer: brain dream_cycle complete");
                    brain_result = resp.data;
                }
                Err(e) => {
                    warn!(error = %e, "dreamer: brain dream_cycle failed");
                }
            }
            // Post-dream hook 1: evolution training step.
            match b.evolution_train().await {
                Ok(resp) if resp.ok => {
                    evolution_ran = true;
                    info!("dreamer: evolution_train complete");
                }
                Ok(resp) => warn!(error = ?resp.error, "dreamer: evolution_train reported failure"),
                Err(e) => warn!(error = %e, "dreamer: evolution_train call failed"),
            }
        }
    }

    // Post-dream hook 2: probabilistic growth reflection per user.
    let mut growth_calls = 0u64;
    let mut growth_stored = 0u64;
    for user_id in &users {
        let roll: f64 = rand::random();
        if roll >= GROWTH_REFLECT_CHANCE {
            continue;
        }
        match recent_memory_contents(db, *user_id, GROWTH_CONTEXT_SIZE).await {
            Ok(ctx) if !ctx.is_empty() => {
                let req = GrowthReflectRequest {
                    service: "dreamer".to_string(),
                    context: ctx,
                    existing_growth: None,
                    prompt_override: None,
                };
                match growth::reflect(db, &req, *user_id).await {
                    Ok(res) => {
                        growth_calls += 1;
                        if res.stored_memory_id.is_some() {
                            growth_stored += 1;
                            info!(
                                user_id = *user_id,
                                "dreamer: growth reflection stored observation"
                            );
                        }
                    }
                    Err(e) => warn!(user_id = *user_id, error = %e, "dreamer: growth::reflect failed"),
                }
            }
            Ok(_) => {}
            Err(e) => warn!(user_id = *user_id, error = %e, "dreamer: failed to load growth context"),
        }
    }

    let duration_ms = cycle_start.elapsed().as_millis() as u64;
    let mut s = stats.write().await;
    s.cycles_total += 1;
    s.last_cycle_started_at = Some(started_at);
    s.last_cycle_duration_ms = duration_ms;
    s.last_users_processed = users.len();
    s.last_pipeline_ok = total_ok;
    s.last_pipeline_failed = total_failed;
    s.last_pipeline_report = last_report;
    s.last_brain_result = brain_result;
    s.totals.pipeline_ok += total_ok as u64;
    s.totals.pipeline_failed += total_failed as u64;
    if brain.is_some() {
        if brain_ok {
            s.totals.brain_cycles += 1;
        } else {
            s.totals.brain_errors += 1;
        }
        if evolution_ran {
            s.totals.evolution_trainings += 1;
        }
    }
    s.totals.growth_reflections += growth_calls;
    s.totals.growth_observations_stored += growth_stored;
}

async fn recent_memory_contents(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<String>, EngError> {
    let limit_i64 = limit as i64;
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT content FROM memories \
                 WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![user_id, limit_i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}
