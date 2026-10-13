//! Database drivers behind one trait. Each driver owns its connections; commands only see
//! `Arc<dyn DbSession>`.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::split::leading_keywords;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExecOutcome, ResultColumn, ResultSet, ServerNotice, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub mod mysql;
pub mod postgres;
pub mod sqlite;

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
pub fn tx_effect(sql: &str, dialect: Dialect, error: Option<&AppError>) -> Option<bool> {
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
            if dialect == Dialect::Mysql =>
        {
            let temporary = matches!(w(0), "CREATE" | "DROP") && w(1) == "TEMPORARY";
            if temporary || error.is_some_and(is_mysql_parse_error) {
                None
            } else {
                Some(false)
            }
        }
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
pub fn cancel_safe_on_stop(sql: &str, dialect: Dialect) -> bool {
    let first = leading_keywords(sql, 1);
    if !matches!(
        first.first().map(String::as_str),
        Some("SELECT" | "WITH" | "TABLE" | "VALUES" | "SHOW")
    ) {
        return false;
    }
    if !crate::sqlgen::split::is_single_statement(sql, dialect) {
        return false;
    }
    let upper = crate::sqlgen::split::code_only(sql, dialect).to_uppercase();
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
pub fn cap_cancel_notice(rows: u64, dialect: Dialect) -> String {
    let undone = match dialect {
        Dialect::Mysql => "changes it made to transactional tables were rolled back",
        _ => "any changes it made were rolled back",
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
    fn dialect(&self) -> Dialect;

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
    /// Runs a statement for its side effects (optionally with RETURNING rows).
    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError>;
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

    /// Lock a whole editor run holds (`lock_editor_runs`), so the statements of concurrent runs
    /// on the shared editor session never interleave. `run_statement` itself does not take it.
    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        None
    }

    /// MariaDB rather than MySQL (EXPLAIN syntax differs).
    fn is_mariadb(&self) -> bool {
        false
    }
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

/// Shared check used by every driver inside `transaction`.
pub fn check_affected(
    st: &TxStatement,
    outcome: &ExecOutcome,
    index: usize,
) -> Result<(), AppError> {
    match st.expect_affected {
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
                .or_else(|| self.profile.driver.default_port())
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

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    match target.profile.driver {
        Driver::Sqlite => Ok(Arc::new(sqlite::LiteSession::open(target.profile).await?)),
        Driver::Postgres => postgres::connect(target).await,
        Driver::Mysql => mysql::connect(target).await,
    }
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
}
