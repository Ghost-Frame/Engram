#[cfg(test)]
mod instincts_tests {
    use crate::brain::hopfield::network::HopfieldNetwork;
    use crate::brain::instincts::{load_instincts_bin, seed_instincts, GHOST_STRENGTH};

    fn corpus_path() -> std::path::PathBuf {
        let manifest = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest)
            .join("data")
            .join("instincts.bin")
    }

    // -----------------------------------------------------------------------
    // Test: binary file loads correctly
    // -----------------------------------------------------------------------

    #[test]
    fn load_bin_produces_memories() {
        let path = corpus_path();
        if !path.exists() {
            // Skip if file not present in this environment
            eprintln!("instincts.bin not found at {:?}, skipping", path);
            return;
        }

        let corpus = load_instincts_bin(&path).expect("load instincts.bin");

        // Should have a meaningful number of synthetic memories
        assert!(
            corpus.memories.len() >= 10,
            "expected at least 10 memories, got {}",
            corpus.memories.len()
        );

        // All IDs should be negative (ghost memories)
        for mem in &corpus.memories {
            assert!(
                mem.id < 0,
                "ghost memory id should be negative, got {}",
                mem.id
            );
        }

        // Embeddings should be non-empty
        for mem in &corpus.memories {
            assert!(
                !mem.embedding.is_empty(),
                "memory {} has empty embedding",
                mem.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test: seeding is idempotent
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn seed_idempotent() {
        let path = corpus_path();
        if !path.exists() {
            eprintln!("instincts.bin not found at {:?}, skipping", path);
            return;
        }

        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        let user_id = 1i64;
        let mut network = HopfieldNetwork::new();

        // First seed
        let count1 = seed_instincts(&db, &mut network, user_id)
            .await
            .expect("first seed");

        assert!(
            count1 > 0,
            "first seed should insert patterns, got {}",
            count1
        );

        let patterns_after_first = network.pattern_count();

        // Second seed -- should be a no-op
        let count2 = seed_instincts(&db, &mut network, user_id)
            .await
            .expect("second seed");

        assert_eq!(
            count2, 0,
            "second seed should be idempotent (returned {})",
            count2
        );

        // Network pattern count must not change
        assert_eq!(
            network.pattern_count(),
            patterns_after_first,
            "pattern count must not change on second seed"
        );
    }

    // -----------------------------------------------------------------------
    // Test: ghost patterns get GHOST_STRENGTH
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ghost_patterns_have_correct_strength() {
        let path = corpus_path();
        if !path.exists() {
            eprintln!("instincts.bin not found at {:?}, skipping", path);
            return;
        }

        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        let user_id = 1i64;
        let mut network = HopfieldNetwork::new();

        seed_instincts(&db, &mut network, user_id)
            .await
            .expect("seed");

        // All seeded ghost patterns should have GHOST_STRENGTH
        let corpus = load_instincts_bin(&path).expect("load corpus");
        for mem in corpus.memories.iter().take(5) {
            if let Some(s) = network.strength(mem.id) {
                assert!(
                    (s - GHOST_STRENGTH).abs() < 1e-5,
                    "ghost {} should have strength {}, got {}",
                    mem.id,
                    GHOST_STRENGTH,
                    s
                );
            }
        }
    }
}
