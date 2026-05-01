use clap::Parser;
use rusqlite::{Connection, Result, params};

#[derive(Parser, Debug)]
#[command(name = "kleos-cleanup", about = "One-shot cleanup of activity/growth pollution in memories table")]
struct Args {
    /// Path to the SQLite database file
    #[arg(long)]
    db: String,

    /// Actually perform mutations (default is dry-run)
    #[arg(long, default_value_t = false)]
    execute: bool,
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
        3 => (parts[0].clone(), Some(parts[1].clone()), parts[2].clone(), rest.to_string()),
        2 => (parts[0].clone(), None, parts[1].clone(), rest.to_string()),
        _ => ("unknown".to_string(), None, "unknown".to_string(), content.to_string()),
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

    let rows: Vec<ActivityRow> = stmt.query_map([], |row| {
        Ok(ActivityRow {
            id: row.get(0)?,
            content: row.get(1)?,
            importance: row.get(2)?,
            project: row.get(3)?,
            user_id: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?.collect::<Result<Vec<_>>>()?;

    let count = rows.len();

    if execute {
        for row in &rows {
            let (agent, _parsed_project, action, summary) = parse_activity_content(&row.content);
            // Use the row's project column if present, otherwise fall back to parsed project
            let project = row.project.as_deref().unwrap_or("");
            conn.execute(
                "INSERT INTO activity_log (agent, action, summary, category, importance, project, user_id, created_at) \
                 VALUES (?1, ?2, ?3, 'activity', ?4, ?5, ?6, ?7)",
                params![agent, action, summary, row.importance, project, row.user_id, row.created_at],
            )?;
            conn.execute("DELETE FROM memories WHERE id = ?1", params![row.id])?;
        }
        println!("  Moved {} activity rows to activity_log and deleted from memories.", count);
    } else {
        println!("  [DRY RUN] Would move {} activity rows to activity_log.", count);
        for row in rows.iter().take(5) {
            let (agent, project, action, summary) = parse_activity_content(&row.content);
            println!("    id={} agent={} project={:?} action={} summary={:.60}", row.id, agent, project, action, summary);
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
        prefix: String,
        _importance: f64,
        _created_at: String,
    }

    let mut stmt = conn.prepare(
        "SELECT id, substr(content, 1, 100) as prefix, importance, created_at \
         FROM memories WHERE category = 'growth' \
         ORDER BY substr(content, 1, 100), importance DESC, created_at ASC"
    )?;

    let rows: Vec<GrowthRow> = stmt.query_map([], |row| {
        Ok(GrowthRow {
            id: row.get(0)?,
            prefix: row.get(1)?,
            _importance: row.get(2)?,
            _created_at: row.get(3)?,
        })
    })?.collect::<Result<Vec<_>>>()?;

    // Group by prefix, keep first per group, archive the rest
    let mut to_archive: Vec<i64> = Vec::new();
    let mut current_prefix: Option<String> = None;
    let mut seen_first = false;

    for row in &rows {
        if current_prefix.as_deref() != Some(&row.prefix) {
            current_prefix = Some(row.prefix.clone());
            seen_first = true;
            // This is the keeper for this prefix group
        } else if seen_first {
            to_archive.push(row.id);
        }
    }

    let count = to_archive.len();

    if execute {
        for id in &to_archive {
            conn.execute("UPDATE memories SET is_archived = 1 WHERE id = ?1", params![id])?;
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
        conn.execute("INSERT INTO memories_fts(memories_fts) VALUES('rebuild')", [])?;
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

    let activity_count = step_a_move_activity(&conn, args.execute)?;
    println!();

    let growth_count = step_b_dedup_growth(&conn, args.execute)?;
    println!();

    step_c_rebuild_fts(&conn, args.execute)?;
    println!();

    if args.execute {
        println!("Done. Moved {} activity rows, archived {} duplicate growth rows.", activity_count, growth_count);
    } else {
        println!("Dry run complete. Would move {} activity rows, archive {} duplicate growth rows.", activity_count, growth_count);
        println!("Re-run with --execute to apply changes.");
    }

    Ok(())
}
