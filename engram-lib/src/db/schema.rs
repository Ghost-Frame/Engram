use crate::db::schema_sql::{
    AUXILIARY_SCHEMA_STATEMENTS, CORE_SCHEMA_SQL, LIBSQL_VECTOR_INDEX_STATEMENTS,
};
use crate::Result;
#[cfg(feature = "db_pool")]
use crate::EngError;
use libsql::Connection as LibsqlConnection;

pub async fn create_tables(conn: &LibsqlConnection) -> Result<()> {
    conn.execute_batch(CORE_SCHEMA_SQL).await?;

    for statement in AUXILIARY_SCHEMA_STATEMENTS {
        conn.execute(statement, ()).await?;
    }

    for statement in LIBSQL_VECTOR_INDEX_STATEMENTS {
        conn.execute(statement, ()).await?;
    }

    Ok(())
}

#[cfg(feature = "db_pool")]
pub fn create_tables_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(CORE_SCHEMA_SQL)
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    for statement in AUXILIARY_SCHEMA_STATEMENTS {
        conn.execute(statement, [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    Ok(())
}
