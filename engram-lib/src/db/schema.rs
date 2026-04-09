use crate::Result;
use libsql::Connection;

/// Create all tables, indexes, FTS virtual tables, triggers, vector indexes,
/// and default data. Idempotent -- safe to run on every startup.
///
/// Table definitions match the TypeScript Engram schema as the source of truth
/// (base.ts, episodes.ts, fts.ts, intelligence.ts, services.ts, tier4.ts).
/// Rust-only service tables (Chiasm, Axon, Broca, Thymus, Loom, Gate, Sessions)
/// are appended after the TS-parity section.
pub async fn create_tables(conn: &Connection) -> Result<()> {
    // =========================================================================
    // Core tables (base.ts)
    // =========================================================================
    conn.execute_batch(
        "
        -- Users
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE NOT NULL,
            email TEXT,
            role TEXT NOT NULL DEFAULT 'admin',
            is_admin BOOLEAN NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Spaces
        CREATE TABLE IF NOT EXISTS spaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, name)
        );
        CREATE INDEX IF NOT EXISTS idx_spaces_user ON spaces(user_id);

        -- Memories (TS base + Rust extensions for FSRS, valence, vector, decay)
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
            user_id INTEGER NOT NULL DEFAULT 1,
            space_id INTEGER,
            -- FSRS-6 columns
            fsrs_stability REAL,
            fsrs_difficulty REAL,
            fsrs_storage_strength REAL DEFAULT 1.0,
            fsrs_retrieval_strength REAL DEFAULT 1.0,
            fsrs_learning_state INTEGER DEFAULT 0,
            fsrs_reps INTEGER DEFAULT 0,
            fsrs_lapses INTEGER DEFAULT 0,
            fsrs_last_review_at TEXT,
            -- Emotional valence
            valence REAL,
            arousal REAL,
            dominant_emotion TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_memories_root ON memories(root_memory_id);
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
        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id);
        CREATE INDEX IF NOT EXISTS idx_memories_fsrs_stability ON memories(fsrs_stability) WHERE fsrs_stability IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_sync_id ON memories(sync_id) WHERE sync_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_valence ON memories(valence) WHERE valence IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_is_fact ON memories(is_fact) WHERE is_fact = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_parent_fact ON memories(parent_memory_id) WHERE is_fact = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_not_decomposed ON memories(is_decomposed) WHERE is_decomposed = 0 AND is_fact = 0;

        -- Memory links (TS base.ts)
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

        -- API keys (TS base.ts + Rust agent_id/expires_at extensions)
        CREATE TABLE IF NOT EXISTS api_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            key_prefix TEXT NOT NULL,
            key_hash TEXT NOT NULL,
            name TEXT NOT NULL DEFAULT 'default',
            scopes TEXT NOT NULL DEFAULT 'read,write',
            rate_limit INTEGER NOT NULL DEFAULT 1000,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            agent_id INTEGER REFERENCES agents(id),
            last_used_at TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_api_keys_prefix ON api_keys(key_prefix);
        CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);
        CREATE INDEX IF NOT EXISTS idx_api_keys_expires ON api_keys(expires_at) WHERE expires_at IS NOT NULL;

        -- Audit log (TS base.ts + Rust agent_id/execution_hash/signature extensions)
        CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER,
            agent_id INTEGER,
            action TEXT NOT NULL,
            target_type TEXT,
            target_id INTEGER,
            details TEXT,
            ip TEXT,
            request_id TEXT,
            execution_hash TEXT,
            signature TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_log(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);
        CREATE INDEX IF NOT EXISTS idx_audit_target ON audit_log(target_type, target_id);
        CREATE INDEX IF NOT EXISTS idx_audit_agent ON audit_log(agent_id);
        ",
    )
    .await?;

    // =========================================================================
    // Episodes and conversations (episodes.ts)
    // =========================================================================
    conn.execute_batch(
        "
        -- Episodes (TS episodes.ts + Rust embedding/FSRS/decay extensions)
        CREATE TABLE IF NOT EXISTS episodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT,
            session_id TEXT,
            agent TEXT,
            summary TEXT,
            user_id INTEGER DEFAULT 1,
            memory_count INTEGER NOT NULL DEFAULT 0,
            embedding BLOB,
            embedding_vec_1024 FLOAT32(1024),
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
        CREATE INDEX IF NOT EXISTS idx_episodes_user ON episodes(user_id);
        CREATE INDEX IF NOT EXISTS idx_episodes_agent ON episodes(agent);

        -- Conversations (TS episodes.ts + Rust user_id extension)
        CREATE TABLE IF NOT EXISTS conversations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            session_id TEXT,
            title TEXT,
            metadata TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            started_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_conversations_agent ON conversations(agent);
        CREATE INDEX IF NOT EXISTS idx_conversations_session ON conversations(session_id);
        CREATE INDEX IF NOT EXISTS idx_conversations_user ON conversations(user_id);

        -- Messages (TS episodes.ts)
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
        ",
    )
    .await?;

    // =========================================================================
    // Intelligence tables (intelligence.ts)
    // =========================================================================
    conn.execute_batch(
        "
        -- Consolidation tracking (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS consolidations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            summary_memory_id INTEGER NOT NULL REFERENCES memories(id),
            source_memory_ids TEXT NOT NULL,
            cluster_label TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Entities (TS intelligence.ts -- Rust uses entity_type/aliases for code compat)
        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            entity_type TEXT NOT NULL DEFAULT 'generic',
            description TEXT,
            aliases TEXT,
            metadata TEXT,
            user_id INTEGER DEFAULT 1,
            space_id INTEGER,
            confidence REAL NOT NULL DEFAULT 1.0,
            occurrence_count INTEGER NOT NULL DEFAULT 1,
            first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, entity_type, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
        CREATE INDEX IF NOT EXISTS idx_entities_user ON entities(user_id);

        -- Entity relationships (TS intelligence.ts -- Rust uses relationship_type for code compat)
        CREATE TABLE IF NOT EXISTS entity_relationships (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relationship_type TEXT NOT NULL DEFAULT 'related',
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_count INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(source_entity_id, target_entity_id, relationship_type)
        );
        CREATE INDEX IF NOT EXISTS idx_entrel_source ON entity_relationships(source_entity_id);
        CREATE INDEX IF NOT EXISTS idx_entrel_target ON entity_relationships(target_entity_id);

        -- Memory-entity join (TS intelligence.ts + Rust salience/created_at extensions)
        CREATE TABLE IF NOT EXISTS memory_entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            salience REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(memory_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_me_entity ON memory_entities(entity_id);
        CREATE INDEX IF NOT EXISTS idx_memory_entities_memory ON memory_entities(memory_id);

        -- Projects (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            metadata TEXT,
            user_id INTEGER DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_projects_user ON projects(user_id);
        CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);

        -- Memory-project join (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS memory_projects (
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            PRIMARY KEY (memory_id, project_id)
        );
        CREATE INDEX IF NOT EXISTS idx_mp_project ON memory_projects(project_id);

        -- Structured facts (TS intelligence.ts + Rust valid_at/invalid_at extensions)
        CREATE TABLE IF NOT EXISTS structured_facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            subject TEXT NOT NULL,
            verb TEXT NOT NULL,
            object TEXT,
            quantity REAL,
            unit TEXT,
            date_ref TEXT,
            date_approx TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            user_id INTEGER DEFAULT 1,
            valid_at TEXT,
            invalid_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sf_memory ON structured_facts(memory_id);
        CREATE INDEX IF NOT EXISTS idx_sf_subject ON structured_facts(subject COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_sf_verb ON structured_facts(verb);
        CREATE INDEX IF NOT EXISTS idx_sf_date ON structured_facts(date_approx);
        CREATE INDEX IF NOT EXISTS idx_sf_user ON structured_facts(user_id);

        -- Current state (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS current_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            previous_value TEXT,
            previous_memory_id INTEGER,
            updated_count INTEGER NOT NULL DEFAULT 1,
            user_id INTEGER DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(key, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_cs_user ON current_state(user_id);

        -- User preferences (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS user_preferences (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            domain TEXT NOT NULL,
            preference TEXT NOT NULL,
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            user_id INTEGER DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(domain, preference, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_up_user ON user_preferences(user_id);

        -- Entity co-occurrences (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS entity_cooccurrences (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_a_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            entity_b_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            cooccurrence_count INTEGER NOT NULL DEFAULT 1,
            score REAL NOT NULL DEFAULT 0.0,
            last_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            user_id INTEGER DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(entity_a_id, entity_b_id, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_ec_entity_a ON entity_cooccurrences(entity_a_id);
        CREATE INDEX IF NOT EXISTS idx_ec_entity_b ON entity_cooccurrences(entity_b_id);
        CREATE INDEX IF NOT EXISTS idx_ec_score ON entity_cooccurrences(score DESC);
        CREATE INDEX IF NOT EXISTS idx_ec_user ON entity_cooccurrences(user_id);

        -- Digests (TS intelligence.ts -- webhook subscription model)
        CREATE TABLE IF NOT EXISTS digests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL DEFAULT 1,
            schedule TEXT NOT NULL DEFAULT 'daily',
            webhook_url TEXT NOT NULL,
            webhook_secret TEXT,
            include_stats BOOLEAN NOT NULL DEFAULT 1,
            include_new_memories BOOLEAN NOT NULL DEFAULT 1,
            include_contradictions BOOLEAN NOT NULL DEFAULT 1,
            include_reflections BOOLEAN NOT NULL DEFAULT 1,
            last_sent_at TEXT,
            next_send_at TEXT,
            active BOOLEAN NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_digests_next ON digests(next_send_at) WHERE active = 1;
        CREATE INDEX IF NOT EXISTS idx_digests_user ON digests(user_id);

        -- Reflections (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS reflections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL DEFAULT 1,
            content TEXT NOT NULL,
            themes TEXT,
            period_start TEXT NOT NULL,
            period_end TEXT NOT NULL,
            memory_count INTEGER NOT NULL DEFAULT 0,
            source_memory_ids TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_reflections_user ON reflections(user_id);
        CREATE INDEX IF NOT EXISTS idx_reflections_period ON reflections(period_end DESC);

        -- Personality signals (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS personality_signals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL DEFAULT 1,
            signal_type TEXT NOT NULL,
            subject TEXT NOT NULL,
            valence TEXT NOT NULL,
            intensity REAL DEFAULT 0.5,
            reasoning TEXT,
            source_text TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_personality_signals_user ON personality_signals(user_id);
        CREATE INDEX IF NOT EXISTS idx_personality_signals_type ON personality_signals(signal_type);
        CREATE INDEX IF NOT EXISTS idx_personality_signals_memory ON personality_signals(memory_id);

        -- Personality profiles (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS personality_profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL UNIQUE,
            profile TEXT NOT NULL,
            signal_count INTEGER NOT NULL DEFAULT 0,
            is_stale BOOLEAN NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_personality_profiles_user ON personality_profiles(user_id);

        -- Scratchpad (TS intelligence.ts)
        CREATE TABLE IF NOT EXISTS scratchpad (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL DEFAULT 1 REFERENCES users(id) ON DELETE CASCADE,
            session TEXT NOT NULL,
            agent TEXT NOT NULL,
            model TEXT NOT NULL,
            entry_key TEXT NOT NULL,
            value TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL DEFAULT (datetime('now', '+30 minutes')),
            UNIQUE(user_id, session, entry_key)
        );
        CREATE INDEX IF NOT EXISTS idx_scratchpad_user_expires ON scratchpad(user_id, expires_at);
        CREATE INDEX IF NOT EXISTS idx_scratchpad_session ON scratchpad(user_id, session);
        CREATE INDEX IF NOT EXISTS idx_scratchpad_agent ON scratchpad(user_id, agent);
        ",
    )
    .await?;

    // =========================================================================
    // Service tables (services.ts)
    // =========================================================================
    conn.execute_batch(
        "
        -- Agents (TS services.ts)
        CREATE TABLE IF NOT EXISTS agents (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
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

        -- Webhooks (TS services.ts)
        CREATE TABLE IF NOT EXISTS webhooks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            events TEXT NOT NULL DEFAULT '[\"*\"]',
            secret TEXT,
            user_id INTEGER DEFAULT 1,
            active BOOLEAN NOT NULL DEFAULT 1,
            last_triggered_at TEXT,
            failure_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);

        -- Rate limits (TS services.ts)
        CREATE TABLE IF NOT EXISTS rate_limits (
            key TEXT PRIMARY KEY,
            count INTEGER NOT NULL DEFAULT 0,
            window_start TEXT NOT NULL DEFAULT (datetime('now')),
            window_seconds INTEGER NOT NULL DEFAULT 60
        );
        CREATE INDEX IF NOT EXISTS idx_rate_limits_window ON rate_limits(window_start);

        -- Tenant quotas (TS services.ts)
        CREATE TABLE IF NOT EXISTS tenant_quotas (
            user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            max_memories INTEGER DEFAULT 10000,
            max_conversations INTEGER DEFAULT 1000,
            max_api_keys INTEGER DEFAULT 10,
            max_spaces INTEGER DEFAULT 5,
            max_memory_size_bytes INTEGER DEFAULT 102400,
            rate_limit_override INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Usage events (TS services.ts)
        CREATE TABLE IF NOT EXISTS usage_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            quantity INTEGER NOT NULL DEFAULT 1,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_usage_user_type ON usage_events(user_id, event_type, created_at);
        CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_events(created_at);

        -- Skill records (TS services.ts -- TEXT primary key)
        CREATE TABLE IF NOT EXISTS skill_records (
            skill_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            path TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            category TEXT NOT NULL DEFAULT 'workflow',
            origin TEXT NOT NULL DEFAULT 'imported',
            generation INTEGER NOT NULL DEFAULT 0,
            lineage_change_summary TEXT,
            creator_id TEXT,
            is_active INTEGER NOT NULL DEFAULT 1,
            total_selections INTEGER NOT NULL DEFAULT 0,
            total_applied INTEGER NOT NULL DEFAULT 0,
            total_completions INTEGER NOT NULL DEFAULT 0,
            embedding BLOB,
            first_seen TEXT NOT NULL DEFAULT (datetime('now')),
            last_updated TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
        CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
        CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active) WHERE is_active = 1;

        -- Skill lineage parents (TS services.ts)
        CREATE TABLE IF NOT EXISTS skill_lineage_parents (
            skill_id TEXT NOT NULL REFERENCES skill_records(skill_id) ON DELETE CASCADE,
            parent_skill_id TEXT NOT NULL,
            PRIMARY KEY (skill_id, parent_skill_id)
        );

        -- Skill tags (TS services.ts)
        CREATE TABLE IF NOT EXISTS skill_tags (
            skill_id TEXT NOT NULL REFERENCES skill_records(skill_id) ON DELETE CASCADE,
            tag TEXT NOT NULL,
            PRIMARY KEY (skill_id, tag)
        );
        CREATE INDEX IF NOT EXISTS idx_skill_tags_tag ON skill_tags(tag);

        -- Execution analyses (TS services.ts)
        CREATE TABLE IF NOT EXISTS execution_analyses (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL UNIQUE,
            timestamp TEXT NOT NULL,
            task_completed INTEGER NOT NULL DEFAULT 0,
            execution_note TEXT NOT NULL DEFAULT '',
            tool_issues TEXT NOT NULL DEFAULT '[]',
            candidate_for_evolution INTEGER NOT NULL DEFAULT 0,
            evolution_suggestions TEXT NOT NULL DEFAULT '[]',
            analyzed_by TEXT NOT NULL DEFAULT '',
            analyzed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_exec_analyses_task ON execution_analyses(task_id);
        CREATE INDEX IF NOT EXISTS idx_exec_analyses_candidate ON execution_analyses(candidate_for_evolution) WHERE candidate_for_evolution = 1;

        -- Skill judgments (TS services.ts)
        CREATE TABLE IF NOT EXISTS skill_judgments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            analysis_id INTEGER NOT NULL REFERENCES execution_analyses(id) ON DELETE CASCADE,
            skill_id TEXT NOT NULL,
            skill_applied INTEGER NOT NULL DEFAULT 0,
            note TEXT NOT NULL DEFAULT '',
            UNIQUE(analysis_id, skill_id)
        );
        CREATE INDEX IF NOT EXISTS idx_skill_judgments_skill ON skill_judgments(skill_id);

        -- Skill tool dependencies (TS services.ts)
        CREATE TABLE IF NOT EXISTS skill_tool_deps (
            skill_id TEXT NOT NULL REFERENCES skill_records(skill_id) ON DELETE CASCADE,
            tool_key TEXT NOT NULL,
            critical INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (skill_id, tool_key)
        );

        -- Tool quality records (TS services.ts)
        CREATE TABLE IF NOT EXISTS tool_quality_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_key TEXT NOT NULL UNIQUE,
            backend TEXT NOT NULL DEFAULT '',
            server TEXT NOT NULL DEFAULT 'default',
            tool_name TEXT NOT NULL DEFAULT '',
            description_hash TEXT NOT NULL DEFAULT '',
            total_calls INTEGER NOT NULL DEFAULT 0,
            total_successes INTEGER NOT NULL DEFAULT 0,
            total_failures INTEGER NOT NULL DEFAULT 0,
            avg_execution_ms REAL NOT NULL DEFAULT 0,
            llm_flagged_count INTEGER NOT NULL DEFAULT 0,
            quality_score REAL NOT NULL DEFAULT 1.0,
            last_execution_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_tool_quality_score ON tool_quality_records(quality_score);

        -- Artifacts (TS services.ts + Rust is_encrypted/is_indexed extensions)
        CREATE TABLE IF NOT EXISTS artifacts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            filename TEXT NOT NULL,
            mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
            size_bytes INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            storage_mode TEXT NOT NULL DEFAULT 'inline',
            data BLOB,
            disk_path TEXT,
            is_encrypted BOOLEAN NOT NULL DEFAULT 0,
            is_indexed BOOLEAN NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_artifacts_memory ON artifacts(memory_id);
        CREATE INDEX IF NOT EXISTS idx_artifacts_hash ON artifacts(sha256);
        ",
    )
    .await?;

    // =========================================================================
    // Tier-4 tables (tier4.ts)
    // =========================================================================
    conn.execute_batch(
        "
        -- Causal chains (TS tier4.ts)
        CREATE TABLE IF NOT EXISTS causal_chains (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_cc_user ON causal_chains(user_id);

        -- Causal links (TS tier4.ts)
        CREATE TABLE IF NOT EXISTS causal_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id INTEGER NOT NULL REFERENCES causal_chains(id) ON DELETE CASCADE,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            position INTEGER NOT NULL DEFAULT 0,
            role TEXT NOT NULL DEFAULT 'event',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(chain_id, memory_id)
        );
        CREATE INDEX IF NOT EXISTS idx_cl_chain ON causal_links(chain_id);
        CREATE INDEX IF NOT EXISTS idx_cl_memory ON causal_links(memory_id);

        -- Reconsolidations (TS tier4.ts)
        CREATE TABLE IF NOT EXISTS reconsolidations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            old_importance INTEGER,
            new_importance INTEGER,
            old_confidence REAL,
            new_confidence REAL,
            reason TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_recon_memory ON reconsolidations(memory_id);

        -- Temporal patterns (TS tier4.ts)
        CREATE TABLE IF NOT EXISTS temporal_patterns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL DEFAULT 1,
            day_of_week INTEGER NOT NULL,
            hour_of_day INTEGER NOT NULL,
            category TEXT,
            project_id INTEGER,
            access_count INTEGER NOT NULL DEFAULT 1,
            UNIQUE(user_id, day_of_week, hour_of_day, category, project_id)
        );
        CREATE INDEX IF NOT EXISTS idx_tp_user ON temporal_patterns(user_id, day_of_week, hour_of_day);
        ",
    )
    .await?;

    // =========================================================================
    // Rust-only service tables (Chiasm, Axon, Broca, Thymus, Loom, Gate, Sessions)
    // No TS equivalent -- these are Engram Rust originals.
    // =========================================================================
    conn.execute_batch(
        "
        -- Chiasm: Task tracking
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

        -- Axon: Event bus
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

        -- Broca: Action log
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

        -- Thymus: Quality scoring
        CREATE TABLE IF NOT EXISTS rubrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            description TEXT,
            criteria TEXT NOT NULL DEFAULT '[]',
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_user_name ON rubrics(user_id, name);
        CREATE INDEX IF NOT EXISTS idx_rubrics_user ON rubrics(user_id);

        CREATE TABLE IF NOT EXISTS evaluations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            rubric_id INTEGER NOT NULL REFERENCES rubrics(id),
            agent TEXT NOT NULL,
            subject TEXT NOT NULL,
            input TEXT NOT NULL DEFAULT '{}',
            output TEXT NOT NULL DEFAULT '{}',
            scores TEXT NOT NULL DEFAULT '{}',
            overall_score REAL NOT NULL,
            notes TEXT,
            evaluator TEXT NOT NULL,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_evaluations_agent_created ON evaluations(agent, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_evaluations_rubric_created ON evaluations(rubric_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_evaluations_user ON evaluations(user_id);

        CREATE TABLE IF NOT EXISTS quality_metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            metric TEXT NOT NULL,
            value REAL NOT NULL,
            tags TEXT NOT NULL DEFAULT '{}',
            user_id INTEGER NOT NULL DEFAULT 1,
            recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_quality_metrics_agent_metric ON quality_metrics(agent, metric, recorded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_quality_metrics_user ON quality_metrics(user_id);

        -- Thymus: Session quality
        CREATE TABLE IF NOT EXISTS session_quality (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            turn_count INTEGER DEFAULT 0,
            rules_followed TEXT DEFAULT '[]',
            rules_drifted TEXT DEFAULT '[]',
            personality_score REAL,
            rule_compliance_rate REAL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        -- Thymus: Behavioral drift
        CREATE TABLE IF NOT EXISTS behavioral_drift_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            session_id TEXT,
            drift_type TEXT NOT NULL,
            severity TEXT DEFAULT 'low',
            signal TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        -- Loom: Workflow definitions
        CREATE TABLE IF NOT EXISTS loom_workflows (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            description TEXT,
            steps TEXT NOT NULL DEFAULT '[]',
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, name)
        );
        CREATE INDEX IF NOT EXISTS idx_loom_workflows_user ON loom_workflows(user_id);

        -- Loom: Workflow runs
        CREATE TABLE IF NOT EXISTS loom_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            workflow_id INTEGER NOT NULL REFERENCES loom_workflows(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'pending',
            input TEXT NOT NULL DEFAULT '{}',
            output TEXT NOT NULL DEFAULT '{}',
            error TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            started_at TEXT,
            completed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_loom_runs_workflow ON loom_runs(workflow_id);
        CREATE INDEX IF NOT EXISTS idx_loom_runs_status ON loom_runs(status);
        CREATE INDEX IF NOT EXISTS idx_loom_runs_user ON loom_runs(user_id);

        -- Loom: Workflow steps
        CREATE TABLE IF NOT EXISTS loom_steps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id INTEGER NOT NULL REFERENCES loom_runs(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            type TEXT NOT NULL,
            config TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'pending',
            input TEXT NOT NULL DEFAULT '{}',
            output TEXT NOT NULL DEFAULT '{}',
            error TEXT,
            depends_on TEXT NOT NULL DEFAULT '[]',
            retry_count INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            timeout_ms INTEGER NOT NULL DEFAULT 30000,
            started_at TEXT,
            completed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_loom_steps_run ON loom_steps(run_id);
        CREATE INDEX IF NOT EXISTS idx_loom_steps_status ON loom_steps(status);

        -- Loom: Run logs
        CREATE TABLE IF NOT EXISTS loom_run_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id INTEGER NOT NULL REFERENCES loom_runs(id) ON DELETE CASCADE,
            step_id INTEGER REFERENCES loom_steps(id) ON DELETE SET NULL,
            level TEXT NOT NULL DEFAULT 'info',
            message TEXT NOT NULL,
            data TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_loom_run_logs_run ON loom_run_logs(run_id);

        -- Gate requests
        CREATE TABLE IF NOT EXISTS gate_requests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            agent TEXT NOT NULL,
            command TEXT NOT NULL,
            context TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            reason TEXT,
            output TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_gate_requests_user ON gate_requests(user_id, status);

        -- Sessions (agent activity sessions with output buffering)
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent TEXT NOT NULL,
            user_id INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id, status);

        -- Session output lines
        CREATE TABLE IF NOT EXISTS session_output (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            line TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_session_output_session ON session_output(session_id);
        ",
    )
    .await?;

    // =========================================================================
    // FTS virtual tables (fts.ts, episodes.ts, services.ts)
    // =========================================================================

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            content, category, source,
            content='memories', content_rowid='id',
            tokenize='porter unicode61'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, content, category, source)
            VALUES (new.id, new.content, new.category, new.source);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, category, source)
            VALUES ('delete', old.id, old.content, old.category, old.source);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, category, source)
            VALUES ('delete', old.id, old.content, old.category, old.source);
            INSERT INTO memories_fts(rowid, content, category, source)
            VALUES (new.id, new.content, new.category, new.source);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
            title, summary,
            content='episodes', content_rowid='id',
            tokenize='porter unicode61'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS episodes_fts_ai AFTER INSERT ON episodes BEGIN
            INSERT INTO episodes_fts(rowid, title, summary)
            VALUES (new.id, new.title, new.summary);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS episodes_fts_ad AFTER DELETE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, title, summary)
            VALUES ('delete', old.id, old.title, old.summary);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS episodes_fts_au AFTER UPDATE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, title, summary)
            VALUES ('delete', old.id, old.title, old.summary);
            INSERT INTO episodes_fts(rowid, title, summary)
            VALUES (new.id, new.title, new.summary);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
            content, role,
            content='messages', content_rowid='id',
            tokenize='porter unicode61'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content, role)
            VALUES (new.id, new.content, new.role);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content, role)
            VALUES ('delete', old.id, old.content, old.role);
        END",
        (),
    )
    .await?;

    // Skills FTS (TS services.ts -- columns: name, description, content)
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
            name, description, content,
            content='skill_records', content_rowid='rowid',
            tokenize='porter unicode61'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS skills_fts_ai AFTER INSERT ON skill_records BEGIN
            INSERT INTO skills_fts(rowid, name, description, content)
            VALUES (new.rowid, new.name, new.description, new.content);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS skills_fts_ad AFTER DELETE ON skill_records BEGIN
            INSERT INTO skills_fts(skills_fts, rowid, name, description, content)
            VALUES ('delete', old.rowid, old.name, old.description, old.content);
        END",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS skills_fts_au AFTER UPDATE ON skill_records BEGIN
            INSERT INTO skills_fts(skills_fts, rowid, name, description, content)
            VALUES ('delete', old.rowid, old.name, old.description, old.content);
            INSERT INTO skills_fts(rowid, name, description, content)
            VALUES (new.rowid, new.name, new.description, new.content);
        END",
        (),
    )
    .await?;

    // Artifacts FTS (TS services.ts -- standalone, no content sync)
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS artifacts_fts USING fts5(
            content,
            tokenize='porter unicode61'
        )",
        (),
    )
    .await?;

    // =========================================================================
    // Cross-tenant link prevention trigger (fts.ts)
    // =========================================================================

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS prevent_cross_tenant_links
        BEFORE INSERT ON memory_links
        BEGIN
            SELECT CASE
                WHEN (SELECT user_id FROM memories WHERE id = NEW.source_id) !=
                     (SELECT user_id FROM memories WHERE id = NEW.target_id)
                THEN RAISE(ABORT, 'Cross-tenant memory link rejected')
            END;
        END",
        (),
    )
    .await?;

    // =========================================================================
    // Vector indexes -- must use individual execute() calls
    // =========================================================================

    conn.execute(
        "CREATE INDEX IF NOT EXISTS memories_vec_1024_idx ON memories(libsql_vector_idx(embedding_vec_1024))",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS episodes_vec_1024_idx ON episodes(libsql_vector_idx(embedding_vec_1024))",
        (),
    )
    .await?;

    // =========================================================================
    // Default data: user 'owner' and 'default' space
    // =========================================================================

    conn.execute(
        "INSERT OR IGNORE INTO users (id, username, role, is_admin) VALUES (1, 'owner', 'admin', 1)",
        (),
    )
    .await?;

    conn.execute(
        "INSERT OR IGNORE INTO spaces (user_id, name, description) VALUES (1, 'default', 'Default memory space')",
        (),
    )
    .await?;

    Ok(())
}
