// brain/instincts/mod.rs -- synthetic pre-training corpus for fresh Engram brains.
//
// Loads ~200 ghost memories from a binary corpus file and seeds them into a
// HopfieldNetwork so a brand-new brain is never a blank slate. Ghosts use
// negative IDs and start at GHOST_STRENGTH = 0.3.
//
// Binary format (version 2):
//   [0..4]  magic  b"INST"
//   [4..8]  version u32 little-endian
//   [8..12] compressed-body length u32 little-endian
//   [12..]  gzip-compressed JSON (InstinctsCorpus)

pub mod types;

pub use types::*;

use std::io::{Read, Write};
use std::path::Path;

use crate::brain::hopfield::network::{self, HopfieldNetwork};
use crate::brain::hopfield::recall;
use crate::db::Database;
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---- Constants ----

pub const GHOST_STRENGTH: f32 = 0.3;
pub const GHOST_REPLACE_SIM: f32 = 0.85;

const INST_MAGIC: &[u8; 4] = b"INST";
// Accept versions 1 and 2 -- the bundled file is version 1, the eidolon
// generator source declares version 2 but has not yet regenerated the corpus.
const INST_VERSION_MIN: u32 = 1;
const INST_VERSION_MAX: u32 = 2;

// ---- Binary loader ----

/// Load an InstinctsCorpus from the binary .bin file.
///
/// Format: 4-byte magic "INST" + 4-byte u32 version + gzip-compressed JSON body.
pub fn load_instincts_bin(path: &Path) -> Result<InstinctsCorpus> {
    let raw = std::fs::read(path)
        .map_err(|e| EngError::Internal(format!("instincts: cannot read {:?}: {}", path, e)))?;

    // Header is: 4-byte magic + 4-byte version u32-LE + 4-byte compressed-length u32-LE
    if raw.len() < 12 {
        return Err(EngError::Internal("instincts: file too short".to_string()));
    }

    // Validate magic header
    if &raw[0..4] != INST_MAGIC {
        return Err(EngError::Internal(
            "instincts: bad magic header (expected INST)".to_string(),
        ));
    }

    // Validate version
    let version = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    if !(INST_VERSION_MIN..=INST_VERSION_MAX).contains(&version) {
        return Err(EngError::Internal(format!(
            "instincts: unsupported version {} (supported {}-{})",
            version, INST_VERSION_MIN, INST_VERSION_MAX
        )));
    }

    // 4-byte compressed-body length (bytes 8..12), then gzip body at offset 12
    let compressed = &raw[12..];
    let mut decoder = flate2::read::GzDecoder::new(compressed);
    let mut json_bytes = Vec::new();
    decoder
        .read_to_end(&mut json_bytes)
        .map_err(|e| EngError::Internal(format!("instincts: gzip decompress failed: {}", e)))?;

    // Parse JSON
    let corpus: InstinctsCorpus = serde_json::from_slice(&json_bytes)
        .map_err(|e| EngError::Internal(format!("instincts: JSON parse failed: {}", e)))?;

    Ok(corpus)
}

// ---- Seeding ----

/// Seed instinct patterns into the Hopfield network and database for a user.
///
/// Loads the corpus from the binary file bundled with the crate. Only seeds
/// if the brain currently has 0 patterns (fresh brain guard). Records
/// completion in brain_meta so re-runs are idempotent.
///
/// Returns the number of patterns actually seeded (0 if already seeded or
/// brain already had patterns).
#[tracing::instrument(skip(db, network), fields(user_id))]
pub async fn seed_instincts(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
) -> Result<usize> {
    // Check if already seeded via brain_meta
    if is_seeded(db, user_id).await? {
        return Ok(0);
    }

    // Only seed a blank brain
    let existing = crate::brain::hopfield::pattern::count_patterns(db, user_id).await?;
    if existing > 0 {
        // Brain already has patterns -- mark seeded to avoid future checks and bail
        mark_seeded(db, user_id).await?;
        return Ok(0);
    }

    // Locate the bundled binary file
    let bin_path = instincts_bin_path();
    if !bin_path.exists() {
        return Err(EngError::Internal(format!(
            "instincts: corpus file not found at {:?}",
            bin_path
        )));
    }

    let corpus = load_instincts_bin(&bin_path)?;
    let count = corpus.memories.len();

    for mem in &corpus.memories {
        if mem.embedding.is_empty() {
            continue;
        }
        recall::store_pattern(
            db,
            network,
            mem.id,
            &mem.embedding,
            user_id,
            mem.importance,
            GHOST_STRENGTH,
        )
        .await?;
    }

    mark_seeded(db, user_id).await?;

    Ok(count)
}

// ---- Internal helpers ----

/// Return the canonical path to the bundled instincts.bin.
/// Resolves relative to CARGO_MANIFEST_DIR at compile time, falls back to
/// a runtime path via the binary's location.
fn instincts_bin_path() -> std::path::PathBuf {
    // Try compile-time manifest dir first (works in tests and dev builds)
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = std::path::PathBuf::from(manifest)
            .join("data")
            .join("instincts.bin");
        if p.exists() {
            return p;
        }
    }

    // Runtime fallback: binary dir / data / instincts.bin
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("data").join("instincts.bin");
            if p.exists() {
                return p;
            }
        }
    }

    // Last resort: relative cwd
    std::path::PathBuf::from("data/instincts.bin")
}

async fn is_seeded(db: &Database, user_id: i64) -> Result<bool> {
    let key = format!("instincts_seeded_{}", user_id);
    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT value FROM brain_meta WHERE key = ?1")
            .map_err(rusqlite_to_eng_error)?;
        let exists = stmt
            .exists(rusqlite::params![key])
            .map_err(rusqlite_to_eng_error)?;
        Ok(exists)
    })
    .await
}

async fn mark_seeded(db: &Database, user_id: i64) -> Result<()> {
    let key = format!("instincts_seeded_{}", user_id);
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.write(move |conn| {
        conn.execute(
            "INSERT OR REPLACE INTO brain_meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, now],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

// ---- Corpus generation ----

/// FNV-1a 64-bit hash of a UTF-8 string.
fn hash_content(content: &str) -> u64 {
    let mut h: u64 = 14695981039346656037u64;
    for byte in content.bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(1099511628211u64);
    }
    h
}

/// Build a deterministic 1024-dim L2-normalised embedding seeded from content.
fn make_embedding(content: &str) -> Vec<f32> {
    const DIM: usize = 1024;
    let seed = hash_content(content);
    let mut emb = vec![0.0f32; DIM];
    for (i, slot) in emb.iter_mut().enumerate() {
        let val = seed
            .wrapping_mul(6364136223846793005u64)
            .wrapping_add((i as u64).wrapping_mul(1442695040888963407u64));
        let angle = (val as f32) * (std::f32::consts::PI / (u32::MAX as f32 * 2.0));
        *slot = angle.sin();
    }
    let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for v in &mut emb {
            *v /= norm;
        }
    }
    emb
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_to_ymd(days_from_epoch: i64) -> (i32, u32, u32) {
    let mut remaining = days_from_epoch;
    let mut year = 1970i32;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let month_days: [i64; 12] = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for md in &month_days {
        if remaining < *md {
            break;
        }
        remaining -= md;
        month += 1;
    }
    (year, month, remaining as u32 + 1)
}

fn format_date(offset_hours: i64) -> String {
    // Base: 2026-01-15T00:00:00Z = 1768521600 seconds since Unix epoch
    let base: i64 = 1768521600;
    let ts = base + offset_hours * 3600;
    let secs = ts;
    let mins = secs / 60;
    let hours_total = mins / 60;
    let days_total = hours_total / 24;
    let s = (secs % 60) as u32;
    let m = (mins % 60) as u32;
    let h = (hours_total % 24) as u32;
    let (year, month, day) = days_to_ymd(days_total);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, h, m, s
    )
}

/// Generate the full synthetic instincts corpus (~200 memories, ~300 edges)
/// across five categories:
///
/// 1. Infrastructure state transitions (10 sets x 4 memories = 40)
/// 2. Architecture decision records (10 x 2 = 20)
/// 3. Reference / discovery notes (20)
/// 4. Task completion records (20)
/// 5. Correction pairs -- wrong belief + correction (20 x 2 = 40)
///
/// All memory IDs are negative. Edges use string edge types matching
/// `EdgeType::from_str_loose`.
pub fn generate_instincts() -> InstinctsCorpus {
    let mut memories: Vec<SyntheticMemory> = Vec::with_capacity(200);
    let mut edges: Vec<SyntheticEdge> = Vec::with_capacity(300);
    let mut next_id: i64 = -1;

    // -- Category 1: Infrastructure state transitions (10 sets x 4 memories = 40) --
    let infra_sets: &[(&str, &str, &str, &str, i64)] = &[
        (
            "nginx v1.18",
            "nginx v1.24",
            "web-proxy",
            "HTTP reverse proxy",
            0,
        ),
        (
            "PostgreSQL 13",
            "PostgreSQL 16",
            "db-primary",
            "primary database",
            48,
        ),
        ("Redis 6.2", "Redis 7.2", "cache-layer", "session cache", 96),
        (
            "Node.js 18",
            "Node.js 22",
            "api-server",
            "REST API runtime",
            144,
        ),
        (
            "Docker 20.10",
            "Podman 4.9",
            "container-runtime",
            "container orchestration",
            192,
        ),
        (
            "Python 3.10",
            "Python 3.12",
            "worker-service",
            "background task runner",
            240,
        ),
        (
            "Elasticsearch 7",
            "OpenSearch 2.11",
            "search-cluster",
            "full-text search index",
            288,
        ),
        (
            "RabbitMQ 3.10",
            "RabbitMQ 3.12",
            "message-broker",
            "async job queue",
            336,
        ),
        (
            "Traefik 2.x",
            "Traefik 3.x",
            "ingress-controller",
            "TLS termination and routing",
            384,
        ),
        (
            "Grafana 9",
            "Grafana 11",
            "monitoring-ui",
            "metrics dashboard",
            432,
        ),
    ];

    for (old_ver, new_ver, service, desc, base_hours) in infra_sets {
        let id1 = next_id;
        next_id -= 1;
        let c1 = format!(
            "{} is running {} on {} - {}. Deployed 2026-01-15. Status: stable.",
            service, old_ver, service, desc
        );
        memories.push(SyntheticMemory {
            id: id1,
            content: c1.clone(),
            category: "state".to_string(),
            importance: 5,
            created_at: format_date(*base_hours),
            embedding: make_embedding(&c1),
        });

        let id2 = next_id;
        next_id -= 1;
        let c2 = format!(
            "Decision to migrate {} from {} to {} on {}. Reason: upstream EOL and security patches. Scheduled downtime: 30 minutes.",
            service, old_ver, new_ver, service
        );
        memories.push(SyntheticMemory {
            id: id2,
            content: c2.clone(),
            category: "decision".to_string(),
            importance: 7,
            created_at: format_date(base_hours + 12),
            embedding: make_embedding(&c2),
        });

        let id3 = next_id;
        next_id -= 1;
        let c3 = format!(
            "{} successfully migrated to {} on {}. Migration completed 2026-01. All health checks passing. Previous version {} decommissioned.",
            service, new_ver, service, old_ver
        );
        memories.push(SyntheticMemory {
            id: id3,
            content: c3.clone(),
            category: "task".to_string(),
            importance: 8,
            created_at: format_date(base_hours + 24),
            embedding: make_embedding(&c3),
        });

        let id4 = next_id;
        next_id -= 1;
        let c4 = format!(
            "{} is NOW running {} on {} - {}. Upgraded from {}. Status: stable, verified.",
            service, new_ver, service, desc, old_ver
        );
        memories.push(SyntheticMemory {
            id: id4,
            content: c4.clone(),
            category: "state".to_string(),
            importance: 9,
            created_at: format_date(base_hours + 25),
            embedding: make_embedding(&c4),
        });

        edges.push(SyntheticEdge {
            source_id: id1,
            target_id: id2,
            weight: 0.8,
            edge_type: "temporal".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id2,
            target_id: id3,
            weight: 0.8,
            edge_type: "temporal".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id3,
            target_id: id4,
            weight: 0.8,
            edge_type: "temporal".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id1,
            target_id: id4,
            weight: 0.7,
            edge_type: "contradiction".to_string(),
        });
    }

    // -- Category 2: Architecture decision records (10 x 2 = 20) --
    let decisions: &[(&str, &str, &str, &str, i64)] = &[
        ("database", "Use PostgreSQL over MySQL for the primary store",
         "ACID compliance, better JSON support, and superior indexing. MySQL replication lag unacceptable for consistency requirements.",
         "PostgreSQL selected. MySQL evaluated and rejected.", 500),
        ("caching", "Use Redis over Memcached for session caching",
         "Redis supports persistence, pub/sub, and sorted sets needed for leaderboards. Memcached is volatile-only.",
         "Redis deployed for sessions and pub/sub.", 520),
        ("container", "Switch from Docker to Podman for production workloads",
         "Rootless Podman reduces attack surface. Docker daemon single point of failure eliminated. OCI compatible.",
         "Podman adopted. Docker daemon removed from production nodes.", 540),
        ("monitoring", "Adopt Prometheus and Grafana over Datadog",
         "Cost: Datadog $3k/month vs self-hosted $50/month infra cost. Prometheus retention and alerting fully customizable.",
         "Prometheus stack deployed. Datadog subscription cancelled.", 560),
        ("proxy", "Use Traefik over HAProxy for ingress",
         "Traefik integrates with Docker service discovery. HAProxy requires manual config for each backend.",
         "Traefik deployed as ingress. HAProxy configs archived.", 580),
        ("logging", "Use Loki over Elasticsearch for log aggregation",
         "Loki label-based indexing 10x cheaper at scale. Elasticsearch full-text not needed for structured logs.",
         "Loki deployed. Elasticsearch retained only for search features.", 600),
        ("queue", "Use RabbitMQ over Kafka for job processing",
         "Kafka overhead unjustified for sub-10k msg/sec. RabbitMQ simpler ops and sufficient throughput.",
         "RabbitMQ in production. Kafka evaluated for future data pipeline.", 620),
        ("cdn", "Self-host Nginx for static assets over CloudFront",
         "Data transfer costs $800/month on CloudFront. Nginx on dedicated node costs $40/month at current traffic.",
         "Nginx static asset server deployed. CloudFront distribution disabled.", 640),
        ("auth", "Implement JWT with refresh tokens over session cookies",
         "Stateless JWT enables horizontal scaling without session store. Refresh token rotation provides security equivalent.",
         "JWT auth implemented. Session store removed from architecture.", 660),
        ("backup", "Use restic over duplicati for backup strategy",
         "restic deduplication more efficient. CLI-first design fits automation. duplicati GUI dependency removed from headless servers.",
         "restic deployed on all nodes. Automated daily snapshots verified.", 680),
    ];

    for (domain, title, rationale, outcome, base_hours) in decisions {
        let id1 = next_id;
        next_id -= 1;
        let c1 = format!(
            "Architecture decision [{}]: {}. Rationale: {}",
            domain, title, rationale
        );
        memories.push(SyntheticMemory {
            id: id1,
            content: c1.clone(),
            category: "decision".to_string(),
            importance: 8,
            created_at: format_date(*base_hours),
            embedding: make_embedding(&c1),
        });

        let id2 = next_id;
        next_id -= 1;
        let c2 = format!(
            "Decision outcome [{}]: {} Implemented and verified in production.",
            domain, outcome
        );
        memories.push(SyntheticMemory {
            id: id2,
            content: c2.clone(),
            category: "task".to_string(),
            importance: 7,
            created_at: format_date(base_hours + 24),
            embedding: make_embedding(&c2),
        });

        edges.push(SyntheticEdge {
            source_id: id1,
            target_id: id2,
            weight: 0.75,
            edge_type: "association".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id2,
            target_id: id1,
            weight: 0.75,
            edge_type: "association".to_string(),
        });
    }

    // -- Category 3: Reference / discovery notes (20) --
    let references: &[(&str, &str, i64, i32)] = &[
        ("server-specs", "app-server-1: 8 vCPU, 32GB RAM, 500GB NVMe SSD, Ubuntu 22.04. Role: application services backend.", 700, 8),
        ("server-specs", "edge-server-1: 4 vCPU, 16GB RAM, 200GB SSD, Rocky Linux 9. Role: CDN edge node and static assets.", 702, 7),
        ("server-specs", "dev-workstation: Xeon W-2125 4.0GHz 8-core, 30GB RAM, 2TB HDD. Role: primary development and build machine.", 704, 9),
        ("endpoint", "Engram memory API: POST /store to persist memories, POST /search to query, GET /recall for recent. Auth via Bearer token.", 710, 8),
        ("endpoint", "Eidolon brain API: JSON over stdio. Commands: init, query, absorb, decay_tick, dream_cycle, get_stats, shutdown.", 712, 9),
        ("filepath", "Brain database location: /brain.db - SQLite, contains memories, edges, pca_state tables.", 714, 8),
        ("filepath", "Instincts binary: /instincts.bin - gzip-compressed JSON corpus, applied on first init when brain.db is empty.", 716, 7),
        ("filepath", "Eidolon Rust source: src/ directory of eidolon-lib crate - substrate.rs, graph.rs, dreaming.rs, instincts.rs, main.rs.", 718, 6),
        ("credential", "SSH key for all servers: operator-configured SSH key. Custom ports may apply for specific servers.", 720, 9),
        ("network", "VPN mesh subnet: configured in operator's mesh network. All nodes reachable by mesh IP. Use internal IPs for inter-service traffic.", 722, 8),
        ("config", "Nginx config directory: /etc/nginx/sites-enabled/. Reload: nginx -t && systemctl reload nginx. Never restart without testing.", 730, 7),
        ("config", "PostgreSQL data directory: /var/lib/postgresql/16/main/. Config: /etc/postgresql/16/main/postgresql.conf.", 732, 7),
        ("config", "Redis config: /etc/redis/redis.conf. Bind 127.0.0.1 only. requirepass enabled. maxmemory-policy allkeys-lru.", 734, 6),
        ("pattern", "Service restart pattern: check state -> back up config -> stop service -> apply change -> start service -> verify health -> monitor logs.", 740, 9),
        ("pattern", "File deployment pattern: write locally -> SCP to /tmp/ -> SSH mv to destination -> set permissions -> verify.", 742, 8),
        ("pattern", "Never use heredoc over SSH for file content - truncates to 0 bytes. Always use SCP for file transfers to remote hosts.", 744, 9),
        ("pattern", "CrowdSec is the intrusion detection system on all nodes. Never install fail2ban. CrowdSec bouncer handles blocking.", 746, 8),
        ("error", "podman cp truncates heredoc content - root cause: shell expansion in subprocess. Fix: scp local file then podman cp from host.", 750, 8),
        ("error", "Unix socket stale fd: when upstream restarts, downstream holds old fd. Both must restart in order: upstream first, then downstream.", 752, 7),
        ("error", "SELinux blocks unexpected service access - check ausearch -m avc -ts recent. Fix: restorecon -Rv /path or semanage.", 754, 6),
    ];

    for (category, content, base_hours, importance) in references {
        let id = next_id;
        next_id -= 1;
        memories.push(SyntheticMemory {
            id,
            content: content.to_string(),
            category: category.to_string(),
            importance: *importance,
            created_at: format_date(*base_hours),
            embedding: make_embedding(content),
        });
    }

    // -- Category 4: Task completion records (20) --
    let tasks: &[(&str, i64, i32)] = &[
        ("Deploy Eidolon brain substrate Phase 1 to the development server. Result: binary at /target/release/eidolon. brain.db initialized with 847 memories from Engram export.", 800, 9),
        ("Migrate Engram database from SQLite to PostgreSQL. Result: 12,847 memories migrated, 0 data loss, query time improved from 45ms to 8ms at p99.", 810, 8),
        ("Fix memory leak in Eidolon decay module. Root cause: Vec not cleared after prune. Fix: add memory.retain() after dead_set removal. Leak eliminated.", 820, 9),
        ("Set up Traefik TLS termination for all subdomains. Let Encrypt wildcard cert via DNS challenge. All subdomains now HTTPS.", 830, 8),
        ("Configure Prometheus scrape targets for all nodes. Added: node_exporter, postgres_exporter, redis_exporter, nginx_exporter.", 840, 7),
        ("Deploy CrowdSec on application and edge servers. Installed bouncer for nginx. Community blocklist active. First 24h: 1,247 IPs blocked.", 850, 8),
        ("Upgrade PostgreSQL 13 to 16 on db-primary. pg_upgrade used for in-place upgrade. Backup taken before: pg_dump 18GB. Zero data loss.", 860, 9),
        ("Implement JWT refresh token rotation in API server. Old refresh tokens invalidated on use. 7-day token expiry. Redis TTL set.", 870, 7),
        ("Set up automated restic backups on all nodes. Daily snapshots at 02:00 UTC. Retention: 7 daily, 4 weekly, 12 monthly.", 880, 8),
        ("Debug and fix Nginx upstream 502 errors. Root cause: backend pool exhausted due to connection leak in Python worker.", 890, 9),
        ("Enable TCP BBR on all Linux servers via sysctl. net.ipv4.tcp_congestion_control=bbr. p99 latency -23%, throughput +18%.", 900, 7),
        ("Consolidate application and edge server nginx configs into shared template. Reduced config duplication from 4 files to 1 template.", 910, 6),
        ("Add memory decay monitoring to Engram dashboard. Alert when avg_decay_factor < 0.3. Grafana panel showing health_distribution over time.", 920, 7),
        ("Write Eidolon dreaming module. Implements: replay_recent, merge_redundant, prune_dead, discover_connections, resolve_lingering.", 930, 9),
        ("Write Eidolon instincts module. Generates 200 synthetic ghost patterns across 5 categories. Ghost replacement on cosine_sim > 0.85.", 940, 9),
        ("Implement Hopfield substrate in Rust with ndarray. PCA projection 1024->512 dims. Retrieval via softmax activation.", 950, 9),
        ("Add graph spread to Eidolon query pipeline. BFS from Hopfield seeds, 3 hops, decay 0.5/hop. Contradiction resolution.", 960, 8),
        ("Deploy Eidolon C++ backend as alternative to Rust. Same JSON protocol. Eigen3 for linear algebra.", 970, 7),
        ("Fix race condition in dream cycle pruning. Root cause: iterating memories while removing. Fix: collect dead_ids first.", 980, 8),
        ("Optimize PCA projection in Eidolon. Moved from per-query projection to cached patterns. Query time improved from 8ms to 2ms p99.", 990, 8),
    ];

    for (content, base_hours, importance) in tasks {
        let id = next_id;
        next_id -= 1;
        memories.push(SyntheticMemory {
            id,
            content: content.to_string(),
            category: "task".to_string(),
            importance: *importance,
            created_at: format_date(*base_hours),
            embedding: make_embedding(content),
        });
    }

    // -- Category 5: Correction pairs (20 x 2 = 40) --
    let corrections: &[(&str, &str, i64, i64, i32, i32)] = &[
        (
            "app-server-1 is running Ubuntu 20.04 LTS.",
            "CORRECTION: app-server-1 is running Ubuntu 22.04 LTS, not 20.04. Upgraded 2025-11. Verify with: lsb_release -a.",
            1100, 1112, 6, 8
        ),
        (
            "Redis is configured to bind to 0.0.0.0 for inter-service access.",
            "CORRECTION: Redis binds to 127.0.0.1 ONLY. Inter-service access via Unix socket or SSH tunnel. Binding 0.0.0.0 was a security incident.",
            1120, 1132, 5, 9
        ),
        (
            "PostgreSQL replication lag is acceptable at 2-3 seconds for read replicas.",
            "CORRECTION: PostgreSQL replication lag target is under 500ms, not 2-3 seconds. Alert threshold: 1000ms.",
            1140, 1152, 5, 8
        ),
        (
            "fail2ban is installed on all nodes for SSH protection.",
            "CORRECTION: CrowdSec is used, NOT fail2ban. fail2ban was removed in 2025-10. CrowdSec bouncer handles all blocking.",
            1160, 1172, 4, 9
        ),
        (
            "The SSH private key location is ~/.ssh/id_ed25519 on all machines.",
            "CORRECTION: SSH key is the operator-configured SSH key, NOT ~/.ssh/id_ed25519. All server logins use the operator-configured SSH key. Custom ports may apply for specific servers.",
            1180, 1192, 5, 9
        ),
        (
            "Eidolon brain.db is stored at /var/lib/eidolon/brain.db.",
            "CORRECTION: brain.db is at /brain.db, not /var/lib/eidolon/. The data_dir is /.",
            1200, 1212, 5, 8
        ),
        (
            "Memory patterns are stored in full 1024-dimensional space in the Hopfield substrate.",
            "CORRECTION: Patterns are PCA-projected to 512 dimensions before Hopfield storage. Raw 1024-dim embeddings stored in brain.db.",
            1220, 1232, 6, 9
        ),
        (
            "Dream cycles run during active query processing to consolidate recent memories.",
            "CORRECTION: Dream cycles run ONLY during idle periods. TypeScript coordinator pauses dreaming when query activity detected.",
            1240, 1252, 5, 8
        ),
        (
            "Ghost patterns from instincts have the same decay rate as real memories.",
            "CORRECTION: Ghost patterns decay at 2x the rate of real memories. Ghost strength starts at 0.3 vs 0.5 for real.",
            1260, 1272, 6, 9
        ),
        (
            "Association edges are created between all memory pairs with cosine similarity above 0.3.",
            "CORRECTION: Association threshold is 0.4, not 0.3. Contradiction threshold is 0.75. Max 15 edges per memory.",
            1280, 1292, 5, 7
        ),
        (
            "The Eidolon binary accepts HTTP REST API requests on port 7433.",
            "CORRECTION: Eidolon binary uses JSON over stdio, NOT HTTP. TypeScript manager wraps the binary in a subprocess.",
            1300, 1312, 5, 9
        ),
        (
            "Engram stores memories in a custom binary format for performance.",
            "CORRECTION: Engram stores memories in SQLite or PostgreSQL. The instincts.bin file is separate and not the main Engram storage.",
            1320, 1332, 5, 7
        ),
        (
            "The PCA transform is recomputed from scratch on every Eidolon startup.",
            "CORRECTION: PCA state is saved to brain.db after first fit and loaded on subsequent startups.",
            1340, 1352, 6, 8
        ),
        (
            "Edge weights in the ConnectionGraph start at 1.0 for all new edges.",
            "CORRECTION: Association edges start at cosine similarity value (0.4-1.0). Temporal edges start at max(cosine_sim, 0.1).",
            1360, 1372, 5, 7
        ),
        (
            "The memory importance field controls retrieval priority directly.",
            "CORRECTION: Importance affects decay rate and tie-breaking. Retrieval priority is determined by activation score.",
            1380, 1392, 6, 8
        ),
        (
            "Heredoc over SSH is a reliable way to write files on remote servers.",
            "CORRECTION: Heredoc over SSH truncates files to 0 bytes in practice. Always use SCP. This is a documented gotcha in AGENTS.md.",
            1400, 1412, 4, 9
        ),
        (
            "Rootless Podman containers access host files using normal Unix permissions.",
            "CORRECTION: Rootless Podman uses user namespace mapping. Files may be owned by UID 100000+. Must chown to mapped UID.",
            1420, 1432, 5, 8
        ),
        (
            "The Hopfield substrate retrieves exact matches for query embeddings.",
            "CORRECTION: Hopfield retrieval is approximate - it finds nearest attractors via energy minimization. Not exact lookup.",
            1440, 1452, 6, 8
        ),
        (
            "All graph edges are bidirectional by default when created.",
            "CORRECTION: Edges are unidirectional in ConnectionGraph.add_edge(). absorb_memory creates bidirectional pairs explicitly.",
            1460, 1472, 5, 7
        ),
        (
            "Memory decay happens automatically in real-time as time passes.",
            "CORRECTION: Decay is applied only on explicit decay_tick commands. The TypeScript manager sends decay_tick periodically.",
            1480, 1492, 5, 8
        ),
    ];

    for (wrong, correction, h1, h2, imp1, imp2) in corrections {
        let id1 = next_id;
        next_id -= 1;
        let id2 = next_id;
        next_id -= 1;

        memories.push(SyntheticMemory {
            id: id1,
            content: wrong.to_string(),
            category: "state".to_string(),
            importance: *imp1,
            created_at: format_date(*h1),
            embedding: make_embedding(wrong),
        });

        memories.push(SyntheticMemory {
            id: id2,
            content: correction.to_string(),
            category: "correction".to_string(),
            importance: *imp2,
            created_at: format_date(*h2),
            embedding: make_embedding(correction),
        });

        edges.push(SyntheticEdge {
            source_id: id1,
            target_id: id2,
            weight: 0.8,
            edge_type: "contradiction".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id2,
            target_id: id1,
            weight: 0.8,
            edge_type: "contradiction".to_string(),
        });
        edges.push(SyntheticEdge {
            source_id: id1,
            target_id: id2,
            weight: 0.6,
            edge_type: "temporal".to_string(),
        });
    }

    InstinctsCorpus {
        version: 2,
        generated_at: "2026-01-01T00:00:00Z".to_string(),
        memories,
        edges,
    }
}

// ---- Binary serialization ----

/// Serialize an `InstinctsCorpus` to the binary .bin format.
///
/// Format: 4-byte magic "INST" + 4-byte u32 version LE +
///         4-byte u32 compressed-length LE + gzip-compressed JSON body.
pub fn save_instincts(corpus: &InstinctsCorpus, path: &Path) -> Result<()> {
    let json_bytes = serde_json::to_vec(corpus)
        .map_err(|e| EngError::Internal(format!("instincts: serialize failed: {}", e)))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&json_bytes)
        .map_err(|e| EngError::Internal(format!("instincts: compress failed: {}", e)))?;
    let compressed = encoder
        .finish()
        .map_err(|e| EngError::Internal(format!("instincts: compress finish failed: {}", e)))?;

    let compressed_len = compressed.len() as u32;

    let mut out = Vec::with_capacity(12 + compressed.len());
    out.extend_from_slice(INST_MAGIC);
    // Version 2 matches INST_VERSION_MAX
    out.extend_from_slice(&INST_VERSION_MAX.to_le_bytes());
    out.extend_from_slice(&compressed_len.to_le_bytes());
    out.extend_from_slice(&compressed);

    std::fs::write(path, &out)
        .map_err(|e| EngError::Internal(format!("instincts: write failed: {}", e)))?;

    Ok(())
}

// ---- Ghost replacement ----

/// Check whether any ghost patterns (negative IDs) in the Hopfield network
/// are sufficiently similar to a newly absorbed real embedding. If so,
/// remove the ghost from both the in-memory network and the database.
///
/// A ghost is replaced when `cosine_sim(new_embedding, ghost_pattern) > GHOST_REPLACE_SIM`.
/// This prevents the corpus from double-representing knowledge that the brain
/// has now learned from real experience.
///
/// Returns the number of ghosts removed.
#[tracing::instrument(skip(db, network, new_embedding), fields(embedding_len = new_embedding.len(), user_id))]
pub async fn check_ghost_replacement(
    db: &Database,
    network: &mut HopfieldNetwork,
    new_embedding: &[f32],
    user_id: i64,
) -> Result<usize> {
    use crate::brain::hopfield::pattern;

    // Only consider negative IDs (ghost patterns) that are in the live network.
    let ghost_ids: Vec<i64> = network
        .pattern_ids()
        .iter()
        .copied()
        .filter(|&id| id < 0)
        .collect();

    if ghost_ids.is_empty() {
        return Ok(0);
    }

    // Load ghost patterns from DB to get their embedding vectors.
    let all_patterns = pattern::list_patterns(db, user_id).await?;
    let ghost_patterns: Vec<_> = all_patterns
        .iter()
        .filter(|p| ghost_ids.contains(&p.id))
        .collect();

    let normalized_new = network::l2_normalize(new_embedding);

    let mut to_remove: Vec<i64> = Vec::new();
    for ghost in ghost_patterns {
        let normalized_ghost = network::l2_normalize(&ghost.pattern);
        let sim = network::cosine_similarity(&normalized_new, &normalized_ghost);
        if sim > GHOST_REPLACE_SIM {
            to_remove.push(ghost.id);
        }
    }

    let count = to_remove.len();
    for id in to_remove {
        network.remove(id);
        if let Err(e) = pattern::delete_pattern(db, id, user_id).await {
            tracing::warn!(pattern_id = id, error = %e, "delete_pattern (ghost replacement) failed");
        }
    }

    Ok(count)
}

// ---- Tests ----

#[cfg(test)]
mod tests;
