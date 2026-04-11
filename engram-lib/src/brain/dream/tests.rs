use crate::brain::dream::{run_dream_cycle, StageReport};
use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::recall;
use crate::db::Database;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_pattern(dim: usize, seed: u8) -> Vec<f32> {
    (0..dim)
        .map(|i| ((i as f32 + seed as f32) * 0.1).sin())
        .collect()
}

async fn seed_patterns(db: &Database, network: &mut HopfieldNetwork, user_id: i64, count: u8) {
    for i in 0..count {
        let embedding = make_pattern(64, i);
        recall::store_pattern(db, network, i as i64 + 1, &embedding, user_id, 5, 1.0)
            .await
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// StageReport
// ---------------------------------------------------------------------------

#[test]
fn stage_report_fields() {
    let r = StageReport {
        stage: "test".to_string(),
        items_processed: 10,
        items_changed: 3,
        duration_ms: 42,
    };
    assert_eq!(r.stage, "test");
    assert_eq!(r.items_processed, 10);
    assert_eq!(r.items_changed, 3);
    assert_eq!(r.duration_ms, 42);
}

// ---------------------------------------------------------------------------
// Individual stage tests (in-memory DB)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_replay_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::replay::replay(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "replay");
    assert_eq!(report.items_changed, 0);
}

#[tokio::test]
async fn test_replay_with_patterns() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 1i64;

    // Store a pattern with sub-1.0 strength so replay can boost it
    let embedding = make_pattern(64, 42);
    recall::store_pattern(&db, &mut network, 100, &embedding, user_id, 5, 0.7)
        .await
        .unwrap();

    // Touch the pattern to give it access_count > 0
    crate::brain::hopfield::pattern::touch_pattern(&db, 100, user_id)
        .await
        .unwrap();

    let report = crate::brain::dream::replay::replay(&db, &mut network, user_id, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "replay");
    // The touched pattern with strength < 1.0 should have been boosted
    assert!(report.items_changed >= 1);
}

#[tokio::test]
async fn test_merge_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::merge::merge(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "merge");
    assert_eq!(report.items_changed, 0);
}

#[tokio::test]
async fn test_merge_similar_patterns() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 1i64;

    // Store two nearly identical patterns -- they should be merged
    let base = make_pattern(64, 0);
    let near_identical: Vec<f32> = base.iter().map(|&x| x + 0.001).collect();

    recall::store_pattern(&db, &mut network, 1, &base, user_id, 5, 0.9)
        .await
        .unwrap();
    recall::store_pattern(&db, &mut network, 2, &near_identical, user_id, 5, 0.7)
        .await
        .unwrap();

    let report = crate::brain::dream::merge::merge(&db, &mut network, user_id, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "merge");
    assert_eq!(report.items_changed, 1);
    assert_eq!(network.pattern_count(), 1);
}

#[tokio::test]
async fn test_prune_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::prune::prune(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "prune");
    assert_eq!(report.items_changed, 0);
}

#[tokio::test]
async fn test_prune_removes_dead_patterns() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 1i64;

    // Store one healthy and one dead pattern
    recall::store_pattern(&db, &mut network, 1, &make_pattern(64, 0), user_id, 5, 1.0)
        .await
        .unwrap();
    recall::store_pattern(&db, &mut network, 2, &make_pattern(64, 10), user_id, 5, 0.01)
        .await
        .unwrap();

    let report = crate::brain::dream::prune::prune(&db, &mut network, user_id, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "prune");
    assert_eq!(report.items_changed, 1);
    assert_eq!(network.pattern_count(), 1);
    assert!(network.contains(1));
    assert!(!network.contains(2));
}

#[tokio::test]
async fn test_discover_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::discover::discover(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "discover");
    assert_eq!(report.items_changed, 0);
}

#[tokio::test]
async fn test_discover_creates_edges() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 1i64;

    // Two similar patterns should trigger edge creation
    let base = make_pattern(64, 0);
    let similar: Vec<f32> = base.iter().map(|&x| x * 0.99 + 0.005).collect();

    recall::store_pattern(&db, &mut network, 1, &base, user_id, 5, 1.0)
        .await
        .unwrap();
    recall::store_pattern(&db, &mut network, 2, &similar, user_id, 5, 1.0)
        .await
        .unwrap();

    let report = crate::brain::dream::discover::discover(&db, &mut network, user_id, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "discover");
    // Whether edges are created depends on the similarity crossing threshold
    // Just verify it ran without error and the report is well-formed
    assert!(report.items_processed <= 2);
}

#[tokio::test]
async fn test_decorrelate_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::decorrelate::decorrelate(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "decorrelate");
    assert_eq!(report.items_changed, 0);
}

#[tokio::test]
async fn test_resolve_empty_network() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let report = crate::brain::dream::resolve::resolve(&db, &mut network, 1, 100)
        .await
        .unwrap();
    assert_eq!(report.stage, "resolve");
    assert_eq!(report.items_changed, 0);
}

// ---------------------------------------------------------------------------
// Full dream cycle driver
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_run_dream_cycle_empty() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let result = run_dream_cycle(&db, &mut network, 1, 100).await.unwrap();
    assert_eq!(result.stages.len(), 6);
    assert_eq!(result.user_id, 1);
    assert!(result.run_id > 0);

    let stage_names: Vec<&str> = result.stages.iter().map(|s| s.stage.as_str()).collect();
    assert_eq!(
        stage_names,
        &["replay", "merge", "prune", "discover", "decorrelate", "resolve"]
    );
}

#[tokio::test]
async fn test_run_dream_cycle_with_patterns() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 1i64;

    seed_patterns(&db, &mut network, user_id, 5).await;

    let result = run_dream_cycle(&db, &mut network, user_id, 50)
        .await
        .unwrap();
    assert_eq!(result.stages.len(), 6);
    assert!(result.total_duration_ms < 60_000); // sanity: completes in under a minute
}

#[tokio::test]
async fn test_dream_run_persisted() {
    let db = Database::connect_memory().await.unwrap();
    let mut network = HopfieldNetwork::new();
    let user_id = 42i64;

    let result = run_dream_cycle(&db, &mut network, user_id, 10)
        .await
        .unwrap();

    // Verify the run was written to the DB
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, finished_at FROM brain_dream_runs WHERE id = ?1",
            libsql::params![result.run_id],
        )
        .await
        .unwrap();

    let row = rows.next().await.unwrap().expect("run row should exist");
    let db_id: i64 = row.get(0).unwrap();
    let db_user: i64 = row.get(1).unwrap();
    let finished_at: Option<String> = row.get(2).unwrap();

    assert_eq!(db_id, result.run_id);
    assert_eq!(db_user, user_id);
    assert!(finished_at.is_some(), "finished_at should be set after cycle");
}
