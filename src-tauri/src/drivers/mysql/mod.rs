//! MySQL / MariaDB driver (mysql_async). A session owns a small worker pool for browsing and
//! introspection plus one dedicated editor connection whose session state (`BEGIN`, `SET …`)
//! persists between statements.

mod connect;
mod decode;
mod editor;
mod exec;
mod introspect;
mod params;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, Pool, Row, TxOpts};
use tokio::sync::Mutex;

use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelHandle, CancelSlot, ConnectTarget, DbSession, LATE_CANCEL_NOTICE,
    RowSink, ServerInfo, StatementOutcome, TxState, TxStatement, arm_cancel, cancel_requested,
    cancel_safe_on_stop, check_affected, settle_cancels, tx_effect,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
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
    /// The pool's connect options, for side connections when the pool is exhausted.
    pool_opts: Opts,
    /// Dedicated SQL-editor connection; `None` once closed.
    editor: Mutex<Option<Conn>>,
    /// Server-side id of the editor connection, for `KILL QUERY <id>` from a pooled connection.
    editor_conn_id: u32,
    is_mariadb: bool,
    /// Editor transaction state, tracked from the statements it runs (heuristic).
    tx_state: TxState,
    /// Set once the editor connection broke (server restart, `wait_timeout`, `KILL`, network):
    /// the editor then reports `NotConnected` and `ping` fails, so the connection shows as lost.
    editor_lost: AtomicBool,
    /// Set by `close`: statements still waiting for the editor give up.
    closing: AtomicBool,
    /// Held by one editor run at a time (`lock_editor_runs`).
    editor_runs: Mutex<()>,
}

impl MySession {
    pub async fn open(target: &ConnectTarget<'_>) -> Result<MySession, AppError> {
        let opened = connect::open(target).await?;
        Ok(MySession {
            pool: opened.pool,
            pool_opts: opened.pool_opts,
            editor_conn_id: opened.editor.id(),
            editor: Mutex::new(Some(opened.editor)),
            is_mariadb: opened.is_mariadb,
            tx_state: TxState::default(),
            editor_lost: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            editor_runs: Mutex::new(()),
        })
    }

    /// The dedicated SQL-editor connection.
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

    fn editor_unusable(&self) -> bool {
        self.closing.load(Ordering::SeqCst) || self.editor_lost.load(Ordering::SeqCst)
    }

    fn killer(&self, conn_id: u32) -> editor::Killer<'_> {
        editor::Killer {
            pool: &self.pool,
            opts: &self.pool_opts,
            conn_id,
        }
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    Ok(Arc::new(MySession::open(&target).await?))
}

async fn settle(cancel: &CancelSlot) {
    if !settle_cancels(cancel, CANCEL_SETTLE_LIMIT).await {
        log::warn!("mysql: a KILL request did not finish in time");
    }
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

    /// Checks the pool and the editor. The editor itself is not pinged (`COM_PING` resets the
    /// `ROW_COUNT()` the user may read next): a broken editor connection is noticed by the
    /// statement that hits it (`run_statement` sets `editor_lost`) and reported here.
    async fn ping(&self) -> Result<(), AppError> {
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let mut conn = self.conn().await?;
        conn.ping().await?;
        Ok(())
    }

    /// Closes the editor and the pool. A statement still running on the editor is ended first
    /// (`KILL CONNECTION`), so closing (disconnect, database switch) does not wait for it.
    async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let editor = match self.editor.try_lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => {
                if let Err(e) =
                    editor::kill_connection(&self.pool, &self.pool_opts, self.editor_conn_id).await
                {
                    log::warn!("mysql: ending the editor statement on close failed: {e}");
                }
                match tokio::time::timeout(CLOSE_TIMEOUT, self.editor.lock()).await {
                    Ok(mut guard) => guard.take(),
                    Err(_) => {
                        log::warn!("mysql: the editor statement did not stop before closing");
                        None
                    }
                }
            }
        };
        if let Some(conn) = editor {
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
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let mut conn = self.conn().await?;
        let mut tx = conn.start_transaction(TxOpts::default()).await?;
        let mut out = Vec::with_capacity(statements.len());
        for (index, st) in statements.iter().enumerate() {
            match exec::run_execute(&mut tx, &st.sql, &st.params)
                .await
                .and_then(|o| {
                    check_affected(st, &o, index)?;
                    Ok(o)
                }) {
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

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        let mut guard = self.editor.lock().await;
        if self.editor_unusable() {
            return Err(AppError::NotConnected);
        }
        let conn = guard.as_mut().ok_or(AppError::NotConnected)?;
        let killer = self.killer(self.editor_conn_id);
        let result = {
            let _armed = arm_cancel(
                &cancel,
                CancelHandle::Mysql {
                    conn_id: self.editor_conn_id,
                },
            )?;
            editor::run(conn, sql, &[], Some(max_rows), sink, &killer, true).await
        };
        if conn.is_disconnected() {
            // mysql_async closes the stream on any I/O or protocol error: the editor is gone.
            log::warn!("mysql: the editor connection was lost: {result:?}");
            guard.take();
            self.editor_lost.store(true, Ordering::SeqCst);
            return Err(AppError::NotConnected);
        }
        let result = if cancel_requested(&cancel) {
            // Every KILL sent for this run must have landed before the editor runs anything
            // else, or it would abort the next statement (another run's).
            settle(&cancel).await;
            editor::absorb_stray_kill(conn).await;
            match result {
                // A killed `SELECT SLEEP(n)` ends without an error; report what the user asked
                // for. Only a provably read-only statement may be reported that way: anything
                // else (`CALL`, a `DELIMITER` batch, DML) may have committed changes that a
                // "cancelled" result would hide, so it keeps its real outcome.
                Ok(outcome)
                    if outcome.kind == StatementKind::Rows
                        && cancel_safe_on_stop(sql, Dialect::Mysql) =>
                {
                    Err(AppError::Cancelled)
                }
                Ok(mut outcome) => {
                    outcome.notices.push(LATE_CANCEL_NOTICE.to_string());
                    Ok(outcome)
                }
                other => other,
            }
        } else {
            result
        };
        if let Some(open) = tx_effect(sql, Dialect::Mysql, result.as_ref().err()) {
            self.tx_state.set(Some(open));
        }
        result
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        let mut conn = self.conn().await?;
        let conn_id = conn.id();
        let killer = self.killer(conn_id);
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Mysql { conn_id })?;
            editor::run(&mut conn, sql, params, None, sink, &killer, false).await
        };
        if cancel_requested(&cancel) && !conn.is_disconnected() {
            // The connection goes back to the pool: no late KILL may follow it there.
            settle(&cancel).await;
            editor::absorb_stray_kill(&mut conn).await;
        }
        Ok(result?.row_count)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        // The ticket (if any) is released once the KILL has been sent.
        let (handle, _ticket) = handle.into_parts();
        match handle {
            CancelHandle::Mysql { conn_id } => {
                editor::kill_query(&self.pool, &self.pool_opts, conn_id).await
            }
            _ => Ok(()),
        }
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx_state.get()
    }

    fn is_mariadb(&self) -> bool {
        self.is_mariadb
    }

    fn editor_run_lock(&self) -> Option<&Mutex<()>> {
        Some(&self.editor_runs)
    }
}
