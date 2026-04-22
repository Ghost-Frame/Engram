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
use kleos_lib::llm::local::LocalModelClient;
use kleos_lib::services::brain::BrainBackend;
use kleos_lib::skills::{analyzer, evolver};
use kleos_lib::tenant::TenantRegistry;
use kleos_lib::EngError;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Program-lifetime monotonic anchor. We store `last_request_time` as
/// milliseconds since this anchor so that idleness checks are immune to
/// wall-clock jumps (NTP step, DST, manual `date` changes). The anchor is
/// lazily initialised on first use and never reset.
fn monotonic_anchor() -> Instant {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    *ANCHOR.get_or_init(Instant::now)
}

/// Milliseconds elapsed since [`monotonic_anchor`]. Always monotonically
/// non-decreasing; safe to store in `AtomicU64` and subtract.
pub fn monotonic_millis() -> u64 {
    monotonic_anchor().elapsed().as_millis() as u64
}

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
    pub last_skill_evolution: Option<SkillEvolutionReport>,
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
    pub skill_fixes_attempted: u64,
    pub skill_fixes_succeeded: u64,
    pub skill_fixes_failed: u64,
    pub skill_captures_attempted: u64,
    pub skill_captures_succeeded: u64,
    pub skill_captures_failed: u64,
    pub skill_derives_attempted: u64,
    pub skill_derives_succeeded: u64,
    pub skill_derives_failed: u64,
    pub skill_evolution_skipped_no_llm: u64,
}

/// Per-tick report for the autonomous skill-evolution phase. Serialised into
/// `DreamerStats.last_skill_evolution` so `/intelligence/dreamer` exposes the
/// most recent fix/capture/derive activity without needing a second call.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillEvolutionReport {
    pub ran_at: String,
    pub users_scanned: usize,
    pub fixes_attempted: u64,
    pub fixes_succeeded: u64,
    pub fixes_failed: u64,
    pub captures_attempted: u64,
    pub captures_succeeded: u64,
    pub captures_failed: u64,
    pub derives_attempted: u64,
    pub derives_succeeded: u64,
    pub derives_failed: u64,
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
    llm: Option<Arc<LocalModelClient>>,
    stats: DreamerStatsHandle,
    last_request_time: Arc<AtomicU64>,
    tenant_registry: Option<Arc<TenantRegistry>>,
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

        // Sub-interval gate for the skill evolution phase. `None` on startup
        // forces the first eligible tick to run the evolution pass.
        let mut last_evolution_run_at: Option<Instant> = None;

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
                        let busy_for_secs = monotonic_millis()
                            .saturating_sub(last) / 1000;
                        info!(
                            idle_threshold,
                            seconds_since_last_request = busy_for_secs,
                            "dreamer: skipping tick -- server still busy"
                        );
                        let mut s = stats.write().await;
                        s.cycles_skipped_busy += 1;
                        continue;
                    }
                    run_cycle(
                        &db,
                        brain.as_ref(),
                        llm.as_ref(),
                        &config,
                        &stats,
                        &mut last_evolution_run_at,
                    )
                    .await;

                    // Tenant-aware pass: iterate all tenant shards
                    if let Some(ref registry) = tenant_registry {
                        run_cycle_tenants(
                            registry,
                            llm.as_ref(),
                            &config,
                            &stats,
                        )
                        .await;
                    }
                }
            }
        }
    });

    (token, handle)
}

fn should_run_evolution(last_run: &Option<Instant>, interval_secs: u64) -> bool {
    if interval_secs == 0 {
        return true;
    }
    match last_run {
        None => true,
        Some(t) => t.elapsed() >= Duration::from_secs(interval_secs),
    }
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
    let elapsed_secs = monotonic_millis().saturating_sub(last) / 1000;
    elapsed_secs >= threshold_secs
}

async fn run_cycle(
    db: &Arc<Database>,
    brain: Option<&Arc<dyn BrainBackend>>,
    llm: Option<&Arc<LocalModelClient>>,
    config: &Arc<Config>,
    stats: &DreamerStatsHandle,
    last_evolution_run_at: &mut Option<Instant>,
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

    // Post-dream hook 1b: hermes-style autonomous skill evolution. Runs on a
    // sub-interval (skill_evolution_interval_secs) independent from the
    // dreamer tick so we do not slam the local LLM every 5 minutes. Silent
    // skip when the local LLM is unavailable.
    let mut skill_evolution_report: Option<SkillEvolutionReport> = None;
    let mut evolution_skipped_no_llm = false;
    if config.skill_evolution_enabled
        && should_run_evolution(last_evolution_run_at, config.skill_evolution_interval_secs)
    {
        match llm {
            None => {
                warn!("dreamer: skill evolution skipped, local LLM unavailable");
                evolution_skipped_no_llm = true;
                *last_evolution_run_at = Some(Instant::now());
            }
            Some(llm_ref) => {
                let report = run_skill_evolution(db, llm_ref.as_ref(), config, &users).await;
                info!(
                    fixes = report.fixes_succeeded,
                    captures = report.captures_succeeded,
                    derives = report.derives_succeeded,
                    "dreamer: skill evolution phase complete",
                );
                skill_evolution_report = Some(report);
                *last_evolution_run_at = Some(Instant::now());
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
                    Err(e) => {
                        warn!(user_id = *user_id, error = %e, "dreamer: growth::reflect failed")
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!(user_id = *user_id, error = %e, "dreamer: failed to load growth context")
            }
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
    if evolution_skipped_no_llm {
        s.totals.skill_evolution_skipped_no_llm += 1;
    }
    if let Some(ev) = &skill_evolution_report {
        s.totals.skill_fixes_attempted += ev.fixes_attempted;
        s.totals.skill_fixes_succeeded += ev.fixes_succeeded;
        s.totals.skill_fixes_failed += ev.fixes_failed;
        s.totals.skill_captures_attempted += ev.captures_attempted;
        s.totals.skill_captures_succeeded += ev.captures_succeeded;
        s.totals.skill_captures_failed += ev.captures_failed;
        s.totals.skill_derives_attempted += ev.derives_attempted;
        s.totals.skill_derives_succeeded += ev.derives_succeeded;
        s.totals.skill_derives_failed += ev.derives_failed;
        s.last_skill_evolution = Some(ev.clone());
    }
}

/// Per-tick skill evolution driver. Iterates the active-user list, runs up
/// to three bounded passes (fix -> capture -> derive) per user, and rolls
/// the results up into a single report. Each sub-pass catches and logs
/// errors individually so one failing LLM call never poisons the whole
/// tick.
async fn run_skill_evolution(
    db: &Database,
    llm: &LocalModelClient,
    config: &Config,
    users: &[i64],
) -> SkillEvolutionReport {
    let mut report = SkillEvolutionReport {
        ran_at: chrono::Utc::now().to_rfc3339(),
        users_scanned: users.len(),
        ..Default::default()
    };

    for &user_id in users {
        // --- Fix pass ---
        let fix_ids = match analyzer::get_failing_skill_candidates(
            db,
            user_id,
            config.skill_evolution_min_executions,
            config.skill_evolution_failure_threshold,
            config.skill_evolution_refix_cooldown_secs,
            config.skill_evolution_max_fixes_per_tick as usize,
        )
        .await
        {
            Ok(ids) => ids,
            Err(e) => {
                warn!(user_id, error = %e, "dreamer: get_failing_skill_candidates failed");
                Vec::new()
            }
        };
        for sid in fix_ids {
            report.fixes_attempted += 1;
            match evolver::fix_skill(db, Some(llm), sid, "dreamer", user_id).await {
                Ok(r) if r.success => {
                    report.fixes_succeeded += 1;
                    info!(user_id, source = sid, new = ?r.skill_id, "dreamer: fix_skill ok");
                }
                Ok(r) => {
                    report.fixes_failed += 1;
                    warn!(user_id, source = sid, message = %r.message, "dreamer: fix_skill reported failure");
                }
                Err(e) => {
                    report.fixes_failed += 1;
                    warn!(user_id, source = sid, error = %e, "dreamer: fix_skill errored");
                }
            }
        }

        // --- Capture pass ---
        let cap_descriptions = match analyzer::get_capture_candidates(
            db,
            user_id,
            &config.skill_evolution_capture_tag,
            config.skill_evolution_interval_secs,
            config.skill_evolution_max_captures_per_tick as usize,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(user_id, error = %e, "dreamer: get_capture_candidates failed");
                Vec::new()
            }
        };
        for description in cap_descriptions {
            report.captures_attempted += 1;
            match evolver::capture_skill(db, Some(llm), &description, "dreamer", user_id).await {
                Ok(r) if r.success => {
                    report.captures_succeeded += 1;
                    info!(user_id, new = ?r.skill_id, "dreamer: capture_skill ok");
                }
                Ok(r) => {
                    report.captures_failed += 1;
                    warn!(user_id, message = %r.message, "dreamer: capture_skill reported failure");
                }
                Err(e) => {
                    report.captures_failed += 1;
                    warn!(user_id, error = %e, "dreamer: capture_skill errored");
                }
            }
        }

        // --- Derive pass ---
        let derive_pairs = match analyzer::get_derive_candidates(
            db,
            user_id,
            config.skill_evolution_derive_similarity,
            config.skill_evolution_max_derives_per_tick as usize,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(user_id, error = %e, "dreamer: get_derive_candidates failed");
                Vec::new()
            }
        };
        for (parents, direction) in derive_pairs {
            report.derives_attempted += 1;
            match evolver::derive_skill(db, Some(llm), &parents, &direction, "dreamer", user_id)
                .await
            {
                Ok(r) if r.success => {
                    report.derives_succeeded += 1;
                    info!(user_id, parents = ?parents, new = ?r.skill_id, "dreamer: derive_skill ok");
                }
                Ok(r) => {
                    report.derives_failed += 1;
                    warn!(user_id, parents = ?parents, message = %r.message, "dreamer: derive_skill reported failure");
                }
                Err(e) => {
                    report.derives_failed += 1;
                    warn!(user_id, parents = ?parents, error = %e, "dreamer: derive_skill errored");
                }
            }
        }
    }

    report
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

/// Run intelligence pipeline across all active tenant shards.
/// Each tenant gets its own Database bridge so the existing pipeline
/// functions work without modification.
async fn run_cycle_tenants(
    registry: &Arc<TenantRegistry>,
    llm: Option<&Arc<LocalModelClient>>,
    config: &Config,
    _stats: &DreamerStatsHandle,
) {
    let tenants = match registry.list() {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "dreamer: failed to list tenants");
            return;
        }
    };

    let mut tenants_processed = 0usize;
    for tenant_row in tenants {
        if tenant_row.status != kleos_lib::tenant::TenantStatus::Active {
            continue;
        }

        let handle = match registry.get(&tenant_row.user_id).await {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(e) => {
                warn!(tenant = %tenant_row.tenant_id, error = %e, "dreamer: failed to load tenant");
                continue;
            }
        };

        let tenant_db = Arc::clone(&handle.db);

        let users = match active_user_ids(&tenant_db).await {
            Ok(u) => u,
            Err(e) => {
                warn!(tenant = %tenant_row.tenant_id, error = %e, "dreamer: failed to get tenant users");
                continue;
            }
        };

        for user_id in &users {
            if let Err(e) = default_pipeline().run(&tenant_db, *user_id).await {
                warn!(
                    tenant = %tenant_row.tenant_id,
                    user_id = *user_id,
                    error = %e,
                    "dreamer: tenant pipeline failed"
                );
            }

            let roll: f64 = rand::random();
            if roll < GROWTH_REFLECT_CHANCE {
                if let Ok(ctx) = recent_memory_contents(&tenant_db, *user_id, GROWTH_CONTEXT_SIZE).await {
                    if !ctx.is_empty() {
                        let req = GrowthReflectRequest {
                            service: "dreamer".to_string(),
                            context: ctx,
                            existing_growth: None,
                            prompt_override: None,
                        };
                        if let Err(e) = growth::reflect(&tenant_db, &req, *user_id).await {
                            warn!(
                                tenant = %tenant_row.tenant_id,
                                user_id = *user_id,
                                error = %e,
                                "dreamer: tenant growth reflect failed"
                            );
                        }
                    }
                }
            }
        }

        if config.skill_evolution_enabled {
            if let Some(llm_ref) = llm {
                let _report = run_skill_evolution(&tenant_db, llm_ref.as_ref(), config, &users).await;
            }
        }

        tenants_processed += 1;
    }

    if tenants_processed > 0 {
        info!(tenants = tenants_processed, "dreamer: tenant cycle complete");
    }
}
