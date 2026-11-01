//! The Vertica session: the editor's own connection, whose `SET`s, temporary tables and
//! transactions last from one run to the next, and a few worker connections in autocommit for
//! what the app runs on its own (introspection, the grid, counts, edits). A running statement is
//! cancelled on the server with a CancelRequest naming its connection's backend key, sent over a
//! new socket (`Dial::cancel`); the server answers the statement with `57014`.
//!
//! Read-only is the server's: every connection of a read-only session (and of the MCP process)
//! runs `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY` after its login, so the server
//! refuses writes. On top of that the session refuses, before anything is sent, every statement
//! the classifier calls a write (which `SET SESSION CHARACTERISTICS … READ WRITE` is) and every
//! read that advances a sequence or waits (`NEXTVAL('s')`, `s.NEXTVAL`, `SLEEP`), on every path:
//! reads, writes, transactions, the editor, exports, kills and startup commands. Its editor
//! commits on its own even with Manual commit, since COMMIT and ROLLBACK are refused there too.
//!
//! The session sends one statement at a time: a simple query runs every statement of its text,
//! so a text that holds more than one (a grid's raw WHERE `1=1; DELETE …`) is refused.
//!
//! A connection whose caller's future was dropped mid-statement (a timeout around a ping) still
//! owes its replies: it reports itself broken (`Connection::is_broken`), so a worker is closed
//! instead of pooled and the editor counts as lost. A user's cancel is settled before the
//! connection runs anything else, so a late CancelRequest cannot stop the next statement.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::sync::Mutex;

use super::catalog::{self, SessionRow, VerticaServerInfo};
use super::conn::{self, Rows};
use super::connect::{Conn, Dial, TlsInfo};
use super::decode;
use super::editor;
use super::params::VerticaParams;
use super::wire::BackendKey;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::conn_options::effective_read_only;
use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelAction, CancelHandle, CancelSlot, ConnectTarget, DbSession, RowSink,
    ServerInfo, SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel,
    cancel_portable, check_affected, classify_statement, is_dml, registry, settle_cancels,
    trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::{SqlDialect, vertica};
use crate::sqlgen::sql::{calls_side_effect_function, is_single_statement, max_params, writes};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{FilterOp, TraceCategory, TracedSql};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::objects::{RoutineKind, RoutineRef, RoutineSummary};
use crate::types::profile::{Driver, PasswordMode};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

pub(super) const DIALECT: SqlDialect = SqlDialect::Vertica;

/// Idle worker connections kept for the next statement; more are closed when they come back.
const MAX_IDLE: usize = 4;

/// How often the app pings an idle session (the descriptor's 30 s; a `SELECT 1` costs nothing).
const KEEPALIVE_SECS: u32 = 30;

/// How long `close` waits for the editor's statement to stop and the connections to close.
const CLOSE_WITHIN: Duration = Duration::from_secs(2);

/// What the session offers, each proven by the driver's tests (docs/drivers/vertica.md, Phase B
/// status). Not `Returning` (Vertica has no RETURNING: the grid reads its page again), not
/// `EditWithoutKey` (no row id, and UPDATE and DELETE take no LIMIT), not `ServerExpressions`
/// (unproven without a server), not `ExplainAnalyze` (PROFILE runs the statement and reports
/// through notices and system tables, not as a plan).
pub const FEATURES: [Feature; 17] = [
    Feature::Editor,
    Feature::Explain,
    Feature::CancelOnServer,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ExactCount,
    Feature::StructureView,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::Routines,
    Feature::ProcessList,
    Feature::KillProcess,
    Feature::SessionDetails,
    Feature::Timezone,
    Feature::StartupCommands,
    Feature::Mcp,
];

/// Every filter but JSON containment, which Vertica SQL has no operator for.
const FILTER_OPS: [FilterOp; 23] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
    FilterOp::Between,
    FilterOp::NotBetween,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::ContainsCase,
    FilterOp::NotContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasSuffix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffixCase,
    FilterOp::Ilike,
];

/// What opening a routine shows when the server keeps no text of it.
const NO_DEFINITION: &str = "-- The definition is not available: Vertica keeps no text of UDx \
                             functions, and EXPORT_OBJECTS returned none for this routine.";

/// The schema of a name without one.
const DEFAULT_SCHEMA: &str = "public";

pub struct VerticaSession {
    pub(super) dial: Arc<Dial>,
    info: VerticaServerInfo,
    read_only: bool,
    confirm: SafeConfirm,
    pub(super) editor: Mutex<Option<Conn>>,
    /// The editor's backend key, which a cancel names.
    pub(super) editor_key: Option<BackendKey>,
    /// The editor's TLS, for Session Details.
    tls: Option<TlsInfo>,
    /// The editor's connection broke: the session is lost (its `SET`s and transaction with it),
    /// which `ping` reports so the app reconnects.
    pub(super) editor_lost: AtomicBool,
    /// A statement runs in the editor (`close` cancels it).
    pub(super) editor_busy: AtomicBool,
    editor_runs: Mutex<()>,
    idle: std::sync::Mutex<Vec<Conn>>,
    pub(super) tx: TxState,
    pub(super) trace: SqlTrace,
    closing: AtomicBool,
}

pub(super) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A worker connection taken from the pool; it goes back when dropped, unless it broke or was
/// given up.
pub(super) struct Worker<'a> {
    session: &'a VerticaSession,
    conn: Option<Conn>,
    /// Autocommit may be off (a transaction began and did not get back to autocommit): the
    /// connection is closed rather than handed to the next caller.
    autocommit_off: bool,
    /// A user's cancel may still be on its way (the caller was dropped while it settled): the
    /// next statement could be the one it stops, so the connection is closed.
    unsettled: bool,
}

impl Worker<'_> {
    pub(super) fn conn(&mut self) -> Result<&mut Conn, AppError> {
        self.conn.as_mut().ok_or(AppError::NotConnected)
    }

    fn key(&self) -> Option<BackendKey> {
        self.conn.as_ref().and_then(Conn::key)
    }

    /// Closes the connection instead of returning it (its state is unknown).
    fn give_up(&mut self) {
        self.conn.take();
    }

    /// Waits for the user's cancels of the run to settle; a connection one may still reach is
    /// given up, also when this future is dropped before they settle.
    async fn settle(&mut self, cancel: &CancelSlot) {
        self.unsettled = true;
        if settle_user_cancels(cancel).await {
            self.unsettled = false;
        } else {
            self.give_up();
        }
    }
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        // Broken also covers a statement whose future was dropped before its reply.
        if conn.is_broken()
            || conn.inner.in_transaction()
            || self.autocommit_off
            || self.unsettled
            || self.session.closing.load(Ordering::SeqCst)
        {
            return;
        }
        // Nobody reads a worker's notices.
        conn.inner.take_notices();
        let mut idle = lock(&self.session.idle);
        if idle.len() < MAX_IDLE {
            idle.push(conn);
        }
    }
}

/// Stops the statement of the connection `key` names: a CancelRequest over a new socket.
struct CancelRequest {
    dial: Arc<Dial>,
    key: BackendKey,
}

impl CancelAction for CancelRequest {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        let (dial, key) = (Arc::clone(&self.dial), self.key);
        Box::pin(async move { dial.cancel(key).await })
    }

    fn label(&self) -> &'static str {
        "cancel request"
    }
}

/// The handle that cancels what the connection `key` runs; a server that gave no key cannot be
/// asked (the statement then runs to its end).
pub(super) fn cancel_handle(dial: &Arc<Dial>, key: Option<BackendKey>) -> CancelHandle {
    match key {
        Some(key) => CancelHandle::Custom(Arc::new(CancelRequest {
            dial: Arc::clone(dial),
            key,
        })),
        None => CancelHandle::Requested,
    }
}

pub(super) fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// What a read-only session refuses: a statement the classifier calls a write (or does not
/// know), and a read that calls a side-effect function or advances a sequence in either form
/// (`NEXTVAL('s')`, `s.NEXTVAL`: a sequence moves on for good).
pub(super) fn refused_read_only(sql: &str) -> bool {
    writes(DIALECT, sql)
        || calls_side_effect_function(DIALECT, sql)
        || vertica::advances_sequence(sql)
}

/// Whether a session of `profile` reads only: a read-only profile, or the MCP process
/// (`unattended`), which reads only like one.
pub(super) fn read_only_of(
    profile: &crate::types::profile::ConnectionProfile,
    unattended: bool,
) -> bool {
    effective_read_only(profile) || unattended
}

/// Waits until the user's cancel requests of a run have reached the server, so none can stop
/// what the connection runs next. False: one is still on its way.
pub(super) async fn settle_user_cancels(cancel: &CancelSlot) -> bool {
    let settled = settle_cancels(cancel, CANCEL_SETTLE_LIMIT).await;
    if !settled {
        log::warn!("Vertica: a cancel request did not finish in time");
    }
    settled
}

/// Refuses a text of more than one statement: a simple query would run them all.
pub(super) fn refuse_several(sql: &str) -> Result<(), AppError> {
    if is_single_statement(DIALECT, sql) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "Vertica: one statement at a time (this text holds several)".into(),
        ))
    }
}

/// Refuses a read that would write: the grid's raw WHERE and an exported query are the user's
/// text, which Safe Mode does not see. Writes the app means to send take `execute`.
fn refuse_added_write(sql: &str) -> Result<(), AppError> {
    if writes(DIALECT, sql) {
        return Err(AppError::InvalidInput(
            "A filter or an exported query can only read rows: this one would write".into(),
        ));
    }
    Ok(())
}

/// The rows of a statement as the grid shows them.
pub(super) fn result_set(rows: Rows) -> ResultSet {
    ResultSet {
        columns: rows.fields.iter().map(decode::result_column).collect(),
        rows: rows.rows,
        truncated: false,
    }
}

/// What a statement the app ran reports: a DML statement its count (from the `OUTPUT` row or
/// the tag), anything else its rows.
fn exec_outcome(sql: &str, rows: Option<Rows>) -> ExecOutcome {
    let Some(rows) = rows else {
        return ExecOutcome::default();
    };
    if is_dml(sql) {
        let affected = conn::affected(&rows);
        return ExecOutcome {
            affected: affected.unwrap_or(0),
            last_insert_id: None,
            rows: None,
            affected_unknown: affected.is_none(),
        };
    }
    ExecOutcome {
        affected: 0,
        last_insert_id: None,
        rows: (!rows.fields.is_empty()).then(|| result_set(rows)),
        affected_unknown: false,
    }
}

fn names(values: &[&str]) -> Vec<Value> {
    values
        .iter()
        .map(|v| Value::Text((*v).to_string()))
        .collect()
}

/// A session row as the process list shows it.
fn process_row(s: SessionRow, database: Option<&str>) -> ProcessRow {
    let own = s.own();
    ProcessRow {
        state: Some(
            if s.statement.is_some() {
                "active"
            } else {
                "idle"
            }
            .into(),
        ),
        started_at: s.statement_start.clone().or_else(|| s.login.clone()),
        id: s.session_id,
        user: s.user,
        database: database.map(str::to_string),
        application: s.client_label,
        client: s.client_hostname,
        command: s.client_type,
        duration_ms: None,
        wait_event: None,
        query: s.statement,
        current: s.current,
        own,
    }
}

impl VerticaSession {
    /// Opens the session of a target: its connections reach the server through the SSH tunnel
    /// when there is one, and a Command password (an OAuth token) is run again when it expires.
    pub async fn open(target: &ConnectTarget<'_>) -> Result<VerticaSession, AppError> {
        let profile = target.profile;
        let params = VerticaParams::from_driver_params(profile.options.driver_params.as_ref())?;
        let read_only = read_only_of(profile, registry::unattended());
        let dial = Arc::new(Dial::new(target, &params, read_only)?);
        let confirm = if profile.options.password_mode() == PasswordMode::Command {
            SafeConfirm::Passcode
        } else {
            SafeConfirm::Password
        };
        // A read-only editor cannot COMMIT (refused as a write), so it commits on its own: a
        // transaction its first SELECT opened would hold its locks until a reconnect.
        let autocommit = !params.manual_commit || read_only;
        let mut editor = dial.open(autocommit).await?;
        let trace = SqlTrace::default();
        let rows = trace
            .run_async(
                TraceCategory::Structure,
                catalog::SERVER_INFO,
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                editor.inner.rows(catalog::SERVER_INFO, &[]),
            )
            .await?;
        let info = catalog::parse_server_info(&rows)?;
        Ok(VerticaSession {
            info,
            read_only,
            confirm,
            editor_key: editor.key(),
            tls: editor.tls.clone(),
            editor: Mutex::new(Some(editor)),
            editor_lost: AtomicBool::new(false),
            editor_busy: AtomicBool::new(false),
            editor_runs: Mutex::new(()),
            idle: std::sync::Mutex::new(Vec::new()),
            tx: TxState::new(Some(false)),
            trace,
            closing: AtomicBool::new(false),
            dial,
        })
    }

    /// An idle worker connection, or a new one.
    pub(super) async fn worker(&self) -> Result<Worker<'_>, AppError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        loop {
            let next = lock(&self.idle).pop();
            match next {
                Some(conn) if !conn.is_broken() => {
                    return Ok(Worker {
                        session: self,
                        conn: Some(conn),
                        autocommit_off: false,
                        unsettled: false,
                    });
                }
                Some(_) => continue,
                None => break,
            }
        }
        let conn = self.dial.open(true).await?;
        Ok(Worker {
            session: self,
            conn: Some(conn),
            autocommit_off: false,
            unsettled: false,
        })
    }

    /// Refuses, on a read-only session, what `refused_read_only` names.
    pub(super) fn refuse_write(&self, sql: &str) -> Result<(), AppError> {
        if self.read_only && refused_read_only(sql) {
            Err(read_only_error())
        } else {
            Ok(())
        }
    }

    /// Every statement's rows of `sql` on a worker, cancellable through `cancel`.
    async fn run_on_worker(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<Vec<Rows>, AppError> {
        let mut worker = self.worker().await?;
        let armed = match cancel {
            Some(slot) => Some(arm_cancel(slot, cancel_handle(&self.dial, worker.key()))?),
            None => None,
        };
        let result = worker.conn()?.inner.query(sql, params).await;
        drop(armed);
        if let Some(slot) = cancel {
            worker.settle(slot).await;
        }
        result
    }

    /// A read the app runs, traced. What reaches it is built by the app (the grid's pages and
    /// counts) and reads, but the grid's raw WHERE is the user's text, so it is refused when it
    /// writes or adds a statement.
    async fn read(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        refuse_several(sql)?;
        self.refuse_write(sql)?;
        refuse_added_write(sql)?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                async {
                    let sets = self.run_on_worker(sql, params, cancel).await?;
                    Ok(sets.into_iter().next().map(result_set).unwrap_or_default())
                },
            )
            .await
    }

    /// A write the app runs, traced, in autocommit.
    async fn write(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        refuse_several(sql)?;
        self.refuse_write(sql)?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                TraceCounts::of_exec,
                async {
                    let sets = self.run_on_worker(sql, params, cancel).await?;
                    Ok(exec_outcome(sql, sets.into_iter().next()))
                },
            )
            .await
    }

    /// A catalog query's rows, traced as structure.
    async fn catalog(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, AppError> {
        self.trace
            .run_async(
                TraceCategory::Structure,
                trace_sql(DIALECT, sql, params),
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                async {
                    let mut worker = self.worker().await?;
                    worker.conn()?.inner.rows(sql, params).await
                },
            )
            .await
    }

    /// Records a statement the app ran inside a transaction.
    fn record(&self, st: &TxStatement, elapsed: Duration, outcome: Result<u64, &AppError>) {
        let traced = trace_sql(DIALECT, &st.sql, &st.params);
        let (affected, error) = match outcome {
            Ok(n) => (Some(n), None),
            Err(e) => (None, Some(traced.scrub(e.to_string()))),
        };
        self.trace.record(TracedSql {
            sql: traced.text,
            category: classify_statement(&st.sql, DIALECT),
            duration_ms: elapsed.as_millis() as u64,
            rows: None,
            affected,
            error,
        });
    }

    /// The statements of a transaction on `conn`, in autocommit off: each counted and checked
    /// against `expect_affected`; the first failure stops it. Returns what ran and the failure.
    async fn run_transaction(
        &self,
        conn: &mut Conn,
        statements: &[TxStatement],
    ) -> (Vec<ExecOutcome>, Option<AppError>) {
        let mut done = Vec::with_capacity(statements.len());
        for (index, st) in statements.iter().enumerate() {
            let started = std::time::Instant::now();
            let result = conn
                .inner
                .query(&st.sql, &st.params)
                .await
                .map(|sets| exec_outcome(&st.sql, sets.into_iter().next()))
                .and_then(|outcome| {
                    check_affected(st, &outcome, index)?;
                    Ok(outcome)
                })
                // The index within the transaction, not within the statement's own text.
                .map_err(|e| e.with_statement_index(index));
            match result {
                Ok(outcome) => {
                    self.record(st, started.elapsed(), Ok(outcome.affected));
                    done.push(outcome);
                }
                Err(e) => {
                    self.record(st, started.elapsed(), Err(&e));
                    return (done, Some(e));
                }
            }
        }
        (done, None)
    }

    async fn simple(conn: &mut Conn, sql: &str) -> Result<(), AppError> {
        conn.inner
            .simple(sql, &mut |_| conn::Flow::Continue)
            .await
            .map(|_| ())
    }

    fn schema_of<'a>(&'a self, table: &'a TableRef) -> &'a str {
        table
            .schema
            .as_deref()
            .or(self.info.schema.as_deref())
            .unwrap_or(DEFAULT_SCHEMA)
    }

    async fn meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let schema = self.schema_of(table).to_string();
        let name = table.name.as_str();
        let four = names(&[&schema, name, &schema, name]);
        let two = names(&[&schema, name]);
        let summary = catalog::parse_tables(&self.catalog(catalog::TABLE, &four).await?, &schema)
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound(format!("{schema}.{name}")))?;
        let mut columns = catalog::parse_columns(&self.catalog(catalog::COLUMNS, &four).await?);
        let comments = self
            .catalog(catalog::COLUMN_COMMENTS, &names(&[&schema]))
            .await?;
        catalog::apply_column_comments(&mut columns, name, &comments);
        let mut meta = TableMeta {
            table: TableRef {
                schema: Some(schema.clone()),
                name: name.to_string(),
            },
            kind: summary.kind,
            columns,
            primary_key: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            triggers: Vec::new(),
            comment: summary.comment,
            row_estimate: None,
            ddl: None,
            checks: Vec::new(),
            options: None,
        };
        if meta.kind == TableKind::View {
            return Ok(meta);
        }
        if let Some(key) =
            catalog::parse_primary_key(&self.catalog(catalog::PRIMARY_KEY, &two).await?)
        {
            catalog::apply_primary_key(&mut meta.columns, &key);
            meta.primary_key = key.columns;
        }
        meta.foreign_keys =
            catalog::parse_foreign_keys(&self.catalog(catalog::FOREIGN_KEYS, &two).await?);
        let (unique, checks) =
            catalog::parse_constraints(&self.catalog(catalog::CONSTRAINTS, &two).await?);
        meta.checks = checks;
        meta.indexes = unique;
        meta.indexes.extend(catalog::parse_projections(
            &self.catalog(catalog::PROJECTIONS, &two).await?,
        ));
        // Sizes need the storage tables, which a user may not read: the table stays without.
        match self.catalog(catalog::TABLE_SIZES, &names(&[&schema])).await {
            Ok(rows) => {
                let mut one = [TableSummary {
                    name: name.to_string(),
                    schema: Some(schema.clone()),
                    kind: meta.kind,
                    row_estimate: None,
                    size_bytes: None,
                    comment: None,
                }];
                catalog::apply_sizes(&mut one, &rows);
                meta.row_estimate = one[0].row_estimate;
            }
            Err(e) => log::debug!("Vertica: table sizes of {schema}: {e}"),
        }
        Ok(meta)
    }

    async fn ddl(&self, table: &TableRef) -> Result<String, AppError> {
        let object = TableRef {
            schema: Some(self.schema_of(table).to_string()),
            name: table.name.clone(),
        };
        let rows = self.catalog(&vertica::export_objects(&object), &[]).await?;
        catalog::parse_ddl(&rows).ok_or_else(|| {
            AppError::NotFound(format!("the DDL of {}", vertica::table_name(&object)))
        })
    }

    async fn routine_text(&self, routine: &RoutineRef) -> Result<String, AppError> {
        let schema = routine
            .schema
            .as_deref()
            .or(self.info.schema.as_deref())
            .unwrap_or(DEFAULT_SCHEMA)
            .to_string();
        if routine.kind != RoutineKind::Procedure {
            let rows = self
                .catalog(
                    catalog::FUNCTION_DEFINITION,
                    &names(&[&schema, &routine.name]),
                )
                .await?;
            if let Some(text) = catalog::parse_function_definition(&rows) {
                return Ok(text);
            }
        }
        let object = TableRef {
            schema: Some(schema),
            name: routine.name.clone(),
        };
        match self.catalog(&vertica::export_objects(&object), &[]).await {
            Ok(rows) => Ok(catalog::parse_ddl(&rows).unwrap_or_else(|| NO_DEFINITION.into())),
            Err(AppError::Db { .. }) => Ok(NO_DEFINITION.into()),
            Err(e) => Err(e),
        }
    }

    async fn sessions(&self) -> Result<Vec<SessionRow>, AppError> {
        let rows = self.catalog(catalog::SESSIONS, &[]).await?;
        Ok(catalog::parse_sessions(
            &rows,
            self.info.session_id.as_deref(),
        ))
    }

    async fn kill_one(&self, id: &str, scope: KillScope) -> Result<(), AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        let sql = match scope {
            KillScope::Connection => vertica::close_session(id),
            KillScope::Query => {
                let statement = self
                    .sessions()
                    .await?
                    .into_iter()
                    .find(|s| s.session_id == id)
                    .ok_or_else(|| AppError::NotFound(format!("session {id}")))?
                    .statement_id
                    .ok_or_else(|| {
                        AppError::InvalidInput(format!("session {id} runs no statement"))
                    })?;
                vertica::interrupt_statement(id, statement)
            }
        };
        self.trace
            .run_async(
                classify_statement(&sql, DIALECT),
                sql.as_str(),
                |_: &()| TraceCounts::NONE,
                async {
                    let mut worker = self.worker().await?;
                    worker.conn()?.inner.query(&sql, &[]).await.map(|_| ())
                },
            )
            .await
    }
}

#[async_trait]
impl DbSession for VerticaSession {
    fn driver(&self) -> Driver {
        Driver::Vertica
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let rows = self.catalog(catalog::SERVER_INFO, &[]).await?;
        let info = catalog::parse_server_info(&rows)?;
        Ok(ServerInfo {
            version: info.label(),
            current_database: info.database,
            current_user: info.user,
        })
    }

    /// A `SELECT 1` on a worker; a broken editor is reported, as its loss ends the session.
    async fn ping(&self) -> Result<(), AppError> {
        // An editor statement whose future was dropped left its connection owing replies.
        if let Ok(mut editor) = self.editor.try_lock()
            && editor.as_ref().is_some_and(Conn::is_broken)
        {
            editor::lose_editor(self, &mut editor);
        }
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let mut worker = self.worker().await?;
        worker
            .conn()?
            .inner
            .query("SELECT 1", &[])
            .await
            .map(|_| ())
    }

    /// Cancels what the editor runs, then closes every connection, all within about 2 s.
    async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let idle: Vec<Conn> = lock(&self.idle).drain(..).collect();
        let all = async {
            if self.editor_busy.load(Ordering::SeqCst)
                && let Some(key) = self.editor_key
                && let Err(e) = self.dial.cancel(key).await
            {
                log::debug!("Vertica: cancelling the editor's statement at close: {e}");
            }
            let workers = futures::future::join_all(idle.into_iter().map(|mut c| async move {
                c.inner.terminate().await;
            }));
            let editor = async {
                if let Some(mut conn) = self.editor.lock().await.take() {
                    conn.inner.terminate().await;
                }
            };
            futures::future::join(workers, editor).await;
        };
        if tokio::time::timeout(CLOSE_WITHIN, all).await.is_err() {
            // The connections left close with their last handle.
            log::warn!("Vertica: the session did not close within 2 s");
        }
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let rows = self.catalog(catalog::DATABASES, &[]).await?;
        Ok(catalog::parse_databases(
            &rows,
            self.info.database.as_deref(),
        ))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let rows = self.catalog(catalog::SCHEMAS, &[]).await?;
        Ok(catalog::parse_schemas(&rows, self.info.schema.as_deref()))
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let rows = self
            .catalog(catalog::TABLES, &names(&[schema, schema]))
            .await?;
        let mut tables = catalog::parse_tables(&rows, schema);
        match self.catalog(catalog::TABLE_SIZES, &names(&[schema])).await {
            Ok(sizes) => catalog::apply_sizes(&mut tables, &sizes),
            Err(e) => log::debug!("Vertica: table sizes of {schema}: {e}"),
        }
        Ok(tables)
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.meta(table).await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let rows = self
            .catalog(
                catalog::VIEW_DEFINITION,
                &names(&[self.schema_of(table), &table.name]),
            )
            .await?;
        Ok(catalog::parse_view_definition(&rows))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let rows = self.catalog(catalog::COMPLETION, &[]).await?;
        Ok(catalog::parse_completion(
            &rows,
            self.info.schema.as_deref(),
        ))
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.read(sql, params, None).await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        self.read(sql, params, Some(&cancel)).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.write(sql, params, None).await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        self.write(sql, params, Some(&cancel)).await
    }

    /// One transaction on a worker with autocommit off: each statement counted and checked
    /// against `expect_affected` before the COMMIT, everything rolled back at the first failure.
    /// A worker whose rollback or return to autocommit fails is closed, not reused.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        for st in &statements {
            refuse_several(&st.sql)?;
            self.refuse_write(&st.sql)?;
        }
        let mut worker = self.worker().await?;
        // Set before the SET is sent: a failed SET, or a future dropped anywhere before autocommit
        // is back on, closes the worker instead of pooling it with a transaction open.
        worker.autocommit_off = true;
        let conn = worker.conn()?;
        VerticaSession::simple(conn, vertica::AUTOCOMMIT_OFF).await?;
        let (done, failure) = self.run_transaction(conn, &statements).await;
        let end = if failure.is_none() {
            "COMMIT"
        } else {
            "ROLLBACK"
        };
        let ended = VerticaSession::simple(conn, end).await;
        let back = VerticaSession::simple(conn, vertica::AUTOCOMMIT_ON).await;
        if ended.is_ok() && back.is_ok() {
            worker.autocommit_off = false;
        }
        if let Some(e) = failure {
            return Err(e);
        }
        ended?;
        Ok(done)
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        editor::run(self, sql, max_rows, sink, cancel).await
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        refuse_several(sql)?;
        self.refuse_write(sql)?;
        refuse_added_write(sql)?;
        let mut worker = self.worker().await?;
        let key = worker.key();
        let armed = arm_cancel(&cancel, cancel_handle(&self.dial, key))?;
        let conn = worker.conn()?;
        let result = self
            .trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rows: &u64| TraceCounts {
                    rows: Some(*rows),
                    affected: None,
                },
                editor::stream_rows(&self.dial, key, conn, sql, params, sink, &cancel),
            )
            .await;
        drop(armed);
        worker.settle(&cancel).await;
        result
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx.get()
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

    fn capabilities(&self) -> SessionCaps {
        let mut caps = SessionCaps::base(Driver::Vertica, DIALECT, None).with(&FEATURES);
        caps.namespaces = NamespaceCaps {
            // One database per cluster: listed, not switched.
            databases: true,
            schemas: SchemaMode::Schemas,
            default_schema: Some(
                self.info
                    .schema
                    .clone()
                    .unwrap_or_else(|| DEFAULT_SCHEMA.into()),
            ),
            labels: NamespaceLabels::default(),
        };
        caps.data = DataCaps {
            paging: Paging::Offset,
            // The super projections' row counts stand for big tables until asked to count.
            count: CountMode::Estimate,
            sort: true,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            transactions: TxMode::Atomic,
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: Vec::new(),
            max_params: max_params(DIALECT) as u32,
        };
        caps.safety = SafetyCaps {
            read_only: ReadOnlyMode::Server,
            // Vertica has no session switch that keeps SQL off the file system and other servers
            // (COPY, EXPORT TO and the libraries' functions are the server's to allow).
            hardened: false,
            confirm: self.confirm,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(self.sessions().await.map(|rows| {
            rows.into_iter()
                .map(|s| process_row(s, self.info.database.as_deref()))
                .collect()
        }))
    }

    async fn kill_processes(&self, ids: &[String], scope: KillScope) -> Option<Vec<KillOutcome>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(match self.kill_one(id, scope).await {
                Ok(()) => KillOutcome {
                    id: id.clone(),
                    ok: true,
                    message: None,
                },
                Err(e) => KillOutcome {
                    id: id.clone(),
                    ok: false,
                    message: Some(e.to_string()),
                },
            });
        }
        Some(out)
    }

    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        Some(self.ddl(table).await)
    }

    async fn list_routines(&self, schema: &str) -> Option<Result<Vec<RoutineSummary>, AppError>> {
        Some(
            self.catalog(catalog::ROUTINES, &names(&[schema, schema]))
                .await
                .map(|rows| catalog::parse_routines(&rows, schema)),
        )
    }

    async fn routine_definition(&self, routine: &RoutineRef) -> Option<Result<String, AppError>> {
        Some(self.routine_text(routine).await)
    }

    /// The editor's TLS as rustls negotiated it; text travels as UTF-8.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(Ok(SessionDetails {
            tls: self.tls.is_some(),
            tls_version: self.tls.as_ref().and_then(|t| t.version.clone()),
            tls_cipher: self.tls.as_ref().and_then(|t| t.cipher.clone()),
            encoding: Some("UTF-8".into()),
        }))
    }
}
