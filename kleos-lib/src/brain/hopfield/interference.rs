pub const SUPPRESSION_FACTOR: f32 = 0.1;
pub const WINNER_BOOST: f32 = 1.2;
pub const RECENCY_HALF_LIFE_DAYS: f32 = 30.0;

pub struct PatternState {
    pub activation: f32,
    pub decay_factor: f32,
    pub importance: i32,
    pub age_days: f32,
}

/// Recency score: 2^(-age_days / 30)
pub fn recency_score(age_days: f32) -> f32 {
    let exponent = -age_days / RECENCY_HALF_LIFE_DAYS;
    2.0_f32.powf(exponent)
}

/// Compute effective strength for interference resolution.
pub fn effective_strength(p: &PatternState) -> f32 {
    let importance_factor = (p.importance as f32 / 10.0).clamp(0.1, 2.0);
    let recency = recency_score(p.age_days);
    p.activation * p.decay_factor * importance_factor * recency
}

/// Resolve interference between two memories at contradiction edges.
/// Returns (a_new_activation, b_new_activation, a_won).
pub fn resolve_interference(a: &PatternState, b: &PatternState) -> (f32, f32, bool) {
    let a_eff = effective_strength(a);
    let b_eff = effective_strength(b);

    if a_eff >= b_eff {
        let a_new = (a.activation * WINNER_BOOST).min(1.0);
        let b_new = b.activation * SUPPRESSION_FACTOR;
        (a_new, b_new, true)
    } else {
        let a_new = a.activation * SUPPRESSION_FACTOR;
        let b_new = (b.activation * WINNER_BOOST).min(1.0);
        (a_new, b_new, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(activation: f32, decay: f32, importance: i32, age_days: f32) -> PatternState {
        PatternState {
            activation,
            decay_factor: decay,
            importance,
            age_days,
        }
    }

    #[test]
    fn recency_recent_beats_old() {
        let recent = recency_score(1.0);
        let old = recency_score(90.0);
        assert!(recent > old, "recent={} old={}", recent, old);
        let now = recency_score(0.0);
        assert!((now - 1.0).abs() < 1e-5);
        let half = recency_score(30.0);
        assert!((half - 0.5).abs() < 1e-5);
    }

    #[test]
    fn resolve_newer_wins() {
        let a = state(0.8, 1.0, 5, 1.0);
        let b = state(0.8, 1.0, 5, 60.0);
        let (a_new, b_new, a_won) = resolve_interference(&a, &b);
        assert!(a_won, "newer memory A should win");
        assert!(a_new > 0.8, "winner gets boost");
        assert!(b_new < 0.8, "loser gets suppressed");
    }

    #[test]
    fn resolve_importance_wins() {
        let a = state(0.8, 1.0, 3, 10.0);
        let b = state(0.7, 1.0, 9, 10.0);
        let (_, _, a_won) = resolve_interference(&a, &b);
        assert!(!a_won, "higher importance B should win");
    }

    #[test]
    fn resolve_tie_favors_a() {
        let a = state(0.5, 1.0, 5, 10.0);
        let b = state(0.5, 1.0, 5, 10.0);
        let (a_new, b_new, a_won) = resolve_interference(&a, &b);
        assert!(a_won, "tie should favor A (>=)");
        assert!(a_new > 0.5);
        assert!(b_new < 0.5);
    }
}
