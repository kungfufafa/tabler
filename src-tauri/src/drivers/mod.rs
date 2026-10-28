//! Database drivers behind one trait. Each driver owns its connections; commands only see
//! `Arc<dyn DbSession>`.

use std::collections::VecDeque;
use std::future::Future;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::dialect::spec::ExplainForm;
use crate::sqlgen::dialect::{DialectRef, SqlDialect};
use crate::sqlgen::split::leading_keywords;
use crate::types::browse::{TraceCategory, TracedSql};
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::Flavor;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{
    ExecOutcome, ExplainMode, ResultColumn, ResultSet, ServerNotice, StatementKind,
};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub mod common;
pub mod mysql;
pub mod native;
pub mod postgres;
pub mod redact;
pub mod registry;
pub mod sharing;
pub mod sqlite;

// The lane drivers, each built only with its cargo feature (`registry` stands in for the others).
#[cfg(feature = "drv-bigquery")]
pub mod bigquery;
#[cfg(feature = "drv-cassandra")]
pub mod cassandra;
#[cfg(feature = "drv-clickhouse")]
pub mod clickhouse;
#[cfg(feature = "drv-duckdb")]
pub mod duckdb;
#[cfg(feature = "drv-dynamodb")]
pub mod dynamodb;
#[cfg(feature = "drv-elasticsearch")]
pub mod elasticsearch;
#[cfg(feature = "drv-mongodb")]
pub mod mongodb;
#[cfg(feature = "drv-redis")]
pub mod redis;
#[cfg(feature = "drv-snowflake")]
pub mod snowflake;
#[cfg(any(feature = "drv-libsql", feature = "drv-d1"))]
pub mod sqlite_http;
#[cfg(feature = "drv-sqlserver")]
pub mod sqlserver;
#[cfg(feature = "drv-vertica")]
pub mod vertica;

/// Receives rows streamed by `run_statement` / `stream_query`. Returning `Break` from `rows`
/// stops consumption (row cap reached or export cancelled).
pub trait RowSink: Send {
    fn begin_set(&mut self, columns: Vec<ResultColumn>);
    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()>;

    /// A server notice (PostgreSQL `RAISE NOTICE`…) raised while the statement ran (`during`),
    /// or before it started (`!during`: bootstrap SQL, an earlier session's activity). Returns
    /// whether the sink took it; otherwise the driver keeps it as text in
    /// `StatementOutcome::notices` (or logs it).
    fn notice(&mut self, _notice: &ServerNotice, _during: bool) -> bool {
        false
    }
}

// --- SQL trace --------------------------------------------------------------------------------

/// Statements a session keeps for `DbSession::drain_trace`; the oldest fall off beyond this.
pub const TRACE_CAP: usize = 500;
/// Longest SQL text a trace entry keeps (a bulk INSERT would otherwise fill memory).
pub const TRACE_SQL_MAX_CHARS: usize = 8 * 1024;

/// What a session really ran, for the console log: every statement the drivers send for the app's
/// own work (introspection, grid reads, counts, writes, exports) with its timing, row count and
/// error. The statements the SQL editor runs (`run_statement`) are not traced: the renderer logs
/// those itself with the exact text the user submitted; the ones the app runs through the same
/// editor session for its own purposes (the Explain command, a SQL dump import) go through
/// `run_statement_traced` and are. Passwords in a statement are masked (`redact`). The log is a
/// capped queue (`TRACE_CAP`) that the renderer
/// empties after each of its calls (`drain_sql_trace`). Cheap to clone: clones share the queue, so
/// a SQLite actor job can hold one.
#[derive(Debug, Clone, Default)]
pub struct SqlTrace(Arc<Mutex<VecDeque<TracedSql>>>);

/// Row and write counts a traced statement reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceCounts {
    pub rows: Option<u64>,
    pub affected: Option<u64>,
}

impl TraceCounts {
    pub const NONE: TraceCounts = TraceCounts {
        rows: None,
        affected: None,
    };

    pub fn rows(n: usize) -> TraceCounts {
        TraceCounts {
            rows: Some(n as u64),
            affected: None,
        }
    }

    pub fn of_exec(outcome: &ExecOutcome) -> TraceCounts {
        TraceCounts {
            rows: outcome.rows.as_ref().map(|r| r.rows.len() as u64),
            affected: Some(outcome.affected),
        }
    }
}

/// What a trace entry shows for a statement, and the passwords the statement carried (masked in
/// `text`, see `redact`), so an error recorded for it can be scrubbed of them too.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TraceSql {
    pub text: String,
    secrets: Vec<String>,
}

impl TraceSql {
    /// A server message about the statement with its passwords masked, should it quote them (a
    /// syntax error echoes the text around its position). A password too short to be masked
    /// without mangling the message withholds the message.
    pub fn scrub(&self, message: String) -> String {
        if self.secrets.is_empty() {
            return message;
        }
        if self.secrets.iter().any(|s| s.chars().count() < 3) {
            return "the statement failed (message withheld: the statement carries a password)"
                .to_string();
        }
        let mut forms: Vec<&String> = self.secrets.iter().collect();
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        forms
            .into_iter()
            .fold(message, |msg, form| msg.replace(form.as_str(), "********"))
    }
}

impl From<String> for TraceSql {
    fn from(text: String) -> TraceSql {
        TraceSql {
            text,
            secrets: Vec::new(),
        }
    }
}

impl From<&str> for TraceSql {
    fn from(text: &str) -> TraceSql {
        text.to_string().into()
    }
}

/// The text a trace entry shows for `sql` run with `params`: the parameters inlined (for display
/// only: the statement itself ran with bound parameters), the passwords it carries masked
/// (`redact::redact_secrets`: the console log must never show one), then capped.
pub fn trace_sql(dialect: impl Into<SqlDialect>, sql: &str, params: &[Value]) -> TraceSql {
    let dialect = dialect.into();
    let text = if params.is_empty() {
        sql.trim().to_string()
    } else {
        crate::sqlgen::sql::inline_params(dialect, sql.trim(), params)
    };
    let redacted = crate::sqlgen::sql::redact(dialect, &text);
    TraceSql {
        text: cap_trace_text(redacted.sql),
        secrets: redacted.secrets,
    }
}

/// `trace_sql`'s text, for the introspection statements, which carry no password.
pub fn trace_text(dialect: impl Into<SqlDialect>, sql: &str, params: &[Value]) -> String {
    trace_sql(dialect, sql, params).text
}

fn cap_trace_text(text: String) -> String {
    let mut chars = text.char_indices().skip(TRACE_SQL_MAX_CHARS);
    match chars.next() {
        None => text,
        Some((at, _)) => format!(
            "{} /* … {} more characters */",
            &text[..at],
            text[at..].chars().count()
        ),
    }
}

/// Catalog tables and functions that mark a read as introspection ("meta") rather than data.
const CATALOG_WORDS: [&str; 26] = [
    "INFORMATION_SCHEMA",
    "PERFORMANCE_SCHEMA",
    "PG_CATALOG",
    "PG_CLASS",
    "PG_NAMESPACE",
    "PG_ATTRIBUTE",
    "PG_ATTRDEF",
    "PG_INDEX",
    "PG_CONSTRAINT",
    "PG_PROC",
    "PG_TYPE",
    "PG_DATABASE",
    "PG_ROLES",
    "PG_TRIGGER",
    "PG_SEQUENCE",
    "PG_VIEWS",
    "PG_TABLES",
    "PG_DESCRIPTION",
    "PG_ENUM",
    "PG_EXTENSION",
    "PG_GET_",
    "PG_DEPEND",
    "SQLITE_MASTER",
    "SQLITE_SCHEMA",
    "SQLITE_TEMP_MASTER",
    "SQLITE_TEMP_SCHEMA",
];

/// Whether a statement the app ran reads or changes the schema (DDL, `PRAGMA`, `SHOW`, a read of
/// the catalogs) rather than rows. A judgement from the statement text, for the paths that run
/// whatever SQL they are given; introspection functions state their category themselves.
pub fn classify_statement(sql: &str, dialect: impl Into<SqlDialect>) -> TraceCategory {
    let dialect = dialect.into();
    let first = leading_keywords(sql, 1);
    if matches!(
        first.first().map(String::as_str),
        Some(
            "CREATE"
                | "ALTER"
                | "DROP"
                | "TRUNCATE"
                | "RENAME"
                | "COMMENT"
                | "GRANT"
                | "REVOKE"
                | "VACUUM"
                | "REINDEX"
                | "PRAGMA"
                | "SHOW"
                | "DESCRIBE"
                | "DESC"
        )
    ) {
        return TraceCategory::Structure;
    }
    let code = crate::sqlgen::sql::code_only(dialect, sql).to_uppercase();
    let catalog = match dialect.resolve() {
        DialectRef::Core(core) => {
            CATALOG_WORDS.iter().any(|w| code.contains(w))
                || (core == Dialect::Mysql && code.contains("MYSQL."))
        }
        DialectRef::Ext(spec) => spec.knobs.catalog_words.iter().any(|w| code.contains(w)),
    };
    if catalog {
        TraceCategory::Structure
    } else {
        TraceCategory::Data
    }
}

impl SqlTrace {
    fn queue(&self) -> std::sync::MutexGuard<'_, VecDeque<TracedSql>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Appends a statement, dropping the oldest ones beyond `TRACE_CAP`.
    pub fn record(&self, entry: TracedSql) {
        let mut queue = self.queue();
        queue.push_back(entry);
        while queue.len() > TRACE_CAP {
            queue.pop_front();
        }
    }

    /// Takes every recorded statement, oldest first.
    pub fn drain(&self) -> Vec<TracedSql> {
        self.queue().drain(..).collect()
    }

    /// Statements waiting to be drained.
    pub fn len(&self) -> usize {
        self.queue().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records one finished statement: `sql` is already display text (`trace_sql`).
    pub fn finish<T>(
        &self,
        category: TraceCategory,
        sql: impl Into<TraceSql>,
        started: Instant,
        result: &Result<T, AppError>,
        counts: impl FnOnce(&T) -> TraceCounts,
    ) {
        let sql = sql.into();
        let (counts, error) = match result {
            Ok(v) => (counts(v), None),
            Err(e) => (TraceCounts::NONE, Some(sql.scrub(e.to_string()))),
        };
        self.record(TracedSql {
            sql: sql.text,
            category,
            duration_ms: started.elapsed().as_millis() as u64,
            rows: counts.rows,
            affected: counts.affected,
            error,
        });
    }

    /// Runs a blocking statement (a SQLite actor job) and records it.
    pub fn run<T>(
        &self,
        category: TraceCategory,
        sql: impl Into<TraceSql>,
        counts: impl FnOnce(&T) -> TraceCounts,
        run: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let started = Instant::now();
        let result = run();
        self.finish(category, sql, started, &result, counts);
        result
    }

    /// Runs an async statement and records it, whether it succeeds, fails or is cancelled.
    pub async fn run_async<T, F>(
        &self,
        category: TraceCategory,
        sql: impl Into<TraceSql>,
        counts: impl FnOnce(&T) -> TraceCounts,
        statement: F,
    ) -> Result<T, AppError>
    where
        F: Future<Output = Result<T, AppError>>,
    {
        let started = Instant::now();
        let result = statement.await;
        self.finish(category, sql, started, &result, counts);
        result
    }
}

/// A handle that can abort the statement currently running on a specific connection.
#[derive(Clone)]
pub enum CancelHandle {
    Postgres(tokio_postgres::CancelToken),
    Mysql {
        conn_id: u32,
    },
    Sqlite(Arc<rusqlite::InterruptHandle>),
    /// Cancels nothing (`DbSession::cancel` returns `Ok`).
    Requested,
    /// What `request_cancel` hands out: the live handle plus a ticket that tells the statement's
    /// driver when the cancel request is done. The ticket is released when the last clone of
    /// this handle is dropped, i.e. once `DbSession::cancel` returns.
    Pending(Box<CancelHandle>, Arc<CancelTicket>),
}

impl CancelHandle {
    /// The handle to act on, plus the ticket to keep alive until the request has been sent.
    pub fn into_parts(self) -> (CancelHandle, Option<Arc<CancelTicket>>) {
        match self {
            CancelHandle::Pending(inner, ticket) => (inner.into_parts().0, Some(ticket)),
            other => (other, None),
        }
    }
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CancelHandle::Postgres(_) => f.write_str("CancelHandle::Postgres"),
            CancelHandle::Mysql { conn_id } => write!(f, "CancelHandle::Mysql({conn_id})"),
            CancelHandle::Sqlite(_) => f.write_str("CancelHandle::Sqlite"),
            CancelHandle::Requested => f.write_str("CancelHandle::Requested"),
            CancelHandle::Pending(inner, _) => write!(f, "CancelHandle::Pending({inner:?})"),
        }
    }
}

/// Shared between one run (`run_sql`, an export…) and whoever may cancel it. The driver puts the
/// live handle of the statement it is executing into the slot (`arm_cancel`) and clears it
/// afterwards; `request_cancel` marks the run as cancelled and returns that handle.
///
/// The slot also counts cancel requests still on their way to the server, so a driver can wait
/// for them (`settle_cancels`) before it reuses the connection: a late `KILL QUERY` or cancel
/// signal would otherwise abort whatever runs there next. Create one with `CancelSlot::default()`.
#[derive(Clone, Default)]
pub struct CancelSlot(Arc<SlotShared>);

#[derive(Default)]
struct SlotShared {
    state: std::sync::Mutex<SlotState>,
    /// Signalled when `in_flight` drops to zero (blocking waiters: SQLite actor threads).
    settled: std::sync::Condvar,
    /// Same, for async waiters.
    notify: tokio::sync::Notify,
}

#[derive(Default)]
struct SlotState {
    /// `request_cancel` was called: no further statement may start with this slot.
    requested: bool,
    /// Handle of the statement executing right now.
    live: Option<CancelHandle>,
    /// Handles given out by `request_cancel` whose `DbSession::cancel` has not finished.
    in_flight: usize,
}

impl CancelSlot {
    fn state(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a statement has put its live handle into the slot and is still executing.
    pub fn is_armed(&self) -> bool {
        self.state().live.is_some()
    }

    /// Cancel requests handed out and not finished yet.
    pub fn cancels_in_flight(&self) -> usize {
        self.state().in_flight
    }
}

impl std::fmt::Debug for CancelSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        f.debug_struct("CancelSlot")
            .field("requested", &state.requested)
            .field("live", &state.live)
            .field("in_flight", &state.in_flight)
            .finish()
    }
}

/// Keeps a cancel request counted as in flight (see `CancelSlot`) until dropped.
pub struct CancelTicket {
    slot: CancelSlot,
}

impl Drop for CancelTicket {
    fn drop(&mut self) {
        let mut state = self.slot.state();
        state.in_flight = state.in_flight.saturating_sub(1);
        if state.in_flight == 0 {
            drop(state);
            self.slot.0.settled.notify_all();
            self.slot.0.notify.notify_waiters();
        }
    }
}

/// Clears a slot armed by `arm_cancel` when dropped (a pending cancel request is kept).
pub struct ArmedCancel<'a> {
    slot: &'a CancelSlot,
}

impl Drop for ArmedCancel<'_> {
    fn drop(&mut self) {
        self.slot.state().live = None;
    }
}

/// Puts the live handle of the statement about to run into `slot`. Fails with `Cancelled` when
/// the run was cancelled before this statement could start.
pub fn arm_cancel(slot: &CancelSlot, handle: CancelHandle) -> Result<ArmedCancel<'_>, AppError> {
    let mut state = slot.state();
    if state.requested {
        return Err(AppError::Cancelled);
    }
    state.live = Some(handle);
    Ok(ArmedCancel { slot })
}

/// Marks the run using `slot` as cancelled and returns the handle of the statement running
/// right now, if any, to pass to `DbSession::cancel`. Calling it again while the same statement
/// still runs returns its handle again, so a cancel that failed (network, busy server) can be
/// retried.
pub fn request_cancel(slot: &CancelSlot) -> Option<CancelHandle> {
    let handle = {
        let mut state = slot.state();
        state.requested = true;
        state.live.clone().map(|live| {
            state.in_flight += 1;
            CancelHandle::Pending(
                Box::new(live),
                Arc::new(CancelTicket { slot: slot.clone() }),
            )
        })
    };
    // Wakes a run still waiting for the editor (`lock_editor_runs`).
    slot.0.notify.notify_waiters();
    handle
}

/// `true` once `request_cancel` was called on the slot.
pub fn cancel_requested(slot: &CancelSlot) -> bool {
    slot.state().requested
}

/// Waits (at most `limit`) until every cancel request handed out by `request_cancel` has been
/// sent, so none of them can still land on the connection once it runs something else.
/// Returns `false` on timeout.
pub async fn settle_cancels(slot: &CancelSlot, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let notified = slot.0.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if slot.state().in_flight == 0 {
            return true;
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return slot.state().in_flight == 0;
        }
    }
}

/// `settle_cancels` for threads outside the async runtime (SQLite actors).
pub fn settle_cancels_blocking(slot: &CancelSlot, limit: Duration) -> bool {
    let state = slot.state();
    let (state, _) = slot
        .0
        .settled
        .wait_timeout_while(state, limit, |s| s.in_flight > 0)
        .unwrap_or_else(|p| p.into_inner());
    state.in_flight == 0
}

/// Upper bound for `settle_cancels` in the drivers (a cancel request has its own timeouts).
pub const CANCEL_SETTLE_LIMIT: Duration = Duration::from_secs(15);

/// Holds a session's editor for one whole run (see `DbSession::editor_run_lock`).
pub type EditorRunGuard<'a> = tokio::sync::MutexGuard<'a, ()>;

/// Waits until no other run uses the session's editor and keeps it for the caller, so the
/// statements of one run (`BEGIN; …; ROLLBACK`) execute back to back and another tab's
/// statements cannot land in between. Runs queue in order. Fails with `Cancelled` when the run
/// is cancelled while it waits. `None` for sessions without an editor lock.
pub async fn lock_editor_runs<'a>(
    session: &'a dyn DbSession,
    cancel: &CancelSlot,
) -> Result<Option<EditorRunGuard<'a>>, AppError> {
    let Some(lock) = session.editor_run_lock() else {
        return Ok(None);
    };
    let acquire = lock.lock();
    tokio::pin!(acquire);
    loop {
        let notified = cancel.0.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if cancel_requested(cancel) {
            return Err(AppError::Cancelled);
        }
        tokio::select! {
            biased;
            guard = &mut acquire => return Ok(Some(guard)),
            _ = notified => {}
        }
    }
}

/// What `run_statement` reports besides the streamed rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatementOutcome {
    pub kind: StatementKind,
    pub result_sets: u32,
    pub row_count: u64,
    pub truncated: bool,
    pub affected: Option<u64>,
    pub last_insert_id: Option<String>,
    pub notices: Vec<String>,
}

/// Rows are handed to the sink in chunks of at most this many…
pub const ROW_CHUNK: usize = 500;
/// …or earlier when rows trickle in slowly, so the grid fills while a query runs.
const CHUNK_MAX_AGE: Duration = Duration::from_millis(250);

/// Buffers decoded rows and forwards them to a `RowSink` in chunks.
pub struct RowChunker<'s> {
    sink: &'s mut dyn RowSink,
    buf: Vec<Vec<Value>>,
    last_flush: Instant,
    /// Rows pushed into the current result set.
    pub rows_in_set: u64,
    /// Rows pushed over all result sets.
    pub total_rows: u64,
    /// Result sets started.
    pub sets: u32,
}

impl<'s> RowChunker<'s> {
    pub fn new(sink: &'s mut dyn RowSink) -> RowChunker<'s> {
        RowChunker {
            sink,
            buf: Vec::new(),
            last_flush: Instant::now(),
            rows_in_set: 0,
            total_rows: 0,
            sets: 0,
        }
    }

    /// Flushes the previous set's rows and announces a new result set.
    pub fn begin_set(&mut self, columns: Vec<ResultColumn>) -> ControlFlow<()> {
        let flow = self.flush();
        self.sink.begin_set(columns);
        self.rows_in_set = 0;
        self.sets += 1;
        self.last_flush = Instant::now();
        flow
    }

    pub fn push(&mut self, row: Vec<Value>) -> ControlFlow<()> {
        self.buf.push(row);
        self.rows_in_set += 1;
        self.total_rows += 1;
        if self.buf.len() >= ROW_CHUNK || self.last_flush.elapsed() >= CHUNK_MAX_AGE {
            self.flush()
        } else {
            ControlFlow::Continue(())
        }
    }

    pub fn flush(&mut self) -> ControlFlow<()> {
        self.last_flush = Instant::now();
        if self.buf.is_empty() {
            return ControlFlow::Continue(());
        }
        let chunk = std::mem::take(&mut self.buf);
        self.sink.rows(chunk)
    }

    /// Hands a server notice to the sink (`RowSink::notice`).
    pub fn notice(&mut self, notice: &ServerNotice, during: bool) -> bool {
        self.sink.notice(notice, during)
    }
}

/// A sink that keeps everything (EXPLAIN output, tests).
#[derive(Debug, Default)]
pub struct CollectSink {
    pub sets: Vec<ResultSet>,
}

impl RowSink for CollectSink {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.sets.push(ResultSet {
            columns,
            rows: Vec::new(),
            truncated: false,
        });
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        if let Some(set) = self.sets.last_mut() {
            set.rows.extend(rows);
        }
        ControlFlow::Continue(())
    }
}

/// Transaction-state change implied by a statement, judged from its leading keywords and its
/// error (`None` when it succeeded); used by the PostgreSQL and MySQL editor sessions, which
/// cannot observe the server's flag directly.
/// `Some(true)`: a transaction is open afterwards; `Some(false)`: none is; `None`: unchanged.
pub fn tx_effect(
    sql: &str,
    dialect: impl Into<SqlDialect>,
    error: Option<&AppError>,
) -> Option<bool> {
    let (mysql, implicit_commit) = match dialect.into().resolve() {
        DialectRef::Core(core) => (core == Dialect::Mysql, false),
        DialectRef::Ext(spec) => (false, (spec.knobs.implicit_commit)(sql)),
    };
    let succeeded = error.is_none();
    let kw = leading_keywords(sql, 4);
    let w = |i: usize| kw.get(i).map(String::as_str).unwrap_or("");
    match w(0) {
        "BEGIN" if w(1) == "NOT" => None, // MariaDB compound statement
        "BEGIN" => succeeded.then_some(true),
        "START" if w(1) == "TRANSACTION" => succeeded.then_some(true),
        "COMMIT" | "ROLLBACK" | "END" | "ABORT" => {
            if w(1) == "TO" || w(1) == "PREPARED" || (w(1) == "WORK" && w(2) == "TO") {
                None // ROLLBACK TO SAVEPOINT, COMMIT/ROLLBACK PREPARED
            } else if kw.iter().any(|k| k == "CHAIN") && !kw.iter().any(|k| k == "NO") {
                // `… AND CHAIN` opens the next transaction only when it worked (PostgreSQL
                // refuses it outside a transaction block; a failed COMMIT ends the transaction).
                Some(succeeded)
            } else {
                Some(false)
            }
        }
        "PREPARE" if w(1) == "TRANSACTION" => Some(false),
        // MySQL commits implicitly before most DDL and administrative statements, once they
        // have been parsed: a statement with a syntax error leaves the transaction open.
        "CREATE" | "ALTER" | "DROP" | "RENAME" | "TRUNCATE" | "GRANT" | "REVOKE" | "LOCK"
        | "UNLOCK" | "ANALYZE" | "OPTIMIZE" | "REPAIR"
            if mysql =>
        {
            let temporary = matches!(w(0), "CREATE" | "DROP") && w(1) == "TEMPORARY";
            if temporary || error.is_some_and(is_mysql_parse_error) {
                None
            } else {
                Some(false)
            }
        }
        // An extension dialect's own implicit commits (its spec names them); whether a failed
        // one still committed is unknown.
        _ if implicit_commit => succeeded.then_some(false),
        _ => None,
    }
}

/// MySQL `ER_PARSE_ERROR` (1064) / `ER_SYNTAX_ERROR` (1149): the statement never started.
fn is_mysql_parse_error(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Db {
            vendor_code: Some(1064 | 1149),
            ..
        }
    )
}

/// Editor transaction state shared between the statement runner and `in_transaction()`.
#[derive(Debug)]
pub struct TxState(std::sync::atomic::AtomicU8);

impl TxState {
    pub fn new(open: Option<bool>) -> TxState {
        let state = TxState(std::sync::atomic::AtomicU8::new(0));
        state.set(open);
        state
    }

    pub fn get(&self) -> Option<bool> {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    pub fn set(&self, open: Option<bool>) {
        let v = match open {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        };
        self.0.store(v, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Default for TxState {
    /// A fresh session has no open transaction.
    fn default() -> Self {
        TxState::new(Some(false))
    }
}

/// Statement kinds whose row count is meaningful even when it is zero.
pub fn is_dml(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "UPSERT" | "LOAD" | "COPY")
    )
}

/// `true` when `word` (upper case) occurs in `upper` (already upper-cased) as a whole word.
pub fn contains_word(upper: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    upper.match_indices(word).any(|(i, _)| {
        let before = upper[..i].chars().next_back();
        let after = upper[i + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// Whether a statement that is stopped early (row cap reached, export done) may be cancelled on
/// the server rather than drained. Cancelling aborts the statement and rolls back what it did,
/// so it is only used for a single read-only query, where it merely saves sending the rows
/// nobody will read. Everything else (`CALL`, `FETCH`, DML with `RETURNING`, data-modifying
/// CTEs, `SELECT … FOR UPDATE`, several statements sent as one) runs to completion. A query
/// calling functions with side effects can still be cancelled; drivers then add a notice
/// (`cap_cancel_notice`). The word checks look at code only (not at string literals, quoted
/// identifiers or comments) and are otherwise deliberately coarse: a false positive only costs
/// draining.
pub fn cancel_safe_on_stop(sql: &str, dialect: impl Into<SqlDialect>) -> bool {
    let dialect = dialect.into();
    let first = leading_keywords(sql, 1);
    if !matches!(
        first.first().map(String::as_str),
        Some("SELECT" | "WITH" | "TABLE" | "VALUES" | "SHOW")
    ) {
        return false;
    }
    if !crate::sqlgen::sql::is_single_statement(dialect, sql) {
        return false;
    }
    let upper = crate::sqlgen::sql::code_only(dialect, sql).to_uppercase();
    !["INSERT", "UPDATE", "DELETE", "MERGE", "CALL"]
        .iter()
        .any(|w| contains_word(&upper, w))
}

/// Notice added when a cancel was requested but the server still completed the statement (it
/// ignored the cancel or got it too late), so its results and changes stand.
pub const LATE_CANCEL_NOTICE: &str =
    "Cancel was requested, but the server completed the statement: its results and changes stand.";

/// Notice added when a statement stopped at the row cap was cancelled on the server and the
/// server confirmed it: the statement did not finish, so whatever it changed was undone.
pub fn cap_cancel_notice(rows: u64, dialect: impl Into<SqlDialect>) -> String {
    let undone = match dialect.into().resolve() {
        DialectRef::Core(Dialect::Mysql) => {
            "changes it made to transactional tables were rolled back"
        }
        DialectRef::Core(Dialect::Postgres | Dialect::Sqlite) => {
            "any changes it made were rolled back"
        }
        DialectRef::Ext(spec) => spec.knobs.cancel_notice,
    };
    format!(
        "Stopped at the row limit ({rows} rows): the rest of the statement was cancelled on the \
         server, so it did not finish and {undone}."
    )
}

#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub version: String,
    pub current_database: Option<String>,
    pub current_user: Option<String>,
}

/// A live connection to one database server (pool + dedicated editor session).
#[async_trait]
pub trait DbSession: Send + Sync {
    fn driver(&self) -> Driver;
    /// The SQL the session speaks. Shared code builds it through the `sqlgen::sql` facade; the
    /// core-only generators take `core_dialect` instead.
    fn dialect(&self) -> SqlDialect;

    async fn server_info(&self) -> Result<ServerInfo, AppError>;
    async fn ping(&self) -> Result<(), AppError>;
    async fn close(&self);

    // --- introspection (worker pool) ---
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError>;
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError>;
    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError>;
    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError>;
    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError>;
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError>;

    // --- generic execution (worker pool) ---
    /// Runs a statement that returns rows. Postgres: columns should be cast to text by the caller
    /// (sqlgen does this); the driver returns `Value::Text`/`Null` plus the described type name so
    /// the caller can decode. MySQL/SQLite return typed values.
    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError>;
    /// `query` for a statement the caller may cancel (a table load, a row count or an EXPLAIN
    /// under the query time out): the driver puts the live handle into `cancel` while the
    /// statement runs (`arm_cancel`), like `execute_cancellable`, and waits for cancel requests
    /// still on their way before the connection is used again. A statement stopped by
    /// `request_cancel` + `DbSession::cancel` fails with `Cancelled`. Every driver overrides this:
    /// the default cannot be cancelled.
    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        let _ = cancel;
        self.query(sql, params).await
    }
    /// Runs a statement for its side effects (optionally with RETURNING rows).
    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError>;
    /// `execute` for a statement the caller may cancel (a Metrics Board object that writes, which
    /// has a time limit): the driver puts the live handle into `cancel` while the statement runs
    /// (`arm_cancel`), as `stream_query` does, and waits for cancel requests still on their way
    /// before the connection is used again. A statement stopped by `request_cancel` +
    /// `DbSession::cancel` fails with `Cancelled`, and the server rolls it back (a single
    /// statement in autocommit); one that finished first returns its outcome. Every driver
    /// overrides this: the default cannot be cancelled.
    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        let _ = cancel;
        self.execute(sql, params).await
    }
    /// Runs several statements inside one transaction; rolls back on the first error or when a
    /// statement's `expect_affected` is not met.
    async fn transaction(&self, statements: Vec<TxStatement>)
    -> Result<Vec<ExecOutcome>, AppError>;

    // --- SQL editor ---
    /// Runs one statement on the dedicated **editor** session (so `BEGIN` / `SET` persist across
    /// runs), streaming result rows into `sink` in chunks. Stops reading after `max_rows` rows
    /// (`truncated`), stopping the server side too where that is safe. The driver puts its
    /// cancel handle into `cancel` while the statement runs (`arm_cancel`).
    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError>;

    /// Streams every row of a query on a **worker** connection (exports): no row cap, stops
    /// early when the sink returns `Break`. Returns the number of rows delivered.
    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError>;

    /// Aborts the statement a handle was taken from (`request_cancel`). Never fails because
    /// nothing was running.
    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError>;

    /// Whether the editor session has an open transaction (`None` when unknown).
    fn in_transaction(&self) -> Option<bool>;

    /// Takes the statements this session ran for the app since the last call (see `SqlTrace`),
    /// oldest first: introspection, table loads, counts, writes, exports. Sessions that keep no
    /// trace return nothing.
    fn drain_trace(&self) -> Vec<TracedSql> {
        Vec::new()
    }

    /// The trace this session records into (`None`: it keeps none).
    fn sql_trace(&self) -> Option<&SqlTrace> {
        None
    }

    /// `run_statement` for the statements the app runs for its own purposes (the EXPLAIN a button
    /// asks for and the transaction it wraps around an Analyze, a SQL dump import): the statement
    /// is recorded in the session's trace like every other statement the app runs, so the console
    /// log shows what really ran. The SQL editor's runs call `run_statement` itself: the renderer
    /// logs those with the text the user submitted, and tracing them too would log them twice.
    async fn run_statement_traced(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        let started = Instant::now();
        let result = self.run_statement(sql, max_rows, sink, cancel).await;
        if let Some(trace) = self.sql_trace() {
            let dialect = self.dialect();
            trace.finish(
                classify_statement(sql, dialect),
                trace_sql(dialect, sql, &[]),
                started,
                &result,
                |o| TraceCounts {
                    rows: (o.result_sets > 0).then_some(o.row_count),
                    affected: o.affected,
                },
            );
        }
        result
    }

    /// Lock a whole editor run holds (`lock_editor_runs`), so the statements of concurrent runs
    /// on the shared editor session never interleave. `run_statement` itself does not take it.
    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        None
    }

    /// MariaDB rather than MySQL (EXPLAIN syntax differs).
    fn is_mariadb(&self) -> bool {
        false
    }

    /// The server family the session found, when its driver serves several.
    fn flavor(&self) -> Option<Flavor> {
        self.is_mariadb().then_some(Flavor::Mariadb)
    }

    /// What the session can do: every feature for a core driver on its core dialect, otherwise
    /// `SessionCaps::minimal` until the driver says more. The backend refuses what they lack
    /// (`require`, `core_dialect`) before anything reaches the server.
    fn capabilities(&self) -> SessionCaps {
        SessionCaps::base(self.driver(), self.dialect(), self.flavor())
    }

    /// The session's own EXPLAIN of `mode`, where its server differs from its dialect's
    /// (`sqlgen::sql::wrap_explain` otherwise).
    fn explain_form(&self, _mode: ExplainMode) -> Option<ExplainForm> {
        None
    }
}

impl dyn DbSession + '_ {
    /// Refuses feature `f` when the session's caps lack it, before the caller sends anything.
    pub fn require(&self, f: Feature) -> Result<(), AppError> {
        self.capabilities().require(f)
    }

    /// The core dialect of the session, for core-only feature `f` (the DDL planner, exports,
    /// administration…): the only way into the core SQL generators besides the `sqlgen::sql`
    /// facade. Refused unless the session's caps grant `f`, the dialect is a core one and the
    /// driver is a core driver or its descriptor lets it reuse `f` (`core_reuse`): an extension
    /// dialect would otherwise fall into a core generator's "else" branch.
    pub fn core_dialect(&self, f: Feature) -> Result<Dialect, AppError> {
        self.require(f)?;
        let driver = self.driver();
        core_dialect_for(driver, driver.descriptor().core_reuse, self.dialect(), f)
    }
}

/// `core_dialect` with the driver's `core_reuse` list given.
fn core_dialect_for(
    driver: Driver,
    core_reuse: &[Feature],
    dialect: SqlDialect,
    f: Feature,
) -> Result<Dialect, AppError> {
    match dialect.core() {
        Some(core) if driver.is_core() || core_reuse.contains(&f) => Ok(core),
        Some(_) => Err(not_offered(driver.label(), f)),
        None => Err(not_offered(dialect.label(), f)),
    }
}

fn not_offered(name: &str, f: Feature) -> AppError {
    AppError::Unsupported(format!("{name} connections do not offer {f:?}"))
}

/// One statement of a transaction. `expect_affected` makes the driver roll back when the
/// statement touches a different number of rows (guards UPDATE/DELETE by key).
#[derive(Debug, Clone)]
pub struct TxStatement {
    pub sql: String,
    pub params: Vec<Value>,
    pub expect_affected: Option<u64>,
}

impl TxStatement {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> TxStatement {
        TxStatement {
            sql: sql.into(),
            params,
            expect_affected: None,
        }
    }

    pub fn expecting(mut self, n: u64) -> TxStatement {
        self.expect_affected = Some(n);
        self
    }
}

/// Shared check used by every driver inside `transaction`, before it commits. An outcome whose
/// count the server did not report (`affected_unknown`) cannot pass a guard: a driver that
/// cannot prove its counts checks its own way first or offers no keyless edits (README of
/// `docs/drivers`).
pub fn check_affected(
    st: &TxStatement,
    outcome: &ExecOutcome,
    index: usize,
) -> Result<(), AppError> {
    match st.expect_affected {
        Some(n) if outcome.affected_unknown => Err(AppError::db(format!(
            "expected to affect {n} row(s), but the server cannot confirm how many rows the \
             statement changed, so it was rolled back"
        ))
        .with_statement_index(index)),
        Some(n) if outcome.affected != n => Err(AppError::db(format!(
            "expected to affect {n} row(s) but matched {}; the row may have been changed or deleted by someone else",
            outcome.affected
        ))
        .with_statement_index(index)),
        _ => Ok(()),
    }
}

/// Everything needed to open a session. `host_override` points at a local SSH tunnel listener.
pub struct ConnectTarget<'a> {
    pub profile: &'a ConnectionProfile,
    pub password: Option<&'a str>,
    pub host_override: Option<(String, u16)>,
}

impl ConnectTarget<'_> {
    pub fn host(&self) -> String {
        match &self.host_override {
            Some((h, _)) => h.clone(),
            None => self
                .profile
                .host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string()),
        }
    }

    pub fn port(&self) -> u16 {
        match &self.host_override {
            Some((_, p)) => *p,
            None => self
                .profile
                .port
                .or_else(|| self.profile.default_port())
                .unwrap_or(0),
        }
    }

    /// Host name used for TLS verification / SNI even when connecting through a tunnel.
    pub fn tls_host(&self) -> String {
        self.profile
            .host
            .clone()
            .unwrap_or_else(|| "localhost".to_string())
    }
}

/// Opens a SQL session for `target`'s profile with its driver (see `registry::connect`).
pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    registry::connect(target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;

    #[derive(Default)]
    struct Recorder {
        sets: usize,
        chunks: Vec<usize>,
        stop_after: Option<usize>,
    }

    impl RowSink for Recorder {
        fn begin_set(&mut self, _columns: Vec<ResultColumn>) {
            self.sets += 1;
        }
        fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
            self.chunks.push(rows.len());
            match self.stop_after {
                Some(n) if self.chunks.len() >= n => ControlFlow::Break(()),
                _ => ControlFlow::Continue(()),
            }
        }
    }

    fn col() -> ResultColumn {
        ResultColumn {
            name: "a".into(),
            data_type: "int".into(),
            kind: ValueKind::Int,
            nullable: None,
            table: None,
            origin: None,
        }
    }

    #[test]
    fn chunker_sends_chunks_of_at_most_500_rows() {
        let mut sink = Recorder::default();
        let mut chunker = RowChunker::new(&mut sink);
        assert!(chunker.begin_set(vec![col()]).is_continue());
        for i in 0..1234 {
            assert!(chunker.push(vec![Value::Int(i)]).is_continue());
        }
        assert!(chunker.flush().is_continue());
        assert_eq!(chunker.total_rows, 1234);
        // A new set flushes nothing (buffer empty) and resets the per-set counter.
        assert!(chunker.begin_set(vec![col()]).is_continue());
        assert_eq!(chunker.rows_in_set, 0);
        assert!(chunker.push(vec![Value::Int(1)]).is_continue());
        assert!(chunker.flush().is_continue());
        assert_eq!(chunker.sets, 2);
        assert_eq!(sink.sets, 2);
        assert_eq!(sink.chunks, [500, 500, 234, 1]);
    }

    #[test]
    fn chunker_propagates_break() {
        let mut sink = Recorder {
            stop_after: Some(1),
            ..Recorder::default()
        };
        let mut chunker = RowChunker::new(&mut sink);
        assert!(chunker.begin_set(vec![col()]).is_continue());
        let mut broke_at = None;
        for i in 0..2000 {
            if chunker.push(vec![Value::Int(i)]).is_break() {
                broke_at = Some(i);
                break;
            }
        }
        assert_eq!(broke_at, Some(499));
    }

    #[test]
    fn collect_sink_keeps_sets() {
        let mut sink = CollectSink::default();
        let mut chunker = RowChunker::new(&mut sink);
        assert!(chunker.begin_set(vec![col()]).is_continue());
        assert!(chunker.push(vec![Value::Int(1)]).is_continue());
        assert!(chunker.begin_set(vec![col(), col()]).is_continue());
        assert!(
            chunker
                .push(vec![Value::Int(2), Value::Int(3)])
                .is_continue()
        );
        assert!(chunker.flush().is_continue());
        assert_eq!(sink.sets.len(), 2);
        assert_eq!(sink.sets[0].rows, vec![vec![Value::Int(1)]]);
        assert_eq!(sink.sets[1].columns.len(), 2);
    }

    fn traced(sql: &str) -> TracedSql {
        TracedSql {
            sql: sql.into(),
            category: TraceCategory::Data,
            duration_ms: 1,
            rows: None,
            affected: None,
            error: None,
        }
    }

    #[test]
    fn trace_drains_oldest_first_and_keeps_the_newest_entries() {
        let trace = SqlTrace::default();
        assert!(trace.is_empty());
        for i in 0..TRACE_CAP + 10 {
            trace.record(traced(&format!("SELECT {i}")));
        }
        assert_eq!(trace.len(), TRACE_CAP);
        // A clone shares the queue (a SQLite actor job holds one).
        let drained = trace.clone().drain();
        assert_eq!(drained.len(), TRACE_CAP);
        assert_eq!(drained[0].sql, "SELECT 10");
        assert_eq!(
            drained.last().unwrap().sql,
            format!("SELECT {}", TRACE_CAP + 9)
        );
        assert!(trace.drain().is_empty());
    }

    #[test]
    fn trace_records_timing_counts_and_errors() {
        let trace = SqlTrace::default();
        let ok: Result<Vec<u8>, AppError> = trace.run(
            TraceCategory::Data,
            "SELECT 1",
            |v: &Vec<u8>| TraceCounts::rows(v.len()),
            || {
                std::thread::sleep(Duration::from_millis(15));
                Ok(vec![1, 2, 3])
            },
        );
        assert!(ok.is_ok());
        let bad: Result<(), AppError> = trace.run(
            TraceCategory::Structure,
            "SELECT nope",
            |_| TraceCounts::NONE,
            || Err(AppError::db("no such column")),
        );
        assert!(bad.is_err());
        let entries = trace.drain();
        assert_eq!(entries[0].rows, Some(3));
        assert!(entries[0].duration_ms >= 10, "{entries:?}");
        assert_eq!(entries[0].error, None);
        assert_eq!(entries[1].category, TraceCategory::Structure);
        assert_eq!(entries[1].error.as_deref(), Some("no such column"));
        assert_eq!(entries[1].rows, None);
    }

    #[test]
    fn trace_text_inlines_parameters_and_caps_long_statements() {
        let shown = trace_text(
            Dialect::Postgres,
            "  SELECT * FROM t WHERE id = $1 AND name = $2 ",
            &[Value::Int(5), Value::Text("o'brien".into())],
        );
        assert_eq!(shown, "SELECT * FROM t WHERE id = 5 AND name = 'o''brien'");
        assert_eq!(trace_text(Dialect::Sqlite, "SELECT 1", &[]), "SELECT 1");
        let long = format!(
            "INSERT INTO t VALUES ('{}')",
            "x".repeat(TRACE_SQL_MAX_CHARS * 2)
        );
        let capped = trace_text(Dialect::Mysql, &long, &[]);
        assert!(capped.starts_with("INSERT INTO t VALUES ('xxx"));
        assert!(
            capped.contains("more characters */"),
            "{}",
            &capped[capped.len() - 40..]
        );
        assert!(capped.chars().count() < TRACE_SQL_MAX_CHARS + 60);
    }

    #[test]
    fn the_trace_never_holds_a_password() {
        let trace = SqlTrace::default();
        let sql = "CREATE USER 'app'@'%' IDENTIFIED BY 'hunter2'";
        // The statement itself ran with the password; what the trace keeps is masked.
        let failed: Result<(), AppError> = trace.run(
            classify_statement(sql, Dialect::Mysql),
            trace_sql(Dialect::Mysql, sql, &[]),
            |_| TraceCounts::NONE,
            || {
                Err(AppError::db(
                    "You have an error in your SQL syntax near 'hunter2' at line 1",
                ))
            },
        );
        assert!(failed.is_err());
        let entry = trace.drain().remove(0);
        assert_eq!(entry.sql, "CREATE USER 'app'@'%' IDENTIFIED BY '********'");
        assert_eq!(entry.category, TraceCategory::Structure);
        assert_eq!(
            entry.error.as_deref(),
            Some("You have an error in your SQL syntax near '********' at line 1")
        );

        // A statement without a password keeps its error as the server wrote it.
        let failed: Result<(), AppError> = trace.run(
            TraceCategory::Data,
            trace_sql(Dialect::Mysql, "SELECT hunter2", &[]),
            |_| TraceCounts::NONE,
            || Err(AppError::db("Unknown column 'hunter2'")),
        );
        assert!(failed.is_err());
        assert_eq!(
            trace.drain()[0].error.as_deref(),
            Some("Unknown column 'hunter2'")
        );
    }

    #[test]
    fn a_password_too_short_to_mask_withholds_the_message() {
        let shown = trace_sql(Dialect::Postgres, "ALTER ROLE r PASSWORD 'ab'", &[]);
        assert_eq!(shown.text, "ALTER ROLE r PASSWORD '********'");
        let message = shown.scrub("syntax error at or near \"ab\"".into());
        assert!(!message.contains("ab"), "{message}");
        assert!(message.contains("withheld"), "{message}");
        // Without a password the message passes through.
        assert_eq!(
            trace_sql(Dialect::Postgres, "SELECT 1", &[]).scrub("x".into()),
            "x"
        );
    }

    #[test]
    fn a_bound_password_is_masked_too() {
        let shown = trace_sql(
            Dialect::Postgres,
            "ALTER ROLE r WITH PASSWORD $1",
            &[Value::Text("hunter2".into())],
        );
        assert_eq!(shown.text, "ALTER ROLE r WITH PASSWORD '********'");
    }

    #[test]
    fn classifies_what_the_app_ran_as_meta_or_data() {
        use TraceCategory::{Data, Structure};
        let pg = Dialect::Postgres;
        for sql in [
            "CREATE TABLE t (a int)",
            "alter table t add column b int",
            "DROP INDEX i",
            "PRAGMA table_info(t)",
            "SHOW TABLES",
            "SELECT * FROM information_schema.tables",
            "SELECT relname FROM pg_class",
            "SELECT sql FROM sqlite_master WHERE type = 'view'",
            "SELECT pg_get_viewdef(c.oid) FROM t c",
        ] {
            assert_eq!(classify_statement(sql, pg), Structure, "{sql}");
        }
        for sql in [
            "SELECT COUNT(*) FROM users",
            "SELECT * FROM t WHERE note = 'information_schema'",
            "INSERT INTO t VALUES (1)",
            "UPDATE users SET a = 1",
            "SELECT 1 -- pg_class",
        ] {
            assert_eq!(classify_statement(sql, pg), Data, "{sql}");
        }
        assert_eq!(
            classify_statement("SELECT * FROM mysql.user", Dialect::Mysql),
            Structure
        );
    }

    fn conn_id(handle: Option<CancelHandle>) -> Option<u32> {
        match handle?.into_parts().0 {
            CancelHandle::Mysql { conn_id } => Some(conn_id),
            _ => None,
        }
    }

    #[test]
    fn cancel_slot_lifecycle() {
        // Fresh slot: arming works, the guard clears it again.
        let slot = CancelSlot::default();
        {
            let _armed = arm_cancel(&slot, CancelHandle::Mysql { conn_id: 7 }).unwrap();
            assert!(slot.is_armed());
        }
        assert!(!slot.is_armed());
        assert!(!cancel_requested(&slot));

        // Cancel while a statement runs: the live handle comes back, the request sticks.
        let armed = arm_cancel(&slot, CancelHandle::Mysql { conn_id: 9 }).unwrap();
        assert_eq!(conn_id(request_cancel(&slot)), Some(9));
        // A failed cancel can be retried while the statement still runs.
        assert_eq!(conn_id(request_cancel(&slot)), Some(9));
        drop(armed);
        assert!(cancel_requested(&slot));
        // No further statement can start with this slot.
        assert!(matches!(
            arm_cancel(&slot, CancelHandle::Mysql { conn_id: 9 }),
            Err(AppError::Cancelled)
        ));
        // Once the statement ended there is nothing left to cancel.
        assert!(request_cancel(&slot).is_none());

        // Cancel between statements: nothing to abort, but the next arm fails.
        let slot = CancelSlot::default();
        assert!(request_cancel(&slot).is_none());
        assert!(arm_cancel(&slot, CancelHandle::Mysql { conn_id: 1 }).is_err());
    }

    #[test]
    fn cancel_tickets_track_requests_in_flight() {
        let slot = CancelSlot::default();
        let armed = arm_cancel(&slot, CancelHandle::Mysql { conn_id: 3 }).unwrap();
        let first = request_cancel(&slot).unwrap();
        let second = first.clone();
        let third = request_cancel(&slot).unwrap();
        drop(armed);
        assert_eq!(slot.cancels_in_flight(), 2);
        assert!(!settle_cancels_blocking(&slot, Duration::from_millis(10)));
        drop(first);
        assert_eq!(
            slot.cancels_in_flight(),
            2,
            "a clone still holds the ticket"
        );
        drop(second);
        assert_eq!(slot.cancels_in_flight(), 1);
        // A waiter on another thread wakes up when the last request is done.
        let waiter = {
            let slot = slot.clone();
            std::thread::spawn(move || settle_cancels_blocking(&slot, Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(20));
        let (inner, ticket) = third.into_parts();
        assert!(matches!(inner, CancelHandle::Mysql { conn_id: 3 }));
        drop(ticket);
        assert!(waiter.join().unwrap());
        assert_eq!(slot.cancels_in_flight(), 0);
    }

    #[tokio::test]
    async fn settle_cancels_waits_for_the_last_ticket() {
        let slot = CancelSlot::default();
        assert!(settle_cancels(&slot, Duration::from_millis(1)).await);
        let armed = arm_cancel(&slot, CancelHandle::Requested).unwrap();
        let handle = request_cancel(&slot).unwrap();
        drop(armed);
        assert!(!settle_cancels(&slot, Duration::from_millis(20)).await);
        let started = std::time::Instant::now();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(handle);
        });
        assert!(settle_cancels(&slot, Duration::from_secs(5)).await);
        assert!(started.elapsed() >= Duration::from_millis(40));
        release.await.unwrap();
    }

    #[test]
    fn stop_safety_and_words() {
        let pg = Dialect::Postgres;
        let my = Dialect::Mysql;
        assert!(cancel_safe_on_stop("select * from big", pg));
        assert!(cancel_safe_on_stop(
            "  with x as (select 1) select * from x",
            pg
        ));
        assert!(cancel_safe_on_stop("show variables", my));
        assert!(!cancel_safe_on_stop("call p()", my));
        assert!(!cancel_safe_on_stop("fetch 100 from c", pg));
        assert!(!cancel_safe_on_stop(
            "insert into t select 1 returning id",
            pg
        ));
        assert!(!cancel_safe_on_stop(
            "with d as (delete from t returning *) select * from d",
            pg
        ));
        assert!(!cancel_safe_on_stop("select * from t for update", pg));
        // A custom-DELIMITER batch arrives as one text.
        assert!(!cancel_safe_on_stop(
            "SELECT * FROM events_big; INSERT INTO t VALUES (2)",
            my
        ));
        assert!(contains_word("A RETURNING B", "RETURNING"));
        assert!(!contains_word("XRETURNING", "RETURNING"));
        assert!(!contains_word("RETURNING_X", "RETURNING"));
        assert!(cap_cancel_notice(10, pg).contains("10 rows"));

        // Words inside literals, quoted identifiers and comments are not statements.
        for d in [pg, my] {
            assert!(cancel_safe_on_stop(
                "select * from audit_log where action = 'UPDATE'",
                d
            ));
            assert!(cancel_safe_on_stop(
                "select * from t where op <> 'DELETE'",
                d
            ));
            assert!(cancel_safe_on_stop("select 1 -- delete later\nfrom t", d));
            assert!(cancel_safe_on_stop("select 1 /* insert */ from t", d));
        }
        assert!(cancel_safe_on_stop(r#"select "update" from t"#, pg));
        assert!(cancel_safe_on_stop("select `delete` from t", my));
        assert!(cancel_safe_on_stop("select $$call$$", pg));
        // Real DML / locking clauses still count.
        assert!(!cancel_safe_on_stop(
            "select 'x' from t where a = 1 for update",
            pg
        ));
        assert!(!cancel_safe_on_stop(
            "with d as (delete from t where n = 'a' returning *) select * from d",
            pg
        ));
    }

    fn db_error(sqlstate: &str, vendor_code: Option<i64>) -> AppError {
        AppError::Db {
            message: "failed".into(),
            detail: None,
            hint: None,
            sqlstate: Some(sqlstate.into()),
            vendor_code,
            position: None,
            statement_index: None,
        }
    }

    #[test]
    fn transaction_heuristics() {
        let pg = Dialect::Postgres;
        let my = Dialect::Mysql;
        let failed = db_error("XX000", None);
        let fail = Some(&failed);
        assert_eq!(tx_effect("BEGIN", pg, None), Some(true));
        assert_eq!(tx_effect("begin", pg, fail), None);
        assert_eq!(
            tx_effect("start transaction read only", pg, None),
            Some(true)
        );
        assert_eq!(tx_effect("COMMIT", pg, None), Some(false));
        assert_eq!(tx_effect("commit", pg, fail), Some(false));
        assert_eq!(tx_effect("END", pg, None), Some(false));
        assert_eq!(tx_effect("abort", pg, None), Some(false));
        assert_eq!(tx_effect("rollback to savepoint a", pg, None), None);
        assert_eq!(tx_effect("rollback work to a", my, None), None);
        assert_eq!(tx_effect("commit prepared 'x'", pg, None), None);
        assert_eq!(tx_effect("commit and chain", pg, None), Some(true));
        assert_eq!(tx_effect("commit and no chain", my, None), Some(false));
        assert_eq!(tx_effect("prepare transaction 'x'", pg, None), Some(false));
        assert_eq!(tx_effect("select 1", pg, None), None);
        assert_eq!(tx_effect("create table t (a int)", pg, None), None);
        assert_eq!(tx_effect("create table t (a int)", my, None), Some(false));
        assert_eq!(
            tx_effect("create temporary table t (a int)", my, None),
            None
        );
        assert_eq!(tx_effect("BEGIN NOT ATOMIC SELECT 1; END", my, None), None);

        // `… AND CHAIN` that failed (outside a transaction block) opens nothing.
        let not_in_tx = db_error("25P01", None);
        assert_eq!(
            tx_effect("commit and chain", pg, Some(&not_in_tx)),
            Some(false)
        );
        assert_eq!(
            tx_effect("rollback and chain", my, Some(&not_in_tx)),
            Some(false)
        );
        // MySQL DDL with a syntax error never reaches its implicit commit…
        let parse = db_error("42000", Some(1064));
        assert_eq!(
            tx_effect("CREATE TABLE t2 (id int,)", my, Some(&parse)),
            None
        );
        let syntax = db_error("42000", Some(1149));
        assert_eq!(tx_effect("drop table", my, Some(&syntax)), None);
        // …but one that fails while executing has committed already.
        let exists = db_error("42S01", Some(1050));
        assert_eq!(
            tx_effect("CREATE TABLE t (id int)", my, Some(&exists)),
            Some(false)
        );
        assert!(is_dml("  insert into t values (1)"));
        assert!(is_dml("UPDATE t SET a = 1"));
        assert!(!is_dml("create table t (a int)"));
    }

    #[test]
    fn core_dialect_needs_a_core_dialect_and_driver() {
        let none: &[Feature] = &[];
        // The core drivers get every core-only feature of their own dialect.
        for (driver, dialect) in [
            (Driver::Postgres, Dialect::Postgres),
            (Driver::Mysql, Dialect::Mysql),
            (Driver::Sqlite, Dialect::Sqlite),
        ] {
            for f in [Feature::StructureEdit, Feature::Users, Feature::ExportSql] {
                assert_eq!(
                    core_dialect_for(driver, none, dialect.into(), f).unwrap(),
                    dialect
                );
            }
        }
        let unsupported = |r: Result<Dialect, AppError>| match r {
            Err(AppError::Unsupported(m)) => m,
            other => panic!("{other:?}"),
        };
        // An extension dialect, even on a core driver (Redshift is a PostgreSQL flavor).
        assert_eq!(
            unsupported(core_dialect_for(
                Driver::Postgres,
                none,
                SqlDialect::Redshift,
                Feature::StructureEdit
            )),
            "Redshift SQL connections do not offer StructureEdit"
        );
        // A driver beyond the core three that speaks a core dialect: only what its descriptor
        // lets it reuse.
        assert_eq!(
            unsupported(core_dialect_for(
                Driver::Libsql,
                none,
                SqlDialect::Sqlite,
                Feature::Backup
            )),
            "LibSQL connections do not offer Backup"
        );
        let reuse = &[Feature::Diagram];
        assert_eq!(
            core_dialect_for(Driver::Libsql, reuse, SqlDialect::Sqlite, Feature::Diagram).unwrap(),
            Dialect::Sqlite
        );
        assert!(
            core_dialect_for(Driver::Libsql, reuse, SqlDialect::Sqlite, Feature::Users).is_err()
        );
        // An allowlist never opens a core generator to an extension dialect.
        assert!(
            core_dialect_for(Driver::Duckdb, reuse, SqlDialect::Duckdb, Feature::Diagram).is_err()
        );
        // Every descriptor of v1 reuses nothing: no other driver reaches a core generator.
        for driver in Driver::ALL.into_iter().filter(|d| !d.is_core()) {
            let core = [SqlDialect::Postgres, SqlDialect::Mysql, SqlDialect::Sqlite];
            for dialect in core {
                assert!(
                    core_dialect_for(
                        driver,
                        driver.descriptor().core_reuse,
                        dialect,
                        Feature::Diagram
                    )
                    .is_err(),
                    "{driver:?}"
                );
            }
        }
    }

    #[test]
    fn check_affected_refuses_an_unconfirmed_count() {
        let guarded = TxStatement::new("DELETE FROM t WHERE id = 1", Vec::new()).expecting(1);
        let exact = ExecOutcome {
            affected: 1,
            ..ExecOutcome::default()
        };
        check_affected(&guarded, &exact, 0).unwrap();
        // A count the server did not confirm never passes a guard, whatever it says.
        let unknown = ExecOutcome {
            affected: 1,
            affected_unknown: true,
            ..ExecOutcome::default()
        };
        let err = check_affected(&guarded, &unknown, 3).unwrap_err();
        assert!(err.to_string().contains("cannot confirm"), "{err}");
        assert_eq!(err.to_payload().statement_index, Some(3));
        // Without a guard nothing is checked.
        let plain = TxStatement::new("INSERT INTO t VALUES (1)", Vec::new());
        check_affected(&plain, &unknown, 0).unwrap();
        // A wrong count is still the old error.
        let wrong = ExecOutcome {
            affected: 2,
            ..ExecOutcome::default()
        };
        let err = check_affected(&guarded, &wrong, 0).unwrap_err();
        assert!(err.to_string().contains("matched 2"), "{err}");
        // The flag is left out of the JSON unless set, and absent reads as false.
        assert!(
            serde_json::to_value(&exact)
                .unwrap()
                .get("affectedUnknown")
                .is_none()
        );
        let back: ExecOutcome = serde_json::from_value(
            serde_json::json!({ "affected": 1, "lastInsertId": null, "rows": null }),
        )
        .unwrap();
        assert!(!back.affected_unknown);
        assert_eq!(
            serde_json::to_value(&unknown).unwrap()["affectedUnknown"],
            serde_json::json!(true)
        );
    }

    /// The helpers keep the core dialects' behaviour and read an extension dialect's spec.
    #[test]
    fn the_trace_and_editor_helpers_read_an_extension_spec() {
        use crate::sqlgen::dialect::DialectSpec;
        use crate::sqlgen::dialect::spec::SqlKnobs;
        use crate::sqlgen::dialect::testing::override_spec;
        use crate::sqlgen::portable::testing::ANSI_TEST;

        fn commits(sql: &str) -> bool {
            leading_keywords(sql, 1)
                .first()
                .is_some_and(|w| w == "CREATE")
        }
        let spec: &'static DialectSpec = Box::leak(Box::new(DialectSpec {
            knobs: SqlKnobs {
                implicit_commit: commits,
                catalog_words: &["V_CATALOG"],
                cancel_notice: "what it changed stays changed",
                ..ANSI_TEST.knobs
            },
            ..ANSI_TEST
        }));
        let _vertica = override_spec(SqlDialect::Vertica, spec);
        let v = SqlDialect::Vertica;
        // The catalog words are the spec's, not the core ones.
        assert_eq!(
            classify_statement("SELECT * FROM v_catalog.tables", v),
            TraceCategory::Structure
        );
        assert_eq!(
            classify_statement("SELECT * FROM pg_catalog.pg_class", v),
            TraceCategory::Data
        );
        assert_eq!(
            classify_statement("SELECT 'v_catalog' FROM t", v),
            TraceCategory::Data
        );
        assert_eq!(
            classify_statement("CREATE TABLE t (a int)", v),
            TraceCategory::Structure
        );
        // Its implicit commits close the transaction once they ran.
        assert_eq!(tx_effect("create table t (a int)", v, None), Some(false));
        let failed = AppError::db("no");
        assert_eq!(tx_effect("create table t (a int)", v, Some(&failed)), None);
        assert_eq!(tx_effect("insert into t values (1)", v, None), None);
        assert_eq!(tx_effect("begin", v, None), Some(true));
        assert_eq!(tx_effect("commit", v, None), Some(false));
        // MySQL's implicit commits are MySQL's only.
        assert_eq!(tx_effect("drop table t", v, None), None);
        assert_eq!(tx_effect("drop table t", Dialect::Mysql, None), Some(false));
        // The row-cap notice ends with the spec's words.
        assert!(
            cap_cancel_notice(3, v)
                .ends_with("so it did not finish and what it changed stays changed.")
        );
        assert!(
            cap_cancel_notice(3, Dialect::Postgres)
                .ends_with("any changes it made were rolled back.")
        );
        // Single statements by the spec's splitter; code only by its lexer.
        assert!(cancel_safe_on_stop("SELECT 'a;b' FROM t", v));
        assert!(!cancel_safe_on_stop("SELECT 1; SELECT 2", v));
        assert!(!cancel_safe_on_stop(
            "WITH d AS (DELETE FROM t) SELECT 1",
            v
        ));
        // Passwords masked by the spec's rules; parameters inlined with its literals.
        let shown = trace_sql(v, "ALTER USER u IDENTIFIED BY 'hunter2'", &[]);
        assert!(!shown.text.contains("hunter2"), "{}", shown.text);
        assert_eq!(
            trace_text(v, "SELECT ? FROM t", &[Value::Text("it's".into())]),
            "SELECT 'it''s' FROM t"
        );
    }
}
