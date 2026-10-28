//! One DuckDB connection on a thread of its own, and the execution of statements on it.
//!
//! `::duckdb::Connection` is `Send` but not `Sync`, so it lives on a dedicated thread that runs
//! jobs in order (like the SQLite actor). Rows travel back to the async caller through a small
//! bounded channel. A statement runs streaming: chunks are fetched as the sink takes them, and a
//! row cap or a full sink drops the result, which ends the query. Cancel interrupts the
//! connection (`InterruptHandle`), armed only while this job's statement runs.
//!
//! Nothing here asks duckdb-rs for a column's type or name before the statement has executed
//! (those accessors panic until then), and rows are decoded from the Arrow chunks by
//! `decode::cell`, never through `Row::get_ref` (see `decode`).

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ::duckdb::arrow::array::Array;
use ::duckdb::{Connection, InterruptHandle};
use futures::future::BoxFuture;
use tokio::sync::mpsc;

use super::decode::{self, Bytes};
use super::engine::Instance;
use super::errors;
use super::types::DuckType;
use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelAction, CancelHandle, CancelSlot, CollectSink, RowChunker, RowSink,
    StatementOutcome, TxState, arm_cancel, cancel_requested, settle_cancels_blocking, tx_effect,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::duckdb::LEX;
use crate::sqlgen::dialect::lex::{TokenKind, scan};
use crate::sqlgen::portable::split::first_reading;
use crate::types::query::{ResultColumn, StatementKind};
use crate::types::value::Value;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// How long [`Worker::close`] waits for the thread to end.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// How often (in rows) a running statement looks for a cancel request between chunks.
const CANCEL_CHECK_EVERY: u64 = 256;
/// Chunks in flight between the connection thread and the async side.
const CHANNEL_DEPTH: usize = 4;

/// A dedicated thread owning one connection; jobs are closures run in order.
pub struct Worker {
    tx: Mutex<Option<std_mpsc::Sender<Job>>>,
    interrupt: Arc<InterruptHandle>,
    /// Set by `close`: jobs still queued are answered with `NotConnected` instead of running.
    closed: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Keeps the database open while the connection is (and its settings known).
    instance: Arc<Instance>,
}

impl Worker {
    /// A new connection of `instance` on its own thread.
    pub fn spawn(instance: Arc<Instance>) -> Result<Worker, AppError> {
        let conn = instance.connect()?;
        let interrupt = conn.interrupt_handle();
        let (tx, rx) = std_mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("tabler-duckdb".into())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
                // The connection closes here, on its thread; the file closes with the last one.
                drop(conn);
            })
            .map_err(|e| AppError::Internal(format!("spawn duckdb thread: {e}")))?;
        Ok(Worker {
            tx: Mutex::new(Some(tx)),
            interrupt,
            closed: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(Some(thread)),
            instance,
        })
    }

    pub fn instance(&self) -> &Arc<Instance> {
        &self.instance
    }

    /// Runs `f` on the connection thread and awaits its result.
    pub async fn run<R, F>(&self, f: F) -> Result<R, AppError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<R, AppError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let closed = self.closed.clone();
        let job: Job = Box::new(move |conn| {
            let result = if closed.load(Ordering::SeqCst) {
                Err(AppError::NotConnected)
            } else {
                f(conn)
            };
            let _ = reply_tx.send(result);
        });
        {
            let guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
            match guard.as_ref() {
                Some(tx) => tx.send(job).map_err(|_| AppError::NotConnected)?,
                None => return Err(AppError::NotConnected),
            }
        }
        reply_rx.await.map_err(|_| AppError::NotConnected)?
    }

    /// Interrupts the statement running on this connection (safe from any thread).
    pub fn interrupt(&self) {
        self.interrupt.interrupt();
    }

    pub fn interrupt_handle(&self) -> Arc<InterruptHandle> {
        self.interrupt.clone()
    }

    /// Stops the running statement, fails queued jobs with `NotConnected`, ends the thread
    /// (closing the connection) and waits for that, at most 2 s.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.interrupt.interrupt();
        self.tx.lock().unwrap_or_else(|p| p.into_inner()).take();
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(handle) = handle else {
            return;
        };
        if handle.thread().id() == thread::current().id() {
            return;
        }
        let deadline = Instant::now() + CLOSE_WAIT;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                log::warn!("duckdb: the connection thread did not end within {CLOSE_WAIT:?}");
                *self.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let _ = handle.join();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
    }
}

/// Cancel: interrupt the connection the statement runs on.
pub struct Interrupt(pub Arc<InterruptHandle>);

impl CancelAction for Interrupt {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        let handle = self.0.clone();
        Box::pin(async move {
            handle.interrupt();
            Ok(())
        })
    }

    fn label(&self) -> &'static str {
        "duckdb interrupt"
    }
}

/// The first words of statements whose rows are their result: queries, inspections, and the
/// ones that return a table (`CALL`, `PRAGMA`, `EXECUTE` of a prepared query).
const ROW_STATEMENTS: &[&str] = &[
    "SELECT",
    "WITH",
    "VALUES",
    "TABLE",
    "FROM",
    "PIVOT",
    "UNPIVOT",
    "PIVOT_WIDER",
    "PIVOT_LONGER",
    "SHOW",
    "DESCRIBE",
    "DESC",
    "SUMMARIZE",
    "EXPLAIN",
    "CALL",
    "PRAGMA",
    "EXECUTE",
];

/// Whether a statement's single `Count` (BIGINT) or `Success` (BOOLEAN) column is DuckDB's
/// status of a statement rather than rows: DML, COPY and DDL answer that way (a DML with
/// RETURNING answers with the rows it returns, and a WITH may lead into an INSERT).
fn status_result(sql: &str, name: &str, ty: &DuckType) -> bool {
    let status_column = (name == "Count" && *ty == DuckType::BigInt)
        || (name == "Success" && *ty == DuckType::Boolean);
    if !status_column {
        return false;
    }
    let s = scan(&LEX, first_reading(&LEX), sql);
    let mut words = s
        .tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| s.upper(t));
    let first = words.next().unwrap_or_default();
    let mut dml = false;
    for w in words {
        match w.as_str() {
            "RETURNING" => return false,
            "INSERT" | "UPDATE" | "DELETE" | "MERGE" => dml = true,
            _ => {}
        }
    }
    dml || !ROW_STATEMENTS.contains(&first.as_str())
}

/// Runs one statement on `conn`, streaming its rows into `sink`. Runs on the connection thread.
pub fn execute_streaming(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, AppError> {
    if cancel_requested(cancel) {
        return Err(AppError::Cancelled);
    }
    let bound = params
        .iter()
        .map(decode::to_param)
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(sql).map_err(errors::map)?;
    let schema = {
        let stream = stmt
            .stream_arrow(::duckdb::params_from_iter(bound.iter()))
            .map_err(errors::map)?;
        // Dropped at once: its iterator panics on a fetch error, `step` returns it.
        stream.get_schema()
    };
    let types: Vec<DuckType> = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| decode::column_type(DuckType::from_logical(&stmt.column_logical_type(i)), f))
        .collect();
    let columns: Vec<ResultColumn> = schema
        .fields()
        .iter()
        .zip(&types)
        .map(|(f, t)| decode::result_column(f.name(), t))
        .collect();
    let mut outcome = StatementOutcome::default();
    if columns.len() == 1 && status_result(sql, &columns[0].name, &types[0]) {
        // A count of changed rows (DML, COPY, CREATE TABLE … AS), or nothing (other DDL,
        // SET): DuckDB sends the count as a row, and no row at all for a statement with none.
        let mut affected: Option<u64> = None;
        while let Some(chunk) = stmt.step().map_err(errors::map)? {
            if let Some(col) = chunk.columns().first() {
                for row in 0..col.len() {
                    if let Value::Int(n) =
                        decode::cell(col.as_ref(), row, &types[0], Bytes::Preview)
                    {
                        *affected.get_or_insert(0) += u64::try_from(n).unwrap_or(0);
                    }
                }
            }
        }
        if types[0] == DuckType::BigInt && (affected.is_some() || crate::drivers::is_dml(sql)) {
            outcome.kind = StatementKind::Affected;
            outcome.affected = Some(affected.unwrap_or(0));
        }
        return Ok(outcome);
    }
    if columns.is_empty() {
        while stmt.step().map_err(errors::map)?.is_some() {}
        return Ok(outcome);
    }
    let mut chunker = RowChunker::new(sink);
    let _ = chunker.begin_set(columns);
    outcome.kind = StatementKind::Rows;
    outcome.result_sets = 1;
    'chunks: loop {
        let chunk = match stmt.step() {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => {
                let _ = chunker.flush();
                return Err(errors::map(e));
            }
        };
        let cols = chunk.columns();
        for row in 0..chunk.len() {
            if let Some(cap) = max_rows
                && chunker.rows_in_set as usize >= cap
            {
                outcome.truncated = true;
                break 'chunks;
            }
            let cells = types
                .iter()
                .enumerate()
                .map(|(i, t)| match cols.get(i) {
                    Some(col) => decode::cell(col.as_ref(), row, t, Bytes::Preview),
                    None => Value::other("missing", ""),
                })
                .collect();
            if chunker.push(cells).is_break() {
                outcome.truncated = true;
                break 'chunks;
            }
            if chunker.total_rows.is_multiple_of(CANCEL_CHECK_EVERY) && cancel_requested(cancel) {
                let _ = chunker.flush();
                return Err(AppError::Cancelled);
            }
        }
    }
    let _ = chunker.flush();
    outcome.row_count = chunker.total_rows;
    Ok(outcome)
}

/// Runs a query to the end on `conn` and returns its columns and rows (catalog reads, tests).
pub fn query_all(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<(Vec<ResultColumn>, Vec<Vec<Value>>), AppError> {
    let mut sink = CollectSink::default();
    execute_streaming(conn, sql, params, None, &mut sink, &CancelSlot::default())?;
    match sink.sets.into_iter().next() {
        Some(set) => Ok((set.columns, set.rows)),
        None => Ok((Vec::new(), Vec::new())),
    }
}

/// The value of one cell, whole (`fetch_cell`): the first column of the first row.
pub fn fetch_whole(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Option<Value>, AppError> {
    let bound = params
        .iter()
        .map(decode::to_param)
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(sql).map_err(errors::map)?;
    let schema = {
        let stream = stmt
            .stream_arrow(::duckdb::params_from_iter(bound.iter()))
            .map_err(errors::map)?;
        stream.get_schema()
    };
    let Some(field) = schema.fields().first() else {
        return Ok(None);
    };
    let ty = decode::column_type(DuckType::from_logical(&stmt.column_logical_type(0)), field);
    while let Some(chunk) = stmt.step().map_err(errors::map)? {
        if let Some(col) = chunk.columns().first()
            && !col.is_empty()
        {
            return Ok(Some(decode::cell(col.as_ref(), 0, &ty, Bytes::Whole)));
        }
    }
    Ok(None)
}

/// Messages from the connection thread to the async side.
enum Msg {
    Columns(Vec<ResultColumn>),
    Rows(Vec<Vec<Value>>),
}

/// The thread-side sink: forwards to the async side, `Break` once the receiver is gone.
struct ChannelSink {
    tx: mpsc::Sender<Msg>,
}

impl RowSink for ChannelSink {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        let _ = self.tx.blocking_send(Msg::Columns(columns));
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        match self.tx.blocking_send(Msg::Rows(rows)) {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        }
    }
}

/// Runs `sql` on `worker`, streaming rows into the async-side `sink`. `tx_state` (the editor's
/// connection) records whether a transaction is open afterwards.
///
/// The interrupt handle goes into `cancel` only while this job's statement runs on the
/// connection thread, and before the job hands the connection on it waits for any cancel
/// already given out, so a late interrupt cannot stop the next job's statement.
pub async fn run(
    worker: &Worker,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
    tx_state: Option<Arc<TxState>>,
) -> Result<StatementOutcome, AppError> {
    let (tx, mut rx) = mpsc::channel::<Msg>(CHANNEL_DEPTH);
    let sql = sql.to_string();
    let params = params.to_vec();
    let interrupt = worker.interrupt_handle();
    let job = worker.run(move |conn| {
        let mut chan = ChannelSink { tx };
        let action: Arc<dyn CancelAction> = Arc::new(Interrupt(interrupt));
        let result = match arm_cancel(&cancel, CancelHandle::Custom(action)) {
            Ok(armed) => {
                let result = execute_streaming(conn, &sql, &params, max_rows, &mut chan, &cancel);
                drop(armed);
                if !settle_cancels_blocking(&cancel, CANCEL_SETTLE_LIMIT) {
                    log::warn!("duckdb: a cancel request did not finish in time");
                }
                result
            }
            Err(e) => Err(e),
        };
        // duckdb-rs's `is_autocommit` always answers true, so the statement tells.
        if let Some(state) = tx_state
            && let Some(open) = tx_effect(&sql, SqlDialect::Duckdb, result.as_ref().err())
        {
            state.set(Some(open));
        }
        result
    });
    tokio::pin!(job);

    let mut stopped = false;
    let mut deliver = |msg: Msg, rx: &mut mpsc::Receiver<Msg>| {
        if stopped {
            return;
        }
        match msg {
            Msg::Columns(columns) => sink.begin_set(columns),
            Msg::Rows(rows) => {
                if sink.rows(rows).is_break() {
                    stopped = true;
                    rx.close();
                }
            }
        }
    };
    let mut open = true;
    let result = loop {
        tokio::select! {
            msg = rx.recv(), if open => match msg {
                Some(msg) => deliver(msg, &mut rx),
                None => open = false,
            },
            result = &mut job => break result,
        }
    };
    while let Ok(msg) = rx.try_recv() {
        deliver(msg, &mut rx);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_columns_of_statements_that_return_no_rows() {
        let count = |sql: &str| status_result(sql, "Count", &DuckType::BigInt);
        assert!(count("INSERT INTO t VALUES (1)"));
        assert!(count("update t set a = 1"));
        assert!(count("WITH s AS (SELECT 1) DELETE FROM t"));
        assert!(count("COPY t FROM 'x.csv'"));
        assert!(count("CREATE TABLE t AS SELECT 1"));
        assert!(count("TRUNCATE t"));
        assert!(status_result(
            "SET threads = 1",
            "Success",
            &DuckType::Boolean
        ));
        assert!(!count("INSERT INTO t VALUES (1) RETURNING id"));
        assert!(!count("SELECT 1 AS \"Count\""));
        assert!(!count("SELECT 'INSERT' AS \"Count\""));
        assert!(!count("PRAGMA table_info('t')"));
        assert!(!status_result(
            "CREATE TABLE t (a INT)",
            "Count",
            &DuckType::Integer
        ));
    }
}
