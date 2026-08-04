//! Typed read model over the forge tables. `get_spec` returns loose JSON for
//! agents; the renderers need typed rows with the full prose, so this module
//! provides that view without disturbing the existing tool.

use crate::db::Database;
use crate::spec_types::{ImplementationTask, TestProperty};
use crate::tools::ToolError;
use serde::de::DeserializeOwned;

/// One design alternative with the full prose the renderer needs. `get_spec`
/// returns only name/score/chosen; rendering a rejected alternative needs its
/// description and cons as well.
pub struct ApproachRow {
    /// Short name of the approach.
    pub name: String,
    /// Prose description of what the approach does.
    pub description: String,
    /// Arguments in favour.
    pub pros: Vec<String>,
    /// Arguments against, used as the rejection reason for unchosen approaches.
    pub cons: Vec<String>,
    /// Optional numeric score, higher is better.
    pub score: Option<f64>,
    /// Whether this approach was the one taken.
    pub chosen: bool,
}

/// A discovery captured mid-work, rendered as a hard-won condition.
pub struct LearnRow {
    /// What was discovered.
    pub discovery: String,
    /// Optional surrounding circumstance.
    pub context: Option<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
}

/// One recorded verification run.
pub struct VerificationRow {
    /// The command that was executed.
    pub command: String,
    /// Whether it exited successfully.
    pub success: bool,
    /// Which acceptance criterion it was aimed at, when the caller supplied one.
    pub criteria_index: Option<i64>,
}

/// Everything needed to render a spec's documentation.
pub struct SpecRecord {
    /// The spec's identifier.
    pub id: String,
    /// One-line intent.
    pub task_description: String,
    /// feature, bugfix, refactor, enhancement, test, or docs.
    pub task_type: String,
    /// Acceptance criteria, parsed from the stored JSON array.
    pub acceptance_criteria: Vec<String>,
    /// Edge cases, parsed from the stored JSON array.
    pub edge_cases: Vec<String>,
    /// Optional interface contract prose.
    pub interface_contract: Option<String>,
    /// Source files the spec expects to change.
    pub files_to_touch: Vec<String>,
    /// Optional dependency prose.
    pub dependencies: Option<String>,
    /// Existing behavior that must remain intact.
    pub unchanged_behaviors: Vec<String>,
    /// Concrete work items linked to criteria.
    pub implementation_tasks: Vec<ImplementationTask>,
    /// Explicit property-based test candidates linked to criteria.
    pub test_properties: Vec<TestProperty>,
    /// Every recorded design alternative, chosen and rejected alike.
    pub approaches: Vec<ApproachRow>,
    /// Discoveries captured during the work.
    pub learns: Vec<LearnRow>,
    /// Verification runs recorded against this spec.
    pub verifications: Vec<VerificationRow>,
}

/// Parse a JSON column into its typed value. A malformed or absent value
/// degrades to that type's default: a partially broken record is still worth
/// rendering, and failing the whole load would lose the parts that are intact.
///
/// This tolerance is deliberately scoped to a single FIELD. A malformed ROW is
/// not tolerated -- see `load_spec_record`, which propagates row-level failures
/// as `DatabaseError` rather than dropping the row. Dropping a row would remove
/// a whole decision from the emitted documentation without saying so, and a
/// record that silently omits a decision is worse than no record at all.
fn parse_json<T>(raw: Option<String>) -> T
where
    T: DeserializeOwned + Default,
{
    raw.and_then(|value| serde_json::from_str::<T>(&value).ok())
        .unwrap_or_default()
}

/// Load one spec and every row linked to it, as typed values ready for rendering.
pub fn load_spec_record(db: &Database, spec_id: &str) -> Result<SpecRecord, ToolError> {
    let (
        task_description,
        task_type,
        criteria_raw,
        edge_raw,
        interface_contract,
        files_raw,
        dependencies,
        unchanged_raw,
        tasks_raw,
        properties_raw,
    ) = db
        .conn()
        .query_row(
            "SELECT task_description, task_type, acceptance_criteria, edge_cases,
                    interface_contract, files_to_touch, dependencies,
                    unchanged_behaviors, implementation_tasks, test_properties
             FROM specs WHERE id = ?1",
            rusqlite::params![spec_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .map_err(|e| ToolError::DatabaseError(format!("Spec not found: {}", e)))?;

    let mut appr_stmt = db
        .conn()
        .prepare(
            "SELECT name, description, pros, cons, score, chosen
             FROM approaches WHERE spec_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;
    let approaches: Vec<ApproachRow> = appr_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(ApproachRow {
                name: row.get(0)?,
                description: row.get(1)?,
                pros: parse_json(row.get(2)?),
                cons: parse_json(row.get(3)?),
                score: row.get(4)?,
                chosen: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let mut learn_stmt = db
        .conn()
        .prepare(
            "SELECT discovery, context, tags
             FROM session_learns WHERE spec_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;
    let learns: Vec<LearnRow> = learn_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(LearnRow {
                discovery: row.get(0)?,
                context: row.get(1)?,
                tags: parse_json(row.get(2)?),
            })
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let mut ver_stmt = db
        .conn()
        .prepare(
            "SELECT command, success, criteria_index
             FROM verifications WHERE spec_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;
    let verifications: Vec<VerificationRow> = ver_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(VerificationRow {
                command: row.get(0)?,
                success: row.get::<_, i64>(1)? != 0,
                criteria_index: row.get(2)?,
            })
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    Ok(SpecRecord {
        id: spec_id.to_string(),
        task_description,
        task_type,
        acceptance_criteria: parse_json(criteria_raw),
        edge_cases: parse_json(edge_raw),
        interface_contract,
        files_to_touch: parse_json(files_raw),
        dependencies,
        unchanged_behaviors: parse_json(unchanged_raw),
        implementation_tasks: parse_json(tasks_raw),
        test_properties: parse_json(properties_raw),
        approaches,
        learns,
        verifications,
    })
}

#[cfg(test)]
/// Tests for loading a spec's full record from the database.
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::tempdir;

    /// Insert a spec with one chosen approach, one learning, and one passing
    /// verification, then confirm every part round-trips into the typed record.
    #[test]
    fn loads_a_full_spec_record() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        db.conn()
            .execute_batch(
                r#"
                INSERT INTO specs (id, created_at, task_description, task_type,
                                   acceptance_criteria, edge_cases, files_to_touch,
                                   dependencies, unchanged_behaviors,
                                   implementation_tasks, test_properties, status)
                VALUES ('spec_1', 1, 'Add a thing', 'feature',
                        '["criterion one","criterion two"]', '["edge one"]',
                        '["src/thing.rs"]', 'serde', '["old thing still works"]',
                        '[{"description":"build it","criteria_indices":[0,1]}]',
                        '[{"description":"all values round-trip","criteria_index":1}]',
                        'active');

                INSERT INTO approaches (id, spec_id, created_at, name, description,
                                        pros, cons, score, chosen)
                VALUES ('appr_1', 'spec_1', 1, 'Direct', 'Do it directly',
                        '["simple"]', '["rigid"]', 8.0, 1),
                       ('appr_2', 'spec_1', 1, 'Indirect', 'Add a layer',
                        '["flexible"]', '["slower"]', 5.0, 0);

                INSERT INTO session_learns (id, created_at, discovery, context, tags, spec_id)
                VALUES ('learn_1', 1, 'The cache lies on cold start',
                        'found while tracing', '["cache"]', 'spec_1');

                INSERT INTO verifications (id, spec_id, created_at, command,
                                           exit_code, success, criteria_index)
                VALUES ('ver_1', 'spec_1', 1, 'cargo test', 0, 1, 0);
                "#,
            )
            .unwrap();

        let record = load_spec_record(&db, "spec_1").unwrap();
        assert_eq!(record.task_description, "Add a thing");
        assert_eq!(
            record.acceptance_criteria,
            vec!["criterion one", "criterion two"]
        );
        assert_eq!(record.edge_cases, vec!["edge one"]);
        assert_eq!(record.files_to_touch, vec!["src/thing.rs"]);
        assert_eq!(record.dependencies.as_deref(), Some("serde"));
        assert_eq!(record.unchanged_behaviors, vec!["old thing still works"]);
        assert_eq!(record.implementation_tasks[0].criteria_indices, vec![0, 1]);
        assert_eq!(record.test_properties[0].criteria_index, 1);
        assert_eq!(record.approaches.len(), 2);

        let chosen: Vec<_> = record.approaches.iter().filter(|a| a.chosen).collect();
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].name, "Direct");
        assert_eq!(chosen[0].pros, vec!["simple"]);

        assert_eq!(record.learns.len(), 1);
        assert_eq!(record.learns[0].discovery, "The cache lies on cold start");
        assert_eq!(record.verifications.len(), 1);
        assert!(record.verifications[0].success);
    }

    /// A missing spec is a database error, not a panic or an empty record.
    #[test]
    fn missing_spec_is_an_error() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        assert!(load_spec_record(&db, "nope").is_err());
    }

    /// A malformed JSON array column degrades to an empty list rather than failing
    /// the whole load, because a broken record is still worth rendering.
    #[test]
    fn malformed_json_columns_degrade_to_empty() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO specs (id, created_at, task_description, task_type,
                                    acceptance_criteria, status)
                 VALUES ('spec_2', 1, 'Broken', 'feature', 'not json', 'active');",
            )
            .unwrap();
        let record = load_spec_record(&db, "spec_2").unwrap();
        assert!(record.acceptance_criteria.is_empty());
    }

    /// A row that cannot be deserialized fails the whole load rather than being
    /// dropped. Silently omitting an approach would silently omit a decision
    /// from the emitted documentation, which is the one failure this tool must
    /// never have. A malformed JSON column degrades (above); a malformed ROW
    /// does not.
    #[test]
    fn unreadable_row_fails_the_load() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        db.conn()
            .execute_batch(
                r#"
                INSERT INTO specs (id, created_at, task_description, task_type,
                                   acceptance_criteria, status)
                VALUES ('spec_3', 1, 'Add a thing', 'feature', '[]', 'active');

                INSERT INTO approaches (id, spec_id, created_at, name, description,
                                        pros, cons, score, chosen)
                VALUES ('appr_bad', 'spec_3', 1, 'Direct', 'Do it directly',
                        '[]', '[]', 8.0, NULL);
                "#,
            )
            .unwrap();
        assert!(load_spec_record(&db, "spec_3").is_err());
    }
}
