//! Intelligence scheduler -- run a DAG of intelligence passes in topological
//! order with per-task timing, error isolation, and upstream-failure skipping.
//!
//! Each pass implements [`IntelligenceTask`] (name, static dependency list,
//! async `run`). [`Scheduler`] holds a registered set of tasks, performs a
//! Kahn's-algorithm topological sort (catching cycles and dangling deps), and
//! then runs tasks sequentially. A task failure is captured into its
//! [`TaskReport`] and causes dependents to be marked [`TaskStatus::Skipped`]
//! so independent branches of the DAG still get a chance to execute.
//!
//! `default_pipeline(consolidation_enabled, auto_link_batch)` wires up the
//! canonical pipeline: auto_link -> deduplicate -> consolidate_sweep ->
//! {contradictions, temporal, reconsolidation} -> reflections. The HTTP handler
//! at `POST /intelligence/run` invokes this.

use super::types::{PipelineReport, TaskReport, TaskStatus};
use crate::db::Database;
use crate::intelligence::{
    consolidation, contradiction, duplicates, linker, reconsolidation, reflections, temporal,
};
use crate::{EngError, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

/// One unit of work in the intelligence pipeline.
#[async_trait]
pub trait IntelligenceTask: Send + Sync {
    fn name(&self) -> &'static str;
    fn dependencies(&self) -> &'static [&'static str] {
        &[]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value>;
}

/// DAG scheduler for intelligence passes.
#[derive(Default)]
pub struct Scheduler {
    tasks: Vec<Arc<dyn IntelligenceTask>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    pub fn add_task(mut self, task: Arc<dyn IntelligenceTask>) -> Self {
        self.tasks.push(task);
        self
    }

    /// Return tasks in topological order. Errors on duplicate names, missing
    /// dependencies, or cycles.
    pub fn topological_order(&self) -> Result<Vec<Arc<dyn IntelligenceTask>>> {
        let mut name_to_idx: HashMap<&'static str, usize> = HashMap::new();
        for (idx, task) in self.tasks.iter().enumerate() {
            if name_to_idx.insert(task.name(), idx).is_some() {
                return Err(EngError::InvalidInput(format!(
                    "scheduler: duplicate task name '{}'",
                    task.name()
                )));
            }
        }

        let mut indegree = vec![0usize; self.tasks.len()];
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); self.tasks.len()];
        for (idx, task) in self.tasks.iter().enumerate() {
            for dep in task.dependencies() {
                let dep_idx = name_to_idx.get(dep).ok_or_else(|| {
                    EngError::InvalidInput(format!(
                        "scheduler: task '{}' depends on unknown task '{}'",
                        task.name(),
                        dep
                    ))
                })?;
                adjacency[*dep_idx].push(idx);
                indegree[idx] += 1;
            }
        }

        let mut queue: VecDeque<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(idx, deg)| if *deg == 0 { Some(idx) } else { None })
            .collect();
        let mut ordered: Vec<Arc<dyn IntelligenceTask>> = Vec::with_capacity(self.tasks.len());
        while let Some(idx) = queue.pop_front() {
            ordered.push(self.tasks[idx].clone());
            for &next in &adjacency[idx] {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        if ordered.len() != self.tasks.len() {
            let remaining: Vec<&'static str> = indegree
                .iter()
                .enumerate()
                .filter_map(|(idx, deg)| {
                    if *deg > 0 {
                        Some(self.tasks[idx].name())
                    } else {
                        None
                    }
                })
                .collect();
            return Err(EngError::InvalidInput(format!(
                "scheduler: dependency cycle detected among tasks {:?}",
                remaining
            )));
        }

        Ok(ordered)
    }

    /// Execute the pipeline. Tasks whose upstream dependency failed (or was
    /// skipped) are themselves marked [`TaskStatus::Skipped`]; independent
    /// branches continue to run.
    #[tracing::instrument(skip(self, db))]
    pub async fn run(&self, db: &Database, user_id: i64) -> Result<PipelineReport> {
        let ordered = self.topological_order()?;
        let started = Instant::now();
        let mut reports: Vec<TaskReport> = Vec::with_capacity(ordered.len());
        let mut failed: HashSet<&'static str> = HashSet::new();

        for task in ordered {
            let blocking: Vec<&'static str> = task
                .dependencies()
                .iter()
                .filter(|dep| failed.contains(**dep))
                .copied()
                .collect();

            if !blocking.is_empty() {
                failed.insert(task.name());
                reports.push(TaskReport {
                    name: task.name().to_string(),
                    status: TaskStatus::Skipped,
                    duration_ms: 0,
                    output: None,
                    error: Some(format!("upstream failure: {}", blocking.join(", "))),
                });
                continue;
            }

            let task_started = Instant::now();
            let result = task.run(db, user_id).await;
            let duration_ms = task_started.elapsed().as_millis() as u64;

            match result {
                Ok(output) => reports.push(TaskReport {
                    name: task.name().to_string(),
                    status: TaskStatus::Ok,
                    duration_ms,
                    output: Some(output),
                    error: None,
                }),
                Err(e) => {
                    failed.insert(task.name());
                    reports.push(TaskReport {
                        name: task.name().to_string(),
                        status: TaskStatus::Failed,
                        duration_ms,
                        output: None,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        let total_duration_ms = started.elapsed().as_millis() as u64;
        let ok_count = reports
            .iter()
            .filter(|r| r.status == TaskStatus::Ok)
            .count();
        let failed_count = reports
            .iter()
            .filter(|r| r.status == TaskStatus::Failed)
            .count();
        let skipped_count = reports
            .iter()
            .filter(|r| r.status == TaskStatus::Skipped)
            .count();

        Ok(PipelineReport {
            reports,
            total_duration_ms,
            ok_count,
            failed_count,
            skipped_count,
        })
    }
}

// ---------------------------------------------------------------------------
// Canonical pipeline
// ---------------------------------------------------------------------------

/// Associative auto-linker: reconnect unlinked memories to their nearest
/// neighbours with `similarity` links. Runs FIRST so the links it creates feed
/// the dedup/consolidation passes (which read `type = 'similarity'`) in the same
/// cycle. `batch` bounds how many unlinked memories are processed per cycle so a
/// large historical backlog drains gradually instead of stalling a tick.
struct AutoLinkTask {
    batch: usize,
}
#[async_trait]
impl IntelligenceTask for AutoLinkTask {
    fn name(&self) -> &'static str {
        "auto_link"
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let report = linker::link_unlinked_batch(db, user_id, self.batch, false).await?;
        Ok(json!(report))
    }
}

/// Placeholder for the auto_link slot when the linker is disabled. Resolves the
/// `deduplicate` dependency without writing links.
struct NoopAutoLinkTask;
#[async_trait]
impl IntelligenceTask for NoopAutoLinkTask {
    fn name(&self) -> &'static str {
        "auto_link"
    }
    async fn run(&self, _db: &Database, _user_id: i64) -> Result<Value> {
        Ok(json!({ "skipped": true, "reason": "auto_link_disabled" }))
    }
}

struct DeduplicateTask;
#[async_trait]
impl IntelligenceTask for DeduplicateTask {
    fn name(&self) -> &'static str {
        "deduplicate"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        // Run after auto_link so freshly created similarity links are visible to
        // the duplicate scan (which filters on `type = 'similarity'`).
        &["auto_link"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let result = duplicates::deduplicate(db, user_id, 0.9, false).await?;
        Ok(json!(result))
    }
}

struct ConsolidateSweepTask;
#[async_trait]
impl IntelligenceTask for ConsolidateSweepTask {
    fn name(&self) -> &'static str {
        "consolidate_sweep"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["deduplicate"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let result = consolidation::sweep(db, user_id, 0.85).await?;
        Ok(json!(result))
    }
}

struct ContradictionScanTask;
#[async_trait]
impl IntelligenceTask for ContradictionScanTask {
    fn name(&self) -> &'static str {
        "contradiction_scan"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["consolidate_sweep"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let contradictions = contradiction::scan_all_contradictions(db, user_id).await?;
        Ok(json!({ "count": contradictions.len() }))
    }
}

struct TemporalDetectTask;
#[async_trait]
impl IntelligenceTask for TemporalDetectTask {
    fn name(&self) -> &'static str {
        "temporal_detect"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["consolidate_sweep"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let patterns = temporal::detect_patterns(db, user_id).await?;
        Ok(json!({ "count": patterns.len() }))
    }
}

struct ReconsolidationSweepTask;
#[async_trait]
impl IntelligenceTask for ReconsolidationSweepTask {
    fn name(&self) -> &'static str {
        "reconsolidation_sweep"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["consolidate_sweep"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let results = reconsolidation::run_reconsolidation_sweep(db, user_id, 20).await?;
        Ok(json!({ "count": results.len() }))
    }
}

struct ReflectionsGenerateTask;
#[async_trait]
impl IntelligenceTask for ReflectionsGenerateTask {
    fn name(&self) -> &'static str {
        "reflections_generate"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["consolidate_sweep", "temporal_detect"]
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let items = reflections::generate_reflections(db, user_id, 10).await?;
        Ok(json!({ "count": items.len() }))
    }
}

/// Placeholder for the consolidate_sweep slot when consolidation is disabled.
/// Returns immediately so downstream tasks still receive a resolved dependency.
struct NoopConsolidateSweepTask;
#[async_trait]
impl IntelligenceTask for NoopConsolidateSweepTask {
    fn name(&self) -> &'static str {
        "consolidate_sweep"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["deduplicate"]
    }
    async fn run(&self, _db: &Database, _user_id: i64) -> Result<Value> {
        Ok(json!({ "skipped": true, "reason": "consolidation_disabled" }))
    }
}

/// Build the canonical intelligence pipeline used by `POST /intelligence/run`.
///
/// `auto_link_batch` controls the associative linker that runs first:
/// `Some(n)` links up to `n` unlinked memories per cycle, `None` installs a
/// no-op (linker disabled) so the `deduplicate` dependency still resolves.
///
/// When `consolidation_enabled` is false the consolidate_sweep slot is filled
/// by a no-op task so downstream tasks (contradiction scan, temporal detect,
/// reconsolidation, reflections) still run normally.
pub fn default_pipeline(consolidation_enabled: bool, auto_link_batch: Option<usize>) -> Scheduler {
    let auto_link: Arc<dyn IntelligenceTask> = match auto_link_batch {
        Some(batch) if batch > 0 => Arc::new(AutoLinkTask { batch }),
        _ => Arc::new(NoopAutoLinkTask),
    };
    let consolidate: Arc<dyn IntelligenceTask> = if consolidation_enabled {
        Arc::new(ConsolidateSweepTask)
    } else {
        Arc::new(NoopConsolidateSweepTask)
    };
    Scheduler::new()
        .add_task(auto_link)
        .add_task(Arc::new(DeduplicateTask))
        .add_task(consolidate)
        .add_task(Arc::new(ContradictionScanTask))
        .add_task(Arc::new(TemporalDetectTask))
        .add_task(Arc::new(ReconsolidationSweepTask))
        .add_task(Arc::new(ReflectionsGenerateTask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    async fn setup_db() -> Database {
        let db_path = std::env::temp_dir()
            .join(format!("engram-scheduler-test-{}.db", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        let config = Config {
            db_path,
            use_lance_index: false,
            ..Config::default()
        };
        Database::connect_with_config(&config, None).await.unwrap()
    }

    struct RecordingTask {
        name: &'static str,
        deps: &'static [&'static str],
        fail: bool,
    }

    #[async_trait]
    impl IntelligenceTask for RecordingTask {
        fn name(&self) -> &'static str {
            self.name
        }
        fn dependencies(&self) -> &'static [&'static str] {
            self.deps
        }
        async fn run(&self, _db: &Database, _user_id: i64) -> Result<Value> {
            if self.fail {
                Err(EngError::Internal(format!("{} failed", self.name)))
            } else {
                Ok(json!({ "task": self.name }))
            }
        }
    }

    #[tokio::test]
    async fn topological_order_respects_dependencies() {
        let scheduler = Scheduler::new()
            .add_task(Arc::new(RecordingTask {
                name: "c",
                deps: &["a", "b"],
                fail: false,
            }))
            .add_task(Arc::new(RecordingTask {
                name: "a",
                deps: &[],
                fail: false,
            }))
            .add_task(Arc::new(RecordingTask {
                name: "b",
                deps: &["a"],
                fail: false,
            }));

        let ordered: Vec<&'static str> = scheduler
            .topological_order()
            .expect("topo")
            .iter()
            .map(|t| t.name())
            .collect();

        let pos_a = ordered.iter().position(|n| *n == "a").unwrap();
        let pos_b = ordered.iter().position(|n| *n == "b").unwrap();
        let pos_c = ordered.iter().position(|n| *n == "c").unwrap();
        assert!(pos_a < pos_b, "a must precede b (ordered={:?})", ordered);
        assert!(pos_b < pos_c, "b must precede c (ordered={:?})", ordered);
    }

    #[tokio::test]
    async fn cycle_detected_returns_error() {
        let scheduler = Scheduler::new()
            .add_task(Arc::new(RecordingTask {
                name: "a",
                deps: &["b"],
                fail: false,
            }))
            .add_task(Arc::new(RecordingTask {
                name: "b",
                deps: &["a"],
                fail: false,
            }));

        let msg = match scheduler.topological_order() {
            Ok(_) => panic!("expected cycle error"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("cycle"), "expected cycle error, got: {}", msg);
    }

    #[tokio::test]
    async fn missing_dependency_returns_error() {
        let scheduler = Scheduler::new().add_task(Arc::new(RecordingTask {
            name: "a",
            deps: &["ghost"],
            fail: false,
        }));

        let msg = match scheduler.topological_order() {
            Ok(_) => panic!("expected missing-dep error"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("ghost"), "expected ghost error, got: {}", msg);
    }

    #[tokio::test]
    async fn upstream_failure_marks_dependent_skipped() {
        let db = setup_db().await;
        let scheduler = Scheduler::new()
            .add_task(Arc::new(RecordingTask {
                name: "root",
                deps: &[],
                fail: true,
            }))
            .add_task(Arc::new(RecordingTask {
                name: "child",
                deps: &["root"],
                fail: false,
            }))
            .add_task(Arc::new(RecordingTask {
                name: "sibling",
                deps: &[],
                fail: false,
            }));

        let report = scheduler.run(&db, 1).await.expect("run");
        let by_name: HashMap<_, _> = report
            .reports
            .iter()
            .map(|r| (r.name.as_str(), r.status))
            .collect();
        assert_eq!(by_name["root"], TaskStatus::Failed);
        assert_eq!(by_name["child"], TaskStatus::Skipped);
        assert_eq!(by_name["sibling"], TaskStatus::Ok);
        assert_eq!(report.ok_count, 1);
        assert_eq!(report.failed_count, 1);
        assert_eq!(report.skipped_count, 1);
    }

    #[tokio::test]
    async fn empty_scheduler_produces_empty_report() {
        let db = setup_db().await;
        let report = Scheduler::new().run(&db, 1).await.expect("run");
        assert!(report.reports.is_empty());
        assert_eq!(report.ok_count, 0);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.skipped_count, 0);
    }

    #[tokio::test]
    async fn default_pipeline_runs_on_empty_db() {
        let db = setup_db().await;
        let report = default_pipeline(true, Some(50))
            .run(&db, 1)
            .await
            .expect("run");
        assert_eq!(report.reports.len(), 7);
        let names: Vec<&str> = report.reports.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"auto_link"));
        assert!(names.contains(&"deduplicate"));
        assert!(names.contains(&"consolidate_sweep"));
        assert!(names.contains(&"reflections_generate"));
    }

    /// auto_link must precede deduplicate so its similarity links feed the scan.
    #[tokio::test]
    async fn auto_link_precedes_deduplicate() {
        let scheduler = default_pipeline(true, Some(10));
        let ordered: Vec<&'static str> = scheduler
            .topological_order()
            .expect("topo")
            .iter()
            .map(|t| t.name())
            .collect();
        let pos_link = ordered.iter().position(|n| *n == "auto_link").unwrap();
        let pos_dedup = ordered.iter().position(|n| *n == "deduplicate").unwrap();
        assert!(
            pos_link < pos_dedup,
            "auto_link must precede deduplicate (ordered={ordered:?})"
        );
    }

    /// A None batch installs the no-op linker but the slot is still present.
    #[tokio::test]
    async fn default_pipeline_disabled_auto_link_still_runs_all_tasks() {
        let db = setup_db().await;
        let report = default_pipeline(true, None).run(&db, 1).await.expect("run");
        assert_eq!(report.reports.len(), 7, "all seven slots must be present");
        let by_name: HashMap<_, _> = report
            .reports
            .iter()
            .map(|r| (r.name.as_str(), r))
            .collect();
        let auto_link = by_name["auto_link"];
        assert_eq!(auto_link.status, TaskStatus::Ok);
        assert_eq!(
            auto_link.output.as_ref().expect("noop auto_link output")["reason"],
            "auto_link_disabled"
        );
        // Dependent must still run since the no-op resolved Ok.
        assert_eq!(by_name["deduplicate"].status, TaskStatus::Ok);
    }

    /// Verify that disabling consolidation still runs all pipeline slots.
    #[tokio::test]
    async fn default_pipeline_disabled_consolidation_still_runs_all_tasks() {
        let db = setup_db().await;
        let report = default_pipeline(false, Some(50))
            .run(&db, 1)
            .await
            .expect("run");
        assert_eq!(report.reports.len(), 7, "all seven slots must be present");

        let by_name: HashMap<_, _> = report
            .reports
            .iter()
            .map(|r| (r.name.as_str(), r))
            .collect();

        // The noop must succeed and carry the skipped marker in its output.
        let consolidate = by_name["consolidate_sweep"];
        assert_eq!(consolidate.status, TaskStatus::Ok);
        let output = consolidate
            .output
            .as_ref()
            .expect("noop consolidate_sweep must return output");
        assert_eq!(output["skipped"], true);
        assert_eq!(output["reason"], "consolidation_disabled");

        // Downstream tasks must not be skipped -- they depend on
        // consolidate_sweep which succeeded (even though it was a noop).
        for name in &[
            "contradiction_scan",
            "temporal_detect",
            "reconsolidation_sweep",
            "reflections_generate",
        ] {
            let task = by_name
                .get(name)
                .unwrap_or_else(|| panic!("missing task: {name}"));
            assert_eq!(
                task.status,
                TaskStatus::Ok,
                "{name} should run normally when consolidation is disabled"
            );
        }
    }
}
