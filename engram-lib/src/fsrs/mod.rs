pub mod decay;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// 21 default weights trained on millions of Anki reviews
pub const FSRS6_WEIGHTS: [f32; 21] = [
    0.212,   // w0:  Initial stability for Again
    1.2931,  // w1:  Initial stability for Hard
    2.3065,  // w2:  Initial stability for Good
    8.2956,  // w3:  Initial stability for Easy
    6.4133,  // w4:  Initial difficulty base
    0.8334,  // w5:  Initial difficulty grade modifier
    3.0194,  // w6:  Difficulty delta
    0.001,   // w7:  Difficulty mean reversion
    1.8722,  // w8:  Stability increase base
    0.1666,  // w9:  Stability saturation
    0.796,   // w10: Retrievability influence on stability
    1.4835,  // w11: Forget stability base
    0.0614,  // w12: Forget difficulty influence
    0.2629,  // w13: Forget stability influence
    1.6483,  // w14: Forget retrievability influence
    0.6014,  // w15: Hard penalty
    1.8729,  // w16: Easy bonus
    0.5425,  // w17: Same-day review base (FSRS-6)
    0.0912,  // w18: Same-day review grade modifier (FSRS-6)
    0.0658,  // w19: Same-day review stability influence (FSRS-6)
    0.1542,  // w20: Forgetting curve decay (FSRS-6, personalizable)
];

pub const FSRS_MIN_STABILITY: f32 = 0.1;
pub const FSRS_MAX_STABILITY: f32 = 36500.0;
pub const FSRS_MIN_DIFFICULTY: f32 = 1.0;
pub const FSRS_MAX_DIFFICULTY: f32 = 10.0;
pub const FSRS_MAX_STORAGE: f32 = 10.0;

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
        let retrieval = f32::powf(1.0 + elapsed_days / (9.0 * stability), -1.0 / 0.5).clamp(0.0, 1.0);
        DualStrength {
            storage: self.storage,
            retrieval,
        }
    }
}

// ---- Helper ----

fn round_3(x: f32) -> f32 {
    (x * 1000.0).round() / 1000.0
}

// ---- Core functions ----

pub fn forgetting_factor(w20: f32) -> f32 {
    f32::powf(0.9_f32, -1.0 / w20) - 1.0
}

pub fn retrievability(stability: f32, elapsed_days: f32) -> f32 {
    retrievability_with_w20(stability, elapsed_days, FSRS6_WEIGHTS[20])
}

pub fn retrievability_with_w20(stability: f32, elapsed_days: f32, w20: f32) -> f32 {
    if stability <= 0.0 {
        return 0.0;
    }
    if elapsed_days <= 0.0 {
        return 1.0;
    }
    let factor = forgetting_factor(w20);
    f32::powf(1.0 + factor * elapsed_days / stability, -w20).clamp(0.0, 1.0)
}

pub fn initial_difficulty(grade: Rating) -> f32 {
    let g = grade as u8 as f32;
    let d = FSRS6_WEIGHTS[4] - f32::exp(FSRS6_WEIGHTS[5] * (g - 1.0)) + 1.0;
    d.clamp(FSRS_MIN_DIFFICULTY, FSRS_MAX_DIFFICULTY)
}

pub fn initial_stability(grade: Rating) -> f32 {
    let idx = (grade as u8 - 1) as usize;
    f32::max(FSRS_MIN_STABILITY, FSRS6_WEIGHTS[idx])
}

pub fn next_difficulty(current_d: f32, grade: Rating) -> f32 {
    let d0 = initial_difficulty(Rating::Easy);
    let g = grade as u8 as f32;
    let delta = -FSRS6_WEIGHTS[6] * (g - 3.0);
    let mean_reversion_scale = (10.0 - current_d) / 9.0;
    let new_d = current_d + delta * mean_reversion_scale;
    let final_d = FSRS6_WEIGHTS[7] * d0 + (1.0 - FSRS6_WEIGHTS[7]) * new_d;
    final_d.clamp(FSRS_MIN_DIFFICULTY, FSRS_MAX_DIFFICULTY)
}

pub fn recall_stability(s: f32, d: f32, r: f32, grade: Rating) -> f32 {
    if grade == Rating::Again {
        return forget_stability(d, s, r);
    }
    let hard_penalty = if grade == Rating::Hard { FSRS6_WEIGHTS[15] } else { 1.0 };
    let easy_bonus = if grade == Rating::Easy { FSRS6_WEIGHTS[16] } else { 1.0 };
    let factor = f32::exp(FSRS6_WEIGHTS[8]) * (11.0 - d)
        * f32::powf(s, -FSRS6_WEIGHTS[9])
        * (f32::exp(FSRS6_WEIGHTS[10] * (1.0 - r)) - 1.0)
        * hard_penalty * easy_bonus + 1.0;
    (s * factor).clamp(FSRS_MIN_STABILITY, FSRS_MAX_STABILITY)
}

pub fn forget_stability(d: f32, s: f32, r: f32) -> f32 {
    let new_s = FSRS6_WEIGHTS[11] * f32::powf(d, -FSRS6_WEIGHTS[12])
        * (f32::powf(s + 1.0, FSRS6_WEIGHTS[13]) - 1.0)
        * f32::exp(FSRS6_WEIGHTS[14] * (1.0 - r));
    new_s.min(s).clamp(FSRS_MIN_STABILITY, FSRS_MAX_STABILITY)
}

pub fn same_day_stability(s: f32, grade: Rating) -> f32 {
    let g = grade as u8 as f32;
    let new_s = s * f32::exp(FSRS6_WEIGHTS[17] * (g - 3.0 + FSRS6_WEIGHTS[18]))
        * f32::powf(s, -FSRS6_WEIGHTS[19]);
    new_s.clamp(FSRS_MIN_STABILITY, FSRS_MAX_STABILITY)
}

pub fn next_interval(stability: f32, desired_r: f32) -> i32 {
    if stability <= 0.0 || desired_r >= 1.0 || desired_r <= 0.0 {
        return 0;
    }
    let factor = forgetting_factor(FSRS6_WEIGHTS[20]);
    let interval =
        stability / factor * (f32::powf(desired_r, -1.0 / FSRS6_WEIGHTS[20]) - 1.0);
    i32::max(0, interval.round() as i32)
}

// ---- Review processor ----

fn now_str() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub fn process_review(state: Option<&FsrsState>, grade: Rating, elapsed_days: f32) -> FsrsState {
    let now = now_str();

    if state.is_none() || state.unwrap().learning_state == LearningState::New {
        let d = initial_difficulty(grade);
        let s = initial_stability(grade);
        let learning = if (grade as u8) <= (Rating::Hard as u8) {
            LearningState::Learning
        } else {
            LearningState::Review
        };
        return FsrsState {
            stability: s,
            difficulty: d,
            storage_strength: 1.0,
            retrieval_strength: 1.0,
            learning_state: learning,
            reps: 1,
            lapses: if grade == Rating::Again { 1 } else { 0 },
            last_review_at: now,
        };
    }

    let state = state.unwrap();
    let r = retrievability(state.stability, elapsed_days);
    let is_same_day = elapsed_days < 1.0;
    let mut ds = DualStrength {
        storage: state.storage_strength,
        retrieval: state.retrieval_strength,
    };

    let new_s: f32;
    let new_d: f32;
    let new_learning_state: LearningState;
    let new_lapses: i32;

    if is_same_day {
        new_s = same_day_stability(state.stability, grade);
        new_d = next_difficulty(state.difficulty, grade);
        new_learning_state = state.learning_state;
        new_lapses = state.lapses;
        ds = ds.on_recall();
    } else if grade == Rating::Again {
        new_s = forget_stability(state.difficulty, state.stability, r);
        new_d = next_difficulty(state.difficulty, Rating::Again);
        new_learning_state = LearningState::Relearning;
        new_lapses = state.lapses + 1;
        ds = ds.on_lapse();
    } else {
        new_s = recall_stability(state.stability, state.difficulty, r, grade);
        new_d = next_difficulty(state.difficulty, grade);
        new_learning_state = LearningState::Review;
        new_lapses = state.lapses;
        ds = ds.on_recall();
    }

    FsrsState {
        stability: round_3(new_s),
        difficulty: round_3(new_d),
        storage_strength: round_3(ds.storage),
        retrieval_strength: round_3(ds.retrieval),
        learning_state: new_learning_state,
        reps: state.reps + 1,
        lapses: new_lapses,
        last_review_at: now,
    }
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weights_count() {
        assert_eq!(FSRS6_WEIGHTS.len(), 21);
    }

    #[test]
    fn test_retrievability_basic() {
        // elapsed=0 -> 1.0
        let r = retrievability(1.0, 0.0);
        assert!((r - 1.0).abs() < 0.001, "expected 1.0, got {}", r);

        // elapsed=1, stability=1 -> near 0.9 (by design of the forgetting curve)
        let r2 = retrievability(1.0, 1.0);
        assert!((r2 - 0.9).abs() < 0.01, "expected ~0.9, got {}", r2);

        // stability <= 0 -> 0.0
        let r3 = retrievability(0.0, 5.0);
        assert!((r3 - 0.0).abs() < 0.001, "expected 0.0, got {}", r3);
    }

    #[test]
    fn test_initial_stability() {
        assert!((initial_stability(Rating::Again) - 0.212).abs() < 0.001);
        assert!((initial_stability(Rating::Hard) - 1.2931).abs() < 0.001);
        assert!((initial_stability(Rating::Good) - 2.3065).abs() < 0.001);
        assert!((initial_stability(Rating::Easy) - 8.2956).abs() < 0.001);
    }

    #[test]
    fn test_initial_difficulty() {
        // grade affects difficulty via w4 - e^(w5*(g-1)) + 1, clamped 1-10
        for grade in [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy] {
            let d = initial_difficulty(grade);
            assert!(
                d >= FSRS_MIN_DIFFICULTY,
                "difficulty {} below min for {:?}",
                d,
                grade
            );
            assert!(
                d <= FSRS_MAX_DIFFICULTY,
                "difficulty {} above max for {:?}",
                d,
                grade
            );
        }
        // Again (g=1): w4 - e^(w5*0) + 1 = 6.4133 - 1 + 1 = 6.4133
        let d_again = initial_difficulty(Rating::Again);
        assert!((d_again - 6.4133).abs() < 0.001, "got {}", d_again);
    }

    #[test]
    fn test_process_review_new() {
        let result = process_review(None, Rating::Good, 0.0);
        assert!(
            (result.stability - 2.3065).abs() < 0.001,
            "expected stability 2.3065, got {}",
            result.stability
        );
        assert_eq!(result.learning_state, LearningState::Review);
        assert_eq!(result.reps, 1);
        assert_eq!(result.lapses, 0);
    }

    #[test]
    fn test_process_review_again() {
        let result = process_review(None, Rating::Again, 0.0);
        assert!(
            (result.stability - 0.212).abs() < 0.001,
            "expected stability 0.212, got {}",
            result.stability
        );
        assert_eq!(result.learning_state, LearningState::Learning);
        assert_eq!(result.lapses, 1);
    }

    #[test]
    fn test_forget_stability_less_than_current() {
        let s = 5.0_f32;
        let d = 6.0_f32;
        let r = 0.7_f32;
        let fs = forget_stability(d, s, r);
        assert!(fs <= s, "forget stability {} should be <= current {}", fs, s);
        assert!(
            fs >= FSRS_MIN_STABILITY,
            "forget stability {} should be >= min",
            fs
        );
    }

    #[test]
    fn test_decay_score_static() {
        let score = decay::calculate_decay_score(
            0.8,
            "2025-01-01 00:00:00",
            0,
            None,
            true,
            1,
            None,
        );
        assert!(
            (score - 0.8).abs() < 0.001,
            "static should return importance, got {}",
            score
        );
    }

    #[test]
    fn test_dual_strength() {
        let ds = DualStrength {
            storage: 3.0,
            retrieval: 0.5,
        };
        let recalled = ds.on_recall();
        assert!((recalled.retrieval - 1.0).abs() < 0.001);
        assert!(
            (recalled.storage - 3.1).abs() < 0.001,
            "storage should increase by 0.1, got {}",
            recalled.storage
        );
    }

    #[test]
    fn test_same_day_review() {
        let s = 2.3065_f32;
        let new_s = same_day_stability(s, Rating::Good);
        // new_s = s * exp(w17 * (3-3+w18)) * s^(-w19)
        let expected = s
            * f32::exp(FSRS6_WEIGHTS[17] * (0.0 + FSRS6_WEIGHTS[18]))
            * f32::powf(s, -FSRS6_WEIGHTS[19]);
        let expected_clamped =
            f32::max(FSRS_MIN_STABILITY, f32::min(FSRS_MAX_STABILITY, expected));
        assert!(
            (new_s - expected_clamped).abs() < 0.001,
            "got {}, expected {}",
            new_s,
            expected_clamped
        );
    }
}
