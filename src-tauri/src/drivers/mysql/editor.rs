//! SQL-editor execution and row streaming for MySQL / MariaDB.
//!
//! Statements without parameters use the text protocol (`query_iter`), which also returns every
//! result set of a `CALL`; parameterised ones the binary protocol (`exec_iter`). Rows are
//! decoded per result set and streamed in chunks. Cancelling is `KILL QUERY <id>` sent from
//! another connection; the interrupted statement fails with error 1317 (`Cancelled`).
//!
//! Nothing else runs on the caller's connection between its statements, except `DO 0` after a
//! cancel (see `absorb_stray_kill`) and `SHOW WARNINGS` after a row-returning statement that
//! raised warnings: both reset `ROW_COUNT()`, which only matters after statements that do not
//! return rows, so those report their warning count instead.

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use mysql_async::prelude::Queryable;
use mysql_async::{
    BinaryProtocol, Column, Conn, Opts, Params, Pool, QueryResult, Row, TextProtocol,
};

use super::decode::{kind_from_column, result_column, text_of, value_from_mysql};
use super::params::bind;
use crate::drivers::{
    RowChunker, RowSink, StatementOutcome, cancel_safe_on_stop, cap_cancel_notice, is_dml,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::query::StatementKind;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// How long a cancel waits for a free pooled connection before opening a dedicated one. Short:
/// the statement keeps running (and its run keeps the connection) until the KILL lands.
const KILL_POOL_WAIT: Duration = Duration::from_millis(500);
/// Bound on opening that dedicated connection and on the `KILL` itself.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// Warnings fetched with `SHOW WARNINGS` after a statement that raised some.
const MAX_WARNINGS: usize = 50;

/// Common view over text and binary `QueryResult`s (`mysql_async::Protocol` is private).
trait Cursor: Send {
    fn columns(&self) -> Option<Arc<[Column]>>;
    fn is_empty(&self) -> bool;
    fn affected_rows(&self) -> u64;
    fn last_insert_id(&self) -> Option<u64>;
    fn warnings(&self) -> u16;
    /// Next row of the current result set; `None` at its end (and moves to the next set).
    fn next_row(&mut self) -> BoxFuture<'_, mysql_async::Result<Option<Row>>>;
}

macro_rules! impl_cursor {
    ($proto:ty) => {
        impl Cursor for QueryResult<'_, '_, $proto> {
            fn columns(&self) -> Option<Arc<[Column]>> {
                QueryResult::columns(self)
            }
            fn is_empty(&self) -> bool {
                QueryResult::is_empty(self)
            }
            fn affected_rows(&self) -> u64 {
                QueryResult::affected_rows(self)
            }
            fn last_insert_id(&self) -> Option<u64> {
                QueryResult::last_insert_id(self)
            }
            fn warnings(&self) -> u16 {
                QueryResult::warnings(self)
            }
            fn next_row(&mut self) -> BoxFuture<'_, mysql_async::Result<Option<Row>>> {
                self.next().boxed()
            }
        }
    };
}

impl_cursor!(TextProtocol);
impl_cursor!(BinaryProtocol);

/// Sends `KILL QUERY` for one server connection.
pub(crate) struct Killer<'a> {
    pub pool: &'a Pool,
    pub opts: &'a Opts,
    pub conn_id: u32,
}

impl Killer<'_> {
    pub(crate) async fn kill(&self) -> Result<(), AppError> {
        kill_query(self.pool, self.opts, self.conn_id).await
    }
}

/// `KILL QUERY <conn_id>` from a pooled connection (or a fresh one when the pool is exhausted,
/// e.g. by the very statements being cancelled). An unknown thread id means nothing to cancel.
pub(crate) async fn kill_query(pool: &Pool, opts: &Opts, conn_id: u32) -> Result<(), AppError> {
    kill(pool, opts, &format!("KILL QUERY {conn_id}")).await
}

/// `KILL CONNECTION <conn_id>`: ends the server thread (and the statement it runs).
pub(crate) async fn kill_connection(
    pool: &Pool,
    opts: &Opts,
    conn_id: u32,
) -> Result<(), AppError> {
    kill(pool, opts, &format!("KILL CONNECTION {conn_id}")).await
}

async fn kill(pool: &Pool, opts: &Opts, statement: &str) -> Result<(), AppError> {
    let mut conn = match tokio::time::timeout(KILL_POOL_WAIT, pool.get_conn()).await {
        Ok(Ok(conn)) => conn,
        _ => tokio::time::timeout(KILL_TIMEOUT, Conn::new(opts.clone()))
            .await
            .map_err(|_| AppError::Timeout)??,
    };
    match tokio::time::timeout(KILL_TIMEOUT, conn.query_drop(statement)).await {
        Err(_) => Err(AppError::Timeout),
        Ok(Ok(())) => Ok(()),
        // Unknown thread id: nothing to cancel.
        Ok(Err(mysql_async::Error::Server(e))) if e.code == 1094 => Ok(()),
        Ok(Err(e)) => Err(e.into()),
    }
}

/// A `KILL QUERY` that arrived after its statement had finished may linger on the connection
/// and interrupt the next statement; a no-op statement absorbs it.
pub(crate) async fn absorb_stray_kill(conn: &mut Conn) {
    if let Err(e) = conn.query_drop("DO 0").await {
        log::debug!("mysql: absorbing stray kill: {e}");
    }
}

/// Runs `sql` on `conn` and streams its rows into `sink`.
///
/// With `max_rows`, each result set shows at most that many rows. A single read-only query
/// (`cancel_safe_on_stop`) is then killed server-side (`killer`) and the rest drained so the
/// connection can be reused; when the server confirms the kill, a notice says the statement did
/// not finish. Any other statement (`CALL`, a custom-`DELIMITER` batch…) keeps running: the
/// surplus rows are read and dropped, later result sets are still delivered, and the statement
/// does all its work. A sink that returns `Break` (exports) stops reading the same way, without
/// the notice. `collect_warnings` adds the statement's warnings as notices.
pub(crate) async fn run(
    conn: &mut Conn,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
    sink: &mut dyn RowSink,
    killer: &Killer<'_>,
    collect_warnings: bool,
) -> Result<StatementOutcome, AppError> {
    let consumed = if params.is_empty() {
        let mut result = conn.query_iter(sql).await?;
        consume(&mut result, sql, max_rows, sink, killer).await
    } else {
        let bound = bind(params)?;
        let mut result = conn.exec_iter(sql, Params::Positional(bound)).await?;
        consume(&mut result, sql, max_rows, sink, killer).await
    };
    let (mut outcome, consumed) = consumed?;
    if consumed.killed && !consumed.saw_cancel {
        absorb_stray_kill(conn).await;
    }
    if collect_warnings && consumed.warnings > 0 {
        if consumed.saw_ok {
            // `SHOW WARNINGS` would turn the `ROW_COUNT()` the user may read next into -1.
            outcome
                .notices
                .push(warning_count_notice(consumed.warnings));
        } else {
            // After a result set `ROW_COUNT()` is -1 anyway.
            outcome.notices.extend(show_warnings(conn).await);
        }
    }
    Ok(outcome)
}

fn warning_count_notice(n: u16) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} warning{plural} (run SHOW WARNINGS to list them)")
}

struct Consumed {
    warnings: u16,
    /// An OK packet (DML, DDL, `CALL` status) was among the results.
    saw_ok: bool,
    killed: bool,
    saw_cancel: bool,
}

/// Errors a statement fails with when `KILL QUERY` stops it (1028: a sort was interrupted).
fn is_kill_error(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Cancelled
            | AppError::Db {
                vendor_code: Some(1028),
                ..
            }
    )
}

async fn consume(
    result: &mut dyn Cursor,
    sql: &str,
    max_rows: Option<usize>,
    sink: &mut dyn RowSink,
    killer: &Killer<'_>,
) -> Result<(StatementOutcome, Consumed), AppError> {
    // Decided once, when a statement first has to stop early.
    let mut safe_to_kill: Option<bool> = None;
    let mut kill_on_stop =
        || *safe_to_kill.get_or_insert_with(|| cancel_safe_on_stop(sql, Dialect::Mysql));
    let mut chunker = RowChunker::new(sink);
    let mut outcome = StatementOutcome::default();
    let mut consumed = Consumed {
        warnings: 0,
        saw_ok: false,
        killed: false,
        saw_cancel: false,
    };
    let mut affected: Option<u64> = None;
    let mut last_insert_id: Option<u64> = None;
    // Reading stopped altogether: `Some(true)` at the row cap, `Some(false)` on a sink `Break`.
    let mut stopped: Option<bool> = None;

    loop {
        match result.columns() {
            Some(columns) if !columns.is_empty() => {
                let described: Vec<_> = columns.iter().map(result_column).collect();
                let kinds: Vec<ValueKind> = columns.iter().map(kind_from_column).collect();
                outcome.kind = StatementKind::Rows;
                outcome.result_sets += 1;
                if chunker.begin_set(described).is_break() {
                    stopped = Some(false);
                    break;
                }
                // Rows past the cap of this set are read and dropped.
                let mut skipping = false;
                loop {
                    let row = match result.next_row().await {
                        Ok(Some(row)) => row,
                        Ok(None) => break,
                        Err(e) => {
                            let _ = chunker.flush();
                            return Err(e.into());
                        }
                    };
                    if skipping {
                        continue;
                    }
                    if let Some(cap) = max_rows
                        && chunker.rows_in_set as usize >= cap
                    {
                        outcome.truncated = true;
                        if kill_on_stop() {
                            stopped = Some(true);
                            break;
                        }
                        skipping = true;
                        continue;
                    }
                    let cells = row
                        .unwrap()
                        .into_iter()
                        .zip(columns.iter())
                        .zip(kinds.iter())
                        .map(|((v, col), kind)| value_from_mysql(*kind, col, v))
                        .collect();
                    if chunker.push(cells).is_break() {
                        outcome.truncated = true;
                        stopped = Some(false);
                        break;
                    }
                }
                if stopped.is_some() {
                    break;
                }
                consumed.warnings = consumed.warnings.max(result.warnings());
            }
            _ => {
                // OK packet (DML, DDL, the status result of CALL): counters are final now.
                consumed.saw_ok = true;
                affected = Some(affected.unwrap_or(0) + result.affected_rows());
                if let Some(id) = result.last_insert_id().filter(|id| *id != 0) {
                    last_insert_id = Some(id);
                }
                consumed.warnings = consumed.warnings.max(result.warnings());
                result.next_row().await?;
            }
        }
        if result.is_empty() {
            break;
        }
    }
    let _ = chunker.flush();
    outcome.row_count = chunker.total_rows;

    if let Some(at_cap) = stopped
        && !result.is_empty()
    {
        if kill_on_stop() {
            consumed.killed = true;
            if let Err(e) = killer.kill().await {
                log::warn!("mysql: KILL QUERY for a stopped statement failed: {e}");
            }
        }
        let mut errors = 0;
        while !result.is_empty() {
            match result.next_row().await {
                Ok(_) => {}
                Err(e) => {
                    let e = AppError::from(e);
                    if matches!(e, AppError::NotConnected | AppError::Connect(_)) {
                        return Err(e);
                    }
                    if consumed.killed && is_kill_error(&e) {
                        consumed.saw_cancel = true;
                    } else if at_cap {
                        // The statement failed on its own while its rows were being dropped.
                        return Err(e);
                    } else {
                        log::debug!("mysql: error while draining a stopped statement: {e}");
                    }
                    errors += 1;
                    if errors > 2 {
                        break;
                    }
                }
            }
        }
        if at_cap && consumed.saw_cancel {
            outcome
                .notices
                .push(cap_cancel_notice(outcome.row_count, Dialect::Mysql));
        }
    }

    if outcome.kind != StatementKind::Rows {
        let n = affected.unwrap_or(0);
        if affected.is_some() && (n > 0 || is_dml(sql)) {
            outcome.kind = StatementKind::Affected;
            outcome.affected = Some(n);
        }
    } else if is_dml(sql) {
        outcome.affected = affected;
    }
    outcome.last_insert_id = last_insert_id.map(|id| id.to_string());
    Ok((outcome, consumed))
}

/// `SHOW WARNINGS` as `"Warning 1265: Data truncated…"` lines.
async fn show_warnings(conn: &mut Conn) -> Vec<String> {
    let rows: Vec<Row> = match conn.query("SHOW WARNINGS").await {
        Ok(rows) => rows,
        Err(e) => {
            log::debug!("mysql: SHOW WARNINGS failed: {e}");
            return Vec::new();
        }
    };
    rows.into_iter()
        .take(MAX_WARNINGS)
        .map(|row| {
            let mut cells = row.unwrap().into_iter().map(text_of);
            let level = cells.next().flatten().unwrap_or_default();
            let code = cells.next().flatten().unwrap_or_default();
            let message = cells.next().flatten().unwrap_or_default();
            format!("{level} {code}: {message}")
        })
        .collect()
}
