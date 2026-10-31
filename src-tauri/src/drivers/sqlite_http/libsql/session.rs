//! The LibSQL session: the app's own statements each on a stream of their own (closed in the
//! same request), the SQL editor on the stream it holds, so `BEGIN` and PRAGMAs last between its
//! statements. A read-only session refuses here, before sending, every statement the classifier
//! does not know is a read (`sqld` and Turso refuse no write for a session); where the server
//! takes `PRAGMA query_only = 1`, every batch also starts with it (the client's prelude).

use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::sqlite_http::catalog::{self, CatalogQuery, MAIN, Remote, Rows};
use crate::drivers::sqlite_http::session::{
    Closed, cancellable, cancellable_by, refuse_write, trace_transaction, traced, traced_catalog,
};
use crate::drivers::sqlite_http::value::inserts;
use crate::drivers::{
    CancelHandle, CancelSlot, DbSession, RowChunker, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, TxState, TxStatement, cancel_portable, is_dml, tx_effect,
};
use crate::error::AppError;
use crate::sqlgen::split::leading_keywords;
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::browse::TracedSql;
use crate::types::caps::{CountMode, Feature, SessionCaps};
use crate::types::driver::ReadOnlyMode;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

use super::client::{CursorEvent, HranaClient, StreamState, Version, single};
use super::decode::{columns, decode_row, exec_outcome, kinds_for, result_set};
use super::error::{STREAM_LOST_HINT, hrana_error};
use super::hrana::{Batch, BatchCond, BatchStep, Col, Stmt};

/// What a LibSQL session offers beyond `SessionCaps::base` (the structure view and the editor),
/// each proven by the lane's tests against the fake server. DDL editing, exports, imports and
/// backups use the core SQLite generators, which a driver beyond the core three may not reuse
/// (its descriptor's `core_reuse` is empty).
pub const FEATURES: [Feature; 15] = [
    Feature::Explain,
    Feature::Format,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    // A table without a key is edited by its rowid, under the batch's `changes()` guard.
    Feature::EditWithoutKey,
    Feature::Returning,
    Feature::ServerExpressions,
    Feature::ExactCount,
    Feature::CopyAsInsert,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::StartupCommands,
    Feature::SessionDetails,
    Feature::Mcp,
];

/// The session's caps: SQLite's grid and namespaces, `FEATURES`, and counts only on request
/// (Turso bills the rows a count reads). Read-only is the classifier's (`ClassifierOnly`), and
/// `PerRequest` once the server took `query_only`, which then starts every batch.
pub fn capabilities(query_only: bool) -> SessionCaps {
    let mut caps = SessionCaps::base(Driver::Libsql, SqlDialect::Sqlite, None).with(&FEATURES);
    caps.data.count = CountMode::Costly;
    caps.safety.read_only = if query_only {
        ReadOnlyMode::PerRequest
    } else {
        ReadOnlyMode::ClassifierOnly
    };
    caps
}

/// The step after the editor's statement that runs only outside a transaction: whether it ran
/// is the stream's transaction state afterwards (`get_autocommit` needs a pipeline).
const PROBE_STEP: u32 = 1;

/// How long `close` waits to close the editor's stream on the server.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

pub struct LibsqlSession {
    client: HranaClient,
    /// What the sidebar shows as the database: one per URL.
    database: String,
    /// `sqlite_version()` at connect.
    version: String,
    tls: bool,
    read_only: bool,
    editor: tokio::sync::Mutex<StreamState>,
    editor_runs: tokio::sync::Mutex<()>,
    tx: TxState,
    trace: SqlTrace,
    closed: Closed,
}

impl LibsqlSession {
    /// Probes the protocol and reads the server's SQLite version, so a refused token or a
    /// failing startup command fails the connect. A read-only session refuses startup commands
    /// that write, and asks once whether the server takes `query_only`.
    pub async fn connect(
        mut client: HranaClient,
        database: String,
        tls: bool,
        read_only: bool,
    ) -> Result<LibsqlSession, AppError> {
        for statement in client.startup() {
            refuse_write(read_only, statement).map_err(|_| {
                AppError::InvalidInput(format!("{READ_ONLY_MESSAGE}: a startup command writes"))
            })?;
        }
        if read_only {
            client.probe_query_only().await?;
        }
        let mut session = LibsqlSession {
            client,
            database,
            version: String::new(),
            tls,
            read_only,
            editor: tokio::sync::Mutex::new(StreamState::default()),
            editor_runs: tokio::sync::Mutex::new(()),
            tx: TxState::default(),
            trace: SqlTrace::default(),
            closed: Closed::default(),
        };
        let rows = session.catalog(&[catalog::version_query()]).await?;
        session.version = catalog::parse_version(catalog::first_set(&rows))?;
        Ok(session)
    }

    async fn catalog(&self, queries: &[CatalogQuery]) -> Result<Vec<Rows>, AppError> {
        self.closed.check()?;
        traced_catalog(&self.trace, queries, self.client.catalog(queries)).await
    }

    /// A query of the app's on a stream of its own; a cancelled one is logged too.
    async fn worker_query(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(
            &self.trace,
            sql,
            params,
            |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
            async {
                let r = cancellable_by(cancel, self.client.execute(sql, params)).await?;
                Ok::<_, AppError>(result_set(&r, None, None))
            },
        )
        .await
    }

    async fn worker_execute(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(&self.trace, sql, params, TraceCounts::of_exec, async {
            let r = cancellable_by(cancel, self.client.execute(sql, params)).await?;
            Ok::<_, AppError>(exec_outcome(sql, &r))
        })
        .await
    }

    /// One editor statement: through the cursor where the server has one (rows arrive as they
    /// are read, and the probe step tells the transaction state), else in one pipeline.
    /// `autocommit` gets what the probe found.
    async fn run_on_editor(
        &self,
        stream: &mut StreamState,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        autocommit: &mut Option<bool>,
    ) -> Result<StatementOutcome, AppError> {
        match self.client.version().await? {
            Version::V3 => {
                self.editor_cursor(stream, sql, max_rows, sink, autocommit)
                    .await
            }
            Version::V2 => {
                let r = self.client.execute_on(stream, sql, &[]).await?;
                let mut chunker = RowChunker::new(sink);
                let mut outcome = StatementOutcome::default();
                if !r.cols.is_empty() {
                    let kinds = begin(&mut chunker, &mut outcome, &r.cols);
                    for row in &r.rows {
                        if chunker.rows_in_set as usize >= max_rows
                            || chunker.push(decode_row(row, &kinds, false)).is_break()
                        {
                            outcome.truncated = true;
                            break;
                        }
                    }
                }
                finish(
                    &mut chunker,
                    &mut outcome,
                    sql,
                    r.affected_row_count,
                    r.last_insert_rowid,
                );
                Ok(outcome)
            }
        }
    }

    async fn editor_cursor(
        &self,
        stream: &mut StreamState,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        autocommit: &mut Option<bool>,
    ) -> Result<StatementOutcome, AppError> {
        // Outside a transaction the stream holds nothing worth reading a result to its end for.
        let disposable = self.tx.get() == Some(false);
        let work = Batch {
            steps: vec![
                BatchStep {
                    condition: None,
                    stmt: Stmt::new(sql, Vec::new()),
                },
                BatchStep {
                    condition: Some(BatchCond::IsAutocommit),
                    stmt: Stmt::no_rows("SELECT 1"),
                },
            ],
        };
        let mut cursor = self.client.cursor(Some(stream), work).await?;
        let mut chunker = RowChunker::new(sink);
        let mut outcome = StatementOutcome::default();
        let mut kinds = Vec::new();
        let mut step = None;
        let mut probed = false;
        let mut failed = None;
        let mut end = (0, None);
        loop {
            let event = match cursor.next_event().await {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(e) => {
                    let _ = chunker.flush();
                    return Err(e);
                }
            };
            match event {
                CursorEvent::Begin { step: 0, cols } => {
                    step = Some(0);
                    if !cols.is_empty() {
                        kinds = begin(&mut chunker, &mut outcome, &cols);
                    }
                }
                CursorEvent::Begin { step: s, .. } => {
                    step = Some(s);
                    probed |= s == PROBE_STEP;
                }
                CursorEvent::Row(row) if step == Some(0) && !outcome.truncated => {
                    if chunker.rows_in_set as usize >= max_rows
                        || chunker.push(decode_row(&row, &kinds, false)).is_break()
                    {
                        outcome.truncated = true;
                        if disposable {
                            // The rest is dropped with the request, and the stream with it.
                            drop(cursor);
                            stream.forget();
                            *autocommit = Some(true);
                            let _ = chunker.flush();
                            outcome.row_count = chunker.total_rows;
                            return Ok(outcome);
                        }
                    }
                }
                CursorEvent::End {
                    affected_row_count,
                    last_insert_rowid,
                } if step == Some(0) => end = (affected_row_count, last_insert_rowid),
                CursorEvent::StepError { step: 0, error } => failed = Some(hrana_error(&error)),
                _ => {}
            }
        }
        cursor.update(stream);
        *autocommit = Some(probed);
        if let Some(e) = failed {
            let _ = chunker.flush();
            return Err(e);
        }
        finish(&mut chunker, &mut outcome, sql, end.0, end.1);
        Ok(outcome)
    }

    /// A worker query's rows into `sink` until it says stop: through a cursor where the server
    /// has one (its stream closed afterwards), else in one pipeline.
    async fn stream_rows(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
    ) -> Result<u64, AppError> {
        let mut chunker = RowChunker::new(sink);
        if self.client.version().await? == Version::V2 {
            let r = self.client.execute(sql, params).await?;
            let kinds = kinds_for(&r.cols, None);
            let _ = chunker.begin_set(columns(&r.cols));
            for row in &r.rows {
                if chunker.push(decode_row(row, &kinds, false)).is_break() {
                    break;
                }
            }
            let _ = chunker.flush();
            return Ok(chunker.total_rows);
        }
        let mut stream = StreamState::default();
        let mut cursor = self
            .client
            .cursor(Some(&mut stream), single(sql, params)?)
            .await?;
        let mut kinds = Vec::new();
        loop {
            match cursor.next_event().await? {
                None => break,
                Some(CursorEvent::Begin { step: 0, cols }) => {
                    kinds = kinds_for(&cols, None);
                    let _ = chunker.begin_set(columns(&cols));
                }
                Some(CursorEvent::Row(row)) => {
                    if chunker.push(decode_row(&row, &kinds, false)).is_break() {
                        // The server expires the stream it leaves behind.
                        let _ = chunker.flush();
                        return Ok(chunker.total_rows);
                    }
                }
                Some(CursorEvent::StepError { error, .. }) => return Err(hrana_error(&error)),
                Some(_) => {}
            }
        }
        cursor.update(&mut stream);
        self.client.close(&mut stream).await;
        let _ = chunker.flush();
        Ok(chunker.total_rows)
    }
}

/// Starts the result set of `cols`; their kinds for decoding the rows.
fn begin(
    chunker: &mut RowChunker<'_>,
    outcome: &mut StatementOutcome,
    cols: &[Col],
) -> Vec<crate::types::schema::ValueKind> {
    let _ = chunker.begin_set(columns(cols));
    outcome.kind = StatementKind::Rows;
    outcome.result_sets = 1;
    kinds_for(cols, None)
}

/// Rows read and rows changed. Hrana reports SQLite's `changes()`, which only DML sets (a DDL
/// statement after an INSERT would report the INSERT's), so only DML, also behind a WITH, shows a
/// count.
fn finish(
    chunker: &mut RowChunker<'_>,
    outcome: &mut StatementOutcome,
    sql: &str,
    affected: u64,
    last_insert_rowid: Option<String>,
) {
    let _ = chunker.flush();
    outcome.row_count = chunker.total_rows;
    let dml = is_dml(sql)
        || leading_keywords(sql, 1)
            .first()
            .is_some_and(|k| k == "WITH" && affected > 0);
    if !dml {
        return;
    }
    if outcome.kind != StatementKind::Rows {
        outcome.kind = StatementKind::Affected;
    }
    outcome.affected = Some(affected);
    if affected > 0 && inserts(sql) {
        outcome.last_insert_id = last_insert_rowid;
    }
}

/// What a savepoint statement that ran did to the transaction state, where Hrana 2 cannot ask
/// (`None`: not one, or it failed and changed nothing). SQLite's `SAVEPOINT` opens a transaction
/// when none is open, and `ROLLBACK TO` keeps it open; whether a `RELEASE` ended it depends on
/// whether its savepoint started it, so the state becomes unknown and the stream is not reused
/// as idle.
fn savepoint_effect(sql: &str, succeeded: bool) -> Option<Option<bool>> {
    let kw = leading_keywords(sql, 3);
    let w = |i: usize| kw.get(i).map(String::as_str).unwrap_or("");
    let rollback_to = w(0) == "ROLLBACK" && (w(1) == "TO" || w(2) == "TO");
    match w(0) {
        _ if !succeeded => None,
        "SAVEPOINT" => Some(Some(true)),
        "RELEASE" => Some(None),
        _ if rollback_to => Some(Some(true)),
        _ => None,
    }
}

/// An error on a statement after the held stream was lost says so.
fn with_lost_hint(e: AppError) -> AppError {
    match e {
        AppError::Db {
            message,
            detail,
            hint: None,
            sqlstate,
            vendor_code,
            position,
            statement_index,
        } => AppError::Db {
            message,
            detail,
            hint: Some(STREAM_LOST_HINT.to_string()),
            sqlstate,
            vendor_code,
            position,
            statement_index,
        },
        other => other,
    }
}

#[async_trait]
impl DbSession for LibsqlSession {
    fn driver(&self) -> Driver {
        Driver::Libsql
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.closed.check()?;
        Ok(ServerInfo {
            version: format!("LibSQL (SQLite {})", self.version),
            current_database: Some(self.database.clone()),
            current_user: None,
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.closed.check()?;
        self.client.ping().await
    }

    /// Closes the editor's stream (ending its transaction now rather than when it expires),
    /// unless a statement still runs on it.
    async fn close(&self) {
        self.closed.close();
        if let Ok(mut stream) = self.editor.try_lock() {
            let _ = tokio::time::timeout(CLOSE_WAIT, self.client.close(&mut stream)).await;
        }
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.closed.check()?;
        Ok(vec![DatabaseInfo {
            name: self.database.clone(),
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.closed.check()?;
        Ok(catalog::schemas())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        if schema != MAIN {
            return Ok(Vec::new());
        }
        let rows = self
            .catalog(&[catalog::list_tables_query(Remote::Libsql)])
            .await?;
        catalog::parse_tables(catalog::first_set(&rows))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let queries = catalog::table_meta_queries(table);
        let rows = self.catalog(&queries).await?;
        catalog::parse_table_meta(table, &rows)
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let rows = self
            .catalog(&[catalog::view_definition_query(table)])
            .await?;
        catalog::parse_view_definition(catalog::first_set(&rows))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let rows = self
            .catalog(&[catalog::completion_query(Remote::Libsql)])
            .await?;
        catalog::parse_completion(catalog::first_set(&rows))
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.worker_query(sql, params, None).await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        self.worker_query(sql, params, Some(&cancel)).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.worker_execute(sql, params, None).await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        self.worker_execute(sql, params, Some(&cancel)).await
    }

    /// One batch: `BEGIN`, the statements each with its `changes()` guard, `COMMIT`, and
    /// `ROLLBACK` when the commit did not happen; the server runs it whole.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        self.closed.check()?;
        for (i, st) in statements.iter().enumerate() {
            refuse_write(self.read_only, &st.sql).map_err(|e| e.with_statement_index(i))?;
        }
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        let result = self.client.transaction(&statements).await;
        trace_transaction(&self.trace, &statements, started, &result);
        result
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        self.closed.check()?;
        // Explain plans come here too: the commands layer does not classify an EXPLAIN QUERY
        // PLAN, but SQLite applies a PRAGMA while preparing it.
        refuse_write(self.read_only, sql)?;
        let mut stream = self.editor.lock().await;
        let open_before = self.tx.get() != Some(false);
        let mut autocommit = None;
        let result = cancellable(
            &cancel,
            self.run_on_editor(&mut stream, sql, max_rows, sink, &mut autocommit),
        )
        .await;
        if matches!(result, Err(AppError::Cancelled)) {
            // The request was dropped: the stream goes at the next statement, which says what
            // became of a transaction.
            return result;
        }
        let lost = stream.take_lost() && open_before;
        let open = match (autocommit, savepoint_effect(sql, result.is_ok())) {
            (Some(auto), _) => Some(!auto),
            (None, Some(effect)) => effect,
            (None, None) => tx_effect(sql, Dialect::Sqlite, result.as_ref().err())
                .or(lost.then_some(false))
                .or(self.tx.get()),
        };
        self.tx.set(open);
        // What the next statement's stream is known to be (Hrana 2 cannot say itself).
        stream.autocommit = open.map(|open| !open);
        match result {
            Ok(mut outcome) => {
                if lost {
                    outcome.notices.push(STREAM_LOST_HINT.to_string());
                }
                Ok(outcome)
            }
            Err(e) if lost => Err(with_lost_hint(e)),
            Err(e) => Err(e),
        }
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(
            &self.trace,
            sql,
            params,
            |n: &u64| TraceCounts::rows(*n as usize),
            cancellable(&cancel, self.stream_rows(sql, params, sink)),
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

    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.editor_runs)
    }

    fn capabilities(&self) -> SessionCaps {
        capabilities(self.client.query_only())
    }

    /// The `CREATE` statements SQLite keeps for the table and its indexes and triggers.
    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        let queries = catalog::table_ddl_queries(table);
        Some(
            self.catalog(&queries)
                .await
                .and_then(|rows| catalog::parse_table_ddl(table, &rows)),
        )
    }

    /// HTTPS or not; the text is UTF-8, as SQLite's always is here.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(self.closed.check().map(|()| SessionDetails {
            tls: self.tls,
            tls_version: None,
            tls_cipher: None,
            encoding: Some("UTF-8".into()),
        }))
    }
}
