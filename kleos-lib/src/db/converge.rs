//! Boot-time schema convergence: additively create any manifest table/column/
//! index that is missing. Never drops, narrows, or alters in place. Atomic via
//! a single SAVEPOINT.

use crate::db::schema_manifest::{ColumnDef, IndexDef, TableDef};
use crate::EngError;
use crate::Result;
use tracing::info;

/// Ensure every table/column/index in `manifest` exists, creating only what is
/// missing. Additive only. Returns a human-readable list of the actions taken
/// (empty when the DB already matches). Atomic: all changes commit together or
/// roll back together.
pub fn converge_schema(conn: &rusqlite::Connection, manifest: &[TableDef]) -> Result<Vec<String>> {
    let mut actions: Vec<String> = Vec::new();
    conn.execute_batch("SAVEPOINT converge_schema")
        .map_err(|e| EngError::DatabaseMessage(format!("converge savepoint: {e}")))?;

    let run = (|| -> Result<()> {
        for t in manifest {
            if !table_exists(conn, t.name)? {
                conn.execute_batch(&create_table_sql(t)).map_err(|e| {
                    EngError::DatabaseMessage(format!("converge create {}: {e}", t.name))
                })?;
                actions.push(format!("create table {}", t.name));
                for idx in t.indexes {
                    conn.execute_batch(&create_index_sql(t.name, idx))
                        .map_err(|e| {
                            EngError::DatabaseMessage(format!("converge index {}: {e}", idx.name))
                        })?;
                    actions.push(format!("create index {}", idx.name));
                }
                continue;
            }
            let existing = table_columns(conn, t.name)?;
            for c in t.columns {
                if existing.contains(c.name) {
                    continue;
                }
                guard_add_column(t.name, c)?;
                conn.execute_batch(&format!(
                    "ALTER TABLE {} ADD COLUMN {}",
                    t.name,
                    column_sql(c)
                ))
                .map_err(|e| {
                    EngError::DatabaseMessage(format!("converge add {}.{}: {e}", t.name, c.name))
                })?;
                actions.push(format!("add column {}.{}", t.name, c.name));
            }
            for idx in t.indexes {
                if !index_exists(conn, idx.name)? {
                    conn.execute_batch(&create_index_sql(t.name, idx))
                        .map_err(|e| {
                            EngError::DatabaseMessage(format!("converge index {}: {e}", idx.name))
                        })?;
                    actions.push(format!("create index {}", idx.name));
                }
            }
        }
        Ok(())
    })();

    match run {
        Ok(()) => {
            conn.execute_batch("RELEASE converge_schema")
                .map_err(|e| EngError::DatabaseMessage(format!("converge release: {e}")))?;
            if !actions.is_empty() {
                info!(
                    "schema converge applied {} change(s): {:?}",
                    actions.len(),
                    actions
                );
            }
            Ok(actions)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO converge_schema; RELEASE converge_schema");
            Err(e)
        }
    }
}

/// SQLite forbids `ADD COLUMN` for NOT NULL-without-default or PRIMARY KEY
/// columns. Such a change belongs in an explicit migration, not the manifest.
fn guard_add_column(table: &str, c: &ColumnDef) -> Result<()> {
    if c.primary_key {
        return Err(EngError::DatabaseMessage(format!(
            "converge: column {table}.{} is PRIMARY KEY; SQLite cannot ADD COLUMN a PK -- write an explicit migration",
            c.name
        )));
    }
    if c.not_null && c.default.is_none() {
        return Err(EngError::DatabaseMessage(format!(
            "converge: column {table}.{} is NOT NULL without a default; SQLite cannot ADD COLUMN it -- write an explicit migration",
            c.name
        )));
    }
    Ok(())
}

/// Whether a table of this name exists in the connected database.
fn table_exists(conn: &rusqlite::Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether an index of this name exists. Index names are global in SQLite,
/// so the owning table is not part of the lookup.
fn index_exists(conn: &rusqlite::Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The set of column names currently on a table, used to decide which manifest
/// columns are still missing.
fn table_columns(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut set = std::collections::HashSet::new();
    for c in rows {
        set.insert(c?);
    }
    Ok(set)
}

/// Render one column definition as it appears inside CREATE TABLE or after
/// ALTER TABLE ADD COLUMN.
fn column_sql(c: &ColumnDef) -> String {
    let mut s = format!("{} {}", c.name, c.sql_type);
    if c.primary_key {
        s.push_str(" PRIMARY KEY");
    }
    if c.not_null {
        s.push_str(" NOT NULL");
    }
    if let Some(d) = c.default {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    s
}

/// Build the `CREATE TABLE` for a manifest entry. Table-level constraints are
/// appended after the columns so composite primary keys, foreign keys, and
/// CHECK clauses survive into the created table.
fn create_table_sql(t: &TableDef) -> String {
    let mut parts = t.columns.iter().map(column_sql).collect::<Vec<_>>();
    parts.extend(t.constraints.iter().map(|c| (*c).to_string()));
    let body = parts.join(", ");
    format!("CREATE TABLE IF NOT EXISTS {} ({body})", t.name)
}

/// Render the CREATE INDEX for one manifest index.
fn create_index_sql(table: &str, idx: &IndexDef) -> String {
    let unique = if idx.unique { "UNIQUE " } else { "" };
    format!(
        "CREATE {unique}INDEX IF NOT EXISTS {} ON {table} ({})",
        idx.name,
        idx.columns.join(", ")
    )
}

/// Converge behaviour: what it creates, what it adds, what it refuses, and
/// that repeating it is a no-op.
#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway in-memory database for a single converge test.
    fn mem() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().expect("open memory db")
    }

    static T_CREATE: &[TableDef] = &[TableDef {
        name: "widget",
        columns: &[
            ColumnDef {
                name: "id",
                sql_type: "INTEGER",
                not_null: false,
                default: None,
                primary_key: true,
            },
            ColumnDef {
                name: "label",
                sql_type: "TEXT",
                not_null: true,
                default: Some("''"),
                primary_key: false,
            },
        ],
        indexes: &[IndexDef {
            name: "idx_widget_label",
            columns: &["label"],
            unique: false,
        }],
        constraints: &[],
    }];

    /// A manifest table absent from the database is created with every column
    /// and index it declares, and converging again changes nothing.
    #[test]
    fn creates_missing_table_with_columns_and_index() -> Result<()> {
        let conn = mem();
        let actions = converge_schema(&conn, T_CREATE)?;
        assert!(actions.iter().any(|a| a.contains("create table widget")));
        let cols: Vec<String> = {
            let mut s = conn.prepare("SELECT name FROM pragma_table_info('widget')")?;
            let r = s.query_map([], |row| row.get::<_, String>(0))?;
            r.collect::<std::result::Result<Vec<_>, _>>().unwrap()
        };
        assert_eq!(cols, vec!["id".to_string(), "label".to_string()]);
        let idx: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_widget_label'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(idx, 1);
        let again = converge_schema(&conn, T_CREATE)?;
        assert!(again.is_empty(), "converge must be idempotent");
        Ok(())
    }

    /// A column present in the manifest but missing from an existing table is
    /// added in place, without touching the rest of the table.
    #[test]
    fn adds_missing_column_to_existing_table() -> Result<()> {
        let conn = mem();
        conn.execute_batch("CREATE TABLE gadget (id INTEGER PRIMARY KEY)")?;
        static T: &[TableDef] = &[TableDef {
            name: "gadget",
            columns: &[
                ColumnDef {
                    name: "id",
                    sql_type: "INTEGER",
                    not_null: false,
                    default: None,
                    primary_key: true,
                },
                ColumnDef {
                    name: "note",
                    sql_type: "TEXT",
                    not_null: true,
                    default: Some("''"),
                    primary_key: false,
                },
            ],
            indexes: &[],
            constraints: &[],
        }];
        let actions = converge_schema(&conn, T)?;
        assert!(actions.iter().any(|a| a == "add column gadget.note"));
        let cols = table_columns(&conn, "gadget")?;
        assert!(cols.contains("note"));
        Ok(())
    }

    /// SQLite cannot ADD COLUMN a NOT NULL column with no default, so converge
    /// must refuse rather than emit SQL the database will reject, and must
    /// leave the table untouched when it does.
    #[test]
    fn refuses_not_null_without_default_on_existing_table() {
        let conn = mem();
        conn.execute_batch("CREATE TABLE thing (id INTEGER PRIMARY KEY)")
            .unwrap();
        static T: &[TableDef] = &[TableDef {
            name: "thing",
            columns: &[
                ColumnDef {
                    name: "id",
                    sql_type: "INTEGER",
                    not_null: false,
                    default: None,
                    primary_key: true,
                },
                ColumnDef {
                    name: "required",
                    sql_type: "TEXT",
                    not_null: true,
                    default: None,
                    primary_key: false,
                },
            ],
            indexes: &[],
            constraints: &[],
        }];
        let err = converge_schema(&conn, T).expect_err("must refuse unsafe ADD COLUMN");
        let msg = format!("{err}");
        assert!(msg.contains("NOT NULL without a default"), "got: {msg}");
        let cols = table_columns(&conn, "thing").unwrap();
        assert!(!cols.contains("required"));
    }

    /// A manifest entry carrying a composite primary key, a cascading foreign
    /// key, and a CHECK constraint must produce a table that actually enforces
    /// all three. Without table-level constraints the manifest could only
    /// describe a laxer table than the one it is meant to replace, which would
    /// silently drop referential integrity when a numbered migration is
    /// retired in favour of a manifest entry.
    #[test]
    fn creates_table_with_composite_pk_foreign_key_and_check() -> Result<()> {
        let conn = mem();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; CREATE TABLE space (id INTEGER PRIMARY KEY)",
        )?;
        static T: &[TableDef] = &[TableDef {
            name: "grant_acl",
            columns: &[
                ColumnDef {
                    name: "space_id",
                    sql_type: "INTEGER",
                    not_null: true,
                    default: None,
                    primary_key: false,
                },
                ColumnDef {
                    name: "grantee_user_id",
                    sql_type: "INTEGER",
                    not_null: true,
                    default: None,
                    primary_key: false,
                },
                ColumnDef {
                    name: "access",
                    sql_type: "TEXT",
                    not_null: true,
                    default: None,
                    primary_key: false,
                },
            ],
            indexes: &[],
            constraints: &[
                "PRIMARY KEY (space_id, grantee_user_id)",
                "FOREIGN KEY (space_id) REFERENCES space(id) ON DELETE CASCADE",
                "CHECK (access IN ('read', 'write'))",
            ],
        }];
        converge_schema(&conn, T)?;

        conn.execute_batch("INSERT INTO space (id) VALUES (1)")?;
        conn.execute_batch("INSERT INTO grant_acl VALUES (1, 7, 'read')")?;

        // CHECK rejects an access level outside the allowed set.
        assert!(
            conn.execute_batch("INSERT INTO grant_acl VALUES (1, 8, 'admin')")
                .is_err(),
            "CHECK constraint must reject an unlisted access level"
        );
        // Composite PK rejects a duplicate (space, grantee) pair.
        assert!(
            conn.execute_batch("INSERT INTO grant_acl VALUES (1, 7, 'write')")
                .is_err(),
            "composite primary key must reject a duplicate grant"
        );
        // Cascading FK removes grants when the referenced space goes away.
        conn.execute_batch("DELETE FROM space WHERE id = 1")?;
        let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM grant_acl", [], |r| r.get(0))?;
        assert_eq!(remaining, 0, "ON DELETE CASCADE must drop the grant row");

        let again = converge_schema(&conn, T)?;
        assert!(again.is_empty(), "converge must be idempotent");
        Ok(())
    }
}
