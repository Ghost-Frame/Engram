use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::db::Database;
use crate::embeddings::EmbeddingProvider;
use crate::{EngError, Result};

// --- BrainBackend trait -- unifies subprocess and in-process Hopfield ---

/// Trait that abstracts over different brain implementations. The server
/// routes call these methods without knowing whether the brain is a
/// subprocess (eidolon binary) or the in-process Hopfield network.
#[async_trait]
pub trait BrainBackend: Send + Sync {
    fn is_ready(&self) -> bool;
    async fn stop(&self);
    /// Query for activated patterns. `user_id` scopes recall to the caller's
    /// pattern space; the in-process Hopfield backend filters by owner so
    /// cross-tenant leakage is impossible.
    async fn query(
        &self,
        embedder: &dyn EmbeddingProvider,
        text: &str,
        user_id: i64,
        options: &BrainQueryOptions,
    ) -> Result<BrainQueryResult>;
    /// Absorb a memory into the brain. `user_id` is the verified owner of
    /// the memory row; it becomes the pattern's owner in the network.
    async fn absorb(
        &self,
        embedder: &dyn EmbeddingProvider,
        user_id: i64,
        memory: AbsorbMemoryData,
    ) -> Result<()>;
    /// Decay the caller's patterns.
    async fn decay_tick(&self, user_id: i64, ticks: u32) -> Result<()>;
    /// Per-tenant brain statistics.
    async fn stats(&self, user_id: i64) -> Result<BrainStats>;
    /// Per-tenant dream cycle. `user_id` scopes the consolidation to one
    /// tenant's pattern space. The route/dreamer layer fans out over all
    /// active users; admin scope is still required at the route boundary.
    async fn dream_cycle(&self, user_id: i64) -> Result<BrainResponse>;
    async fn feedback_signal(
        &self,
        user_id: i64,
        memory_ids: Vec<i64>,
        edge_pairs: Vec<(i64, i64)>,
        useful: bool,
    ) -> Result<BrainResponse>;
    async fn evolution_train(&self) -> Result<BrainResponse>;
    async fn evolution_stats(&self) -> Result<BrainResponse>;
    async fn reapply_instincts(&self) -> Result<BrainResponse> {
        Err(crate::EngError::Internal(
            "reapply_instincts not supported by this backend".to_string(),
        ))
    }
    /// M-014: graceful shutdown -- kill subprocess + abort reader tasks.
    /// Default implementation delegates to stop(); backends that track
    /// JoinHandles (BrainManager) override this.
    async fn shutdown(&self) {
        self.stop().await;
    }
}
// --- Types (from types.ts) ---

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
        user_id: i64,
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
    EvolutionStats {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
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
// --- Brain query state (from state.ts) ---

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
// --- Brain Manager (from manager.ts) ---

const REQUEST_TIMEOUT_MS: u64 = 30_000;
const MAX_RESTART_ATTEMPTS: u32 = 3;

struct PendingRequest {
    tx: tokio::sync::oneshot::Sender<BrainResponse>,
}

/// M-014: max entries in the pending map (in-flight brain requests).
const PENDING_CAP: usize = 1024;

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
    /// M-014: track reader task handles so shutdown() can abort them.
    reader_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}
impl BrainManager {
    pub fn new(data_dir: String) -> Self {
        let backend = crate::kleos_env("BRAIN_BACKEND").unwrap_or_else(|_| "rust".into());
        let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
        let binary_path = if backend == "cpp" {
            crate::kleos_env("BRAIN_CPP_BIN")
                .unwrap_or_else(|_| format!("eidolon-cpp{}", exe_suffix))
        } else {
            crate::kleos_env("BRAIN_RUST_BIN").unwrap_or_else(|_| format!("eidolon{}", exe_suffix))
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
            reader_handles: Arc::new(Mutex::new(Vec::new())),
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
            BrainCommand::DreamCycle { seq, .. } => *seq = Some(this_seq), // user_id forwarded as-is
            BrainCommand::FeedbackSignal { seq, .. } => *seq = Some(this_seq),
            BrainCommand::EvolutionTrain { seq, .. } => *seq = Some(this_seq),
            BrainCommand::EvolutionStats { seq, .. } => *seq = Some(this_seq),
        }

        let payload = serde_json::to_string(&cmd)
            .map_err(|e| EngError::Internal(format!("brain_serialize: {}", e)))?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            // M-015: cap the in-flight request map to prevent unbounded growth.
            if pending.len() >= PENDING_CAP {
                return Err(EngError::Resource("brain pending queue full".into()));
            }
            pending.insert(this_seq, PendingRequest { tx });
        }

        // Write to stdin
        {
            let mut stdin_lock = self.stdin.lock().await;
            if let Some(ref mut stdin) = *stdin_lock {
                let write_result = stdin
                    .write_all(
                        format!(
                            "{}
",
                            payload
                        )
                        .as_bytes(),
                    )
                    .await;
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
                Err(EngError::Internal(format!(
                    "brain_channel_closed seq={}",
                    this_seq
                )))
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&this_seq);
                Err(EngError::Internal(format!(
                    "brain_timeout seq={}",
                    this_seq
                )))
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

        // M-014: track reader JoinHandles so shutdown() can abort them.
        let mut handles = self.reader_handles.lock().await;
        handles.clear();

        // Spawn stderr reader
        if let Some(stderr) = stderr {
            let h = tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    info!(brain_stderr = %line);
                }
            });
            handles.push(h);
        }

        // Spawn stdout reader to resolve pending requests
        if let Some(stdout) = stdout {
            let pending = self.pending.clone();
            let h = tokio::spawn(async move {
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
            handles.push(h);
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

        // SECURITY: acquire locks in the same order as send_command()
        // (pending -> stdin) to prevent AB/BA deadlock. child lock is
        // independent (never held by send_command) so acquired separately.
        {
            let mut pending = self.pending.lock().await;
            pending.clear();
        }
        {
            let mut stdin_lock = self.stdin.lock().await;
            *stdin_lock = None;
        }

        // Kill the process
        let mut child_lock = self.child.lock().await;
        if let Some(ref mut child) = *child_lock {
            let _ = child.kill().await;
        }
        *child_lock = None;

        info!(msg = "brain_stopped");
    }

    /// M-014: graceful shutdown with reader-task abort.
    ///
    /// 1. Mark not-ready and clear pending (drops Senders, waking waiters with RecvError).
    /// 2. Drop stdin to signal EOF to the subprocess.
    /// 3. Kill the child process.
    /// 4. Abort the two reader JoinHandles with a 5-second timeout.
    pub async fn shutdown(&self) {
        self.ready.store(false, Ordering::Relaxed);

        // Clear pending first so any waiter unblocks immediately.
        {
            let mut pending = self.pending.lock().await;
            pending.clear();
        }

        // Drop stdin so the subprocess sees EOF and can exit cleanly.
        {
            let mut stdin_lock = self.stdin.lock().await;
            *stdin_lock = None;
        }

        // Kill the child process.
        {
            let mut child_lock = self.child.lock().await;
            if let Some(ref mut child) = *child_lock {
                let _ = child.kill().await;
            }
            *child_lock = None;
        }

        // Abort reader tasks and wait up to 5s for them to exit.
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self.reader_handles.lock().await;
            std::mem::take(&mut *guard)
        };
        for h in &handles {
            h.abort();
        }
        let _ =
            tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(handles)).await;

        info!(msg = "brain_shutdown_complete");
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

    pub async fn dream_cycle(&self, user_id: i64) -> Result<BrainResponse> {
        self.send_command(BrainCommand::DreamCycle { user_id, seq: None })
            .await
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

    pub async fn evolution_stats(&self) -> Result<BrainResponse> {
        self.send_command(BrainCommand::EvolutionStats { seq: None })
            .await
    }
}

#[async_trait]
impl BrainBackend for BrainManager {
    fn is_ready(&self) -> bool {
        self.is_ready()
    }

    async fn stop(&self) {
        self.stop().await;
    }

    async fn query(
        &self,
        embedder: &dyn EmbeddingProvider,
        text: &str,
        _user_id: i64,
        options: &BrainQueryOptions,
    ) -> Result<BrainQueryResult> {
        // The subprocess protocol does not carry tenant identity. Single-user
        // deployments using the subprocess backend keep the legacy behaviour;
        // multi-tenant deployments should run the in-process Hopfield backend.
        self.query(embedder, text, options).await
    }

    async fn absorb(
        &self,
        embedder: &dyn EmbeddingProvider,
        _user_id: i64,
        memory: AbsorbMemoryData,
    ) -> Result<()> {
        self.absorb(embedder, memory).await
    }

    async fn decay_tick(&self, _user_id: i64, ticks: u32) -> Result<()> {
        self.decay_tick(ticks).await
    }

    async fn stats(&self, _user_id: i64) -> Result<BrainStats> {
        self.stats().await
    }

    async fn dream_cycle(&self, user_id: i64) -> Result<BrainResponse> {
        self.dream_cycle(user_id).await
    }

    async fn feedback_signal(
        &self,
        _user_id: i64,
        memory_ids: Vec<i64>,
        edge_pairs: Vec<(i64, i64)>,
        useful: bool,
    ) -> Result<BrainResponse> {
        self.feedback_signal(memory_ids, edge_pairs, useful).await
    }

    async fn evolution_train(&self) -> Result<BrainResponse> {
        self.evolution_train().await
    }

    async fn evolution_stats(&self) -> Result<BrainResponse> {
        self.evolution_stats().await
    }

    /// M-014: override to use the full shutdown() with JoinHandle abort.
    async fn shutdown(&self) {
        self.shutdown().await;
    }
}

// --- HopfieldBrainManager -- in-process Hopfield network backend ---

#[cfg(feature = "brain_hopfield")]
pub struct HopfieldBrainManager {
    /// In-process Hopfield network. RwLock so reads (query, stats) don't
    /// block each other; writes (absorb, dream, feedback, instincts) get
    /// exclusive access.
    network: RwLock<crate::brain::hopfield::HopfieldNetwork>,
    db: Arc<Database>,
    ready: std::sync::atomic::AtomicBool,
    pub query_state: BrainQueryState,
    /// Evolution state for neuroevolution training loop.
    evolution: RwLock<crate::brain::evolution::EvolutionState>,
}

#[cfg(feature = "brain_hopfield")]
impl HopfieldBrainManager {
    /// Create a new in-process Hopfield brain manager. Loads patterns from
    /// every user into one shared in-memory network; tenant isolation happens
    /// at recall/mutation time by passing the caller's `user_id`.
    pub async fn new(db: Arc<Database>) -> Result<Self> {
        use crate::brain::hopfield::pattern;

        info!(msg = "hopfield_brain_init");

        let db_patterns = pattern::list_all_patterns(&db).await?;
        let batch: Vec<(i64, i64, Vec<f32>, f32)> = db_patterns
            .into_iter()
            .map(|p| (p.id, p.user_id, p.pattern, p.strength))
            .collect();

        let pattern_count = batch.len();
        let network = crate::brain::hopfield::HopfieldNetwork::from_patterns(batch);

        info!(
            msg = "hopfield_brain_ready",
            patterns_loaded = pattern_count
        );

        let evolution = crate::brain::evolution::EvolutionState::load_state(&db).await;

        Ok(Self {
            network: RwLock::new(network),
            db,
            ready: std::sync::atomic::AtomicBool::new(true),
            query_state: BrainQueryState::new(),
            evolution: RwLock::new(evolution),
        })
    }
}

#[cfg(feature = "brain_hopfield")]
#[async_trait]
impl BrainBackend for HopfieldBrainManager {
    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    async fn stop(&self) {
        self.ready.store(false, Ordering::Relaxed);
        info!(msg = "hopfield_brain_stopped");
    }

    async fn query(
        &self,
        embedder: &dyn EmbeddingProvider,
        text: &str,
        user_id: i64,
        options: &BrainQueryOptions,
    ) -> Result<BrainQueryResult> {
        use crate::brain::hopfield::{recall, DEFAULT_BETA};

        let embedding = embedder.embed(text).await?;
        let top_k = options.top_k.unwrap_or(10);
        let beta = options.beta.map(|b| b as f32).unwrap_or(DEFAULT_BETA);

        let network = self.network.read().await;
        let results =
            recall::recall_pattern(&self.db, &network, &embedding, user_id, top_k, beta).await?;

        self.query_state.touch();

        // Convert RecallResults to BrainMemory format
        let mut activated = Vec::new();
        for r in &results {
            // Load the full memory content from the memories table
            let mem = load_brain_memory(&self.db, r.pattern_id).await;
            if let Ok(mut m) = mem {
                m.activation = r.activation as f64;
                activated.push(m);
            }
        }

        Ok(BrainQueryResult {
            activated,
            contradictions: Vec::new(), // Contradiction detection is done by the intelligence layer
        })
    }

    async fn absorb(
        &self,
        embedder: &dyn EmbeddingProvider,
        user_id: i64,
        memory: AbsorbMemoryData,
    ) -> Result<()> {
        use crate::brain::hopfield::recall;
        use crate::brain::instincts;

        if !self.is_ready() {
            return Ok(());
        }

        let embedding = embedder.embed(&memory.content).await?;
        let importance = memory.importance.round() as i32;

        let mut network = self.network.write().await;

        // Store pattern and create causal edges to temporal neighbours.
        recall::store_pattern_with_causal_edges(
            &self.db,
            &mut network,
            memory.id,
            &embedding,
            user_id,
            importance,
            1.0, // Initial strength
            &memory.content,
            &memory.created_at,
        )
        .await?;

        // Remove any ghost patterns superseded by this real memory.
        let ghosts_removed =
            instincts::check_ghost_replacement(&self.db, &mut network, &embedding, user_id)
                .await
                .unwrap_or(0);

        if ghosts_removed > 0 {
            info!(
                msg = "hopfield_ghost_replaced",
                id = memory.id,
                ghosts_removed = ghosts_removed
            );
        }

        info!(msg = "hopfield_absorbed", id = memory.id);
        Ok(())
    }

    async fn decay_tick(&self, user_id: i64, ticks: u32) -> Result<()> {
        use crate::brain::hopfield::recall;

        if !self.is_ready() {
            return Ok(());
        }

        let mut network = self.network.write().await;
        let stats = recall::decay_tick(&self.db, &mut network, user_id, ticks).await?;

        info!(
            msg = "hopfield_decay",
            ticks = ticks,
            patterns_decayed = stats.patterns_decayed,
            patterns_removed = stats.patterns_removed,
            edges_decayed = stats.edges_decayed,
            edges_removed = stats.edges_removed
        );
        Ok(())
    }

    async fn stats(&self, user_id: i64) -> Result<BrainStats> {
        use crate::brain::hopfield::pattern;

        let network = self.network.read().await;
        let db_patterns = pattern::list_patterns(&self.db, user_id).await?;

        let total_strength: f32 = db_patterns.iter().map(|p| p.strength).sum();
        let avg_strength = if db_patterns.is_empty() {
            0.0
        } else {
            total_strength / db_patterns.len() as f32
        };

        Ok(serde_json::json!({
            "mode": "hopfield",
            "pattern_count": network.pattern_count(),
            "db_pattern_count": db_patterns.len(),
            "avg_strength": avg_strength,
            "total_strength": total_strength,
        }))
    }

    async fn dream_cycle(&self, user_id: i64) -> Result<BrainResponse> {
        use crate::brain::dream::run_dream_cycle;

        // Dream scoped to a single tenant's pattern space. The route/dreamer
        // layer fans out over all active users; admin scope required at the
        // route boundary (H-R3-001).
        let mut network = self.network.write().await;

        // Full 6-stage consolidation: replay, merge, prune, discover, decorrelate, resolve.
        // Budget caps items processed per stage so a long idle period does not
        // translate to an unbounded work burst.
        const DREAM_BUDGET: u32 = 64;
        let result = run_dream_cycle(&self.db, &mut network, user_id, DREAM_BUDGET).await?;

        Ok(BrainResponse {
            seq: None,
            ok: true,
            error: None,
            data: Some(serde_json::to_value(&result).unwrap_or(serde_json::Value::Null)),
        })
    }

    async fn feedback_signal(
        &self,
        user_id: i64,
        memory_ids: Vec<i64>,
        edge_pairs: Vec<(i64, i64)>,
        useful: bool,
    ) -> Result<BrainResponse> {
        use crate::brain::evolution::FeedbackSignal;
        use crate::brain::hopfield::recall;

        let mut network = self.network.write().await;

        let mut reinforced = Vec::new();
        for id in &memory_ids {
            if useful {
                // Reinforce useful patterns in the Hopfield network
                match recall::reinforce(&self.db, &mut network, *id, user_id).await {
                    Ok(new_strength) => {
                        reinforced.push(serde_json::json!({
                            "id": id,
                            "new_strength": new_strength,
                        }));
                    }
                    Err(e) => {
                        warn!(msg = "hopfield_reinforce_failed", id = id, error = %e);
                    }
                }
            }
            // For non-useful patterns, natural decay handles weakening
        }

        // Record into evolution buffer
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let signal = FeedbackSignal {
            memory_ids,
            edge_pairs,
            useful,
            timestamp,
        };
        self.evolution.write().await.record_feedback(signal);

        Ok(BrainResponse {
            seq: None,
            ok: true,
            error: None,
            data: Some(serde_json::json!({
                "reinforced": reinforced,
                "useful": useful,
            })),
        })
    }

    async fn evolution_train(&self) -> Result<BrainResponse> {
        let mut evo = self.evolution.write().await;
        let before = evo.generation;
        evo.train_step();
        let after = evo.generation;
        evo.save_state(&self.db).await?;

        Ok(BrainResponse {
            seq: None,
            ok: true,
            error: None,
            data: Some(serde_json::json!({
                "generation_before": before,
                "generation_after": after,
                "num_node_weights": evo.node_weights.len(),
                "num_edge_weights": evo.edge_weights.len(),
            })),
        })
    }

    async fn evolution_stats(&self) -> Result<BrainResponse> {
        use crate::brain::evolution::EvolutionStatsResult;

        let evo = self.evolution.read().await;
        let stats = EvolutionStatsResult::from(&*evo);

        Ok(BrainResponse {
            seq: None,
            ok: true,
            error: None,
            data: Some(
                serde_json::to_value(&stats)
                    .map_err(|e| EngError::Internal(format!("evolution_stats serialize: {}", e)))?,
            ),
        })
    }

    async fn reapply_instincts(&self) -> Result<BrainResponse> {
        use crate::brain::instincts;

        // Admin-gated at the route layer; scoped to operator namespace.
        let user_id: i64 = 1;
        let mut network = self.network.write().await;
        let report = instincts::reapply_instincts(&self.db, &mut network, user_id).await?;

        Ok(BrainResponse {
            seq: None,
            ok: true,
            error: None,
            data: Some(serde_json::to_value(&report).unwrap_or(Value::Null)),
        })
    }
}

/// Load a BrainMemory from the memories table for a given pattern id.
/// Falls back to a minimal record if the memory doesn't exist in the
/// memories table (pattern may have been created directly).
#[cfg(feature = "brain_hopfield")]
async fn load_brain_memory(db: &Database, memory_id: i64) -> Result<BrainMemory> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, content, category, source, importance, created_at, tags
                 FROM memories WHERE id = ?1",
        )?;

        let mut rows = stmt.query(rusqlite::params![memory_id])?;

        if let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let category: String = row.get(2)?;
            let source: String = row.get(3)?;
            let importance: f64 = row.get(4)?;
            let created_at: Option<String> = row.get(5)?;
            let tags_raw: Option<String> = row.get(6)?;
            let tags = tags_raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());

            Ok(BrainMemory {
                id,
                content,
                category,
                source,
                importance,
                activation: 0.0, // Will be set by caller
                created_at,
                tags,
            })
        } else {
            Err(EngError::NotFound(format!("memory {}", memory_id)))
        }
    })
    .await
}

/// Pure parse of the `TENANT_SHARDING` value. Sharding is the default (None),
/// and only the explicit off-values disable it. Mirrors the parse in
/// kleos-server/src/main.rs.
fn parse_tenant_sharding(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let s = v.trim().to_ascii_lowercase();
            !(s == "0" || s == "false" || s == "off" || s == "no")
        }
        None => true,
    }
}

/// Whether the server is running with per-tenant sharding (the default).
fn tenant_sharding_enabled() -> bool {
    parse_tenant_sharding(crate::kleos_env("TENANT_SHARDING").ok().as_deref())
}

/// The subprocess brain backend's protocol carries no tenant identity, so a
/// single global brain store is shared across every tenant -- a cross-tenant
/// isolation failure. Permit it only when sharding is disabled (single-user
/// mode); otherwise refuse (the server runs with no brain rather than a
/// tenant-leaking one). Logs the refusal and the remedy.
fn subprocess_brain_permitted() -> bool {
    if tenant_sharding_enabled() {
        warn!(
            msg = "subprocess_brain_refused_under_sharding",
            reason = "subprocess brain protocol has no tenant identity; one brain store would be shared across all tenants",
            remedy = "build with the brain_hopfield feature, or set KLEOS_TENANT_SHARDING=0 for single-user mode"
        );
        false
    } else {
        true
    }
}

/// Factory function to create the appropriate brain backend based on the
/// `ENGRAM_BRAIN_MODE` environment variable.
///
/// - `"hopfield"` (default when feature enabled): In-process Hopfield network
/// - `"subprocess"`: External eidolon binary via stdin/stdout JSON
/// - `"none"`: No brain backend
#[cfg(feature = "brain_hopfield")]
#[tracing::instrument(skip(db), fields(data_dir = %data_dir))]
pub async fn create_brain_backend(
    db: Arc<Database>,
    data_dir: &str,
) -> Option<Arc<dyn BrainBackend>> {
    let mode = crate::kleos_env("BRAIN_MODE").unwrap_or_else(|_| "hopfield".into());

    match mode.as_str() {
        "hopfield" => match HopfieldBrainManager::new(db).await {
            Ok(mgr) => {
                info!(msg = "brain_backend_selected", mode = "hopfield");
                Some(Arc::new(mgr))
            }
            Err(e) => {
                // Only fall back to subprocess when it is safe to do so. Under
                // sharding the silent fallback would drop tenant isolation, so
                // refuse and run without a brain instead.
                if !subprocess_brain_permitted() {
                    warn!(msg = "hopfield_init_failed_no_safe_fallback", error = %e);
                    return None;
                }
                warn!(msg = "hopfield_init_failed", error = %e, fallback = "subprocess");
                let mgr = BrainManager::new(data_dir.to_string());
                if mgr.start().await {
                    Some(Arc::new(mgr))
                } else {
                    warn!(msg = "brain_both_backends_failed");
                    None
                }
            }
        },
        "subprocess" => {
            if !subprocess_brain_permitted() {
                return None;
            }
            let mgr = BrainManager::new(data_dir.to_string());
            if mgr.start().await {
                info!(msg = "brain_backend_selected", mode = "subprocess");
                Some(Arc::new(mgr))
            } else {
                warn!(msg = "subprocess_brain_failed");
                None
            }
        }
        "none" => {
            info!(msg = "brain_disabled");
            None
        }
        other => {
            warn!(
                msg = "unknown_brain_mode",
                mode = other,
                fallback = "hopfield"
            );
            match HopfieldBrainManager::new(db).await {
                Ok(mgr) => Some(Arc::new(mgr)),
                Err(_) => None,
            }
        }
    }
}

/// Non-feature-gated factory -- returns subprocess or None.
#[cfg(not(feature = "brain_hopfield"))]
#[tracing::instrument(skip(_db), fields(data_dir = %data_dir))]
pub async fn create_brain_backend(
    _db: Arc<Database>,
    data_dir: &str,
) -> Option<Arc<dyn BrainBackend>> {
    let mode = crate::kleos_env("BRAIN_MODE").unwrap_or_else(|_| "subprocess".into());
    if mode == "none" {
        return None;
    }
    // Subprocess is the only backend in this build; refuse it under sharding
    // (no tenant isolation) rather than silently sharing one brain store.
    if !subprocess_brain_permitted() {
        return None;
    }
    let mgr = BrainManager::new(data_dir.to_string());
    if mgr.start().await {
        Some(Arc::new(mgr))
    } else {
        None
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

/// Returns true if the `memories` table on `conn` carries a `user_id` column.
/// Monolith schema keeps it; shard schema (tenant migration v22) drops it.
fn memories_has_user_id_column(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'user_id'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// Look up a memory row by id for the absorb route.
///
/// `user_id` is required so callers cannot accidentally absorb memories they
/// do not own when the underlying DB is the monolith (where `memories` has
/// the `user_id` column and may hold rows from multiple tenants). On shard
/// DBs the column was dropped in tenant v22; isolation is by-DB so the
/// predicate is omitted.
#[tracing::instrument(skip(db), fields(memory_id = id, user_id))]
pub async fn get_memory_for_absorb(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<AbsorbMemoryData> {
    db.read(move |conn| {
        let scoped = memories_has_user_id_column(conn);
        let sql = if scoped {
            "SELECT id, content, category, source, importance, created_at, tags
             FROM memories WHERE id = ?1 AND user_id = ?2"
        } else {
            "SELECT id, content, category, source, importance, created_at, tags
             FROM memories WHERE id = ?1"
        };
        let mut stmt = conn.prepare(sql)?;

        let mut rows = if scoped {
            stmt.query(rusqlite::params![id, user_id])?
        } else {
            stmt.query(rusqlite::params![id])?
        };

        let row = rows
            .next()?
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
    })
    .await
}

/// Verify that every memory ID is OWNED by `user_id`. Returns true only if
/// every id resolves to a row that the caller actually owns.
///
/// On monolith (memories has `user_id`) this is enforced by the SQL predicate.
/// On shards (column dropped in tenant v22) the per-DB isolation is the
/// ownership boundary, so existence-of-id implies ownership.
///
/// C-R3-001: the previous version of this function ignored `user_id` entirely
/// and only checked that the IDs existed somewhere in the table. The name
/// lied. With sharding ON by default and the conditional predicate below,
/// the function now matches its name.
#[tracing::instrument(skip(db, memory_ids), fields(memory_count = memory_ids.len(), user_id))]
pub async fn verify_memory_ownership(
    db: &Database,
    memory_ids: &[i64],
    user_id: i64,
) -> Result<bool> {
    if memory_ids.is_empty() {
        return Ok(true);
    }

    let id_count = memory_ids.len();
    let id_placeholders: Vec<String> = (1..=id_count).map(|i| format!("?{}", i)).collect();
    let placeholders_joined = id_placeholders.join(",");

    let mut params_vec: Vec<rusqlite::types::Value> = memory_ids
        .iter()
        .map(|id| rusqlite::types::Value::Integer(*id))
        .collect();

    let expected = id_count as i64;

    db.read(move |conn| {
        let scoped = memories_has_user_id_column(conn);
        let sql = if scoped {
            params_vec.push(rusqlite::types::Value::Integer(user_id));
            format!(
                "SELECT COUNT(*) FROM memories WHERE id IN ({}) AND user_id = ?{}",
                placeholders_joined,
                id_count + 1,
            )
        } else {
            format!(
                "SELECT COUNT(*) FROM memories WHERE id IN ({})",
                placeholders_joined,
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params_vec.iter().cloned()))?;

        let row = rows
            .next()?
            .ok_or_else(|| EngError::Internal("count query failed".into()))?;
        let count: i64 = row.get(0)?;

        Ok(count == expected)
    })
    .await
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Sharding is the default; only explicit off-values disable it. The
    /// subprocess brain (no tenant isolation) is permitted iff sharding is off.
    #[test]
    fn tenant_sharding_parse_and_subprocess_gate() {
        assert!(parse_tenant_sharding(None), "default is sharded");
        for off in ["0", "false", "off", "no", "  Off  ", "FALSE"] {
            assert!(!parse_tenant_sharding(Some(off)), "{off} disables sharding");
        }
        for on in ["1", "true", "yes", "anything"] {
            assert!(parse_tenant_sharding(Some(on)), "{on} keeps sharding on");
        }
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
