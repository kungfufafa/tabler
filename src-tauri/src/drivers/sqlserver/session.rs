//! The SQL Server session: the editor's own connection, whose `USE`, `SET` options, temporary
//! tables and transactions last from one run to the next, and a few worker connections for what
//! the app runs on its own (introspection, the grid, counts, edits). Each connection is an actor
//! (`conn`), so a run is cancelled on the server with a TDS Attention: the run's `CancelSlot`
//! holds the statement's `Stop`.
//!
//! Read-only is Tabler's classifier alone (`ApplicationIntent=ReadOnly` only routes to a
//! replica). Besides the commands' own checks, the session refuses every statement the T-SQL
//! classifier calls a write or has no word for (`tsql::hides_statement`), and every read that
//! calls a side-effect function (`NEXT VALUE FOR`), before anything is sent, on every path:
//! reads, writes, transactions, the editor, exports, kills and the startup commands. The one
//! exception is the plan of a lone DML statement that reaches no other server, which runs nothing
//! (`tsql::plan_writes_nothing`) once the server has shown that plan output is on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ::tiberius::ColumnData;
use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::sync::Mutex;

use super::conn::{Conn, Mode, Stop, TxRequest};
use super::connect::{self, Dial, Endpoint};
use super::decode::{Bytes, to_param};
use super::editor;
use super::introspect::{self as catalog, MssqlServerInfo, TableParts};
use super::params::MssqlParams;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::conn_options::effective_read_only;
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, ConnectTarget, DbSession, RowSink, ServerInfo,
    SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel, cancel_portable,
    classify_statement, is_dml, trace_sql,
};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::spec::ExplainForm;
use crate::sqlgen::dialect::tsql;
use crate::sqlgen::sql::{calls_side_effect_function, max_params, writes};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{FilterOp, TraceCategory, TracedSql};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::ServerExpr;
use crate::types::objects::{RoutineRef, RoutineSummary};
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};
use crate::types::query::{ExecOutcome, ExplainMode, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

/// Opens one logged-in, set-up connection of the session.
pub type Connect = Arc<dyn Fn() -> BoxFuture<'static, Result<Conn, AppError>> + Send + Sync>;

pub(super) const DIALECT: SqlDialect = SqlDialect::Tsql;

/// Idle worker connections kept for the next statement; more are closed when they come back.
const MAX_IDLE: usize = 4;

/// How often the app pings an idle session (a `SELECT 1` on a worker costs nothing).
const KEEPALIVE_SECS: u32 = 30;

/// How long `close` waits for the editor's statement to stop and the connections to close.
const CLOSE_WITHIN: Duration = Duration::from_secs(2);

/// What the session offers, each proven by the driver's tests (docs/drivers/sqlserver.md, Phase
/// B status). Not `Returning`: OUTPUT without INTO fails on a table with an enabled trigger, so
/// the grid reads its page again after a commit instead; not `EditWithoutKey`: the portable
/// builders address rows by primary key only.
const FEATURES: [Feature; 18] = [
    Feature::Editor,
    Feature::Explain,
    Feature::ExplainAnalyze,
    Feature::CancelOnServer,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ServerExpressions,
    Feature::ExactCount,
    Feature::StructureView,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::Routines,
    Feature::SwitchDatabase,
    Feature::ProcessList,
    Feature::KillProcess,
    Feature::SessionDetails,
    Feature::StartupCommands,
];

/// Every filter but JSON containment, which T-SQL has no operator for.
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

const SERVER_EXPRS: [ServerExpr; 6] = [
    ServerExpr::Now,
    ServerExpr::CurrentTimestamp,
    ServerExpr::CurrentDate,
    ServerExpr::CurrentTime,
    ServerExpr::LocalTimestamp,
    ServerExpr::Default,
];

/// What opening a routine shows when the server keeps no text of it.
const NO_DEFINITION: &str =
    "-- The definition is not available: the object was created WITH ENCRYPTION or is a CLR one.";

pub struct MssqlSession {
    connect: Connect,
    info: MssqlServerInfo,
    read_only: bool,
    confirm: SafeConfirm,
    /// The program name the connections log in with, which marks Tabler's own sessions.
    application: String,
    pub(super) editor: Mutex<Option<Conn>>,
    /// The editor's connection broke: the session is lost (its `USE`, options and transaction
    /// with it), which `ping` reports so the app reconnects.
    pub(super) editor_lost: AtomicBool,
    /// The Stop of the statement running in the editor, which `close` fires.
    pub(super) editor_stop: std::sync::Mutex<Option<Stop>>,
    editor_runs: Mutex<()>,
    idle: std::sync::Mutex<Vec<Conn>>,
    pub(super) tx: TxState,
    trace: SqlTrace,
    closing: AtomicBool,
}

pub(super) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A worker connection taken from the pool; it goes back when dropped, unless it broke.
struct Worker<'a> {
    session: &'a MssqlSession,
    conn: Conn,
}

impl std::ops::Deref for Worker<'_> {
    type Target = Conn;

    fn deref(&self) -> &Conn {
        &self.conn
    }
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        if self.conn.is_broken() || self.session.closing.load(Ordering::SeqCst) {
            return;
        }
        let mut idle = lock(&self.session.idle);
        if idle.len() < MAX_IDLE {
            idle.push(self.conn.clone());
        }
    }
}

/// Stops the statement its `Stop` belongs to: its connection sends the Attention.
struct Attention(Stop);

impl CancelAction for Attention {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.fire();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "attention"
    }
}

pub(super) fn cancel_handle(stop: &Stop) -> CancelHandle {
    CancelHandle::Custom(Arc::new(Attention(stop.clone())))
}

pub(super) fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// What a read-only session refuses: a statement the classifier calls a write (or does not
/// know) or one it has no word for (`tsql::hides_statement`), and a read that calls a
/// side-effect function. Safe Mode lets `NEXT VALUE FOR s` through as a read, but it advances
/// the sequence for good (a rollback keeps the step), where PostgreSQL's read-only transaction
/// refuses `nextval`.
fn refused_read_only(sql: &str) -> bool {
    adds_a_statement(sql) || calls_side_effect_function(DIALECT, sql)
}

/// Whether a text runs more than a read: what the classifier calls a write, or a statement it
/// has no word for. `sp_executesql` runs every statement of its text.
fn adds_a_statement(sql: &str) -> bool {
    writes(DIALECT, sql) || tsql::hides_statement(sql)
}

/// Refuses, on every session, a read that holds another statement. The grid's raw WHERE (in a
/// browse, a count or a table export) is the user's text, and so is a query export, which the
/// T-SQL splitter cuts only at `GO`: `1=1) DELETE FROM t WHERE (1=1` would run the DELETE
/// through `sp_executesql`, and Safe Mode sees neither a browse nor an export. Writes the app
/// means to send (metrics that write) take `execute_cancellable`.
fn refuse_added_statement(sql: &str) -> Result<(), AppError> {
    if adds_a_statement(sql) {
        return Err(AppError::InvalidInput(
            "A filter or an exported query can only read rows: this one would run another statement"
                .into(),
        ));
    }
    Ok(())
}

/// Bind parameters, typed as the decoder binds them.
fn binds(values: &[Value]) -> Result<Vec<ColumnData<'static>>, AppError> {
    values.iter().map(|v| to_param(v, None)).collect()
}

fn first(sets: Vec<ResultSet>) -> ResultSet {
    sets.into_iter().next().unwrap_or_default()
}

/// A name for `@P1` of the catalog queries.
fn named(name: String) -> [Value; 1] {
    [Value::Text(name)]
}

/// How much of a binary value a read keeps: the whole of the one cell `fetch_cell` asks for, the
/// grid's preview otherwise.
fn bytes_for(sql: &str) -> Bytes {
    if tsql::reads_one_cell(sql) {
        Bytes::Whole
    } else {
        Bytes::Preview
    }
}

impl MssqlSession {
    /// Opens the session of a target: its connections reach the server through the SSH tunnel
    /// when there is one, and a Command password (an Entra token) is run again when it expires.
    pub async fn open(target: &ConnectTarget<'_>) -> Result<MssqlSession, AppError> {
        let profile = target.profile;
        let params = MssqlParams::from_driver_params(profile.options.driver_params.as_ref())?;
        let ttl = Duration::from_secs(u64::from(Driver::Sqlserver.descriptor().command_ttl_secs));
        let dial = Arc::new(Dial {
            profile: profile.clone(),
            params: params.clone(),
            endpoint: Endpoint::of(target),
            timeout: connect::connect_timeout(profile),
            password: target.password.map(str::to_string),
            token: CommandPassword::for_profile_ttl(profile, target.password, ttl),
        });
        let connect: Connect = Arc::new(move || {
            let dial = Arc::clone(&dial);
            Box::pin(async move { dial.conn().await })
        });
        MssqlSession::start(profile, &params, connect).await
    }

    /// A session over the connections `connect` opens. The editor's connection opens first and
    /// says who and where the session is.
    pub async fn start(
        profile: &ConnectionProfile,
        params: &MssqlParams,
        connect: Connect,
    ) -> Result<MssqlSession, AppError> {
        let read_only = effective_read_only(profile);
        // The startup commands run on every connection, before any check of the session's.
        if read_only && connect::startup_sql(profile).is_some_and(refused_read_only) {
            return Err(AppError::InvalidInput(
                "SQL Server: this connection is read-only, and its startup commands write (or \
                 are not known to only read)"
                    .into(),
            ));
        }
        let editor = connect().await?;
        let trace = SqlTrace::default();
        let rows = trace
            .run_async(
                TraceCategory::Structure,
                catalog::SERVER_INFO,
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                async {
                    editor
                        .query(
                            catalog::SERVER_INFO,
                            Vec::new(),
                            Mode::Rpc,
                            Bytes::Preview,
                            Stop::new(),
                        )
                        .await
                        .map(|sets| first(sets).rows)
                },
            )
            .await?;
        let info = catalog::parse_server_info(&rows)?;
        let confirm = if profile.options.password_mode() == PasswordMode::Command {
            SafeConfirm::Passcode
        } else {
            SafeConfirm::Password
        };
        Ok(MssqlSession {
            connect,
            info,
            read_only,
            confirm,
            application: params.application_name().to_string(),
            editor: Mutex::new(Some(editor)),
            editor_lost: AtomicBool::new(false),
            editor_stop: std::sync::Mutex::new(None),
            editor_runs: Mutex::new(()),
            idle: std::sync::Mutex::new(Vec::new()),
            tx: TxState::default(),
            trace,
            closing: AtomicBool::new(false),
        })
    }

    /// An idle worker connection, or a new one.
    async fn worker(&self) -> Result<Worker<'_>, AppError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        loop {
            let next = lock(&self.idle).pop();
            match next {
                Some(conn) if !conn.is_broken() => {
                    return Ok(Worker {
                        session: self,
                        conn,
                    });
                }
                Some(_) => continue,
                None => break,
            }
        }
        let conn = (self.connect)().await?;
        Ok(Worker {
            session: self,
            conn,
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

    /// A read the app runs, traced. What reaches it is built by the app (the grid's pages and
    /// counts, Show DDL) and reads, but the grid's raw WHERE is the user's text, so it is
    /// refused when it adds a statement (`refuse_added_statement`).
    async fn read(&self, sql: &str, params: &[Value], stop: Stop) -> Result<ResultSet, AppError> {
        self.refuse_write(sql)?;
        refuse_added_statement(sql)?;
        let binds = binds(params)?;
        let worker = self.worker().await?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                async {
                    worker
                        .query(sql, binds, Mode::Rpc, bytes_for(sql), stop)
                        .await
                        .map(first)
                },
            )
            .await
    }

    /// A write the app runs, traced. A DML statement is counted by `@@ROWCOUNT`.
    async fn write(
        &self,
        sql: &str,
        params: &[Value],
        stop: Stop,
    ) -> Result<ExecOutcome, AppError> {
        self.refuse_write(sql)?;
        let binds = binds(params)?;
        let worker = self.worker().await?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                TraceCounts::of_exec,
                async {
                    if is_dml(sql) {
                        let done = worker.counted(sql, binds, stop).await?;
                        return Ok(ExecOutcome {
                            affected: done.affected,
                            last_insert_id: None,
                            rows: done.rows,
                            affected_unknown: false,
                        });
                    }
                    let sets = worker
                        .query(sql, binds, Mode::Rpc, Bytes::Whole, stop)
                        .await?;
                    Ok(ExecOutcome {
                        affected: 0,
                        last_insert_id: None,
                        rows: sets.into_iter().next(),
                        affected_unknown: false,
                    })
                },
            )
            .await
    }

    /// A catalog query's rows, traced as structure.
    async fn catalog(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, AppError> {
        let binds = binds(params)?;
        let worker = self.worker().await?;
        self.trace
            .run_async(
                TraceCategory::Structure,
                trace_sql(DIALECT, sql, params),
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                async {
                    worker
                        .query(sql, binds, Mode::Rpc, Bytes::Preview, Stop::new())
                        .await
                        .map(|sets| first(sets).rows)
                },
            )
            .await
    }

    /// Records a statement of a transaction, which ran inside one job of its connection.
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

    async fn kill_one(&self, id: &str, scope: KillScope) -> Result<(), AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        if scope == KillScope::Query {
            return Err(AppError::Unsupported(
                "SQL Server ends a whole session, not one of its queries: use Kill Connection"
                    .into(),
            ));
        }
        let spid: i64 = id
            .trim()
            .parse()
            .map_err(|_| AppError::InvalidInput(format!("{id} is not a session id")))?;
        let sql = tsql::kill(spid)?;
        let worker = self.worker().await?;
        self.trace
            .run_async(
                classify_statement(&sql, DIALECT),
                sql.as_str(),
                |_: &()| TraceCounts::NONE,
                async {
                    worker
                        .query(
                            sql.as_str(),
                            Vec::new(),
                            Mode::Batch,
                            Bytes::Preview,
                            Stop::new(),
                        )
                        .await
                        .map(|_| ())
                },
            )
            .await
    }

    async fn routines(&self, schema: &str) -> Result<Vec<RoutineSummary>, AppError> {
        let name = named(schema.to_string());
        let routines = self.catalog(catalog::ROUTINES, &name).await?;
        let parameters = self.catalog(catalog::ROUTINE_PARAMETERS, &name).await?;
        Ok(catalog::parse_routines(&routines, &parameters, schema))
    }

    async fn definition_of(
        &self,
        schema: Option<&str>,
        name: &str,
    ) -> Result<Option<String>, AppError> {
        let rows = self
            .catalog(catalog::DEFINITION, &named(tsql::object_name(schema, name)))
            .await?;
        Ok(catalog::parse_definition(&rows))
    }

    async fn ddl(&self, table: &TableRef) -> Result<String, AppError> {
        let meta = self.table_meta(table).await?;
        if meta.kind == TableKind::View {
            return Ok(self
                .definition_of(table.schema.as_deref(), &table.name)
                .await?
                .unwrap_or_else(|| NO_DEFINITION.into()));
        }
        Ok(tsql::create_table(&meta))
    }
}

#[async_trait]
impl DbSession for MssqlSession {
    fn driver(&self) -> Driver {
        Driver::Sqlserver
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
            current_user: info.login,
        })
    }

    /// A `SELECT 1` on a worker. The editor is not pinged: a statement there would reset the
    /// `@@ROWCOUNT` the user may read next; a broken editor is reported, as its loss ends the
    /// session.
    async fn ping(&self) -> Result<(), AppError> {
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let worker = self.worker().await?;
        worker
            .query(
                tsql::PING,
                Vec::new(),
                Mode::Batch,
                Bytes::Preview,
                Stop::new(),
            )
            .await
            .map(|_| ())
    }

    /// Stops what the editor runs, then closes every connection, all within about 2 s: the idle
    /// workers close while the editor's statement stops.
    async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(stop) = lock(&self.editor_stop).take() {
            stop.fire();
        }
        let idle: Vec<Conn> = lock(&self.idle).drain(..).collect();
        let editor = async {
            let conn = self.editor.lock().await.take();
            if let Some(conn) = conn {
                conn.close().await;
            }
        };
        let workers = futures::future::join_all(idle.iter().map(Conn::close));
        let all = futures::future::join(editor, workers);
        if tokio::time::timeout(CLOSE_WITHIN, all).await.is_err() {
            // The connections left close with their last handle.
            log::warn!("SQL Server: the session did not close within 2 s");
        }
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let rows = self.catalog(catalog::DATABASES, &[]).await?;
        Ok(catalog::parse_databases(&rows))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let rows = self.catalog(catalog::SCHEMAS, &[]).await?;
        Ok(catalog::parse_schemas(&rows))
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let rows = self
            .catalog(catalog::TABLES, &named(schema.to_string()))
            .await?;
        Ok(catalog::parse_tables(&rows, schema))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let object = named(tsql::object_name(table.schema.as_deref(), &table.name));
        let info = catalog::parse_table(&self.catalog(catalog::TABLE, &object).await?, table)?;
        let columns = catalog::parse_columns(&self.catalog(catalog::COLUMNS, &object).await?);
        let (indexes, primary_key) =
            catalog::parse_indexes(&self.catalog(catalog::INDEXES, &object).await?);
        let foreign_keys =
            catalog::parse_foreign_keys(&self.catalog(catalog::FOREIGN_KEYS, &object).await?);
        let checks = catalog::parse_checks(&self.catalog(catalog::CHECKS, &object).await?);
        let triggers = catalog::parse_triggers(&self.catalog(catalog::TRIGGERS, &object).await?);
        Ok(catalog::table_meta(
            table.clone(),
            TableParts {
                info,
                columns,
                indexes,
                primary_key,
                foreign_keys,
                checks,
                triggers,
            },
        ))
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        self.definition_of(table.schema.as_deref(), &table.name)
            .await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let rows = self.catalog(catalog::COMPLETION, &[]).await?;
        Ok(catalog::parse_completion(
            &rows,
            self.info.default_schema.clone(),
        ))
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.read(sql, params, Stop::new()).await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        let stop = Stop::new();
        let _armed = arm_cancel(&cancel, cancel_handle(&stop))?;
        self.read(sql, params, stop).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.write(sql, params, Stop::new()).await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        let stop = Stop::new();
        let _armed = arm_cancel(&cancel, cancel_handle(&stop))?;
        self.write(sql, params, stop).await
    }

    /// One transaction of the TDS transaction manager on a worker: each statement counted by
    /// `@@ROWCOUNT` and checked against `expect_affected` before the commit, everything rolled
    /// back at the first failure (inside the connection's job, so a caller that goes away
    /// cannot leave it open).
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        for st in &statements {
            self.refuse_write(&st.sql)?;
        }
        let requests = statements
            .iter()
            .map(|st| {
                Ok(TxRequest {
                    sql: st.sql.clone(),
                    params: binds(&st.params)?,
                    expect_affected: st.expect_affected,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let worker = self.worker().await?;
        let (done, failure) = match worker.transaction(requests, Stop::new()).await {
            Ok(done) => (done, None),
            Err(f) => (f.done, Some((f.error, f.elapsed))),
        };
        for (st, d) in statements.iter().zip(&done) {
            self.record(st, d.elapsed, Ok(d.affected));
        }
        if let Some((error, elapsed)) = failure {
            if let Some(st) = statements.get(done.len()) {
                self.record(st, elapsed, Err(&error));
            }
            return Err(error);
        }
        Ok(done
            .into_iter()
            .map(|d| ExecOutcome {
                affected: d.affected,
                last_insert_id: None,
                rows: d.rows,
                affected_unknown: false,
            })
            .collect())
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
        self.refuse_write(sql)?;
        refuse_added_statement(sql)?;
        let binds = binds(params)?;
        let stop = Stop::new();
        let _armed = arm_cancel(&cancel, cancel_handle(&stop))?;
        let worker = self.worker().await?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rows: &u64| TraceCounts {
                    rows: Some(*rows),
                    affected: None,
                },
                editor::stream_rows(&worker, sql, binds, sink, &stop, &cancel),
            )
            .await
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
        let mut caps = SessionCaps::base(Driver::Sqlserver, DIALECT, None).with(&FEATURES);
        caps.namespaces = NamespaceCaps {
            databases: true,
            schemas: SchemaMode::Schemas,
            default_schema: Some(
                self.info
                    .default_schema
                    .clone()
                    .unwrap_or_else(|| "dbo".into()),
            ),
            labels: NamespaceLabels::default(),
        };
        caps.data = DataCaps {
            paging: Paging::Offset,
            // Row counts of `sys.partitions` stand for big tables until asked to count.
            count: CountMode::Estimate,
            sort: true,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            transactions: TxMode::Atomic,
            affected: Affected::Exact,
            default_keyword: true,
            upsert: false,
            server_exprs: SERVER_EXPRS.to_vec(),
            max_params: max_params(DIALECT) as u32,
        };
        caps.safety = SafetyCaps {
            read_only: ReadOnlyMode::ClassifierOnly,
            // SQL Server has no session switch that keeps SQL off the file system and other
            // servers (xp_cmdshell, OPENROWSET and linked servers are server settings).
            hardened: false,
            confirm: self.confirm,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    /// `SHOWPLAN_XML` and `STATISTICS XML` must be set in batches of their own: the prefix is the
    /// driver's own, which the editor reads back (`tsql::explain_of`).
    fn explain_form(&self, mode: ExplainMode) -> Option<ExplainForm> {
        Some(ExplainForm::Prefix(tsql::explain_prefix(mode)))
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(
            self.catalog(catalog::PROCESSES, &[])
                .await
                .map(|rows| catalog::parse_processes(&rows, &self.application)),
        )
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
        Some(self.routines(schema).await)
    }

    async fn routine_definition(&self, routine: &RoutineRef) -> Option<Result<String, AppError>> {
        Some(
            self.definition_of(routine.schema.as_deref(), &routine.name)
                .await
                .map(|d| d.unwrap_or_else(|| NO_DEFINITION.into())),
        )
    }

    /// Whether the connection is encrypted; text travels as UTF-16 (nvarchar) in TDS. The TLS
    /// version and cipher are the client's, which tiberius does not report.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(
            self.catalog(catalog::SESSION_TLS, &[])
                .await
                .map(|rows| SessionDetails {
                    tls: catalog::parse_session_tls(&rows),
                    tls_version: None,
                    tls_cipher: None,
                    encoding: Some("UTF-16".into()),
                }),
        )
    }
}
