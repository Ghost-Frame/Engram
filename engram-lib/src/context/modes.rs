// ============================================================================
// CONTEXT DOMAIN -- Mode preset resolution and strategy helpers
// ============================================================================

use super::types::{
    ContextMode, ContextOptions, ContextStrategy, LayerFlags,
    DEFAULT_SEMANTIC_CEILING_BALANCED, DEFAULT_SEMANTIC_CEILING_BREADTH,
    DEFAULT_SEMANTIC_CEILING_PRECISION, STATIC_BUDGET_BALANCED, STATIC_BUDGET_PRECISION,
};

/// Applies mode preset defaults to the options. Presets only set values
/// that have not already been provided.
pub fn apply_context_mode(opts: &mut ContextOptions) {
    let mode = match opts.mode {
        Some(m) => m,
        None => return,
    };
    match mode {
        ContextMode::Fast => {
            if opts.depth.is_none() {
                opts.depth = Some(1);
            }
            if opts.max_tokens.is_none() {
                opts.max_tokens = Some(2000);
            }
        }
        ContextMode::Balanced => {
            if opts.depth.is_none() {
                opts.depth = Some(2);
            }
            if opts.max_tokens.is_none() {
                opts.max_tokens = Some(6000);
            }
        }
        ContextMode::Deep => {
            if opts.depth.is_none() {
                opts.depth = Some(3);
            }
            if opts.max_tokens.is_none() {
                opts.max_tokens = Some(16000);
            }
            if opts.include_inference.is_none() {
                opts.include_inference = Some(true);
            }
        }
        ContextMode::Decision => {
            if opts.depth.is_none() {
                opts.depth = Some(3);
            }
            if opts.max_tokens.is_none() {
                opts.max_tokens = Some(10000);
            }
            opts.include_linked = Some(true);
            opts.include_structured_facts = Some(true);
        }
    }
}

/// Resolve the 10 layer flags from the context options and depth level.
pub fn resolve_layer_flags(opts: &ContextOptions, depth: u8) -> LayerFlags {
    LayerFlags {
        include_static: opts.include_static.unwrap_or(true) && depth >= 1,
        include_recent: opts.include_recent.unwrap_or(true) && depth >= 2,
        include_episodes: opts.include_episodes.unwrap_or(true) && depth >= 3,
        include_linked: opts.include_linked.unwrap_or(true) && depth >= 3,
        include_inference: opts.include_inference.unwrap_or(false) && depth >= 3,
        include_current_state: opts.include_current_state.unwrap_or(true) && depth >= 1,
        include_preferences: opts.include_preferences.unwrap_or(true) && depth >= 2,
        include_structured_facts: opts.include_structured_facts.unwrap_or(true) && depth >= 2,
        include_working_memory: opts.include_working_memory.unwrap_or(true) && depth >= 3,
        include_personality: depth >= 2,
    }
}

/// Resolve the semantic ceiling fraction for the given strategy.
pub fn resolve_semantic_ceiling(strategy: &ContextStrategy, override_val: Option<f64>) -> f64 {
    if let Some(v) = override_val {
        return v;
    }
    match strategy {
        ContextStrategy::Precision => DEFAULT_SEMANTIC_CEILING_PRECISION,
        ContextStrategy::Breadth => DEFAULT_SEMANTIC_CEILING_BREADTH,
        ContextStrategy::Balanced => DEFAULT_SEMANTIC_CEILING_BALANCED,
    }
}

/// Resolve the semantic result limit for the given strategy.
pub fn resolve_semantic_limit(strategy: &ContextStrategy, override_val: Option<usize>) -> usize {
    if let Some(v) = override_val {
        return v;
    }
    match strategy {
        ContextStrategy::Precision => 30,
        ContextStrategy::Breadth => 80,
        ContextStrategy::Balanced => 50,
    }
}

/// Resolve the static facts budget fraction for the given strategy.
pub fn resolve_static_budget_fraction(strategy: &ContextStrategy) -> f64 {
    match strategy {
        ContextStrategy::Precision => STATIC_BUDGET_PRECISION,
        _ => STATIC_BUDGET_BALANCED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_mode_fast() {
        let mut opts = ContextOptions {
            query: "test".into(),
            mode: Some(ContextMode::Fast),
            ..Default::default()
        };
        apply_context_mode(&mut opts);
        assert_eq!(opts.depth, Some(1));
        assert_eq!(opts.max_tokens, Some(2000));
    }

    #[test]
    fn test_apply_mode_deep() {
        let mut opts = ContextOptions {
            query: "test".into(),
            mode: Some(ContextMode::Deep),
            ..Default::default()
        };
        apply_context_mode(&mut opts);
        assert_eq!(opts.depth, Some(3));
        assert_eq!(opts.max_tokens, Some(16000));
        assert_eq!(opts.include_inference, Some(true));
    }

    #[test]
    fn test_apply_mode_decision() {
        let mut opts = ContextOptions {
            query: "test".into(),
            mode: Some(ContextMode::Decision),
            ..Default::default()
        };
        apply_context_mode(&mut opts);
        assert_eq!(opts.depth, Some(3));
        assert_eq!(opts.max_tokens, Some(10000));
        assert_eq!(opts.include_linked, Some(true));
        assert_eq!(opts.include_structured_facts, Some(true));
    }

    #[test]
    fn test_apply_mode_does_not_override_explicit() {
        let mut opts = ContextOptions {
            query: "test".into(),
            mode: Some(ContextMode::Fast),
            depth: Some(3),
            max_tokens: Some(50000),
            ..Default::default()
        };
        apply_context_mode(&mut opts);
        assert_eq!(opts.depth, Some(3));
        assert_eq!(opts.max_tokens, Some(50000));
    }

    #[test]
    fn test_layer_flags_depth_1() {
        let opts = ContextOptions::default();
        let flags = resolve_layer_flags(&opts, 1);
        assert!(flags.include_static);
        assert!(!flags.include_recent);
        assert!(!flags.include_episodes);
        assert!(flags.include_current_state);
        assert!(!flags.include_personality);
    }

    #[test]
    fn test_layer_flags_depth_3() {
        let opts = ContextOptions::default();
        let flags = resolve_layer_flags(&opts, 3);
        assert!(flags.include_static);
        assert!(flags.include_recent);
        assert!(flags.include_episodes);
        assert!(flags.include_linked);
        assert!(flags.include_working_memory);
        assert!(flags.include_personality);
        assert!(!flags.include_inference);
    }

    #[test]
    fn test_semantic_ceiling() {
        assert_eq!(resolve_semantic_ceiling(&ContextStrategy::Balanced, None), DEFAULT_SEMANTIC_CEILING_BALANCED);
        assert_eq!(resolve_semantic_ceiling(&ContextStrategy::Precision, None), DEFAULT_SEMANTIC_CEILING_PRECISION);
        assert_eq!(resolve_semantic_ceiling(&ContextStrategy::Balanced, Some(0.5)), 0.5);
    }

    #[test]
    fn test_semantic_limit() {
        assert_eq!(resolve_semantic_limit(&ContextStrategy::Balanced, None), 50);
        assert_eq!(resolve_semantic_limit(&ContextStrategy::Precision, None), 30);
        assert_eq!(resolve_semantic_limit(&ContextStrategy::Breadth, None), 80);
    }

    #[test]
    fn test_static_budget_fraction() {
        assert_eq!(resolve_static_budget_fraction(&ContextStrategy::Balanced), STATIC_BUDGET_BALANCED);
        assert_eq!(resolve_static_budget_fraction(&ContextStrategy::Precision), STATIC_BUDGET_PRECISION);
    }
}
