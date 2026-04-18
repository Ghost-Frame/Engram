use super::FSRS_MAX_STORAGE;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ---- Rating ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rating::Again => write!(f, "again"),
            Rating::Hard => write!(f, "hard"),
            Rating::Good => write!(f, "good"),
            Rating::Easy => write!(f, "easy"),
        }
    }
}

impl FromStr for Rating {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "again" | "1" => Ok(Rating::Again),
            "hard" | "2" => Ok(Rating::Hard),
            "good" | "3" => Ok(Rating::Good),
            "easy" | "4" => Ok(Rating::Easy),
            _ => Err(format!("unknown rating: {}", s)),
        }
    }
}

// ---- LearningState ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum LearningState {
    New = 0,
    Learning = 1,
    Review = 2,
    Relearning = 3,
}

// ---- FsrsState ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsrsState {
    pub stability: f32,
    pub difficulty: f32,
    pub storage_strength: f32,
    pub retrieval_strength: f32,
    pub learning_state: LearningState,
    pub reps: i32,
    pub lapses: i32,
    pub last_review_at: String,
}

// ---- DualStrength ----

#[derive(Debug, Clone, Copy)]
pub struct DualStrength {
    pub storage: f32,
    pub retrieval: f32,
}

impl DualStrength {
    pub fn retention(&self) -> f32 {
        (self.retrieval * 0.7) + ((self.storage / FSRS_MAX_STORAGE) * 0.3)
    }

    pub fn on_recall(&self) -> DualStrength {
        DualStrength {
            storage: f32::min(self.storage + 0.1, FSRS_MAX_STORAGE),
            retrieval: 1.0,
        }
    }

    pub fn on_lapse(&self) -> DualStrength {
        DualStrength {
            storage: f32::min(self.storage + 0.3, FSRS_MAX_STORAGE),
            retrieval: 1.0,
        }
    }

    pub fn decay(&self, elapsed_days: f32, stability: f32) -> DualStrength {
        if elapsed_days <= 0.0 || stability <= 0.0 {
            return *self;
        }
        let retrieval =
            f32::powf(1.0 + elapsed_days / (9.0 * stability), -1.0 / 0.5).clamp(0.0, 1.0);
        DualStrength {
            storage: self.storage,
            retrieval,
        }
    }
}
