use clap::{Parser, Subcommand};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "kleos-cli")]
#[command(about = "Kleos memory system CLI", long_about = None)]
struct Cli {
    /// Server URL
    #[arg(long, default_value = "http://127.0.0.1:4200", env = "KLEOS_URL")]
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
    /// Surface memories most in need of reinforcement for a topic
    RecallDue {
        /// Topic to search for
        topic: String,
        /// Maximum results
        #[arg(short, long, default_value = "5")]
        limit: usize,
        /// Session filter
        #[arg(short, long)]
        session: Option<String>,
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
    /// Session handoff management
    #[command(subcommand)]
    Handoff(HandoffCommands),
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
    /// Fetch a secret from credd and exec a child command with the secret
    /// injected as an environment variable. The secret is set in the
    /// child's environment block directly and is never written to stdout,
    /// stderr, or the process command line, so it does not leak into shell
    /// history, agent context capture, or `ps` output.
    ///
    /// Example:
    ///   kleos-cli cred exec kleos claude-code-wsl --env EIDOLON_KEY -- \
    ///     curl -H "Authorization: Bearer $EIDOLON_KEY" http://...
    Exec {
        /// Category (service namespace)
        category: String,
        /// Secret name
        name: String,
        /// Env var name to set in the child process
        #[arg(long)]
        env: String,
        /// Specific field to extract (defaults to the primary value:
        /// key/password/client_secret/private_key/content in that order).
        #[arg(long)]
        field: Option<String>,
        /// Command + args to exec. Use `--` to separate from cred flags.
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
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

#[derive(Subcommand)]
enum HandoffCommands {
    /// Store a session handoff
    Dump {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, name = "type")]
        handoff_type: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Get latest handoff(s) with filters
    Restore {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, name = "type")]
        handoff_type: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value = "1")]
        limit: i64,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Get the single latest handoff
    Latest {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Gather and store mechanical state from git
    Mechanical {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        host: Option<String>,
    },
    /// List recent handoffs
    List {
        #[arg(long, default_value = "20")]
        limit: i64,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, name = "type")]
        handoff_type: Option<String>,
    },
    /// Full-text search across handoff content
    Search {
        query: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "10")]
        limit: i64,
    },
    /// Show database statistics
    Stats,
    /// Garbage-collect old handoffs
    Gc {
        #[arg(long)]
        tiered: bool,
        #[arg(long)]
        keep: Option<i64>,
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
        let resp = req
            .send()
            .await
            .map_err(|e| format_reqwest_error("GET", &url, &e))?;
        self.handle_response("GET", &url, resp).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format_reqwest_error("POST", &url, &e))?;
        self.handle_response("POST", &url, resp).await
    }

    async fn delete(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.delete(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format_reqwest_error("DELETE", &url, &e))?;
        self.handle_response("DELETE", &url, resp).await
    }

    async fn handle_response(
        &self,
        method: &str,
        url: &str,
        resp: reqwest::Response,
    ) -> Result<Value, String> {
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| {
            format!(
                "{} {} succeeded but reading response body failed: {}",
                method,
                url,
                format_error_chain(&e)
            )
        })?;
        let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
        if status.is_success() {
            parsed.map_err(|e| {
                format!(
                    "{} {} returned {} but body was not valid JSON: {} (body: {})",
                    method,
                    url,
                    status,
                    e,
                    body_excerpt(&bytes)
                )
            })
        } else {
            let msg = parsed
                .as_ref()
                .ok()
                .and_then(|b| {
                    b.get("error")
                        .or_else(|| b.get("message"))
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| body_excerpt(&bytes));
            Err(format!("HTTP {}: {}", status, msg))
        }
    }
}

fn format_reqwest_error(method: &str, url: &str, err: &reqwest::Error) -> String {
    format!("{} {} failed: {}", method, url, format_error_chain(err))
}

fn format_error_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
    let mut out = err.to_string();
    let mut source: Option<&dyn std::error::Error> = err.source();
    for _ in 0..16 {
        let Some(cause) = source else { break };
        out.push_str(" -> ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

fn body_excerpt(bytes: &[u8]) -> String {
    const MAX: usize = 512;
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= MAX {
        return s.into_owned();
    }
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes total)", &s[..end], bytes.len())
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
    let api_key = if let Some(k) = cli.key.clone() {
        Some(k)
    } else {
        let slot = kleos_lib::cred::bootstrap::current_agent_slot();
        match kleos_lib::cred::bootstrap::resolve_api_key(&slot).await {
            Ok(k) => Some(k),
            Err(e) => {
                eprintln!("warning: could not resolve API key: {}", e);
                None
            }
        }
    };
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

        Commands::RecallDue {
            topic,
            limit,
            session,
        } => {
            let encoded_topic = utf8_percent_encode(topic, NON_ALPHANUMERIC).to_string();
            let mut url = format!("/fsrs/recall-due?topic={}&limit={}", encoded_topic, limit);
            if let Some(s) = &session {
                let encoded_s = utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
                url.push_str(&format!("&session={}", encoded_s));
            }
            match client.get(&url).await {
                Ok(v) => {
                    let results = v
                        .get("results")
                        .and_then(|r| r.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if results.is_empty() {
                        println!("No recall-due memories for \"{}\".", topic);
                    }
                    for item in &results {
                        let id = value_as_string(item.get("memory_id"))
                            .unwrap_or_else(|| "?".to_string());
                        let r = item
                            .get("retrievability")
                            .and_then(|x| x.as_f64())
                            .map(|v| format!("R={:.2}", v))
                            .unwrap_or_else(|| "R=?".to_string());
                        let score = item
                            .get("recall_due_score")
                            .and_then(|x| x.as_f64())
                            .map(|s| format!("{:.3}", s))
                            .unwrap_or_else(|| "?".to_string());
                        let content = item.get("content").and_then(|x| x.as_str()).unwrap_or("");
                        println!("#{} [{}] ({}) {}", id, score, r, truncate(content, 90));
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
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
            // credd's auth middleware only accepts the cred master key,
            // a DB-backed agent key, or a file-backed bootstrap-agent
            // token. The Kleos bearer in `api_key` is none of those, so
            // pull credd auth from CREDD_AGENT_KEY env (set by the shell
            // rc from ~/.config/cred/credd-agent-key.token) and only
            // fall back to the Kleos bearer if that is missing.
            let credd_token = std::env::var("CREDD_AGENT_KEY")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let path = std::env::var("HOME")
                        .map(|h| {
                            std::path::PathBuf::from(h)
                                .join(".config/cred/credd-agent-key.token")
                        })
                        .ok()?;
                    std::fs::read_to_string(path)
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .or_else(|| api_key.clone());
            let cred_client = Client::new(cli.credd_url.clone(), credd_token);
            handle_cred_command(&cred_client, cred_cmd).await;
        }

        Commands::Handoff(handoff_cmd) => {
            handle_handoff_command(&client, handoff_cmd).await;
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
    let filename = file.as_ref().and_then(|p| {
        p.file_name()
            .and_then(|n| n.to_str().map(|s| s.to_string()))
    });
    let content_type =
        filename
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
            let memories = v
                .get("ingested_memories")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let chunks = v
                .get("chunks_processed")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
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
                        match client
                            .post(&format!("/jobs/{job_id}/retry"), json!({}))
                            .await
                        {
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

        CredCommands::Exec {
            category,
            name,
            env,
            field,
            cmd,
        } => {
            // Two routes depending on what the caller is asking for:
            //
            //   category in {"engram-rust","kleos"} -> per-agent Kleos
            //     bearer via /bootstrap/kleos-bearer (the bootstrap-broker
            //     path; bootstrap-agent token has scope for this).
            //   otherwise -> centralized credd secret store via
            //     /secret/{cat}/{name} (requires a DB-backed agent key
            //     with category permissions).
            //
            // The bootstrap path is the one most agents need (injecting
            // a Kleos API key into a child like curl), so it gets the
            // easy `kleos-cli cred exec engram-rust <slot>` form.
            let secret = if matches!(category.as_str(), "engram-rust" | "kleos") {
                // Use the same bootstrap-broker path that resolve_api_key
                // takes -- this picks up CREDD_SOCKET / CREDD_BIND /
                // CREDD_AGENT_KEY / PIV pubkeys automatically and works
                // without needing a Kleos API key already in hand.
                match kleos_lib::cred::bootstrap::resolve_api_key(name).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error fetching bootstrap bearer: {}", e);
                        std::process::exit(2);
                    }
                }
            } else {
                let secret_value = match client.get(&format!("/secret/{}/{}", category, name)).await
                {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Error fetching secret: {}", e);
                        std::process::exit(2);
                    }
                };
                let value_obj = match secret_value.get("value") {
                    Some(v) => v,
                    None => {
                        eprintln!("Error: response missing `value` field");
                        std::process::exit(2);
                    }
                };
                if let Some(f) = field.as_deref() {
                    match value_obj.get(f).and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => {
                            eprintln!("Error: field `{}` not found or not a string", f);
                            std::process::exit(2);
                        }
                    }
                } else {
                    let primary = value_obj
                        .get("key")
                        .or_else(|| value_obj.get("password"))
                        .or_else(|| value_obj.get("client_secret"))
                        .or_else(|| value_obj.get("private_key"))
                        .or_else(|| value_obj.get("content"))
                        .and_then(|v| v.as_str());
                    match primary {
                        Some(s) => s.to_string(),
                        None => {
                            eprintln!(
                                "Error: no primary value (key/password/client_secret/private_key/content) in secret"
                            );
                            std::process::exit(2);
                        }
                    }
                }
            };

            // Build child Command. The secret is passed via env() which
            // hands it directly to the kernel exec call -- it never
            // appears on the command line, in stdout, or in shell history.
            let (program, args) = match cmd.split_first() {
                Some((p, a)) => (p.clone(), a.to_vec()),
                None => {
                    eprintln!("Error: no command supplied after `--`");
                    std::process::exit(2);
                }
            };
            let status = std::process::Command::new(&program)
                .args(&args)
                .env(env, &secret)
                .status();
            match status {
                Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                Err(e) => {
                    eprintln!("Error: failed to exec `{}`: {}", program, e);
                    std::process::exit(2);
                }
            }
        }
    }
}

fn detect_project(dir: Option<&str>) -> Option<String> {
    let dir = dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    if let Ok(val) = std::env::var("SESSION_HANDOFF_PROJECT") {
        if !val.is_empty() {
            return Some(val);
        }
    }

    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&dir)
        .output()
        .ok()?;
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let name = url.rsplit('/').next().unwrap_or(&url);
        let name = name.strip_suffix(".git").unwrap_or(name);
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }

    dir.file_name().map(|n| n.to_string_lossy().to_string())
}

fn detect_branch(dir: Option<&str>) -> Option<String> {
    let dir = dir.unwrap_or(".");
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    None
}

fn detect_host() -> String {
    if let Ok(val) = std::env::var("SESSION_HANDOFF_HOST") {
        if !val.is_empty() {
            return val;
        }
    }
    if std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists() {
        return "wsl".to_string();
    }
    if cfg!(target_os = "windows") {
        return "windows".to_string();
    }
    if std::path::Path::new("/etc/cachyos-release").exists() {
        return "cachyos".to_string();
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn detect_agent() -> String {
    std::env::var("SESSION_HANDOFF_AGENT").unwrap_or_else(|_| "unknown".to_string())
}

fn detect_model() -> Option<String> {
    std::env::var("SESSION_HANDOFF_MODEL")
        .or_else(|_| std::env::var("ANTHROPIC_MODEL"))
        .or_else(|_| std::env::var("MODEL_ID"))
        .ok()
        .filter(|s| !s.is_empty())
}

async fn handle_handoff_command(client: &Client, cmd: &HandoffCommands) {
    match cmd {
        HandoffCommands::Dump {
            project,
            branch,
            agent,
            handoff_type,
            session,
            model,
            host,
            content,
            dir,
        } => {
            let content_str = if let Some(c) = content {
                c.clone()
            } else {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .unwrap_or_else(|e| {
                        eprintln!("Error reading stdin: {}", e);
                        std::process::exit(1);
                    });
                if buf.is_empty() {
                    eprintln!("Error: provide --content or pipe content via stdin");
                    std::process::exit(1);
                }
                buf
            };

            let dir_str = dir.as_deref();
            let project = project.clone().or_else(|| detect_project(dir_str));
            let branch = branch.clone().or_else(|| detect_branch(dir_str));

            let Some(ref project) = project else {
                eprintln!("Error: could not detect project. Use --project");
                std::process::exit(1);
            };

            let body = json!({
                "project": project,
                "branch": branch,
                "directory": dir.clone().or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string())),
                "agent": agent.clone().unwrap_or_else(detect_agent),
                "type": handoff_type.clone().unwrap_or_else(|| "manual".to_string()),
                "content": content_str,
                "session_id": session.clone().or_else(|| std::env::var("SESSION_ID").ok()),
                "model": model.clone().or_else(detect_model),
                "host": host.clone().unwrap_or_else(detect_host),
            });

            match client.post("/handoffs", body).await {
                Ok(v) => {
                    if v.get("skipped").and_then(|s| s.as_bool()).unwrap_or(false) {
                        eprintln!("Skipped duplicate handoff for '{}'", project);
                    } else if let Some(id) = v.get("id") {
                        println!("Stored handoff #{} (project={})", id, project);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::Restore {
            project,
            agent,
            handoff_type,
            model,
            session,
            since,
            limit,
            dir,
        } => {
            let project = project.clone().or_else(|| detect_project(dir.as_deref()));
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(ref p) = project {
                params.push(("project", p.clone()));
            }
            if let Some(ref a) = agent {
                params.push(("agent", a.clone()));
            }
            if let Some(ref t) = handoff_type {
                params.push(("type", t.clone()));
            }
            if let Some(ref m) = model {
                params.push(("model", m.clone()));
            }
            if let Some(ref s) = session {
                params.push(("session_id", s.clone()));
            }
            if let Some(ref s) = since {
                params.push(("since", s.clone()));
            }
            params.push(("limit", limit.to_string()));

            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, utf8_percent_encode(v, NON_ALPHANUMERIC)))
                .collect::<Vec<_>>()
                .join("&");

            match client.get(&format!("/handoffs?{}", query)).await {
                Ok(v) => {
                    if let Some(handoffs) = v.get("handoffs").and_then(|h| h.as_array()) {
                        for h in handoffs {
                            if let Some(content) = h.get("content").and_then(|c| c.as_str()) {
                                println!("{}", content);
                                if handoffs.len() > 1 {
                                    println!("\n---\n");
                                }
                            }
                        }
                        if handoffs.is_empty() {
                            eprintln!("No handoffs found");
                        }
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::Latest { project, dir } => {
            let project = project.clone().or_else(|| detect_project(dir.as_deref()));
            let query = if let Some(ref p) = project {
                format!("?project={}", utf8_percent_encode(p, NON_ALPHANUMERIC))
            } else {
                String::new()
            };

            match client.get(&format!("/handoffs/latest{}", query)).await {
                Ok(v) => {
                    if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
                        println!("{}", content);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::Mechanical {
            project,
            agent,
            dir,
            session,
            model,
            host,
        } => {
            let work_dir = dir.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            let project = project.clone().or_else(|| detect_project(Some(&work_dir)));

            let Some(ref project) = project else {
                eprintln!("Error: could not detect project. Use --project");
                std::process::exit(1);
            };

            let branch = detect_branch(Some(&work_dir));

            let git = |args: &[&str]| -> String {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&work_dir)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default()
            };

            let status = git(&["status", "--porcelain"]);
            let log = git(&["log", "--oneline", "--no-decorate", "-15"]);
            let diff_stat = git(&["diff", "--stat", "HEAD"]);
            let stash_list = git(&["stash", "list"]);

            let recent_files = std::process::Command::new("find")
                .args([
                    &*work_dir,
                    "-maxdepth",
                    "4",
                    "-mmin",
                    "-30",
                    "-not",
                    "-path",
                    "*/.git/*",
                    "-not",
                    "-path",
                    "*/node_modules/*",
                    "-not",
                    "-path",
                    "*/__pycache__/*",
                    "-not",
                    "-path",
                    "*/target/*",
                    "-type",
                    "f",
                    "-print",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            let recent_lines: Vec<&str> = recent_files.lines().take(30).collect();

            let mut content = format!("# Mechanical State: {}\n\n", project);
            content.push_str(&format!("**Directory:** {}\n", work_dir));
            if let Some(ref b) = branch {
                content.push_str(&format!("**Branch:** {}\n", b));
            }
            content.push_str(&format!(
                "Generated: {}\n\n",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            ));

            if !status.is_empty() {
                content.push_str("## Git Status\n```\n");
                content.push_str(&status);
                content.push_str("\n```\n\n");
            }
            if !log.is_empty() {
                content.push_str("## Recent Commits\n```\n");
                content.push_str(&log);
                content.push_str("\n```\n\n");
            }
            if !diff_stat.is_empty() {
                content.push_str("## Uncommitted Changes\n```\n");
                content.push_str(&diff_stat);
                content.push_str("\n```\n\n");
            }
            if !stash_list.is_empty() {
                content.push_str("## Git Stashes\n```\n");
                content.push_str(&stash_list);
                content.push_str("\n```\n\n");
            }
            if !recent_lines.is_empty() {
                content.push_str("## Recently Modified Files\n");
                for f in &recent_lines {
                    content.push_str(&format!("- {}\n", f));
                }
                content.push('\n');
            }

            let body = json!({
                "project": project,
                "branch": branch,
                "directory": work_dir,
                "agent": agent.clone().unwrap_or_else(detect_agent),
                "type": "mechanical",
                "content": content,
                "session_id": session.clone().or_else(|| std::env::var("SESSION_ID").ok()),
                "model": model.clone().or_else(detect_model),
                "host": host.clone().unwrap_or_else(detect_host),
                "metadata": json!({"cwd": work_dir, "auto": true}),
            });

            match client.post("/handoffs", body).await {
                Ok(v) => {
                    if v.get("skipped").and_then(|s| s.as_bool()).unwrap_or(false) {
                        eprintln!("Skipped duplicate mechanical handoff for '{}'", project);
                    } else if let Some(id) = v.get("id") {
                        println!("Stored mechanical handoff #{} (project={})", id, project);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::List {
            limit,
            project,
            agent,
            handoff_type,
        } => {
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(ref p) = project {
                params.push(("project", p.clone()));
            }
            if let Some(ref a) = agent {
                params.push(("agent", a.clone()));
            }
            if let Some(ref t) = handoff_type {
                params.push(("type", t.clone()));
            }
            params.push(("limit", limit.to_string()));

            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, utf8_percent_encode(v, NON_ALPHANUMERIC)))
                .collect::<Vec<_>>()
                .join("&");

            match client.get(&format!("/handoffs?{}", query)).await {
                Ok(v) => {
                    if let Some(handoffs) = v.get("handoffs").and_then(|h| h.as_array()) {
                        println!(
                            "{:>6}  {:<20}  {:<20}  {:<12}  {:<10}  {:<8}  {:<10}  {:>6}",
                            "ID", "Created", "Project", "Agent", "Type", "Host", "Session", "Size"
                        );
                        println!("{}", "-".repeat(100));
                        for h in handoffs {
                            let session_display = h
                                .get("session_id")
                                .and_then(|s| s.as_str())
                                .map(|s| &s[..s.len().min(8)])
                                .unwrap_or("");
                            let content_len = h
                                .get("content")
                                .and_then(|c| c.as_str())
                                .map(|c| c.len())
                                .unwrap_or(0);
                            println!(
                                "{:>6}  {:<20}  {:<20}  {:<12}  {:<10}  {:<8}  {:<10}  {:>6}",
                                h.get("id").and_then(|i| i.as_i64()).unwrap_or(0),
                                h.get("created_at").and_then(|s| s.as_str()).unwrap_or(""),
                                h.get("project").and_then(|s| s.as_str()).unwrap_or(""),
                                h.get("agent").and_then(|s| s.as_str()).unwrap_or(""),
                                h.get("type").and_then(|s| s.as_str()).unwrap_or(""),
                                h.get("host").and_then(|s| s.as_str()).unwrap_or(""),
                                session_display,
                                content_len,
                            );
                        }
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::Search {
            query,
            project,
            limit,
        } => {
            let mut params: Vec<(&str, String)> = vec![("q", query.clone())];
            if let Some(ref p) = project {
                params.push(("project", p.clone()));
            }
            params.push(("limit", limit.to_string()));

            let query_str = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, utf8_percent_encode(v, NON_ALPHANUMERIC)))
                .collect::<Vec<_>>()
                .join("&");

            match client.get(&format!("/handoffs/search?{}", query_str)).await {
                Ok(v) => {
                    if let Some(results) = v.get("results").and_then(|r| r.as_array()) {
                        for r in results {
                            let model_str = r
                                .get("model")
                                .and_then(|m| m.as_str())
                                .map(|m| format!(" [model={}]", m))
                                .unwrap_or_default();
                            println!(
                                "#{:<5}  {}  [{}]  {}  {}{}",
                                r.get("id").and_then(|i| i.as_i64()).unwrap_or(0),
                                r.get("created_at").and_then(|s| s.as_str()).unwrap_or(""),
                                r.get("project").and_then(|s| s.as_str()).unwrap_or(""),
                                r.get("agent").and_then(|s| s.as_str()).unwrap_or(""),
                                r.get("type").and_then(|s| s.as_str()).unwrap_or(""),
                                model_str,
                            );
                            if let Some(snippet) = r.get("snippet").and_then(|s| s.as_str()) {
                                println!("  {}", snippet.replace('\n', " "));
                            }
                        }
                        if results.is_empty() {
                            eprintln!("No results found");
                        }
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        HandoffCommands::Stats => match client.get("/handoffs/stats").await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => eprintln!("Error: {}", e),
        },

        HandoffCommands::Gc { tiered, keep } => {
            let body = json!({
                "tiered": tiered,
                "keep": keep,
            });
            match client.post("/handoffs/gc", body).await {
                Ok(v) => {
                    let deleted = v.get("deleted").and_then(|d| d.as_i64()).unwrap_or(0);
                    let remaining = v.get("remaining").and_then(|r| r.as_i64()).unwrap_or(0);
                    println!("Deleted {} handoffs. Remaining: {}", deleted, remaining);
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}
