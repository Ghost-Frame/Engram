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
        /// Importance score 0.0-1.0
        #[arg(short, long)]
        importance: Option<f32>,
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
        #[arg(short, long, default_value = "engram.db")]
        db: String,
    },
    /// Credential management (talks to credd)
    #[command(subcommand)]
    Cred(CredCommands),
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

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

        Commands::Cred(cred_cmd) => {
            let cred_client = Client::new(cli.credd_url.clone(), api_key.clone());
            handle_cred_command(&cred_client, cred_cmd).await;
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
