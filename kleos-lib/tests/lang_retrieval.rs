//! Multi-language retrieval quality harness. Prints recall@5 and MRR per
//! language so each phase of the language-agnostic plan has a before/after
//! baseline. English is the regression guard.
use std::fs;
use std::path::PathBuf;

use kleos_lib::config::Config;
use kleos_lib::db::Database;
use kleos_lib::embeddings::onnx::OnnxProvider;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::memory;
use kleos_lib::memory::search::hybrid_search_reranked;
use kleos_lib::memory::types::{SearchBudget, SearchRequest, StoreRequest};
use kleos_lib::reranker::{OnnxReranker, Reranker};

/// One indexed document in a language fixture.
#[derive(serde::Deserialize)]
struct CorpusRow {
    /// Fixture-local id, referenced by query rows' `expect_id`.
    id: i64,
    /// Document text stored verbatim through the normal write path.
    content: String,
}

/// One query with the corpus id it should retrieve.
#[derive(serde::Deserialize)]
struct QueryRow {
    /// Natural-language query run through reranked hybrid search.
    query: String,
    /// Corpus id the query is expected to surface in the top results.
    expect_id: i64,
}

/// Load a `.jsonl` fixture file for `lang` ("de"/"fr"/"en") by `name`.
fn load<T: serde::de::DeserializeOwned>(lang: &str, name: &str) -> Vec<T> {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/lang");
    p.push(lang);
    p.push(name);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid jsonl"))
        .collect()
}

/// Ingest the corpus, run every query, print recall@5 and MRR for `lang`.
async fn run_eval(lang: &str) {
    let corpus: Vec<CorpusRow> = load(lang, "corpus.jsonl");
    let queries: Vec<QueryRow> = load(lang, "queries.jsonl");

    // File-backed temp DB so LanceDB vector search is exercised (connect_memory
    // would set vector_index = None and degrade to FTS-only).
    let temp_dir = tempfile::tempdir().expect("temp eval dir");
    let config = Config {
        db_path: temp_dir
            .path()
            .join("eval.sqlite")
            .to_string_lossy()
            .into_owned(),
        lance_index_path: Some(temp_dir.path().join("lance").to_string_lossy().into_owned()),
        ..Default::default()
    };
    let db = Database::connect_with_config(&config, None)
        .await
        .expect("fresh eval db");
    let embedder = OnnxProvider::new(&config).await.expect("onnx embedder");
    let reranker: std::sync::Arc<dyn Reranker> =
        std::sync::Arc::new(OnnxReranker::new(&config).await.expect("onnx reranker"));
    // Cover the full reranker input window, but never fewer than 10 results.
    let search_limit = config.reranker_top_k.max(10);
    // Maps fixture-local ids to the database ids assigned on store.
    let mut id_map = std::collections::HashMap::new();

    for row in &corpus {
        let stored = memory::store_with_chunks(
            &db,
            &embedder,
            StoreRequest {
                content: row.content.clone(),
                source: "lang_retrieval_eval".to_string(),
                user_id: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("store fixture row");
        id_map.insert(row.id, stored.id);
    }

    let (mut hits_at_5, mut mrr_sum) = (0usize, 0.0f64);
    for q in &queries {
        let emb = embedder.embed(&q.query).await.expect("query embedding");
        let results = hybrid_search_reranked(
            &db,
            SearchRequest {
                query: q.query.clone(),
                embedding: Some(emb),
                limit: Some(search_limit),
                user_id: Some(1),
                budget: Some(SearchBudget::High),
                ..Default::default()
            },
            &q.query,
            Some(std::sync::Arc::clone(&reranker)),
        )
        .await
        .expect("reranked hybrid search");
        let results: Vec<i64> = results.iter().map(|r| r.memory.id).collect();
        let want = id_map.get(&q.expect_id).copied().unwrap_or(q.expect_id);
        if let Some(pos) = results.iter().position(|id| *id == want) {
            if pos < 5 {
                hits_at_5 += 1;
            }
            mrr_sum += 1.0 / (pos as f64 + 1.0);
        }
    }

    let n = queries.len() as f64;
    eprintln!(
        "{} recall@5 = {:.3}  MRR = {:.3}",
        lang.to_uppercase(),
        hits_at_5 as f64 / n,
        mrr_sum / n
    );
    assert!(!queries.is_empty());
}

/// German retrieval eval: prints recall@5 and MRR over the `de` fixtures.
#[tokio::test]
#[ignore = "retrieval eval loads local ONNX models; run explicitly with --ignored"]
async fn de_retrieval() {
    run_eval("de").await;
}

/// French retrieval eval: prints recall@5 and MRR over the `fr` fixtures.
#[tokio::test]
#[ignore = "retrieval eval loads local ONNX models; run explicitly with --ignored"]
async fn fr_retrieval() {
    run_eval("fr").await;
}

/// English regression canary: prints recall@5 and MRR over the `en` fixtures.
/// Used to detect non-English changes that silently hurt English retrieval.
#[tokio::test]
#[ignore = "retrieval eval loads local ONNX models; run explicitly with --ignored"]
async fn en_retrieval_canary() {
    run_eval("en").await;
}
