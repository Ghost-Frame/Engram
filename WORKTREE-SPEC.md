# Worktree F: Multi-Tenant Data Isolation Hardening

## Context

Codex's `codex/multi-tenant-libsql` branch established multi-tenant architecture for engram-rust. The TypeScript source repo (engram v5.7.0) fixed 30+ cross-tenant boundaries. Our recent worktree agents (B-E) delivered functional implementations but introduced or perpetuated multi-tenant gaps. A full codebase audit found 31+ functions where one user could read, modify, or delete another user's data by knowing record IDs.

## Mandate

**Every database query that touches user data MUST include a `user_id` or `AND user_id = ?N` clause.** No exceptions. ID-only lookups are a cross-tenant data leak.

**Every route handler that accesses user-scoped data MUST extract `auth.user_id` and pass it to the lib function.** The `Auth(_auth)` pattern (extracting auth but discarding user_id) is BANNED.

## Gap Inventory

### Tier 1 -- CRITICAL (data destruction or full read across tenants)

| # | File | Function | Gap |
|---|------|----------|-----|
| 1 | `engram-lib/src/admin/mod.rs` | `gc()` | Deletes ALL forgotten memories globally. Must scope to user_id or require explicit admin auth. |
| 2 | `engram-lib/src/memory/mod.rs` | `get()` | `WHERE id = ?1` -- no user_id. Any user can read any memory by ID. |
| 3 | `engram-lib/src/memory/mod.rs` | `mark_forgotten()` | `WHERE id = ?1` -- no user_id. Cross-tenant deletion. |
| 4 | `engram-lib/src/memory/mod.rs` | `mark_archived()` | `WHERE id = ?1` -- no user_id. |
| 5 | `engram-lib/src/memory/mod.rs` | `mark_unarchived()` | `WHERE id = ?1` -- no user_id. |
| 6 | `engram-lib/src/memory/mod.rs` | `insert_link()` | No validation that both memory IDs belong to the same user. |
| 7 | `engram-lib/src/memory/mod.rs` | `update_forget_reason()` | `WHERE id = ?1` -- no user_id. |
| 8 | `engram-lib/src/inbox.rs` | `edit_and_approve()` | `WHERE id = ?N` -- no user_id. Cross-tenant memory modification. |
| 9 | `engram-lib/src/inbox.rs` | `set_forget_reason()` | `WHERE id = ?1` -- no user_id. |
| 10 | `engram-lib/src/graph/entities.rs` | `get_entity()` | `WHERE id = ?1` -- no user_id. |
| 11 | `engram-lib/src/graph/entities.rs` | `delete_entity()` | `WHERE id = ?1` -- no user_id. |
| 12 | `engram-lib/src/graph/communities.rs` | `detect_communities()` | `UPDATE memories SET community_id = ?1 WHERE id = ?2` -- no user_id. |
| 13 | `engram-lib/src/graph/pagerank.rs` | `update_pagerank_scores()` | `UPDATE memories SET pagerank_score = ?1 WHERE id = ?2` -- no user_id. |
| 14 | `engram-lib/src/graph/cooccurrence.rs` | `build_cooccurrence_edges()` | No user_id parameter at all. Queries entire table. |
| 15 | `engram-lib/src/services/axon.rs` | `get_event()` | `WHERE id = ?1` -- no user_id. |
| 16 | `engram-lib/src/services/axon.rs` | `query_events()` | No default user_id filter. |
| 17 | `engram-lib/src/services/broca.rs` | `get_action()` | `WHERE id = ?1` -- no user_id. |
| 18 | `engram-lib/src/services/broca.rs` | `query_actions()` | No default user_id filter. |
| 19 | `engram-lib/src/services/chiasm.rs` | `get_task()` | `WHERE id = ?1` -- no user_id. |
| 20 | `engram-lib/src/services/chiasm.rs` | `delete_task()` | `WHERE id = ?1` -- no user_id. |
| 21 | `engram-lib/src/services/soma.rs` | `get_agent()` | `WHERE id = ?1` -- no user_id. |

### Tier 2 -- HIGH (data leak via indirect path)

| # | File | Function | Gap |
|---|------|----------|-----|
| 22 | `engram-lib/src/intelligence/valence.rs` | `store_valence()` | Updates valence/arousal/emotion on memories by ID only. |
| 23 | `engram-lib/src/context/deps.rs` | `get_memory_without_embedding()` | `WHERE id = ?1` -- no user_id. Used in context assembly. |
| 24 | `engram-lib/src/cognithor/compression.rs` | (anonymous) | `SELECT content, importance FROM memories WHERE id = ?1` -- no user_id. |
| 25 | `engram-lib/src/intelligence/temporal.rs` | (query) | `SELECT created_at FROM memories WHERE id = ?1` -- no user_id. |
| 26 | `engram-lib/src/memory/mod.rs` | `update_source_count()` | `WHERE id = ?1` -- no user_id. |

### Tier 3 -- Route Handler Gaps

| # | File | Handler | Gap |
|---|------|---------|-----|
| 27 | `engram-server/src/routes/graph.rs` | `get_entity_handler()` | `Auth(_auth)` -- extracts auth, discards user_id. |
| 28 | `engram-server/src/routes/graph.rs` | `delete_entity_handler()` | `Auth(_auth)` -- discards user_id. |
| 29 | `engram-server/src/routes/graph.rs` | `entity_relationships_handler()` | `Auth(_auth)` -- discards user_id. |
| 30 | `engram-server/src/routes/graph.rs` | `entity_memories_handler()` | `Auth(_auth)` -- discards user_id. |
| 31 | `engram-server/src/routes/graph.rs` | `memory_entities_handler()` | `Auth(_auth)` -- discards user_id. |
| 32 | `engram-server/src/routes/grounding.rs` | ALL handlers | Zero user_id references. |
| 33 | `engram-lib/src/audit.rs` | `query_audit_log()` | No user_id filter -- anyone can read anyone's audit trail. |
| 34 | `engram-lib/src/guard.rs` | `evaluate()`, `create_rule()`, `list_rules()` | No user_id scoping (may be intentionally global -- verify). |
| 35 | `engram-lib/src/services/axon.rs` | `list_channels()` | `SELECT DISTINCT channel FROM events` -- no user_id. Leaks channel names. |

## Implementation Plan

### Phase 1: Core Memory Layer (highest blast radius)

Add `user_id: i64` parameter to every function in `engram-lib/src/memory/mod.rs` that currently takes only an ID:
- `get(db, id)` -> `get(db, id, user_id)` with `WHERE id = ?1 AND user_id = ?2`
- `mark_forgotten(db, id)` -> `mark_forgotten(db, id, user_id)`
- `mark_archived(db, id)` -> `mark_archived(db, id, user_id)`
- `mark_unarchived(db, id)` -> `mark_unarchived(db, id, user_id)`
- `update_forget_reason(db, reason, id)` -> add user_id
- `update_source_count(db, id, count)` -> add user_id
- `insert_link(db, source, target, sim, type)` -> add user_id, validate both memories belong to that user before inserting

Then fix every caller. `cargo check` will find them all since the signatures change.

### Phase 2: Graph + Intelligence Layer

- `graph/entities.rs`: Add user_id to `get_entity()`, `delete_entity()`
- `graph/communities.rs`: Add user_id to UPDATE in `detect_communities()`
- `graph/pagerank.rs`: Add user_id to UPDATE in `update_pagerank_scores()`
- `graph/cooccurrence.rs`: Add user_id param to `build_cooccurrence_edges()`
- `intelligence/valence.rs`: Add user_id to `store_valence()`
- `intelligence/temporal.rs`: Add user_id to temporal query
- `context/deps.rs`: Add user_id to `get_memory_without_embedding()`
- `cognithor/compression.rs`: Add user_id to memory lookup

### Phase 3: Service Layer

- `services/axon.rs`: Add user_id to `get_event()`, `query_events()`, `list_channels()`
- `services/broca.rs`: Add user_id to `get_action()`, `query_actions()`
- `services/chiasm.rs`: Add user_id to `get_task()`, `delete_task()`
- `services/soma.rs`: Add user_id to `get_agent()`

### Phase 4: Routes + Middleware

- `routes/graph.rs`: Replace ALL `Auth(_auth)` with `Auth(auth)` and pass `auth.user_id`
- `routes/grounding.rs`: Add `auth.user_id` to all handler calls
- `audit.rs`: Add user_id filter to `query_audit_log()`
- `guard.rs`: Decision needed -- if guard rules are per-user, add user_id. If global (admin-managed), leave but document.
- `inbox.rs`: Add user_id to `edit_and_approve()`, `set_forget_reason()`

### Phase 5: Admin + GC

- `admin/mod.rs`: `gc()` must either scope to a specific user_id OR require admin-only auth (not regular API key)

## Verification

After all changes:

1. `cargo check --workspace` -- must pass
2. `cargo clippy --workspace` -- must pass
3. `grep -rn "WHERE id = ?1" engram-lib/src/` -- should return ZERO results for memory/entity/event/task/action tables (all should have `AND user_id = ?N`)
4. `grep -rn "Auth(_auth)" engram-server/src/routes/` -- should return ZERO results (all auth must be used)
5. `grep -rn "_user_id\|_auth" engram-server/src/routes/` -- should return ZERO results (no suppressed user_id params)

## Constraints

- Do NOT change public API response shapes
- Do NOT change route paths
- Do NOT rename existing functions -- only add parameters
- When adding user_id parameters, add them as the LAST parameter to minimize diff churn
- If a function is called internally (not from a route), trace the call chain to find where user_id originates and thread it through
- `admin/mod.rs` gc is a special case -- it may legitimately need to operate globally for system maintenance. If so, it must be behind admin-only auth, not regular API key auth.
