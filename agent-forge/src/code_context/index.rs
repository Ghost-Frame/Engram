//! SQLite persistence, incremental refresh, ranking, and relation queries.

use super::extract::{extract_file, ExtractedFile};
use super::model::{
    CodeRelation, ContextPack, ContextQuery, ContextSnippet, IndexedSymbol, RefreshReport,
    RelationDirection, RelationQuery,
};
use crate::treesitter::{is_supported_extension, parser::parse_file};
use ignore::WalkBuilder;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use thiserror::Error;

/// Current on-disk schema version for the independent code index.
const SCHEMA_VERSION: i64 = 1;
/// Maximum supported files accepted from one repository.
const MAX_REPOSITORY_FILES: usize = 20_000;
/// Maximum source file size accepted by the parser.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Maximum snippets selected from one source file.
const MAX_SNIPPETS_PER_FILE: usize = 2;
/// Minimum deterministic score required to return automatic context.
const MIN_CONTEXT_SCORE: f64 = 4.0;

/// Failures produced by local code indexing and retrieval.
#[derive(Debug, Error)]
pub enum IndexError {
    /// The request does not identify a usable repository or selector.
    #[error("invalid code-index request: {0}")]
    InvalidInput(String),
    /// A local filesystem operation failed.
    #[error("code-index I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A local SQLite operation failed.
    #[error("code-index database failed: {0}")]
    Database(#[from] rusqlite::Error),
}

/// A lightweight handle to the dedicated local code-index database.
#[derive(Clone, Debug)]
pub struct CodeIndex {
    /// SQLite database path; connections are opened per operation.
    database_path: PathBuf,
}

/// One indexed file row used to detect incremental changes.
#[derive(Clone, Debug)]
struct IndexedFileState {
    /// File database identifier.
    id: i64,
    /// Last indexed SHA-256 content hash.
    content_hash: String,
}

/// Parsed update held outside the database transaction.
#[derive(Clone, Debug)]
struct ChangedFile {
    /// Repository-relative path.
    relative_path: String,
    /// Stable language label.
    language: String,
    /// File size in bytes.
    size: i64,
    /// Best-effort modification time in nanoseconds since the Unix epoch.
    modified_ns: i64,
    /// SHA-256 content hash.
    content_hash: String,
    /// Extracted semantic content.
    extracted: ExtractedFile,
}

/// Internal retrieval row before ranking and budget selection.
#[derive(Clone, Debug)]
struct Candidate {
    /// Unit database identifier.
    id: i64,
    /// Repository-relative source path.
    path: String,
    /// Parser language label.
    language: String,
    /// Indexed file content hash.
    content_hash: String,
    /// Semantic unit kind.
    kind: String,
    /// Extracted symbol name.
    name: String,
    /// Signature-like first source line.
    signature: String,
    /// Adjacent leading comments.
    docs: String,
    /// Exact bounded source body.
    body: String,
    /// One-based first line.
    start_line: usize,
    /// One-based final line.
    end_line: usize,
    /// Deterministic relevance score.
    score: f64,
    /// Human-readable ranking signals.
    signals: Vec<String>,
}

/// Open, initialize, and query the persistent code index.
impl CodeIndex {
    /// Open the default index at `~/.agent-forge/code-index.db`.
    pub fn open_default() -> Result<Self, IndexError> {
        let home = dirs::home_dir().ok_or_else(|| {
            IndexError::InvalidInput("cannot resolve the home directory".to_string())
        })?;
        Self::open(home.join(".agent-forge/code-index.db"))
    }

    /// Open an index at an explicit path and apply its idempotent schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IndexError> {
        let database_path = path.as_ref().to_path_buf();
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let index = Self { database_path };
        let connection = index.connection()?;
        initialize_schema(&connection)?;
        Ok(index)
    }

    /// Return the database path for diagnostics and tests.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Incrementally refresh every supported file beneath a Git repository.
    pub fn refresh(&self, path: impl AsRef<Path>) -> Result<RefreshReport, IndexError> {
        let repository_root = resolve_repository_root(path.as_ref())?;
        let root_text = repository_root.to_string_lossy().to_string();
        let mut connection = self.connection()?;
        let (repository_id, prior_revision) = ensure_repository(&connection, &root_text)?;
        let prior_files = load_file_states(&connection, repository_id)?;
        let paths = supported_paths(&repository_root)?;
        let mut seen = HashSet::new();
        let mut changed = Vec::new();
        let mut failures = 0usize;

        for source_path in &paths {
            let relative_path = relative_path(&repository_root, source_path)?;
            seen.insert(relative_path.clone());
            let bytes = std::fs::read(source_path)?;
            let content_hash = hash_bytes(&bytes);
            if prior_files
                .get(&relative_path)
                .is_some_and(|state| state.content_hash == content_hash)
            {
                continue;
            }
            let Some(parsed) = parse_file(source_path) else {
                failures += 1;
                continue;
            };
            let metadata = std::fs::metadata(source_path)?;
            changed.push(ChangedFile {
                relative_path,
                language: language_name(source_path),
                size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                modified_ns: modified_ns(&metadata),
                content_hash,
                extracted: extract_file(&parsed),
            });
        }

        let removed: Vec<IndexedFileState> = prior_files
            .iter()
            .filter(|(path, _)| !seen.contains(*path))
            .map(|(_, state)| state.clone())
            .collect();
        let files_indexed = changed.len();
        let files_removed = removed.len();
        let files_unchanged = paths
            .len()
            .saturating_sub(files_indexed)
            .saturating_sub(failures);
        let mut revision = prior_revision;

        if files_indexed > 0 || files_removed > 0 {
            revision += 1;
            let transaction = connection.transaction()?;
            for state in &removed {
                delete_file(&transaction, state.id)?;
            }
            for file in changed {
                replace_file(&transaction, repository_id, file)?;
            }
            resolve_edge_targets(&transaction, repository_id)?;
            transaction.execute(
                "UPDATE repositories SET index_revision = ?1, last_indexed_at = unixepoch() WHERE id = ?2",
                params![revision, repository_id],
            )?;
            transaction.commit()?;
        }

        Ok(RefreshReport {
            repo_root: root_text,
            index_revision: revision,
            files_seen: paths.len(),
            files_indexed,
            files_removed,
            files_unchanged,
            parse_failures: failures,
        })
    }

    /// Refresh only if needed, then return a high-confidence token-bounded context pack.
    pub fn context(&self, query: &ContextQuery) -> Result<ContextPack, IndexError> {
        let refresh = self.refresh(&query.repo_root)?;
        let terms = query_terms(&query.query);
        if terms.is_empty() || query.max_tokens == 0 {
            return Ok(ContextPack {
                index_revision: refresh.index_revision,
                ..ContextPack::default()
            });
        }

        let repository_root = PathBuf::from(&refresh.repo_root);
        let connection = self.connection()?;
        let repository_id = repository_id(&connection, &refresh.repo_root)?;
        let mut candidates = search_candidates(&connection, repository_id, &terms)?;
        for candidate in &mut candidates {
            score_candidate(
                candidate,
                &query.query,
                &terms,
                &query.focus_paths,
                &query.recent_paths,
            );
        }
        add_one_hop_candidates(&connection, repository_id, &mut candidates)?;
        candidates.sort_by(compare_candidates);
        candidates.dedup_by_key(|candidate| candidate.id);

        let mut snippets = Vec::new();
        let mut per_file = HashMap::<String, usize>::new();
        let mut freshness = HashMap::<String, bool>::new();
        let mut estimated_tokens = 0usize;
        let mut stale_skipped = 0usize;
        let mut truncated = false;

        for candidate in candidates {
            if candidate.score < MIN_CONTEXT_SCORE {
                continue;
            }
            let fresh = match freshness.get(&candidate.path) {
                Some(fresh) => *fresh,
                None => {
                    let current = hash_path(&repository_root.join(&candidate.path));
                    let fresh = current.as_deref() == Some(candidate.content_hash.as_str());
                    freshness.insert(candidate.path.clone(), fresh);
                    fresh
                }
            };
            if !fresh {
                stale_skipped += 1;
                continue;
            }
            if per_file.get(&candidate.path).copied().unwrap_or(0) >= MAX_SNIPPETS_PER_FILE
                || overlaps_selected(&snippets, &candidate)
            {
                continue;
            }
            let token_cost = estimate_tokens(&candidate.body).saturating_add(24);
            if estimated_tokens.saturating_add(token_cost) > query.max_tokens {
                truncated = true;
                continue;
            }
            estimated_tokens += token_cost;
            *per_file.entry(candidate.path.clone()).or_default() += 1;
            snippets.push(ContextSnippet {
                path: candidate.path,
                start_line: candidate.start_line,
                end_line: candidate.end_line,
                language: candidate.language,
                kind: candidate.kind,
                symbol: candidate.name,
                reason: candidate.signals.join(", "),
                score: candidate.score,
                text: candidate.body,
                content_hash: candidate.content_hash,
            });
        }

        Ok(ContextPack {
            snippets,
            estimated_tokens,
            truncated,
            index_revision: refresh.index_revision,
            stale_skipped,
        })
    }

    /// Refresh the repository and return syntax-derived relations for one selector.
    pub fn relations(&self, query: &RelationQuery) -> Result<Vec<CodeRelation>, IndexError> {
        let has_symbol = query
            .symbol
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_path = query
            .path
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        if has_symbol == has_path {
            return Err(IndexError::InvalidInput(
                "provide exactly one of symbol or path".to_string(),
            ));
        }
        let refresh = self.refresh(&query.repo_root)?;
        let connection = self.connection()?;
        let repository_id = repository_id(&connection, &refresh.repo_root)?;
        let mut statement = connection.prepare(
            "SELECT e.kind, source.name, e.target_name, f.path, e.start_line, e.end_line, e.confidence
             FROM edges e
             JOIN units source ON source.id = e.source_unit_id
             JOIN files f ON f.id = source.file_id
             WHERE e.repo_id = ?1
             ORDER BY f.path, e.start_line, e.kind, e.target_name",
        )?;
        let rows = statement.query_map([repository_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })?;
        let mut output = Vec::new();
        let limit = query.limit.clamp(1, 200);

        for row in rows {
            let (kind, source, target, path, start_line, end_line, confidence) = row?;
            if !query.kinds.is_empty() && !query.kinds.iter().any(|allowed| allowed == &kind) {
                continue;
            }
            if let Some(selected_path) = &query.path {
                if !path_matches(&path, selected_path) {
                    continue;
                }
                output.push(CodeRelation {
                    kind,
                    direction: RelationDirection::Outgoing,
                    source,
                    target,
                    path,
                    start_line: usize::try_from(start_line).unwrap_or(1),
                    end_line: usize::try_from(end_line).unwrap_or(1),
                    confidence,
                });
            } else if let Some(selected_symbol) = &query.symbol {
                let outgoing = symbol_matches(&source, selected_symbol);
                let incoming = symbol_matches(&target, selected_symbol);
                if outgoing
                    && matches!(
                        query.direction,
                        RelationDirection::Outgoing | RelationDirection::Both
                    )
                {
                    output.push(CodeRelation {
                        kind: kind.clone(),
                        direction: RelationDirection::Outgoing,
                        source: source.clone(),
                        target: target.clone(),
                        path: path.clone(),
                        start_line: usize::try_from(start_line).unwrap_or(1),
                        end_line: usize::try_from(end_line).unwrap_or(1),
                        confidence,
                    });
                }
                if incoming
                    && matches!(
                        query.direction,
                        RelationDirection::Incoming | RelationDirection::Both
                    )
                {
                    output.push(CodeRelation {
                        kind,
                        direction: RelationDirection::Incoming,
                        source,
                        target,
                        path,
                        start_line: usize::try_from(start_line).unwrap_or(1),
                        end_line: usize::try_from(end_line).unwrap_or(1),
                        confidence,
                    });
                }
            }
            if output.len() >= limit {
                break;
            }
        }
        Ok(output)
    }

    /// Refresh and return compact symbols for repository-map compatibility.
    pub fn repository_symbols(
        &self,
        repo_root: impl AsRef<Path>,
        focus_paths: &[String],
        max_tokens: usize,
    ) -> Result<(RefreshReport, Vec<IndexedSymbol>), IndexError> {
        let refresh = self.refresh(repo_root)?;
        let connection = self.connection()?;
        let repository_id = repository_id(&connection, &refresh.repo_root)?;
        let mut symbols = all_symbols(&connection, repository_id)?;
        symbols.sort_by(|left, right| {
            let left_focus = focus_paths
                .iter()
                .any(|focus| path_matches(&left.path, focus));
            let right_focus = focus_paths
                .iter()
                .any(|focus| path_matches(&right.path, focus));
            right_focus
                .cmp(&left_focus)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        let mut used = 0usize;
        symbols.retain(|symbol| {
            let cost = estimate_tokens(&format!(
                "{} {} {}:{}",
                symbol.kind, symbol.name, symbol.path, symbol.line
            ));
            if used.saturating_add(cost) > max_tokens {
                false
            } else {
                used += cost;
                true
            }
        });
        Ok((refresh, symbols))
    }

    /// Refresh and search indexed symbol names for CLI compatibility.
    pub fn search_symbols(
        &self,
        repo_root: impl AsRef<Path>,
        query: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IndexedSymbol>, IndexError> {
        let refresh = self.refresh(repo_root)?;
        let connection = self.connection()?;
        let repository_id = repository_id(&connection, &refresh.repo_root)?;
        let needle = query.trim().to_ascii_lowercase();
        let mut symbols = all_symbols(&connection, repository_id)?;
        symbols.retain(|symbol| {
            symbol.name.to_ascii_lowercase().contains(&needle)
                && kind.is_none_or(|selected| selected == "any" || selected == symbol.kind)
        });
        symbols.sort_by(|left, right| {
            let left_exact = left.name.eq_ignore_ascii_case(query);
            let right_exact = right.name.eq_ignore_ascii_case(query);
            right_exact
                .cmp(&left_exact)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        symbols.truncate(limit.clamp(1, 500));
        Ok(symbols)
    }

    /// Open a configured connection with bounded lock waiting and foreign keys.
    fn connection(&self) -> Result<Connection, IndexError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(connection)
    }
}

/// Resolve the nearest ancestor containing a Git metadata directory or file.
pub fn resolve_repository_root(path: &Path) -> Result<PathBuf, IndexError> {
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let canonical = start.canonicalize().map_err(|error| {
        IndexError::InvalidInput(format!("cannot resolve {}: {error}", start.display()))
    })?;
    canonical
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            IndexError::InvalidInput(format!("{} is not inside a Git repository", path.display()))
        })
}

/// Create the versioned SQLite schema and FTS5 index.
fn initialize_schema(connection: &Connection) -> Result<(), IndexError> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS code_index_metadata (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS repositories (
             id INTEGER PRIMARY KEY,
             root TEXT NOT NULL UNIQUE,
             index_revision INTEGER NOT NULL DEFAULT 0,
             last_indexed_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS files (
             id INTEGER PRIMARY KEY,
             repo_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
             path TEXT NOT NULL,
             language TEXT NOT NULL,
             size INTEGER NOT NULL,
             modified_ns INTEGER NOT NULL,
             content_hash TEXT NOT NULL,
             UNIQUE(repo_id, path)
         );
         CREATE TABLE IF NOT EXISTS units (
             id INTEGER PRIMARY KEY,
             file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
             kind TEXT NOT NULL,
             name TEXT NOT NULL,
             signature TEXT NOT NULL,
             docs TEXT NOT NULL,
             body TEXT NOT NULL,
             start_line INTEGER NOT NULL,
             end_line INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS unit_fts USING fts5(
             path, name, signature, docs, body, tokenize='unicode61'
         );
         CREATE TABLE IF NOT EXISTS edges (
             id INTEGER PRIMARY KEY,
             repo_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
             source_unit_id INTEGER NOT NULL REFERENCES units(id) ON DELETE CASCADE,
             target_unit_id INTEGER REFERENCES units(id) ON DELETE SET NULL,
             kind TEXT NOT NULL,
             target_name TEXT NOT NULL,
             start_line INTEGER NOT NULL,
             end_line INTEGER NOT NULL,
             confidence REAL NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_files_repo_path ON files(repo_id, path);
         CREATE INDEX IF NOT EXISTS idx_units_file_name ON units(file_id, name);
         CREATE INDEX IF NOT EXISTS idx_edges_repo_source ON edges(repo_id, source_unit_id);
         CREATE INDEX IF NOT EXISTS idx_edges_repo_target_name ON edges(repo_id, target_name);",
    )?;
    connection.execute(
        "INSERT INTO code_index_metadata(key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// Insert or fetch one canonical repository record.
fn ensure_repository(connection: &Connection, root: &str) -> Result<(i64, i64), IndexError> {
    connection.execute(
        "INSERT INTO repositories(root) VALUES (?1) ON CONFLICT(root) DO NOTHING",
        [root],
    )?;
    connection
        .query_row(
            "SELECT id, index_revision FROM repositories WHERE root = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(IndexError::from)
}

/// Fetch a repository identifier after a refresh has guaranteed its existence.
fn repository_id(connection: &Connection, root: &str) -> Result<i64, IndexError> {
    connection
        .query_row(
            "SELECT id FROM repositories WHERE root = ?1",
            [root],
            |row| row.get(0),
        )
        .map_err(IndexError::from)
}

/// Load indexed file hashes and identifiers for incremental comparison.
fn load_file_states(
    connection: &Connection,
    repository_id: i64,
) -> Result<HashMap<String, IndexedFileState>, IndexError> {
    let mut statement =
        connection.prepare("SELECT id, path, content_hash FROM files WHERE repo_id = ?1")?;
    let rows = statement.query_map([repository_id], |row| {
        Ok((
            row.get::<_, String>(1)?,
            IndexedFileState {
                id: row.get(0)?,
                content_hash: row.get(2)?,
            },
        ))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(IndexError::from)
}

/// Walk supported files while respecting Git ignore and hidden-file rules.
fn supported_paths(repository_root: &Path) -> Result<Vec<PathBuf>, IndexError> {
    let mut paths = Vec::new();
    for entry in WalkBuilder::new(repository_root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .follow_links(false)
        .build()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() || std::fs::metadata(path)?.len() > MAX_FILE_BYTES {
            continue;
        }
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(is_supported_extension);
        if supported {
            paths.push(path.to_path_buf());
            if paths.len() > MAX_REPOSITORY_FILES {
                return Err(IndexError::InvalidInput(format!(
                    "repository exceeds the {MAX_REPOSITORY_FILES}-file safety limit"
                )));
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Convert an absolute source path to a slash-separated repository-relative path.
fn relative_path(repository_root: &Path, source_path: &Path) -> Result<String, IndexError> {
    source_path
        .strip_prefix(repository_root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

/// Compute a file's modification timestamp in nanoseconds.
fn modified_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
        .unwrap_or(0)
}

/// Return a stable SHA-256 hexadecimal digest.
fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Hash a current source path, returning none when it is inaccessible.
fn hash_path(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| hash_bytes(&bytes))
}

/// Return a compact parser language label for a supported path.
fn language_name(path: &Path) -> String {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "json" => "json",
        other => other,
    }
    .to_string()
}

/// Delete one file and its manually maintained FTS rows.
fn delete_file(transaction: &Transaction<'_>, file_id: i64) -> Result<(), IndexError> {
    let mut statement = transaction.prepare("SELECT id FROM units WHERE file_id = ?1")?;
    let unit_ids = statement
        .query_map([file_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for unit_id in unit_ids {
        transaction.execute("DELETE FROM unit_fts WHERE rowid = ?1", [unit_id])?;
    }
    transaction.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
    Ok(())
}

/// Replace one changed file, its semantic units, FTS rows, and edges atomically.
fn replace_file(
    transaction: &Transaction<'_>,
    repository_id: i64,
    file: ChangedFile,
) -> Result<(), IndexError> {
    let prior_id = transaction
        .query_row(
            "SELECT id FROM files WHERE repo_id = ?1 AND path = ?2",
            params![repository_id, file.relative_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(file_id) = prior_id {
        delete_file(transaction, file_id)?;
    }
    transaction.execute(
        "INSERT INTO files(repo_id, path, language, size, modified_ns, content_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            repository_id,
            file.relative_path,
            file.language,
            file.size,
            file.modified_ns,
            file.content_hash
        ],
    )?;
    let file_id = transaction.last_insert_rowid();
    let mut unit_ids = Vec::with_capacity(file.extracted.units.len());
    for unit in file.extracted.units {
        transaction.execute(
            "INSERT INTO units(file_id, kind, name, signature, docs, body, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                file_id,
                unit.kind,
                unit.name,
                unit.signature,
                unit.docs,
                unit.body,
                unit.start_line as i64,
                unit.end_line as i64
            ],
        )?;
        let unit_id = transaction.last_insert_rowid();
        unit_ids.push(unit_id);
        transaction.execute(
            "INSERT INTO unit_fts(rowid, path, name, signature, docs, body)
             SELECT ?1, path, ?2, ?3, ?4, ?5 FROM files WHERE id = ?6",
            params![
                unit_id,
                unit.name,
                unit.signature,
                unit.docs,
                unit.body,
                file_id
            ],
        )?;
    }
    for edge in file.extracted.edges {
        let Some(source_unit_id) = unit_ids.get(edge.source_index) else {
            continue;
        };
        transaction.execute(
            "INSERT INTO edges(repo_id, source_unit_id, kind, target_name, start_line, end_line, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![repository_id, source_unit_id, edge.kind, edge.target_name, edge.start_line as i64, edge.end_line as i64, edge.confidence],
        )?;
    }
    Ok(())
}

/// Resolve edge targets to exact repository symbols when a unique best match exists.
fn resolve_edge_targets(
    transaction: &Transaction<'_>,
    repository_id: i64,
) -> Result<(), IndexError> {
    transaction.execute(
        "UPDATE edges
         SET target_unit_id = (
             SELECT candidate.id
             FROM units candidate
             JOIN files candidate_file ON candidate_file.id = candidate.file_id
             WHERE candidate_file.repo_id = edges.repo_id
               AND lower(candidate.name) = lower(edges.target_name)
             ORDER BY candidate_file.path, candidate.start_line
             LIMIT 1
         )
         WHERE repo_id = ?1",
        [repository_id],
    )?;
    Ok(())
}

/// Tokenize a task into safe FTS terms while removing weak conversational words.
fn query_terms(query: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "about", "after", "before", "build", "change", "code", "could", "from", "have", "into",
        "make", "please", "should", "that", "their", "then", "this", "want", "with", "would",
    ];
    let mut terms: Vec<String> = query
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 2 && !STOPWORDS.contains(&term.as_str()))
        .collect();
    terms.sort();
    terms.dedup();
    terms.truncate(12);
    terms
}

/// Load broad FTS candidates for deterministic re-ranking.
fn search_candidates(
    connection: &Connection,
    repository_id: i64,
    terms: &[String],
) -> Result<Vec<Candidate>, IndexError> {
    let fts_query = terms
        .iter()
        .map(|term| format!("{term}*"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = connection.prepare(
        "SELECT u.id, f.path, f.language, f.content_hash, u.kind, u.name, u.signature,
                u.docs, u.body, u.start_line, u.end_line
         FROM unit_fts
         JOIN units u ON u.id = unit_fts.rowid
         JOIN files f ON f.id = u.file_id
         WHERE f.repo_id = ?1 AND unit_fts MATCH ?2
         ORDER BY bm25(unit_fts), f.path, u.start_line
         LIMIT 160",
    )?;
    let rows = statement.query_map(params![repository_id, fts_query], candidate_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(IndexError::from)
}

/// Decode one joined unit row into an unscored candidate.
fn candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        id: row.get(0)?,
        path: row.get(1)?,
        language: row.get(2)?,
        content_hash: row.get(3)?,
        kind: row.get(4)?,
        name: row.get(5)?,
        signature: row.get(6)?,
        docs: row.get(7)?,
        body: row.get(8)?,
        start_line: usize::try_from(row.get::<_, i64>(9)?).unwrap_or(1),
        end_line: usize::try_from(row.get::<_, i64>(10)?).unwrap_or(1),
        score: 0.0,
        signals: Vec::new(),
    })
}

/// Apply exact symbol, lexical, path, focus, and recency ranking signals.
fn score_candidate(
    candidate: &mut Candidate,
    raw_query: &str,
    terms: &[String],
    focus_paths: &[String],
    recent_paths: &[String],
) {
    let query = raw_query.trim().to_ascii_lowercase();
    let name = candidate.name.to_ascii_lowercase();
    let path = candidate.path.to_ascii_lowercase();
    let signature = candidate.signature.to_ascii_lowercase();
    let docs = candidate.docs.to_ascii_lowercase();
    let body = candidate.body.to_ascii_lowercase();

    if name == query || terms.iter().any(|term| term == &name) {
        candidate.score += 30.0;
        candidate.signals.push("exact symbol".to_string());
    }
    let mut lexical = 0.0;
    for term in terms {
        if name.contains(term) {
            lexical += 7.0;
        }
        if path.contains(term) {
            lexical += 4.0;
        }
        if signature.contains(term) {
            lexical += 3.0;
        }
        if docs.contains(term) {
            lexical += 2.0;
        }
        if body.contains(term) {
            lexical += 1.0;
        }
    }
    if lexical > 0.0 {
        candidate.score += lexical;
        candidate.signals.push("lexical match".to_string());
    }
    if focus_paths
        .iter()
        .any(|focus| path_matches(&candidate.path, focus))
    {
        candidate.score += 12.0;
        candidate.signals.push("focus path".to_string());
    }
    if recent_paths
        .iter()
        .any(|recent| path_matches(&candidate.path, recent))
    {
        candidate.score += 8.0;
        candidate.signals.push("recent path".to_string());
    }
}

/// Add one conservative relation hop from the strongest direct candidates.
fn add_one_hop_candidates(
    connection: &Connection,
    repository_id: i64,
    candidates: &mut Vec<Candidate>,
) -> Result<(), IndexError> {
    candidates.sort_by(compare_candidates);
    let seeds: Vec<Candidate> = candidates.iter().take(8).cloned().collect();
    let mut known: HashSet<i64> = candidates.iter().map(|candidate| candidate.id).collect();
    for seed in seeds {
        let mut statement = connection.prepare(
            "SELECT target.id, target_file.path, target_file.language, target_file.content_hash,
                    target.kind, target.name, target.signature, target.docs, target.body,
                    target.start_line, target.end_line, e.kind
             FROM edges e
             JOIN units target ON target.id = e.target_unit_id
             JOIN files target_file ON target_file.id = target.file_id
             WHERE e.repo_id = ?1 AND e.source_unit_id = ?2
             ORDER BY e.confidence DESC, target_file.path, target.start_line
             LIMIT 12",
        )?;
        let rows = statement.query_map(params![repository_id, seed.id], |row| {
            let mut candidate = candidate_from_row(row)?;
            let relation: String = row.get(11)?;
            candidate.score = seed.score * 0.55;
            candidate
                .signals
                .push(format!("one-hop {relation} from {}", seed.name));
            Ok(candidate)
        })?;
        for candidate in rows {
            let candidate = candidate?;
            if known.insert(candidate.id) {
                candidates.push(candidate);
            }
        }
    }
    Ok(())
}

/// Sort candidates deterministically by score, path, line, and symbol.
fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.start_line.cmp(&right.start_line))
        .then_with(|| left.name.cmp(&right.name))
}

/// Return whether a candidate overlaps an already selected range in the same file.
fn overlaps_selected(snippets: &[ContextSnippet], candidate: &Candidate) -> bool {
    snippets.iter().any(|snippet| {
        snippet.path == candidate.path
            && candidate.start_line <= snippet.end_line
            && snippet.start_line <= candidate.end_line
    })
}

/// Estimate tokens conservatively from Unicode scalar count.
fn estimate_tokens(value: &str) -> usize {
    value.chars().count().div_ceil(4)
}

/// Compare a stored path against an absolute, relative, or suffix selector.
fn path_matches(path: &str, selected: &str) -> bool {
    let normalized = selected.replace('\\', "/");
    path == normalized || path.ends_with(&normalized) || normalized.ends_with(path)
}

/// Compare symbols case-insensitively by exact name or qualified suffix.
fn symbol_matches(symbol: &str, selected: &str) -> bool {
    let symbol = symbol.to_ascii_lowercase();
    let selected = selected.trim().to_ascii_lowercase();
    symbol == selected
        || symbol.ends_with(&format!("::{selected}"))
        || symbol.ends_with(&format!(".{selected}"))
}

/// Load every indexed declaration in deterministic source order.
fn all_symbols(
    connection: &Connection,
    repository_id: i64,
) -> Result<Vec<IndexedSymbol>, IndexError> {
    let mut statement = connection.prepare(
        "SELECT f.path, u.start_line, u.kind, u.name, u.signature
         FROM units u JOIN files f ON f.id = u.file_id
         WHERE f.repo_id = ?1 AND u.kind != 'file'
         ORDER BY f.path, u.start_line, u.name",
    )?;
    let rows = statement.query_map([repository_id], |row| {
        Ok(IndexedSymbol {
            path: row.get(0)?,
            line: usize::try_from(row.get::<_, i64>(1)?).unwrap_or(1),
            kind: row.get(2)?,
            name: row.get(3)?,
            signature: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(IndexError::from)
}

#[cfg(test)]
/// Persistent index, freshness, relation, and budget tests.
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Create a minimal Git-shaped repository and independent index path.
    fn fixture() -> (tempfile::TempDir, PathBuf, CodeIndex) {
        let directory = tempdir().unwrap();
        let repository = directory.path().join("repo");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        std::fs::create_dir_all(repository.join("src")).unwrap();
        let index = CodeIndex::open(directory.path().join("index.db")).unwrap();
        (directory, repository, index)
    }

    /// Incremental refresh reparses changed files and removes deleted files.
    #[test]
    fn refreshes_by_content_hash() {
        let (_directory, repository, index) = fixture();
        let source = repository.join("src/lib.rs");
        std::fs::write(&source, "pub fn alpha() {}\n").unwrap();
        let first = index.refresh(&repository).unwrap();
        assert_eq!(first.files_indexed, 1);
        let unchanged = index.refresh(&repository).unwrap();
        assert_eq!(unchanged.files_unchanged, 1);
        assert_eq!(unchanged.index_revision, first.index_revision);
        std::fs::write(&source, "pub fn beta() {}\n").unwrap();
        let changed = index.refresh(&repository).unwrap();
        assert_eq!(changed.files_indexed, 1);
        assert!(changed.index_revision > first.index_revision);
        std::fs::remove_file(source).unwrap();
        let removed = index.refresh(&repository).unwrap();
        assert_eq!(removed.files_removed, 1);
    }

    /// Context retrieval ranks an exact symbol and remains inside its token cap.
    #[test]
    fn returns_bounded_exact_context() {
        let (_directory, repository, index) = fixture();
        std::fs::write(
            repository.join("src/lib.rs"),
            "pub fn calculate_invoice_total() -> u64 { 42 }\npub fn unrelated() {}\n",
        )
        .unwrap();
        let pack = index
            .context(&ContextQuery {
                repo_root: repository.to_string_lossy().to_string(),
                query: "calculate_invoice_total".to_string(),
                max_tokens: 200,
                focus_paths: Vec::new(),
                recent_paths: Vec::new(),
            })
            .unwrap();
        assert_eq!(pack.snippets[0].symbol, "calculate_invoice_total");
        assert!(pack.estimated_tokens <= 200);
        assert!(pack.render().contains("src/lib.rs:1-1"));
    }

    /// Weak conversational input abstains instead of injecting generic code.
    #[test]
    fn abstains_on_stopwords() {
        let (_directory, repository, index) = fixture();
        std::fs::write(repository.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
        let pack = index
            .context(&ContextQuery {
                repo_root: repository.to_string_lossy().to_string(),
                query: "please make this code".to_string(),
                max_tokens: 200,
                focus_paths: Vec::new(),
                recent_paths: Vec::new(),
            })
            .unwrap();
        assert!(pack.snippets.is_empty());
    }

    /// Relation lookup returns a conservative outgoing call edge.
    #[test]
    fn returns_call_relations() {
        let (_directory, repository, index) = fixture();
        std::fs::write(
            repository.join("src/lib.rs"),
            "fn helper() {}\nfn caller() { helper(); }\n",
        )
        .unwrap();
        let relations = index
            .relations(&RelationQuery {
                repo_root: repository.to_string_lossy().to_string(),
                symbol: Some("caller".to_string()),
                path: None,
                kinds: vec!["calls".to_string()],
                direction: RelationDirection::Outgoing,
                limit: 10,
            })
            .unwrap();
        assert!(relations.iter().any(|relation| relation.target == "helper"));
    }
}
