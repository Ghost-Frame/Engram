//! Fact extraction -- regex-based extraction of structured facts, preferences, and state.
//!
//! Ported from intelligence/extraction.ts. Pure regex, no LLM needed.

use std::sync::OnceLock;

use crate::db::Database;
use crate::intelligence::types::ExtractionStats;
use crate::{EngError, Result};
use regex::Regex;
use tracing::{debug, warn};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// Static regex patterns compiled once via OnceLock (DOS-H4 fix).
fn buy_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(bought|purchased|got|acquired|received|ordered|picked up)\s+(\d+)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn spent_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bspent\s+\$([\d,.]+)\s+(?:on|for)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn have_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:I\s+)?(?:have|has|own|got)\s+(\d+)\s+(.+?)(?:\.|,|\s+(?:and|but|so|now))").unwrap()
    })
}

fn exercise_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(ran|jogged|walked|hiked|swam|cycled|biked|exercised)\s+(?:for\s+)?(\d+(?:\.\d+)?)\s+(hours?|minutes?|mins?|miles?|km)").unwrap()
    })
}

fn made_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(made|baked|cooked|prepared)\s+(?:a\s+|some\s+)?(.+?)(?:\.|,|\s+(?:and|but|for|from|yesterday|today|last))").unwrap()
    })
}

fn earned_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(earned|made|received|got)\s+\$([\d,.]+)(?:\s+(?:from|for|in)\s+(.+?))?(?:\.|,|$)").unwrap()
    })
}

fn like_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:I\s+)?(love|like|enjoy|adore|prefer)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn dislike_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:I\s+)?(hate|dislike|can't stand|don't like)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn favorite_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bmy\s+(?:favorite|fav|favourite)\s+(food|movie|book|show|game|song|color|colour|sport|place|drink|artist|band|author)\s+(?:is|are)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn location_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:I\s+)?(?:moved to|relocated to|live in|living in|staying in)\s+(.+?)(?:\.|,|$)").unwrap()
    })
}

fn role_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:I\s+)?(?:started|began|got promoted to|now work as|am now|just became)\s+(?:a\s+|an\s+|my\s+)?(.+?)(?:\.|,|$)").unwrap()
    })
}

// Collected operations to execute in a single transaction
struct FactInsert {
    subject: String,
    verb: String,
    object: String,
}

struct PrefUpsert {
    key: String,
    value: String,
}

struct StateUpsert {
    key: String,
    value: String,
}

/// Extract structured facts, preferences, and state updates from memory content.
pub async fn fast_extract_facts(
    db: &Database,
    content: &str,
    memory_id: i64,
    user_id: i64,
    episode_id: Option<i64>,
) -> Result<ExtractionStats> {
    // Collect all operations first
    let mut facts: Vec<FactInsert> = Vec::new();
    let mut prefs: Vec<PrefUpsert> = Vec::new();
    let mut states: Vec<StateUpsert> = Vec::new();

    // Extract date context from content
    let _date_approx = extract_date_approx(content);
    let date_ref = extract_date_ref(content);

    // -- Pattern 1: bought/purchased N items --
    for cap in buy_regex().captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let quantity: i64 = cap[2].parse().unwrap_or(0);
        let object = cap[3].trim();
        if object.len() > 200 {
            continue;
        }
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb,
            object: format_fact_object(object, Some(quantity), None, date_ref.as_deref()),
        });
    }

    // -- Pattern 2: spent $N on X --
    for cap in spent_regex().captures_iter(content) {
        let amount: f64 = cap[1].replace(',', "").parse().unwrap_or(0.0);
        let object = cap[2].trim();
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb: "spent".to_string(),
            object: format_fact_object(object, Some(amount as i64), Some("dollars"), date_ref.as_deref()),
        });
    }

    // -- Pattern 3: have/own N X --
    for cap in have_regex().captures_iter(content) {
        let quantity: i64 = cap[1].parse().unwrap_or(0);
        let object = cap[2].trim();
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb: "has".to_string(),
            object: format_fact_object(object, Some(quantity), None, date_ref.as_deref()),
        });
    }

    // -- Pattern 4: exercised for N time --
    for cap in exercise_regex().captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let quantity: f64 = cap[2].parse().unwrap_or(0.0);
        let unit = cap[3].to_lowercase();
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb,
            object: format_fact_object("", Some(quantity as i64), Some(&unit), date_ref.as_deref()),
        });
    }

    // -- Pattern 5: made/baked/cooked X --
    for cap in made_regex().captures_iter(content) {
        let verb = cap[1].to_lowercase();
        let object = cap[2].trim();
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb,
            object: format_fact_object(object, Some(1), None, date_ref.as_deref()),
        });
    }

    // -- Pattern 6: earned/made $N --
    for cap in earned_regex().captures_iter(content) {
        let amount: f64 = cap[1].replace(',', "").parse().unwrap_or(0.0);
        let object = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        facts.push(FactInsert {
            subject: "user".to_string(),
            verb: "earned".to_string(),
            object: format_fact_object(object, Some(amount as i64), Some("dollars"), date_ref.as_deref()),
        });
    }

    // -- Preferences: likes/enjoys --
    for cap in like_regex().captures_iter(content) {
        let object = cap[2].trim();
        if object.len() > 3 && object.len() < 100 {
            let domain = infer_domain(object);
            prefs.push(PrefUpsert {
                key: format!("{}:likes {}", domain, object),
                value: format!("evidence_memory_id:{}", memory_id),
            });
        }
    }

    // -- Preferences: dislikes --
    for cap in dislike_regex().captures_iter(content) {
        let object = cap[2].trim();
        if object.len() > 3 && object.len() < 100 {
            let domain = infer_domain(object);
            prefs.push(PrefUpsert {
                key: format!("{}:dislikes {}", domain, object),
                value: format!("evidence_memory_id:{}", memory_id),
            });
        }
    }

    // -- Preferences: favorites --
    for cap in favorite_regex().captures_iter(content) {
        let category = cap[1].trim().to_lowercase();
        let value = cap[2].trim();
        prefs.push(PrefUpsert {
            key: format!("{}:favorite: {}", category, value),
            value: format!("evidence_memory_id:{}", memory_id),
        });
    }

    // -- State updates: location changes --
    for cap in location_regex().captures_iter(content) {
        let location = cap[1].trim();
        states.push(StateUpsert {
            key: "current_location".to_string(),
            value: format!("{} (memory:{})", location, memory_id),
        });
    }

    // -- State updates: role changes --
    for cap in role_regex().captures_iter(content) {
        let role = cap[1].trim();
        if role.len() > 3 && role.len() < 100 {
            states.push(StateUpsert {
                key: "current_role".to_string(),
                value: format!("{} (memory:{})", role, memory_id),
            });
        }
    }

    // Execute all operations in a single write
    let fact_count = facts.len();
    let pref_count = prefs.len();
    let state_count = states.len();

    if fact_count + pref_count + state_count > 0 {
        db.write(move |conn| {
            // Insert facts
            for fact in &facts {
                if let Err(e) = conn.execute(
                    "INSERT INTO structured_facts (memory_id, subject, predicate, object, confidence, user_id) \
                     VALUES (?1, ?2, ?3, ?4, 1.0, ?5)",
                    rusqlite::params![memory_id, fact.subject, fact.verb, fact.object, user_id],
                ) {
                    warn!(error = %e, "fact_insert_failed");
                }
            }

            // Upsert preferences
            for pref in &prefs {
                if let Err(e) = conn.execute(
                    "INSERT INTO user_preferences (user_id, key, value, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(user_id, key) DO UPDATE SET \
                       value = excluded.value, \
                       updated_at = datetime('now')",
                    rusqlite::params![user_id, pref.key, pref.value],
                ) {
                    warn!(error = %e, "preference_upsert_failed");
                }
            }

            // Upsert state
            for state in &states {
                if let Err(e) = conn.execute(
                    "INSERT INTO current_state (agent, key, value, user_id, created_at, updated_at) \
                     VALUES ('system', ?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(agent, key, user_id) DO UPDATE SET \
                       value = excluded.value, \
                       updated_at = datetime('now')",
                    rusqlite::params![state.key, state.value, user_id],
                ) {
                    warn!(error = %e, "state_upsert_failed");
                }
            }

            // Stamp episode provenance on newly extracted facts
            if !facts.is_empty() {
                if let Some(ep_id) = episode_id {
                    let _ = conn.execute(
                        "UPDATE structured_facts SET episode_id = ?1
                         WHERE memory_id = ?2 AND user_id = ?3 AND episode_id IS NULL",
                        rusqlite::params![ep_id, memory_id, user_id],
                    );
                }
            }

            Ok(())
        })
        .await?;
    }

    let stats = ExtractionStats {
        facts: fact_count as i32,
        preferences: pref_count as i32,
        state_updates: state_count as i32,
    };

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

fn format_fact_object(
    object: &str,
    quantity: Option<i64>,
    unit: Option<&str>,
    date_ref: Option<&str>,
) -> String {
    format!(
        "{}{}{}{}",
        object,
        quantity.map(|q| format!(" [qty:{}]", q)).unwrap_or_default(),
        unit.map(|u| format!(" [unit:{}]", u)).unwrap_or_default(),
        date_ref.map(|d| format!(" [date:{}]", d)).unwrap_or_default(),
    )
}

fn extract_date_approx(content: &str) -> Option<String> {
    let re = Regex::new(r"\[Conversation date:\s*([\d/]+)\]").ok()?;
    re.captures(content).map(|c| c[1].to_string())
}

fn extract_date_ref(content: &str) -> Option<String> {
    // Look for explicit date references like "yesterday", "last week", specific dates
    let re = Regex::new(r"(?i)\b(yesterday|today|last\s+(?:week|month|year)|(?:on\s+)?(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2}(?:st|nd|rd|th)?(?:\s*,?\s*\d{4})?)\b").ok()?;
    re.find(content).map(|m| m.as_str().to_string())
}

fn infer_domain(object: &str) -> String {
    let lower = object.to_lowercase();
    if lower.contains("food") || lower.contains("eat") || lower.contains("cook") || lower.contains("drink")
        || lower.contains("pizza") || lower.contains("coffee") || lower.contains("breakfast")
        || lower.contains("lunch") || lower.contains("dinner") || lower.contains("snack")
        || lower.contains("restaurant") || lower.contains("recipe") || lower.contains("grocer")
    {
        "food".to_string()
    } else if lower.contains("movie") || lower.contains("show") || lower.contains("series") || lower.contains("watch") {
        "entertainment".to_string()
    } else if lower.contains("book") || lower.contains("read") || lower.contains("author") {
        "reading".to_string()
    } else if lower.contains("music") || lower.contains("song") || lower.contains("band") || lower.contains("listen") {
        "music".to_string()
    } else if lower.contains("game") || lower.contains("play") {
        "gaming".to_string()
    } else if lower.contains("sport") || lower.contains("exercise") || lower.contains("run") || lower.contains("gym") {
        "fitness".to_string()
    } else if lower.contains("travel") || lower.contains("visit") || lower.contains("trip") {
        "travel".to_string()
    } else {
        "general".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buy_regex_captures() {
        let content = "I bought 3 apples yesterday.";
        let caps: Vec<_> = buy_regex().captures_iter(content).collect();
        assert_eq!(caps.len(), 1);
        assert_eq!(&caps[0][1], "bought");
        assert_eq!(&caps[0][2], "3");
        assert_eq!(&caps[0][3], "apples yesterday");
    }

    #[test]
    fn test_spent_regex_captures() {
        let content = "I spent $50.00 on groceries.";
        let caps: Vec<_> = spent_regex().captures_iter(content).collect();
        assert_eq!(caps.len(), 1);
        assert_eq!(&caps[0][1], "50.00");
        assert_eq!(&caps[0][2], "groceries");
    }

    #[test]
    fn test_infer_domain() {
        assert_eq!(infer_domain("pizza"), "food");
        assert_eq!(infer_domain("movie night"), "entertainment");
        assert_eq!(infer_domain("running shoes"), "fitness");
        assert_eq!(infer_domain("random thing"), "general");
    }

    #[test]
    fn test_format_fact_object() {
        assert_eq!(
            format_fact_object("apples", Some(3), None, None),
            "apples [qty:3]"
        );
        assert_eq!(
            format_fact_object("", Some(5), Some("miles"), Some("yesterday")),
            " [qty:5] [unit:miles] [date:yesterday]"
        );
    }
}
