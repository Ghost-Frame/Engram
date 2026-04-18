/// Integration tests for the Hopfield substrate.
/// Unit tests for individual components live in their own modules
/// (network.rs, etc.). These tests exercise the full stack including
/// database persistence.
#[cfg(test)]
mod hopfield_tests {
    use crate::brain::hopfield::network::{
        cosine_similarity, l2_normalize, HopfieldNetwork, DEFAULT_BETA,
    };
    use crate::brain::hopfield::recall::{self, DEATH_THRESHOLD};

    // -----------------------------------------------------------------------
    // Helper: make a deterministic pattern from a seed
    // -----------------------------------------------------------------------

    fn make_pattern(dim: usize, seed: u8) -> Vec<f32> {
        (0..dim)
            .map(|i| ((i as f32 + seed as f32) * 0.1).sin())
            .collect()
    }

    fn make_distinct_patterns(dim: usize, n: usize) -> Vec<Vec<f32>> {
        // Use large seed offsets to ensure patterns are distinct
        (0..n)
            .map(|i| make_pattern(dim, (i * 37 + 13) as u8))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Unit: store + recall
    // -----------------------------------------------------------------------

    #[test]
    fn store_and_recall_exact_match() {
        let mut net = HopfieldNetwork::new();
        let dim = 128;
        let p = make_pattern(dim, 42);

        net.store(1, &p, 1.0);

        let results = net.retrieve(&p, 5, DEFAULT_BETA);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 1);
        assert!(
            results[0].1 > 0.9,
            "activation should be high for exact match"
        );
    }

    #[test]
    fn recall_from_partial_cue() {
        let mut net = HopfieldNetwork::new();
        let dim = 128;
        let original = make_pattern(dim, 42);

        net.store(1, &original, 1.0);

        // 50% masked cue
        let mut cue = original.clone();
        for (i, val) in cue.iter_mut().enumerate().take(dim) {
            if i % 2 == 0 {
                *val = 0.0;
            }
        }

        // Pattern completion
        let completed = net.complete(&cue, 10, DEFAULT_BETA);
        let norm_original = l2_normalize(&original);
        let sim = cosine_similarity(&completed, &norm_original);
        assert!(
            sim > 0.85,
            "50% masked cue should recall well; got sim={}",
            sim
        );
    }

    // -----------------------------------------------------------------------
    // Unit: reinforce
    // -----------------------------------------------------------------------

    #[test]
    fn reinforce_increases_strength() {
        let mut net = HopfieldNetwork::new();
        net.store(1, &make_pattern(64, 1), 0.3);

        let old = net.strength(1).unwrap();
        // Simulate reinforce: new = old + 0.3 * (1 - old)
        let new = old + 0.3 * (1.0 - old);
        net.update_strength(1, new);

        let s = net.strength(1).unwrap();
        assert!(s > old, "strength should increase after reinforce");
        assert!((s - 0.51).abs() < 0.01, "0.3 + 0.3*0.7 = 0.51; got {}", s);
    }

    // -----------------------------------------------------------------------
    // Unit: decay + prune
    // -----------------------------------------------------------------------

    #[test]
    fn decay_reduces_strength() {
        let mut net = HopfieldNetwork::new();
        net.store(1, &make_pattern(64, 1), 1.0);

        // Simulate 100 ticks at importance=0
        let rate: f32 = 0.995;
        let new_strength = 1.0 * rate.powi(100);
        net.update_strength(1, new_strength);

        let s = net.strength(1).unwrap();
        assert!(s < 1.0, "strength should decrease after decay");
        assert!((s - 0.995f32.powi(100)).abs() < 1e-5);
    }

    #[test]
    fn prune_removes_weak_patterns() {
        let mut net = HopfieldNetwork::new();
        net.store(1, &make_pattern(64, 1), 1.0);
        net.store(2, &make_pattern(64, 5), 0.03); // Below DEATH_THRESHOLD
        net.store(3, &make_pattern(64, 10), 0.8);

        assert_eq!(net.pattern_count(), 3);

        // Remove weak
        let mut dead = Vec::new();
        for &id in net.pattern_ids().to_vec().iter() {
            if net.strength(id).unwrap_or(0.0) < DEATH_THRESHOLD {
                dead.push(id);
            }
        }
        for id in &dead {
            net.remove(*id);
        }

        assert_eq!(net.pattern_count(), 2);
        assert!(!net.contains(2), "weak pattern should be pruned");
        assert!(net.contains(1));
        assert!(net.contains(3));
    }

    // -----------------------------------------------------------------------
    // Property: Hopfield capacity test
    // -----------------------------------------------------------------------

    #[test]
    fn capacity_recall_rate() {
        let dim = 256;
        // For the modern Hopfield net, capacity is O(exp(d)), so even modest
        // d can handle many patterns. We test with N=30 which is well within
        // both classic (0.14 * 256 = 35) and modern capacity.
        let n = 30;
        let patterns = make_distinct_patterns(dim, n);

        let mut net = HopfieldNetwork::new();
        for (i, p) in patterns.iter().enumerate() {
            net.store(i as i64 + 1, p, 1.0);
        }

        assert_eq!(net.pattern_count(), n);

        // Test recall from 50% masked cues
        let mut successes = 0;
        for original in patterns.iter() {
            let mut cue = original.clone();
            // Mask every other element
            for (j, val) in cue.iter_mut().enumerate().take(dim) {
                if j % 2 == 0 {
                    *val = 0.0;
                }
            }

            let completed = net.complete(&cue, 10, DEFAULT_BETA);
            let norm_original = l2_normalize(original);
            let sim = cosine_similarity(&completed, &norm_original);

            if sim > 0.7 {
                successes += 1;
            }
        }

        let recall_rate = successes as f32 / n as f32;
        // For the modern Hopfield net at this capacity, we expect near-perfect
        // recall. Classic threshold is 0.14*N, modern is much higher.
        assert!(
            recall_rate > 0.8,
            "recall rate should be above 80%; got {:.0}% ({}/{})",
            recall_rate * 100.0,
            successes,
            n
        );
    }

    #[test]
    fn retrieval_selects_correct_pattern() {
        let dim = 128;
        let n = 20;
        let patterns = make_distinct_patterns(dim, n);

        let mut net = HopfieldNetwork::new();
        for (i, p) in patterns.iter().enumerate() {
            net.store(i as i64 + 1, p, 1.0);
        }

        // Each pattern should be its own top-1 retrieval result
        let mut correct = 0;
        for (i, p) in patterns.iter().enumerate() {
            let results = net.retrieve(p, 1, DEFAULT_BETA);
            if !results.is_empty() && results[0].0 == (i as i64 + 1) {
                correct += 1;
            }
        }

        assert_eq!(
            correct, n,
            "all {} patterns should self-retrieve as top-1",
            n
        );
    }

    // -----------------------------------------------------------------------
    // Integration: merge_similar (network-only, no DB)
    // -----------------------------------------------------------------------

    #[test]
    fn merge_similar_in_memory() {
        let mut net = HopfieldNetwork::new();
        let p1 = make_pattern(64, 1);
        // p2 is nearly identical to p1
        let p2: Vec<f32> = p1.iter().map(|x| x + 0.001).collect();
        let p3 = make_pattern(64, 100); // Very different

        net.store(1, &p1, 0.9);
        net.store(2, &p2, 0.5);
        net.store(3, &p3, 0.8);

        let n1 = l2_normalize(&p1);
        let n2 = l2_normalize(&p2);
        let sim = cosine_similarity(&n1, &n2);
        assert!(sim > 0.99, "p1 and p2 should be very similar: sim={}", sim);

        // After merge, pattern 2 should be gone (weaker)
        // (This tests the logic; the full DB-backed merge is in recall.rs)
        if sim > 0.92 {
            net.remove(2);
            net.update_strength(1, 0.9f32.max(0.5));
        }

        assert_eq!(net.pattern_count(), 2);
        assert!(net.contains(1));
        assert!(!net.contains(2));
        assert!(net.contains(3));
    }

    // -----------------------------------------------------------------------
    // Integration: full lifecycle (DB-backed)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn full_lifecycle_with_db() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        let user_id = 1i64;
        let dim = 64;
        let mut network = HopfieldNetwork::new();

        // Store 3 patterns (strength < 1.0 so reinforcement can increase it)
        for i in 1..=3i64 {
            let p = make_pattern(dim, (i * 10) as u8);
            recall::store_pattern(&db, &mut network, i, &p, user_id, 5, 0.5)
                .await
                .expect("store");
        }
        assert_eq!(network.pattern_count(), 3);

        // Recall pattern 1
        let p1 = make_pattern(dim, 10);
        let results = recall::recall_pattern(&db, &network, &p1, user_id, 3, DEFAULT_BETA)
            .await
            .expect("recall");
        assert!(!results.is_empty());
        assert_eq!(results[0].pattern_id, 1);

        // Reinforce pattern 1
        let old_s = network.strength(1).unwrap();
        let new_s = recall::reinforce(&db, &mut network, 1, user_id)
            .await
            .expect("reinforce");
        assert!(new_s > old_s);

        // Decay
        let stats = recall::decay_tick(&db, &mut network, user_id, 10)
            .await
            .expect("decay");
        assert!(stats.patterns_decayed > 0);

        // All patterns should still be alive after 10 ticks
        assert_eq!(network.pattern_count(), 3);

        // Store a very weak pattern and prune it
        let weak = make_pattern(dim, 200);
        recall::store_pattern(&db, &mut network, 99, &weak, user_id, 1, 0.01)
            .await
            .expect("store weak");
        assert_eq!(network.pattern_count(), 4);

        let pruned = recall::prune_weak(&db, &mut network, user_id, DEATH_THRESHOLD)
            .await
            .expect("prune");
        assert_eq!(pruned, 1);
        assert_eq!(network.pattern_count(), 3);
        assert!(!network.contains(99));
    }

    #[tokio::test]
    async fn merge_similar_with_db() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        let user_id = 1i64;
        let dim = 64;
        let mut network = HopfieldNetwork::new();

        let p1 = make_pattern(dim, 1);
        let p2: Vec<f32> = p1.iter().map(|x| x + 0.0001).collect(); // Near-identical
        let p3 = make_pattern(dim, 100); // Different

        recall::store_pattern(&db, &mut network, 1, &p1, user_id, 5, 0.9)
            .await
            .unwrap();
        recall::store_pattern(&db, &mut network, 2, &p2, user_id, 5, 0.5)
            .await
            .unwrap();
        recall::store_pattern(&db, &mut network, 3, &p3, user_id, 5, 0.8)
            .await
            .unwrap();

        let merged = recall::merge_similar(&db, &mut network, user_id, 0.92)
            .await
            .unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, 1); // Winner
        assert_eq!(merged[0].1, 2); // Loser
        assert_eq!(network.pattern_count(), 2);
    }
}
