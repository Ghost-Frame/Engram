//! Emotional valence tracking -- sentiment/affect analysis.
//! Ported from tier4/valence.ts.

use crate::db::Database;
use crate::Result;
use libsql::params;
use regex::Regex;
use std::sync::LazyLock;

use super::types::{
    EmotionMatch, EmotionMemory, EmotionStat, EmotionalProfile, OverallEmotionStats, ValenceResult,
};

struct EmotionPattern {
    regex: Regex,
    emotion: &'static str,
    valence: f64,
    arousal: f64,
}

static EMOTION_PATTERNS: LazyLock<Vec<EmotionPattern>> = LazyLock::new(|| {
    vec![
        ep(r"\b(furious|enraged|livid|outraged)\b", "anger", -0.9, 0.9),
        ep(r"\b(angry|pissed|mad|frustrated|annoyed|irritated)\b", "anger", -0.7, 0.7),
        ep(r"\b(terrified|panicked|horrified)\b", "fear", -0.8, 0.9),
        ep(r"\b(anxious|worried|nervous|stressed|afraid|scared)\b", "fear", -0.6, 0.6),
        ep(r"\b(devastated|heartbroken|grief|mourning)\b", "sadness", -0.9, 0.3),
        ep(r"\b(sad|disappointed|depressed|miserable|upset|bummed)\b", "sadness", -0.6, 0.3),
        ep(r"\b(bored|tired|exhausted|drained|burned out|burnt out)\b", "fatigue", -0.3, 0.1),
        ep(r"\b(confused|lost|stuck|puzzled|stumped)\b", "confusion", -0.3, 0.4),
        ep(r"\b(crashed|broken|failed|down|error|bug|issue|problem)\b", "frustration", -0.5, 0.6),
        ep(r"\b(hate|worst|terrible|awful|horrible|garbage|trash)\b", "disgust", -0.8, 0.5),
        ep(r"\b(ecstatic|thrilled|elated|overjoyed)\b", "joy", 0.9, 0.9),
        ep(r"\b(excited|pumped|stoked|hyped|amazing|incredible)\b", "excitement", 0.8, 0.8),
        ep(r"\b(happy|glad|pleased|delighted|great|awesome|fantastic)\b", "joy", 0.7, 0.6),
        ep(r"\b(proud|accomplished|nailed|crushed it|killed it)\b", "pride", 0.7, 0.6),
        ep(r"\b(satisfied|content|good|nice|fine|pleasant|comfortable)\b", "satisfaction", 0.4, 0.3),
        ep(r"\b(calm|relaxed|peaceful|serene|chill)\b", "calm", 0.3, 0.1),
        ep(r"\b(grateful|thankful|appreciate)\b", "gratitude", 0.6, 0.3),
        ep(r"\b(curious|interested|intrigued|fascinated)\b", "curiosity", 0.4, 0.5),
        ep(r"\b(fixed|resolved|working|deployed|shipped|launched|completed|done|finished)\b", "accomplishment", 0.5, 0.5),
        ep(r"\b(love|perfect|beautiful|elegant|clean|brilliant)\b", "admiration", 0.7, 0.4),
        ep(r"\b(surprised|unexpected|wow|whoa)\b", "surprise", 0.0, 0.7),
    ]
});

fn ep(pattern: &str, emotion: &'static str, valence: f64, arousal: f64) -> EmotionPattern {
    EmotionPattern {
        regex: Regex::new(&format!("(?i){}", pattern)).expect("invalid emotion regex"),
        emotion, valence, arousal,
    }
}

pub fn analyze_valence(content: &str) -> ValenceResult {
    let mut matches: Vec<EmotionMatch> = Vec::new();
    for pat in EMOTION_PATTERNS.iter() {
        if pat.regex.is_match(content) {
            matches.push(EmotionMatch {
                emotion: pat.emotion.to_string(), valence: pat.valence, arousal: pat.arousal,
            });
        }
    }
    if matches.is_empty() {
        return ValenceResult { valence: 0.0, arousal: 0.0, dominant_emotion: "neutral".into(), all_emotions: vec![] };
    }
    let total_weight: f64 = matches.iter().map(|m| m.valence.abs()).sum();
    let avg_valence = matches.iter().map(|m| m.valence * m.valence.abs()).sum::<f64>() / total_weight;
    let avg_arousal = matches.iter().map(|m| m.arousal * m.valence.abs()).sum::<f64>() / total_weight;
    let dominant = matches.iter().max_by(|a, b| a.valence.abs().partial_cmp(&b.valence.abs()).unwrap()).unwrap();
    ValenceResult {
        valence: (avg_valence * 100.0).round() / 100.0,
        arousal: (avg_arousal * 100.0).round() / 100.0,
        dominant_emotion: dominant.emotion.clone(),
        all_emotions: matches,
    }
}

pub async fn store_valence(db: &Database, memory_id: i64, content: &str, user_id: i64) -> Result<ValenceResult> {
    let result = analyze_valence(content);
    if result.dominant_emotion != "neutral" {
        db.conn.execute(
            "UPDATE memories SET valence = ?1, arousal = ?2, dominant_emotion = ?3 WHERE id = ?4 AND user_id = ?5",
            params![result.valence, result.arousal, result.dominant_emotion.clone(), memory_id, user_id],
        ).await?;
    }
    Ok(result)
}

pub async fn query_by_emotion(db: &Database, emotion: &str, user_id: i64, limit: i64) -> Result<Vec<EmotionMemory>> {
    let mut rows = db.conn.query(
        "SELECT id, content, category, importance, valence, arousal, dominant_emotion, created_at          FROM memories          WHERE user_id = ?1 AND dominant_emotion = ?2 AND is_forgotten = 0 AND is_archived = 0          ORDER BY ABS(valence) DESC, created_at DESC LIMIT ?3",
        params![user_id, emotion, limit],
    ).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(EmotionMemory {
            id: row.get(0)?, content: row.get(1)?, category: row.get(2)?, importance: row.get(3)?,
            valence: row.get::<Option<f64>>(4)?.unwrap_or(0.0),
            arousal: row.get::<Option<f64>>(5)?.unwrap_or(0.0),
            dominant_emotion: row.get::<Option<String>>(6)?.unwrap_or_default(),
            created_at: row.get(7)?,
        });
    }
    Ok(results)
}

pub async fn get_emotional_profile(db: &Database, user_id: i64) -> Result<EmotionalProfile> {
    let mut rows = db.conn.query(
        "SELECT dominant_emotion, COUNT(*) as count, AVG(valence), AVG(arousal)          FROM memories WHERE user_id = ?1 AND dominant_emotion IS NOT NULL AND is_forgotten = 0          GROUP BY dominant_emotion ORDER BY count DESC",
        params![user_id],
    ).await?;
    let mut emotions = Vec::new();
    while let Some(row) = rows.next().await? {
        emotions.push(EmotionStat {
            dominant_emotion: row.get(0)?, count: row.get(1)?,
            avg_valence: row.get::<Option<f64>>(2)?.unwrap_or(0.0),
            avg_arousal: row.get::<Option<f64>>(3)?.unwrap_or(0.0),
        });
    }
    let mut orows = db.conn.query(
        "SELECT AVG(valence), AVG(arousal),             SUM(CASE WHEN valence > 0.2 THEN 1 ELSE 0 END),             SUM(CASE WHEN valence < -0.2 THEN 1 ELSE 0 END),             SUM(CASE WHEN valence BETWEEN -0.2 AND 0.2 THEN 1 ELSE 0 END)          FROM memories WHERE user_id = ?1 AND valence IS NOT NULL AND is_forgotten = 0",
        params![user_id],
    ).await?;
    let overall = if let Some(row) = orows.next().await? {
        OverallEmotionStats {
            avg_valence: row.get::<Option<f64>>(0)?.unwrap_or(0.0),
            avg_arousal: row.get::<Option<f64>>(1)?.unwrap_or(0.0),
            positive_count: row.get::<Option<i64>>(2)?.unwrap_or(0),
            negative_count: row.get::<Option<i64>>(3)?.unwrap_or(0),
            neutral_count: row.get::<Option<i64>>(4)?.unwrap_or(0),
        }
    } else {
        OverallEmotionStats { avg_valence: 0.0, avg_arousal: 0.0, positive_count: 0, negative_count: 0, neutral_count: 0 }
    };
    Ok(EmotionalProfile { emotions, overall })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positive() {
        let r = analyze_valence("I am so happy and excited about this!");
        assert!(r.valence > 0.0);
        assert!(r.dominant_emotion == "excitement" || r.dominant_emotion == "joy");
    }

    #[test]
    fn test_negative() {
        let r = analyze_valence("The server crashed and I am frustrated");
        assert!(r.valence < 0.0);
        assert_eq!(r.all_emotions.len(), 2);
    }

    #[test]
    fn test_neutral() {
        let r = analyze_valence("The meeting is at 3pm tomorrow");
        assert_eq!(r.valence, 0.0);
        assert_eq!(r.dominant_emotion, "neutral");
    }

    #[test]
    fn test_case_insensitive() {
        let r = analyze_valence("I am FURIOUS about this");
        assert!(r.valence < -0.5);
        assert_eq!(r.dominant_emotion, "anger");
    }
}
