//! Shared structured fields attached to a task specification. These types stay
//! outside the optional Fluency module so both core persistence and renderers
//! use one JSON contract.

use serde::{Deserialize, Serialize};

/// One implementation task linked to the acceptance criteria it advances.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ImplementationTask {
    /// Concrete work to perform.
    pub description: String,
    /// Zero-based acceptance-criterion indices advanced by this task.
    pub criteria_indices: Vec<usize>,
}

/// One candidate invariant suitable for property-based testing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TestProperty {
    /// Invariant or broad behavioral property to exercise.
    pub description: String,
    /// Zero-based acceptance-criterion index this property supports.
    pub criteria_index: usize,
}
