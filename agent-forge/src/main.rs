mod db;
mod json_io;
mod tools;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agent-forge")]
#[command(about = "Structured reasoning and code quality workflow")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to input JSON file
    #[arg(long)]
    input: PathBuf,

    /// Path to output JSON file
    #[arg(long)]
    output: PathBuf,

    /// Path to database file
    #[arg(long, default_value = "~/.agent-forge/forge.db")]
    db: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Commands {
    SpecTask,
    LogHypothesis,
    Verify,
    Checkpoint,
    Rollback,
    SessionLearn,
    SessionRecall,
    RepoMap,
    SearchCode,
}

fn main() {
    let cli = Cli::parse();
    println!("agent-forge: {:?}", cli.command);
}
