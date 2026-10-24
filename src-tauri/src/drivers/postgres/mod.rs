//! PostgreSQL driver: tokio-postgres + rustls behind a small deadpool worker pool (browsing,
//! introspection, generic execution) plus one dedicated editor client whose session state
//! (`BEGIN`, `SET`) persists between runs in the SQL editor.

mod connect;
pub mod decode;
mod editor;
pub mod flavor;
mod introspect;
pub mod params;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use deadpool_postgres::{Object, Pool};
use futures::TryStreamExt as _;
use tokio::sync::Mutex;
use tokio_postgres::error::SqlState;
use tokio_postgres::{CancelToken, Client, Config};

use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelHandle, CancelSlot, ConnectTarget, DbSession, RowChunker, RowSink,
    ServerInfo, SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel,
    cancel_requested, cancel_safe_on_stop, check_affected, classify_statement, settle_cancels,
    trace_sql, tx_effect,
};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::Dialect;
use crate::types::browse::TracedSql;
use crate::types::profile::{Driver, SslMode};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use connect::PgTls;

/// How long `close` waits for a cancelled editor statement to give the connection back.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

pub struct PgSession {
    pool: Pool,
    /// Dedicated SQL-editor client; `None` once the session is closed.
    editor: Mutex<Option<Client>>,
    /// Set when the editor's connection task finishes (server closed it, network dropped…).
    editor_lost: Arc<AtomicBool>,
    /// Cancel token of the editor connection, usable while a statement holds the editor.
    editor_token: CancelToken,
    /// Ends the editor's connection task (last resort of `close`).
    editor_task: tokio::task::AbortHandle,
    /// Set by `close`: statements still waiting for the editor give up.
    closing: AtomicBool,
    /// Notices raised on the editor connection since the current statement started.
    notices: connect::NoticeBuffer,
    /// Editor transaction state, tracked from the statements it runs (heuristic).
    tx_state: TxState,
    /// TLS connector and transport config, for cancel requests. `config` is the editor's
    /// (pinned to the address it reached the server at), `pool_config` the pool's (the host
    /// name, resolved per connection).
    tls: PgTls,
    config: Config,
    pool_config: Config,
    /// Where pool connections get their password from when the profile's comes from a command.
    token: Option<Arc<CommandPassword>>,
    /// Held by one editor run at a time (`lock_editor_runs`).
    editor_runs: Mutex<()>,
    /// tokio-postgres's prepared type-lookup statements on the editor, as `PREPARE` statements,
    /// once they had to be re-created (they are then SQL-level and no longer listed by
    /// `editor::protocol_statements`).
    kept_statements: std::sync::Mutex<Vec<(String, String)>>,
    /// The statements this session ran for the app, for the console log (see `SqlTrace`).
    trace: SqlTrace,
}

impl PgSession {
    pub async fn open(target: ConnectTarget<'_>) -> Result<PgSession, AppError> {
        let profile = target.profile;
        let tls = connect::make_tls(&target)?;
        let profile_config = connect::make_config(&target);
        let init_sql = connect::session_init_sql(&profile.options);
        let timeout = connect::connect_timeout(&profile.options);
        let (config, editor, editor_lost, editor_task, notices) = connect::open_editor(
            profile_config.clone(),
            &tls,
            &init_sql,
            timeout,
            profile.ssl == SslMode::Prefer,
        )
        .await?;
        let pool_config = connect::pool_config(&profile_config, &config);
        let token = CommandPassword::for_profile(profile, target.password);
        let pool = connect::build_pool(pool_config.clone(), tls.clone(), init_sql, token.clone())?;
        Ok(PgSession {
            pool,
            editor_token: editor.cancel_token(),
            editor: Mutex::new(Some(editor)),
            editor_lost,
            editor_task,
            closing: AtomicBool::new(false),
            notices,
            tx_state: TxState::default(),
            tls,
            config,
            pool_config,
            token,
            editor_runs: Mutex::new(()),
            kept_statements: std::sync::Mutex::new(Vec::new()),
            trace: SqlTrace::default(),
        })
    }

    /// A worker client from the pool (session init already applied by the `post_create` hook).
    /// When a new connection could not be opened after the password command failed, that failure
    /// is what is reported.
    async fn client(&self) -> Result<Object, AppError> {
        self.pool
            .get()
            .await
            .map_err(|e| match self.token.as_ref().and_then(|t| t.failure()) {
                Some(failure) => failure,
                None => connect::pool_error(e),
            })
    }

    /// Runs `f` with the editor client, or `NotConnected` when it is gone.
    #[allow(dead_code)]
    pub(crate) async fn with_editor<R>(
        &self,
        f: impl AsyncFnOnce(&Client) -> Result<R, AppError>,
    ) -> Result<R, AppError> {
        let guard = self.editor.lock().await;
        let client = guard.as_ref().ok_or(AppError::NotConnected)?;
        if self.editor_unusable(client) {
            return Err(AppError::NotConnected);
        }
        f(client).await
    }

    fn editor_unusable(&self, client: &Client) -> bool {
        self.closing.load(Ordering::SeqCst)
            || self.editor_lost.load(Ordering::SeqCst)
            || client.is_closed()
    }

    fn take_notices(&self) -> Vec<String> {
        self.notices.take_text()
    }

    /// The editor's prepared statements that must survive `DEALLOCATE` / `DISCARD ALL`.
    async fn statements_to_keep(&self, client: &Client) -> Vec<(String, String)> {
        let mut keep = self
            .kept_statements
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for (name, prepare) in editor::protocol_statements(client).await {
            match keep.iter_mut().find(|(n, _)| *n == name) {
                Some(entry) => entry.1 = prepare,
                None => keep.push((name, prepare)),
            }
        }
        keep
    }

    /// A worker client whose catalog queries are recorded in this session's trace.
    fn traced<'a>(&'a self, client: &'a Client) -> introspect::Traced<'a> {
        introspect::Traced::new(client, &self.trace)
    }

    /// Cancels what runs on `client`, which connected with `config` (the editor's or the
    /// pool's).
    fn canceller<'a>(&'a self, client: &Client, config: &'a Config) -> editor::Canceller<'a> {
        editor::Canceller {
            token: client.cancel_token(),
            tls: &self.tls,
            config,
        }
    }
}

/// Runs a row-returning statement through the extended protocol and decodes the binary rows.
pub(crate) async fn run_query(
    client: &Client,
    sql: &str,
    params: &[Value],
) -> Result<ResultSet, AppError> {
    run_query_checked(client, sql, params, None).await
}

/// `run_query` for a run that may be cancelled (`DbSession::query_cancellable`): a cancel
/// requested while the statement was being prepared may have reached the server while it was
/// idle, where it is ignored, so the statement must not start anyway.
async fn run_query_checked(
    client: &Client,
    sql: &str,
    params: &[Value],
    cancel: Option<&CancelSlot>,
) -> Result<ResultSet, AppError> {
    let (types, bound) = params::bind(params)?;
    let stmt = client.prepare_typed(sql, &types).await?;
    if cancel.is_some_and(cancel_requested) {
        return Err(AppError::Cancelled);
    }
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
    run_execute_checked(client, sql, params, None).await
}

/// `run_execute` for a run that may be cancelled (`DbSession::execute_cancellable`): a cancel
/// requested while the statement was being prepared may have reached the server while it was
/// idle, where it is ignored, so the statement must not start anyway.
async fn run_execute_checked(
    client: &Client,
    sql: &str,
    params: &[Value],
    cancel: Option<&CancelSlot>,
) -> Result<ExecOutcome, AppError> {
    let (types, bound) = params::bind(params)?;
    let stmt = client.prepare_typed(sql, &types).await?;
    if cancel.is_some_and(cancel_requested) {
        return Err(AppError::Cancelled);
    }
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

    /// Checks the pool, and keeps the editor connection alive when it is idle (no editor run
    /// holds it; never waits behind one) with an empty query, which changes no session state
    /// and is accepted even in an aborted transaction.
    async fn ping(&self) -> Result<(), AppError> {
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let client = self.client().await?;
        client.query_one("SELECT 1", &[]).await?;
        let Ok(_run) = self.editor_runs.try_lock() else {
            return Ok(());
        };
        let Ok(guard) = self.editor.try_lock() else {
            return Ok(());
        };
        match guard.as_ref() {
            Some(editor) if !self.editor_unusable(editor) => Ok(editor.batch_execute("").await?),
            _ => Ok(()),
        }
    }

    /// Closes the pool and the editor. A statement still running on the editor is cancelled
    /// first, so closing (disconnect, database switch) does not wait for it to finish; if the
    /// server does not react, the editor connection is dropped.
    async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.pool.close();
        if let Ok(mut guard) = self.editor.try_lock() {
            guard.take();
            return;
        }
        let cancelled = editor::cancel_and_wait(&self.editor_token, &self.tls, &self.config).await;
        if let Err(e) = &cancelled {
            log::warn!("postgres: cancelling the editor statement on close failed: {e}");
        }
        if cancelled.is_ok()
            && let Ok(mut guard) = tokio::time::timeout(CLOSE_WAIT, self.editor.lock()).await
        {
            guard.take();
            return;
        }
        // The statement does not stop: drop the editor connection under it.
        log::warn!("postgres: dropping the editor connection of a statement that did not stop");
        self.editor_task.abort();
        if let Ok(mut guard) = tokio::time::timeout(CLOSE_WAIT, self.editor.lock()).await {
            guard.take();
        }
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let client = self.client().await?;
        introspect::list_databases(&self.traced(&client)).await
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let client = self.client().await?;
        introspect::list_schemas(&self.traced(&client)).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let client = self.client().await?;
        introspect::list_tables(&self.traced(&client), schema).await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let client = self.client().await?;
        introspect::table_meta(&self.traced(&client), table).await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let client = self.client().await?;
        introspect::view_definition(&self.traced(&client), table).await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let client = self.client().await?;
        introspect::completion_metadata(&self.traced(&client)).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let client = self.client().await?;
        self.trace
            .run_async(
                classify_statement(sql, Dialect::Postgres),
                trace_sql(Dialect::Postgres, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                run_query(&client, sql, params),
            )
            .await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        let client = self.client().await?;
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Postgres(client.cancel_token()))?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Postgres),
                    trace_sql(Dialect::Postgres, sql, params),
                    |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                    run_query_checked(&client, sql, params, Some(&cancel)),
                )
                .await
        };
        if cancel_requested(&cancel) {
            // The pooled connection goes back to the pool: no late cancel may follow it there.
            settle(&cancel).await;
        }
        result
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let client = self.client().await?;
        self.trace
            .run_async(
                classify_statement(sql, Dialect::Postgres),
                trace_sql(Dialect::Postgres, sql, params),
                TraceCounts::of_exec,
                run_execute(&client, sql, params),
            )
            .await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        let client = self.client().await?;
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Postgres(client.cancel_token()))?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Postgres),
                    trace_sql(Dialect::Postgres, sql, params),
                    TraceCounts::of_exec,
                    run_execute_checked(&client, sql, params, Some(&cancel)),
                )
                .await
        };
        if cancel_requested(&cancel) {
            // The pooled connection goes back to the pool: no late cancel may follow it there.
            settle(&cancel).await;
        }
        result
    }

    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let client = self.client().await?;
        client.batch_execute("BEGIN").await?;
        let mut out = Vec::with_capacity(statements.len());
        for (i, st) in statements.iter().enumerate() {
            match self
                .trace
                .run_async(
                    classify_statement(&st.sql, Dialect::Postgres),
                    trace_sql(Dialect::Postgres, &st.sql, &st.params),
                    TraceCounts::of_exec,
                    run_execute(&client, &st.sql, &st.params),
                )
                .await
                .and_then(|o| {
                    check_affected(st, &o, i)?;
                    Ok(o)
                }) {
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

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        editor::reject_unsupported(sql)?;
        let guard = self.editor.lock().await;
        let client = guard.as_ref().ok_or(AppError::NotConnected)?;
        if self.editor_unusable(client) {
            return Err(AppError::NotConnected);
        }
        let armed = arm_cancel(&cancel, CancelHandle::Postgres(client.cancel_token()))?;
        self.notices.hand_over_earlier(sink);
        // tokio-postgres keeps its type-lookup queries prepared on the connection for good: put
        // back any that the statement drops, or describing would fail from then on.
        let prepared = if editor::drops_prepared_statements(sql) {
            self.statements_to_keep(client).await
        } else {
            Vec::new()
        };
        let run = self
            .run_on_editor(client, sql, max_rows, sink, &cancel)
            .await;
        drop(armed);
        if cancel_requested(&cancel) {
            // A cancel signal still on its way would hit the next statement on the editor (or
            // the savepoint cleanup below).
            settle(&cancel).await;
        }
        let cleanup = run.savepoint_cleanup();
        let mut result = run.result;
        if let Some(cleanup) = cleanup
            && let Err(e) = client.batch_execute(&cleanup).await
        {
            log::warn!("postgres: `{cleanup}` failed: {e}");
            if result.is_ok() {
                result = Err(e.into());
            }
        }
        if !prepared.is_empty() {
            editor::restore_protocol_statements(client, &prepared).await;
            *self
                .kept_statements
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = prepared;
        }
        if let Some(open) = tx_effect(sql, Dialect::Postgres, result.as_ref().err()) {
            self.tx_state.set(Some(open));
        }
        let notices = self.take_notices();
        let mut outcome = result.map(|s| s.outcome).map_err(|e| {
            if self.editor_lost.load(Ordering::SeqCst) {
                AppError::NotConnected
            } else {
                e
            }
        })?;
        outcome.notices.splice(0..0, notices);
        Ok(outcome)
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        let client = self.client().await?;
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Postgres(client.cancel_token()))?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Postgres),
                    trace_sql(Dialect::Postgres, sql, params),
                    |rows: &u64| TraceCounts::rows(*rows as usize),
                    self.stream_on(&client, sql, params, sink),
                )
                .await
        };
        if cancel_requested(&cancel) {
            // The pooled connection goes back to the pool: no late cancel may follow it there.
            settle(&cancel).await;
        }
        result
    }

    /// The handle may be the editor's or a pool connection's: the request goes to the editor's
    /// address (`cancel_and_wait` falls back to the host name when that address is gone).
    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        let (handle, _ticket) = handle.into_parts();
        match handle {
            CancelHandle::Postgres(token) => {
                editor::cancel_and_wait(&token, &self.tls, &self.config).await
            }
            _ => Ok(()),
        }
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx_state.get()
    }

    fn drain_trace(&self) -> Vec<TracedSql> {
        self.trace.drain()
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    fn editor_run_lock(&self) -> Option<&Mutex<()>> {
        Some(&self.editor_runs)
    }
}

async fn settle(cancel: &CancelSlot) {
    if !settle_cancels(cancel, CANCEL_SETTLE_LIMIT).await {
        log::warn!("postgres: a cancel request did not finish in time");
    }
}

/// Savepoint a capped read inside a transaction block runs under (see `run_on_editor`).
const CAP_SAVEPOINT: &str = "tabler_row_cap";

/// How `PgSession::run_on_editor` went.
struct EditorRun {
    result: Result<editor::Streamed, AppError>,
    /// `CAP_SAVEPOINT` was set before the statement.
    savepoint: bool,
    /// The run was cancelled before the statement was sent: the transaction is intact.
    cancelled_before_start: bool,
}

impl EditorRun {
    /// SQL that ends `CAP_SAVEPOINT`, run once no cancel request can land any more.
    fn savepoint_cleanup(&self) -> Option<String> {
        if !self.savepoint {
            return None;
        }
        match &self.result {
            // Stopped at the cap by a server-side cancel: undo the statement, keep the
            // transaction.
            Ok(streamed) if streamed.cancelled => Some(format!(
                "ROLLBACK TO SAVEPOINT {CAP_SAVEPOINT}; RELEASE SAVEPOINT {CAP_SAVEPOINT}"
            )),
            Ok(_) => Some(format!("RELEASE SAVEPOINT {CAP_SAVEPOINT}")),
            Err(_) if self.cancelled_before_start => {
                Some(format!("RELEASE SAVEPOINT {CAP_SAVEPOINT}"))
            }
            // The statement failed (or the user cancelled it): the transaction is aborted, the
            // same as without the savepoint.
            Err(_) => None,
        }
    }
}

impl PgSession {
    /// Describes and runs one editor statement (the editor is locked and the slot armed).
    ///
    /// Stopping a read at the row cap cancels it on the server, which inside a transaction
    /// block would abort the whole transaction. There, a read that may be cancelled at the cap
    /// first sets `CAP_SAVEPOINT` (sent together with the describe round trip) so the cancel can
    /// be rolled back to it (`EditorRun::savepoint_cleanup`) and the transaction goes on.
    /// Statements that must run to completion (`CALL`, DML with `RETURNING`…) are drained.
    async fn run_on_editor(
        &self,
        client: &Client,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: &CancelSlot,
    ) -> EditorRun {
        let mut run = EditorRun {
            result: Err(AppError::Cancelled),
            savepoint: false,
            cancelled_before_start: false,
        };
        let mut in_tx = self.tx_state.get() == Some(true);
        let returns_rows = editor::may_return_rows(sql);
        let wants_savepoint = in_tx && returns_rows && cancel_safe_on_stop(sql, Dialect::Postgres);
        let savepoint_sql = format!("SAVEPOINT {CAP_SAVEPOINT}");
        let (described, savepoint) = futures::join!(
            async {
                match returns_rows {
                    true => editor::describe(client, sql, in_tx).await.map(Some),
                    false => Ok(None),
                }
            },
            async {
                match wants_savepoint {
                    true => Some(client.batch_execute(&savepoint_sql).await),
                    false => None,
                }
            },
        );
        match savepoint {
            None => {}
            Some(Ok(())) => run.savepoint = true,
            // The transaction-state heuristic was wrong: there is no transaction block.
            Some(Err(e)) if e.code() == Some(&SqlState::NO_ACTIVE_SQL_TRANSACTION) => {
                in_tx = false;
                self.tx_state.set(Some(false));
            }
            Some(Err(e)) if e.code() == Some(&SqlState::QUERY_CANCELED) => {
                run.result = Err(AppError::Cancelled);
                return run;
            }
            // An aborted transaction: the statement reports that itself.
            Some(Err(e)) => log::debug!("postgres: setting the row-cap savepoint failed: {e}"),
        }
        let described = match described {
            Ok(d) => d.unwrap_or_default(),
            Err(e) => {
                run.result = Err(e);
                return run;
            }
        };
        // A cancel requested during the describe round trip may have reached the server while
        // it was idle, where it is ignored: the statement must not start anyway.
        if cancel_requested(cancel) {
            run.cancelled_before_start = true;
            run.result = Err(AppError::Cancelled);
            return run;
        }
        let canceller = (!in_tx || run.savepoint).then(|| self.canceller(client, &self.config));
        run.result = editor::stream_simple(
            client,
            sql,
            described.types.as_deref(),
            Some(max_rows),
            canceller,
            Some(&self.notices),
            sink,
        )
        .await
        .map(|mut streamed| {
            if let Some(notice) = described.notice {
                streamed.outcome.notices.insert(0, notice);
            }
            streamed
        });
        run
    }

    /// `stream_query` on an already checked-out worker client.
    async fn stream_on(
        &self,
        client: &Client,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
    ) -> Result<u64, AppError> {
        if params.is_empty() {
            // Text protocol: every type decodes losslessly.
            editor::reject_unsupported(sql)?;
            let described = editor::describe(client, sql, false).await?;
            let streamed = editor::stream_simple(
                client,
                sql,
                described.types.as_deref(),
                None,
                Some(self.canceller(client, &self.pool_config)),
                None,
                sink,
            )
            .await?;
            return Ok(streamed.outcome.row_count);
        }
        // Parameters need the extended protocol (binary rows; sqlgen casts columns to text).
        let (types, bound) = params::bind(params)?;
        let stmt = client.prepare_typed(sql, &types).await?;
        let columns = stmt.columns().iter().map(decode::result_column).collect();
        let stream = client.query_raw(&stmt, params::as_refs(&bound)).await?;
        futures::pin_mut!(stream);
        let mut chunker = RowChunker::new(sink);
        let mut stopped = chunker.begin_set(columns).is_break();
        while !stopped {
            match stream.try_next().await? {
                Some(row) => stopped = chunker.push(decode::decode_row(&row)?).is_break(),
                None => break,
            }
        }
        let _ = chunker.flush();
        let total = chunker.total_rows;
        if stopped {
            // Only a read-only query may be cut short on the server (see `cancel_safe_on_stop`).
            if cancel_safe_on_stop(sql, Dialect::Postgres) {
                self.canceller(client, &self.pool_config).cancel_now().await;
            }
            editor::drain_rows(stream.as_mut()).await?;
        }
        Ok(total)
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    Ok(Arc::new(PgSession::open(target).await?))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::drivers::CollectSink;
    use crate::types::profile::{ConnectionProfile, ProfileOptions};

    /// A session on `TABLER_TEST_PG_URL` (`postgres://user[:pass]@host:port/db`); None (the test
    /// passes) when it is unset.
    async fn open() -> Option<PgSession> {
        let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
            println!("skipped: TABLER_TEST_PG_URL unset");
            return None;
        };
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .expect("a postgres:// URL");
        let (creds, hostdb) = rest.rsplit_once('@').unwrap_or(("", rest));
        let (user, password) = creds
            .split_once(':')
            .map_or((creds, None), |(u, p)| (u, Some(p.to_string())));
        let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
        let db = db.split('?').next().unwrap_or("");
        let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, "5432"));
        let profile = ConnectionProfile {
            id: uuid::Uuid::new_v4(),
            name: "pg-keepalive".into(),
            color: None,
            group: None,
            driver: Driver::Postgres,
            host: Some(host.into()),
            port: port.parse().ok(),
            user: Some(user.into()).filter(|u: &String| !u.is_empty()),
            database: Some(db.into()).filter(|d: &String| !d.is_empty()),
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        let target = ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: None,
        };
        Some(PgSession::open(target).await.expect("connect"))
    }

    /// Runs `sql` on the editor; the first value of its first row.
    async fn on_editor(session: &PgSession, sql: &str) -> Result<Option<Value>, AppError> {
        let mut sink = CollectSink::default();
        session
            .run_statement(sql, 10, &mut sink, CancelSlot::default())
            .await?;
        Ok(sink
            .sets
            .first()
            .and_then(|s| s.rows.first())
            .and_then(|r| r.first())
            .cloned())
    }

    #[tokio::test]
    async fn ping_touches_an_idle_editor_without_changing_its_transaction() {
        let Some(session) = open().await else {
            return;
        };
        let pid = on_editor(&session, "SELECT pg_backend_pid()")
            .await
            .unwrap();
        on_editor(&session, "BEGIN").await.unwrap();
        assert!(on_editor(&session, "SELECT 1/0").await.is_err());
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        session.ping().await.unwrap();
        // The editor's backend just did something (its idle time restarted)…
        let activity = session
            .query(
                "SELECT pid, clock_timestamp() - state_change < interval '500 milliseconds' \
                 FROM pg_stat_activity",
                &[],
            )
            .await
            .unwrap();
        let fresh = activity.rows.iter().find(|r| r.first() == pid.as_ref());
        assert_eq!(fresh.and_then(|r| r.get(1)), Some(&Value::Bool(true)));
        // …and its transaction is still the aborted one.
        assert!(on_editor(&session, "SELECT 1").await.is_err());
        on_editor(&session, "ROLLBACK").await.unwrap();
        session.close().await;
    }

    #[tokio::test]
    async fn ping_does_not_wait_for_a_running_editor_statement() {
        let Some(session) = open().await else {
            return;
        };
        let session = Arc::new(session);
        let running = tokio::spawn({
            let session = session.clone();
            async move { on_editor(&session, "SELECT pg_sleep(2)").await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        session.ping().await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        running.await.unwrap().unwrap();
        session.close().await;
    }
}
