//! PostgreSQL driver: tokio-postgres + rustls behind a small deadpool worker pool (browsing,
//! introspection, generic execution) plus one dedicated editor client whose session state
//! (`BEGIN`, `SET`) persists between runs in the SQL editor.

mod connect;
pub mod decode;
mod introspect;
pub mod params;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use deadpool_postgres::{Object, Pool};
use futures::TryStreamExt as _;
use tokio::sync::Mutex;
use tokio_postgres::{Client, Config};

use crate::drivers::{ConnectTarget, DbSession, ServerInfo};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::{Driver, SslMode};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use connect::PgTls;

pub struct PgSession {
    pool: Pool,
    /// Dedicated SQL-editor client; `None` once the session is closed.
    editor: Mutex<Option<Client>>,
    /// Set when the editor's connection task finishes (server closed it, network dropped…).
    editor_lost: Arc<AtomicBool>,
    /// Kept for cancel tokens / side connections in later milestones.
    #[allow(dead_code)]
    tls: PgTls,
    #[allow(dead_code)]
    config: Config,
}

impl PgSession {
    pub async fn open(target: ConnectTarget<'_>) -> Result<PgSession, AppError> {
        let profile = target.profile;
        let tls = connect::make_tls(&target)?;
        let config = connect::make_config(&target);
        let init_sql = connect::session_init_sql(&profile.options);
        let timeout = connect::connect_timeout(&profile.options);
        let (config, editor, editor_lost) = connect::open_editor(
            config,
            &tls,
            &init_sql,
            timeout,
            profile.ssl == SslMode::Prefer,
        )
        .await?;
        let pool = connect::build_pool(config.clone(), tls.clone(), init_sql)?;
        Ok(PgSession {
            pool,
            editor: Mutex::new(Some(editor)),
            editor_lost,
            tls,
            config,
        })
    }

    /// A worker client from the pool (session init already applied by the `post_create` hook).
    async fn client(&self) -> Result<Object, AppError> {
        self.pool.get().await.map_err(connect::pool_error)
    }

    /// Runs `f` with the editor client, or `NotConnected` when it is gone.
    #[allow(dead_code)]
    pub(crate) async fn with_editor<R>(
        &self,
        f: impl AsyncFnOnce(&Client) -> Result<R, AppError>,
    ) -> Result<R, AppError> {
        let guard = self.editor.lock().await;
        let client = guard.as_ref().ok_or(AppError::NotConnected)?;
        if self.editor_lost.load(Ordering::SeqCst) || client.is_closed() {
            return Err(AppError::NotConnected);
        }
        f(client).await
    }
}

/// Runs a row-returning statement through the extended protocol and decodes the binary rows.
pub(crate) async fn run_query(
    client: &Client,
    sql: &str,
    params: &[Value],
) -> Result<ResultSet, AppError> {
    let (types, bound) = params::bind(params)?;
    let stmt = client.prepare_typed(sql, &types).await?;
    let columns = stmt.columns().iter().map(decode::result_column).collect();
    let stream = client.query_raw(&stmt, params::as_refs(&bound)).await?;
    futures::pin_mut!(stream);
    let mut rows = Vec::new();
    while let Some(row) = stream.try_next().await? {
        rows.push(decode::decode_row(&row)?);
    }
    Ok(ResultSet {
        columns,
        rows,
        truncated: false,
    })
}

/// Runs a statement for its side effects; statements with result columns (`RETURNING`, or a
/// SELECT) also return their rows.
pub(crate) async fn run_execute(
    client: &Client,
    sql: &str,
    params: &[Value],
) -> Result<ExecOutcome, AppError> {
    let (types, bound) = params::bind(params)?;
    let stmt = client.prepare_typed(sql, &types).await?;
    let has_columns = !stmt.columns().is_empty();
    let columns: Vec<_> = stmt.columns().iter().map(decode::result_column).collect();
    let stream = client.query_raw(&stmt, params::as_refs(&bound)).await?;
    futures::pin_mut!(stream);
    let mut rows = Vec::new();
    while let Some(row) = stream.try_next().await? {
        if has_columns {
            rows.push(decode::decode_row(&row)?);
        }
    }
    Ok(ExecOutcome {
        affected: stream.rows_affected().unwrap_or(0),
        last_insert_id: None,
        rows: has_columns.then_some(ResultSet {
            columns,
            rows,
            truncated: false,
        }),
    })
}

#[async_trait]
impl DbSession for PgSession {
    fn driver(&self) -> Driver {
        Driver::Postgres
    }

    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let client = self.client().await?;
        let row = client
            .query_one(
                "SELECT version()::text, current_database()::text, current_user::text",
                &[],
            )
            .await?;
        Ok(ServerInfo {
            version: row.try_get(0)?,
            current_database: row.try_get(1)?,
            current_user: row.try_get(2)?,
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let client = self.client().await?;
        client.query_one("SELECT 1", &[]).await?;
        Ok(())
    }

    async fn close(&self) {
        self.pool.close();
        self.editor.lock().await.take();
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let client = self.client().await?;
        introspect::list_databases(&client).await
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let client = self.client().await?;
        introspect::list_schemas(&client).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let client = self.client().await?;
        introspect::list_tables(&client, schema).await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let client = self.client().await?;
        introspect::table_meta(&client, table).await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let client = self.client().await?;
        introspect::view_definition(&client, table).await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let client = self.client().await?;
        introspect::completion_metadata(&client).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let client = self.client().await?;
        run_query(&client, sql, params).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let client = self.client().await?;
        run_execute(&client, sql, params).await
    }

    async fn transaction(
        &self,
        statements: Vec<(String, Vec<Value>)>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let client = self.client().await?;
        client.batch_execute("BEGIN").await?;
        let mut out = Vec::with_capacity(statements.len());
        for (i, (sql, params)) in statements.iter().enumerate() {
            match run_execute(&client, sql, params).await {
                Ok(outcome) => out.push(outcome),
                Err(err) => {
                    if client.batch_execute("ROLLBACK").await.is_err() {
                        // The connection is in an unknown state: drop it from the pool.
                        drop(Object::take(client));
                    }
                    return Err(err.with_statement_index(i));
                }
            }
        }
        if let Err(err) = client.batch_execute("COMMIT").await {
            if client.batch_execute("ROLLBACK").await.is_err() {
                drop(Object::take(client));
            }
            return Err(err.into());
        }
        Ok(out)
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    Ok(Arc::new(PgSession::open(target).await?))
}
