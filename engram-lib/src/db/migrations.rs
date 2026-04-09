use crate::Result;
use libsql::Connection;
use tracing::{info, warn};

/// Import data from an existing TypeScript Engram memory.db file.
///
/// Attaches the source database as `ts_src`, copies all rows with appropriate
/// column mappings, then detaches. The destination schema must already exist
/// (i.e. `schema::create_tables` has been called on `conn`).
///
/// Idempotent: uses INSERT OR IGNORE so re-running is safe (existing rows are
/// kept, new rows from the source are added).
pub async fn migrate_from_typescript(conn: &Connection, source_path: &str) -> Result<()> {
    info!(source = source_path, "starting TypeScript DB migration");

    // Attach the source database
    conn.execute(
        &format!("ATTACH DATABASE '{}' AS ts_src", source_path.replace('\'', "''")),
        (),
    )
    .await?;

    // Wrap all inserts in a transaction for performance
    conn.execute("BEGIN", ()).await?;

    let result = migrate_tables(conn).await;

    match &result {
        Ok(()) => {
            conn.execute("COMMIT", ()).await?;
            info!("TypeScript DB migration committed successfully");
        }
        Err(e) => {
            warn!(error = %e, "TypeScript DB migration failed, rolling back");
            let _ = conn.execute("ROLLBACK", ()).await;
        }
    }

    // Always detach, even on error
    let _ = conn.execute("DETACH DATABASE ts_src", ()).await;

    result
}

/// Run all table-by-table migrations inside an already-open transaction.
async fn migrate_tables(conn: &Connection) -> Result<()> {
    // Helper: run a migration SQL, log table name, skip gracefully if the
    // source table does not exist in the TS database.
    macro_rules! migrate_table {
        ($conn:expr, $table:expr, $sql:expr) => {
            match $conn.execute($sql, ()).await {
                Ok(n) => info!(table = $table, rows = n, "migrated"),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("no such table") {
                        info!(table = $table, "source table not present, skipping");
                    } else {
                        warn!(table = $table, error = %e, "migration error");
                        return Err(crate::EngError::Internal(
                            format!("migration of {} failed: {}", $table, e),
                        ));
                    }
                }
            }
        };
    }

    // -----------------------------------------------------------------
    // Core tables (base.ts)
    // -----------------------------------------------------------------

    migrate_table!(conn, "users",
        "INSERT OR IGNORE INTO main.users (id, username, email, role, is_admin, created_at)
         SELECT id, username, email, role, is_admin, created_at
         FROM ts_src.users"
    );

    migrate_table!(conn, "spaces",
        "INSERT OR IGNORE INTO main.spaces (id, user_id, name, description, created_at)
         SELECT id, user_id, name, description, created_at
         FROM ts_src.spaces"
    );

    migrate_table!(conn, "memories",
        "INSERT OR IGNORE INTO main.memories (
            id, content, category, source, session_id, importance,
            embedding, version, is_latest, parent_memory_id, root_memory_id,
            source_count, is_static, is_forgotten, forget_after, forget_reason,
            is_inference, created_at, updated_at
         )
         SELECT
            id, content, category, source, session_id, importance,
            embedding, version, is_latest, parent_memory_id, root_memory_id,
            source_count, is_static, is_forgotten, forget_after, forget_reason,
            is_inference, created_at, updated_at
         FROM ts_src.memories"
    );

    migrate_table!(conn, "api_keys",
        "INSERT OR IGNORE INTO main.api_keys (
            id, user_id, key_prefix, key_hash, name, scopes,
            rate_limit, is_active, last_used_at, created_at
         )
         SELECT
            id, user_id, key_prefix, key_hash, name, scopes,
            rate_limit, is_active, last_used_at, created_at
         FROM ts_src.api_keys"
    );

    migrate_table!(conn, "memory_links",
        "INSERT OR IGNORE INTO main.memory_links (
            id, source_id, target_id, similarity, type, created_at
         )
         SELECT id, source_id, target_id, similarity, type, created_at
         FROM ts_src.memory_links"
    );

    migrate_table!(conn, "audit_log",
        "INSERT OR IGNORE INTO main.audit_log (
            id, user_id, action, target_type, target_id, details, ip, request_id, created_at
         )
         SELECT id, user_id, action, target_type, target_id, details, ip, request_id, created_at
         FROM ts_src.audit_log"
    );

    // -----------------------------------------------------------------
    // Episode tables (episodes.ts)
    // -----------------------------------------------------------------

    migrate_table!(conn, "episodes",
        "INSERT OR IGNORE INTO main.episodes (
            id, title, session_id, agent, summary, user_id,
            memory_count, started_at, ended_at, created_at
         )
         SELECT id, title, session_id, agent, summary, user_id,
                memory_count, started_at, ended_at, created_at
         FROM ts_src.episodes"
    );

    migrate_table!(conn, "conversations",
        "INSERT OR IGNORE INTO main.conversations (
            id, agent, session_id, title, metadata, started_at, updated_at
         )
         SELECT id, agent, session_id, title, metadata, started_at, updated_at
         FROM ts_src.conversations"
    );

    migrate_table!(conn, "messages",
        "INSERT OR IGNORE INTO main.messages (
            id, conversation_id, role, content, metadata, created_at
         )
         SELECT id, conversation_id, role, content, metadata, created_at
         FROM ts_src.messages"
    );

    // -----------------------------------------------------------------
    // Intelligence tables (intelligence.ts)
    // -----------------------------------------------------------------

    migrate_table!(conn, "consolidations",
        "INSERT OR IGNORE INTO main.consolidations (
            id, summary_memory_id, source_memory_ids, cluster_label, created_at
         )
         SELECT id, summary_memory_id, source_memory_ids, cluster_label, created_at
         FROM ts_src.consolidations"
    );

    migrate_table!(conn, "entities",
        "INSERT OR IGNORE INTO main.entities (
            id, name, entity_type, description, aliases, metadata,
            user_id, created_at, updated_at
         )
         SELECT id, name, type, description, aka, metadata,
                user_id, created_at, updated_at
         FROM ts_src.entities"
    );

    migrate_table!(conn, "entity_relationships",
        "INSERT OR IGNORE INTO main.entity_relationships (
            id, source_entity_id, target_entity_id, relationship_type, created_at
         )
         SELECT id, source_entity_id, target_entity_id, relationship, created_at
         FROM ts_src.entity_relationships"
    );

    migrate_table!(conn, "memory_entities",
        "INSERT OR IGNORE INTO main.memory_entities (memory_id, entity_id)
         SELECT memory_id, entity_id
         FROM ts_src.memory_entities"
    );

    migrate_table!(conn, "projects",
        "INSERT OR IGNORE INTO main.projects (
            id, name, description, status, metadata, user_id, created_at, updated_at
         )
         SELECT id, name, description, status, metadata, user_id, created_at, updated_at
         FROM ts_src.projects"
    );

    migrate_table!(conn, "memory_projects",
        "INSERT OR IGNORE INTO main.memory_projects (memory_id, project_id)
         SELECT memory_id, project_id
         FROM ts_src.memory_projects"
    );

    migrate_table!(conn, "structured_facts",
        "INSERT OR IGNORE INTO main.structured_facts (
            id, memory_id, subject, verb, object, quantity, unit,
            date_ref, date_approx, confidence, user_id, created_at
         )
         SELECT id, memory_id, subject, verb, object, quantity, unit,
                date_ref, date_approx, confidence, user_id, created_at
         FROM ts_src.structured_facts"
    );

    migrate_table!(conn, "current_state",
        "INSERT OR IGNORE INTO main.current_state (
            id, key, value, memory_id, previous_value, previous_memory_id,
            updated_count, user_id, created_at, updated_at
         )
         SELECT id, key, value, memory_id, previous_value, previous_memory_id,
                updated_count, user_id, created_at, updated_at
         FROM ts_src.current_state"
    );

    migrate_table!(conn, "user_preferences",
        "INSERT OR IGNORE INTO main.user_preferences (
            id, domain, preference, strength, evidence_memory_id, user_id, created_at, updated_at
         )
         SELECT id, domain, preference, strength, evidence_memory_id, user_id, created_at, updated_at
         FROM ts_src.user_preferences"
    );

    migrate_table!(conn, "entity_cooccurrences",
        "INSERT OR IGNORE INTO main.entity_cooccurrences (
            id, entity_a_id, entity_b_id, cooccurrence_count, score,
            last_memory_id, user_id, created_at, updated_at
         )
         SELECT id, entity_a_id, entity_b_id, cooccurrence_count, score,
                last_memory_id, user_id, created_at, updated_at
         FROM ts_src.entity_cooccurrences"
    );

    migrate_table!(conn, "digests",
        "INSERT OR IGNORE INTO main.digests (
            id, user_id, schedule, webhook_url, webhook_secret,
            include_stats, include_new_memories, include_contradictions,
            include_reflections, last_sent_at, next_send_at, active, created_at
         )
         SELECT id, user_id, schedule, webhook_url, webhook_secret,
                include_stats, include_new_memories, include_contradictions,
                include_reflections, last_sent_at, next_send_at, active, created_at
         FROM ts_src.digests"
    );

    migrate_table!(conn, "reflections",
        "INSERT OR IGNORE INTO main.reflections (
            id, user_id, content, themes, period_start, period_end,
            memory_count, source_memory_ids, created_at
         )
         SELECT id, user_id, content, themes, period_start, period_end,
                memory_count, source_memory_ids, created_at
         FROM ts_src.reflections"
    );

    migrate_table!(conn, "personality_signals",
        "INSERT OR IGNORE INTO main.personality_signals (
            id, memory_id, user_id, signal_type, subject, valence,
            intensity, reasoning, source_text, created_at
         )
         SELECT id, memory_id, user_id, signal_type, subject, valence,
                intensity, reasoning, source_text, created_at
         FROM ts_src.personality_signals"
    );

    migrate_table!(conn, "personality_profiles",
        "INSERT OR IGNORE INTO main.personality_profiles (
            id, user_id, profile, signal_count, is_stale, created_at, updated_at
         )
         SELECT id, user_id, profile, signal_count, is_stale, created_at, updated_at
         FROM ts_src.personality_profiles"
    );

    migrate_table!(conn, "scratchpad",
        "INSERT OR IGNORE INTO main.scratchpad (
            id, user_id, session, agent, model, entry_key, value,
            created_at, updated_at, expires_at
         )
         SELECT id, user_id, session, agent, model, entry_key, value,
                created_at, updated_at, expires_at
         FROM ts_src.scratchpad"
    );

    // -----------------------------------------------------------------
    // Service tables (services.ts)
    // -----------------------------------------------------------------

    migrate_table!(conn, "agents",
        "INSERT OR IGNORE INTO main.agents (
            id, user_id, name, category, description, code_hash,
            trust_score, total_ops, successful_ops, failed_ops,
            guard_allows, guard_warns, guard_blocks, is_active,
            revoked_at, revoke_reason, last_seen_at, created_at
         )
         SELECT id, user_id, name, category, description, code_hash,
                trust_score, total_ops, successful_ops, failed_ops,
                guard_allows, guard_warns, guard_blocks, is_active,
                revoked_at, revoke_reason, last_seen_at, created_at
         FROM ts_src.agents"
    );

    migrate_table!(conn, "webhooks",
        "INSERT OR IGNORE INTO main.webhooks (
            id, url, events, secret, user_id, active,
            last_triggered_at, failure_count, created_at
         )
         SELECT id, url, events, secret, user_id, active,
                last_triggered_at, failure_count, created_at
         FROM ts_src.webhooks"
    );

    migrate_table!(conn, "tenant_quotas",
        "INSERT OR IGNORE INTO main.tenant_quotas (
            user_id, max_memories, max_conversations, max_api_keys,
            max_spaces, max_memory_size_bytes, rate_limit_override,
            created_at, updated_at
         )
         SELECT user_id, max_memories, max_conversations, max_api_keys,
                max_spaces, max_memory_size_bytes, rate_limit_override,
                created_at, updated_at
         FROM ts_src.tenant_quotas"
    );

    migrate_table!(conn, "usage_events",
        "INSERT OR IGNORE INTO main.usage_events (
            id, user_id, event_type, quantity, metadata, created_at
         )
         SELECT id, user_id, event_type, quantity, metadata, created_at
         FROM ts_src.usage_events"
    );

    migrate_table!(conn, "skill_records",
        "INSERT OR IGNORE INTO main.skill_records (
            skill_id, name, description, path, content, category, origin,
            generation, lineage_change_summary, creator_id, is_active,
            total_selections, total_applied, total_completions,
            embedding, first_seen, last_updated
         )
         SELECT skill_id, name, description, path, content, category, origin,
                generation, lineage_change_summary, creator_id, is_active,
                total_selections, total_applied, total_completions,
                embedding, first_seen, last_updated
         FROM ts_src.skill_records"
    );

    migrate_table!(conn, "skill_lineage_parents",
        "INSERT OR IGNORE INTO main.skill_lineage_parents (skill_id, parent_skill_id)
         SELECT skill_id, parent_skill_id
         FROM ts_src.skill_lineage_parents"
    );

    migrate_table!(conn, "skill_tags",
        "INSERT OR IGNORE INTO main.skill_tags (skill_id, tag)
         SELECT skill_id, tag
         FROM ts_src.skill_tags"
    );

    migrate_table!(conn, "execution_analyses",
        "INSERT OR IGNORE INTO main.execution_analyses (
            id, task_id, timestamp, task_completed, execution_note,
            tool_issues, candidate_for_evolution, evolution_suggestions,
            analyzed_by, analyzed_at
         )
         SELECT id, task_id, timestamp, task_completed, execution_note,
                tool_issues, candidate_for_evolution, evolution_suggestions,
                analyzed_by, analyzed_at
         FROM ts_src.execution_analyses"
    );

    migrate_table!(conn, "skill_judgments",
        "INSERT OR IGNORE INTO main.skill_judgments (
            id, analysis_id, skill_id, skill_applied, note
         )
         SELECT id, analysis_id, skill_id, skill_applied, note
         FROM ts_src.skill_judgments"
    );

    migrate_table!(conn, "skill_tool_deps",
        "INSERT OR IGNORE INTO main.skill_tool_deps (skill_id, tool_key, critical)
         SELECT skill_id, tool_key, critical
         FROM ts_src.skill_tool_deps"
    );

    migrate_table!(conn, "tool_quality_records",
        "INSERT OR IGNORE INTO main.tool_quality_records (
            id, tool_key, backend, server, tool_name, description_hash,
            total_calls, total_successes, total_failures, avg_execution_ms,
            llm_flagged_count, quality_score, last_execution_at,
            created_at, updated_at
         )
         SELECT id, tool_key, backend, server, tool_name, description_hash,
                total_calls, total_successes, total_failures, avg_execution_ms,
                llm_flagged_count, quality_score, last_execution_at,
                created_at, updated_at
         FROM ts_src.tool_quality_records"
    );

    migrate_table!(conn, "artifacts",
        "INSERT OR IGNORE INTO main.artifacts (
            id, memory_id, filename, mime_type, size_bytes, sha256,
            storage_mode, data, disk_path, created_at
         )
         SELECT id, memory_id, filename, mime_type, size_bytes, sha256,
                storage_mode, data, disk_path, created_at
         FROM ts_src.artifacts"
    );

    // -----------------------------------------------------------------
    // Tier-4 tables (tier4.ts)
    // -----------------------------------------------------------------

    migrate_table!(conn, "causal_chains",
        "INSERT OR IGNORE INTO main.causal_chains (id, name, user_id, created_at)
         SELECT id, name, user_id, created_at
         FROM ts_src.causal_chains"
    );

    migrate_table!(conn, "causal_links",
        "INSERT OR IGNORE INTO main.causal_links (
            id, chain_id, memory_id, position, role, created_at
         )
         SELECT id, chain_id, memory_id, position, role, created_at
         FROM ts_src.causal_links"
    );

    migrate_table!(conn, "reconsolidations",
        "INSERT OR IGNORE INTO main.reconsolidations (
            id, memory_id, old_importance, new_importance,
            old_confidence, new_confidence, reason, created_at
         )
         SELECT id, memory_id, old_importance, new_importance,
                old_confidence, new_confidence, reason, created_at
         FROM ts_src.reconsolidations"
    );

    migrate_table!(conn, "temporal_patterns",
        "INSERT OR IGNORE INTO main.temporal_patterns (
            id, user_id, day_of_week, hour_of_day, category,
            project_id, access_count
         )
         SELECT id, user_id, day_of_week, hour_of_day, category,
                project_id, access_count
         FROM ts_src.temporal_patterns"
    );

    // -----------------------------------------------------------------
    // Rebuild FTS indexes after bulk import
    // -----------------------------------------------------------------

    rebuild_fts(conn, "memories_fts").await;
    rebuild_fts(conn, "episodes_fts").await;
    rebuild_fts(conn, "messages_fts").await;
    rebuild_fts(conn, "skills_fts").await;

    info!("all table migrations complete");
    Ok(())
}

/// Rebuild a content-sync FTS table by running the 'rebuild' command.
/// Logs warnings on failure but does not abort the migration.
async fn rebuild_fts(conn: &Connection, fts_table: &str) {
    let sql = format!("INSERT INTO {}({}) VALUES('rebuild')", fts_table, fts_table);
    match conn.execute(&sql, ()).await {
        Ok(_) => info!(fts = fts_table, "FTS index rebuilt"),
        Err(e) => warn!(fts = fts_table, error = %e, "FTS rebuild failed (non-fatal)"),
    }
}
