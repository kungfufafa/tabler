//! MySQL / MariaDB driver (mysql_async). A session owns a small worker pool for browsing and
//! introspection plus one dedicated editor connection whose session state (`BEGIN`, `SET …`)
//! persists between statements.

mod connect;
mod decode;
mod exec;
mod introspect;
mod params;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Pool, Row, TxOpts};
use tokio::sync::Mutex;

use crate::drivers::{ConnectTarget, DbSession, ServerInfo};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use decode::{kind_from_column, value_from_mysql};
pub use introspect::{kind_from_data_type, parse_enum_values};
pub use params::bind;

/// How long `close` waits for the server to acknowledge disconnects.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct MySession {
    pool: Pool,
    /// Dedicated SQL-editor connection; `None` once closed.
    editor: Mutex<Option<Conn>>,
    /// Server-side id of the editor connection, for `KILL QUERY <id>` from a pooled connection.
    editor_conn_id: u32,
    is_mariadb: bool,
}

impl MySession {
    pub async fn open(target: &ConnectTarget<'_>) -> Result<MySession, AppError> {
        let opened = connect::open(target).await?;
        Ok(MySession {
            pool: opened.pool,
            editor_conn_id: opened.editor.id(),
            editor: Mutex::new(Some(opened.editor)),
            is_mariadb: opened.is_mariadb,
        })
    }

    /// The editor connection slot (taken by the SQL editor in a later milestone).
    pub fn editor(&self) -> &Mutex<Option<Conn>> {
        &self.editor
    }

    pub fn editor_conn_id(&self) -> u32 {
        self.editor_conn_id
    }

    pub fn is_mariadb(&self) -> bool {
        self.is_mariadb
    }

    async fn conn(&self) -> Result<Conn, AppError> {
        Ok(self.pool.get_conn().await?)
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    Ok(Arc::new(MySession::open(&target).await?))
}

#[async_trait]
impl DbSession for MySession {
    fn driver(&self) -> Driver {
        Driver::Mysql
    }

    fn dialect(&self) -> Dialect {
        Dialect::Mysql
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let mut conn = self.conn().await?;
        let row: Option<Row> = conn
            .query_first("SELECT VERSION(), DATABASE(), CURRENT_USER()")
            .await?;
        let mut row = row.ok_or_else(|| AppError::internal("SELECT VERSION() returned no row"))?;
        let mut text = |i: usize| {
            row.take::<mysql_async::Value, _>(i)
                .and_then(decode::text_of)
        };
        let raw = text(0).unwrap_or_default();
        let current_database = text(1);
        let current_user = text(2);
        let is_mariadb = self.is_mariadb || connect::is_mariadb_version(&raw);
        Ok(ServerInfo {
            version: connect::display_version(&raw, is_mariadb),
            current_database,
            current_user,
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        let mut conn = self.conn().await?;
        conn.ping().await?;
        Ok(())
    }

    async fn close(&self) {
        if let Some(conn) = self.editor.lock().await.take() {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, conn.disconnect()).await;
        }
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.pool.clone().disconnect()).await;
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_databases(&mut conn).await
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_schemas(&mut conn).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_tables(&mut conn, schema).await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let mut conn = self.conn().await?;
        introspect::table_meta(&mut conn, table).await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let mut conn = self.conn().await?;
        introspect::view_definition(&mut conn, table).await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let mut conn = self.conn().await?;
        introspect::completion_metadata(&mut conn).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let mut conn = self.conn().await?;
        exec::run_query(&mut conn, sql, params).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let mut conn = self.conn().await?;
        exec::run_execute(&mut conn, sql, params).await
    }

    async fn transaction(
        &self,
        statements: Vec<(String, Vec<Value>)>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let mut conn = self.conn().await?;
        let mut tx = conn.start_transaction(TxOpts::default()).await?;
        let mut out = Vec::with_capacity(statements.len());
        for (index, (sql, params)) in statements.iter().enumerate() {
            match exec::run_execute(&mut tx, sql, params).await {
                Ok(outcome) => out.push(outcome),
                Err(e) => {
                    // Dropping the transaction would also roll back; do it explicitly so the
                    // pooled connection is clean when it is returned.
                    let _ = tx.rollback().await;
                    return Err(e.with_statement_index(index));
                }
            }
        }
        tx.commit().await?;
        Ok(out)
    }
}
