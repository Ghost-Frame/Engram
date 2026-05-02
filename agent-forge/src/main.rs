use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod db;
mod json_io;
mod tools;
mod treesitter;

use db::Database;
use json_io::{read_input, write_output, Output};

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
    db: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    SpecTask,
    ConsiderApproaches,
    LogHypothesis,
    LogOutcome,
    RecallErrors,
    Verify,
    ChallengeCode,
    Checkpoint,
    Rollback,
    SessionLearn,
    SessionRecall,
    SessionDiff,
    Think,
    DeclareUnknowns,
    RepoMap,
    SearchCode,
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn main() {
    let cli = Cli::parse();

    let db_path = expand_path(&cli.db);
    let db = match Database::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            let output = Output::error(format!("Database error: {}", e));
            write_output(&cli.output, &output).ok();
            std::process::exit(1);
        }
    };

    let result = match cli.command {
        Commands::SpecTask => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::spec::spec_task(&db, input).map_err(|e| e.to_string())),
        Commands::LogHypothesis => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::hypothesis::log_hypothesis(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::LogOutcome => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::hypothesis::log_outcome(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::RecallErrors => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::hypothesis::recall_errors(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::Verify => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::verify::verify(&db, input).map_err(|e| e.to_string())),
        Commands::ChallengeCode => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::verify::challenge_code(&db, input).map_err(|e| e.to_string())),
        Commands::SessionDiff => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::verify::session_diff(&db, input).map_err(|e| e.to_string())),
        Commands::Checkpoint => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::session::checkpoint(&db, input).map_err(|e| e.to_string())),
        Commands::Rollback => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::session::rollback(&db, input).map_err(|e| e.to_string())),
        Commands::SessionLearn => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::session::session_learn(&db, input).map_err(|e| e.to_string())),
        Commands::SessionRecall => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::session::session_recall(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::Think => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::think::think(&db, input).map_err(|e| e.to_string())),
        Commands::DeclareUnknowns => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::think::declare_unknowns(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::ConsiderApproaches => Ok(Output::error("Not yet implemented")),
        Commands::RepoMap => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| {
                tools::ast::repo_map::repo_map(&db, input).map_err(|e| e.to_string())
            }),
        Commands::SearchCode => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::ast::search::search_code(&db, input).map_err(|e| e.to_string())
                })
        }
    };

    let output = match result {
        Ok(out) => out,
        Err(e) => Output::error(e),
    };

    if let Err(e) = write_output(&cli.output, &output) {
        eprintln!("Failed to write output: {}", e);
        std::process::exit(1);
    }
}
