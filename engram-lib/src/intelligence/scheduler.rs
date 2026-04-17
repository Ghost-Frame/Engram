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
//! `default_pipeline()` wires up the canonical nightly pipeline: deduplicate
//! -> consolidate_sweep -> {contradictions, temporal, reconsolidation} ->
//! reflections. The HTTP handler at `POST /intelligence/run` invokes this.

use crate::db::Database;
use crate::intelligence::{
    consolidation, contradiction, duplicates, reconsolidation, reflections, temporal,
};
use crate::{EngError, Result};
use async_trait::async_trait;
use serde::Serialize;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskReport {
    pub name: String,
    pub status: TaskStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineReport {
    pub reports: Vec<TaskReport>,
    pub total_duration_ms: u64,
    pub ok_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
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

struct DeduplicateTask;
#[async_trait]
impl IntelligenceTask for DeduplicateTask {
    fn name(&self) -> &'static str {
        "deduplicate"
    }
    async fn run(&self, db: &Database, user_id: i64) -> Result<Value> {
        let result = duplicates::deduplicate(db, user_id, 0.9, true).await?;
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

/// Build the canonical intelligence pipeline used by `POST /intelligence/run`.
pub fn default_pipeline() -> Scheduler {
    Scheduler::new()
        .add_task(Arc::new(DeduplicateTask))
        .add_task(Arc::new(ConsolidateSweepTask))
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
        let report = default_pipeline().run(&db, 1).await.expect("run");
        assert_eq!(report.reports.len(), 6);
        let names: Vec<&str> = report.reports.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"deduplicate"));
        assert!(names.contains(&"consolidate_sweep"));
        assert!(names.contains(&"reflections_generate"));
    }
}
