//! SQLite driver. Each session owns two actor threads with their own `rusqlite::Connection`:
//! `editor` for the SQL editor (keeps transaction state) and `worker` for browsing/introspection.

mod actor;
mod decode;
mod introspect;

use async_trait::async_trait;
use rusqlite::OpenFlags;

use crate::drivers::{DbSession, ServerInfo};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use actor::Actor;

pub struct LiteSession {
    pub(crate) path: String,
    pub(crate) editor: Actor,
    pub(crate) worker: Actor,
}

impl LiteSession {
    pub async fn open(profile: &ConnectionProfile) -> Result<LiteSession, AppError> {
        let path = profile
            .sqlite_path
            .clone()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| AppError::InvalidInput("SQLite profile has no file path".into()))?;
        let read_only = profile.options.read_only;
        let foreign_keys = profile.options.sqlite_foreign_keys;
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        };
        let editor = Actor::spawn(&path, flags, foreign_keys)?;
        let worker = Actor::spawn(&path, flags, foreign_keys)?;
        Ok(LiteSession {
            path,
            editor,
            worker,
        })
    }
}

#[async_trait]
impl DbSession for LiteSession {
    fn driver(&self) -> Driver {
        Driver::Sqlite
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let path = self.path.clone();
        self.worker
            .run(move |conn| {
                let version: String =
                    conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
                Ok(ServerInfo {
                    version: format!("SQLite {version}"),
                    current_database: Some(path),
                    current_user: None,
                })
            })
            .await
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.worker
            .run(|conn| {
                conn.query_row("SELECT 1", [], |_| Ok(()))?;
                Ok(())
            })
            .await
    }

    async fn close(&self) {
        self.editor.close();
        self.worker.close();
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let path = self.path.clone();
        Ok(vec![DatabaseInfo {
            name: path,
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.worker.run(introspect::list_schemas).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let schema = schema.to_string();
        self.worker
            .run(move |conn| introspect::list_tables(conn, &schema))
            .await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let table = table.clone();
        self.worker
            .run(move |conn| introspect::table_meta(conn, &table))
            .await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let table = table.clone();
        self.worker
            .run(move |conn| introspect::view_definition(conn, &table))
            .await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        self.worker.run(introspect::completion_metadata).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        self.worker
            .run(move |conn| decode::run_query(conn, &sql, &params, None))
            .await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        self.worker
            .run(move |conn| decode::run_execute(conn, &sql, &params))
            .await
    }

    async fn transaction(
        &self,
        statements: Vec<(String, Vec<Value>)>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        self.worker
            .run(move |conn| {
                conn.execute_batch("BEGIN")?;
                let mut out = Vec::with_capacity(statements.len());
                for (sql, params) in &statements {
                    match decode::run_execute(conn, sql, params) {
                        Ok(o) => out.push(o),
                        Err(e) => {
                            let _ = conn.execute_batch("ROLLBACK");
                            return Err(e);
                        }
                    }
                }
                conn.execute_batch("COMMIT")?;
                Ok(out)
            })
            .await
    }
}
