//! Verification result persistence for agent-forge.
//!
//! Only the DB-persistence side is ported here. Command execution (`Command::new`)
//! remains client-side because the server cannot run arbitrary commands unless the
//! target path is on a server-visible root. The route handler ships the result of
//! a client-side run and calls `record_verification` to store it.

use crate::db::Database;
use chrono::Utc;
use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;

/// Persist one verification result row to `forge_verifications`.
///
/// Called by the server route handler after receiving a client-side run result.
/// `stdout` and `stderr` are clipped to 4096 bytes on the server side to keep
/// rows reasonably sized; callers should pre-clip or accept that the DB will
/// hold only the first 4096 bytes of each stream.
#[allow(clippy::too_many_arguments)]
pub async fn record_verification(
    db: &Database,
    user_id: i64,
    spec_id: Option<String>,
    command: String,
    exit_code: i32,
    success: bool,
    duration_ms: Option<i64>,
    criteria_index: Option<i64>,
    stdout: Option<String>,
    stderr: Option<String>,
) -> crate::Result<Value> {
    let id = format!("ver_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();
    let success_int = success as i32;
    let id_clone = id.clone();

    // Clip output streams at a char boundary to avoid panic on multi-byte chars.
    let stdout_clipped = stdout.map(|s| clip_at_char_boundary(s, 4096));
    let stderr_clipped = stderr.map(|s| clip_at_char_boundary(s, 4096));

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO forge_verifications
             (id, user_id, spec_id, created_at, command, exit_code, success,
              duration_ms, criteria_index, stdout, stderr)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id_clone,
                user_id,
                spec_id,
                now,
                command,
                exit_code,
                success_int,
                duration_ms,
                criteria_index,
                stdout_clipped,
                stderr_clipped,
            ],
        )?;
        Ok(())
    })
    .await?;

    Ok(serde_json::json!({
        "id": id,
        "message": "Verification recorded",
        "success": success,
    }))
}

/// Clip a String to at most `max` bytes without splitting a UTF-8 character.
///
/// Subprocess output is arbitrary, so a raw byte slice can panic on a
/// multi-byte boundary. This walks back to the nearest char boundary before
/// truncating.
fn clip_at_char_boundary(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
