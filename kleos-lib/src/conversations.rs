use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::db::Database;
use crate::sessions::scrub::scrub_message;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

const CONVERSATION_COLUMNS: &str = "id, agent, session_id, title, metadata, started_at, updated_at";

const CONVERSATION_LIST_COLUMNS: &str =
    "c.id, c.agent, c.session_id, c.title, c.metadata, c.started_at, c.updated_at, \
     (SELECT COUNT(*) FROM messages WHERE conversation_id = c.id) as message_count";

const MESSAGE_COLUMNS: &str = "id, conversation_id, role, content, metadata, created_at";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: i64,
    pub agent: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListItem {
    pub id: i64,
    pub agent: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<String>,
    pub started_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSearchResult {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: String,
    pub agent: String,
    pub conv_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversationRequest {
    pub agent: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConversationRequest {
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMessageRequest {
    pub role: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkMessageInput {
    pub role: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkInsertRequest {
    pub agent: String,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub messages: Vec<BulkMessageInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertConversationRequest {
    pub agent: String,
    pub session_id: String,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub messages: Option<Vec<BulkMessageInput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMessagesRequest {
    pub query: String,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn row_to_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        agent: row.get(1)?,
        session_id: row.get(2)?,
        title: row.get(3)?,
        metadata: row.get(4)?,
        started_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_conversation_list_item(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConversationListItem> {
    Ok(ConversationListItem {
        id: row.get(0)?,
        agent: row.get(1)?,
        session_id: row.get(2)?,
        title: row.get(3)?,
        metadata: row.get(4)?,
        started_at: row.get(5)?,
        updated_at: row.get(6)?,
        message_count: row.get(7)?,
    })
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        metadata: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_message_search_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageSearchResult> {
    Ok(MessageSearchResult {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        metadata: row.get(4)?,
        created_at: row.get(5)?,
        agent: row.get(6)?,
        conv_title: row.get(7)?,
    })
}

fn sanitize_fts_query(query: &str) -> String {
    let sanitized: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    sanitized
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn metadata_to_string(meta: &Option<serde_json::Value>) -> Option<String> {
    meta.as_ref().map(|v| v.to_string())
}

// ---------------------------------------------------------------------------
// Conversation CRUD
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, req), fields(agent = %req.agent, session_id = ?req.session_id, user_id))]
pub async fn create_conversation(
    db: &Database,
    req: CreateConversationRequest,
    user_id: i64,
) -> Result<Conversation> {
    let meta_str = metadata_to_string(&req.metadata);
    let agent = req.agent.clone();
    let session_id = req.session_id.clone();
    let title = req.title.clone();
    let new_id: i64 = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO conversations (agent, session_id, title, metadata) VALUES (?1, ?2, ?3, ?4)",
                params![agent, session_id, title, meta_str],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    get_conversation_for_user(db, new_id, user_id).await
}

#[tracing::instrument(skip(db), fields(conversation_id = id, user_id))]
pub async fn get_conversation_for_user(
    db: &Database,
    id: i64,
    user_id: i64,
) -> Result<Conversation> {
    let sql = format!(
        "SELECT {} FROM conversations WHERE id = ?1",
        CONVERSATION_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![id], row_to_conversation)
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("conversation {} not found", id)))
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent, session_id = %session_id, user_id))]
pub async fn get_conversation_by_session(
    db: &Database,
    agent: &str,
    session_id: &str,
    user_id: i64,
) -> Result<Option<Conversation>> {
    let agent = agent.to_string();
    let session_id = session_id.to_string();
    let sql = format!(
        "SELECT {} FROM conversations WHERE agent = ?1 AND session_id = ?2 ORDER BY started_at DESC LIMIT 1",
        CONVERSATION_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![agent, session_id], |row| {
            row_to_conversation(row)
        })
        .optional()
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn list_conversations(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ConversationListItem>> {
    let sql = format!(
        "SELECT {} FROM conversations c ORDER BY c.updated_at DESC LIMIT ?1",
        CONVERSATION_LIST_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                row_to_conversation_list_item(row)
            })
            .map_err(rusqlite_to_eng_error)?;
        let mut convs = Vec::new();
        for row in rows {
            convs.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(convs)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, agent = %agent, limit))]
pub async fn list_conversations_by_agent(
    db: &Database,
    user_id: i64,
    agent: &str,
    limit: usize,
) -> Result<Vec<ConversationListItem>> {
    let agent = agent.to_string();
    let sql = format!(
        "SELECT {} FROM conversations c WHERE c.agent = ?1 ORDER BY c.updated_at DESC LIMIT ?2",
        CONVERSATION_LIST_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![agent, limit as i64], |row| {
                row_to_conversation_list_item(row)
            })
            .map_err(rusqlite_to_eng_error)?;
        let mut convs = Vec::new();
        for row in rows {
            convs.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(convs)
    })
    .await
}

#[tracing::instrument(skip(db, req), fields(conversation_id = id, user_id))]
pub async fn update_conversation(
    db: &Database,
    id: i64,
    user_id: i64,
    req: UpdateConversationRequest,
) -> Result<Conversation> {
    let meta_str = metadata_to_string(&req.metadata);
    let title = req.title.clone();
    db.write(move |conn| {
        conn.execute(
            "UPDATE conversations SET title = COALESCE(?1, title), metadata = COALESCE(?2, metadata), \
             updated_at = datetime('now') WHERE id = ?3",
            params![title, meta_str, id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;
    get_conversation_for_user(db, id, user_id).await
}

#[tracing::instrument(skip(db), fields(conversation_id = id, user_id))]
pub async fn delete_conversation(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])
                .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("conversation {} not found", id)));
    }
    Ok(())
}

#[tracing::instrument(skip(db), fields(conversation_id = id, user_id))]
pub async fn touch_conversation(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Message operations
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, credd, req), fields(conversation_id, user_id, role = %req.role))]
pub async fn add_message(
    db: &Database,
    credd: &crate::cred::CreddClient,
    conversation_id: i64,
    user_id: i64,
    req: AddMessageRequest,
) -> Result<Message> {
    // Defense-in-depth: verify conversation ownership at the library layer
    // so callers that skip the route-level check cannot write to another
    // tenant's conversation.
    let conversation = get_conversation_for_user(db, conversation_id, user_id).await?;
    let meta_str = metadata_to_string(&req.metadata);
    let content = scrub_message(db, credd, user_id, &conversation.agent, &req.content).await?;
    let role = req.role.clone();
    let new_id: i64 = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO messages (conversation_id, role, content, metadata) VALUES (?1, ?2, ?3, ?4)",
                params![conversation_id, role, content, meta_str],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    // Touch the conversation updated_at (scoped by user_id).
    let _ = touch_conversation(db, conversation_id, user_id).await;
    let qualified_cols = MESSAGE_COLUMNS
        .split(", ")
        .map(|c| format!("m.{}", c))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {} FROM messages m \
         INNER JOIN conversations c ON m.conversation_id = c.id \
         WHERE m.id = ?1",
        qualified_cols
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![new_id], row_to_message)
            .optional()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("failed to fetch newly created message".into()))
    })
    .await
}

#[tracing::instrument(skip(db), fields(conversation_id, user_id, limit, offset))]
pub async fn list_messages(
    db: &Database,
    conversation_id: i64,
    user_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<Message>> {
    // Defense-in-depth: route layer also calls get_conversation_for_user
    // before invoking this, but library functions must not trust callers.
    let sql = format!(
        "SELECT {} FROM messages m
         INNER JOIN conversations c ON m.conversation_id = c.id
         WHERE m.conversation_id = ?1
         ORDER BY m.created_at ASC LIMIT ?2 OFFSET ?3",
        MESSAGE_COLUMNS
            .split(", ")
            .map(|c| format!("m.{}", c))
            .collect::<Vec<_>>()
            .join(", ")
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(
                params![conversation_id, limit as i64, offset as i64],
                row_to_message,
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut msgs = Vec::new();
        for row in rows {
            msgs.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(msgs)
    })
    .await
}

#[tracing::instrument(skip(db, req), fields(user_id, limit = ?req.limit))]
pub async fn search_messages(
    db: &Database,
    req: SearchMessagesRequest,
    user_id: i64,
) -> Result<Vec<MessageSearchResult>> {
    let limit = req.limit.unwrap_or(20).min(100);
    let sanitized = sanitize_fts_query(&req.query);
    if sanitized.is_empty() {
        return Ok(vec![]);
    }
    let sql = "SELECT m.id, m.conversation_id, m.role, m.content, m.metadata, m.created_at, \
         c.agent, c.title as conv_title \
         FROM messages_fts f \
         JOIN messages m ON f.rowid = m.id \
         JOIN conversations c ON m.conversation_id = c.id \
         WHERE messages_fts MATCH ?1 \
         ORDER BY m.created_at DESC LIMIT ?2"
        .to_string();
    match db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![sanitized, limit as i64], |row| {
                    row_to_message_search_result(row)
                })
                .map_err(rusqlite_to_eng_error)?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(results)
        })
        .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            warn!("message FTS search failed: {}", e);
            Ok(vec![])
        }
    }
}

// ---------------------------------------------------------------------------
// Bulk and Upsert
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, credd, req), fields(agent = %req.agent, message_count = req.messages.len(), user_id))]
pub async fn bulk_insert_conversation(
    db: &Database,
    credd: &crate::cred::CreddClient,
    req: BulkInsertRequest,
    user_id: i64,
) -> Result<Conversation> {
    let meta_str = metadata_to_string(&req.metadata);
    let agent = req.agent.clone();
    let session_id = req.session_id.clone();
    let title = req.title.clone();
    // INSERT ... RETURNING avoids the cross-connection last_insert_rowid race.
    let conv_id: i64 = db
        .write(move |conn| {
            conn.query_row(
                "INSERT INTO conversations (agent, session_id, title, metadata) \
                 VALUES (?1, ?2, ?3, ?4) RETURNING id",
                params![agent, session_id, title, meta_str],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    for msg in req.messages {
        add_message(
            db,
            credd,
            conv_id,
            user_id,
            AddMessageRequest {
                role: msg.role,
                content: msg.content,
                metadata: msg.metadata,
            },
        )
        .await?;
    }
    get_conversation_for_user(db, conv_id, user_id).await
}

#[tracing::instrument(skip(db, credd, req), fields(agent = %req.agent, session_id = %req.session_id, user_id))]
pub async fn upsert_conversation(
    db: &Database,
    credd: &crate::cred::CreddClient,
    req: UpsertConversationRequest,
    user_id: i64,
) -> Result<Conversation> {
    // Try to find existing conversation by agent + session_id + user_id
    let existing = get_conversation_by_session(db, &req.agent, &req.session_id, user_id).await?;
    let conv = if let Some(existing) = existing {
        // Update title/metadata if provided
        if req.title.is_some() || req.metadata.is_some() {
            update_conversation(
                db,
                existing.id,
                user_id,
                UpdateConversationRequest {
                    title: req.title,
                    metadata: req.metadata,
                },
            )
            .await?
        } else {
            existing
        }
    } else {
        // Create new
        create_conversation(
            db,
            CreateConversationRequest {
                agent: req.agent,
                session_id: Some(req.session_id),
                title: req.title,
                metadata: req.metadata,
            },
            user_id,
        )
        .await?
    };
    // Insert any messages
    if let Some(messages) = req.messages {
        for msg in messages {
            add_message(
                db,
                credd,
                conv.id,
                user_id,
                AddMessageRequest {
                    role: msg.role,
                    content: msg.content,
                    metadata: msg.metadata,
                },
            )
            .await?;
        }
        let _ = touch_conversation(db, conv.id, user_id).await;
    }
    get_conversation_for_user(db, conv.id, user_id).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::scrub::apply_scrub;

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(sanitize_fts_query("hello world"), "hello world");
        assert_eq!(sanitize_fts_query("hello-world!"), "hello world");
        assert_eq!(sanitize_fts_query("a b cd"), "cd");
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn test_metadata_to_string() {
        let none: Option<serde_json::Value> = None;
        assert_eq!(metadata_to_string(&none), None);
        let some = Some(serde_json::json!({"key": "value"}));
        let result = metadata_to_string(&some);
        assert!(result.is_some());
        assert!(result.unwrap().contains("key"));
    }

    #[test]
    fn test_apply_scrub_redacts_known_secret() {
        let result = apply_scrub("token alpha-secret seen", &["alpha-secret".to_string()]);
        assert_eq!(result, "token [REDACTED] seen");
    }

    #[test]
    fn test_apply_scrub_leaves_clean_text_unchanged() {
        let input = "normal conversation text";
        let result = apply_scrub(input, &["alpha-secret".to_string()]);
        assert_eq!(result, input);
    }
}
