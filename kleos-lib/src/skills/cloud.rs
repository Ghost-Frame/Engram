//! Skills Cloud bridge -- dispatches the legacy `/skills/cloud/*` route
//! surface (originally a federated peer registry) to the local Skills Cloud
//! v50 implementation. Searches go through `find_skills` (hybrid FTS +
//! alias + fuzzy + vector ranking) and uploads go through `create_skill`
//! against the local skill registry. No outbound HTTP.

use super::create_skill;
use super::get_skill_tags;
use super::search::{find_skills, FindOpts};
use super::types::{CloudSearchResult, CreateSkillRequest};
use crate::db::Database;
use crate::Result;

/// Search the local Skills Cloud (v50) and return results in the
/// `CloudSearchResult` shape expected by `/skills/cloud/search` clients.
///
/// `user_id` scopes the tag lookup per result -- the underlying skill rows
/// are tenant-scoped through `db`, so cross-tenant leakage is not possible
/// on a sharded DB.
#[tracing::instrument(skip(db, query), fields(query_len = query.len(), limit, user_id))]
pub async fn search_skills_cloud(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<CloudSearchResult>> {
    let opts = FindOpts {
        limit: Some(limit),
        ..Default::default()
    };
    let results = find_skills(db, query, user_id, opts).await?;

    let mut out = Vec::with_capacity(results.len());
    for r in results {
        // Per-skill tag lookup. N small indexed queries; capped by `limit`.
        let tags = get_skill_tags(db, r.skill.id, user_id)
            .await
            .unwrap_or_default();
        out.push(CloudSearchResult {
            skill_id: r.skill.id.to_string(),
            name: r.skill.name,
            description: r.skill.description.unwrap_or_default(),
            // `category` historically maps to the skill's programming
            // language label (e.g. "rust", "markdown") for parity with the
            // sync-to-cloud path which writes `skill.language` into the
            // category field.
            category: r.skill.language,
            origin: "local".to_string(),
            tags,
            score: r.score,
        });
    }
    Ok(out)
}

/// Create a new skill in the local Skills Cloud (v50) via `create_skill`,
/// returning the new row's id as a string for `/skills/cloud/upload`
/// response compatibility.
#[tracing::instrument(
    skip(db, description, content, tags),
    fields(name = %name, category = %category, tags_count = tags.len(), content_len = content.len(), user_id)
)]
#[allow(clippy::too_many_arguments)]
pub async fn upload_skill_to_cloud(
    db: &Database,
    agent: &str,
    user_id: i64,
    name: &str,
    description: &str,
    content: &str,
    category: &str,
    tags: &[String],
) -> Result<String> {
    let req = CreateSkillRequest {
        name: name.to_string(),
        agent: agent.to_string(),
        description: Some(description.to_string()),
        code: content.to_string(),
        language: Some(category.to_string()),
        parent_skill_id: None,
        metadata: None,
        user_id: Some(user_id),
        tags: Some(tags.to_vec()),
        tool_deps: None,
        kind: None,
        source_plugin: None,
        source_path: None,
        content_hash: None,
    };
    let skill = create_skill(db, req).await?;
    Ok(skill.id.to_string())
}

// Unit tests live with the underlying `find_skills` (search.rs) and
// `create_skill` (skills/mod.rs) implementations. Integration coverage of
// the `/skills/cloud/search` and `/skills/cloud/upload` routes lives in
// `kleos-server/tests/`.
