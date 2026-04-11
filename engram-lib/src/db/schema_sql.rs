pub const CORE_SCHEMA_SQL: &str = r#"
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

        -- Memories (main table -- all columns from final state)
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
            is_superseded INTEGER NOT NULL DEFAULT 0,
            is_consolidated INTEGER NOT NULL DEFAULT 0,
            community_id INTEGER,
            -- Emotional valence
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
        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id);
        CREATE INDEX IF NOT EXISTS idx_memories_fsrs_stability ON memories(fsrs_stability) WHERE fsrs_stability IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_sync_id ON memories(sync_id) WHERE sync_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_valence ON memories(valence) WHERE valence IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_is_fact ON memories(is_fact) WHERE is_fact = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_parent_fact ON memories(parent_memory_id) WHERE is_fact = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_not_decomposed ON memories(is_decomposed) WHERE is_decomposed = 0 AND is_fact = 0;
        CREATE INDEX IF NOT EXISTS idx_memories_community ON memories(community_id) WHERE community_id IS NOT NULL;

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

        -- Agents (Soma) -- defined before api_keys which references it
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

        -- API keys
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

        -- Audit log
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

        -- Episodes
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

        -- Conversations
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
        CREATE INDEX IF NOT EXISTS idx_conv_started ON conversations(started_at DESC);

        -- Messages
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_msg_conv ON messages(conversation_id, created_at);

        -- Webhooks
        CREATE TABLE IF NOT EXISTS webhooks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            url TEXT NOT NULL,
            events TEXT NOT NULL DEFAULT 'memory.created',
            secret TEXT,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            active BOOLEAN NOT NULL DEFAULT 1,
            last_triggered_at TEXT,
            failure_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);
        CREATE INDEX IF NOT EXISTS idx_webhooks_active ON webhooks(is_active);

        -- Rate limits
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

        -- Tenant quotas
        CREATE TABLE IF NOT EXISTS tenant_quotas (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
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

        -- Usage events
        CREATE TABLE IF NOT EXISTS usage_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            agent_id INTEGER REFERENCES agents(id),
            event_type TEXT NOT NULL,
            quantity INTEGER NOT NULL DEFAULT 1,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_usage_user ON usage_events(user_id);
        CREATE INDEX IF NOT EXISTS idx_usage_type ON usage_events(event_type);
        CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_events(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_usage_user_type ON usage_events(user_id, event_type, created_at);

        -- Intelligence: consolidations
        CREATE TABLE IF NOT EXISTS consolidations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_ids TEXT NOT NULL,
            result_memory_id INTEGER REFERENCES memories(id),
            summary_memory_id INTEGER REFERENCES memories(id),
            source_memory_ids TEXT,
            cluster_label TEXT,
            strategy TEXT NOT NULL DEFAULT 'merge',
            confidence REAL NOT NULL DEFAULT 1.0,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_consolidations_user ON consolidations(user_id);

        -- Entities
        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            entity_type TEXT NOT NULL DEFAULT 'concept',
            type TEXT NOT NULL DEFAULT 'generic',
            description TEXT,
            aliases TEXT,
            aka TEXT,
            metadata TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            space_id INTEGER,
            confidence REAL NOT NULL DEFAULT 1.0,
            occurrence_count INTEGER NOT NULL DEFAULT 1,
            first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, entity_type, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
        CREATE INDEX IF NOT EXISTS idx_entities_user ON entities(user_id);

        -- Entity relationships
        CREATE TABLE IF NOT EXISTS entity_relationships (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relationship_type TEXT NOT NULL DEFAULT 'related',
            relationship TEXT NOT NULL DEFAULT 'related',
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_count INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(source_entity_id, target_entity_id, relationship_type)
        );
        CREATE INDEX IF NOT EXISTS idx_entity_rel_source ON entity_relationships(source_entity_id);
        CREATE INDEX IF NOT EXISTS idx_entity_rel_target ON entity_relationships(target_entity_id);

        -- Memory-entity join
        CREATE TABLE IF NOT EXISTS memory_entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            salience REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(memory_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_memory_entities_memory ON memory_entities(memory_id);
        CREATE INDEX IF NOT EXISTS idx_memory_entities_entity ON memory_entities(entity_id);

        -- Projects
        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            user_id INTEGER NOT NULL DEFAULT 1,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_projects_user ON projects(user_id);
        CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);

        -- Memory-project join
        CREATE TABLE IF NOT EXISTS memory_projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(memory_id, project_id)
        );
        CREATE INDEX IF NOT EXISTS idx_memory_projects_memory ON memory_projects(memory_id);
        CREATE INDEX IF NOT EXISTS idx_memory_projects_project ON memory_projects(project_id);

        -- Structured facts
        CREATE TABLE IF NOT EXISTS structured_facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
            subject TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object TEXT NOT NULL,
            verb TEXT NOT NULL DEFAULT '',
            quantity REAL,
            unit TEXT,
            date_ref TEXT,
            date_approx TEXT,
            location TEXT,
            context TEXT,
            episode_id INTEGER,
            valid_at TEXT,
            invalid_at TEXT,
            invalidated_by INTEGER,
            confidence REAL NOT NULL DEFAULT 1.0,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_facts_subject ON structured_facts(subject);
        CREATE INDEX IF NOT EXISTS idx_facts_predicate ON structured_facts(predicate);
        CREATE INDEX IF NOT EXISTS idx_facts_memory ON structured_facts(memory_id);
        CREATE INDEX IF NOT EXISTS idx_facts_user ON structured_facts(user_id);
        CREATE INDEX IF NOT EXISTS idx_sf_verb ON structured_facts(verb);
        CREATE INDEX IF NOT EXISTS idx_sf_date ON structured_facts(date_approx);
        CREATE INDEX IF NOT EXISTS idx_sf_episode ON structured_facts(episode_id) WHERE episode_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_sf_location ON structured_facts(location COLLATE NOCASE) WHERE location IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_sf_valid ON structured_facts(valid_at) WHERE valid_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_sf_invalid ON structured_facts(invalid_at) WHERE invalid_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_sf_subject_verb ON structured_facts(subject COLLATE NOCASE, verb, user_id);
        CREATE INDEX IF NOT EXISTS idx_facts_user_subject_predicate ON structured_facts(user_id, subject, predicate);

        -- Current state (per-agent key-value store)
        CREATE TABLE IF NOT EXISTS current_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            previous_value TEXT,
            previous_memory_id INTEGER,
            updated_count INTEGER NOT NULL DEFAULT 1,
            user_id INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(agent, key, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
        CREATE INDEX IF NOT EXISTS idx_current_state_user ON current_state(user_id);
        CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_cs_key_user ON current_state(key, user_id);

        -- User preferences
        CREATE TABLE IF NOT EXISTS user_preferences (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            domain TEXT,
            preference TEXT,
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, key)
        );
        CREATE INDEX IF NOT EXISTS idx_user_prefs_user ON user_preferences(user_id);
        CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref_user ON user_preferences(domain, preference, user_id);

        -- Entity co-occurrences
        CREATE TABLE IF NOT EXISTS entity_cooccurrences (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_a_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            entity_b_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            count INTEGER NOT NULL DEFAULT 1,
            cooccurrence_count INTEGER NOT NULL DEFAULT 1,
            score REAL NOT NULL DEFAULT 0.0,
            last_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
            user_id INTEGER DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(entity_a_id, entity_b_id)
        );
        CREATE INDEX IF NOT EXISTS idx_cooccurrences_a ON entity_cooccurrences(entity_a_id);
        CREATE INDEX IF NOT EXISTS idx_cooccurrences_b ON entity_cooccurrences(entity_b_id);
        CREATE INDEX IF NOT EXISTS idx_ec_score ON entity_cooccurrences(score DESC);
        CREATE INDEX IF NOT EXISTS idx_ec_user ON entity_cooccurrences(user_id);

        -- Tier4: causal chains
        CREATE TABLE IF NOT EXISTS causal_chains (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            root_memory_id INTEGER REFERENCES memories(id),
            description TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_causal_chains_user ON causal_chains(user_id);

        -- Tier4: causal links
        CREATE TABLE IF NOT EXISTS causal_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id INTEGER NOT NULL REFERENCES causal_chains(id) ON DELETE CASCADE,
            cause_memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            effect_memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            strength REAL NOT NULL DEFAULT 1.0,
            order_index INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_causal_links_chain ON causal_links(chain_id);

        -- Tier4: reconsolidations
        CREATE TABLE IF NOT EXISTS reconsolidations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            old_content TEXT NOT NULL,
            new_content TEXT NOT NULL,
            reason TEXT,
            triggered_by INTEGER REFERENCES memories(id),
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_reconsolidations_memory ON reconsolidations(memory_id);

        -- Tier4: temporal patterns
        CREATE TABLE IF NOT EXISTS temporal_patterns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            pattern_type TEXT NOT NULL DEFAULT 'daily',
            description TEXT NOT NULL,
            memory_ids TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            recurrence TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_temporal_patterns_user ON temporal_patterns(user_id);

        -- Tier4: digests
        CREATE TABLE IF NOT EXISTS digests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            period TEXT NOT NULL DEFAULT 'daily',
            content TEXT NOT NULL,
            memory_count INTEGER NOT NULL DEFAULT 0,
            user_id INTEGER NOT NULL DEFAULT 1,
            schedule TEXT NOT NULL DEFAULT 'daily',
            webhook_url TEXT,
            webhook_secret TEXT,
            include_stats BOOLEAN NOT NULL DEFAULT 1,
            include_new_memories BOOLEAN NOT NULL DEFAULT 1,
            include_contradictions BOOLEAN NOT NULL DEFAULT 1,
            include_reflections BOOLEAN NOT NULL DEFAULT 1,
            last_sent_at TEXT,
            next_send_at TEXT,
            active BOOLEAN NOT NULL DEFAULT 1,
            failure_count INTEGER NOT NULL DEFAULT 0,
            started_at TEXT,
            ended_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_digests_user ON digests(user_id);
        CREATE INDEX IF NOT EXISTS idx_digests_period ON digests(period);
        CREATE INDEX IF NOT EXISTS idx_digests_next ON digests(next_send_at) WHERE active = 1;

        -- Sessions
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent TEXT NOT NULL,
            user_id INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);

        -- Session output lines
        CREATE TABLE IF NOT EXISTS session_output (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            line TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_session_output_session ON session_output(session_id);

        -- Gate requests (command approval flow)
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
        CREATE INDEX IF NOT EXISTS idx_gate_requests_user ON gate_requests(user_id);
        CREATE INDEX IF NOT EXISTS idx_gate_requests_status ON gate_requests(status);

        -- Memory feedback (intelligence endpoints)
        CREATE TABLE IF NOT EXISTS memory_feedback (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL,
            rating TEXT NOT NULL,
            context TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);
        CREATE INDEX IF NOT EXISTS idx_feedback_memory ON memory_feedback(memory_id);

        -- Tier4: reflections
        CREATE TABLE IF NOT EXISTS reflections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            reflection_type TEXT NOT NULL DEFAULT 'insight',
            themes TEXT,
            period_start TEXT,
            period_end TEXT,
            source_memory_ids TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_reflections_user ON reflections(user_id);
        CREATE INDEX IF NOT EXISTS idx_reflections_type ON reflections(reflection_type);
        CREATE INDEX IF NOT EXISTS idx_reflections_period ON reflections(period_end DESC);

        -- Tier4: personality signals
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

        -- Tier4: personality profiles
        CREATE TABLE IF NOT EXISTS personality_profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
            traits TEXT NOT NULL DEFAULT '{}',
            profile TEXT NOT NULL DEFAULT '{}',
            signal_count INTEGER NOT NULL DEFAULT 0,
            is_stale BOOLEAN NOT NULL DEFAULT 0,
            last_updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_personality_profiles_user ON personality_profiles(user_id);

        -- Tier4: scratchpad
        CREATE TABLE IF NOT EXISTS scratchpad (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL DEFAULT 'unknown',
            session TEXT NOT NULL DEFAULT 'default',
            model TEXT NOT NULL DEFAULT '',
            entry_key TEXT NOT NULL,
            value TEXT NOT NULL DEFAULT '',
            expires_at TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, session, entry_key)
        );
        CREATE INDEX IF NOT EXISTS idx_scratchpad_agent ON scratchpad(agent);
        CREATE INDEX IF NOT EXISTS idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_scratchpad_user_expires ON scratchpad(user_id, expires_at);
        CREATE INDEX IF NOT EXISTS idx_scratchpad_session ON scratchpad(user_id, session);

        -- Skill system: skill records
        CREATE TABLE IF NOT EXISTS skill_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id TEXT UNIQUE,
            name TEXT NOT NULL,
            agent TEXT NOT NULL,
            description TEXT,
            code TEXT NOT NULL,
            path TEXT,
            content TEXT NOT NULL DEFAULT '',
            category TEXT NOT NULL DEFAULT 'workflow',
            origin TEXT NOT NULL DEFAULT 'imported',
            generation INTEGER NOT NULL DEFAULT 0,
            lineage_change_summary TEXT,
            creator_id TEXT,
            language TEXT NOT NULL DEFAULT 'javascript',
            version INTEGER NOT NULL DEFAULT 1,
            parent_skill_id INTEGER REFERENCES skill_records(id),
            root_skill_id INTEGER REFERENCES skill_records(id),
            embedding BLOB,
            embedding_vec_1024 FLOAT32(1024),
            trust_score REAL NOT NULL DEFAULT 50,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0,
            execution_count INTEGER NOT NULL DEFAULT 0,
            avg_duration_ms REAL,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            is_deprecated BOOLEAN NOT NULL DEFAULT 0,
            total_selections INTEGER NOT NULL DEFAULT 0,
            total_applied INTEGER NOT NULL DEFAULT 0,
            total_completions INTEGER NOT NULL DEFAULT 0,
            visibility TEXT NOT NULL DEFAULT 'private',
            lineage_source_task_id TEXT,
            lineage_content_diff TEXT NOT NULL DEFAULT '',
            lineage_content_snapshot TEXT NOT NULL DEFAULT '{}',
            total_fallbacks INTEGER NOT NULL DEFAULT 0,
            metadata TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            first_seen TEXT NOT NULL DEFAULT (datetime('now')),
            last_updated TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, agent, version, user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
        CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
        CREATE INDEX IF NOT EXISTS idx_skill_records_user ON skill_records(user_id);
        CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
        CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
        CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

        -- Skill lineage parents (many-to-many)
        CREATE TABLE IF NOT EXISTS skill_lineage_parents (
            skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            parent_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            parent_skill_id TEXT,
            PRIMARY KEY (skill_id, parent_id)
        );

        -- Skill tags
        CREATE TABLE IF NOT EXISTS skill_tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            tag TEXT NOT NULL,
            UNIQUE(skill_id, tag)
        );
        CREATE INDEX IF NOT EXISTS idx_skill_tags_skill ON skill_tags(skill_id);
        CREATE INDEX IF NOT EXISTS idx_skill_tags_tag ON skill_tags(tag);

        -- Execution analyses
        CREATE TABLE IF NOT EXISTS execution_analyses (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            success BOOLEAN NOT NULL,
            duration_ms REAL,
            error_type TEXT,
            error_message TEXT,
            input_hash TEXT,
            output_hash TEXT,
            metadata TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_exec_analyses_skill ON execution_analyses(skill_id);

        -- Skill judgments
        CREATE TABLE IF NOT EXISTS skill_judgments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            judge_agent TEXT NOT NULL,
            score REAL NOT NULL,
            rationale TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_skill_judgments_skill ON skill_judgments(skill_id);

        -- Skill tool dependencies
        CREATE TABLE IF NOT EXISTS skill_tool_deps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
            tool_name TEXT NOT NULL,
            tool_key TEXT,
            critical INTEGER NOT NULL DEFAULT 0,
            is_optional BOOLEAN NOT NULL DEFAULT 0,
            UNIQUE(skill_id, tool_name)
        );
        CREATE INDEX IF NOT EXISTS idx_skill_tool_deps_skill ON skill_tool_deps(skill_id);

        -- Tool quality records
        CREATE TABLE IF NOT EXISTS tool_quality_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_key TEXT UNIQUE,
            backend TEXT NOT NULL DEFAULT '',
            server TEXT NOT NULL DEFAULT 'default',
            tool_name TEXT NOT NULL,
            description_hash TEXT NOT NULL DEFAULT '',
            total_calls INTEGER NOT NULL DEFAULT 0,
            total_successes INTEGER NOT NULL DEFAULT 0,
            total_failures INTEGER NOT NULL DEFAULT 0,
            avg_execution_ms REAL NOT NULL DEFAULT 0,
            llm_flagged_count INTEGER NOT NULL DEFAULT 0,
            quality_score REAL NOT NULL DEFAULT 1.0,
            last_execution_at TEXT,
            agent TEXT NOT NULL,
            success BOOLEAN NOT NULL,
            latency_ms REAL,
            error_type TEXT,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_tool_quality_tool ON tool_quality_records(tool_name);
        CREATE INDEX IF NOT EXISTS idx_tool_quality_agent ON tool_quality_records(agent);
        CREATE INDEX IF NOT EXISTS idx_tool_quality_score ON tool_quality_records(quality_score);

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
            user_id INTEGER NOT NULL DEFAULT 0,
            created_at TEXT DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);

        -- Thymus: Behavioral drift
        CREATE TABLE IF NOT EXISTS behavioral_drift_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            session_id TEXT,
            drift_type TEXT NOT NULL,
            severity TEXT DEFAULT 'low',
            signal TEXT NOT NULL,
            user_id INTEGER NOT NULL DEFAULT 0,
            created_at TEXT DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);

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

        -- Application-wide key/value state (bootstrap sentinel, feature flags, etc.)
        CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
"#;

pub const AUXILIARY_SCHEMA_STATEMENTS: &[&str] = &[
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            content, category, source,
            content='memories', content_rowid='id',
            tokenize='porter unicode61'
        )"#,
    r#"CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, content, category, source)
            VALUES (new.id, new.content, new.category, new.source);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, category, source)
            VALUES ('delete', old.id, old.content, old.category, old.source);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content, category, source)
            VALUES ('delete', old.id, old.content, old.category, old.source);
            INSERT INTO memories_fts(rowid, content, category, source)
            VALUES (new.id, new.content, new.category, new.source);
        END"#,
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
            title, summary, agent,
            content='episodes', content_rowid='id',
            tokenize='porter unicode61'
        )"#,
    r#"CREATE TRIGGER IF NOT EXISTS episodes_fts_insert AFTER INSERT ON episodes BEGIN
            INSERT INTO episodes_fts(rowid, title, summary, agent)
            VALUES (new.id, new.title, new.summary, new.agent);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS episodes_fts_delete AFTER DELETE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, title, summary, agent)
            VALUES ('delete', old.id, old.title, old.summary, old.agent);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS episodes_fts_update AFTER UPDATE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, title, summary, agent)
            VALUES ('delete', old.id, old.title, old.summary, old.agent);
            INSERT INTO episodes_fts(rowid, title, summary, agent)
            VALUES (new.id, new.title, new.summary, new.agent);
        END"#,
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
            content, role,
            content='messages', content_rowid='id',
            tokenize='porter unicode61'
        )"#,
    r#"CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content, role)
            VALUES (new.id, new.content, new.role);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content, role)
            VALUES ('delete', old.id, old.content, old.role);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content, role)
            VALUES ('delete', old.id, old.content, old.role);
            INSERT INTO messages_fts(rowid, content, role)
            VALUES (new.id, new.content, new.role);
        END"#,
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
            name, description, code,
            content='skill_records', content_rowid='id',
            tokenize='porter unicode61'
        )"#,
    r#"CREATE TRIGGER IF NOT EXISTS skills_fts_insert AFTER INSERT ON skill_records BEGIN
            INSERT INTO skills_fts(rowid, name, description, code)
            VALUES (new.id, new.name, new.description, new.code);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS skills_fts_delete AFTER DELETE ON skill_records BEGIN
            INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
            VALUES ('delete', old.id, old.name, old.description, old.code);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS skills_fts_update AFTER UPDATE ON skill_records BEGIN
            INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
            VALUES ('delete', old.id, old.name, old.description, old.code);
            INSERT INTO skills_fts(rowid, name, description, code)
            VALUES (new.id, new.name, new.description, new.code);
        END"#,
    r#"CREATE VIRTUAL TABLE IF NOT EXISTS artifacts_fts USING fts5(
            name, content,
            content='artifacts', content_rowid='id',
            tokenize='porter unicode61'
        )"#,
    r#"CREATE TRIGGER IF NOT EXISTS artifacts_fts_insert AFTER INSERT ON artifacts BEGIN
            INSERT INTO artifacts_fts(rowid, name, content)
            VALUES (new.id, new.name, new.content);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS artifacts_fts_delete AFTER DELETE ON artifacts BEGIN
            INSERT INTO artifacts_fts(artifacts_fts, rowid, name, content)
            VALUES ('delete', old.id, old.name, old.content);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS artifacts_fts_update AFTER UPDATE ON artifacts BEGIN
            INSERT INTO artifacts_fts(artifacts_fts, rowid, name, content)
            VALUES ('delete', old.id, old.name, old.content);
            INSERT INTO artifacts_fts(rowid, name, content)
            VALUES (new.id, new.name, new.content);
        END"#,
    r#"CREATE TRIGGER IF NOT EXISTS prevent_cross_tenant_links
        BEFORE INSERT ON memory_links
        BEGIN
            SELECT RAISE(ABORT, 'cross-tenant memory links are not permitted')
            WHERE (SELECT user_id FROM memories WHERE id = NEW.source_id)
               != (SELECT user_id FROM memories WHERE id = NEW.target_id);
        END"#,
    r#"INSERT OR IGNORE INTO users (id, username, role, is_admin) VALUES (1, 'owner', 'admin', 1)"#,
    r#"INSERT OR IGNORE INTO spaces (user_id, name) VALUES (1, 'default')"#,
];

pub const LIBSQL_VECTOR_INDEX_STATEMENTS: &[&str] = &[
    r#"CREATE INDEX IF NOT EXISTS memories_vec_1024_idx ON memories(libsql_vector_idx(embedding_vec_1024))"#,
    r#"CREATE INDEX IF NOT EXISTS episodes_vec_1024_idx ON episodes(libsql_vector_idx(embedding_vec_1024))"#,
    r#"CREATE INDEX IF NOT EXISTS skill_records_vec_1024_idx ON skill_records(libsql_vector_idx(embedding_vec_1024))"#,
];
