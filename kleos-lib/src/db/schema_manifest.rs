//! Declarative schema manifest.
//!
//! The single source of truth for the *additive* structural shape of the
//! database (tables, columns, indexes). `converge::converge_schema` ensures
//! every entry here exists, creating only what is missing. New structural
//! changes go here -- not into a numbered migration. Anything that removes,
//! narrows, rebuilds, or transforms data still goes through a numbered
//! migration in `migrations.rs` / `tenant_migrations.rs`.
//!
//! Note: the manifest does NOT need to restate the entire legacy schema. It is
//! additive-going-forward; the drift test only requires that whatever IS listed
//! here matches the migrated schema (converge is a no-op against an up-to-date
//! DB). Add a table/column/index here to introduce it without a migration.

/// One column in a manifest table.
pub struct ColumnDef {
    pub name: &'static str,
    /// SQLite type/affinity, e.g. "INTEGER", "TEXT".
    pub sql_type: &'static str,
    pub not_null: bool,
    /// SQL default expression (without the `DEFAULT` keyword), e.g. "0",
    /// "(datetime('now'))". Required when `not_null` and the column is added to
    /// an existing table (SQLite forbids NOT NULL without default on ADD COLUMN).
    pub default: Option<&'static str>,
    pub primary_key: bool,
}

/// One index on a manifest table.
pub struct IndexDef {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub unique: bool,
}

/// One table in the manifest.
pub struct TableDef {
    pub name: &'static str,
    pub columns: &'static [ColumnDef],
    pub indexes: &'static [IndexDef],
}

/// Global (system DB) structural manifest. Empty at introduction: the legacy
/// schema is already established by the numbered migration chain, and converge
/// is a no-op until entries are added here.
pub static SCHEMA_MANIFEST: &[TableDef] = &[];

/// Per-tenant (shard DB) structural manifest. Empty at introduction.
pub static TENANT_SCHEMA_MANIFEST: &[TableDef] = &[];
