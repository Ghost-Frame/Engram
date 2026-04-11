-- Per-tenant schema v1
-- No user_id columns - each database belongs to exactly one tenant

-- Schema migrations tracking
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);

-- Memories (main table)
CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'general',
    source TEXT NOT NULL DEFAULT 'unknown',
    session_id TEXT,
    importance INTEGER NOT NULL DEFAULT 5,
    embedding BLOB,
    embedding_vec_1024 FLOAT32(1024),
    version INTEGER NOT NULL DEFAULT 1,
    is_latest BOOLEAN NOT NULL DEFAULT 1,
    parent_memory_id INTEGER REFERENCES memories(id),
    root_memory_id INTEGER REFERENCES memories(id),
    source_count INTEGER NOT NULL DEFAULT 1,
    is_static BOOLEAN NOT NULL DEFAULT 0,
    is_forgotten BOOLEAN NOT NULL DEFAULT 0,
    is_archived BOOLEAN NOT NULL DEFAULT 0,
    is_inference BOOLEAN NOT NULL DEFAULT 0,
    is_fact INTEGER DEFAULT 0,
    is_decomposed INTEGER DEFAULT 0,
    forget_after TEXT,
    forget_reason TEXT,
    model TEXT,
    recall_hits INTEGER NOT NULL DEFAULT 0,
    recall_misses INTEGER NOT NULL DEFAULT 0,
    adaptive_score REAL,
    pagerank_score REAL DEFAULT 0,
    last_accessed_at TEXT,
    access_count INTEGER NOT NULL DEFAULT 0,
    tags TEXT,
    episode_id INTEGER,
    decay_score REAL,
    confidence REAL NOT NULL DEFAULT 1.0,
    sync_id TEXT,
    status TEXT NOT NULL DEFAULT 'approved',
    space_id INTEGER,
    fsrs_stability REAL,
    fsrs_difficulty REAL,
    fsrs_storage_strength REAL DEFAULT 1.0,
    fsrs_retrieval_strength REAL DEFAULT 1.0,
    fsrs_learning_state INTEGER DEFAULT 0,
    fsrs_reps INTEGER DEFAULT 0,
    fsrs_lapses INTEGER DEFAULT 0,
    fsrs_last_review_at TEXT,
    is_superseded INTEGER NOT NULL DEFAULT 0,
    is_consolidated INTEGER NOT NULL DEFAULT 0,
    valence REAL,
    arousal REAL,
    dominant_emotion TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_memories_root ON memories(root_memory_id);
CREATE INDEX IF NOT EXISTS idx_memories_superseded ON memories(is_superseded) WHERE is_superseded = 1;
CREATE INDEX IF NOT EXISTS idx_memories_consolidated ON memories(is_consolidated) WHERE is_consolidated = 1;
CREATE INDEX IF NOT EXISTS idx_memories_parent ON memories(parent_memory_id);
CREATE INDEX IF NOT EXISTS idx_memories_latest ON memories(is_latest) WHERE is_latest = 1;
CREATE INDEX IF NOT EXISTS idx_memories_forgotten ON memories(is_forgotten);
CREATE INDEX IF NOT EXISTS idx_memories_archived ON memories(is_archived) WHERE is_archived = 1;
CREATE INDEX IF NOT EXISTS idx_memories_forget_after ON memories(forget_after) WHERE forget_after IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories(tags) WHERE tags IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_episode ON memories(episode_id) WHERE episode_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_access ON memories(access_count DESC);
CREATE INDEX IF NOT EXISTS idx_memories_decay ON memories(decay_score DESC);
CREATE INDEX IF NOT EXISTS idx_memories_status ON memories(status);
CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id);
CREATE INDEX IF NOT EXISTS idx_memories_fsrs_stability ON memories(fsrs_stability) WHERE fsrs_stability IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_sync_id ON memories(sync_id) WHERE sync_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_valence ON memories(valence) WHERE valence IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_is_fact ON memories(is_fact) WHERE is_fact = 1;
CREATE INDEX IF NOT EXISTS idx_memories_parent_fact ON memories(parent_memory_id) WHERE is_fact = 1;
CREATE INDEX IF NOT EXISTS idx_memories_not_decomposed ON memories(is_decomposed) WHERE is_decomposed = 0 AND is_fact = 0;

-- Memory links
CREATE TABLE IF NOT EXISTS memory_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    similarity REAL NOT NULL,
    type TEXT NOT NULL DEFAULT 'similarity',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_id, target_id, type)
);
CREATE INDEX IF NOT EXISTS idx_links_source ON memory_links(source_id);
CREATE INDEX IF NOT EXISTS idx_links_target ON memory_links(target_id);

-- Episodes
CREATE TABLE IF NOT EXISTS episodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT,
    session_id TEXT,
    agent TEXT,
    summary TEXT,
    memory_count INTEGER NOT NULL DEFAULT 0,
    embedding BLOB,
    duration_seconds INTEGER,
    fsrs_stability REAL,
    fsrs_difficulty REAL,
    fsrs_last_review_at TEXT,
    fsrs_reps INTEGER DEFAULT 0,
    decay_score REAL DEFAULT 1.0,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_episodes_session ON episodes(session_id);
CREATE INDEX IF NOT EXISTS idx_episodes_agent ON episodes(agent);

-- Structured facts
CREATE TABLE IF NOT EXISTS structured_facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    source TEXT,
    valid_from TEXT,
    valid_until TEXT,
    is_current BOOLEAN NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_facts_memory ON structured_facts(memory_id);
CREATE INDEX IF NOT EXISTS idx_facts_subject ON structured_facts(subject);
CREATE INDEX IF NOT EXISTS idx_facts_predicate ON structured_facts(predicate);
CREATE INDEX IF NOT EXISTS idx_facts_current ON structured_facts(is_current) WHERE is_current = 1;

-- Entities (graph nodes)
CREATE TABLE IF NOT EXISTS entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL DEFAULT 'unknown',
    aliases TEXT,
    description TEXT,
    memory_ids TEXT,
    importance REAL DEFAULT 0,
    pagerank_score REAL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(type);

-- Entity relationships
CREATE TABLE IF NOT EXISTS entity_relationships (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relationship_type TEXT NOT NULL,
    weight REAL DEFAULT 1.0,
    memory_id INTEGER REFERENCES memories(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_entity_rel_source ON entity_relationships(source_entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_rel_target ON entity_relationships(target_entity_id);

-- Memory pagerank (Phase 3)
CREATE TABLE IF NOT EXISTS memory_pagerank (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER UNIQUE NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    score REAL NOT NULL DEFAULT 0,
    in_degree INTEGER NOT NULL DEFAULT 0,
    out_degree INTEGER NOT NULL DEFAULT 0,
    computed_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_pagerank_score ON memory_pagerank(score DESC);

-- Communities
CREATE TABLE IF NOT EXISTS communities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT,
    description TEXT,
    member_count INTEGER NOT NULL DEFAULT 0,
    centroid_embedding BLOB,
    coherence_score REAL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Inferences
CREATE TABLE IF NOT EXISTS inferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    source_memory_ids TEXT NOT NULL,
    inference_type TEXT NOT NULL DEFAULT 'deduction',
    is_validated BOOLEAN DEFAULT 0,
    validated_at TEXT,
    validation_result TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_inferences_type ON inferences(inference_type);
CREATE INDEX IF NOT EXISTS idx_inferences_validated ON inferences(is_validated);

-- Sessions
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    agent TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent);

-- Scratchpad
CREATE TABLE IF NOT EXISTS scratchpad (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL,
    agent TEXT,
    model TEXT,
    entry_key TEXT NOT NULL,
    value TEXT NOT NULL,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(session, agent, entry_key)
);
CREATE INDEX IF NOT EXISTS idx_scratchpad_session ON scratchpad(session);
CREATE INDEX IF NOT EXISTS idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;

-- Current state (key-value for context)
CREATE TABLE IF NOT EXISTS current_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT UNIQUE NOT NULL,
    value TEXT NOT NULL,
    updated_count INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_current_state_key ON current_state(key);

-- User preferences
CREATE TABLE IF NOT EXISTS user_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain TEXT NOT NULL,
    preference TEXT NOT NULL,
    strength REAL NOT NULL DEFAULT 1.0,
    evidence_count INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(domain, preference)
);
CREATE INDEX IF NOT EXISTS idx_preferences_domain ON user_preferences(domain);
CREATE INDEX IF NOT EXISTS idx_preferences_strength ON user_preferences(strength DESC);

-- Artifacts
CREATE TABLE IF NOT EXISTS artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    filename TEXT,
    artifact_type TEXT NOT NULL DEFAULT 'file',
    content TEXT,
    content_hash TEXT,
    mime_type TEXT,
    size_bytes INTEGER,
    sha256 TEXT,
    storage_mode TEXT NOT NULL DEFAULT 'inline',
    data BLOB,
    disk_path TEXT,
    is_indexed INTEGER NOT NULL DEFAULT 0,
    is_encrypted INTEGER NOT NULL DEFAULT 0,
    source_url TEXT,
    agent TEXT,
    session_id TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    space_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_artifacts_user ON artifacts(user_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_type ON artifacts(artifact_type);
CREATE INDEX IF NOT EXISTS idx_artifacts_agent ON artifacts(agent);
CREATE INDEX IF NOT EXISTS idx_artifacts_session ON artifacts(session_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_memory ON artifacts(memory_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_hash ON artifacts(sha256);

-- Full-text search
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='id'
);

-- Triggers for FTS sync
CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE OF content ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
END;
