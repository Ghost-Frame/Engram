use engram_lib::db::Database;
use engram_lib::graph::pagerank::{compute_pagerank_for_user, persist_pagerank};
use engram_lib::memory;
use engram_lib::memory::search::hybrid_search;
use engram_lib::memory::types::{QuestionType, SearchRequest, StoreRequest};
use std::cmp::min;
use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Args {
    memories: usize,
    fanout: usize,
    samples: usize,
    limit: usize,
    query: String,
    user_id: i64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            memories: 5_000,
            fanout: 4,
            samples: 20,
            limit: 20,
            query: "pagerank benchmark token".to_string(),
            user_id: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct Stats {
    min_ms: f64,
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

fn usage() -> &'static str {
    "Usage: cargo run -p engram-lib --example pagerank_benchmark --release -- [options]\n\n\
Options:\n\
  --memories <n>   Number of memories to create (default: 5000)\n\
  --fanout <n>     Outgoing ring links per memory (default: 4)\n\
  --samples <n>    Timed search samples per scenario (default: 20)\n\
  --limit <n>      Search result limit (default: 20)\n\
  --query <text>   Search query token set (default: \"pagerank benchmark token\")\n\
  --user-id <id>   Benchmark user id (default: 1)\n\
  -h, --help       Show this help"
}

fn parse_value<T: std::str::FromStr>(flag: &str, value: Option<String>) -> Result<T, String> {
    let raw = value.ok_or_else(|| format!("missing value for {flag}"))?;
    raw.parse::<T>()
        .map_err(|_| format!("invalid value for {flag}: {raw}"))
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--memories" => args.memories = parse_value("--memories", iter.next())?,
            "--fanout" => args.fanout = parse_value("--fanout", iter.next())?,
            "--samples" => args.samples = parse_value("--samples", iter.next())?,
            "--limit" => args.limit = parse_value("--limit", iter.next())?,
            "--query" => {
                args.query = iter
                    .next()
                    .ok_or_else(|| "missing value for --query".to_string())?
            }
            "--user-id" => args.user_id = parse_value("--user-id", iter.next())?,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if args.memories < 2 {
        return Err("--memories must be at least 2".to_string());
    }
    if args.samples == 0 {
        return Err("--samples must be at least 1".to_string());
    }
    if args.limit == 0 {
        return Err("--limit must be at least 1".to_string());
    }

    Ok(args)
}

fn store_request(content: &str, user_id: i64) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "benchmark".to_string(),
        source: "pagerank_benchmark".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        session_id: None,
        is_static: None,
        user_id: Some(user_id),
        space_id: None,
        parent_memory_id: None,
    }
}

fn search_request(args: &Args) -> SearchRequest {
    SearchRequest {
        query: args.query.clone(),
        embedding: None,
        limit: Some(args.limit),
        category: None,
        source: None,
        tags: None,
        threshold: None,
        user_id: Some(args.user_id),
        space_id: None,
        include_forgotten: None,
        mode: None,
        question_type: Some(QuestionType::FactRecall),
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    }
}

async fn build_dataset(db: &Database, args: &Args) -> engram_lib::Result<(usize, usize)> {
    let mut ids = Vec::with_capacity(args.memories);

    for i in 0..args.memories {
        let content = format!(
            "{} node_{i} cluster_{} shard_{} pulse_{} bridge_{}",
            args.query,
            i % 23,
            i % 37,
            i * 17,
            i * 31
        );
        let stored = memory::store(db, store_request(&content, args.user_id)).await?;
        if stored.created {
            ids.push(stored.id);
        }
    }

    if ids.len() < 2 {
        return Ok((ids.len(), 0));
    }

    let mut edges = 0usize;
    let hub = ids[0];
    for &id in ids.iter().skip(1) {
        memory::insert_link(db, id, hub, 1.0, "causes", args.user_id).await?;
        edges += 1;
    }

    let ring_fanout = min(args.fanout, ids.len().saturating_sub(1));
    for index in 0..ids.len() {
        for step in 1..=ring_fanout {
            let source = ids[index];
            let target = ids[(index + step) % ids.len()];
            let similarity = (0.95 - (step as f64 * 0.05)).max(0.5);
            let link_type = if step == 1 { "related" } else { "extends" };
            memory::insert_link(db, source, target, similarity, link_type, args.user_id).await?;
            edges += 1;
        }
    }

    Ok((ids.len(), edges))
}

async fn clear_pagerank_cache(db: &Database, user_id: i64) -> engram_lib::Result<()> {
    db.connection()
        .execute(
            "DELETE FROM memory_pagerank WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    db.connection()
        .execute(
            "DELETE FROM pagerank_dirty WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    Ok(())
}

async fn warm_pagerank_cache(db: &Database, user_id: i64) -> engram_lib::Result<usize> {
    let scores = compute_pagerank_for_user(db, user_id).await?;
    let count = scores.len();
    persist_pagerank(db, user_id, &scores).await?;
    Ok(count)
}

async fn timed_search(db: &Database, args: &Args) -> engram_lib::Result<Duration> {
    let request = search_request(args);
    let started = Instant::now();
    let results = hybrid_search(db, request).await?;
    black_box(results.len());
    if results.is_empty() {
        return Err(engram_lib::EngError::Internal(
            "benchmark search returned no results".to_string(),
        ));
    }
    Ok(started.elapsed())
}

async fn measure_cold_searches(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    let mut samples = Vec::with_capacity(args.samples);
    for _ in 0..args.samples {
        clear_pagerank_cache(db, args.user_id).await?;
        samples.push(timed_search(db, args).await?);
    }
    Ok(samples)
}

async fn measure_warm_searches(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    warm_pagerank_cache(db, args.user_id).await?;
    let mut samples = Vec::with_capacity(args.samples);
    for _ in 0..args.samples {
        samples.push(timed_search(db, args).await?);
    }
    Ok(samples)
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn percentile_ms(sorted_ms: &[f64], percentile: f64) -> f64 {
    let last = sorted_ms.len().saturating_sub(1);
    let index = ((last as f64) * percentile).ceil() as usize;
    sorted_ms[index.min(last)]
}

fn summarize(samples: &[Duration]) -> Stats {
    let mut values: Vec<f64> = samples.iter().copied().map(duration_ms).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = values.iter().sum();
    Stats {
        min_ms: values[0],
        avg_ms: total / values.len() as f64,
        p50_ms: percentile_ms(&values, 0.50),
        p95_ms: percentile_ms(&values, 0.95),
        p99_ms: percentile_ms(&values, 0.99),
        max_ms: values[values.len() - 1],
    }
}

fn improvement_pct(before: f64, after: f64) -> f64 {
    if before == 0.0 {
        0.0
    } else {
        ((before - after) / before) * 100.0
    }
}

fn print_stats(label: &str, stats: &Stats) {
    println!(
        "{label}: min={:.2}ms avg={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
        stats.min_ms,
        stats.avg_ms,
        stats.p50_ms,
        stats.p95_ms,
        stats.p99_ms,
        stats.max_ms
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    if cfg!(debug_assertions) {
        eprintln!("warning: debug build detected; use --release for meaningful latency numbers");
    }

    let db = Database::connect_memory().await?;
    let setup_started = Instant::now();
    let (memories, edges) = build_dataset(&db, &args).await?;
    let setup_ms = duration_ms(setup_started.elapsed());

    println!("PageRank Cache Benchmark");
    println!(
        "dataset: memories={} edges={} fanout={} samples={} limit={} setup={:.2}ms",
        memories, edges, args.fanout, args.samples, args.limit, setup_ms
    );
    println!("query: {}", args.query);

    let cold = measure_cold_searches(&db, &args).await?;
    let warm = measure_warm_searches(&db, &args).await?;
    let cold_stats = summarize(&cold);
    let warm_stats = summarize(&warm);

    print_stats("cold first-query fallback", &cold_stats);
    print_stats("warm cached search", &warm_stats);
    println!(
        "improvement: p50={:.1}% p95={:.1}% p99={:.1}%",
        improvement_pct(cold_stats.p50_ms, warm_stats.p50_ms),
        improvement_pct(cold_stats.p95_ms, warm_stats.p95_ms),
        improvement_pct(cold_stats.p99_ms, warm_stats.p99_ms)
    );

    Ok(())
}
