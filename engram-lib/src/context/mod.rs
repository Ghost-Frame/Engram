// ============================================================================
// CONTEXT DOMAIN -- Context assembly engine
// Port of assembly.ts: budget-aware RAG context from 8 layers
// ============================================================================

pub mod budget;
pub mod deps;
pub mod modes;
pub mod scoring;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::db::Database;
use crate::embeddings::EmbeddingProvider;
use crate::llm::local::LocalModelClient;
use crate::memory::search::hybrid_search;
use crate::memory::types::SearchRequest;
use crate::Result;
use crate::{personality, scratchpad};

use budget::{estimate_tokens, resolve_budget, truncate_to_token_budget};
use deps::*;
use modes::*;
use scoring::cosine_similarity;
pub use types::*;

// ---------------------------------------------------------------------------
// Attribution helper
// ---------------------------------------------------------------------------

/// Build an attribution tag string for a context block.
fn build_attribution(block: &ContextBlock) -> String {
    let mut parts = Vec::new();
    if let Some(ref m) = block.model {
        if !m.is_empty() {
            parts.push(format!("model:{}", m));
        }
    }
    if let Some(ref o) = block.origin {
        if !o.is_empty() {
            parts.push(format!("via:{}", o));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Context string assembly from blocks
// ---------------------------------------------------------------------------

/// Assembles the final context string from layers of blocks plus supplementary
/// sections (working memory, current state, personality, preferences, facts).
/// Wrap user-supplied memory content with a structural delimiter so that
/// embedded instructions in stored memories cannot escape into the prompt
/// as top-level directives (SEC-LOW-3).
fn wrap_user_content(content: &str) -> String {
    format!("<user_memory>{}</user_memory>", content)
}

/// This is the formatting step only -- no DB calls here.
pub fn assemble_context_string(
    blocks: &[ContextBlock],
    supplementary: &[SupplementarySection],
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Supplementary sections come first.
    // Wrap user-generated content with structural delimiters to prevent prompt injection
    // (SEC-LOW-3 fix). working_memory already has its own <working-memory> tags.
    for s in supplementary {
        let wrapped = match s.label.as_str() {
            "working_memory" => s.content.clone(), // Already has structural tags
            "current_state" | "personality" | "preferences" => wrap_user_content(&s.content),
            _ => s.content.clone(), // Unknown labels pass through unchanged
        };
        parts.push(wrapped);
    }

    // Single-pass partition: bucket blocks by source in one iteration instead
    // of 7 separate filter+collect passes over the same slice.
    let mut by_source: HashMap<ContextBlockSource, Vec<&ContextBlock>> = HashMap::new();
    for b in blocks {
        by_source.entry(b.source).or_default().push(b);
    }
    let empty = Vec::new();
    let static_blocks = by_source.get(&ContextBlockSource::Static).unwrap_or(&empty);
    let semantic_blocks = by_source.get(&ContextBlockSource::Semantic).unwrap_or(&empty);
    let evolution_blocks = by_source.get(&ContextBlockSource::Evolution).unwrap_or(&empty);
    let episode_blocks = by_source.get(&ContextBlockSource::Episode).unwrap_or(&empty);
    let linked_blocks = by_source.get(&ContextBlockSource::Linked).unwrap_or(&empty);
    let recent_blocks = by_source.get(&ContextBlockSource::Recent).unwrap_or(&empty);
    let inference_blocks = by_source.get(&ContextBlockSource::Inference).unwrap_or(&empty);

    if !static_blocks.is_empty() {
        let lines: Vec<String> = static_blocks
            .iter()
            .map(|b| {
                format!(
                    "- {}{}",
                    wrap_user_content(&b.content),
                    build_attribution(b)
                )
            })
            .collect();
        parts.push(format!("## Permanent Facts\n{}", lines.join("\n")));
    }

    if !semantic_blocks.is_empty() {
        let fact_blocks: Vec<_> = semantic_blocks
            .iter()
            .filter(|b| b.category == "fact")
            .collect();
        let non_fact_blocks: Vec<_> = semantic_blocks
            .iter()
            .filter(|b| b.category != "fact")
            .collect();

        let mut lines: Vec<String> = Vec::new();
        for b in &non_fact_blocks {
            lines.push(format!(
                "- [{}] {}{}",
                b.category,
                wrap_user_content(&b.content),
                build_attribution(b)
            ));
        }

        if !fact_blocks.is_empty() {
            let mut by_parent: HashMap<i64, Vec<&&ContextBlock>> = HashMap::new();
            for b in &fact_blocks {
                let parent_id = b.parent_id.unwrap_or(0);
                by_parent.entry(parent_id).or_default().push(b);
            }
            for (parent_id, facts) in &by_parent {
                if *parent_id > 0 {
                    lines.push(format!("- [facts from memory #{}]", parent_id));
                    for f in facts {
                        lines.push(format!("  - {}", wrap_user_content(&f.content)));
                    }
                } else {
                    for f in facts {
                        lines.push(format!("- [fact] {}", wrap_user_content(&f.content)));
                    }
                }
            }
        }

        parts.push(format!("## Relevant Memories\n{}", lines.join("\n")));
    }

    if !evolution_blocks.is_empty() {
        let lines: Vec<String> = evolution_blocks
            .iter()
            .map(|b| wrap_user_content(&b.content))
            .collect();
        parts.push(format!(
            "## Preference/Fact Evolution\n{}",
            lines.join("\n\n")
        ));
    }

    if !episode_blocks.is_empty() {
        let lines: Vec<String> = episode_blocks
            .iter()
            .map(|b| {
                format!(
                    "- [{}] {}{}",
                    b.created_at.as_deref().unwrap_or(""),
                    wrap_user_content(&b.content),
                    build_attribution(b)
                )
            })
            .collect();
        parts.push(format!("## Episode Context\n{}", lines.join("\n")));
    }

    if !linked_blocks.is_empty() {
        let lines: Vec<String> = linked_blocks
            .iter()
            .map(|b| {
                format!(
                    "- {}{}",
                    wrap_user_content(&b.content),
                    build_attribution(b)
                )
            })
            .collect();
        parts.push(format!("## Related Context\n{}", lines.join("\n")));
    }

    if !recent_blocks.is_empty() {
        let lines: Vec<String> = recent_blocks
            .iter()
            .map(|b| {
                format!(
                    "- [{}] {}{}",
                    b.created_at.as_deref().unwrap_or(""),
                    wrap_user_content(&b.content),
                    build_attribution(b)
                )
            })
            .collect();
        parts.push(format!("## Recent Activity\n{}", lines.join("\n")));
    }

    if !inference_blocks.is_empty() {
        let lines: Vec<String> = inference_blocks
            .iter()
            .map(|b| wrap_user_content(&b.content))
            .collect();
        parts.push(format!("## Implicit Connections\n{}", lines.join("\n")));
    }

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether content is semantically duplicate against already-added blocks.
/// Computes the candidate embedding on-demand using the provider.
/// Returns false when no provider or embedding fails.
async fn is_semantic_duplicate(
    content: &str,
    block_embeddings: &[Vec<f32>],
    provider: &Option<Arc<dyn EmbeddingProvider>>,
    thresh: f64,
) -> bool {
    if block_embeddings.is_empty() {
        return false;
    }
    let p = match provider {
        Some(p) => p,
        None => return false,
    };
    let emb = match p.embed(content).await {
        Ok(e) => e,
        Err(_) => return false,
    };
    block_embeddings
        .iter()
        .any(|e| cosine_similarity(&emb, e) as f64 > thresh)
}

/// Build a working-memory block from scratchpad entries.
/// Returns None when rows is empty.
fn build_working_memory_block(rows: &[scratchpad::ScratchEntry]) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 4000;
    const VALUE_MAX: usize = 300;
    let mut lines: Vec<String> = Vec::new();
    let mut total_len: usize = 0;
    for (i, row) in rows.iter().enumerate() {
        let model_part = if !row.model.is_empty() {
            format!("/{}", row.model)
        } else {
            String::new()
        };
        let mut value = row.value.trim().to_string();
        if value.len() > VALUE_MAX {
            value = format!("{}...", &value[..VALUE_MAX]);
        }
        let session_prefix: String = row.session.chars().take(8).collect();
        let time_part = format_scratch_age(&row.updated_at);
        let value_part = if !value.is_empty() {
            format!(" {}", value)
        } else {
            String::new()
        };
        let line = format!(
            "- [{}{} #{}] {}{} ({})",
            row.agent, model_part, session_prefix, row.key, value_part, time_part
        );
        if total_len + line.len() > MAX_CHARS && !lines.is_empty() {
            lines.push(format!("- ... {} more entries truncated", rows.len() - i));
            break;
        }
        total_len += line.len() + 1;
        lines.push(line);
    }
    Some(format!(
        "<working-memory>\n{}\n</working-memory>",
        lines.join("\n")
    ))
}

/// Format a relative age string for a scratchpad entry timestamp.
fn format_scratch_age(updated_at: &str) -> String {
    let normalized = if updated_at.contains('Z') {
        updated_at.to_string()
    } else {
        format!("{}Z", updated_at.replace(' ', "T"))
    };
    if let Ok(dt) = normalized.parse::<chrono::DateTime<chrono::Utc>>() {
        let diff_min = chrono::Utc::now()
            .signed_duration_since(dt)
            .num_minutes()
            .max(0);
        if diff_min <= 1 {
            "just now".to_string()
        } else {
            format!("{}m ago", diff_min)
        }
    } else {
        "just now".to_string()
    }
}

// ---------------------------------------------------------------------------
// Core context assembly -- progressive disclosure algorithm
// ---------------------------------------------------------------------------

/// Core progressive disclosure algorithm.
///
/// Assembles context from 8 layers:
///   1. Static facts (permanent, ranked by query relevance)
///   2. Semantic search (hybrid vector + FTS, optional rerank)
///      - 2.5a. Version chain evolution (preference/fact change history)
///      - 2.5b. Episode context (summarized conversation episodes)
///   3. Linked memories (graph expansion from semantic results)
///   4. Recent memories (temporal context)
///   5. Inference (LLM-generated implicit connections via local model)
///   + Supplementary: working memory, current state, personality, preferences, facts
pub async fn assemble_context(
    db: &Database,
    mut opts: ContextOptions,
    user_id: i64,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    llm_client: Option<Arc<LocalModelClient>>,
) -> Result<ContextResult> {
    // --- Apply mode preset ---
    apply_context_mode(&mut opts);

    // --- Resolve parameters ---
    let explicit_budget = opts
        .max_tokens
        .or(opts.token_budget)
        .or(opts.budget);
    let (token_budget, _budget_note) = resolve_budget(
        explicit_budget,
        opts.model_id.as_deref(),
        DEFAULT_TOKEN_BUDGET,
    );
    let context_strategy = opts.strategy.unwrap_or(ContextStrategy::Balanced);
    let depth = opts.depth.unwrap_or(3).clamp(1, 3);

    let max_memory_tokens = opts.max_memory_tokens.unwrap_or(DEFAULT_MAX_MEMORY_TOKENS);
    let dedup_thresh = opts.dedup_threshold.unwrap_or(DEFAULT_DEDUP_THRESHOLD);
    let source_filter = opts.source.clone();

    let flags = resolve_layer_flags(&opts, depth);
    let semantic_ceiling = resolve_semantic_ceiling(&context_strategy, opts.semantic_ceiling);
    let semantic_limit = resolve_semantic_limit(&context_strategy, opts.semantic_limit);
    let min_relev = opts.min_relevance.unwrap_or(DEFAULT_MIN_RELEVANCE);

    let truncate = |content: &str| truncate_to_token_budget(content, max_memory_tokens);

    // --- State ---
    let mut blocks: Vec<ContextBlock> = Vec::new();
    let mut used_tokens: usize = 0;
    let mut seen_ids: HashSet<i64> = HashSet::new();
    let t0 = Instant::now();
    let mut timing = ContextTiming::default();

    // --- Embedding map for dedup ---
    let mut block_embeddings: Vec<Vec<f32>> = Vec::new();

    // --- Embed query ---
    let t_embed = Instant::now();
    let query_emb: Option<Vec<f32>> = if let Some(ref p) = embedding_provider {
        p.embed(&opts.query).await.ok()
    } else {
        None
    };
    timing.embed_ms = Some(t_embed.elapsed().as_millis() as u64);

    // ---- Phase 1: Static facts, ranked by query relevance ----
    if flags.include_static {
        let mut statics = get_static_memories(db, user_id).await.unwrap_or_default();
        if let Some(ref sf) = source_filter {
            statics.retain(|s| s.source.contains(sf.as_str()));
        }

        // Score by cosine similarity when embedding provider is available; fall back to source_count.
        let mut scored: Vec<(usize, f64, Option<Vec<f32>>)> = Vec::new();
        for (i, s) in statics.iter().enumerate() {
            let mut relevance = 0.5;
            let static_emb: Option<Vec<f32>> = if let Some(ref p) = embedding_provider {
                p.embed(&s.content).await.ok()
            } else {
                None
            };
            if let (Some(ref qe), Some(ref emb)) = (&query_emb, &static_emb) {
                relevance = cosine_similarity(qe, emb) as f64;
            }
            relevance += (s.source_count as f64 / 20.0).min(0.1);
            scored.push((i, relevance, static_emb));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let static_budget_fraction = resolve_static_budget_fraction(&context_strategy);
        for (idx, relevance, static_emb) in scored {
            let mem = &statics[idx];
            let truncated = truncate(&mem.content);
            let tokens = estimate_tokens(&truncated);
            if used_tokens + tokens > (token_budget as f64 * static_budget_fraction) as usize {
                break;
            }
            blocks.push(ContextBlock {
                id: mem.id,
                content: truncated,
                category: mem.category.clone(),
                score: relevance * 100.0,
                source: ContextBlockSource::Static,
                tokens,
                created_at: None,
                model: mem.model.clone(),
                origin: Some(mem.source.clone()),
                parent_id: None,
            });
            seen_ids.insert(mem.id);
            used_tokens += tokens;
            if let Some(emb) = static_emb {
                block_embeddings.push(emb);
            }
        }
    }
    timing.static_ms = Some(t0.elapsed().as_millis() as u64 - timing.embed_ms.unwrap_or(0));

    // ---- Phase 2: Semantic search ----
    let t_search = Instant::now();
    let search_req = SearchRequest {
        query: opts.query.clone(),
        embedding: query_emb,
        limit: Some(semantic_limit),
        category: None,
        source: source_filter.clone(),
        tags: None,
        threshold: None,
        user_id: Some(user_id),
        space_id: None,
        include_forgotten: Some(false),
        mode: None,
        question_type: None,
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    };
    let semantic_results = hybrid_search(db, search_req).await.unwrap_or_default();
    timing.search_ms = Some(t_search.elapsed().as_millis() as u64);

    // TODO: Optional cross-encoder rerank when reranker module is available

    let now_ms = chrono::Utc::now().timestamp_millis();

    for r in &semantic_results {
        if seen_ids.contains(&r.memory.id) {
            continue;
        }
        let truncated = truncate(&r.memory.content);
        let tokens = estimate_tokens(&truncated);
        if used_tokens + tokens > (token_budget as f64 * semantic_ceiling) as usize {
            break;
        }

        // Compute embedding once for dedup check and block tracking
        let candidate_emb: Option<Vec<f32>> = if let Some(ref p) = embedding_provider {
            p.embed(&truncated).await.ok()
        } else {
            None
        };
        if let Some(ref emb) = candidate_emb {
            if !block_embeddings.is_empty() {
                // Cosine similarity over potentially large embedding vectors is
                // CPU-bound. Offload to a blocking thread so the async runtime
                // is not stalled (S5-22).
                let emb_clone = emb.clone();
                let embeddings_clone = block_embeddings.clone();
                let is_dup = tokio::task::spawn_blocking(move || {
                    embeddings_clone
                        .iter()
                        .any(|e| cosine_similarity(&emb_clone, e) as f64 > dedup_thresh)
                })
                .await
                .unwrap_or(false);
                if is_dup {
                    continue;
                }
            }
        }

        let raw_score = r.score;
        if raw_score < min_relev {
            continue;
        }

        // Recency boost: last 48h get +10%
        let mut score = raw_score;
        if let Ok(created) = r
            .memory
            .created_at
            .replace(' ', "T")
            .parse::<chrono::DateTime<chrono::Utc>>()
        {
            let age_ms = now_ms - created.timestamp_millis();
            if age_ms < RECENCY_BOOST_MS {
                score *= 1.10;
            }
        }

        // Check if this is a fact with a parent
        let mem_detail = get_memory_without_embedding(db, r.memory.id, user_id)
            .await
            .ok()
            .flatten();
        let parent_id = mem_detail
            .as_ref()
            .filter(|m| m.is_fact)
            .and_then(|m| m.parent_memory_id);

        blocks.push(ContextBlock {
            id: r.memory.id,
            content: truncated,
            category: r.memory.category.clone(),
            score,
            source: ContextBlockSource::Semantic,
            tokens,
            created_at: Some(r.memory.created_at.clone()),
            model: r.memory.model.clone(),
            origin: Some(r.memory.source.clone()),
            parent_id,
        });
        seen_ids.insert(r.memory.id);
        used_tokens += tokens;
        if let Some(emb) = candidate_emb {
            block_embeddings.push(emb);
        }
    }

    timing.semantic_ms = Some(
        t0.elapsed().as_millis() as u64
            - timing.embed_ms.unwrap_or(0)
            - timing.static_ms.unwrap_or(0)
            - timing.search_ms.unwrap_or(0),
    );

    // ---- Phase 2.5a: Version chain evolution ----
    let t_evolution = Instant::now();
    if depth >= 2 && used_tokens < (token_budget as f64 * 0.72) as usize {
        let semantic_for_evo: Vec<_> = blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Semantic)
            .take(8)
            .map(|b| b.id)
            .collect();
        for sid in semantic_for_evo {
            if used_tokens >= (token_budget as f64 * 0.72) as usize {
                break;
            }
            let mem = get_memory_without_embedding(db, sid, user_id)
                .await
                .ok()
                .flatten();
            let mem = match mem {
                Some(m) => m,
                None => continue,
            };
            let root_id = mem.root_memory_id.unwrap_or(mem.id);
            let chain = get_version_chain(db, root_id, user_id)
                .await
                .unwrap_or_default();
            if chain.len() < 2 {
                continue;
            }
            let evolution_lines: Vec<String> = chain
                .iter()
                .map(|c| {
                    let date = if c.created_at.len() >= 10 {
                        &c.created_at[..10]
                    } else {
                        "?"
                    };
                    format!("v{} ({}): {}", c.version, date, c.content)
                })
                .collect();
            let evolution_text = format!(
                "[Evolution of memory #{}]\n{}",
                root_id,
                evolution_lines.join("\n")
            );
            let truncated = truncate(&evolution_text);
            let tokens = estimate_tokens(&truncated);
            if used_tokens + tokens > (token_budget as f64 * 0.75) as usize {
                break;
            }
            blocks.push(ContextBlock {
                id: -root_id,
                content: truncated,
                category: "evolution".to_string(),
                score: 70.0,
                source: ContextBlockSource::Evolution,
                tokens,
                created_at: chain.last().map(|c| c.created_at.clone()),
                model: None,
                origin: None,
                parent_id: None,
            });
            for c in &chain {
                seen_ids.insert(c.id);
            }
            used_tokens += tokens;
        }
    }
    timing.evolution_ms = Some(t_evolution.elapsed().as_millis() as u64);

    // ---- Phase 2.5b: Episode context ----
    let t_episodes = Instant::now();
    let mut seen_episode_ids: HashSet<i64> = HashSet::new();
    if flags.include_episodes && used_tokens < (token_budget as f64 * 0.75) as usize {
        let semantic_for_ep: Vec<i64> = blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Semantic)
            .take(5)
            .map(|b| b.id)
            .collect();
        for sid in semantic_for_ep {
            let mem = get_memory_without_embedding(db, sid, user_id)
                .await
                .ok()
                .flatten();
            let ep_id = match mem.and_then(|m| m.episode_id) {
                Some(id) => id,
                None => continue,
            };
            if seen_episode_ids.contains(&ep_id) {
                continue;
            }
            seen_episode_ids.insert(ep_id);
            let ep = match get_episode_summary(db, ep_id, user_id).await.ok().flatten() {
                Some(e) => e,
                None => continue,
            };
            if let Some(ref summary) = ep.summary {
                let truncated = truncate(summary);
                let tokens = estimate_tokens(&truncated);
                if used_tokens + tokens <= (token_budget as f64 * 0.8) as usize {
                    blocks.push(ContextBlock {
                        id: -ep_id,
                        content: truncated,
                        category: "episode".to_string(),
                        score: 75.0,
                        source: ContextBlockSource::Episode,
                        tokens,
                        created_at: ep.started_at,
                        model: None,
                        origin: None,
                        parent_id: None,
                    });
                    used_tokens += tokens;
                }
            }
        }
    }
    timing.episodes_ms = Some(t_episodes.elapsed().as_millis() as u64);

    // ---- Phase 3: Linked memories (graph expansion) ----
    let t_linked = Instant::now();
    if flags.include_linked
        && context_strategy != ContextStrategy::Precision
        && used_tokens < (token_budget as f64 * 0.85) as usize
    {
        let semantic_ids: Vec<i64> = blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Semantic)
            .take(5)
            .map(|b| b.id)
            .collect();
        for sid in semantic_ids {
            if used_tokens >= (token_budget as f64 * 0.85) as usize {
                break;
            }
            let linked = get_links(db, sid, user_id).await.unwrap_or_default();
            for l in &linked {
                if seen_ids.contains(&l.id) || l.is_forgotten {
                    continue;
                }
                let truncated = truncate(&l.content);
                let tokens = estimate_tokens(&truncated);
                if used_tokens + tokens > (token_budget as f64 * 0.88) as usize {
                    break;
                }
                let candidate_emb: Option<Vec<f32>> = if let Some(ref p) = embedding_provider {
                    p.embed(&truncated).await.ok()
                } else {
                    None
                };
                if let Some(ref emb) = candidate_emb {
                    if !block_embeddings.is_empty() {
                        let emb_clone = emb.clone();
                        let embeddings_clone = block_embeddings.clone();
                        let is_dup = tokio::task::spawn_blocking(move || {
                            embeddings_clone
                                .iter()
                                .any(|e| cosine_similarity(&emb_clone, e) as f64 > dedup_thresh)
                        })
                        .await
                        .unwrap_or(false);
                        if is_dup {
                            continue;
                        }
                    }
                }
                blocks.push(ContextBlock {
                    id: l.id,
                    content: truncated,
                    category: l.category.clone(),
                    score: l.similarity * 50.0,
                    source: ContextBlockSource::Linked,
                    tokens,
                    created_at: None,
                    model: l.model.clone(),
                    origin: l.source.clone(),
                    parent_id: None,
                });
                seen_ids.insert(l.id);
                used_tokens += tokens;
                if let Some(emb) = candidate_emb {
                    block_embeddings.push(emb);
                }
            }
        }
    }
    timing.linked_ms = Some(t_linked.elapsed().as_millis() as u64);

    // ---- Phase 4: Recent memories (temporal context) ----
    let t_recent = Instant::now();
    let recent_ceiling = (token_budget as f64 * 0.93) as usize;
    if flags.include_recent && used_tokens < recent_ceiling {
        let recent = get_recent_dynamic(db, user_id, 5).await.unwrap_or_default();
        for r in &recent {
            if seen_ids.contains(&r.id) {
                continue;
            }
            let truncated = truncate(&r.content);
            let tokens = estimate_tokens(&truncated);
            if used_tokens + tokens > recent_ceiling {
                break;
            }
            let candidate_emb: Option<Vec<f32>> = if let Some(ref p) = embedding_provider {
                p.embed(&truncated).await.ok()
            } else {
                None
            };
            if let Some(ref emb) = candidate_emb {
                if !block_embeddings.is_empty() {
                    let emb_clone = emb.clone();
                    let embeddings_clone = block_embeddings.clone();
                    let is_dup = tokio::task::spawn_blocking(move || {
                        embeddings_clone
                            .iter()
                            .any(|e| cosine_similarity(&emb_clone, e) as f64 > dedup_thresh)
                    })
                    .await
                    .unwrap_or(false);
                    if is_dup {
                        continue;
                    }
                }
            }
            blocks.push(ContextBlock {
                id: r.id,
                content: truncated,
                category: r.category.clone(),
                score: 10.0,
                source: ContextBlockSource::Recent,
                tokens,
                created_at: Some(r.created_at.clone()),
                model: r.model.clone(),
                origin: Some(r.source.clone()),
                parent_id: None,
            });
            seen_ids.insert(r.id);
            used_tokens += tokens;
            if let Some(emb) = candidate_emb {
                block_embeddings.push(emb);
            }
        }
    }
    timing.recent_ms = Some(t_recent.elapsed().as_millis() as u64);

    // ---- Phase 5: Inference (LLM-generated implicit connections) ----
    let t_inference = Instant::now();
    let semantic_for_inference: Vec<_> = blocks
        .iter()
        .filter(|b| b.source == ContextBlockSource::Semantic)
        .collect();
    if flags.include_inference
        && semantic_for_inference.len() >= 2
        && used_tokens < (token_budget as f64 * 0.95) as usize
    {
        if let Some(ref llm) = llm_client {
            if llm.is_available() {
                let top_facts: String = semantic_for_inference
                    .iter()
                    .take(6)
                    .map(|b| format!("[{}] {}", b.id, b.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                let system_prompt = "You find implicit connections between memories that aren't directly stated. Given these memories, identify 0-3 implicit connections. For each, write a single sentence stating the connection. If none exist, return \"none\". Be concise. Only state connections that are genuinely useful and non-obvious.";
                let user_prompt = format!("Query: {}\n\nMemories:\n{}", opts.query, top_facts);
                if let Ok(result) = llm.call(system_prompt, &user_prompt, None).await {
                    if !result.to_lowercase().starts_with("none") {
                        let tokens = estimate_tokens(&result);
                        if used_tokens + tokens <= token_budget {
                            blocks.push(ContextBlock {
                                id: 0,
                                content: result.trim().to_string(),
                                category: "inference".to_string(),
                                score: 60.0,
                                source: ContextBlockSource::Inference,
                                tokens,
                                created_at: None,
                                model: None,
                                origin: None,
                                parent_id: None,
                            });
                            used_tokens += tokens;
                        }
                    }
                }
            }
        }
    }
    timing.inference_ms = Some(t_inference.elapsed().as_millis() as u64);

    // ---- Assembly: supplementary sections ----
    let t_assembly = Instant::now();
    let mut supplementary: Vec<SupplementarySection> = Vec::new();
    let mut personality_block_tokens: usize = 0;

    // Fire independent supplementary DB fetches in parallel. Each returns
    // Option/None based on its flag so we skip unnecessary work but still
    // overlap the I/O of whichever fetches are enabled.
    let session_filter: Option<&str> = opts.session.as_deref().filter(|s| !s.is_empty());
    let (scratch_res, state_res, personality_res, pref_res) = tokio::join!(
        async {
            if !flags.include_working_memory {
                return None;
            }
            scratchpad::list_entries(db, user_id, None, None, session_filter)
                .await
                .ok()
        },
        async {
            if !flags.include_current_state {
                return None;
            }
            get_current_state(db, user_id).await.ok()
        },
        async {
            if !flags.include_personality {
                return None;
            }
            personality::get_profile_for_injection(db, user_id)
                .await
                .ok()
                .flatten()
        },
        async {
            if !flags.include_preferences {
                return None;
            }
            get_user_preferences(db, user_id).await.ok()
        },
    );

    // Process results sequentially to build supplementary sections.
    if let Some(scratch_rows) = scratch_res {
        if let Some(wm) = build_working_memory_block(&scratch_rows) {
            supplementary.push(SupplementarySection {
                label: "working_memory".to_string(),
                content: wm,
            });
        }
    }

    if let Some(state_rows) = state_res {
        if !state_rows.is_empty() {
            let state_lines: Vec<String> = state_rows
                .iter()
                .map(|s| {
                    if s.updated_count > 1 {
                        format!("- {}: {} (updated {}x)", s.key, s.value, s.updated_count)
                    } else {
                        format!("- {}: {}", s.key, s.value)
                    }
                })
                .collect();
            supplementary.push(SupplementarySection {
                label: "current_state".to_string(),
                content: format!("## Current State\n{}", state_lines.join("\n")),
            });
        }
    }

    if let Some((profile, _is_stale)) = personality_res {
        let tokens = estimate_tokens(&profile);
        if tokens <= (token_budget as f64 * 0.10) as usize {
            supplementary.push(SupplementarySection {
                label: "personality".to_string(),
                content: format!("## Personality\n{}", profile),
            });
            personality_block_tokens = tokens;
            used_tokens += tokens;
        }
    }

    if let Some(pref_rows) = pref_res {
        if !pref_rows.is_empty() {
            let pref_lines: Vec<String> = pref_rows
                .iter()
                .map(|p| format!("- [{}] {}", p.domain, p.preference))
                .collect();
            supplementary.push(SupplementarySection {
                label: "preferences".to_string(),
                content: format!("## User Preferences\n{}", pref_lines.join("\n")),
            });
        }
    }

    // Structured facts
    if flags.include_structured_facts {
        let mem_ids: Vec<i64> = blocks.iter().map(|b| b.id).collect();
        if !mem_ids.is_empty() {
            if let Ok(sf_rows) = get_structured_facts(db, &mem_ids, user_id).await {
                if !sf_rows.is_empty() {
                    let now = chrono::Utc::now().timestamp_millis();
                    let stale_ms: i64 = 90 * 24 * 60 * 60 * 1000;
                    let year_ms: f64 = 365.0 * 24.0 * 60.0 * 60.0 * 1000.0;

                    let mut scored: Vec<(&StructuredFact, f64, bool)> = sf_rows
                        .iter()
                        .map(|sf| {
                            let freshness = parse_freshness(
                                sf.valid_at.as_deref(),
                                sf.date_approx.as_deref(),
                                now,
                                year_ms,
                            );
                            let is_stale = sf.valid_at.as_ref().is_some_and(|va| {
                                parse_date_ms(va)
                                    .map(|ms| now - ms > stale_ms)
                                    .unwrap_or(false)
                            });
                            (sf, freshness, is_stale)
                        })
                        .collect();

                    scored
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                    let sf_lines: Vec<String> = scored
                        .iter()
                        .map(|(sf, _, is_stale)| format_structured_fact(sf, *is_stale))
                        .collect();

                    supplementary.push(SupplementarySection {
                        label: "structured_facts".to_string(),
                        content: format!("## Extracted Facts\n{}", sf_lines.join("\n")),
                    });
                }
            }
        }
    }

    let context_string = assemble_context_string(&blocks, &supplementary);
    timing.assembly_ms = Some(t_assembly.elapsed().as_millis() as u64);
    timing.total_ms = Some(t0.elapsed().as_millis() as u64);

    // Defer access tracking (scoped to user_id)
    let block_ids: Vec<i64> = blocks.iter().filter(|b| b.id > 0).map(|b| b.id).collect();
    track_access(db, &block_ids, user_id).await;

    // Build breakdown
    let breakdown = ContextBreakdown {
        static_count: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Static)
            .count(),
        semantic: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Semantic)
            .count(),
        evolution: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Evolution)
            .count(),
        episode: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Episode)
            .count(),
        linked: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Linked)
            .count(),
        recent: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Recent)
            .count(),
        inference: blocks
            .iter()
            .filter(|b| b.source == ContextBlockSource::Inference)
            .count(),
        personality: if personality_block_tokens > 0 { 1 } else { 0 },
    };

    let block_summaries: Vec<ContextBlockSummary> = blocks
        .iter()
        .map(|b| ContextBlockSummary {
            id: b.id,
            category: b.category.clone(),
            source: b.source,
            model: b.model.clone(),
            origin: b.origin.clone(),
            score: (b.score * 100.0).round() / 100.0,
            tokens: b.tokens,
        })
        .collect();

    let utilization = if token_budget > 0 {
        (used_tokens as f64 / token_budget as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };

    Ok(ContextResult {
        context: context_string,
        blocks: block_summaries,
        token_estimate: used_tokens,
        token_budget,
        utilization,
        strategy: context_strategy,
        breakdown,
        timing,
    })
}

// ---------------------------------------------------------------------------
// Helper: parse a date string to epoch milliseconds
// ---------------------------------------------------------------------------

fn parse_date_ms(s: &str) -> Option<i64> {
    let normalized = if s.contains('Z') {
        s.to_string()
    } else {
        format!("{}Z", s.replace(" ", "T"))
    };
    normalized
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn parse_freshness(
    valid_at: Option<&str>,
    date_approx: Option<&str>,
    now: i64,
    year_ms: f64,
) -> f64 {
    if let Some(va) = valid_at {
        if let Some(ms) = parse_date_ms(va) {
            let age = now - ms;
            return if age < 0 {
                1.0
            } else {
                (1.0 - age as f64 / year_ms).max(0.1)
            };
        }
    }
    if let Some(da) = date_approx {
        if let Some(ms) = parse_date_ms(da) {
            let age = now - ms;
            return if age < 0 {
                1.0
            } else {
                (1.0 - age as f64 / year_ms).max(0.1)
            };
        }
    }
    0.5
}

fn format_structured_fact(sf: &StructuredFact, is_stale: bool) -> String {
    let mut line = format!("- {} {}", sf.subject, sf.verb);
    if let Some(ref obj) = sf.object {
        line.push_str(&format!(" {}", obj));
    }
    if let Some(qty) = sf.quantity {
        if let Some(ref unit) = sf.unit {
            line.push_str(&format!(" (qty: {} {})", qty, unit));
        } else {
            line.push_str(&format!(" (qty: {})", qty));
        }
    }
    if let Some(ref va) = sf.valid_at {
        line.push_str(&format!(" [{}]", va));
    } else if let Some(ref da) = sf.date_approx {
        line.push_str(&format!(" [{}]", da));
    } else if let Some(ref dr) = sf.date_ref {
        line.push_str(&format!(" [{}]", dr));
    }
    if is_stale {
        line.push_str(" [possibly outdated]");
    }
    line
}
