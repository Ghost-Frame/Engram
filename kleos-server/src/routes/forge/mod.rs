//! Route family for agent-forge stateful operations.
//!
//! All handlers require a valid bearer token. The authenticated `user_id`
//! provides the tenant isolation boundary for every `forge_*` table query.
//!
//! ## Ledger write for the `ke` edit-gate
//!
//! `ke` (kleos-fs) calls `GET /scratchpad/get?namespace=spec-task&key=<session_id>:<path>`
//! to check whether a spec covers a file before allowing an edit.
//!
//! The forge `spec-task` handler writes the ledger entry via
//! `kleos_lib::scratchpad::upsert_entry` using the column mapping:
//!   - session    = `<spec session_id>`
//!   - agent      = `"spec-task"` (matches `namespace=spec-task` in ke's GET)
//!   - model      = `""`
//!   - entry_key  = `<session_id>:<absolute_file_path>` (exactly what `ke` queries)
//!   - value      = the new spec ID
//!
//! The GET endpoint `GET /scratchpad/get` is served by the scratchpad router
//! and uses `kleos_lib::scratchpad::get_by_namespace_key` to resolve the lookup.
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::EngError;
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;

mod fsroots;
mod types;
use types::{
    ConsiderApproachesBody, DeclareUnknownsBody, FileOrContentBody, ListSpecsQuery,
    LogHypothesisBody, LogOutcomeBody, RecallErrorsQuery, RepoMapBody, SearchCodeBody,
    SessionLearnBody, SessionRecallQuery, SpecTaskBody, ThinkBody, UpdateSpecBody, VerifyBody,
};

/// Build the forge route family and register all endpoints.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/forge/spec-task", post(spec_task_handler))
        .route("/forge/update-spec", post(update_spec_handler))
        .route("/forge/specs", get(list_specs_handler))
        .route("/forge/spec/{id}", get(get_spec_handler))
        .route("/forge/log-hypothesis", post(log_hypothesis_handler))
        .route("/forge/log-outcome", post(log_outcome_handler))
        .route("/forge/recall-errors", get(recall_errors_handler))
        .route(
            "/forge/consider-approaches",
            post(consider_approaches_handler),
        )
        .route("/forge/verify", post(verify_handler))
        .route("/forge/session-learn", post(session_learn_handler))
        .route("/forge/session-recall", get(session_recall_handler))
        // Stateless compute routes backed by the agent_forge library.
        .route("/forge/think", post(think_handler))
        .route("/forge/declare-unknowns", post(declare_unknowns_handler))
        .route("/forge/comment-check", post(comment_check_handler))
        .route("/forge/challenge-code", post(challenge_code_handler))
        .route("/forge/repo-map", post(repo_map_handler))
        .route("/forge/search-code", post(search_code_handler))
}

/// Map an `agent_forge::tools::ToolError` to an `AppError` for HTTP responses.
///
/// `MissingField` and `InvalidValue` become 400 Bad Request; `IoError` and
/// `DatabaseError` become 500 Internal Server Error with details logged
/// server-side only (not echoed to the caller).
fn tool_error_to_app(e: agent_forge::tools::ToolError) -> AppError {
    match e {
        agent_forge::tools::ToolError::MissingField(msg) => {
            AppError(EngError::InvalidInput(format!("missing field: {}", msg)))
        }
        agent_forge::tools::ToolError::InvalidValue(msg) => {
            AppError(EngError::InvalidInput(format!("invalid value: {}", msg)))
        }
        agent_forge::tools::ToolError::IoError(msg) => {
            tracing::error!("forge compute I/O error: {}", msg);
            AppError(EngError::Internal("compute I/O error".into()))
        }
        agent_forge::tools::ToolError::DatabaseError(msg) => {
            tracing::error!("forge compute database error: {}", msg);
            AppError(EngError::Internal("compute database error".into()))
        }
    }
}

/// Open a throwaway `agent_forge::db::Database` for compute calls that accept
/// a `&Database` argument but do not use it (all `_db` params).
///
/// The database lives in a fresh temp file. Finding [42] residual: the old
/// version dropped the NamedTempFile guard (deleting the file) and then let
/// `Database::open` recreate a plain file at the freed path -- which nothing
/// ever deleted, so every compute call leaked a SQLite file into the temp dir
/// until reboot. Returning a `TempPath` guard keeps deletion tied to the
/// request: the caller holds it for the duration of the tool call and the
/// file is removed when both values drop. `into_temp_path` closes the write
/// handle first, so rusqlite is the only writer on the path.
fn throwaway_db() -> Result<(agent_forge::db::Database, tempfile::TempPath), AppError> {
    let tmp = NamedTempFile::new().map_err(|e| {
        tracing::error!("failed to create temp db: {}", e);
        AppError(EngError::Internal("failed to create throwaway db".into()))
    })?;
    let path = tmp.into_temp_path();
    let db = agent_forge::db::Database::open(&path).map_err(|e| {
        tracing::error!("failed to open throwaway db: {}", e);
        AppError(EngError::Internal("failed to open throwaway db".into()))
    })?;
    Ok((db, path))
}

/// Resolve a caller-supplied `path` through the FS roots allow-list, or write
/// inline `content` to a `NamedTempFile` and return its path.
///
/// Returns `(resolved_path_string, Option<NamedTempFile>)`. The `NamedTempFile`
/// must be kept alive until the tool call finishes so the path remains valid.
/// When None is returned from this function as `Err(AppError)`, both a path and
/// content were missing or the path was outside the allowed roots.
fn resolve_path_or_content(
    path: Option<String>,
    content: Option<String>,
    extension: Option<String>,
) -> Result<(String, Option<NamedTempFile>), AppError> {
    if let Some(src) = content {
        // Write inline content to a temp file so the tool can read it by path.
        let mut builder = tempfile::Builder::new();
        // Bind the suffix string to a local so the borrow on `builder` outlives
        // the `format!` temporary (which would otherwise be freed immediately).
        let suffix_owned;
        if let Some(ref ext) = extension {
            suffix_owned = format!(".{}", ext);
            builder.suffix(&suffix_owned);
        }
        let mut tmp = builder.tempfile().map_err(|e| {
            tracing::error!("failed to create content temp file: {}", e);
            AppError(EngError::Internal("failed to create temp file".into()))
        })?;
        std::io::Write::write_all(&mut tmp, src.as_bytes()).map_err(|e| {
            tracing::error!("failed to write content temp file: {}", e);
            AppError(EngError::Internal("failed to write temp file".into()))
        })?;
        let path_str = tmp.path().to_string_lossy().into_owned();
        return Ok((path_str, Some(tmp)));
    }

    if let Some(p) = path {
        match fsroots::resolve_within_roots(&p) {
            Some(canonical) => {
                return Ok((canonical.to_string_lossy().into_owned(), None));
            }
            None => {
                return Err(AppError(EngError::InvalidInput(
                    "path not within KLEOS_FORGE_FS_ROOTS and no content supplied".into(),
                )));
            }
        }
    }

    Err(AppError(EngError::InvalidInput(
        "path not within KLEOS_FORGE_FS_ROOTS and no content supplied".into(),
    )))
}

/// `POST /forge/think` -- pure structured-reasoning prompt builder.
///
/// Accepts a problem statement, optional constraints, and optional context.
/// Returns the generated reasoning prompt and supporting metadata. No filesystem
/// access; no database writes.
async fn think_handler(
    Auth(_auth): Auth,
    Json(body): Json<ThinkBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    let input = agent_forge::tools::think::ThinkInput {
        problem: body.problem,
        constraints: body.constraints,
        context: body.context,
    };
    let output = agent_forge::tools::think::think(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// `POST /forge/declare-unknowns` -- partition unknowns into blocking and
/// non-blocking sets and return a clear action directive.
///
/// Pure: no filesystem access; no database writes.
async fn declare_unknowns_handler(
    Auth(_auth): Auth,
    Json(body): Json<DeclareUnknownsBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    // Convert the request body unknowns to the agent_forge type.
    let unknowns: Option<Vec<agent_forge::tools::think::UnknownItem>> =
        body.unknowns.map(|items| {
            items
                .into_iter()
                .map(|u| agent_forge::tools::think::UnknownItem {
                    description: u.description,
                    blocking: u.blocking,
                    resolution_hint: u.resolution_hint,
                })
                .collect()
        });
    let input = agent_forge::tools::think::DeclareUnknownsInput { unknowns };
    let output =
        agent_forge::tools::think::declare_unknowns(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// `POST /forge/comment-check` -- scan a source file for declarations that
/// lack a preceding comment and return a coverage report.
///
/// Supply either `path` (server-visible file path within `KLEOS_FORGE_FS_ROOTS`)
/// or `content` (raw source text; written to a temp file). When `content` is
/// supplied, `extension` is used as the temp file suffix so the scanner can
/// detect the language (e.g. `"rs"`, `"ts"`).
async fn comment_check_handler(
    Auth(_auth): Auth,
    Json(body): Json<FileOrContentBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    let (file_path, _tmp) = resolve_path_or_content(body.path, body.content, body.extension)?;
    let input = agent_forge::tools::comments::CommentCheckInput {
        file_path: Some(file_path),
    };
    let output =
        agent_forge::tools::comments::comment_check(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// `POST /forge/challenge-code` -- build an adversarial review prompt for a
/// source file, embedding a mechanical comment-coverage report.
///
/// Supply either `path` (server-visible within `KLEOS_FORGE_FS_ROOTS`) or
/// `content` (written to a temp file; use `extension` to set the language).
async fn challenge_code_handler(
    Auth(_auth): Auth,
    Json(body): Json<FileOrContentBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    let (file_path, _tmp) = resolve_path_or_content(body.path, body.content, body.extension)?;
    let input = agent_forge::tools::verify::ChallengeCodeInput {
        file_path: Some(file_path),
        // `focus_areas` is not part of `FileOrContentBody`; use tool default.
        focus_areas: None,
    };
    let output =
        agent_forge::tools::verify::challenge_code(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// `POST /forge/repo-map` -- walk a directory tree, extract named symbols, and
/// return a ranked symbol map within a configurable token budget.
///
/// `path` must resolve within `KLEOS_FORGE_FS_ROOTS`; directory walk needs a
/// real server-visible path. Inline content is not supported for this tool.
async fn repo_map_handler(
    Auth(_auth): Auth,
    Json(body): Json<RepoMapBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    let canonical = fsroots::resolve_within_roots(&body.path).ok_or_else(|| {
        AppError(EngError::InvalidInput(
            "repo-map requires a path under KLEOS_FORGE_FS_ROOTS (mount the tree or run locally)"
                .into(),
        ))
    })?;
    let input = agent_forge::tools::ast::repo_map::RepoMapInput {
        path: Some(canonical.to_string_lossy().into_owned()),
        focus: body.focus,
        max_tokens: body.max_tokens,
    };
    let output =
        agent_forge::tools::ast::repo_map::repo_map(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// `POST /forge/search-code` -- walk a directory tree and return symbols whose
/// names contain the supplied query string (case-insensitive).
///
/// `path` must resolve within `KLEOS_FORGE_FS_ROOTS`; directory walk needs a
/// real server-visible path. Inline content is not supported for this tool.
async fn search_code_handler(
    Auth(_auth): Auth,
    Json(body): Json<SearchCodeBody>,
) -> Result<Json<Value>, AppError> {
    let (db, _db_file) = throwaway_db()?;
    let canonical = fsroots::resolve_within_roots(&body.path).ok_or_else(|| {
        AppError(EngError::InvalidInput(
            "search-code requires a path under KLEOS_FORGE_FS_ROOTS (mount the tree or run locally)"
                .into(),
        ))
    })?;
    let input = agent_forge::tools::ast::search::SearchCodeInput {
        query: body.query,
        path: Some(canonical.to_string_lossy().into_owned()),
        symbol_type: body.symbol_type,
        limit: body.limit,
    };
    let output =
        agent_forge::tools::ast::search::search_code(&db, input).map_err(tool_error_to_app)?;
    Ok(Json(serde_json::json!({
        "success": output.success,
        "message": output.message,
        "data": output.data,
    })))
}

/// Create a new task spec, enforce minimum criteria counts, and write a
/// scratchpad ledger entry for each file in `files_to_touch` so the `ke`
/// edit-gate can verify coverage.
///
/// Returns 201 Created with the new spec ID on success.
async fn spec_task_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SpecTaskBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.effective_user_id();

    let result = kleos_lib::forge::spec::spec_task(
        &db,
        user_id,
        body.session_id.as_deref(),
        body.task_description,
        body.task_type,
        body.acceptance_criteria,
        body.interface_contract,
        body.edge_cases,
        body.files_to_touch.clone(),
        body.dependencies,
    )
    .await?;

    // Write a scratchpad ledger entry for each declared file so `ke` can find
    // it via the spec-task namespace. See the module-level doc for the column
    // mapping.
    //
    // Key format: `<session_id>:<absolute_file_path>` -- exactly what `ke`
    // assembles as `format!("{}:{}", session_id, path)` before calling
    // `/scratchpad/get?namespace=spec-task&key=<key>`.
    //
    // TTL: 1440 minutes (24 h) -- long enough to outlast a typical coding
    // session. The entry is invalidated naturally when it expires or is
    // overwritten by a follow-up spec.
    if let Some(ref files) = body.files_to_touch {
        if let Some(ref sid) = body.session_id {
            let spec_id = result
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            for file in files {
                let ledger_key = format!("{}:{}", sid, file);
                // Ignore ledger write errors: the spec is already committed.
                // A failure here degrades the gate to ServerUnavailable (which
                // ke treats as fail-closed by default), not to a false allow.
                let _ = kleos_lib::scratchpad::upsert_entry(
                    &db,
                    user_id,
                    sid,
                    // Must match the namespace `ke` queries:
                    // `GET /scratchpad/get?namespace=spec-task&...`
                    "spec-task",
                    "",
                    &ledger_key,
                    spec_id,
                    1440,
                )
                .await;
            }
        }
    }

    Ok((StatusCode::CREATED, Json(result)))
}

/// Transition a spec to a new lifecycle status.
async fn update_spec_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<UpdateSpecBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result =
        kleos_lib::forge::spec::update_spec(&db, user_id, body.spec_id, body.status, body.note)
            .await?;
    Ok(Json(result))
}

/// List specs for the authenticated user, optionally filtered by status.
async fn list_specs_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<ListSpecsQuery>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::spec::list_specs(&db, user_id, q.status, q.limit).await?;
    Ok(Json(result))
}

/// Fetch one full spec by ID including related sub-records.
async fn get_spec_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::spec::get_spec(&db, user_id, id).await?;
    Ok(Json(result))
}

/// Record a new hypothesis before touching code in response to a bug.
///
/// Returns 201 Created with the new hypothesis ID on success.
async fn log_hypothesis_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<LogHypothesisBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::hypothesis::log_hypothesis(
        &db,
        user_id,
        body.session_id.as_deref(),
        body.bug_description,
        body.hypothesis,
        body.confidence,
        body.spec_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// Record the outcome of an existing hypothesis.
async fn log_outcome_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<LogOutcomeBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::hypothesis::log_outcome(
        &db,
        user_id,
        body.hypothesis_id,
        body.outcome,
        body.notes,
    )
    .await?;
    Ok(Json(result))
}

/// Search past hypotheses by keyword across bug description and hypothesis text.
async fn recall_errors_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<RecallErrorsQuery>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result =
        kleos_lib::forge::hypothesis::recall_errors(&db, user_id, q.query, q.limit).await?;
    Ok(Json(result))
}

/// Store two or more named design alternatives and return a structured
/// comparison prompt suitable for agent reasoning.
///
/// Returns 201 Created with all stored approach IDs on success.
async fn consider_approaches_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ConsiderApproachesBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::approaches::consider_approaches(
        &db,
        user_id,
        body.spec_id,
        body.problem,
        body.approaches,
        body.chosen_index,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// Record the result of a client-side verification run.
///
/// Command execution stays client-side; only the result is persisted here.
/// Returns 201 Created with the new verification record ID.
async fn verify_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<VerifyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::verify::record_verification(
        &db,
        user_id,
        body.spec_id,
        body.command,
        body.exit_code,
        body.success,
        body.duration_ms,
        body.criteria_index,
        body.stdout,
        body.stderr,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// Persist a mid-session discovery to `forge_session_learns`.
///
/// Returns 201 Created with the new learning ID.
async fn session_learn_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SessionLearnBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::session::session_learn(
        &db,
        user_id,
        body.discovery,
        body.context,
        body.tags,
        body.spec_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// Search `forge_session_learns` by keyword in the discovery text.
async fn session_recall_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<SessionRecallQuery>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let result = kleos_lib::forge::session::session_recall(&db, user_id, q.query, q.limit).await?;
    Ok(Json(result))
}
