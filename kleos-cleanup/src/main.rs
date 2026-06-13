use clap::Parser;
use rusqlite::{params, Connection, Result};

#[derive(Parser, Debug)]
#[command(
    name = "kleos-cleanup",
    about = "One-shot cleanup of activity/growth pollution in memories table"
)]
struct Args {
    /// Path to the SQLite database file
    #[arg(long)]
    db: String,

    /// SQLCipher encryption key (hex string). Read from ENGRAM_DB_KEY env if not passed.
    #[arg(long, env = "ENGRAM_DB_KEY")]
    key: Option<String>,

    /// Actually perform mutations (default is dry-run)
    #[arg(long, default_value_t = false)]
    execute: bool,

    /// Run only the high-precision pollution purge (Step P) plus the FTS
    /// rebuild, skipping the activity-move (A) and growth-dedup (B) steps.
    #[arg(long, default_value_t = false)]
    purge_only: bool,

    /// Operator-supplied SQL WHERE condition to delete from `memories`, run as
    /// an extra step (Step W) after the built-in patterns. For deployment- or
    /// operator-specific junk that is not a universal signature, e.g.
    /// `--delete-where "source = 'kleos-ingest' AND category != 'session-summary'"`.
    /// Dry-run reports the count; execute performs the DELETE.
    #[arg(long)]
    delete_where: Option<String>,
}

/// Delete rows matching an operator-supplied WHERE condition. This is the
/// escape hatch for non-universal, deployment-specific pollution; the condition
/// is the operator's responsibility (they already hold the DB key). Dry-run
/// reports the count; execute performs the DELETE. FTS stays consistent via the
/// Step C rebuild that follows.
fn step_custom_delete(conn: &Connection, where_clause: &str, execute: bool) -> Result<usize> {
    println!("Step W: Operator delete WHERE {}", where_clause);
    let count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM memories WHERE {}", where_clause),
        [],
        |r| r.get(0),
    )?;
    if execute {
        let deleted = conn.execute(&format!("DELETE FROM memories WHERE {}", where_clause), [])?;
        println!("  deleted {} rows", deleted);
        Ok(deleted)
    } else {
        println!("  [DRY RUN] would delete {} rows", count);
        Ok(count as usize)
    }
}

/// High-precision pollution signatures. Each matches only unambiguous junk that
/// was never meant to be a durable memory: pre-Rust-port Node Engram consolidation
/// summaries with a literal "undefined" title, and leaked tool-call / task
/// notification fragments captured verbatim by an over-eager ingestion path.
const POLLUTION_PATTERNS: &[(&str, &str)] = &[
    (
        "consolidation-undefined",
        "source = 'consolidation' AND content LIKE '[Consolidated: undefined]%'",
    ),
    ("task-notification", "content LIKE '%<task-notification>%'"),
    ("tool-use-id-leak", "content LIKE '%<tool-use-id>%'"),
];

/// Delete rows matching each high-precision pollution signature. Dry-run prints
/// per-pattern counts; execute performs the DELETEs. The caller rebuilds the FTS
/// index afterward (Step C) so full-text search stays consistent; the LanceDB
/// vector index is reconciled out-of-band via `kleos-cli admin vector-chunk-sync`.
fn step_purge_pollution(conn: &Connection, execute: bool) -> Result<usize> {
    println!("Step P: Purging high-precision pollution...");
    let mut total = 0usize;
    for (label, where_clause) in POLLUTION_PATTERNS {
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM memories WHERE {}", where_clause),
            [],
            |r| r.get(0),
        )?;
        if execute {
            let deleted =
                conn.execute(&format!("DELETE FROM memories WHERE {}", where_clause), [])?;
            println!("  [{}] deleted {} rows", label, deleted);
            total += deleted;
        } else {
            println!("  [DRY RUN] [{}] would delete {} rows", label, count);
            total += count as usize;
        }
    }
    Ok(total)
}

fn parse_activity_content(content: &str) -> (String, Option<String>, String, String) {
    // Returns (agent, project, action, summary)
    let mut parts = Vec::new();
    let mut rest = content;
    while rest.starts_with('[') {
        if let Some(end) = rest.find(']') {
            parts.push(rest[1..end].to_string());
            rest = rest[end + 1..].trim_start();
        } else {
            break;
        }
    }
    match parts.len() {
        3 => (
            parts[0].clone(),
            Some(parts[1].clone()),
            parts[2].clone(),
            rest.to_string(),
        ),
        2 => (parts[0].clone(), None, parts[1].clone(), rest.to_string()),
        _ => (
            "unknown".to_string(),
            None,
            "unknown".to_string(),
            content.to_string(),
        ),
    }
}

fn step_a_move_activity(conn: &Connection, execute: bool) -> Result<usize> {
    println!("Step A: Moving activity rows from memories to activity_log...");

    struct ActivityRow {
        id: i64,
        content: String,
        importance: f64,
        project: Option<String>,
        user_id: Option<String>,
        created_at: String,
    }

    let mut stmt = conn.prepare(
        "SELECT id, content, importance, project, user_id, created_at FROM memories WHERE category = 'activity'"
    )?;

    let rows: Vec<ActivityRow> = stmt
        .query_map([], |row| {
            Ok(ActivityRow {
                id: row.get(0)?,
                content: row.get(1)?,
                importance: row.get(2)?,
                project: row.get(3)?,
                user_id: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;

    let count = rows.len();

    if execute {
        // CLEAN-2: wrap the per-row INSERT+DELETE in a single transaction. In
        // autocommit each statement fsyncs independently (slow) and an
        // interruption mid-run leaves activity rows half-moved between the two
        // tables. BEGIN IMMEDIATE/COMMIT (with ROLLBACK on error) makes the move
        // atomic; conn is &Connection so we drive the transaction by SQL rather
        // than conn.transaction() (which needs &mut).
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let moved = (|| -> Result<()> {
            for row in &rows {
                let (agent, parsed_project, action, summary) = parse_activity_content(&row.content);
                // Use the row's project column if present, otherwise fall back to
                // the project parsed from the activity content (the fallback was
                // being dropped, permanently blanking project associations for
                // rows with no project column).
                let project = row
                    .project
                    .as_deref()
                    .or(parsed_project.as_deref())
                    .unwrap_or("");
                conn.execute(
                    "INSERT INTO activity_log (agent, action, summary, category, importance, project, user_id, created_at) \
                     VALUES (?1, ?2, ?3, 'activity', ?4, ?5, ?6, ?7)",
                    params![agent, action, summary, row.importance, project, row.user_id, row.created_at],
                )?;
                conn.execute("DELETE FROM memories WHERE id = ?1", params![row.id])?;
            }
            Ok(())
        })();
        match moved {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        println!(
            "  Moved {} activity rows to activity_log and deleted from memories.",
            count
        );
    } else {
        println!(
            "  [DRY RUN] Would move {} activity rows to activity_log.",
            count
        );
        for row in rows.iter().take(5) {
            let (agent, project, action, summary) = parse_activity_content(&row.content);
            println!(
                "    id={} agent={} project={:?} action={} summary={:.60}",
                row.id, agent, project, action, summary
            );
        }
        if count > 5 {
            println!("    ... and {} more", count - 5);
        }
    }

    Ok(count)
}

fn step_b_dedup_growth(conn: &Connection, execute: bool) -> Result<usize> {
    println!("Step B: Deduplicating growth rows...");

    struct GrowthRow {
        id: i64,
        content: String,
        _importance: f64,
        _created_at: String,
    }

    let mut stmt = conn.prepare(
        "SELECT id, content, importance, created_at \
         FROM memories WHERE category = 'growth' \
         ORDER BY content, importance DESC, created_at ASC",
    )?;

    let rows: Vec<GrowthRow> = stmt
        .query_map([], |row| {
            Ok(GrowthRow {
                id: row.get(0)?,
                content: row.get(1)?,
                _importance: row.get(2)?,
                _created_at: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;

    // Group by full content, keep first per group, archive the rest
    let mut to_archive: Vec<i64> = Vec::new();
    let mut current_content: Option<String> = None;
    let mut seen_first = false;

    for row in &rows {
        if current_content.as_deref() != Some(&row.content) {
            current_content = Some(row.content.clone());
            seen_first = true;
        } else if seen_first {
            to_archive.push(row.id);
        }
    }

    let count = to_archive.len();

    if execute {
        // CLEAN-2: wrap the bulk UPDATE loop in one transaction so the dedup is
        // atomic and avoids a per-row fsync. See step_a for why we drive the
        // transaction by SQL rather than conn.transaction().
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let archived = (|| -> Result<()> {
            for id in &to_archive {
                conn.execute(
                    "UPDATE memories SET is_archived = 1 WHERE id = ?1",
                    params![id],
                )?;
            }
            Ok(())
        })();
        match archived {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        println!("  Archived {} duplicate growth rows.", count);
    } else {
        println!("  [DRY RUN] Would archive {} duplicate growth rows.", count);
        for id in to_archive.iter().take(5) {
            println!("    id={} would be archived", id);
        }
        if count > 5 {
            println!("    ... and {} more", count - 5);
        }
    }

    Ok(count)
}

fn step_c_rebuild_fts(conn: &Connection, execute: bool) -> Result<()> {
    println!("Step C: Rebuilding FTS index...");

    if execute {
        conn.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
            [],
        )?;
        println!("  FTS index rebuilt.");
    } else {
        println!("  [DRY RUN] Would rebuild FTS index.");
    }

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.execute {
        println!("=== DRY RUN MODE (pass --execute to perform mutations) ===");
    } else {
        println!("=== EXECUTE MODE -- mutations will be applied ===");
    }
    println!("Database: {}", args.db);
    println!();

    let conn = Connection::open(&args.db)?;

    if let Some(ref key) = args.key {
        // SECURITY: the key is interpolated into a PRAGMA; restrict it to hex so
        // a value containing a quote cannot break out of the `x'...'` literal
        // and inject SQL/PRAGMAs.
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some("--key/ENGRAM_DB_KEY must be a non-empty hex string".into()),
            ));
        }
        // SQLCipher raw-hex key mode, matching the server (kleos-lib pool.rs):
        // emit `PRAGMA key = x'<hex>';` verbatim via execute_batch. pragma_update
        // single-quotes the value, turning the raw key into a passphrase, which
        // fails to open a database that was created with a raw hex key.
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key))?;
        conn.pragma_query_value(None, "schema_version", |_| Ok(()))
            .map_err(|_| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(26),
                    Some("PRAGMA key failed -- wrong key or not a SQLCipher database".into()),
                )
            })?;
    }

    let (activity_count, growth_count) = if args.purge_only {
        println!("Step A: skipped (--purge-only)\n");
        println!("Step B: skipped (--purge-only)\n");
        (0, 0)
    } else {
        let a = step_a_move_activity(&conn, args.execute)?;
        println!();
        let b = step_b_dedup_growth(&conn, args.execute)?;
        println!();
        (a, b)
    };

    let purge_count = step_purge_pollution(&conn, args.execute)?;
    println!();

    let custom_count = if let Some(ref where_clause) = args.delete_where {
        let c = step_custom_delete(&conn, where_clause, args.execute)?;
        println!();
        c
    } else {
        0
    };

    step_c_rebuild_fts(&conn, args.execute)?;
    println!();

    if args.execute {
        println!(
            "Done. Moved {} activity rows, archived {} duplicate growth rows, purged {} pollution rows, deleted {} operator-WHERE rows.",
            activity_count, growth_count, purge_count, custom_count
        );
    } else {
        println!(
            "Dry run complete. Would move {} activity rows, archive {} duplicate growth rows, purge {} pollution rows, delete {} operator-WHERE rows.",
            activity_count, growth_count, purge_count, custom_count
        );
        println!("Re-run with --execute to apply changes.");
    }

    Ok(())
}
