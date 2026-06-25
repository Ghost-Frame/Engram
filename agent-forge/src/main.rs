//! agent-forge CLI entrypoint. Each subcommand reads a JSON input file, runs
//! one tool from the `tools` module against the on-disk SQLite forge DB, and
//! writes a JSON result back. External hooks can enforce that agents call
//! these tools before and after editing code.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use agent_forge::db::Database;
use agent_forge::json_io::{read_input, write_output, Output};
use agent_forge::tools;

/// Top-level CLI: every invocation specifies a subcommand plus input/output JSON paths.
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

/// One enum variant per agent-forge tool. Names map 1:1 to the agent-forge
/// tool reference.
#[derive(Subcommand, Debug)]
enum Commands {
    SpecTask,
    ConsiderApproaches,
    LogHypothesis,
    LogOutcome,
    RecallErrors,
    Verify,
    ChallengeCode,
    CommentCheck,
    Checkpoint,
    Rollback,
    SessionLearn,
    SessionRecall,
    SessionDiff,
    Think,
    DeclareUnknowns,
    UpdateSpec,
    ListSpecs,
    GetSpec,
    Stats,
    RepoMap,
    SearchCode,
    SkillSearch,
    SkillCapture,
    SkillRecordExec,
    SkillFix,
    SkillDerive,
    SkillLineage,
}

/// Expand a leading `~/` in a path string to the user's home directory.
fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Parse args, open the forge DB, dispatch to the requested tool, and write
/// the JSON result to `--output`. Any error becomes an `Output::error` payload.
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
        Commands::CommentCheck => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::comments::comment_check(&db, input).map_err(|e| e.to_string())
                })
        }
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
        Commands::ConsiderApproaches => {
            read_input(&cli.input)
                .map_err(|e| e.to_string())
                .and_then(|input| {
                    tools::approaches::consider_approaches(&db, input).map_err(|e| e.to_string())
                })
        }
        Commands::UpdateSpec => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::spec::update_spec(&db, input).map_err(|e| e.to_string())),
        Commands::ListSpecs => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::spec::list_specs(&db, input).map_err(|e| e.to_string())),
        Commands::GetSpec => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::spec::get_spec(&db, input).map_err(|e| e.to_string())),
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
        Commands::SkillSearch => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_search(input).map_err(|e| e.to_string())),
        Commands::SkillCapture => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_capture(input).map_err(|e| e.to_string())),
        Commands::SkillRecordExec => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_record_exec(input).map_err(|e| e.to_string())),
        Commands::SkillFix => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_fix(input).map_err(|e| e.to_string())),
        Commands::SkillDerive => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_derive(input).map_err(|e| e.to_string())),
        Commands::SkillLineage => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::skills::skill_lineage(input).map_err(|e| e.to_string())),
        Commands::Stats => read_input(&cli.input)
            .map_err(|e| e.to_string())
            .and_then(|input| tools::stats::stats(&db, input).map_err(|e| e.to_string())),
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
