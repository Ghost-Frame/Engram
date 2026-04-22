use clap::{Parser, Subcommand};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "engram-cli")]
#[command(about = "Engram memory system CLI", long_about = None)]
struct Cli {
    /// Server URL
    #[arg(long, default_value = "http://127.0.0.1:4200", env = "ENGRAM_URL")]
    server: String,

    /// Credd daemon URL
    #[arg(long, default_value = "http://127.0.0.1:4400", env = "CREDD_URL")]
    credd_url: String,

    /// API key
    #[arg(long)]
    key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Store a new memory
    Store {
        /// Memory content
        content: String,
        /// Category (task, discovery, decision, state, issue, general, reference)
        #[arg(short, long, default_value = "general")]
        category: String,
        /// Importance score 0-10 (integer)
        #[arg(short, long)]
        importance: Option<u8>,
        /// Comma-separated tags
        #[arg(short, long)]
        tags: Option<String>,
        /// Source identifier
        #[arg(short, long)]
        source: Option<String>,
    },
    /// Search memories
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Get context for a query (richer output than search)
    Context {
        /// Query
        query: String,
        /// Maximum memories to return
        #[arg(short, long, default_value = "5")]
        limit: usize,
    },
    /// Recall a specific memory by ID
    Recall {
        /// Memory ID
        id: String,
    },
    /// Evaluate content through the guard system
    Guard {
        /// Content to evaluate
        content: String,
    },
    /// List memories
    List {
        /// Maximum results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Offset
        #[arg(short, long, default_value = "0")]
        offset: usize,
    },
    /// Delete a memory
    Delete {
        /// Memory ID
        id: String,
    },
    /// Bootstrap the database schema
    Bootstrap {
        /// Database path
        #[arg(short, long, default_value = "kleos.db")]
        db: String,
    },
    /// Ingest text or a file into the memory pipeline
    Ingest {
        /// Inline text to ingest (use --file for paths)
        #[arg(short, long, conflicts_with = "file")]
        text: Option<String>,
        /// Read content from a file (any supported format: .md, .txt, .html, .csv, .jsonl, .pdf, .docx, .zip)
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        /// Ingestion mode: raw | extract
        #[arg(short, long, default_value = "raw")]
        mode: String,
        /// Source label recorded on each memory
        #[arg(short, long)]
        source: Option<String>,
        /// Category to assign
        #[arg(short, long, default_value = "general")]
        category: String,
    },
    /// Check server health
    Health,
    /// Durable job queue inspection and control
    #[command(subcommand)]
    Jobs(JobsCommands),
    /// Skill management
    #[command(subcommand)]
    Skill(SkillCommands),
    /// Credential management (talks to credd)
    #[command(subcommand)]
    Cred(CredCommands),
}

#[derive(Subcommand)]
enum JobsCommands {
    /// Show queue stats (pending / running / completed / failed counts)
    Stats,
    /// List jobs filtered by status
    List {
        /// Status filter: pending | running | failed
        #[arg(short, long, default_value = "pending")]
        status: String,
        /// Maximum rows to return
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Row offset
        #[arg(short, long, default_value = "0")]
        offset: usize,
    },
    /// Retry a failed job by id, or retry every failed job when --all is given
    Retry {
        /// Job id to retry (omit together with --all to retry every failed job)
        id: Option<i64>,
        /// Retry every failed job
        #[arg(long)]
        all: bool,
    },
    /// Delete failed jobs older than N days
    Purge {
        /// Only purge failed jobs older than this many days
        #[arg(long, default_value = "7")]
        older_than_days: i64,
    },
    /// Delete completed jobs older than N days
    Cleanup {
        /// Only remove completed jobs older than this many days
        #[arg(long, default_value = "1")]
        older_than_days: i64,
    },
}

#[derive(Subcommand)]
enum CredCommands {
    /// Get a secret value
    Get {
        /// Category (service namespace)
        category: String,
        /// Secret name
        name: String,
        /// Output raw value only (no JSON)
        #[arg(long)]
        raw: bool,
    },
    /// Set a secret
    Set {
        /// Category (service namespace)
        category: String,
        /// Secret name
        name: String,
        /// Secret type (api_key, login, oauth_app, ssh_key, note, environment)
        #[arg(short = 't', long, default_value = "api_key")]
        secret_type: String,
        /// Secret value (prompted if not provided)
        #[arg(short, long)]
        value: Option<String>,
        /// For login: username
        #[arg(long)]
        username: Option<String>,
        /// For login/oauth: URL
        #[arg(long)]
        url: Option<String>,
    },
    /// List secrets
    List {
        /// Filter by category
        #[arg(short, long)]
        category: Option<String>,
    },
    /// Delete a secret
    Delete {
        /// Category
        category: String,
        /// Secret name
        name: String,
    },
    /// Create an agent key
    AgentCreate {
        /// Agent name
        name: String,
        /// Allowed categories (comma-separated, empty = all)
        #[arg(short, long)]
        categories: Option<String>,
        /// Allow raw access tier
        #[arg(long)]
        allow_raw: bool,
    },
    /// List agent keys
    AgentList,
    /// Revoke an agent key
    AgentRevoke {
        /// Agent name to revoke
        name: String,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Search skills by query
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// List skills
    List {
        /// Maximum results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Offset
        #[arg(short, long, default_value = "0")]
        offset: usize,
        /// Filter by agent
        #[arg(short, long)]
        agent: Option<String>,
    },
    /// Get a skill by ID
    Get {
        /// Skill ID
        id: i64,
    },
    /// Record an execution result for a skill
    Execute {
        /// Skill ID
        id: i64,
        /// Whether the execution succeeded
        #[arg(short, long)]
        success: bool,
        /// Duration in milliseconds
        #[arg(short, long)]
        duration_ms: Option<i64>,
        /// Error type (if failed)
        #[arg(long)]
        error_type: Option<String>,
        /// Error message (if failed)
        #[arg(long)]
        error_message: Option<String>,
    },
    /// Capture a new skill from a description
    Capture {
        /// Description of the skill to capture
        description: String,
        /// Agent name
        #[arg(short, long, default_value = "claude-code")]
        agent: String,
    },
    /// Fix / refine an existing skill
    Fix {
        /// Skill ID
        id: i64,
        /// Direction for the fix
        #[arg(short, long)]
        direction: Option<String>,
        /// Agent name
        #[arg(short, long, default_value = "claude-code")]
        agent: String,
    },
    /// Derive a new skill from parent skills
    Derive {
        /// Parent skill IDs (at least one required)
        #[arg(required = true)]
        parent_ids: Vec<i64>,
        /// Direction for derivation
        #[arg(short, long)]
        direction: String,
        /// Agent name
        #[arg(short, long, default_value = "claude-code")]
        agent: String,
    },
    /// Show skill dashboard stats
    Stats,
    /// Show lineage for a skill
    Lineage {
        /// Skill ID
        id: i64,
    },
    /// Show recent skill evolution
    Evolve {
        /// Hours to look back
        #[arg(short = 'H', long, default_value = "24")]
        hours: u64,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
}

struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl Client {
    fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp).await
    }

    async fn delete(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.delete(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp).await
    }

    async fn handle_response(&self, resp: reqwest::Response) -> Result<Value, String> {
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            Ok(body)
        } else {
            let msg = body
                .get("error")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            Err(format!("HTTP {}: {}", status, msg))
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn value_as_string(value: Option<&Value>) -> Option<String> {
    value.and_then(|v| {
        v.as_str()
            .map(ToOwned::to_owned)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })
}

#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard = kleos_lib::observability::init_tracing("engram-cli", "warn");

    let cli = Cli::parse();
    let api_key = cli
        .key
        .clone()
        .or_else(|| std::env::var("ENGRAM_API_KEY").ok())
        .or_else(|| std::env::var("ENGRAM_KEY").ok());
    let client = Client::new(cli.server.clone(), api_key.clone());

    match &cli.command {
        Commands::Store {
            content,
            category,
            importance,
            tags,
            source,
        } => {
            let tags_list: Vec<String> = tags
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();

            let mut body = json!({
                "content": content,
                "category": category,
            });

            if let Some(imp) = importance {
                body["importance"] = json!(imp);
            }
            if !tags_list.is_empty() {
                body["tags"] = json!(tags_list);
            }
            if let Some(src) = source {
                body["source"] = json!(src);
            }

            match client.post("/store", body).await {
                Ok(v) => {
                    if let Some(existing_id) = value_as_string(v.get("existing_id")) {
                        println!("Duplicate of #{}", existing_id);
                    } else if let Some(id) = value_as_string(v.get("id")) {
                        println!("Stored memory #{}", id);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        Commands::Search { query, limit } => {
            let body = json!({ "query": query, "limit": limit });
            match client.post("/search", body).await {
                Ok(v) => {
                    let results = v.as_array().cloned().unwrap_or_else(|| {
                        v.get("results")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if results.is_empty() {
                        println!("No results.");
                    }
                    for item in &results {
                        let id = value_as_string(item.get("id")).unwrap_or_else(|| "?".to_string());
                        let score = item
                            .get("score")
                            .and_then(|x| x.as_f64())
                            .map(|s| format!("{:.3}", s))
                            .unwrap_or_else(|| "?".to_string());
                        let content = item.get("content").and_then(|x| x.as_str()).unwrap_or("");
                        println!("#{} [{}] {}", id, score, truncate(content, 100));
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        Commands::Context { query, limit } => {
            let body = json!({ "query": query, "context": query, "limit": limit });
            match client.post("/recall", body).await {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        Commands::Recall { id } => match client.get(&format!("/memory/{}", id)).await {
            Ok(v) => {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            Err(e) => eprintln!("Error: {}", e),
        },

        Commands::Guard { content: _ } => {
            println!("guard not implemented");
        }

        Commands::List { limit, offset } => {
            match client
                .get(&format!("/list?limit={}&offset={}", limit, offset))
                .await
            {
                Ok(v) => {
                    let items = v.as_array().cloned().unwrap_or_else(|| {
                        v.get("results")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if items.is_empty() {
                        println!("No memories.");
                    }
                    for item in &items {
                        let id = value_as_string(item.get("id")).unwrap_or_else(|| "?".to_string());
                        let category = item.get("category").and_then(|x| x.as_str()).unwrap_or("?");
                        let content = item.get("content").and_then(|x| x.as_str()).unwrap_or("");
                        println!("#{} [{}] {}", id, category, truncate(content, 100));
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        Commands::Delete { id } => match client.delete(&format!("/memory/{}", id)).await {
            Ok(_) => println!("Deleted memory #{}", id),
            Err(e) => eprintln!("Error: {}", e),
        },

        Commands::Bootstrap { db: _ } => match client.post("/bootstrap", json!({})).await {
            Ok(v) => {
                if let Some(key) =
                    value_as_string(v.get("api_key")).or_else(|| value_as_string(v.get("key")))
                {
                    println!("{}", key);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        },

        Commands::Ingest {
            text,
            file,
            mode,
            source,
            category,
        } => {
            handle_ingest(&client, text, file, mode, source, category).await;
        }

        Commands::Health => match client.get("/health").await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => eprintln!("Error: {}", e),
        },

        Commands::Jobs(jobs_cmd) => {
            handle_jobs_command(&client, jobs_cmd).await;
        }

        Commands::Skill(skill_cmd) => {
            handle_skill_command(&client, skill_cmd).await;
        }

        Commands::Cred(cred_cmd) => {
            let cred_client = Client::new(cli.credd_url.clone(), api_key.clone());
            handle_cred_command(&cred_client, cred_cmd).await;
        }
    }
}

async fn handle_ingest(
    client: &Client,
    text: &Option<String>,
    file: &Option<std::path::PathBuf>,
    mode: &str,
    source: &Option<String>,
    category: &str,
) {
    // Prefer --file when given; fall back to --text; error otherwise.
    let (raw_bytes, is_binary, source_label) = match (file, text) {
        (Some(path), _) => match std::fs::read(path) {
            Ok(bytes) => {
                // We treat UTF-8 decodable input as text ingest and hand off
                // binary content to the chunked upload flow.
                let looks_text = std::str::from_utf8(&bytes).is_ok();
                let label = source
                    .clone()
                    .or_else(|| {
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "file".to_string());
                (bytes, !looks_text, label)
            }
            Err(e) => {
                eprintln!("Error reading {}: {}", path.display(), e);
                return;
            }
        },
        (None, Some(t)) => (
            t.as_bytes().to_vec(),
            false,
            source.clone().unwrap_or_else(|| "cli".to_string()),
        ),
        (None, None) => {
            eprintln!("Error: supply --text or --file");
            return;
        }
    };

    if !is_binary {
        // Hot path: POST /ingest with the decoded string body.
        let text_body = String::from_utf8(raw_bytes).expect("utf8 verified above");
        let body = json!({
            "text": text_body,
            "mode": mode,
            "source": source_label,
            "category": category,
        });
        match client.post("/ingest", body).await {
            Ok(v) => {
                if let Some(id) = value_as_string(v.get("job_id")) {
                    let memories = v
                        .get("ingested")
                        .and_then(|v| v.as_i64())
                        .or_else(|| v.get("ingested_memories").and_then(|v| v.as_i64()))
                        .unwrap_or(0);
                    let chunks = v
                        .get("chunks_processed")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    println!("Ingested: job {id} -- {memories} memories, {chunks} chunks");
                } else {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        }
        return;
    }

    // Binary path: chunked upload flow. Split into 1MiB chunks, POST init →
    // chunk* → complete so PDF/DOCX/ZIP payloads reach ingest_binary().
    const CHUNK_SIZE: usize = 1 << 20;
    let filename = file
        .as_ref()
        .and_then(|p| p.file_name().and_then(|n| n.to_str().map(|s| s.to_string())));
    let content_type = filename
        .as_deref()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, ext)| match ext.to_ascii_lowercase().as_str() {
            "pdf" => "application/pdf",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "zip" => "application/zip",
            _ => "application/octet-stream",
        });

    let total_chunks = raw_bytes.len().div_ceil(CHUNK_SIZE);
    let mut init_body = json!({
        "total_chunks": total_chunks as i64,
        "source": source_label,
    });
    if let Some(name) = &filename {
        init_body["filename"] = json!(name);
    }
    if let Some(ct) = content_type {
        init_body["content_type"] = json!(ct);
    }

    let init_resp = match client.post("/ingest/upload/init", init_body).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error initiating upload: {}", e);
            return;
        }
    };
    let upload_id = match init_resp.get("upload_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            eprintln!("Upload init returned no upload_id: {init_resp}");
            return;
        }
    };

    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut full_hasher = Sha256::new();
    for (idx, chunk) in raw_bytes.chunks(CHUNK_SIZE).enumerate() {
        full_hasher.update(chunk);
        let chunk_hash = format!("{:x}", Sha256::digest(chunk));
        let body = json!({
            "upload_id": upload_id,
            "chunk_index": idx as i64,
            "chunk_hash": chunk_hash,
            "data": b64.encode(chunk),
        });
        if let Err(e) = client.post("/ingest/upload/chunk", body).await {
            eprintln!("Error uploading chunk {}: {}", idx, e);
            return;
        }
    }
    let final_hash = format!("{:x}", full_hasher.finalize());
    let complete_body = json!({
        "upload_id": upload_id,
        "total_chunks": total_chunks as i64,
        "final_sha256": final_hash,
        "mode": mode,
        "category": category,
    });
    match client.post("/ingest/upload/complete", complete_body).await {
        Ok(v) => {
            let memories = v.get("ingested_memories").and_then(|v| v.as_i64()).unwrap_or(0);
            let chunks = v.get("chunks_processed").and_then(|v| v.as_i64()).unwrap_or(0);
            let job = value_as_string(v.get("job_id")).unwrap_or_else(|| "?".into());
            println!("Ingested: job {job} -- {memories} memories, {chunks} chunks");
        }
        Err(e) => eprintln!("Error completing upload: {}", e),
    }
}

async fn handle_jobs_command(client: &Client, cmd: &JobsCommands) {
    match cmd {
        JobsCommands::Stats => match client.get("/jobs/stats").await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => eprintln!("Error: {}", e),
        },
        JobsCommands::List {
            status,
            limit,
            offset,
        } => {
            let path = format!("/jobs?status={status}&limit={limit}&offset={offset}");
            match client.get(&path).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        JobsCommands::Retry { id, all } => {
            if *all {
                // Server has no "retry all" endpoint -- emulate by listing
                // failed jobs and retrying each id.
                let listing = match client.get("/jobs/failed?limit=200&offset=0").await {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Error listing failed jobs: {}", e);
                        return;
                    }
                };
                let empty = Vec::new();
                let jobs = listing
                    .get("jobs")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty);
                if jobs.is_empty() {
                    println!("No failed jobs to retry.");
                    return;
                }
                let mut retried = 0usize;
                for job in jobs {
                    if let Some(job_id) = job.get("id").and_then(|v| v.as_i64()) {
                        match client.post(&format!("/jobs/{job_id}/retry"), json!({})).await {
                            Ok(_) => retried += 1,
                            Err(e) => eprintln!("Retry {job_id} failed: {e}"),
                        }
                    }
                }
                println!("Retried {retried} of {} failed jobs.", jobs.len());
                return;
            }
            let Some(id) = id else {
                eprintln!("Error: provide a job id or --all");
                return;
            };
            match client.post(&format!("/jobs/{id}/retry"), json!({})).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        JobsCommands::Purge { older_than_days } => {
            let body = json!({ "older_than_days": older_than_days });
            match client.post("/jobs/purge", body).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        JobsCommands::Cleanup { older_than_days } => {
            let body = json!({ "older_than_days": older_than_days });
            match client.post("/jobs/cleanup", body).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}

async fn handle_skill_command(client: &Client, cmd: &SkillCommands) {
    match cmd {
        SkillCommands::Search { query, limit } => {
            let body = json!({ "query": query, "limit": limit });
            match client.post("/skills/search", body).await {
                Ok(v) => {
                    let results = v.as_array().cloned().unwrap_or_else(|| {
                        v.get("results")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if results.is_empty() {
                        println!("No results.");
                    }
                    for item in &results {
                        let id = value_as_string(item.get("id")).unwrap_or_else(|| "?".to_string());
                        let trust = item
                            .get("trust_score")
                            .and_then(|x| x.as_f64())
                            .unwrap_or(0.0);
                        let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                        let desc = item
                            .get("description")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        println!(
                            "#{} [trust:{:.2}] {} -- {}",
                            id,
                            trust,
                            name,
                            truncate(desc, 80)
                        );
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::List {
            limit,
            offset,
            agent,
        } => {
            let mut path = format!("/skills?limit={}&offset={}", limit, offset);
            if let Some(a) = agent {
                path.push_str(&format!("&agent={}", a));
            }
            match client.get(&path).await {
                Ok(v) => {
                    let items = v.as_array().cloned().unwrap_or_else(|| {
                        v.get("skills")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if items.is_empty() {
                        println!("No skills.");
                    }
                    for item in &items {
                        let id = value_as_string(item.get("id")).unwrap_or_else(|| "?".to_string());
                        let version =
                            value_as_string(item.get("version")).unwrap_or_else(|| "?".to_string());
                        let trust = item
                            .get("trust_score")
                            .and_then(|x| x.as_f64())
                            .unwrap_or(0.0);
                        let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                        println!("#{} [v{} trust:{:.2}] {}", id, version, trust, name);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Get { id } => match client.get(&format!("/skills/{}", id)).await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => eprintln!("Error: {}", e),
        },

        SkillCommands::Execute {
            id,
            success,
            duration_ms,
            error_type,
            error_message,
        } => {
            let body = json!({
                "success": success,
                "duration_ms": duration_ms,
                "error_type": error_type,
                "error_message": error_message,
            });
            match client.post(&format!("/skills/{}/execute", id), body).await {
                Ok(_) => println!("Recorded execution for skill #{}", id),
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Capture { description, agent } => {
            let body = json!({ "description": description, "agent": agent });
            match client.post("/skills/capture", body).await {
                Ok(v) => {
                    let skill_id =
                        value_as_string(v.get("skill_id")).unwrap_or_else(|| "?".to_string());
                    let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
                    let success = v.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
                    if success {
                        println!("Captured skill #{}: {}", skill_id, message);
                    } else {
                        println!("Capture failed: {}", message);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Fix {
            id,
            direction,
            agent,
        } => {
            let dir = direction.as_deref().unwrap_or("").to_string();
            let body = json!({ "direction": dir, "agent": agent });
            match client.post(&format!("/skills/{}/fix", id), body).await {
                Ok(v) => {
                    let skill_id =
                        value_as_string(v.get("skill_id")).unwrap_or_else(|| "?".to_string());
                    let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
                    let success = v.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
                    if success {
                        println!("Fixed skill #{}: {}", skill_id, message);
                    } else {
                        println!("Fix failed: {}", message);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Derive {
            parent_ids,
            direction,
            agent,
        } => {
            let body = json!({ "parent_ids": parent_ids, "direction": direction, "agent": agent });
            match client.post("/skills/derive", body).await {
                Ok(v) => {
                    let skill_id =
                        value_as_string(v.get("skill_id")).unwrap_or_else(|| "?".to_string());
                    let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
                    let success = v.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
                    if success {
                        println!("Derived skill #{}: {}", skill_id, message);
                    } else {
                        println!("Derive failed: {}", message);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Stats => match client.get("/skills/dashboard/overview").await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => eprintln!("Error: {}", e),
        },

        SkillCommands::Lineage { id } => {
            match client.get(&format!("/skills/{}/lineage", id)).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        SkillCommands::Evolve { hours, limit } => {
            match client
                .get(&format!(
                    "/skills/evolution/recent?hours={}&limit={}",
                    hours, limit
                ))
                .await
            {
                Ok(v) => {
                    let evolutions = v.as_array().cloned().unwrap_or_else(|| {
                        v.get("evolutions")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default()
                    });
                    if evolutions.is_empty() {
                        println!("No recent evolutions.");
                    }
                    for item in &evolutions {
                        let skill_id = value_as_string(item.get("skill_id"))
                            .unwrap_or_else(|| "?".to_string());
                        let version =
                            value_as_string(item.get("version")).unwrap_or_else(|| "?".to_string());
                        let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                        let origin = item.get("origin").and_then(|x| x.as_str()).unwrap_or("?");
                        let parent_ids: Vec<String> = item
                            .get("parent_ids")
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| value_as_string(Some(v)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        println!(
                            "#{} [v{}] {} ({}) -- parents: {:?}",
                            skill_id, version, name, origin, parent_ids
                        );
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}

async fn handle_cred_command(client: &Client, cmd: &CredCommands) {
    match cmd {
        CredCommands::Get {
            category,
            name,
            raw,
        } => {
            match client.get(&format!("/secret/{}/{}", category, name)).await {
                Ok(v) => {
                    if *raw {
                        // Extract primary value
                        if let Some(value) = v.get("value") {
                            let primary = value
                                .get("key")
                                .or_else(|| value.get("password"))
                                .or_else(|| value.get("client_secret"))
                                .or_else(|| value.get("private_key"))
                                .or_else(|| value.get("content"))
                                .and_then(|v| v.as_str());
                            if let Some(s) = primary {
                                println!("{}", s);
                            } else {
                                println!("{}", serde_json::to_string(&value).unwrap_or_default());
                            }
                        }
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        CredCommands::Set {
            category,
            name,
            secret_type,
            value,
            username,
            url,
        } => {
            let secret_value = match value {
                Some(v) => v.clone(),
                None => {
                    eprint!("Enter secret value: ");
                    std::io::Write::flush(&mut std::io::stderr()).ok();
                    rpassword::read_password().unwrap_or_default()
                }
            };

            let data = match secret_type.as_str() {
                "login" => json!({
                    "type": "login",
                    "username": username.clone().unwrap_or_default(),
                    "password": secret_value,
                    "url": url,
                }),
                "api_key" => json!({
                    "type": "api_key",
                    "key": secret_value,
                }),
                "oauth_app" => json!({
                    "type": "oauth_app",
                    "client_id": username.clone().unwrap_or_default(),
                    "client_secret": secret_value,
                    "redirect_uri": url,
                }),
                "ssh_key" => json!({
                    "type": "ssh_key",
                    "private_key": secret_value,
                }),
                "note" => json!({
                    "type": "note",
                    "content": secret_value,
                }),
                _ => json!({
                    "type": "api_key",
                    "key": secret_value,
                }),
            };

            let body = json!({ "data": data });
            match client
                .post(&format!("/secret/{}/{}", category, name), body)
                .await
            {
                Ok(v) => {
                    println!(
                        "Stored {}/{} (id: {})",
                        category,
                        name,
                        value_as_string(v.get("id")).unwrap_or_else(|| "?".to_string())
                    );
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        CredCommands::List { category } => {
            let path = match category {
                Some(cat) => format!("/secrets?category={}", cat),
                None => "/secrets".to_string(),
            };
            match client.get(&path).await {
                Ok(v) => {
                    let secrets = v
                        .get("secrets")
                        .and_then(|s| s.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if secrets.is_empty() {
                        println!("No secrets.");
                    } else {
                        for s in &secrets {
                            let cat = s.get("category").and_then(|x| x.as_str()).unwrap_or("?");
                            let name = s.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let stype =
                                s.get("secret_type").and_then(|x| x.as_str()).unwrap_or("?");
                            println!("{}/{} [{}]", cat, name, stype);
                        }
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        CredCommands::Delete { category, name } => {
            match client
                .delete(&format!("/secret/{}/{}", category, name))
                .await
            {
                Ok(_) => println!("Deleted {}/{}", category, name),
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        CredCommands::AgentCreate {
            name,
            categories,
            allow_raw,
        } => {
            let cats: Vec<String> = categories
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let body = json!({
                "name": name,
                "categories": cats,
                "allow_raw": allow_raw,
            });

            match client.post("/agents", body).await {
                Ok(v) => {
                    if let Some(key) = v.get("key").and_then(|k| k.as_str()) {
                        println!("Agent key created: {}", name);
                        println!("Key: {}", key);
                        println!("Save this key -- it cannot be retrieved later.");
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        CredCommands::AgentList => match client.get("/agents").await {
            Ok(v) => {
                let keys = v
                    .get("keys")
                    .and_then(|k| k.as_array())
                    .cloned()
                    .unwrap_or_default();
                if keys.is_empty() {
                    println!("No agent keys.");
                } else {
                    for k in &keys {
                        let name = k.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let valid = k.get("is_valid").and_then(|v| v.as_bool()).unwrap_or(false);
                        let status = if valid { "active" } else { "revoked" };
                        println!("{} [{}]", name, status);
                    }
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        },

        CredCommands::AgentRevoke { name } => {
            match client
                .post(&format!("/agents/{}/revoke", name), json!({}))
                .await
            {
                Ok(_) => println!("Revoked agent key: {}", name),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}
