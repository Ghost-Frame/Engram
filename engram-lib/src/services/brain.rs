use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::db::Database;
use crate::embeddings::EmbeddingProvider;
use crate::{EngError, Result};
// ---------------------------------------------------------------------------
// Types (from types.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: String,
    pub importance: f64,
    pub activation: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainContradiction {
    pub winner_id: i64,
    pub winner_activation: f64,
    pub loser_id: i64,
    pub loser_activation: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainQueryResult {
    pub activated: Vec<BrainMemory>,
    pub contradictions: Vec<BrainContradiction>,
}

/// BrainStats is an opaque JSON blob from the subprocess.
pub type BrainStats = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
/// All possible commands sent to the brain subprocess via stdin JSON.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum BrainCommand {
    Init {
        db_path: String,
        data_dir: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Query {
        embedding: Vec<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        top_k: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        beta: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        spread_hops: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Absorb {
        id: i64,
        content: String,
        category: String,
        source: String,
        importance: f64,
        created_at: String,
        embedding: Vec<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tags: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    DecayTick {
        ticks: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    GetStats {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Shutdown {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    DreamCycle {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    FeedbackSignal {
        memory_ids: Vec<i64>,
        edge_pairs: Vec<(i64, i64)>,
        useful: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    EvolutionTrain {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
}
// ---------------------------------------------------------------------------
// Oracle types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleResult {
    pub answer: String,
    pub sources: Vec<i64>,
    pub confidence: f64,
    pub contradictions: Vec<BrainContradiction>,
    pub hallucination_flags: Vec<String>,
    pub fallback: bool,
}

#[derive(Debug, Deserialize)]
pub struct BrainQueryOptions {
    pub query: String,
    #[serde(default)]
    pub top_k: Option<usize>,
    #[serde(default)]
    pub beta: Option<f64>,
    #[serde(default)]
    pub spread_hops: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AbsorbRequest {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct FeedbackRequest {
    pub memory_ids: Vec<i64>,
    pub edge_pairs: Vec<(i64, i64)>,
    pub useful: bool,
}

#[derive(Debug, Deserialize)]
pub struct DecayRequest {
    #[serde(default = "default_ticks")]
    pub ticks: u32,
}

fn default_ticks() -> u32 {
    1
}
// ---------------------------------------------------------------------------
// Brain query state (from state.ts)
// ---------------------------------------------------------------------------

pub struct BrainQueryState {
    last_query_time: AtomicU64,
}

impl BrainQueryState {
    pub fn new() -> Self {
        Self {
            last_query_time: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
        }
    }

    pub fn touch(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_query_time.store(now, Ordering::Relaxed);
    }

    pub fn last_query_time(&self) -> u64 {
        self.last_query_time.load(Ordering::Relaxed)
    }
}

impl Default for BrainQueryState {
    fn default() -> Self {
        Self::new()
    }
}
// ---------------------------------------------------------------------------
// Brain Manager (from manager.ts)
// ---------------------------------------------------------------------------

const REQUEST_TIMEOUT_MS: u64 = 30_000;
const MAX_RESTART_ATTEMPTS: u32 = 3;
#[allow(dead_code)]
const RESTART_BACKOFF_MS: u64 = 2_000;

struct PendingRequest {
    tx: tokio::sync::oneshot::Sender<BrainResponse>,
}

pub struct BrainManager {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    ready: Arc<std::sync::atomic::AtomicBool>,
    seq: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, PendingRequest>>>,
    restart_attempts: Arc<std::sync::atomic::AtomicU32>,
    binary_path: String,
    data_dir: String,
    pub query_state: BrainQueryState,
}
impl BrainManager {
    pub fn new(data_dir: String) -> Self {
        let backend = std::env::var("ENGRAM_BRAIN_BACKEND").unwrap_or_else(|_| "rust".into());
        let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
        let binary_path = if backend == "cpp" {
            std::env::var("ENGRAM_BRAIN_CPP_BIN")
                .unwrap_or_else(|_| format!("eidolon-cpp{}", exe_suffix))
        } else {
            std::env::var("ENGRAM_BRAIN_RUST_BIN")
                .unwrap_or_else(|_| format!("eidolon{}", exe_suffix))
        };

        Self {
            child: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(None)),
            ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            seq: AtomicI64::new(0),
            pending: Arc::new(Mutex::new(HashMap::new())),
            restart_attempts: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            binary_path,
            data_dir,
            query_state: BrainQueryState::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
    async fn send_command(&self, mut cmd: BrainCommand) -> Result<BrainResponse> {
        if !self.is_ready() {
            return Err(EngError::Internal("brain_not_ready".into()));
        }

        let this_seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;

        // Inject seq into the command
        match &mut cmd {
            BrainCommand::Init { seq, .. } => *seq = Some(this_seq),
            BrainCommand::Query { seq, .. } => *seq = Some(this_seq),
            BrainCommand::Absorb { seq, .. } => *seq = Some(this_seq),
            BrainCommand::DecayTick { seq, .. } => *seq = Some(this_seq),
            BrainCommand::GetStats { seq, .. } => *seq = Some(this_seq),
            BrainCommand::Shutdown { seq, .. } => *seq = Some(this_seq),
            BrainCommand::DreamCycle { seq, .. } => *seq = Some(this_seq),
            BrainCommand::FeedbackSignal { seq, .. } => *seq = Some(this_seq),
            BrainCommand::EvolutionTrain { seq, .. } => *seq = Some(this_seq),
        }

        let payload = serde_json::to_string(&cmd)
            .map_err(|e| EngError::Internal(format!("brain_serialize: {}", e)))?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(this_seq, PendingRequest { tx });
        }

        // Write to stdin
        {
            let mut stdin_lock = self.stdin.lock().await;
            if let Some(ref mut stdin) = *stdin_lock {
                let write_result = stdin.write_all(format!("{}
", payload).as_bytes()).await;
                if let Err(e) = write_result {
                    let mut pending = self.pending.lock().await;
                    pending.remove(&this_seq);
                    return Err(EngError::Internal(format!("brain_write_failed: {}", e)));
                }
            } else {
                let mut pending = self.pending.lock().await;
                pending.remove(&this_seq);
                return Err(EngError::Internal("brain_stdin_unavailable".into()));
            }
        }

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_millis(REQUEST_TIMEOUT_MS), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&this_seq);
                Err(EngError::Internal(format!("brain_channel_closed seq={}", this_seq)))
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&this_seq);
                Err(EngError::Internal(format!("brain_timeout seq={}", this_seq)))
            }
        }
    }
    pub async fn start(&self) -> bool {
        if !std::path::Path::new(&self.binary_path).exists() {
            info!(msg = "brain_start_skipped", reason = "binary_not_found", path = %self.binary_path);
            return false;
        }

        self.spawn_brain().await;
        self.init_brain().await
    }

    async fn spawn_brain(&self) {
        info!(msg = "brain_spawn", binary = %self.binary_path);

        let mut child = match Command::new(&self.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(msg = "brain_spawn_error", error = %e);
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        *self.stdin.lock().await = stdin;
        *self.child.lock().await = Some(child);

        // Spawn stderr reader
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    info!(brain_stderr = %line);
                }
            });
        }

        // Spawn stdout reader to resolve pending requests
        if let Some(stdout) = stdout {
            let pending = self.pending.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<BrainResponse>(&trimmed) {
                        Ok(resp) => {
                            if let Some(s) = resp.seq {
                                let mut map = pending.lock().await;
                                if let Some(req) = map.remove(&s) {
                                    let _ = req.tx.send(resp);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                msg = "brain_parse_error",
                                line = %trimmed.chars().take(200).collect::<String>(),
                                error = %e
                            );
                        }
                    }
                }
            });
        }
    }
    async fn init_brain(&self) -> bool {
        // Give process a moment to start
        tokio::time::sleep(Duration::from_millis(200)).await;

        if self.child.lock().await.is_none() {
            return false;
        }

        // Temporarily mark ready so send_command works
        self.ready.store(true, Ordering::Relaxed);

        let db_path = std::path::Path::new(&self.data_dir)
            .join("brain.db")
            .to_string_lossy()
            .into_owned();

        match self
            .send_command(BrainCommand::Init {
                db_path,
                data_dir: self.data_dir.clone(),
                seq: None,
            })
            .await
        {
            Ok(resp) => {
                if resp.ok {
                    self.restart_attempts.store(0, Ordering::Relaxed);
                    info!(msg = "brain_ready");
                    true
                } else {
                    warn!(msg = "brain_init_failed", error = ?resp.error);
                    self.ready.store(false, Ordering::Relaxed);
                    false
                }
            }
            Err(e) => {
                warn!(msg = "brain_init_error", error = %e);
                self.ready.store(false, Ordering::Relaxed);
                false
            }
        }
    }
    pub async fn stop(&self) {
        // Max out restart attempts to prevent auto-restart
        self.restart_attempts
            .store(MAX_RESTART_ATTEMPTS, Ordering::Relaxed);

        if self.is_ready() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.send_command(BrainCommand::Shutdown { seq: None }),
            )
            .await;
        }

        self.ready.store(false, Ordering::Relaxed);

        // Kill the process
        let mut child_lock = self.child.lock().await;
        if let Some(ref mut child) = *child_lock {
            let _ = child.kill().await;
        }
        *child_lock = None;
        *self.stdin.lock().await = None;

        // Reject all pending
        let mut pending = self.pending.lock().await;
        pending.clear();

        info!(msg = "brain_stopped");
    }
    pub async fn query(
        &self,
        embedder: &dyn EmbeddingProvider,
        text: &str,
        options: &BrainQueryOptions,
    ) -> Result<BrainQueryResult> {
        let embedding = embedder.embed(text).await?;
        let resp = self
            .send_command(BrainCommand::Query {
                embedding,
                top_k: options.top_k,
                beta: options.beta,
                spread_hops: options.spread_hops,
                seq: None,
            })
            .await?;

        if !resp.ok {
            return Err(EngError::Internal(
                resp.error.unwrap_or_else(|| "brain_query_failed".into()),
            ));
        }

        let data = resp
            .data
            .ok_or_else(|| EngError::Internal("brain_query: no data".into()))?;
        serde_json::from_value(data)
            .map_err(|e| EngError::Internal(format!("brain_query parse: {}", e)))
    }

    pub async fn absorb(
        &self,
        embedder: &dyn EmbeddingProvider,
        memory: AbsorbMemoryData,
    ) -> Result<()> {
        if !self.is_ready() {
            return Ok(());
        }

        let embedding = embedder.embed(&memory.content).await?;
        let resp = self
            .send_command(BrainCommand::Absorb {
                id: memory.id,
                content: memory.content,
                category: memory.category,
                source: memory.source,
                importance: memory.importance,
                created_at: memory.created_at,
                embedding,
                tags: memory.tags,
                seq: None,
            })
            .await;

        if let Err(e) = resp {
            warn!(msg = "brain_absorb_failed", id = memory.id, error = %e);
        }
        Ok(())
    }
    pub async fn decay_tick(&self, ticks: u32) -> Result<()> {
        if !self.is_ready() {
            return Ok(());
        }
        let _ = self
            .send_command(BrainCommand::DecayTick { ticks, seq: None })
            .await;
        Ok(())
    }

    pub async fn stats(&self) -> Result<BrainStats> {
        let resp = self
            .send_command(BrainCommand::GetStats { seq: None })
            .await?;
        if !resp.ok {
            return Err(EngError::Internal(
                resp.error.unwrap_or_else(|| "brain_stats_failed".into()),
            ));
        }
        Ok(resp.data.unwrap_or(Value::Null))
    }

    pub async fn dream_cycle(&self) -> Result<BrainResponse> {
        self.send_command(BrainCommand::DreamCycle { seq: None }).await
    }

    pub async fn feedback_signal(
        &self,
        memory_ids: Vec<i64>,
        edge_pairs: Vec<(i64, i64)>,
        useful: bool,
    ) -> Result<BrainResponse> {
        self.send_command(BrainCommand::FeedbackSignal {
            memory_ids,
            edge_pairs,
            useful,
            seq: None,
        })
        .await
    }

    pub async fn evolution_train(&self) -> Result<BrainResponse> {
        self.send_command(BrainCommand::EvolutionTrain { seq: None })
            .await
    }
}
#[derive(Debug, Clone)]
pub struct AbsorbMemoryData {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: String,
    pub importance: f64,
    pub created_at: String,
    pub tags: Option<Vec<String>>,
}

/// Look up a memory row by id and user_id, for the absorb route.
pub async fn get_memory_for_absorb(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<AbsorbMemoryData> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, content, category, source, importance, created_at, tags
             FROM memories WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("memory {}", id)))?;

    let mem_id: i64 = row.get(0)?;
    let content: String = row.get(1)?;
    let category: String = row.get(2)?;
    let source: String = row.get(3)?;
    let importance: f64 = row.get(4)?;
    let created_at: String = row.get(5)?;
    let tags_raw: Option<String> = row.get(6)?;

    let tags = tags_raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());

    Ok(AbsorbMemoryData {
        id: mem_id,
        content,
        category,
        source,
        importance,
        created_at,
        tags,
    })
}
/// Verify that all memory IDs belong to the given user. Returns true if all match.
pub async fn verify_memory_ownership(
    db: &Database,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<bool> {
    if memory_ids.is_empty() {
        return Ok(true);
    }

    let placeholders: Vec<String> = (1..=memory_ids.len())
        .map(|i| format!("?{}", i))
        .collect();
    let sql = format!(
        "SELECT COUNT(*) FROM memories WHERE id IN ({}) AND user_id = ?{}",
        placeholders.join(","),
        memory_ids.len() + 1
    );

    let mut params: Vec<libsql::Value> = memory_ids
        .iter()
        .map(|id| libsql::Value::Integer(*id))
        .collect();
    params.push(libsql::Value::Integer(user_id));

    let mut rows = db
        .conn
        .query(&sql, libsql::params_from_iter(params))
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("count query failed".into()))?;
    let count: i64 = row.get(0)?;

    Ok(count == memory_ids.len() as i64)
}
// ---------------------------------------------------------------------------
// Oracle (from oracle.ts)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
const ORACLE_SYSTEM_PROMPT: &str = "You are Eidolon, a living memory system. You answer questions \nusing ONLY the memories provided below. You are not a general AI assistant - you are a specific \nintelligence that knows what it has been taught and nothing else.";

pub fn build_user_prompt(
    query: &str,
    result: &BrainQueryResult,
    context: Option<&str>,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!("QUERY: {}", query));
    lines.push(String::new());

    if result.activated.is_empty() {
        lines.push("MEMORIES: none activated".into());
    } else {
        lines.push("MEMORIES (sorted by activation, highest first):".into());
        let mut sorted = result.activated.clone();
        sorted.sort_by(|a, b| b.activation.partial_cmp(&a.activation).unwrap_or(std::cmp::Ordering::Equal));
        let top = sorted.into_iter().take(8);
        for mem in top {
            let age = mem.created_at.as_deref().unwrap_or("");
            let age_str = if age.is_empty() {
                String::new()
            } else {
                format!("created: {}", age)
            };
            lines.push(format!(
                "  [#{}] activation={:.4} importance={} {}",
                mem.id, mem.activation, mem.importance, age_str
            ));
            let truncated = if mem.content.len() > 300 {
                format!("{}...", &mem.content[..300])
            } else {
                mem.content.clone()
            };
            lines.push(format!("  {}", truncated));
            lines.push(String::new());
        }
    }

    if !result.contradictions.is_empty() {
        lines.push("CONTRADICTIONS DETECTED:".into());
        for c in &result.contradictions {
            lines.push(format!(
                "  winner=#{} (activation={:.4}) vs loser=#{} (activation={:.4}): {}",
                c.winner_id, c.winner_activation, c.loser_id, c.loser_activation, c.reason
            ));
        }
        lines.push(String::new());
    }

    if let Some(ctx) = context {
        lines.push("CONVERSATION CONTEXT:".into());
        lines.push(ctx.to_string());
        lines.push(String::new());
    }


    lines.join("
")
}

pub fn extract_claims(text: &str) -> Vec<String> {
    text.split(['.', '!', '?'])
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() > 20)
        .collect()
}

pub fn detect_hallucinations(answer: &str, result: &BrainQueryResult) -> Vec<String> {
    if result.activated.is_empty() {
        return Vec::new();
    }

    let memory_corpus: String = result
        .activated
        .iter()
        .map(|m| m.content.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    let claims = extract_claims(answer);
    let mut flags = Vec::new();

    let stopwords: HashSet<&str> = [
        "this", "that", "with", "from", "have", "been", "were", "they",
        "about", "their", "there", "which", "would", "could", "should",
        "these", "those", "then", "than", "when", "what", "also", "into",
    ]
    .iter()
    .copied()
    .collect();

    for claim in &claims {
        let keywords: Vec<String> = claim
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .filter(|w| w.len() > 4 && !stopwords.contains(w))
            .map(String::from)
            .collect();

        if keywords.is_empty() {
            continue;
        }

        let matched = keywords.iter().filter(|kw| memory_corpus.contains(kw.as_str())).count();
        let ratio = matched as f64 / keywords.len() as f64;

        if ratio < 0.25 {
            let truncated = if claim.len() > 80 {
                format!("{}...", &claim[..80])
            } else {
                claim.clone()
            };
            flags.push(format!("Claim not grounded in memories: \"{}\"", truncated));
        }
    }

    flags
}

pub fn format_fallback(result: &BrainQueryResult) -> OracleResult {
    let sources: Vec<i64> = result.activated.iter().map(|m| m.id).collect();
    let confidence = if result.activated.is_empty() {
        0.0
    } else {
        let top: Vec<&BrainMemory> = result.activated.iter().take(5).collect();
        top.iter().map(|m| m.activation).sum::<f64>() / top.len() as f64
    };

    let answer = if result.activated.is_empty() {
        "I do not have information about that.".to_string()
    } else {
        let top_str: Vec<String> = result
            .activated
            .iter()
            .take(3)
            .map(|m| format!("[#{}] {}", m.id, m.content))
            .collect();
        format!(
            "[Fallback - LLM unavailable] Relevant memories: {}",
            top_str.join("; ")
        )
    };

    OracleResult {
        answer,
        sources,
        confidence,
        contradictions: result.contradictions.clone(),
        hallucination_flags: Vec::new(),
        fallback: true,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_claims() {
        let text = "The server runs on port 8080. It also handles websockets! Is it fast? Yes.";
        let claims = extract_claims(text);
        assert_eq!(claims.len(), 2);
        assert!(claims[0].contains("server runs on port"));
        assert!(claims[1].contains("handles websockets"));
    }

    #[test]
    fn test_detect_hallucinations_empty_memories() {
        let result = BrainQueryResult {
            activated: vec![],
            contradictions: vec![],
        };
        let flags = detect_hallucinations("Some random answer about things.", &result);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_detect_hallucinations_grounded() {
        let result = BrainQueryResult {
            activated: vec![BrainMemory {
                id: 1,
                content: "The database server runs PostgreSQL version 15 on port 5432".into(),
                category: "infra".into(),
                source: "manual".into(),
                importance: 1.0,
                activation: 0.9,
                created_at: None,
                tags: None,
            }],
            contradictions: vec![],
        };
        let flags = detect_hallucinations(
            "The database server runs PostgreSQL on port 5432.",
            &result,
        );
        assert!(flags.is_empty(), "Expected no flags, got {:?}", flags);
    }

    #[test]
    fn test_detect_hallucinations_ungrounded() {
        let result = BrainQueryResult {
            activated: vec![BrainMemory {
                id: 1,
                content: "The database runs PostgreSQL on port 5432".into(),
                category: "infra".into(),
                source: "manual".into(),
                importance: 1.0,
                activation: 0.9,
                created_at: None,
                tags: None,
            }],
            contradictions: vec![],
        };
        let flags = detect_hallucinations(
            "The kubernetes cluster deploys microservices through ArgoCD pipelines.",
            &result,
        );
        assert!(!flags.is_empty());
    }

    #[test]
    fn test_format_fallback_no_memories() {
        let result = BrainQueryResult {
            activated: vec![],
            contradictions: vec![],
        };
        let oracle = format_fallback(&result);
        assert!(oracle.fallback);
        assert_eq!(oracle.confidence, 0.0);
        assert!(oracle.answer.contains("do not have information"));
    }

    #[test]
    fn test_format_fallback_with_memories() {
        let result = BrainQueryResult {
            activated: vec![
                BrainMemory {
                    id: 42,
                    content: "Engram stores memories in SQLite".into(),
                    category: "docs".into(),
                    source: "manual".into(),
                    importance: 0.8,
                    activation: 0.95,
                    created_at: None,
                    tags: None,
                },
            ],
            contradictions: vec![],
        };
        let oracle = format_fallback(&result);
        assert!(oracle.fallback);
        assert!(oracle.confidence > 0.0);
        assert!(oracle.answer.contains("Fallback"));
        assert!(oracle.answer.contains("[#42]"));
    }

    #[test]
    fn test_build_user_prompt_empty() {
        let result = BrainQueryResult {
            activated: vec![],
            contradictions: vec![],
        };
        let prompt = build_user_prompt("What is engram?", &result, None);
        assert!(prompt.contains("QUERY: What is engram?"));
        assert!(prompt.contains("none activated"));
    }

    #[test]
    fn test_build_user_prompt_with_context() {
        let result = BrainQueryResult {
            activated: vec![BrainMemory {
                id: 1,
                content: "Engram is a memory system".into(),
                category: "docs".into(),
                source: "manual".into(),
                importance: 1.0,
                activation: 0.8,
                created_at: Some("2025-01-01".into()),
                tags: None,
            }],
            contradictions: vec![],
        };
        let prompt = build_user_prompt("test?", &result, Some("previous conversation"));
        assert!(prompt.contains("CONVERSATION CONTEXT:"));
        assert!(prompt.contains("previous conversation"));
        assert!(prompt.contains("[#1]"));
    }

    #[test]
    fn test_brain_query_state() {
        let state = BrainQueryState::new();
        let t1 = state.last_query_time();
        assert!(t1 > 0);
        std::thread::sleep(std::time::Duration::from_millis(10));
        state.touch();
        let t2 = state.last_query_time();
        assert!(t2 >= t1);
    }

    #[test]
    fn test_default_ticks() {
        assert_eq!(default_ticks(), 1);
    }
}
