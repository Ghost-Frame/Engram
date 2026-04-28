-- v44: full schema parity with monolith.
--
-- Adds every table that exists in the monolith (schema_sql.rs / migrations.rs)
-- but was never created by the tenant migration chain. Without these,
-- ResolvedDb-backed routes hit "no such table" when the user_id==1 monolith
-- carve-out is removed.
--
-- All tables use IF NOT EXISTS so the migration is safe to re-run and
-- harmless on shards that already have some of these tables from other paths.

-- Agents (Soma) -- routes/agents uses ResolvedDb
CREATE TABLE IF NOT EXISTS agents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    name TEXT NOT NULL,
    category TEXT,
    description TEXT,
    code_hash TEXT,
    trust_score REAL NOT NULL DEFAULT 50,
    total_ops INTEGER NOT NULL DEFAULT 0,
    successful_ops INTEGER NOT NULL DEFAULT 0,
    failed_ops INTEGER NOT NULL DEFAULT 0,
    guard_allows INTEGER NOT NULL DEFAULT 0,
    guard_warns INTEGER NOT NULL DEFAULT 0,
    guard_blocks INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    revoked_at TEXT,
    revoke_reason TEXT,
    last_seen_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, name)
);
CREATE INDEX IF NOT EXISTS idx_agents_user ON agents(user_id);
CREATE INDEX IF NOT EXISTS idx_agents_active ON agents(is_active);

-- Gate requests (command approval flow) -- routes/gate uses ResolvedDb
CREATE TABLE IF NOT EXISTS gate_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    agent TEXT NOT NULL,
    command TEXT NOT NULL,
    context TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    reason TEXT,
    output TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_gate_requests_user ON gate_requests(user_id);
CREATE INDEX IF NOT EXISTS idx_gate_requests_status ON gate_requests(status);

-- Personality signals -- memory/profile handler passes ResolvedDb to personality
CREATE TABLE IF NOT EXISTS personality_signals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    signal_type TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '',
    valence TEXT NOT NULL DEFAULT 'neutral',
    value REAL NOT NULL,
    intensity REAL DEFAULT 0.5,
    reasoning TEXT,
    source_text TEXT,
    evidence TEXT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL DEFAULT 1,
    agent TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_personality_signals_user ON personality_signals(user_id);
CREATE INDEX IF NOT EXISTS idx_personality_signals_type ON personality_signals(signal_type);
CREATE INDEX IF NOT EXISTS idx_personality_signals_memory ON personality_signals(memory_id);

-- Personality profiles -- memory/profile handler passes ResolvedDb to personality
CREATE TABLE IF NOT EXISTS personality_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL UNIQUE,
    traits TEXT NOT NULL DEFAULT '{}',
    profile TEXT NOT NULL DEFAULT '{}',
    signal_count INTEGER NOT NULL DEFAULT 0,
    is_stale BOOLEAN NOT NULL DEFAULT 0,
    last_updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_personality_profiles_user ON personality_profiles(user_id);

-- Spaces -- admin delete-user route + quota checks use ResolvedDb
CREATE TABLE IF NOT EXISTS spaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    name TEXT NOT NULL,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, name)
);
CREATE INDEX IF NOT EXISTS idx_spaces_user ON spaces(user_id);

-- Chiasm: tasks -- context/admin routes use ResolvedDb
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    priority INTEGER NOT NULL DEFAULT 5,
    agent TEXT,
    project TEXT,
    tags TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    due_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_user ON tasks(user_id);
CREATE INDEX IF NOT EXISTS idx_tasks_agent ON tasks(agent);
CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project);

-- Axon: events -- context/activity routes use ResolvedDb
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    action TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    source TEXT,
    agent TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_events_channel ON events(channel);
CREATE INDEX IF NOT EXISTS idx_events_action ON events(action);
CREATE INDEX IF NOT EXISTS idx_events_source ON events(source);
CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_user ON events(user_id);

-- Broca: action log -- activity routes use ResolvedDb
CREATE TABLE IF NOT EXISTS action_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    action TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    project TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_action_log_agent ON action_log(agent);
CREATE INDEX IF NOT EXISTS idx_action_log_action ON action_log(action);
CREATE INDEX IF NOT EXISTS idx_action_log_project ON action_log(project);
CREATE INDEX IF NOT EXISTS idx_action_log_created ON action_log(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_action_log_user ON action_log(user_id);

-- Brain: Hopfield pattern storage -- brain routes use ResolvedDb
CREATE TABLE IF NOT EXISTS brain_patterns (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    pattern BLOB NOT NULL,
    strength REAL NOT NULL DEFAULT 1.0,
    importance INTEGER NOT NULL DEFAULT 5,
    access_count INTEGER NOT NULL DEFAULT 0,
    last_activated_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_brain_patterns_user ON brain_patterns(user_id);
CREATE INDEX IF NOT EXISTS idx_brain_patterns_strength ON brain_patterns(strength);

-- Brain: Hopfield edge adjacency
CREATE TABLE IF NOT EXISTS brain_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER NOT NULL,
    target_id INTEGER NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    edge_type TEXT NOT NULL DEFAULT 'association',
    user_id INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_id, target_id, edge_type)
);
CREATE INDEX IF NOT EXISTS idx_brain_edges_source ON brain_edges(source_id);
CREATE INDEX IF NOT EXISTS idx_brain_edges_target ON brain_edges(target_id);
CREATE INDEX IF NOT EXISTS idx_brain_edges_user ON brain_edges(user_id);

-- Brain: key-value metadata
CREATE TABLE IF NOT EXISTS brain_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Brain: PCA dimensionality reduction models
CREATE TABLE IF NOT EXISTS brain_pca_models (
    id INTEGER PRIMARY KEY,
    source_dim INTEGER NOT NULL,
    target_dim INTEGER NOT NULL,
    fit_at TEXT NOT NULL,
    model_blob BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pca_models_dims ON brain_pca_models(source_dim, target_dim);

-- Brain: dream cycle audit trail
CREATE TABLE IF NOT EXISTS brain_dream_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    replay_count INTEGER NOT NULL DEFAULT 0,
    merge_count INTEGER NOT NULL DEFAULT 0,
    prune_count INTEGER NOT NULL DEFAULT 0,
    discover_count INTEGER NOT NULL DEFAULT 0,
    decorrelate_count INTEGER NOT NULL DEFAULT 0,
    resolve_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_brain_dream_runs_user ON brain_dream_runs(user_id);
CREATE INDEX IF NOT EXISTS idx_brain_dream_runs_started ON brain_dream_runs(started_at);

-- Error events -- error tracking
CREATE TABLE IF NOT EXISTS error_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL,
    context TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_error_events_level ON error_events(level);
CREATE INDEX IF NOT EXISTS idx_error_events_source ON error_events(source);
CREATE INDEX IF NOT EXISTS idx_error_events_created_at ON error_events(created_at);

-- Rate limits -- per-tenant rate limiting
CREATE TABLE IF NOT EXISTS rate_limits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE,
    count INTEGER NOT NULL DEFAULT 0,
    window_start TEXT NOT NULL DEFAULT (datetime('now')),
    window_seconds INTEGER NOT NULL DEFAULT 60,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_rate_limits_key ON rate_limits(key);
CREATE INDEX IF NOT EXISTS idx_rate_limits_window ON rate_limits(window_start);

-- Usage events -- per-tenant usage tracking
CREATE TABLE IF NOT EXISTS usage_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    agent_id INTEGER,
    event_type TEXT NOT NULL,
    quantity INTEGER NOT NULL DEFAULT 1,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_usage_user ON usage_events(user_id);
CREATE INDEX IF NOT EXISTS idx_usage_type ON usage_events(event_type);
CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_events(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_usage_user_type ON usage_events(user_id, event_type, created_at);

-- Tenant quotas -- per-tenant quota enforcement
CREATE TABLE IF NOT EXISTS tenant_quotas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL UNIQUE,
    max_memories INTEGER NOT NULL DEFAULT 100000,
    max_conversations INTEGER DEFAULT 1000,
    max_spaces INTEGER NOT NULL DEFAULT 10,
    max_api_keys INTEGER NOT NULL DEFAULT 10,
    max_memory_size_bytes INTEGER DEFAULT 102400,
    max_agents INTEGER NOT NULL DEFAULT 20,
    storage_bytes_limit INTEGER NOT NULL DEFAULT 1073741824,
    rate_limit_override INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Jobs: durable queue -- ingestion routes enqueue via ResolvedDb
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    claimed_at TEXT,
    completed_at TEXT,
    next_retry_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, next_retry_at);
CREATE INDEX IF NOT EXISTS idx_jobs_type ON jobs(type, status);

-- Scheduler: named leases for singleton background jobs
CREATE TABLE IF NOT EXISTS scheduler_leases (
    job_name TEXT PRIMARY KEY,
    holder_id TEXT NOT NULL,
    acquired_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    last_run_at TEXT
);

INSERT OR IGNORE INTO schema_migrations (version) VALUES (44);
