//! Comprehensive zero-LLM database benchmark for Engram.
//!
//! Measures pure database performance with no embedding or LLM calls.
//! Covers: store throughput, FTS search, hybrid search (FTS-only),
//! hydration, graph neighbors, link insertion, and PageRank computation.
//!
//! Usage:
//!   cargo run -p engram-lib --example db_benchmark --release -- [options]
//!
//! Options:
//!   --memories <n>    Number of memories to seed (default: 5000)
//!   --fanout <n>      Ring links per memory (default: 4)
//!   --samples <n>     Timed iterations per scenario (default: 20)
//!   --limit <n>       Search result limit (default: 10)
//!   --query <text>    Search query (default: "server deployment config")
//!   --user-id <id>    Benchmark user id (default: 1)
//!   -h, --help        Show help

use engram_lib::db::Database;
use engram_lib::graph::pagerank::{compute_pagerank_for_user, persist_pagerank};
use engram_lib::memory;
use engram_lib::memory::fts::fts_search;
use engram_lib::memory::search::hybrid_search;
use engram_lib::memory::types::{QuestionType, SearchRequest, StoreRequest};
use std::cmp::min;
use std::hint::black_box;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

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
            limit: 10,
            query: "server deployment config".to_string(),
            user_id: 1,
        }
    }
}

fn usage() -> &'static str {
    "Usage: cargo run -p engram-lib --example db_benchmark --release -- [options]\n\n\
Options:\n\
  --memories <n>    Number of memories to seed (default: 5000)\n\
  --fanout <n>      Ring links per memory (default: 4)\n\
  --samples <n>     Timed iterations per scenario (default: 20)\n\
  --limit <n>       Search result limit (default: 10)\n\
  --query <text>    Search query (default: \"server deployment config\")\n\
  --user-id <id>    Benchmark user id (default: 1)\n\
  -h, --help        Show this help"
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
    if args.memories < 10 {
        return Err("--memories must be at least 10".to_string());
    }
    if args.samples == 0 {
        return Err("--samples must be at least 1".to_string());
    }
    Ok(args)
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Stats {
    min_ms: f64,
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

fn duration_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percentile_ms(sorted: &[f64], p: f64) -> f64 {
    let last = sorted.len().saturating_sub(1);
    let idx = ((last as f64) * p).ceil() as usize;
    sorted[idx.min(last)]
}

fn summarize(samples: &[Duration]) -> Stats {
    let mut v: Vec<f64> = samples.iter().copied().map(duration_ms).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = v.iter().sum();
    Stats {
        min_ms: v[0],
        avg_ms: total / v.len() as f64,
        p50_ms: percentile_ms(&v, 0.50),
        p95_ms: percentile_ms(&v, 0.95),
        p99_ms: percentile_ms(&v, 0.99),
        max_ms: v[v.len() - 1],
    }
}

fn print_stats(label: &str, stats: &Stats) {
    println!(
        "  {label:<32} min={:>8.2}ms  avg={:>8.2}ms  p50={:>8.2}ms  p95={:>8.2}ms  p99={:>8.2}ms  max={:>8.2}ms",
        stats.min_ms, stats.avg_ms, stats.p50_ms, stats.p95_ms, stats.p99_ms, stats.max_ms
    );
}

fn print_throughput(label: &str, count: usize, elapsed: Duration) {
    let ops = count as f64 / elapsed.as_secs_f64();
    println!(
        "  {label:<32} {count} ops in {:.2}ms  ({:.0} ops/sec)",
        duration_ms(elapsed),
        ops
    );
}

// ---------------------------------------------------------------------------
// Data helpers
// ---------------------------------------------------------------------------

/// Realistic-ish memory content with varied vocabulary for FTS.
const TOPICS: &[&str] = &[
    "server deployment config nginx proxy reverse",
    "database migration schema index sqlite rusqlite",
    "authentication api key token bearer scope",
    "memory search hybrid vector fts ranking",
    "kubernetes pod container orchestration helm",
    "monitoring prometheus grafana alerting metrics",
    "ci cd pipeline github actions workflow",
    "backup restore snapshot replication failover",
    "networking vpn wireguard tailscale headscale",
    "security audit vulnerability patch cve",
    "infrastructure terraform ansible provisioning",
    "logging structured tracing opentelemetry spans",
    "caching redis memcached invalidation ttl",
    "queue rabbitmq kafka nats message broker",
    "dns domain nameserver record cname alias",
    "ssl tls certificate letsencrypt renewal",
    "storage s3 minio blob object bucket",
    "user preference dark mode theme language",
    "project timeline milestone deadline sprint",
    "debugging profiling flame graph bottleneck",
    "refactoring legacy code technical debt cleanup",
    "testing integration unit mock fixture assertion",
    "documentation api spec openapi swagger",
];

fn make_content(index: usize, query: &str) -> String {
    let topic = TOPICS[index % TOPICS.len()];
    format!(
        "{query} node_{index} -- {topic} cluster_{} shard_{} rev_{}",
        index % 23,
        index % 37,
        index % 13,
    )
}

fn store_request(content: &str, user_id: i64) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "benchmark".to_string(),
        source: "db_benchmark".to_string(),
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

// ---------------------------------------------------------------------------
// Benchmark scenarios
// ---------------------------------------------------------------------------

/// 1. Store throughput -- bulk insert
async fn bench_store_throughput(
    db: &Database,
    args: &Args,
) -> engram_lib::Result<(Vec<i64>, Duration)> {
    let mut ids = Vec::with_capacity(args.memories);
    let start = Instant::now();
    for i in 0..args.memories {
        let content = make_content(i, &args.query);
        let result = memory::store(db, store_request(&content, args.user_id)).await?;
        if result.created {
            ids.push(result.id);
        }
    }
    let elapsed = start.elapsed();
    Ok((ids, elapsed))
}

/// 2. Link insertion throughput -- ring + hub topology
async fn bench_link_insertion(
    db: &Database,
    ids: &[i64],
    args: &Args,
) -> engram_lib::Result<(usize, Duration)> {
    let start = Instant::now();
    let mut edges = 0usize;

    // Hub: first node connects to all others
    let hub = ids[0];
    for &id in ids.iter().skip(1) {
        memory::insert_link(db, id, hub, 1.0, "causes", args.user_id).await?;
        edges += 1;
    }

    // Ring: each node links to its next `fanout` neighbors
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

    let elapsed = start.elapsed();
    Ok((edges, elapsed))
}

/// 3. FTS search latency
async fn bench_fts_search(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    let queries = &[
        "server deployment config",
        "database migration schema",
        "kubernetes container orchestration",
        "authentication token bearer",
        "monitoring prometheus alerting",
        "backup restore replication",
        "security audit vulnerability",
        "networking vpn wireguard",
    ];

    let mut samples = Vec::with_capacity(args.samples);
    for i in 0..args.samples {
        let q = queries[i % queries.len()];
        let start = Instant::now();
        let hits = fts_search(db, q, args.limit, args.user_id).await?;
        black_box(hits.len());
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 4. Hybrid search (FTS-only, no embeddings)
async fn bench_hybrid_search(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    let queries = &[
        "server deployment config",
        "database migration schema",
        "kubernetes container orchestration",
        "authentication token bearer",
        "security audit vulnerability",
    ];

    let mut samples = Vec::with_capacity(args.samples);
    for i in 0..args.samples {
        let mut req = search_request(args);
        req.query = queries[i % queries.len()].to_string();
        let start = Instant::now();
        let results = hybrid_search(db, req).await?;
        black_box(results.len());
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 5. Hybrid search with include_links=true (exercises the N+1 path)
async fn bench_hybrid_with_links(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    let mut samples = Vec::with_capacity(args.samples);
    for _ in 0..args.samples {
        let mut req = search_request(args);
        req.include_links = true;
        let start = Instant::now();
        let results = hybrid_search(db, req).await?;
        black_box(results.len());
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 6. Hydration latency -- sequential get by ID
async fn bench_hydration(
    db: &Database,
    ids: &[i64],
    args: &Args,
) -> engram_lib::Result<Vec<Duration>> {
    let batch_size = args.limit;
    let mut samples = Vec::with_capacity(args.samples);
    for i in 0..args.samples {
        let offset = (i * batch_size) % ids.len().saturating_sub(batch_size).max(1);
        let batch: Vec<i64> = ids[offset..min(offset + batch_size, ids.len())].to_vec();
        let start = Instant::now();
        for &id in &batch {
            let mem = memory::get(db, id, args.user_id).await;
            black_box(&mem);
        }
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 7. Graph neighbor fetch (single-hop)
async fn bench_graph_neighbors(
    db: &Database,
    ids: &[i64],
    args: &Args,
) -> engram_lib::Result<Vec<Duration>> {
    let mut samples = Vec::with_capacity(args.samples);
    for i in 0..args.samples {
        let seed = ids[i % ids.len()];
        let start = Instant::now();
        let links = memory::get_links_for(db, seed, args.user_id).await?;
        black_box(links.len());
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 8. PageRank computation
async fn bench_pagerank(db: &Database, args: &Args) -> engram_lib::Result<Vec<Duration>> {
    let cap = args.samples.min(5); // PageRank is expensive, cap iterations
    let mut samples = Vec::with_capacity(cap);
    let user_id = args.user_id;
    for _ in 0..cap {
        // Clear cache
        db.write(move |conn| {
            conn.execute(
                "DELETE FROM memory_pagerank WHERE user_id = ?1",
                rusqlite::params![user_id],
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            conn.execute(
                "DELETE FROM pagerank_dirty WHERE user_id = ?1",
                rusqlite::params![user_id],
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

        let start = Instant::now();
        let scores = compute_pagerank_for_user(db, args.user_id).await?;
        persist_pagerank(db, args.user_id, &scores).await?;
        black_box(scores.len());
        samples.push(start.elapsed());
    }
    Ok(samples)
}

/// 9. Single memory get by ID
async fn bench_single_get(
    db: &Database,
    ids: &[i64],
    args: &Args,
) -> engram_lib::Result<Vec<Duration>> {
    let mut samples = Vec::with_capacity(args.samples);
    for i in 0..args.samples {
        let id = ids[i % ids.len()];
        let start = Instant::now();
        let mem = memory::get(db, id, args.user_id).await?;
        black_box(&mem);
        samples.push(start.elapsed());
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    if cfg!(debug_assertions) {
        eprintln!("WARNING: debug build detected -- use --release for meaningful numbers");
    }

    println!("=== Engram Database Benchmark (zero-LLM) ===");
    println!(
        "config: memories={} fanout={} samples={} limit={} user_id={}",
        args.memories, args.fanout, args.samples, args.limit, args.user_id
    );
    println!("query: \"{}\"", args.query);
    println!();

    let db = Database::connect_memory().await?;

    // -- 1. Store throughput --
    print!("seeding {} memories... ", args.memories);
    let (ids, store_elapsed) = bench_store_throughput(&db, &args).await?;
    println!("done");
    print_throughput("store (bulk insert)", ids.len(), store_elapsed);
    println!();

    // -- 2. Link insertion --
    print!("building link graph (fanout={})... ", args.fanout);
    let (edge_count, link_elapsed) = bench_link_insertion(&db, &ids, &args).await?;
    println!("done");
    print_throughput("link insertion", edge_count, link_elapsed);
    println!();

    // -- 3. Single get --
    println!("--- single memory::get ---");
    let single_get = bench_single_get(&db, &ids, &args).await?;
    print_stats("get by id", &summarize(&single_get));
    println!();

    // -- 4. FTS search --
    println!("--- FTS search ---");
    let fts = bench_fts_search(&db, &args).await?;
    print_stats("fts_search", &summarize(&fts));
    println!();

    // -- 5. Hybrid search (FTS-only) --
    println!("--- hybrid search (no embeddings) ---");
    let hybrid = bench_hybrid_search(&db, &args).await?;
    print_stats("hybrid_search", &summarize(&hybrid));
    println!();

    // -- 6. Hybrid search with links --
    println!("--- hybrid search + include_links ---");
    let hybrid_links = bench_hybrid_with_links(&db, &args).await?;
    print_stats("hybrid+links", &summarize(&hybrid_links));
    println!();

    // -- 7. Hydration (batch get) --
    println!("--- hydration (batch get x{}) ---", args.limit);
    let hydration = bench_hydration(&db, &ids, &args).await?;
    print_stats("batch hydration", &summarize(&hydration));
    println!();

    // -- 8. Graph neighbors --
    println!("--- graph neighbor fetch (1-hop) ---");
    let graph = bench_graph_neighbors(&db, &ids, &args).await?;
    print_stats("graph neighbors", &summarize(&graph));
    println!();

    // -- 9. PageRank (capped at 5 samples) --
    println!("--- PageRank compute + persist ---");
    let pr = bench_pagerank(&db, &args).await?;
    print_stats("pagerank full cycle", &summarize(&pr));
    println!();

    // -- Summary --
    println!("=== Summary ===");
    println!(
        "  dataset: {} memories, {} edges",
        ids.len(),
        edge_count
    );
    print_throughput("store throughput", ids.len(), store_elapsed);
    print_throughput("link throughput", edge_count, link_elapsed);
    println!(
        "  fts p50={:.2}ms  hybrid p50={:.2}ms  hybrid+links p50={:.2}ms",
        summarize(&fts).p50_ms,
        summarize(&hybrid).p50_ms,
        summarize(&hybrid_links).p50_ms,
    );
    println!(
        "  single_get p50={:.2}ms  graph_neighbors p50={:.2}ms  pagerank p50={:.2}ms",
        summarize(&single_get).p50_ms,
        summarize(&graph).p50_ms,
        summarize(&pr).p50_ms,
    );

    Ok(())
}
