//! The ClickHouse session: one HTTP request per statement. The SQL editor's run in an HTTP
//! session of their own (`session_id`), so its `SET`s and temporary tables last from one run to
//! the next; what the app runs for itself (the catalog, the grid, counts, edits) goes outside
//! it, so it never waits for the editor's lock on the server.
//!
//! Read-only is the server's: a read-only or unattended session sends `readonly=2` with every
//! request (`request::build`). The session also refuses, before sending, what the classifier
//! calls a write, so a write fails the same way on every path and the server's refusal is the
//! second wall, not the only one.
//!
//! Cancel stops the request (closing the connection stops a read on the server,
//! `cancel_http_readonly_queries_on_client_close`) and sends `KILL QUERY` for its query id from
//! outside the session, which stops a write too.

use std::future::Future;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::catalog::{self, ClickhouseServerInfo, MIN_VERSION, Process};
use super::client::{Client, RunOutcome, new_query_id};
use super::request::{ConnSettings, Credentials, Endpoint, HttpSession, Query};
use super::transport::Transport;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, DbSession, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, TxStatement, arm_cancel, cancel_portable, classify_statement,
    trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::clickhouse::{EditKind, edit_kind};
use crate::sqlgen::sql::{max_params, writes};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{FilterOp, TraceCategory, TracedSql};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::ServerExpr;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultColumn, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

const DIALECT: SqlDialect = SqlDialect::Clickhouse;

/// How often the app pings an idle session: `GET /ping` is free and needs no credentials.
const KEEPALIVE_SECS: u32 = 30;

/// How long a cancel waits for `KILL QUERY`: the run itself stops at once, so a server that does
/// not answer only leaves the query running there.
const KILL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the session offers beyond the structure view and the editor, each proven by the lane's
/// tests against the fake server (docs/drivers/clickhouse.md, Phase B status). Edits address
/// rows by primary key only, under a count of what the key matches (no `EditWithoutKey`); no
/// `Returning` (ClickHouse has none), no `ExplainAnalyze` (only recent servers have it, and the
/// classifier calls every one a write), and nothing that writes a core dialect's SQL.
pub const FEATURES: [Feature; 13] = [
    Feature::Explain,
    Feature::CancelOnServer,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ServerExpressions,
    Feature::ExactCount,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::ProcessList,
    Feature::KillProcess,
    Feature::SessionDetails,
    Feature::Mcp,
];

/// Every filter but JSON containment, which ClickHouse SQL has no operator for.
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

/// The clocks the dialect writes (`now()`, `now64(n)`, `today()`); DEFAULT leaves a new row's
/// column out (an UPDATE cannot set it: `default_keyword` is off).
const SERVER_EXPRS: [ServerExpr; 4] = [
    ServerExpr::Now,
    ServerExpr::CurrentTimestamp,
    ServerExpr::CurrentDate,
    ServerExpr::Default,
];

/// How a session reaches its server and what it says of itself.
pub struct SessionOptions {
    pub tls: bool,
    pub confirm: SafeConfirm,
    /// The editor's HTTP session: seconds idle before the server drops it (the server's default,
    /// 60 s, when None).
    pub session_timeout_secs: Option<u32>,
}

pub struct ClickhouseSession {
    client: Arc<Client>,
    info: ClickhouseServerInfo,
    tls: bool,
    /// Writes are refused before sending (and `readonly=2` goes with every request).
    read_only: bool,
    confirm: SafeConfirm,
    editor: HttpSession,
    editor_runs: tokio::sync::Mutex<()>,
    trace: SqlTrace,
    closed: AtomicBool,
}

/// Stops one request: the run stops reading (its connection closes) and `KILL QUERY` stops the
/// query on the server.
struct Kill {
    client: Arc<Client>,
    query_id: String,
    stop: CancellationToken,
}

impl CancelAction for Kill {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.stop.cancel();
        let client = self.client.clone();
        let id = self.query_id.clone();
        Box::pin(async move {
            tokio::time::timeout(KILL_TIMEOUT, client.kill(&id))
                .await
                .unwrap_or(Err(AppError::Timeout))
        })
    }

    fn label(&self) -> &'static str {
        "kill query"
    }
}

fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// The error of statement `index` of a commit, naming it whatever kind it is: the changes before
/// it were applied, and without an index the renderer would keep them staged and apply them
/// again on a retry (only `Db` errors carry one).
fn at_statement(e: AppError, index: usize) -> AppError {
    match e {
        e @ AppError::Db { .. } => e.with_statement_index(index),
        other => AppError::Db {
            message: other.to_string(),
            detail: None,
            hint: None,
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: Some(index),
        },
    }
}

/// Takes at most `max` rows of a result and stops the read past them.
struct Capped<'s> {
    sink: &'s mut dyn RowSink,
    max: u64,
    taken: u64,
    truncated: bool,
}

impl RowSink for Capped<'_> {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.sink.begin_set(columns);
    }

    fn rows(&mut self, mut rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        let room = usize::try_from(self.max.saturating_sub(self.taken)).unwrap_or(usize::MAX);
        if rows.len() > room {
            rows.truncate(room);
            self.truncated = true;
        }
        self.taken += rows.len() as u64;
        let flow = if rows.is_empty() {
            ControlFlow::Continue(())
        } else {
            self.sink.rows(rows)
        };
        if self.truncated {
            ControlFlow::Break(())
        } else {
            flow
        }
    }
}

/// A process of `system.processes` as the Running Queries list shows it. The query id is what
/// Kill takes; Tabler's own requests carry ids of its own (`new_query_id`).
fn process_row(p: Process, listing: &str) -> ProcessRow {
    ProcessRow {
        own: p.query_id.starts_with("tabler-"),
        current: p.query_id == listing,
        id: p.query_id,
        user: p.user,
        database: p.database,
        application: None,
        client: p.address,
        state: None,
        command: None,
        started_at: None,
        duration_ms: p.elapsed_secs.map(|s| (s * 1000.0) as i64),
        wait_event: None,
        query: p.query,
    }
}

impl ClickhouseSession {
    /// Reads who and where the session is, which also proves the credentials, and refuses a
    /// server older than the driver supports.
    pub async fn start(
        transport: Arc<dyn Transport>,
        endpoint: Endpoint,
        creds: Credentials,
        conn: ConnSettings,
        options: SessionOptions,
    ) -> Result<ClickhouseSession, AppError> {
        let read_only = conn.read_only;
        let client = Arc::new(Client::new(transport, endpoint, creds, conn));
        let trace = SqlTrace::default();
        let info = trace
            .run_async(
                TraceCategory::Structure,
                catalog::SERVER_INFO,
                |_: &ClickhouseServerInfo| TraceCounts::rows(1),
                client.server_info(),
            )
            .await?;
        // A version that does not parse proves nothing about the floor, so it is refused too.
        match info.release() {
            Some(v) if v >= MIN_VERSION => {}
            Some(_) => {
                return Err(AppError::Unsupported(format!(
                    "{} is older than ClickHouse {}.{}, the oldest Tabler supports",
                    info.label(),
                    MIN_VERSION.0,
                    MIN_VERSION.1
                )));
            }
            None => {
                return Err(AppError::Unsupported(format!(
                    "{} does not say which release it is; Tabler supports ClickHouse {}.{} and \
                     later",
                    info.label(),
                    MIN_VERSION.0,
                    MIN_VERSION.1
                )));
            }
        }
        Ok(ClickhouseSession {
            client,
            info,
            tls: options.tls,
            read_only,
            confirm: options.confirm,
            editor: HttpSession {
                id: new_query_id(),
                timeout_secs: options.session_timeout_secs,
            },
            editor_runs: tokio::sync::Mutex::new(()),
            trace,
            closed: AtomicBool::new(false),
        })
    }

    fn check_open(&self) -> Result<(), AppError> {
        if self.closed.load(Ordering::SeqCst) {
            Err(AppError::NotConnected)
        } else {
            Ok(())
        }
    }

    /// Refuses, on a read-only session, what the classifier calls a write (or does not know).
    fn refuse_write(&self, sql: &str) -> Result<(), AppError> {
        if self.read_only && writes(DIALECT, sql) {
            Err(read_only_error())
        } else {
            Ok(())
        }
    }

    /// `work` (which runs the request `query_id`), stopped through `cancel` when there is one.
    async fn cancellable<T>(
        &self,
        cancel: Option<&CancelSlot>,
        query_id: &str,
        work: impl Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        let Some(slot) = cancel else {
            return work.await;
        };
        let stop = CancellationToken::new();
        let _armed = arm_cancel(
            slot,
            CancelHandle::Custom(Arc::new(Kill {
                client: self.client.clone(),
                query_id: query_id.to_string(),
                stop: stop.clone(),
            })),
        )?;
        tokio::select! {
            biased;
            () = stop.cancelled() => Err(AppError::Cancelled),
            r = work => r,
        }
    }

    /// A catalog query with its bound names, traced as structure.
    async fn catalog(
        &self,
        sql: &str,
        named: &[(&str, &str)],
    ) -> Result<Vec<Vec<Value>>, AppError> {
        self.check_open()?;
        let shown: Vec<Value> = named
            .iter()
            .map(|(_, v)| Value::Text(v.to_string()))
            .collect();
        self.trace
            .run_async(
                TraceCategory::Structure,
                trace_sql(DIALECT, sql, &shown),
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                self.client.catalog(sql, named),
            )
            .await
    }

    /// A read the app runs (a page, a count, a cell), traced.
    async fn read(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        self.check_open()?;
        self.refuse_write(sql)?;
        let query_id = new_query_id();
        let q = Query {
            sql,
            params,
            query_id: Some(&query_id),
            ..Query::default()
        };
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                self.cancellable(cancel, &query_id, self.client.result_set(&q)),
            )
            .await
    }

    /// A write the app runs, traced; its count is what the server reports (`Client::execute`).
    async fn write(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        self.check_open()?;
        self.refuse_write(sql)?;
        // A write the app may cancel runs under a query id of its own, which `KILL QUERY` stops.
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                TraceCounts::of_exec,
                async {
                    match cancel {
                        Some(slot) => {
                            let query_id = new_query_id();
                            self.cancellable(
                                Some(slot),
                                &query_id,
                                self.client.execute_as(&query_id, sql, params),
                            )
                            .await
                        }
                        None => self.client.execute(sql, params, &[]).await,
                    }
                },
            )
            .await
    }

    /// Records a statement of a commit.
    fn record(
        &self,
        st: &TxStatement,
        started: std::time::Instant,
        outcome: &Result<ExecOutcome, AppError>,
    ) {
        let traced = trace_sql(DIALECT, &st.sql, &st.params);
        let (affected, error) = match outcome {
            Ok(o) => (Some(o.affected), None),
            Err(e) => (None, Some(traced.scrub(e.to_string()))),
        };
        self.trace.record(TracedSql {
            sql: traced.text,
            category: classify_statement(&st.sql, DIALECT),
            duration_ms: started.elapsed().as_millis() as u64,
            rows: None,
            affected,
            error,
        });
    }

    async fn kill_one(&self, id: &str, scope: KillScope) -> Result<(), AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        if scope == KillScope::Connection {
            return Err(AppError::Unsupported(
                "ClickHouse stops queries, not connections (each request is one): use Kill Query"
                    .into(),
            ));
        }
        self.check_open()?;
        self.trace
            .run_async(
                TraceCategory::Data,
                trace_sql(
                    DIALECT,
                    crate::sqlgen::dialect::clickhouse::KILL_QUERY,
                    &[Value::Text(id.to_string())],
                ),
                |_: &()| TraceCounts::NONE,
                self.client.kill(id),
            )
            .await
    }

    async fn processes(&self) -> Result<Vec<ProcessRow>, AppError> {
        self.check_open()?;
        let listing = new_query_id();
        let rows = self
            .trace
            .run_async(
                TraceCategory::Structure,
                catalog::PROCESSES,
                |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
                self.client.rows(&Query {
                    sql: catalog::PROCESSES,
                    query_id: Some(&listing),
                    ..Query::default()
                }),
            )
            .await?;
        Ok(catalog::parse_processes(&rows)
            .into_iter()
            .map(|p| process_row(p, &listing))
            .collect())
    }

    async fn table_info(&self, table: &TableRef) -> Result<Vec<Vec<Value>>, AppError> {
        let db = table.schema.as_deref().unwrap_or_default();
        self.catalog(catalog::TABLE_INFO, &[("db", db), ("t", &table.name)])
            .await
    }

    /// What an editor run reports of a statement once its rows are read.
    fn outcome(sql: &str, run: &RunOutcome, capped: &Capped<'_>) -> StatementOutcome {
        let mut outcome = StatementOutcome {
            row_count: capped.taken,
            truncated: capped.truncated,
            ..StatementOutcome::default()
        };
        if run.result_set {
            outcome.kind = StatementKind::Rows;
            outcome.result_sets = 1;
        } else if edit_kind(sql) == EditKind::Insert {
            // `written_rows` counts what the table's materialized views wrote too.
            outcome.kind = StatementKind::Affected;
            outcome.affected = run.summary.map(|s| s.written_rows);
        }
        outcome
    }
}

#[async_trait]
impl DbSession for ClickhouseSession {
    fn driver(&self) -> Driver {
        Driver::Clickhouse
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.check_open()?;
        Ok(ServerInfo {
            version: self.info.label(),
            current_database: self.info.database.clone(),
            current_user: self.info.user.clone(),
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.check_open()?;
        self.client.ping().await
    }

    /// Nothing stays open between requests; the server drops the editor's HTTP session once it
    /// has been idle for its timeout.
    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// The server is the one database entry; its databases are the schemas.
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.check_open()?;
        Ok(vec![DatabaseInfo {
            name: self.client.endpoint.host.clone(),
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let rows = self.catalog(catalog::DATABASES, &[]).await?;
        Ok(catalog::parse_schemas(&rows))
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let rows = self.catalog(catalog::TABLES, &[("db", schema)]).await?;
        Ok(catalog::parse_tables(schema, &rows))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let db = table.schema.as_deref().unwrap_or_default();
        let named = [("db", db), ("t", table.name.as_str())];
        let info = self.table_info(table).await?;
        let columns = self.catalog(catalog::COLUMNS, &named).await?;
        let skipping = self.catalog(catalog::SKIPPING_INDEXES, &named).await?;
        // `system.projections` exists from 24.4.
        let projections = if self.info.at_least(24, 4) {
            self.catalog(catalog::PROJECTIONS, &named).await?
        } else {
            Vec::new()
        };
        catalog::parse_table(table, &info, &columns, &skipping, &projections)
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        Ok(catalog::view_definition(&self.table_info(table).await?))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let rows = self.catalog(catalog::COMPLETION, &[]).await?;
        Ok(catalog::parse_completion(
            self.info.database.as_deref(),
            &rows,
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

    /// ClickHouse has no transactions: the statements apply one by one, each under its guard
    /// (`Client::apply`), and the first failure stops the rest. The error names its statement,
    /// so the changes before it count as applied (`TxMode::Sequential`).
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        self.check_open()?;
        for (i, st) in statements.iter().enumerate() {
            self.refuse_write(&st.sql)
                .map_err(|e| e.with_statement_index(i))?;
        }
        let mut done = Vec::with_capacity(statements.len());
        for (i, st) in statements.iter().enumerate() {
            let started = std::time::Instant::now();
            let result = self.client.apply(st, i).await;
            self.record(st, started, &result);
            done.push(result.map_err(|e| at_statement(e, i))?);
        }
        Ok(done)
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        self.check_open()?;
        self.refuse_write(sql)?;
        let query_id = new_query_id();
        let mut capped = Capped {
            sink,
            max: max_rows as u64,
            taken: 0,
            truncated: false,
        };
        let q = Query {
            sql,
            query_id: Some(&query_id),
            session: Some(&self.editor),
            ..Query::default()
        };
        let run = self
            .cancellable(Some(&cancel), &query_id, self.client.run(&q, &mut capped))
            .await?;
        Ok(Self::outcome(sql, &run, &capped))
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.check_open()?;
        self.refuse_write(sql)?;
        let query_id = new_query_id();
        let q = Query {
            sql,
            params,
            query_id: Some(&query_id),
            ..Query::default()
        };
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rows: &u64| TraceCounts {
                    rows: Some(*rows),
                    affected: None,
                },
                async {
                    let run = self
                        .cancellable(Some(&cancel), &query_id, self.client.run(&q, sink))
                        .await?;
                    Ok(run.rows)
                },
            )
            .await
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    /// No transactions in v1.
    fn in_transaction(&self) -> Option<bool> {
        Some(false)
    }

    fn drain_trace(&self) -> Vec<TracedSql> {
        self.trace.drain()
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    /// The editor's HTTP session takes one request at a time (the server refuses a second one
    /// while the first runs).
    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.editor_runs)
    }

    fn capabilities(&self) -> SessionCaps {
        let mut caps = SessionCaps::base(Driver::Clickhouse, DIALECT, None).with(&FEATURES);
        caps.namespaces = NamespaceCaps {
            databases: false,
            schemas: SchemaMode::Schemas,
            default_schema: self.info.database.clone(),
            labels: NamespaceLabels {
                schema: "Database".into(),
                ..NamespaceLabels::default()
            },
        };
        caps.data = DataCaps {
            paging: Paging::Offset,
            // `system.tables.total_rows` stands for a big table until asked to count.
            count: CountMode::Estimate,
            sort: true,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            transactions: TxMode::Sequential,
            // Every edit is counted: inserts by the summary, the others by the guard's count.
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: SERVER_EXPRS.to_vec(),
            max_params: max_params(DIALECT) as u32,
        };
        caps.safety = SafetyCaps {
            read_only: ReadOnlyMode::PerRequest,
            hardened: self.client.hardened(),
            confirm: self.confirm,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(self.processes().await)
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

    /// The `CREATE` text the server keeps.
    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        Some(self.table_info(table).await.and_then(|info| {
            catalog::table_ddl(&info)
                .ok_or_else(|| AppError::NotFound(format!("the definition of {}", table.name)))
        }))
    }

    /// HTTPS or not; ClickHouse's text is UTF-8.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(self.check_open().map(|()| SessionDetails {
            tls: self.tls,
            tls_version: None,
            tls_cipher: None,
            encoding: Some("UTF-8".into()),
        }))
    }
}
