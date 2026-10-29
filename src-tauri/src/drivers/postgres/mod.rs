//! PostgreSQL driver: tokio-postgres + rustls behind a small deadpool worker pool (browsing,
//! introspection, generic execution) plus one dedicated editor client whose session state
//! (`BEGIN`, `SET`) persists between runs in the SQL editor. The PostgreSQL-compatible engines
//! it also serves (CockroachDB, Greenplum, Redshift) differ through `flavor::PgFlavorHooks`,
//! picked at connect for the engine the server names.

mod connect;
pub mod decode;
mod editor;
pub mod flavor;
pub(crate) mod introspect;
pub mod params;
#[cfg(test)]
pub(crate) mod testing;

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
use crate::sqlgen::dialect::spec::ExplainForm;
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::TracedSql;
use crate::types::caps::SessionCaps;
use crate::types::driver::{Flavor, PgFlavor};
use crate::types::profile::{Driver, SslMode};
use crate::types::query::{ExecOutcome, ExplainMode, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use connect::PgTls;
use flavor::PgFlavorHooks;

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
    /// The engine the server is: the one its `version()` named at connect, otherwise the one its
    /// profile says (`ProfileOptions::pg_flavor`).
    flavor: Option<PgFlavor>,
    /// `server_version_num` as libpq computes it from the server's `server_version`.
    server_version_num: i32,
    /// What that engine does differently (`flavor::hooks`).
    hooks: &'static dyn PgFlavorHooks,
}

/// Picks the hooks of an engine (`flavor::hooks`; tests pass their own).
type PickHooks = fn(Option<PgFlavor>) -> &'static dyn PgFlavorHooks;

/// How often `transaction` runs when the engine asks for retries (`retry_serialization`).
const SERIALIZATION_ATTEMPTS: u32 = 3;

impl PgSession {
    pub async fn open(target: ConnectTarget<'_>) -> Result<PgSession, AppError> {
        PgSession::open_with(target, flavor::hooks).await
    }

    /// Connects the editor, asks the server which engine it is (that engine overrides the
    /// profile's), and runs that engine's session init on the editor and on each pool
    /// connection.
    async fn open_with(target: ConnectTarget<'_>, pick: PickHooks) -> Result<PgSession, AppError> {
        let profile = target.profile;
        let profile_flavor = profile.options.pg_flavor;
        let tls = connect::make_tls(&target)?;
        let profile_config = connect::make_config(&target, pick(profile_flavor).default_database());
        let timeout = connect::connect_timeout(&profile.options);
        let editor = connect::open_editor(
            profile_config.clone(),
            &tls,
            timeout,
            profile.ssl == SslMode::Prefer,
        )
        .await?;
        let found = flavor::detect(&connect::server_version(&editor.client).await?);
        if let Some(found) = found
            && profile_flavor != Some(found)
        {
            log::info!("the server is {found:?}, not {profile_flavor:?} as its profile says");
        }
        let engine = found.or(profile_flavor);
        let hooks = pick(engine);
        flavor::refuse_unsupported(found, hooks.sql_dialect())?;
        let options = &profile.options;
        let init = connect::SessionInit::new(
            hooks.init_sql(
                profile,
                connect::read_only_session(options),
                connect::session_init_statements(options),
            ),
            hooks.init_one_by_one(),
        );
        init.run(&editor.client).await?;
        let pool_config = connect::pool_config(&profile_config, &editor.config);
        let token = CommandPassword::for_profile(profile, target.password);
        let pool = connect::build_pool(pool_config.clone(), tls.clone(), init, token.clone())?;
        Ok(PgSession {
            pool,
            editor_token: editor.client.cancel_token(),
            editor: Mutex::new(Some(editor.client)),
            editor_lost: editor.lost,
            editor_task: editor.task,
            closing: AtomicBool::new(false),
            notices: editor.notices,
            tx_state: TxState::default(),
            tls,
            config: editor.config,
            pool_config,
            token,
            editor_runs: Mutex::new(()),
            kept_statements: std::sync::Mutex::new(Vec::new()),
            trace: SqlTrace::default(),
            flavor: engine,
            server_version_num: editor
                .server_version
                .as_deref()
                .map_or(0, flavor::server_version_num),
            hooks,
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
        introspect::Traced::new(client, &self.trace, self.server_version_num)
    }

    /// Plain PostgreSQL's hooks (`VANILLA`): the callers' own SQL does what the engine hooks
    /// would, without a pool connection taken for nothing.
    fn plain(&self) -> bool {
        self.hooks.flavor().is_none()
    }

    /// `transaction` on one pool connection.
    async fn transaction_on(
        &self,
        client: Object,
        statements: &[TxStatement],
    ) -> Result<Vec<ExecOutcome>, AppError> {
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
        affected_unknown: false,
    })
}

#[async_trait]
impl DbSession for PgSession {
    fn driver(&self) -> Driver {
        Driver::Postgres
    }

    fn dialect(&self) -> SqlDialect {
        self.hooks.sql_dialect()
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

    fn flavor(&self) -> Option<Flavor> {
        self.flavor.map(flavor::session_flavor)
    }

    /// The PostgreSQL caps, without what a flavor is not proven to do (`flavor::adjust_caps`)
    /// unless its hooks grant it back.
    fn capabilities(&self) -> SessionCaps {
        let mut caps = flavor::adjust_caps(SessionCaps::base(
            Driver::Postgres,
            self.dialect(),
            self.flavor(),
        ));
        self.hooks.adjust_caps(&mut caps, self.server_version_num);
        caps
    }

    fn explain_form(&self, mode: ExplainMode) -> Option<ExplainForm> {
        self.hooks.explain_form(mode)
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        if self.plain() {
            return None;
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return Some(Err(e)),
        };
        self.hooks.list_processes(&self.traced(&client)).await
    }

    async fn kill_processes(&self, ids: &[String], scope: KillScope) -> Option<Vec<KillOutcome>> {
        if self.plain() {
            return None;
        }
        let client = match self.client().await {
            Ok(client) => client,
            // What the core path reports when it cannot reach the server: each kill failed.
            Err(e) => {
                let failed = |id: &String| KillOutcome {
                    id: id.clone(),
                    ok: false,
                    message: Some(e.to_string()),
                };
                return Some(ids.iter().map(failed).collect());
            }
        };
        self.hooks.kill(&self.traced(&client), ids, scope).await
    }

    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        if self.plain() {
            return None;
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return Some(Err(e)),
        };
        self.hooks.table_ddl(&self.traced(&client), table).await
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

    // The catalog reads: the engine's hooks first, then the `pg_catalog` queries.

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.list_databases(&c).await {
            Some(databases) => databases,
            None => introspect::list_databases(&c).await,
        }
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.list_schemas(&c).await {
            Some(schemas) => schemas,
            None => introspect::list_schemas(&c).await,
        }
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.list_tables(&c, schema).await {
            Some(tables) => tables,
            None => introspect::list_tables(&c, schema).await,
        }
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.table_meta(&c, table).await {
            Some(meta) => meta,
            None => introspect::table_meta(&c, table).await,
        }
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.view_definition(&c, table).await {
            Some(definition) => definition,
            None => introspect::view_definition(&c, table).await,
        }
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let client = self.client().await?;
        let c = self.traced(&client);
        match self.hooks.completion_metadata(&c).await {
            Some(meta) => meta,
            None => introspect::completion_metadata(&c).await,
        }
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

    /// Runs `statements` in one transaction. An engine that aborts transactions it cannot
    /// serialize and expects the client to run them again (`retry_serialization`) gets them
    /// again, up to `SERIALIZATION_ATTEMPTS` times; nothing of an aborted attempt remains.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let mut attempt = 1;
        loop {
            let client = self.client().await?;
            match self.transaction_on(client, &statements).await {
                Err(e)
                    if attempt < SERIALIZATION_ATTEMPTS
                        && self.hooks.retry_serialization()
                        && is_serialization_failure(&e) =>
                {
                    log::info!("postgres: running the transaction again after: {e}");
                    tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await;
                    attempt += 1;
                }
                result => return result,
            }
        }
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

/// The server aborted the transaction because it could not serialize it (40001).
fn is_serialization_failure(e: &AppError) -> bool {
    matches!(e, AppError::Db { sqlstate: Some(code), .. } if code == SqlState::T_R_SERIALIZATION_FAILURE.code())
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
    /// Statements that must run to completion (`CALL`, DML with `RETURNING`…) are drained, and
    /// so is a read in a transaction block on an engine without savepoints
    /// (`PgFlavorHooks::row_cap_savepoint`).
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
        // An engine without savepoints drains the read instead (no canceller below).
        let wants_savepoint = in_tx
            && returns_rows
            && cancel_safe_on_stop(sql, Dialect::Postgres)
            && self.hooks.row_cap_savepoint();
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
    use crate::drivers::postgres::flavor::{InitKind, InitStatement};
    use crate::drivers::postgres::testing::{Cell, Col, FakePg};
    use crate::types::caps::Feature;
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

    /// A profile for the fake server on `port`.
    fn fake_profile(port: u16) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "fake".into(),
            color: None,
            group: None,
            driver: Driver::Postgres,
            host: Some("127.0.0.1".into()),
            port: Some(port),
            user: Some("tabler".into()),
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    fn target(profile: &ConnectionProfile) -> ConnectTarget<'_> {
        crate::testkit::install_crypto();
        ConnectTarget {
            profile,
            password: Some("pw"),
            host_override: None,
        }
    }

    /// What a fake server received, section by section, one statement per line: the connection
    /// (`c1` is the first one that sent something) and the SQL as a JSON string.
    #[derive(Default)]
    struct Received {
        text: String,
        seen: usize,
        connections: Vec<i32>,
    }

    impl Received {
        fn section(&mut self, name: &str, pg: &FakePg) {
            self.text.push_str(&format!("# {name}\n"));
            let statements = pg.statements();
            for (pid, sql) in &statements[self.seen..] {
                if !self.connections.contains(pid) {
                    self.connections.push(*pid);
                }
                let n = self.connections.iter().position(|p| p == pid).unwrap() + 1;
                let sql = serde_json::to_string(sql).unwrap();
                self.text.push_str(&format!("c{n} {sql}\n"));
            }
            self.seen = statements.len();
        }

        /// Compares with `tests/fixtures/postgres/<name>.txt`, or rewrites it under
        /// `TABLER_BLESS=1`.
        fn check(&self, name: &str) {
            let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/postgres")
                .join(format!("{name}.txt"));
            if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &self.text).unwrap();
                return;
            }
            let golden = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e} (TABLER_BLESS=1 writes it)", path.display()))
                .replace("\r\n", "\n");
            let want: Vec<&str> = golden.lines().collect();
            let got: Vec<&str> = self.text.lines().collect();
            if let Some(at) = (0..want.len().max(got.len())).find(|i| want.get(*i) != got.get(*i)) {
                panic!(
                    "{} differs at line {}:\n  golden: {}\n  now:    {}\nIf the change is \
                     intended: TABLER_BLESS=1 cargo test --lib drivers::postgres, then review \
                     the diff.",
                    path.display(),
                    at + 1,
                    want.get(at).unwrap_or(&"<end of file>"),
                    got.get(at).unwrap_or(&"<end of file>"),
                );
            }
        }
    }

    /// Plain PostgreSQL gets exactly the SQL it got before the flavor seam: the session setup
    /// and every catalog read, as the server received them (`tests/fixtures/postgres/
    /// vanilla.txt`).
    #[tokio::test]
    async fn plain_postgres_gets_the_same_sql() {
        let pg = FakePg::new()
            .server_version("17.2")
            .version("PostgreSQL 17.2 on x86_64-pc-linux-gnu, compiled by gcc")
            .rows(
                "SELECT (current_schemas(false))[1]::text",
                vec![Col::text("current_schemas")],
                vec![vec![Cell::text("public")]],
            )
            .rows(
                "SELECT c.relkind::text, obj_description",
                vec![
                    Col::text("relkind"),
                    Col::text("comment"),
                    Col::int8("reltuples"),
                ],
                vec![vec![Cell::text("r"), Cell::Null, Cell::Null]],
            )
            .start()
            .await;
        let mut profile = fake_profile(pg.port());
        profile.options.timezone = Some("Asia/Jakarta".into());
        profile.options.startup_sql = Some("SET search_path = app, public".into());
        let mut received = Received::default();
        let session = PgSession::open(target(&profile)).await.unwrap();
        received.section("open", &pg);
        session.server_info().await.unwrap();
        received.section("server_info", &pg);
        session.list_databases().await.unwrap();
        received.section("list_databases", &pg);
        session.list_schemas().await.unwrap();
        received.section("list_schemas", &pg);
        session.list_tables("public").await.unwrap();
        received.section("list_tables", &pg);
        let view = TableRef::new(Some("public"), "v");
        session.view_definition(&view).await.unwrap();
        received.section("view_definition", &pg);
        let meta = session.table_meta(&TableRef::new(None, "t")).await.unwrap();
        assert_eq!(meta.table, TableRef::new(Some("public"), "t"));
        received.section("table_meta", &pg);
        session.completion_metadata().await.unwrap();
        received.section("completion_metadata", &pg);
        session.close().await;
        received.check("vanilla");
    }

    const COCKROACH: &str =
        "CockroachDB CCL v24.2.4 (x86_64-pc-linux-gnu, built 2024/10/14, go1.22.5)";
    const GREENPLUM: &str = "PostgreSQL 9.4.26 (Greenplum Database 6.27.1 build commit:1d9f0a3) \
                             on x86_64-unknown-linux-gnu, compiled by gcc";
    const REDSHIFT: &str = "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by GCC gcc (GCC) \
                            3.4.2 20041017 (Red Hat 3.4.2-6.fc3), Redshift 1.0.77467";

    fn vanilla(_: Option<PgFlavor>) -> &'static dyn PgFlavorHooks {
        &flavor::VANILLA
    }

    /// The hooks of a made-up engine that changes everything an engine's hooks can.
    struct TestHooks;

    #[async_trait]
    impl PgFlavorHooks for TestHooks {
        fn flavor(&self) -> Option<PgFlavor> {
            Some(PgFlavor::Redshift)
        }

        fn sql_dialect(&self) -> SqlDialect {
            SqlDialect::Redshift
        }

        fn default_database(&self) -> &'static str {
            "dev"
        }

        fn init_sql(
            &self,
            _: &ConnectionProfile,
            _: bool,
            base: Vec<InitStatement>,
        ) -> Vec<InitStatement> {
            let unknown = |s: &InitStatement| {
                matches!(s.kind, InitKind::IntervalStyle | InitKind::FloatDigits)
            };
            base.into_iter().filter(|s| !unknown(s)).collect()
        }

        fn init_one_by_one(&self) -> bool {
            true
        }

        async fn list_schemas(
            &self,
            c: &introspect::Traced<'_>,
        ) -> Option<Result<Vec<SchemaInfo>, AppError>> {
            Some(Ok(vec![SchemaInfo {
                name: format!("v{}", c.server_version_num()),
                is_default: true,
            }]))
        }

        async fn list_processes(
            &self,
            _: &introspect::Traced<'_>,
        ) -> Option<Result<Vec<ProcessRow>, AppError>> {
            Some(Ok(Vec::new()))
        }

        async fn kill(
            &self,
            _: &introspect::Traced<'_>,
            ids: &[String],
            _: KillScope,
        ) -> Option<Vec<KillOutcome>> {
            let killed = |id: &String| KillOutcome {
                id: id.clone(),
                ok: true,
                message: None,
            };
            Some(ids.iter().map(killed).collect())
        }

        async fn table_ddl(
            &self,
            _: &introspect::Traced<'_>,
            table: &TableRef,
        ) -> Option<Result<String, AppError>> {
            Some(Ok(format!("CREATE TABLE {}", table.name)))
        }

        fn explain_form(&self, _: ExplainMode) -> Option<ExplainForm> {
            Some(ExplainForm::Prefix("EXPLAIN VERBOSE"))
        }

        fn adjust_caps(&self, caps: &mut SessionCaps, server_version_num: i32) {
            if server_version_num == 80_002 {
                *caps = caps.clone().with(&[Feature::Users]);
            }
        }

        fn retry_serialization(&self) -> bool {
            true
        }

        fn row_cap_savepoint(&self) -> bool {
            false
        }
    }

    static TEST_HOOKS: TestHooks = TestHooks;

    fn test_hooks(f: Option<PgFlavor>) -> &'static dyn PgFlavorHooks {
        match f {
            Some(PgFlavor::Redshift) => &TEST_HOOKS,
            _ => &flavor::VANILLA,
        }
    }

    /// The fake answering `version()` with a CockroachDB, a Greenplum and a Redshift string: the
    /// session is on that engine, and without the engine's own hooks it runs the PostgreSQL code
    /// paths with less (CockroachDB, Greenplum) or not at all (Redshift).
    #[tokio::test]
    async fn the_server_names_its_engine_at_connect() {
        for (version, server_version, engine, num) in [
            (COCKROACH, "13.0.0", PgFlavor::Cockroachdb, 130_000),
            (GREENPLUM, "9.4.26", PgFlavor::Greenplum, 90_426),
        ] {
            assert_eq!(flavor::detect(version), Some(engine));
            let pg = FakePg::new()
                .server_version(server_version)
                .version(version)
                .start()
                .await;
            let profile = fake_profile(pg.port());
            let session = PgSession::open_with(target(&profile), vanilla)
                .await
                .unwrap();
            assert_eq!(session.flavor(), Some(flavor::session_flavor(engine)));
            assert_eq!(session.dialect(), SqlDialect::Postgres);
            assert_eq!(session.server_version_num, num);
            let caps = session.capabilities();
            assert!(caps.has(Feature::EditRows), "{engine:?}");
            for f in flavor::UNPROVEN_ON_FLAVORS {
                assert!(!caps.has(f), "{engine:?} {f:?}");
            }
            // The callers' PostgreSQL SQL, without a pool connection taken for it.
            let table = TableRef::new(Some("public"), "t");
            assert!(session.list_processes().await.is_none());
            assert!(
                session
                    .kill_processes(&["1".into()], KillScope::Query)
                    .await
                    .is_none()
            );
            assert!(session.table_ddl(&table).await.is_none());
            assert_eq!(session.explain_form(ExplainMode::Analyze), None);
            assert_eq!(pg.startups().len(), 1, "{engine:?}");
            session.close().await;
        }

        assert_eq!(flavor::detect(REDSHIFT), Some(PgFlavor::Redshift));
        let pg = FakePg::new()
            .server_version("8.0.2")
            .version(REDSHIFT)
            .start()
            .await;
        let err = PgSession::open_with(target(&fake_profile(pg.port())), vanilla)
            .await
            .err()
            .expect("refused");
        assert_eq!(
            err.to_string(),
            "unsupported: Amazon Redshift is not built into this Tabler"
        );
        // Refused before anything of PostgreSQL's was sent.
        assert_eq!(pg.queries(), ["SELECT version()"]);
    }

    /// The engine the server names wins over the profile's; the profile's stands when the server
    /// names none.
    #[tokio::test]
    async fn the_engine_the_server_names_overrides_the_profiles() {
        for (version, profile_engine, engine) in [
            (
                COCKROACH,
                Some(PgFlavor::Greenplum),
                Some(Flavor::Cockroachdb),
            ),
            (
                "PostgreSQL 17.2",
                Some(PgFlavor::Greenplum),
                Some(Flavor::Greenplum),
            ),
            ("PostgreSQL 17.2", None, None),
        ] {
            let pg = FakePg::new().version(version).start().await;
            let mut profile = fake_profile(pg.port());
            profile.options.pg_flavor = profile_engine;
            let session = PgSession::open_with(target(&profile), vanilla)
                .await
                .unwrap();
            assert_eq!(session.flavor(), engine, "{version} {profile_engine:?}");
            assert_eq!(session.capabilities().flavor, engine);
            session.close().await;
        }
    }

    /// An engine's hooks decide the default database, the dialect, the session init, the catalog
    /// reads, the admin calls and the caps.
    #[tokio::test]
    async fn the_engines_hooks_drive_the_session() {
        let pg = FakePg::new()
            .server_version("8.0.2")
            .version(REDSHIFT)
            .error(
                "IntervalStyle",
                "42704",
                "unrecognized configuration parameter",
            )
            .error(
                "extra_float_digits",
                "42704",
                "unrecognized configuration parameter",
            )
            .start()
            .await;
        let mut profile = fake_profile(pg.port());
        profile.options.pg_flavor = Some(PgFlavor::Redshift);
        profile.options.timezone = Some("UTC".into());
        let session = PgSession::open_with(target(&profile), test_hooks)
            .await
            .unwrap();
        assert_eq!(session.dialect(), SqlDialect::Redshift);
        assert_eq!(session.flavor(), Some(Flavor::Redshift));
        let database = ("database".to_string(), "dev".to_string());
        assert!(pg.startups()[0].contains(&database), "{:?}", pg.startups());
        // The engine's init, one statement at a time.
        assert_eq!(
            pg.queries(),
            [
                "SELECT version()",
                "SET DateStyle = 'ISO, YMD';",
                "SET client_encoding = 'UTF8';",
                "SET TIME ZONE 'UTC';",
            ]
        );
        let schemas = session.list_schemas().await.unwrap();
        assert_eq!(schemas[0].name, "v80002");
        assert!(!pg.queries().iter().any(|q| q.contains("pg_namespace")));
        // The pool connection got the engine's init too.
        assert_eq!(pg.queries().len(), 7, "{:?}", pg.queries());
        assert!(session.list_processes().await.unwrap().unwrap().is_empty());
        let killed = session
            .kill_processes(&["7".into()], KillScope::Connection)
            .await
            .unwrap();
        assert!(killed[0].ok);
        let table = TableRef::new(Some("public"), "t");
        let ddl = session.table_ddl(&table).await.unwrap().unwrap();
        assert_eq!(ddl, "CREATE TABLE t");
        assert_eq!(
            session.explain_form(ExplainMode::Plan),
            Some(ExplainForm::Prefix("EXPLAIN VERBOSE"))
        );
        let caps = session.capabilities();
        assert!(caps.has(Feature::Users) && !caps.has(Feature::StructureEdit));
        session.close().await;
    }

    /// A transaction the server could not serialize runs again where the engine asks for that,
    /// and only there.
    #[tokio::test]
    async fn a_transaction_the_engine_could_not_serialize_runs_again() {
        fn retrying(_: Option<PgFlavor>) -> &'static dyn PgFlavorHooks {
            &TEST_HOOKS
        }
        for (pick, tx) in [
            (
                retrying as PickHooks,
                vec!["BEGIN", "UPDATE", "ROLLBACK", "BEGIN", "UPDATE", "COMMIT"],
            ),
            (vanilla as PickHooks, vec!["BEGIN", "UPDATE", "ROLLBACK"]),
        ] {
            let pg = FakePg::new()
                .error_times("UPDATE t", "40001", "restart transaction", Some(1))
                .command("UPDATE t", "UPDATE 1")
                .start()
                .await;
            let session = PgSession::open_with(target(&fake_profile(pg.port())), pick)
                .await
                .unwrap();
            let update = TxStatement {
                sql: "UPDATE t SET a = 1".into(),
                params: Vec::new(),
                expect_affected: Some(1),
            };
            let result = session.transaction(vec![update]).await;
            let sent: Vec<String> = pg
                .queries()
                .into_iter()
                .filter_map(|q| {
                    let word = q.split_whitespace().next().unwrap_or_default().to_string();
                    ["BEGIN", "UPDATE", "ROLLBACK", "COMMIT"]
                        .contains(&word.as_str())
                        .then_some(word)
                })
                .collect();
            assert_eq!(sent, tx);
            match tx.len() {
                3 => assert!(is_serialization_failure(&result.unwrap_err())),
                _ => assert_eq!(result.unwrap()[0].affected, 1),
            }
            session.close().await;
        }
    }

    /// A capped read in a transaction block sets the row-cap savepoint, except on an engine that
    /// has none: there a SAVEPOINT would abort the user's transaction, so the read is drained.
    #[tokio::test]
    async fn a_capped_read_in_a_transaction_sets_a_savepoint_only_where_the_engine_has_them() {
        fn no_savepoints(_: Option<PgFlavor>) -> &'static dyn PgFlavorHooks {
            &TEST_HOOKS
        }
        for (pick, savepoint) in [(vanilla as PickHooks, true), (no_savepoints, false)] {
            let pg = FakePg::new()
                .rows(
                    "FROM t",
                    vec![Col::int4("a")],
                    (0..20).map(|n| vec![Cell::Int(n)]).collect(),
                )
                .error("SAVEPOINT", "0A000", "SAVEPOINT is not supported")
                .start()
                .await;
            let session = PgSession::open_with(target(&fake_profile(pg.port())), pick)
                .await
                .unwrap();
            on_editor(&session, "BEGIN").await.unwrap();
            assert_eq!(session.in_transaction(), Some(true));
            let first = on_editor(&session, "SELECT a FROM t").await.unwrap();
            assert_eq!(first, Some(Value::Int(0)));
            on_editor(&session, "COMMIT").await.unwrap();
            let sent = pg.queries();
            assert_eq!(
                sent.iter().any(|q| q.contains("SAVEPOINT")),
                savepoint,
                "{sent:?}"
            );
            if !savepoint {
                // Nothing was cancelled on the server: the read ran to its end.
                assert!(pg.cancels().is_empty());
            }
            session.close().await;
        }
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
