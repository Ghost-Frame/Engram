//! Offline retrieval-eval harness for store/search/recall quality.
//!
//! Seeds a deterministic in-memory corpus from `fixtures/retrieval/golden.json`,
//! runs `hybrid_search` for each labelled query, and reports recall@k, MRR, and
//! nDCG@10 per query and aggregated by class. The in-memory test DB exercises the
//! FTS5 + RRF fusion + structured-filter path only (sqlite-vec / LanceDB are not
//! loaded here, so the vector channel is inert and `embedding` is None).
//!
//! Purpose: give the store/search/recall improvement work a measurable baseline so
//! ranking changes are scored, not guessed. The `stable` queries are a hard CI floor.
//! The `probe_*` queries deliberately expose two known correctness bugs (FTS
//! implicit-AND, and truncate-before-filter); their metrics are reported now and will
//! jump when Phase 1.3 and Phase 1.4 land, at which point their asserts are enabled.

use std::collections::{HashMap, HashSet};

use kleos_lib::db::Database;
use kleos_lib::memory;
use kleos_lib::memory::search::hybrid_search;
use kleos_lib::memory::types::{SearchRequest, StoreRequest};
use serde_json::Value;

/// Embedded golden fixture (corpus + labelled queries). Path is relative to this file.
const FIXTURE: &str = include_str!("fixtures/retrieval/golden.json");

/// Single owner id for the whole harness (monolith in-memory DB is single-tenant here).
const UID: i64 = 1;

/// Minimum mean recall@10 the `stable` query set must hold. Current code passes this;
/// it is the regression floor for every later ranking change.
const STABLE_RECALL_AT_10_FLOOR: f64 = 0.95;

/// Build a store request for one labelled corpus memory (no embedding: FTS path only).
fn corpus_store_req(
    content: &str,
    category: &str,
    tags: Vec<String>,
    importance: i32,
) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: category.to_string(),
        source: "eval".to_string(),
        importance,
        tags: Some(tags),
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: Some(false),
        user_id: Some(UID),
        space_id: None,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Fraction of the relevant set that appears in the top-k ranked keys.
fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    // An empty relevant set is a fixture-authoring mistake, not a 1.0 freebie: silently
    // returning perfect recall would let a query with no labels inflate the class mean and
    // mask a regression. Fail loudly so the bad fixture is fixed.
    assert!(
        !relevant.is_empty(),
        "recall@k: query has an empty `relevant` set; every fixture query must label >=1 doc"
    );
    // Count distinct relevant keys found within the first k ranked results.
    let hits = ranked
        .iter()
        .take(k)
        .filter(|key| relevant.contains(*key))
        .count();
    hits as f64 / relevant.len() as f64
}

/// Reciprocal rank of the first relevant result (1-indexed); 0 when none appear.
fn mrr(ranked: &[String], relevant: &HashSet<String>) -> f64 {
    // Scan in rank order and return as soon as the first relevant key is seen.
    for (idx, key) in ranked.iter().enumerate() {
        if relevant.contains(key) {
            return 1.0 / (idx as f64 + 1.0);
        }
    }
    0.0
}

/// Binary-relevance normalized discounted cumulative gain over the top-k ranked keys.
fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    // Discounted cumulative gain of the observed ranking (gain 1 for relevant, else 0).
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(idx, key)| {
            if relevant.contains(key) {
                1.0 / ((idx as f64 + 2.0).log2())
            } else {
                0.0
            }
        })
        .sum();
    // Ideal DCG places every relevant item at the top, capped at k.
    let ideal_hits = relevant.len().min(k);
    let idcg: f64 = (0..ideal_hits)
        .map(|idx| 1.0 / ((idx as f64 + 2.0).log2()))
        .sum();
    if idcg == 0.0 {
        return 1.0;
    }
    dcg / idcg
}

/// Read a required string field from a JSON object, panicking with context if absent.
fn str_field<'a>(obj: &'a Value, field: &str) -> &'a str {
    obj.get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("fixture object missing string field `{field}`: {obj}"))
}

/// One row of the printed eval report.
struct QueryScore {
    /// Query id from the fixture.
    id: String,
    /// Query class (`stable`, `probe_fts_or`, `probe_filter`).
    class: String,
    /// recall@5.
    r5: f64,
    /// recall@10.
    r10: f64,
    /// Mean reciprocal rank.
    mrr: f64,
    /// nDCG@10.
    ndcg: f64,
}

/// Seed the golden corpus, score every labelled query, print the report, and enforce the
/// per-class recall@10 floors (stable + the two locked-in bug probes).
#[tokio::test]
async fn retrieval_eval_golden() {
    // Parse the embedded fixture once.
    let root: Value = serde_json::from_str(FIXTURE).expect("golden.json parses");
    let corpus = root["corpus"].as_array().expect("corpus array");
    let queries = root["queries"].as_array().expect("queries array");

    // Fresh single-tenant in-memory DB.
    let db = Database::connect_memory().await.expect("connect_memory");

    // Seed the corpus, recording key -> row id and the reverse id -> key map.
    let mut key_to_id: HashMap<String, i64> = HashMap::new();
    let mut id_to_key: HashMap<i64, String> = HashMap::new();
    for item in corpus {
        let key = str_field(item, "key").to_string();
        let content = str_field(item, "content");
        let category = str_field(item, "category");
        let importance = item["importance"].as_i64().unwrap_or(5) as i32;
        let tags: Vec<String> = item["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let res = memory::store(
            &db,
            corpus_store_req(content, category, tags, importance),
            None,
            false,
        )
        .await
        .expect("store corpus memory");
        key_to_id.insert(key.clone(), res.id);
        id_to_key.insert(res.id, key);
    }

    // Dedup sanity: every distinct fixture row must persist as its own memory. If
    // simhash dedup collapses near-duplicate filler rows this assert catches it and
    // the fixture must be made more distinct.
    assert_eq!(
        key_to_id.len(),
        corpus.len(),
        "every corpus row must store as a distinct memory (simhash dedup collapsed one)"
    );

    // Score every query.
    let mut scores: Vec<QueryScore> = Vec::with_capacity(queries.len());
    for q in queries {
        let id = str_field(q, "id").to_string();
        let class = str_field(q, "class").to_string();
        let query_text = str_field(q, "q").to_string();
        let limit = q["limit"].as_u64().unwrap_or(20) as usize;
        let filter_category = q
            .get("filter_category")
            .and_then(Value::as_str)
            .map(str::to_string);
        let relevant: HashSet<String> = q["relevant"]
            .as_array()
            .expect("relevant array")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();

        // Run the hybrid pipeline exactly as a real caller would (FTS channel only here).
        let req = SearchRequest {
            query: query_text,
            user_id: Some(UID),
            limit: Some(limit),
            category: filter_category,
            ..Default::default()
        };
        let results = hybrid_search(&db, req).await.expect("hybrid_search");

        // Map result row ids back to fixture keys, preserving rank order.
        let ranked: Vec<String> = results
            .iter()
            .filter_map(|r| id_to_key.get(&r.memory.id).cloned())
            .collect();

        scores.push(QueryScore {
            id,
            class,
            r5: recall_at_k(&ranked, &relevant, 5),
            r10: recall_at_k(&ranked, &relevant, 10),
            mrr: mrr(&ranked, &relevant),
            ndcg: ndcg_at_k(&ranked, &relevant, 10),
        });
    }

    // Print the per-query report (visible with `--nocapture`).
    println!("\n=== retrieval eval: golden.json ===");
    println!(
        "{:<24} {:<14} {:>6} {:>6} {:>6} {:>7}",
        "query", "class", "R@5", "R@10", "MRR", "nDCG@10"
    );
    for s in &scores {
        println!(
            "{:<24} {:<14} {:>6.2} {:>6.2} {:>6.2} {:>7.2}",
            s.id, s.class, s.r5, s.r10, s.mrr, s.ndcg
        );
    }

    // Aggregate means by class and print a compact machine-readable summary line.
    let mut by_class: HashMap<&str, Vec<&QueryScore>> = HashMap::new();
    for s in &scores {
        by_class.entry(s.class.as_str()).or_default().push(s);
    }
    println!("\n--- class means ---");
    let mut summary = serde_json::Map::new();
    let mut class_names: Vec<&&str> = by_class.keys().collect();
    class_names.sort();
    for class in class_names {
        let rows = &by_class[*class];
        let n = rows.len() as f64;
        let mean = |f: &dyn Fn(&QueryScore) -> f64| rows.iter().map(|s| f(s)).sum::<f64>() / n;
        let mr5 = mean(&|s| s.r5);
        let mr10 = mean(&|s| s.r10);
        let mmrr = mean(&|s| s.mrr);
        let mndcg = mean(&|s| s.ndcg);
        println!(
            "{:<14} n={:<2} R@5={:.3} R@10={:.3} MRR={:.3} nDCG@10={:.3}",
            class,
            rows.len(),
            mr5,
            mr10,
            mmrr,
            mndcg
        );
        summary.insert(
            (*class).to_string(),
            serde_json::json!({ "n": rows.len(), "r5": mr5, "r10": mr10, "mrr": mmrr, "ndcg": mndcg }),
        );
    }
    println!("EVAL_SUMMARY {}", Value::Object(summary));

    // Hard CI floor: every query class must hold its recall@10. `stable` is the always-on
    // floor; `probe_fts_or` (FTS OR-fusion, Phase 1.3) and `probe_filter` (over-fetch before
    // filter, Phase 1.4) are locked in now that both fixes have landed, so a regression that
    // reintroduces implicit-AND or truncate-before-filter fails this test.
    let class_floors: &[&str] = &["stable", "probe_fts_or", "probe_filter"];
    for class in class_floors {
        let rows: Vec<&QueryScore> = scores.iter().filter(|s| &s.class == class).collect();
        assert!(!rows.is_empty(), "fixture must contain `{class}` queries");
        let mean_r10 = rows.iter().map(|s| s.r10).sum::<f64>() / rows.len() as f64;
        assert!(
            mean_r10 >= STABLE_RECALL_AT_10_FLOOR,
            "`{class}` mean recall@10 {mean_r10:.3} fell below floor {STABLE_RECALL_AT_10_FLOOR:.3}"
        );
        // probe_filter's three relevant docs all fit within k=5, so recall@10 is trivially 1.0
        // and would still pass if over-fetch regressed to return only two of three. Assert
        // recall@5 instead: it is the sensitive metric that catches a dropped in-category doc.
        if *class == "probe_filter" {
            let mean_r5 = rows.iter().map(|s| s.r5).sum::<f64>() / rows.len() as f64;
            assert!(
                mean_r5 >= 0.999,
                "`probe_filter` mean recall@5 {mean_r5:.3} below 1.0; over-fetch-before-filter regressed"
            );
        }
    }
}
