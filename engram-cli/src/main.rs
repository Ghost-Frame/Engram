use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "engram-cli")]
#[command(about = "Engram memory system CLI", long_about = None)]
struct Cli {
    /// Server URL
    #[arg(long, default_value = "http://127.0.0.1:7700", env = "ENGRAM_URL")]
    server: String,

    /// API key
    #[arg(long, env = "ENGRAM_KEY")]
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
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Store { content, category, importance, tags, source } => {
            todo!("store: content={}, category={}", content, category)
        }
        Commands::Search { query, limit } => {
            todo!("search: query={}, limit={}", query, limit)
        }
        Commands::Context { query, limit } => {
            todo!("context: query={}, limit={}", query, limit)
        }
        Commands::Recall { id } => {
            todo!("recall: id={}", id)
        }
        Commands::Guard { content } => {
            todo!("guard: content={}", content)
        }
        Commands::List { limit, offset } => {
            todo!("list: limit={}, offset={}", limit, offset)
        }
        Commands::Delete { id } => {
            todo!("delete: id={}", id)
        }
        Commands::Bootstrap { db } => {
            todo!("bootstrap: db={}", db)
        }
    }
}
