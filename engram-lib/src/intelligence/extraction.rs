//! Fact extraction -- regex-based extraction of structured facts, preferences, and state.
//!
//! Ported from intelligence/extraction.ts. Pure regex, no LLM needed.

use crate::db::Database;
use crate::intelligence::types::ExtractionStats;
use crate::Result;
use regex::Regex;
use tracing::{debug, warn};

/// Extract structured facts, preferences, and state updates from memory content.
pub async fn fast_extract_facts(
    db: &Database,
    content: &str,
    memory_id: i64,
    user_id: i64,
    episode_id: Option<i64>,
) -> Result<ExtractionStats> {
    let conn = db.connection();
    let mut stats = ExtractionStats::default();

    // Extract date context from content
    let date_approx = extract_date_approx(content);
    let date_ref = extract_date_ref(content);

    // -- Pattern 1: bought/purchased N items --
    let buy_re = Regex::new(
        r"(?i)\b(bought|purchased|got|acquired|received|ordered|picked up)\s+(\d+)\s+(.+?)(?:\.|,|$)"
    ).unwrap();
    for cap in buy_re.captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let quantity: i64 = cap[2].parse().unwrap_or(0);
        let object = cap[3].trim();
        if object.len() > 200 {
            continue;
        }
        if insert_fact(
            conn,
            memory_id,
            "user",
            &verb,
            object,
            Some(quantity),
            None,
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Pattern 2: spent $N on X --
    let spent_re = Regex::new(r"(?i)\bspent\s+\$([\d,.]+)\s+(?:on|for)\s+(.+?)(?:\.|,|$)").unwrap();
    for cap in spent_re.captures_iter(content) {
        let amount: f64 = cap[1].replace(',', "").parse().unwrap_or(0.0);
        let object = cap[2].trim();
        if insert_fact(
            conn,
            memory_id,
            "user",
            "spent",
            object,
            Some(amount as i64),
            Some("dollars"),
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Pattern 3: have/own N X --
    let have_re = Regex::new(
        r"(?i)\b(?:I\s+)?(?:have|has|own|got)\s+(\d+)\s+(.+?)(?:\.|,|\s+(?:and|but|so|now))",
    )
    .unwrap();
    for cap in have_re.captures_iter(content) {
        let quantity: i64 = cap[1].parse().unwrap_or(0);
        let object = cap[2].trim();
        if insert_fact(
            conn,
            memory_id,
            "user",
            "has",
            object,
            Some(quantity),
            None,
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Pattern 4: exercised for N time --
    let exercise_re = Regex::new(
        r"(?i)\b(ran|jogged|walked|hiked|swam|cycled|biked|exercised)\s+(?:for\s+)?(\d+(?:\.\d+)?)\s+(hours?|minutes?|mins?|miles?|km)"
    ).unwrap();
    for cap in exercise_re.captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let quantity: f64 = cap[2].parse().unwrap_or(0.0);
        let unit = cap[3].to_lowercase();
        if insert_fact(
            conn,
            memory_id,
            "user",
            &verb,
            "",
            Some(quantity as i64),
            Some(&unit),
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Pattern 5: made/baked/cooked X --
    let made_re = Regex::new(
        r"(?i)\b(made|baked|cooked|prepared)\s+(?:a\s+|some\s+)?(.+?)(?:\.|,|\s+(?:and|but|for|from|yesterday|today|last))"
    ).unwrap();
    for cap in made_re.captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let object = cap[2].trim();
        if insert_fact(
            conn,
            memory_id,
            "user",
            &verb,
            object,
            Some(1),
            None,
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Pattern 6: earned/made $N --
    let earned_re = Regex::new(
        r"(?i)\b(?:earned|made|received|got paid)\s+\$([\d,.]+)(?:\s+(?:from|for|by)\s+(.+?))?(?:\.|,|$)"
    ).unwrap();
    for cap in earned_re.captures_iter(content) {
        let amount: f64 = cap[1].replace(',', "").parse().unwrap_or(0.0);
        let object = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if insert_fact(
            conn,
            memory_id,
            "user",
            "earned",
            object,
            Some(amount as i64),
            Some("dollars"),
            date_ref.as_deref(),
            date_approx.as_deref(),
            user_id,
        )
        .await
        .is_ok()
        {
            stats.facts += 1;
        }
    }

    // -- Preferences: likes/enjoys --
    let like_re = Regex::new(
        r"(?i)\b(?:I\s+)?(love|like|enjoy|prefer|adore)\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
    ).unwrap();
    for cap in like_re.captures_iter(content) {
        let object = cap[2].trim();
        if object.len() > 3 && object.len() < 100 {
            let domain = infer_domain(object);
            if upsert_preference(
                conn,
                &domain,
                &format!("likes {}", object),
                memory_id,
                user_id,
            )
            .await
            .is_ok()
            {
                stats.preferences += 1;
            }
        }
    }

    // -- Preferences: dislikes --
    let dislike_re = Regex::new(
        r"(?i)\b(?:I\s+)?(hate|dislike|don't like|can't stand|avoid)\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
    ).unwrap();
    for cap in dislike_re.captures_iter(content) {
        let object = cap[2].trim();
        if object.len() > 3 && object.len() < 100 {
            let domain = infer_domain(object);
            if upsert_preference(
                conn,
                &domain,
                &format!("dislikes {}", object),
                memory_id,
                user_id,
            )
            .await
            .is_ok()
            {
                stats.preferences += 1;
            }
        }
    }

    // -- Preferences: favorites --
    let fav_re = Regex::new(r"(?i)\bmy favorite\s+(.+?)\s+(?:is|are)\s+(.+?)(?:\.|,|$)").unwrap();
    for cap in fav_re.captures_iter(content) {
        let category = cap[1].trim().to_lowercase();
        let value = cap[2].trim();
        if upsert_preference(
            conn,
            &category,
            &format!("favorite: {}", value),
            memory_id,
            user_id,
        )
        .await
        .is_ok()
        {
            stats.preferences += 1;
        }
    }

    // -- State updates: location changes --
    let loc_re = Regex::new(
        r"(?i)\b(?:I\s+)?(?:moved to|relocated to|live in|living in|staying in)\s+(.+?)(?:\.|,|$)",
    )
    .unwrap();
    for cap in loc_re.captures_iter(content) {
        let location = cap[1].trim();
        if upsert_state(conn, "current_location", location, memory_id, user_id)
            .await
            .is_ok()
        {
            stats.state_updates += 1;
        }
    }

    // -- State updates: role changes --
    let role_re = Regex::new(
        r"(?i)\b(?:I\s+)?(?:started|began|got promoted to|now work as|am now|just became)\s+(?:a\s+|an\s+|my\s+)?(.+?)(?:\.|,|$)"
    ).unwrap();
    for cap in role_re.captures_iter(content) {
        let role = cap[1].trim();
        if role.len() > 3
            && role.len() < 100
            && upsert_state(conn, "current_role", role, memory_id, user_id)
                .await
                .is_ok()
        {
            stats.state_updates += 1;
        }
    }

    // Stamp episode provenance on newly extracted facts
    if stats.facts > 0 {
        if let Some(ep_id) = episode_id {
            let _ = conn
                .execute(
                    "UPDATE structured_facts SET episode_id = ?1 WHERE memory_id = ?2 AND episode_id IS NULL",
                    libsql::params![ep_id, memory_id],
                )
                .await;
        }
    }

    if stats.facts + stats.preferences + stats.state_updates > 0 {
        debug!(
            memory_id,
            facts = stats.facts,
            prefs = stats.preferences,
            state = stats.state_updates,
            "fast_extract"
        );
    }

    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
async fn insert_fact(
    conn: &libsql::Connection,
    memory_id: i64,
    subject: &str,
    verb: &str,
    object: &str,
    quantity: Option<i64>,
    unit: Option<&str>,
    date_ref: Option<&str>,
    _date_approx: Option<&str>,
    user_id: i64,
) -> std::result::Result<(), ()> {
    conn.execute(
        "INSERT INTO structured_facts (memory_id, subject, predicate, object, confidence, user_id) \
         VALUES (?1, ?2, ?3, ?4, 1.0, ?5)",
        libsql::params![
            memory_id,
            subject.to_string(),
            verb.to_string(),
            format!(
                "{}{}{}{}",
                object,
                quantity.map(|q| format!(" [qty:{}]", q)).unwrap_or_default(),
                unit.map(|u| format!(" [unit:{}]", u)).unwrap_or_default(),
                date_ref.map(|d| format!(" [date:{}]", d)).unwrap_or_default(),
            ),
            user_id
        ],
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        warn!(error = %e, "fact_insert_failed");
    })
}

async fn upsert_preference(
    conn: &libsql::Connection,
    domain: &str,
    preference: &str,
    memory_id: i64,
    user_id: i64,
) -> std::result::Result<(), ()> {
    conn.execute(
        "INSERT INTO user_preferences (user_id, key, value, created_at, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now'), datetime('now')) \
         ON CONFLICT(user_id, key) DO UPDATE SET \
           value = excluded.value, \
           updated_at = datetime('now')",
        libsql::params![
            user_id,
            format!("{}:{}", domain, preference),
            format!("evidence_memory_id:{}", memory_id)
        ],
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        warn!(error = %e, "preference_upsert_failed");
    })
}

async fn upsert_state(
    conn: &libsql::Connection,
    key: &str,
    value: &str,
    memory_id: i64,
    user_id: i64,
) -> std::result::Result<(), ()> {
    conn.execute(
        "INSERT INTO current_state (agent, key, value, user_id, created_at, updated_at) \
         VALUES ('system', ?1, ?2, ?3, datetime('now'), datetime('now')) \
         ON CONFLICT(agent, key, user_id) DO UPDATE SET \
           value = excluded.value, \
           updated_at = datetime('now')",
        libsql::params![
            key.to_string(),
            format!("{} (memory:{})", value, memory_id),
            user_id
        ],
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        warn!(error = %e, "state_upsert_failed");
    })
}

fn extract_date_approx(content: &str) -> Option<String> {
    let re = Regex::new(r"\[Conversation date:\s*([\d/]+)\]").ok()?;
    re.captures(content).map(|c| c[1].replace('/', "-"))
}

fn extract_date_ref(content: &str) -> Option<String> {
    let re = Regex::new(
        r"(?i)\b(yesterday|today|last (?:week|month|Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)|two (?:days|weeks|months) ago|a (?:week|month) ago|(?:this|last) (?:morning|afternoon|evening))\b"
    ).ok()?;
    re.captures(content).map(|c| c[1].to_string())
}

fn infer_domain(text: &str) -> String {
    let t = text.to_lowercase();
    if Regex::new(r"(?i)food|cook|bak|\beat\b|restaurant|meal|recipe|cuisine")
        .unwrap()
        .is_match(&t)
    {
        return "food".to_string();
    }
    if Regex::new(r"(?i)movie|film|show|series|watch|tv|netflix|stream")
        .unwrap()
        .is_match(&t)
    {
        return "entertainment".to_string();
    }
    if Regex::new(r"(?i)book|read|novel|author|library")
        .unwrap()
        .is_match(&t)
    {
        return "reading".to_string();
    }
    if Regex::new(r"(?i)music|song|album|artist|band|listen|concert")
        .unwrap()
        .is_match(&t)
    {
        return "music".to_string();
    }
    if Regex::new(r"(?i)travel|trip|visit|vacation|hotel|flight|city|country")
        .unwrap()
        .is_match(&t)
    {
        return "travel".to_string();
    }
    if Regex::new(r"(?i)sport|run|jog|gym|exercise|hike|swim|yoga|fitness")
        .unwrap()
        .is_match(&t)
    {
        return "fitness".to_string();
    }
    if Regex::new(r"(?i)game|play|gaming|console")
        .unwrap()
        .is_match(&t)
    {
        return "gaming".to_string();
    }
    if Regex::new(r"(?i)tech|code|program|software|app|computer")
        .unwrap()
        .is_match(&t)
    {
        return "technology".to_string();
    }
    if Regex::new(r"(?i)pet|dog|cat|fish|animal|tank|aquarium")
        .unwrap()
        .is_match(&t)
    {
        return "pets".to_string();
    }
    "general".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_date_approx() {
        let content = "[Conversation date: 2026/04/08] Hello world";
        assert_eq!(extract_date_approx(content), Some("2026-04-08".to_string()));
    }

    #[test]
    fn test_extract_date_ref() {
        let content = "I did it yesterday and it was great";
        assert_eq!(extract_date_ref(content), Some("yesterday".to_string()));
    }

    #[test]
    fn test_infer_domain() {
        assert_eq!(infer_domain("I love cooking pasta"), "food");
        assert_eq!(infer_domain("great movie"), "entertainment");
        assert_eq!(infer_domain("running shoes"), "fitness");
        assert_eq!(infer_domain("random thing"), "general");
    }

    #[test]
    fn test_buy_pattern() {
        let re = Regex::new(
            r"(?i)\b(bought|purchased|got|acquired|received|ordered|picked up)\s+(\d+)\s+(.+?)(?:\.|,|$)"
        ).unwrap();
        let content = "I bought 3 monitors for the office.";
        let caps: Vec<_> = re.captures_iter(content).collect();
        assert_eq!(caps.len(), 1);
        assert_eq!(&caps[0][2], "3");
    }

    #[test]
    fn test_like_pattern() {
        let re = Regex::new(
            r"(?i)\b(?:I\s+)?(love|like|enjoy|prefer|adore)\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
        ).unwrap();
        let content = "I love programming and building systems.";
        let caps: Vec<_> = re.captures_iter(content).collect();
        assert!(!caps.is_empty());
    }
}
