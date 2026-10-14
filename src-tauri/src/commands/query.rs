//! SQL editor commands: run a script statement by statement with streamed results, cancel it,
//! explain a statement. The orchestration lives in `run_sql_inner` / `explain_inner`, which
//! only need a `DbSession`, so they are tested without Tauri.

use std::future::Future;
use std::ops::ControlFlow;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use tauri::State;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::drivers::{
    CancelSlot, CollectSink, DbSession, RowSink, StatementOutcome, cancel_requested, contains_word,
    is_dml, lock_editor_runs, request_cancel,
};
use crate::error::{AppError, ErrorPayload, ErrorPos};
use crate::sqlgen::Dialect;
use crate::sqlgen::explain::{analyze_wrapper, explain_offset, wrap_explain};
use crate::sqlgen::split::{
    SplitStatement, code_only, is_meta_command, is_single_statement, leading_keywords,
    split_statements,
};
use crate::state::{AppState, Connection};
use crate::types::history::NewHistoryEntry;
use crate::types::query::{
    DEFAULT_MAX_ROWS, ExplainMode, ExplainResult, QueryEvent, ResultColumn, ResultSet, RunOptions,
    ServerNotice, StatementKind, StatementResult, StatementSpan,
};
use crate::types::value::Value;

/// Upper bound for `RunOptions::max_rows`.
pub const MAX_ROWS_LIMIT: u32 = 1_000_000;
/// Rows kept for an EXPLAIN result.
const EXPLAIN_MAX_ROWS: usize = 100_000;
/// Longer statements are stored truncated in the history.
const HISTORY_SQL_MAX_CHARS: usize = 100_000;

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn effective_max_rows(requested: u32) -> usize {
    match requested {
        0 => DEFAULT_MAX_ROWS as usize,
        n => n.min(MAX_ROWS_LIMIT) as usize,
    }
}

/// Forwards one statement's result sets and row chunks as `QueryEvent`s.
struct EventSink<'a> {
    index: usize,
    sets: u32,
    rows: u64,
    first_columns: Option<Vec<ResultColumn>>,
    emit: &'a mut (dyn FnMut(QueryEvent) + Send),
}

impl RowSink for EventSink<'_> {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        if self.first_columns.is_none() {
            self.first_columns = Some(columns.clone());
        }
        (self.emit)(QueryEvent::Columns {
            index: self.index,
            set: self.sets,
            columns,
        });
        self.sets += 1;
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.rows += rows.len() as u64;
        (self.emit)(QueryEvent::Rows {
            index: self.index,
            set: self.sets.saturating_sub(1),
            rows,
        });
        ControlFlow::Continue(())
    }

    /// Notices go to the renderer as they arrive (`QueryEvent::Notice`), not in the summary.
    fn notice(&mut self, notice: &ServerNotice, during: bool) -> bool {
        (self.emit)(QueryEvent::notice(during.then_some(self.index), notice));
        true
    }
}

static MYSQL_NEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)near '(.*)' at line (\d+)\s*$").expect("valid MySQL error regex")
});

/// Longest "near" text MySQL quotes (`… near '%-.80s' at line N`).
const MYSQL_NEAR_MAX_CHARS: usize = 80;

/// Char offset of a MySQL syntax error inside `sql`, from the server's
/// `… near '<rest of statement>' at line N` message.
///
/// The quoted text is the statement from the error onwards, cut at 80 characters, so when it is
/// shorter the error sits exactly that many characters before the end. Otherwise (or when the
/// text does not match the end) the last occurrence starting on line N is taken: the near text
/// often also appears earlier on the line (`limit 10 10`, `)`).
pub fn mysql_error_offset(message: &str, sql: &str) -> Option<usize> {
    let caps = MYSQL_NEAR.captures(message)?;
    let near = caps.get(1)?.as_str();
    let line: usize = caps.get(2)?.as_str().parse().ok()?;
    let chars: Vec<char> = sql.chars().collect();
    let mut line_start = 0;
    if line > 1 {
        let mut current = 1;
        for (i, c) in chars.iter().enumerate() {
            if *c == '\n' {
                current += 1;
                if current == line {
                    line_start = i + 1;
                    break;
                }
            }
        }
        if current != line {
            return None;
        }
    }
    let line_end = chars[line_start..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |p| line_start + p);
    let near: Vec<char> = near.chars().collect();
    if near.is_empty() {
        return Some(chars.len());
    }
    // Not cut: the rest of the statement is exactly the quoted text.
    if near.len() < MYSQL_NEAR_MAX_CHARS
        && near.len() <= chars.len()
        && chars[chars.len() - near.len()..] == near[..]
    {
        let at = chars.len() - near.len();
        if (line_start..=line_end).contains(&at) {
            return Some(at);
        }
    }
    // Cut (or not an exact suffix): the last match that starts on the reported line.
    let found = (line_start..=line_end).rev().find(|&i| {
        let rest = &chars[i..];
        rest.len() >= near.len() && rest[..near.len()] == near[..]
    });
    Some(found.unwrap_or(line_start))
}

/// Moves a driver error's position (relative to `exec_sql`, which is the statement possibly
/// behind an EXPLAIN prefix of `shift` chars) onto the whole editor text. Returns the error and
/// the statement-relative offset.
fn locate_error(
    mut err: AppError,
    st: &SplitStatement,
    full_sql: &str,
    exec_sql: &str,
    shift: usize,
    dialect: Dialect,
) -> (AppError, Option<usize>) {
    let mut relative = None;
    if let AppError::Db {
        message, position, ..
    } = &mut err
    {
        let raw = match position.as_ref() {
            Some(pos) => Some(pos.offset as usize),
            None if dialect == Dialect::Mysql => mysql_error_offset(message, exec_sql),
            None => None,
        };
        *position = None;
        if let Some(rel) = raw.and_then(|r| r.checked_sub(shift)) {
            let rel = rel.min(st.end - st.start);
            *position = Some(ErrorPos::from_offset(full_sql, st.start + rel));
            relative = Some(rel);
        }
    }
    (err, relative)
}

fn statement_error(
    err: AppError,
    index: usize,
    st: &SplitStatement,
    full_sql: &str,
    exec_sql: &str,
    shift: usize,
    dialect: Dialect,
) -> ErrorPayload {
    let (err, relative) = locate_error(err, st, full_sql, exec_sql, shift, dialect);
    let mut payload = err.with_statement_index(index).to_payload();
    payload.statement_index = Some(index);
    if let Some(rel) = relative {
        payload.data = Some(serde_json::json!({
            "statementOffset": rel,
            "statementStart": st.start,
        }));
    }
    payload
}

/// EXPLAIN must wrap exactly one statement: the servers run every statement of a multi-statement
/// text (MySQL always allows them), so wrapping a custom-`DELIMITER` block such as
/// `SELECT …; INSERT …` would show the SELECT's plan and silently execute the INSERT.
fn check_explainable(sql: &str, dialect: Dialect) -> Result<(), AppError> {
    if is_single_statement(sql, dialect) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "EXPLAIN needs a single statement, but this one contains several (a DELIMITER \
             block?); explain them one at a time"
                .into(),
        ))
    }
}

fn meta_command_error() -> AppError {
    AppError::Unsupported(
        "psql meta-commands (\\d, \\copy, \\timing…) are not SQL and cannot run here".into(),
    )
}

/// Whether running `sql` changes data: DML, or a `WITH` whose code (not its strings, quoted
/// identifiers or comments) holds a data-modifying `INSERT` / `UPDATE` / `DELETE` / `MERGE`.
/// Deliberately coarse (`… FOR UPDATE` counts): it only decides whether an Explain Analyze may run
/// when the transaction state is unknown.
pub fn analyze_writes(sql: &str, dialect: Dialect) -> bool {
    if is_dml(sql) {
        return true;
    }
    if leading_keywords(sql, 1).first().map(String::as_str) != Some("WITH") {
        return false;
    }
    let code = code_only(sql, dialect).to_uppercase();
    ["INSERT", "UPDATE", "DELETE", "MERGE"]
        .iter()
        .any(|w| contains_word(&code, w))
}

/// Whether `sql` (one statement) can change the schema, so cached table metadata is stale after
/// it ran: its first keyword is CREATE, ALTER, DROP, RENAME, TRUNCATE or COMMENT.
pub fn changes_schema(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("CREATE" | "ALTER" | "DROP" | "RENAME" | "TRUNCATE" | "COMMENT")
    )
}

/// Throws away whatever the Explain Analyze wrapper statements return.
struct DiscardSink;

impl RowSink for DiscardSink {
    fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}

    fn rows(&mut self, _rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

/// Opens what an executing EXPLAIN ANALYZE of `sql` runs in (`analyze_wrapper`: a transaction
/// or a savepoint, rolled back afterwards) on the session's editor connection, and returns the
/// statements that close it again. Nothing is left open when this fails.
///
/// The wrapper statements run with slots of their own: they are not the user's statement (and
/// are quick), and the closing ones must run even after the user cancelled the EXPLAIN.
async fn open_analyze(
    session: &dyn DbSession,
    sql: &str,
    traced: bool,
) -> Result<Vec<String>, AppError> {
    let dialect = session.dialect();
    let writes = analyze_writes(sql, dialect);
    let in_tx = transaction_state(session, traced).await;
    let Some((open, close)) = analyze_wrapper(dialect, in_tx, writes)? else {
        return Ok(Vec::new());
    };
    run_around_explain(
        session,
        traced,
        &open,
        1,
        &mut DiscardSink,
        CancelSlot::default(),
    )
    .await?;
    Ok(close)
}

/// `run_statement` for what runs around an EXPLAIN: the statement itself and the wrapper and
/// probes of an Analyze. `traced` records it in the session's trace (the Explain command: the
/// console log shows what really ran); the SQL editor's runs are logged by the renderer from the
/// run's events and are not.
async fn run_around_explain(
    session: &dyn DbSession,
    traced: bool,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    if traced {
        session
            .run_statement_traced(sql, max_rows, sink, cancel)
            .await
    } else {
        session.run_statement(sql, max_rows, sink, cancel).await
    }
}

/// Whether the editor connection has a transaction open, for `analyze_wrapper`: the session's
/// own tracking, checked against the server on MySQL / MariaDB. With `autocommit` switched off
/// (`SET autocommit = 0`) a transaction is always open (started implicitly by the user's last
/// statement), which the tracking does not see and `START TRANSACTION` would commit. MariaDB
/// reports `@@in_transaction` as well, which also covers its `ANALYZE <statement>` (the tracking
/// takes that for the implicitly committing `ANALYZE TABLE`). `None` when unknown.
async fn transaction_state(session: &dyn DbSession, traced: bool) -> Option<bool> {
    let tracked = session.in_transaction();
    if session.dialect() != Dialect::Mysql {
        return tracked;
    }
    let mariadb = session.is_mariadb();
    if tracked == Some(true) && !mariadb {
        return tracked;
    }
    let read = |sql: &'static str| async move {
        let mut sink = CollectSink::default();
        match run_around_explain(session, traced, sql, 1, &mut sink, CancelSlot::default()).await {
            Ok(_) => sink
                .sets
                .into_iter()
                .next()
                .and_then(|set| set.rows.into_iter().next())
                .map(|row| row.iter().map(flag).collect::<Vec<_>>()),
            Err(e) => {
                log::warn!("explain analyze: `{sql}` failed: {e}");
                None
            }
        }
    };
    let flags = if mariadb {
        match read("SELECT @@autocommit, @@in_transaction").await {
            Some(flags) => Some(flags),
            // An older server without the variable.
            None => read("SELECT @@autocommit").await,
        }
    } else {
        read("SELECT @@autocommit").await
    };
    let Some(flags) = flags else {
        return if mariadb { None } else { tracked };
    };
    match (
        flags.first().copied().flatten(),
        flags.get(1).copied().flatten(),
    ) {
        (Some(false), _) => Some(true),
        (Some(true), Some(open)) => Some(open),
        (Some(true), None) if !mariadb => tracked,
        _ => None,
    }
}

/// A 0 / 1 server variable as a flag.
fn flag(v: &Value) -> Option<bool> {
    match v {
        Value::Int(n) => Some(*n != 0),
        Value::Text(t) | Value::Decimal(t) | Value::BigInt(t) => match t.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

/// Runs the statements `open_analyze` returned, whatever happened to the EXPLAIN. A failure is
/// logged (never reported in place of the EXPLAIN's own result) and stops the rest: releasing a
/// savepoint that could not be rolled back to would keep the statement's changes.
async fn close_analyze(session: &dyn DbSession, traced: bool, close: &[String]) {
    for sql in close {
        if let Err(e) = run_around_explain(
            session,
            traced,
            sql,
            1,
            &mut DiscardSink,
            CancelSlot::default(),
        )
        .await
        {
            log::warn!("explain analyze: `{sql}` failed: {e}");
            return;
        }
    }
}

/// Runs the (already wrapped) EXPLAIN `exec_sql` of `statement`; an Analyze runs inside a
/// rolled-back transaction or savepoint (`open_analyze`). `traced`: see `run_around_explain`.
#[allow(clippy::too_many_arguments)]
async fn run_explain(
    session: &dyn DbSession,
    statement: &str,
    exec_sql: &str,
    mode: ExplainMode,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
    traced: bool,
) -> Result<StatementOutcome, AppError> {
    if cancel_requested(cancel) {
        return Err(AppError::Cancelled);
    }
    let close = match mode {
        ExplainMode::Analyze => open_analyze(session, statement, traced).await?,
        ExplainMode::Plan => Vec::new(),
    };
    let result =
        run_around_explain(session, traced, exec_sql, max_rows, sink, cancel.clone()).await;
    close_analyze(session, traced, &close).await;
    result
}

/// Error code of a statement stopped by `RunOptions::timeout_secs`. Not `TIMEOUT`: the renderer
/// takes that for a connection that stopped answering.
pub const QUERY_TIMEOUT_CODE: &str = crate::error::QUERY_TIMEOUT_CODE;

/// What a statement stopped by the query time out reports.
pub fn timeout_payload(secs: u32, index: usize) -> ErrorPayload {
    let mut payload = AppError::QueryTimeout { secs }.to_payload();
    payload.statement_index = Some(index);
    payload
}

/// The query time out preference (Settings ▸ Connections ▸ Query time out, in seconds, 0 = no
/// limit) as a limit.
pub fn time_limit(secs: u32) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs.into()))
}

/// The limit every statement the app runs for the user is under: the preference in the app
/// settings (default 300 s, as the renderer's), read on every call so a change applies at once;
/// `explicit` (a `timeoutSecs` argument, 0 = none) overrides it. A settings database that cannot
/// be read leaves the default in force.
pub fn query_time_limit(appdb: &crate::appdb::AppDb, explicit: Option<u32>) -> Option<Duration> {
    let secs = explicit.unwrap_or_else(|| {
        crate::mcp::prefs::McpPrefs::load(appdb)
            .map(|p| p.query_timeout_secs)
            .unwrap_or(crate::mcp::prefs::DEFAULT_QUERY_TIMEOUT_SECS)
    });
    time_limit(secs)
}

/// Turns the `Cancelled` a timed-out statement ends with into the timeout error (a statement that
/// finished before its cancel landed keeps its outcome).
pub fn timed_out_as_error<T>(
    result: Result<T, AppError>,
    expired: bool,
    limit: Option<Duration>,
) -> Result<T, AppError> {
    match result {
        Err(AppError::Cancelled) if expired => Err(AppError::QueryTimeout {
            secs: limit.map_or(0, |l| l.as_secs() as u32),
        }),
        other => other,
    }
}

/// `session.query` under the query time out: past `limit` the statement is cancelled on the server
/// (the pool connection is not left running it) and the call fails with `QueryTimeout`. Used for
/// the table grid's page loads, row counts and cell fetches; without a limit it is a plain query.
pub async fn query_limited(
    session: &dyn DbSession,
    sql: &str,
    params: &[Value],
    limit: Option<Duration>,
) -> Result<ResultSet, AppError> {
    if limit.is_none() {
        return session.query(sql, params).await;
    }
    let slot = CancelSlot::default();
    let (result, expired) = with_timeout(
        session,
        &slot,
        limit,
        session.query_cancellable(sql, params, slot.clone()),
    )
    .await;
    timed_out_as_error(result, expired, limit)
}

/// Runs one statement under the run's time limit. When `limit` passes first, the statement is
/// cancelled the way Stop cancels it (through the run's slot, so the rest of the run is not
/// started either) and its end is still awaited. Returns whether the limit was hit.
pub(crate) async fn with_timeout<T>(
    session: &dyn DbSession,
    cancel: &CancelSlot,
    limit: Option<Duration>,
    statement: impl Future<Output = Result<T, AppError>>,
) -> (Result<T, AppError>, bool) {
    let Some(limit) = limit else {
        return (statement.await, false);
    };
    tokio::pin!(statement);
    tokio::select! {
        result = &mut statement => return (result, false),
        () = tokio::time::sleep(limit) => {}
    }
    let cancel_now = async {
        if let Some(handle) = request_cancel(cancel)
            && let Err(e) = session.cancel(handle).await
        {
            log::warn!("query time out: cancelling the statement failed: {e}");
        }
    };
    let (result, ()) = tokio::join!(statement, cancel_now);
    (result, true)
}

/// Splits `sql` and runs each statement on the session's editor connection, emitting
/// `started`, `columns`/`rows`, `finished`/`failed` per statement and `done` at the end.
/// Stops at the first error when `stop_on_error` is set, and always after a cancellation
/// (`cancel` is the slot `cancel_query` uses for this run) or a statement that ran longer than
/// `timeout_secs`, which is cancelled and reported as timed out.
///
/// The run keeps the editor for all its statements (`lock_editor_runs`): a second run on the same
/// connection (another tab) waits for it instead of slipping its statements in between, e.g.
/// into this run's `BEGIN … ROLLBACK`.
pub async fn run_sql_inner(
    session: &dyn DbSession,
    sql: &str,
    options: &RunOptions,
    cancel: &CancelSlot,
    emit: &mut (dyn FnMut(QueryEvent) + Send),
) -> Vec<StatementResult> {
    // PostgreSQL resolves result column origins through the catalog: once per table and run.
    crate::drivers::postgres::decode::with_origin_cache(run_statements(
        session, sql, options, cancel, emit,
    ))
    .await
}

async fn run_statements(
    session: &dyn DbSession,
    sql: &str,
    options: &RunOptions,
    cancel: &CancelSlot,
    emit: &mut (dyn FnMut(QueryEvent) + Send),
) -> Vec<StatementResult> {
    let run_started = Instant::now();
    let time_limit =
        (options.timeout_secs > 0).then(|| Duration::from_secs(options.timeout_secs.into()));
    let dialect = session.dialect();
    let mariadb = session.is_mariadb();
    let max_rows = effective_max_rows(options.max_rows);
    let statements = split_statements(sql, dialect);
    let mut results = Vec::with_capacity(statements.len());
    // Err: cancelled while waiting for another run; the first statement reports it.
    let editor = lock_editor_runs(session, cancel).await;

    for (index, st) in statements.iter().enumerate() {
        let span = StatementSpan {
            start: st.start as u32,
            end: st.end as u32,
            line: st.line,
        };
        emit(QueryEvent::Started {
            index,
            sql: st.sql.clone(),
            span,
        });
        let started_at = now_rfc3339();
        let started = Instant::now();
        let (exec_sql, shift) = match options.explain {
            Some(mode) => (
                wrap_explain(dialect, &st.sql, mode, mariadb),
                explain_offset(dialect, mode, mariadb),
            ),
            None => (st.sql.clone(), 0),
        };

        let mut sink = EventSink {
            index,
            sets: 0,
            rows: 0,
            first_columns: None,
            emit: &mut *emit,
        };
        let mut timed_out = false;
        let outcome = if is_meta_command(&st.sql) {
            Err(meta_command_error())
        } else if editor.is_err() {
            Err(AppError::Cancelled)
        } else if options.explain.is_some()
            && let Err(e) = check_explainable(&st.sql, dialect)
        {
            Err(e)
        } else {
            let statement = async {
                match options.explain {
                    Some(mode) => {
                        run_explain(
                            session, &st.sql, &exec_sql, mode, max_rows, &mut sink, cancel, false,
                        )
                        .await
                    }
                    None => {
                        session
                            .run_statement(&exec_sql, max_rows, &mut sink, cancel.clone())
                            .await
                    }
                }
            };
            let (outcome, expired) = with_timeout(session, cancel, time_limit, statement).await;
            timed_out = expired;
            outcome
        };
        if changes_schema(&st.sql) {
            crate::drivers::postgres::decode::clear_origin_cache();
        }
        let (first_columns, sets, delivered) = (sink.first_columns, sink.sets, sink.rows);

        let mut result = StatementResult {
            index,
            sql: st.sql.clone(),
            span,
            kind: StatementKind::Empty,
            columns: first_columns.unwrap_or_default(),
            result_sets: sets,
            row_count: delivered,
            truncated: false,
            affected: None,
            last_insert_id: None,
            started_at,
            duration_ms: started.elapsed().as_millis() as u64,
            error: None,
            notices: Vec::new(),
        };
        let stop = match outcome {
            Ok(outcome) => {
                result.kind = if options.explain.is_some() {
                    StatementKind::Explain
                } else {
                    outcome.kind
                };
                result.result_sets = outcome.result_sets;
                result.row_count = outcome.row_count;
                result.truncated = outcome.truncated;
                result.affected = outcome.affected;
                result.last_insert_id = outcome.last_insert_id;
                result.notices = outcome.notices;
                if timed_out {
                    result.notices.push(format!(
                        "The query time out ({} s) passed, but the statement completed before it \
                         could be cancelled; the rest of the run was not started.",
                        options.timeout_secs
                    ));
                }
                emit(QueryEvent::Finished {
                    index,
                    result: result.clone(),
                });
                timed_out
            }
            Err(err) => {
                let cancelled = matches!(err, AppError::Cancelled);
                let payload = if timed_out && cancelled {
                    timeout_payload(options.timeout_secs, index)
                } else {
                    statement_error(err, index, st, sql, &exec_sql, shift, dialect)
                };
                result.error = Some(payload.clone());
                emit(QueryEvent::Failed {
                    index,
                    error: payload,
                });
                // A timed-out run is cancelled as a whole: nothing after it may start.
                cancelled || timed_out || options.stop_on_error
            }
        };
        results.push(result);
        if stop {
            break;
        }
    }
    drop(editor);
    emit(QueryEvent::Done {
        total_ms: run_started.elapsed().as_millis() as u64,
    });
    results
}

/// The plan as plain text: one-column plans (PostgreSQL, MySQL tree, MariaDB JSON) joined by
/// newlines; SQLite's `EXPLAIN QUERY PLAN` rows rendered as an indented tree.
fn plan_text(dialect: Dialect, set: &ResultSet) -> Option<String> {
    let cell_text = |v: &Value| match v {
        Value::Text(s) | Value::Json(s) => s.clone(),
        other => other.as_text().unwrap_or_default(),
    };
    if set.columns.len() == 1 {
        let lines: Vec<String> = set
            .rows
            .iter()
            .map(|r| r.first().map(cell_text).unwrap_or_default())
            .collect();
        return Some(lines.join("\n"));
    }
    if dialect == Dialect::Sqlite {
        let idx = |name: &str| set.columns.iter().position(|c| c.name == name);
        let (id_i, parent_i, detail_i) = (idx("id")?, idx("parent")?, idx("detail")?);
        let int = |v: &Value| match v {
            Value::Int(n) => *n,
            _ => 0,
        };
        let mut depths: std::collections::HashMap<i64, usize> = Default::default();
        let mut lines = Vec::with_capacity(set.rows.len());
        for row in &set.rows {
            let id = row.get(id_i).map(int).unwrap_or(0);
            let parent = row.get(parent_i).map(int).unwrap_or(0);
            let depth = depths.get(&parent).map(|d| d + 1).unwrap_or(0);
            depths.insert(id, depth);
            let detail = row.get(detail_i).map(cell_text).unwrap_or_default();
            lines.push(format!("{}{detail}", "  ".repeat(depth)));
        }
        return Some(lines.join("\n"));
    }
    None
}

/// EXPLAINs the first statement of `sql` on the editor connection.
pub async fn explain_inner(
    session: &dyn DbSession,
    sql: &str,
    mode: ExplainMode,
    cancel: &CancelSlot,
) -> Result<ExplainResult, AppError> {
    explain_limited(session, sql, mode, cancel, None).await
}

/// `explain_inner` under the query time out: past `limit` the statement is cancelled the way Stop
/// cancels it and the call fails with `QueryTimeout`.
pub async fn explain_limited(
    session: &dyn DbSession,
    sql: &str,
    mode: ExplainMode,
    cancel: &CancelSlot,
    limit: Option<Duration>,
) -> Result<ExplainResult, AppError> {
    let dialect = session.dialect();
    let statements = split_statements(sql, dialect);
    let st = statements
        .first()
        .ok_or_else(|| AppError::InvalidInput("there is no statement to explain".into()))?;
    if is_meta_command(&st.sql) {
        return Err(meta_command_error());
    }
    check_explainable(&st.sql, dialect)?;
    let mariadb = session.is_mariadb();
    let exec_sql = wrap_explain(dialect, &st.sql, mode, mariadb);
    let shift = explain_offset(dialect, mode, mariadb);
    let mut sink = CollectSink::default();
    let _editor = lock_editor_runs(session, cancel).await?;
    let started = Instant::now();
    let (outcome, expired) = with_timeout(
        session,
        cancel,
        limit,
        run_explain(
            session,
            &st.sql,
            &exec_sql,
            mode,
            EXPLAIN_MAX_ROWS,
            &mut sink,
            cancel,
            true,
        ),
    )
    .await;
    // A MySQL `KILL` can end an EXPLAIN ANALYZE without an error, leaving a plan of a statement
    // that did not run to completion: past the limit that counts as timed out too.
    let outcome = match outcome {
        Ok(_) if expired => Err(AppError::Cancelled),
        other => other,
    };
    timed_out_as_error(outcome, expired, limit)
        .map_err(|e| locate_error(e, st, sql, &exec_sql, shift, dialect).0)?;
    let set = sink.sets.into_iter().next().unwrap_or_default();
    let text = plan_text(dialect, &set);
    Ok(ExplainResult {
        columns: set.columns,
        rows: set.rows,
        text,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Registers a running query's cancel slot under (connection, query id); removed on drop.
struct ActiveQuery<'a> {
    state: &'a AppState,
    key: (Uuid, String),
}

impl<'a> ActiveQuery<'a> {
    fn register(
        state: &'a AppState,
        connection_id: Uuid,
        query_id: String,
        slot: CancelSlot,
    ) -> Result<ActiveQuery<'a>, AppError> {
        let key = (connection_id, query_id);
        let mut active = state
            .active_queries
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if active.contains_key(&key) {
            return Err(AppError::InvalidInput(format!(
                "query \"{}\" is already running",
                key.1
            )));
        }
        active.insert(key.clone(), slot);
        Ok(ActiveQuery { state, key })
    }
}

impl Drop for ActiveQuery<'_> {
    fn drop(&mut self) {
        self.state
            .active_queries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}

fn history_sql(sql: &str) -> String {
    match sql.char_indices().nth(HISTORY_SQL_MAX_CHARS) {
        Some((byte, _)) => format!("{}\n-- … (truncated)", &sql[..byte]),
        None => sql.to_string(),
    }
}

/// One history row per executed statement; failures are logged, never returned.
async fn record_history(state: &AppState, conn: &Connection, rows: Vec<NewHistoryEntry>) {
    if rows.is_empty() {
        return;
    }
    let database = conn.info().await.current_database;
    let rows: Vec<NewHistoryEntry> = rows
        .into_iter()
        .map(|mut r| {
            r.database = database.clone();
            r
        })
        .collect();
    if let Err(e) = state.appdb.insert_history(&rows) {
        log::warn!("could not record query history: {e}");
    }
}

fn history_entry(conn: &Connection, r: &StatementResult, source: &str) -> NewHistoryEntry {
    NewHistoryEntry {
        profile_id: Some(conn.profile.id),
        connection_name: conn.profile.name.clone(),
        database: None,
        sql: history_sql(&r.sql),
        started_at: r.started_at.clone(),
        duration_ms: Some(r.duration_ms),
        row_count: (r.kind == StatementKind::Rows || r.kind == StatementKind::Explain)
            .then_some(r.row_count),
        affected: r.affected,
        success: r.error.is_none(),
        error: r.error.as_ref().map(|e| e.message.clone()),
        source: source.to_string(),
    }
}

/// Runs the editor text `sql` on the connection's editor session. Results stream through
/// `on_event`; the summaries are also returned. `queryId` (chosen by the renderer) is what
/// `cancel_query` takes.
#[tauri::command]
pub async fn run_sql(
    state: State<'_, AppState>,
    connection_id: Uuid,
    query_id: String,
    sql: String,
    options: Option<RunOptions>,
    on_event: Channel<QueryEvent>,
) -> Result<Vec<StatementResult>, AppError> {
    let options = options.unwrap_or_default();
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    crate::commands::safety::refuse_read_only_run(&conn, &sql, session.dialect(), options.explain)?;
    let slot = CancelSlot::default();
    let results = {
        let _active = ActiveQuery::register(&state, connection_id, query_id, slot.clone())?;
        let mut emit = move |event: QueryEvent| {
            if let Err(e) = on_event.send(event) {
                log::debug!("run_sql: event channel closed: {e}");
            }
        };
        run_sql_inner(session.as_ref(), &sql, &options, &slot, &mut emit).await
    };
    // Browsing, counts and cell fetches read the cached metadata: DDL from the editor makes it
    // stale (also when it failed partway; dropping the cache only costs a re-read).
    if results.iter().any(|r| changes_schema(&r.sql)) {
        conn.invalidate(None).await;
    }
    let source = if options.explain.is_some() {
        "explain"
    } else {
        "editor"
    };
    let rows = results
        .iter()
        .map(|r| history_entry(&conn, r, source))
        .collect();
    record_history(&state, &conn, rows).await;
    Ok(results)
}

/// Cancels a running `run_sql` (or `explain` started with a query id). Succeeds when nothing
/// is running under that id.
#[tauri::command]
pub async fn cancel_query(
    state: State<'_, AppState>,
    connection_id: Uuid,
    query_id: String,
) -> Result<(), AppError> {
    let slot = state
        .active_queries
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(connection_id, query_id))
        .cloned();
    let Some(slot) = slot else {
        return Ok(());
    };
    // While the statement still runs, every call returns its live handle again, so a cancel
    // that failed (network, server busy) can simply be retried.
    if let Some(handle) = request_cancel(&slot) {
        let session = match state.session(connection_id).await {
            Ok(session) => session,
            // Disconnecting: closing the session stops the statement itself.
            Err(AppError::NotConnected) => return Ok(()),
            Err(e) => return Err(e),
        };
        session.cancel(handle).await?;
    }
    Ok(())
}

/// EXPLAINs (or EXPLAIN ANALYZEs) the first statement of `sql`. Pass `queryId` to make it
/// cancellable through `cancel_query`.
#[tauri::command]
pub async fn explain(
    state: State<'_, AppState>,
    connection_id: Uuid,
    sql: String,
    mode: ExplainMode,
    query_id: Option<String>,
    timeout_secs: Option<u32>,
) -> Result<ExplainResult, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    crate::commands::safety::refuse_read_only_explain(&conn, &sql, session.dialect(), mode)?;
    let slot = CancelSlot::default();
    let started_at = now_rfc3339();
    let result = {
        let _active = match query_id {
            Some(id) => Some(ActiveQuery::register(
                &state,
                connection_id,
                id,
                slot.clone(),
            )?),
            None => None,
        };
        let limit = query_time_limit(&state.appdb, timeout_secs);
        explain_limited(session.as_ref(), &sql, mode, &slot, limit).await
    };
    let statement = split_statements(&sql, session.dialect())
        .into_iter()
        .next()
        .map(|s| s.sql)
        .unwrap_or_default();
    if !statement.is_empty() {
        let entry = NewHistoryEntry {
            profile_id: Some(conn.profile.id),
            connection_name: conn.profile.name.clone(),
            database: None,
            sql: history_sql(&statement),
            started_at,
            duration_ms: result.as_ref().ok().map(|r| r.duration_ms),
            row_count: result.as_ref().ok().map(|r| r.rows.len() as u64),
            affected: None,
            success: result.is_ok(),
            error: result.as_ref().err().map(|e| e.to_string()),
            source: "explain".into(),
        };
        record_history(&state, &conn, vec![entry]).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sqlite::LiteSession;
    use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
    use crate::types::query::ExecOutcome;
    use crate::types::schema::{
        CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
    };

    async fn sqlite_session() -> (LiteSession, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let profile = ConnectionProfile {
            id: Uuid::new_v4(),
            name: "t".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.to_string_lossy().into_owned()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        let session = LiteSession::open(&profile).await.unwrap();
        (session, dir)
    }

    async fn run(
        session: &dyn DbSession,
        sql: &str,
        options: RunOptions,
    ) -> (Vec<StatementResult>, Vec<QueryEvent>) {
        let mut events = Vec::new();
        let slot = CancelSlot::default();
        let results = {
            let mut emit = |e: QueryEvent| events.push(e);
            run_sql_inner(session, sql, &options, &slot, &mut emit).await
        };
        (results, events)
    }

    fn event_names(events: &[QueryEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn runs_statements_and_streams_events() {
        let (session, _dir) = sqlite_session().await;
        let sql = "create table t (id integer primary key, name text);\n\
                   insert into t (name) values ('a'), ('b');\n\
                   select * from t order by id;";
        let (results, events) = run(&session, sql, RunOptions::default()).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].kind, StatementKind::Empty);
        assert_eq!(results[1].kind, StatementKind::Affected);
        assert_eq!(results[1].affected, Some(2));
        assert_eq!(results[1].last_insert_id.as_deref(), Some("2"));
        assert_eq!(results[2].kind, StatementKind::Rows);
        assert_eq!(results[2].row_count, 2);
        assert_eq!(results[2].result_sets, 1);
        assert_eq!(results[2].columns.len(), 2);
        assert_eq!(results[2].span.line, 3);
        assert!(results.iter().all(|r| r.error.is_none()));
        assert_eq!(
            event_names(&events),
            [
                "started", "finished", "started", "finished", "started", "columns", "rows",
                "finished", "done"
            ]
        );
        match &events[6] {
            QueryEvent::Rows { index, set, rows } => {
                assert_eq!((*index, *set), (2, 0));
                assert_eq!(rows[1][1], Value::Text("b".into()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn stops_on_error_with_absolute_position() {
        let (session, _dir) = sqlite_session().await;
        let sql = "select 1;\nselect 2 frm x;\nselect 3;";
        let (results, events) = run(&session, sql, RunOptions::default()).await;
        assert_eq!(results.len(), 2);
        let err = results[1].error.as_ref().expect("error");
        assert_eq!(err.statement_index, Some(1));
        let pos = err.position.as_ref().expect("position");
        // `x` is the unexpected token: line 2, column 14 of the editor text.
        assert_eq!((pos.line, pos.column), (2, 14));
        assert_eq!(pos.offset, 23);
        assert_eq!(err.data.as_ref().unwrap()["statementOffset"], 13);
        assert_eq!(
            event_names(&events),
            [
                "started", "columns", "rows", "finished", "started", "failed", "done"
            ]
        );

        // Without stop_on_error the remaining statements still run.
        let opts = RunOptions {
            stop_on_error: false,
            ..RunOptions::default()
        };
        let (results, _) = run(&session, sql, opts).await;
        assert_eq!(results.len(), 3);
        assert!(results[2].error.is_none());
    }

    #[tokio::test]
    async fn caps_rows_and_explains() {
        let (session, _dir) = sqlite_session().await;
        let sql = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 1200) select x from c";
        let opts = RunOptions {
            max_rows: 1000,
            ..RunOptions::default()
        };
        let (results, events) = run(&session, sql, opts).await;
        assert!(results[0].truncated);
        assert_eq!(results[0].row_count, 1000);
        let chunks: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                QueryEvent::Rows { rows, .. } => Some(rows.len()),
                _ => None,
            })
            .collect();
        assert_eq!(chunks.iter().sum::<usize>(), 1000);
        assert!(chunks.iter().all(|n| *n <= crate::drivers::ROW_CHUNK));

        let opts = RunOptions {
            explain: Some(ExplainMode::Plan),
            ..RunOptions::default()
        };
        let (results, _) = run(&session, "select 1;", opts).await;
        assert_eq!(results[0].kind, StatementKind::Explain);

        let slot = CancelSlot::default();
        session
            .run_statement(
                "create table e (a int)",
                10,
                &mut CollectSink::default(),
                slot.clone(),
            )
            .await
            .unwrap();
        let plan = explain_inner(
            &session,
            "select * from e where a = 1;",
            ExplainMode::Plan,
            &slot,
        )
        .await
        .unwrap();
        assert!(plan.text.unwrap().contains("SCAN e"));
        let err = explain_inner(&session, "  \n select * frm e", ExplainMode::Plan, &slot)
            .await
            .unwrap_err();
        match err {
            AppError::Db { position, .. } => {
                let pos = position.expect("position");
                // `frm` sits at line 2, column 11 of the submitted text.
                assert_eq!((pos.line, pos.column), (2, 11));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn meta_commands_and_cancelled_runs() {
        let (session, _dir) = sqlite_session().await;
        let (results, _) = run(&session, "\\d users", RunOptions::default()).await;
        assert_eq!(results.len(), 1);
        // SQLite does not treat `\` lines as meta-commands; the splitter only does for PG.
        assert!(results[0].error.is_some());

        let slot = CancelSlot::default();
        request_cancel(&slot);
        let mut events = Vec::new();
        let results = {
            let mut emit = |e: QueryEvent| events.push(e);
            run_sql_inner(
                &session,
                "select 1; select 2",
                &RunOptions {
                    stop_on_error: false,
                    ..RunOptions::default()
                },
                &slot,
                &mut emit,
            )
            .await
        };
        assert_eq!(results.len(), 1, "a cancelled run stops");
        assert_eq!(results[0].error.as_ref().unwrap().code, "CANCELLED");
        assert_eq!(event_names(&events), ["started", "failed", "done"]);
    }

    /// A statement that keeps the SQLite editor busy for a while.
    const SLOW: &str = "with recursive c(x) as (select 1 union all select x + 1 from c \
                        where x < 3000000) select count(*) from c";

    /// Runs `sql` with its own slot, logging `<label> <event> <index>` lines into `log`.
    async fn logged_run(
        session: &dyn DbSession,
        sql: &str,
        slot: &CancelSlot,
        label: &str,
        log: &std::sync::Mutex<Vec<String>>,
    ) -> Vec<StatementResult> {
        let mut emit = |e: QueryEvent| {
            let v = serde_json::to_value(&e).unwrap();
            let index = v["data"]["index"].as_u64().map(|i| format!(" {i}"));
            log.lock().unwrap().push(format!(
                "{label} {}{}",
                v["event"].as_str().unwrap(),
                index.unwrap_or_default()
            ));
        };
        run_sql_inner(session, sql, &RunOptions::default(), slot, &mut emit).await
    }

    async fn wait_for(log: &std::sync::Mutex<Vec<String>>, line: &str) {
        for _ in 0..1000 {
            if log.lock().unwrap().iter().any(|l| l == line) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("never saw {line:?}: {:?}", log.lock().unwrap());
    }

    #[tokio::test]
    async fn concurrent_runs_do_not_interleave() {
        let (session, _dir) = sqlite_session().await;
        let (results, _) = run(
            &session,
            "create table acc (id integer primary key, flag int); insert into acc values (1, 0);",
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");

        let log = std::sync::Mutex::new(Vec::new());
        let (slot_a, slot_b) = (CancelSlot::default(), CancelSlot::default());
        let sql_a = format!("begin; {SLOW}; rollback;");
        let run_a = logged_run(&session, &sql_a, &slot_a, "A", &log);
        let run_b = async {
            // B starts while A is inside its transaction.
            wait_for(&log, "A finished 0").await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            logged_run(&session, "update acc set flag = 1", &slot_b, "B", &log).await
        };
        let (a, b) = tokio::join!(run_a, run_b);
        assert!(a.iter().all(|r| r.error.is_none()), "{a:?}");
        assert!(b[0].error.is_none(), "{b:?}");
        assert_eq!(b[0].affected, Some(1));
        let log = log.into_inner().unwrap();
        let a_done = log.iter().position(|l| l == "A done").unwrap();
        let b_started = log.iter().position(|l| l == "B started 0").unwrap();
        assert!(a_done < b_started, "B ran inside A's run: {log:?}");
        // B's update was not rolled back by A's ROLLBACK.
        let (results, events) = run(&session, "select flag from acc", RunOptions::default()).await;
        assert!(results[0].error.is_none());
        match &events[2] {
            QueryEvent::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Int(1)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_queued_run_can_be_cancelled_while_it_waits() {
        let (session, _dir) = sqlite_session().await;
        let log = std::sync::Mutex::new(Vec::new());
        let (slot_a, slot_b) = (CancelSlot::default(), CancelSlot::default());
        let sql_a = format!("{SLOW}; {SLOW}");
        let run_a = logged_run(&session, &sql_a, &slot_a, "A", &log);
        let run_b = async {
            wait_for(&log, "A started 0").await;
            let queued = logged_run(&session, "select 1", &slot_b, "B", &log);
            let cancel = async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                assert!(request_cancel(&slot_b).is_none(), "B never started");
            };
            let (results, ()) = tokio::join!(queued, cancel);
            let a_still_running = !log.lock().unwrap().iter().any(|l| l == "A done");
            (results, a_still_running)
        };
        let (a, (b, a_still_running)) = tokio::join!(run_a, run_b);
        assert!(a.iter().all(|r| r.error.is_none()), "{a:?}");
        assert!(a_still_running, "B waited for A instead of stopping");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].error.as_ref().unwrap().code, "CANCELLED");
    }

    #[test]
    fn event_sink_sends_notices_as_events() {
        let mut events = Vec::new();
        {
            let mut emit = |e: QueryEvent| events.push(e);
            let mut sink = EventSink {
                index: 3,
                sets: 0,
                rows: 0,
                first_columns: None,
                emit: &mut emit,
            };
            let notice = ServerNotice {
                severity: "WARNING".into(),
                message: "careful".into(),
                hint: Some("h".into()),
                ..ServerNotice::default()
            };
            assert!(sink.notice(&notice, true));
            assert!(sink.notice(&notice, false));
        }
        let shapes: Vec<_> = events
            .iter()
            .map(|e| match e {
                QueryEvent::Notice {
                    index,
                    severity,
                    message,
                    hint,
                    ..
                } => (*index, severity.as_str(), message.as_str(), hint.as_deref()),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            shapes,
            [
                (Some(3), "WARNING", "careful", Some("h")),
                (None, "WARNING", "careful", Some("h"))
            ]
        );
    }

    #[test]
    fn analyze_write_detection() {
        for d in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            // DML (also behind comments and parentheses).
            assert!(analyze_writes("DELETE FROM users", d));
            assert!(analyze_writes("  -- tidy up\n update t set a = 1", d));
            assert!(analyze_writes("/* x */ INSERT INTO t VALUES (1)", d));
            assert!(analyze_writes(
                "merge into t using s on true when matched then delete",
                d
            ));
            // Data-modifying CTEs.
            assert!(analyze_writes(
                "WITH gone AS (DELETE FROM t RETURNING *) SELECT count(*) FROM gone",
                d
            ));
            assert!(analyze_writes(
                "with x as (select 1) update t set a = (select * from x)",
                d
            ));
            // Plain reads.
            assert!(!analyze_writes("SELECT * FROM users", d));
            assert!(!analyze_writes(
                "WITH r AS (SELECT 1 AS n) SELECT n FROM r",
                d
            ));
            assert!(!analyze_writes("select updated_at, deleted from t", d));
            // Keywords in strings and comments are not code.
            assert!(!analyze_writes(
                "WITH r AS (SELECT 'DELETE' AS a) SELECT a FROM r -- update later",
                d
            ));
            assert!(!analyze_writes(
                "with r as (select 1 /* insert */) select * from r",
                d
            ));
            assert!(!analyze_writes("-- DELETE FROM t\nSELECT 1", d));
        }
        // A quoted identifier is not code either.
        assert!(!analyze_writes(
            "WITH r AS (SELECT \"update\" FROM t) SELECT * FROM r",
            Dialect::Postgres
        ));
        assert!(!analyze_writes(
            "WITH r AS (SELECT `delete` FROM t) SELECT * FROM r",
            Dialect::Mysql
        ));
    }

    #[test]
    fn schema_changing_statements() {
        for sql in [
            "CREATE TABLE t (a int)",
            "  -- add it\n alter table users drop column bio",
            "drop view v",
            "RENAME TABLE a TO b",
            "truncate t",
            "COMMENT ON COLUMN t.a IS 'x'",
            "/* c */ (CREATE INDEX i ON t (a))",
        ] {
            assert!(changes_schema(sql), "{sql}");
        }
        for sql in [
            "select * from created",
            "INSERT INTO t VALUES ('CREATE TABLE')",
            "-- DROP TABLE t\nSELECT 1",
            "update t set dropped = 1",
            "",
        ] {
            assert!(!changes_schema(sql), "{sql}");
        }
    }

    #[tokio::test]
    async fn sqlite_explain_analyze_shows_the_plan_without_a_transaction() {
        let (session, _dir) = sqlite_session().await;
        let (results, _) = run(
            &session,
            "create table e (a int); insert into e values (1), (2);",
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let slot = CancelSlot::default();
        let plan = explain_inner(&session, "delete from e;", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert!(plan.text.is_some());
        let opts = RunOptions {
            explain: Some(ExplainMode::Analyze),
            ..RunOptions::default()
        };
        let (results, _) = run(&session, "select * from e where a = 1", opts).await;
        assert!(results[0].error.is_none(), "{results:?}");
        assert_eq!(results[0].kind, StatementKind::Explain);
        // EXPLAIN QUERY PLAN never ran the DELETE, and no transaction was left open.
        assert_eq!(session.in_transaction(), Some(false));
        let (_, events) = run(&session, "select count(*) from e", RunOptions::default()).await;
        match &events[2] {
            QueryEvent::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Int(2)),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Records the statements run on the editor; `fail` makes statements containing it fail.
    struct Recorder {
        dialect: Dialect,
        in_tx: Option<bool>,
        fail: Option<&'static str>,
        /// The user cancels while a statement containing this runs.
        cancel_during: Option<&'static str>,
        /// What `SELECT @@autocommit` returns.
        autocommit: i64,
        /// MariaDB, whose `@@in_transaction` reports this.
        mariadb_in_tx: Option<i64>,
        log: std::sync::Mutex<Vec<String>>,
    }

    impl Recorder {
        fn new(dialect: Dialect, in_tx: Option<bool>) -> Recorder {
            Recorder {
                dialect,
                in_tx,
                fail: None,
                cancel_during: None,
                autocommit: 1,
                mariadb_in_tx: None,
                log: Default::default(),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DbSession for Recorder {
        fn driver(&self) -> crate::types::profile::Driver {
            match self.dialect {
                Dialect::Postgres => Driver::Postgres,
                Dialect::Mysql => Driver::Mysql,
                Dialect::Sqlite => Driver::Sqlite,
            }
        }
        fn dialect(&self) -> Dialect {
            self.dialect
        }
        async fn server_info(&self) -> Result<crate::drivers::ServerInfo, AppError> {
            Ok(Default::default())
        }
        async fn ping(&self) -> Result<(), AppError> {
            Ok(())
        }
        async fn close(&self) {}
        async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
            Ok(Vec::new())
        }
        async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
            Ok(Vec::new())
        }
        async fn list_tables(&self, _schema: &str) -> Result<Vec<TableSummary>, AppError> {
            Ok(Vec::new())
        }
        async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
            Err(AppError::NotFound(table.name.clone()))
        }
        async fn view_definition(&self, _table: &TableRef) -> Result<Option<String>, AppError> {
            Ok(None)
        }
        async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
            Err(AppError::Unsupported("recorder".into()))
        }
        async fn query(&self, sql: &str, _params: &[Value]) -> Result<ResultSet, AppError> {
            panic!("the worker must not run {sql}")
        }
        async fn execute(&self, sql: &str, _params: &[Value]) -> Result<ExecOutcome, AppError> {
            panic!("the worker must not run {sql}")
        }
        async fn transaction(
            &self,
            _statements: Vec<crate::drivers::TxStatement>,
        ) -> Result<Vec<ExecOutcome>, AppError> {
            panic!("the worker must not run a transaction")
        }
        async fn run_statement(
            &self,
            sql: &str,
            _max_rows: usize,
            sink: &mut dyn RowSink,
            cancel: CancelSlot,
        ) -> Result<StatementOutcome, AppError> {
            self.log.lock().unwrap().push(sql.to_string());
            let _armed =
                crate::drivers::arm_cancel(&cancel, crate::drivers::CancelHandle::Requested)?;
            if self.fail.is_some_and(|f| sql.contains(f)) {
                return Err(AppError::InvalidInput(format!("{sql} failed")));
            }
            if self.cancel_during.is_some_and(|f| sql.contains(f)) {
                request_cancel(&cancel);
                return Err(AppError::Cancelled);
            }
            if let Some(vars) = sql.strip_prefix("SELECT @@") {
                let row: Vec<Value> = vars
                    .split(", @@")
                    .map(|v| match v {
                        "autocommit" => Value::Int(self.autocommit),
                        _ => Value::Int(self.mariadb_in_tx.unwrap_or(0)),
                    })
                    .collect();
                sink.begin_set(
                    row.iter()
                        .map(|_| ResultColumn {
                            name: "flag".into(),
                            data_type: "BIGINT".into(),
                            kind: crate::types::schema::ValueKind::Int,
                            nullable: None,
                            table: None,
                            origin: None,
                        })
                        .collect(),
                );
                let _ = sink.rows(vec![row]);
            }
            if sql.starts_with("EXPLAIN") {
                sink.begin_set(vec![ResultColumn {
                    name: "QUERY PLAN".into(),
                    data_type: "text".into(),
                    kind: crate::types::schema::ValueKind::Text,
                    nullable: None,
                    table: None,
                    origin: None,
                }]);
                let _ = sink.rows(vec![vec![Value::Text("Delete on t".into())]]);
            }
            Ok(StatementOutcome::default())
        }
        async fn stream_query(
            &self,
            sql: &str,
            _params: &[Value],
            _sink: &mut dyn RowSink,
            _cancel: CancelSlot,
        ) -> Result<u64, AppError> {
            panic!("the worker must not run {sql}")
        }
        async fn cancel(&self, _handle: crate::drivers::CancelHandle) -> Result<(), AppError> {
            Ok(())
        }
        fn in_transaction(&self) -> Option<bool> {
            self.in_tx
        }
        fn is_mariadb(&self) -> bool {
            self.mariadb_in_tx.is_some()
        }
    }

    const PG_ANALYZE: &str = "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) DELETE FROM t";

    #[tokio::test]
    async fn explain_analyze_runs_in_a_rolled_back_transaction() {
        let slot = CancelSlot::default();
        // No transaction open: one of its own on the editor, rolled back.
        let pg = Recorder::new(Dialect::Postgres, Some(false));
        let plan = explain_inner(&pg, "DELETE FROM t;", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(plan.text.as_deref(), Some("Delete on t"));
        assert_eq!(pg.log(), ["BEGIN", PG_ANALYZE, "ROLLBACK"]);

        let my = Recorder::new(Dialect::Mysql, Some(false));
        explain_inner(&my, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(
            my.log(),
            [
                "SELECT @@autocommit",
                "START TRANSACTION",
                "EXPLAIN ANALYZE DELETE FROM t",
                "ROLLBACK"
            ]
        );

        // MySQL with autocommit off always has a transaction open (START TRANSACTION would
        // commit it): a savepoint, as inside an explicit one.
        let mut my = Recorder::new(Dialect::Mysql, Some(false));
        my.autocommit = 0;
        explain_inner(&my, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(
            my.log(),
            [
                "SELECT @@autocommit",
                "SAVEPOINT tabler_explain",
                "EXPLAIN ANALYZE DELETE FROM t",
                "ROLLBACK TO SAVEPOINT tabler_explain",
                "RELEASE SAVEPOINT tabler_explain"
            ]
        );

        // MariaDB: the server says whether a transaction is open (its `ANALYZE …` throws the
        // session's own tracking off).
        let mut maria = Recorder::new(Dialect::Mysql, Some(false));
        maria.mariadb_in_tx = Some(1);
        explain_inner(&maria, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(
            maria.log(),
            [
                "SELECT @@autocommit, @@in_transaction",
                "SAVEPOINT tabler_explain",
                "ANALYZE FORMAT=JSON DELETE FROM t",
                "ROLLBACK TO SAVEPOINT tabler_explain",
                "RELEASE SAVEPOINT tabler_explain"
            ]
        );
        let mut maria = Recorder::new(Dialect::Mysql, Some(true));
        maria.mariadb_in_tx = Some(0);
        explain_inner(&maria, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(maria.log()[1], "START TRANSACTION");

        // Inside the user's transaction: a savepoint keeps it open with its earlier work.
        let my = Recorder::new(Dialect::Mysql, Some(true));
        explain_inner(&my, "select * from t", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(
            my.log(),
            [
                "SAVEPOINT tabler_explain",
                "EXPLAIN ANALYZE select * from t",
                "ROLLBACK TO SAVEPOINT tabler_explain",
                "RELEASE SAVEPOINT tabler_explain"
            ]
        );

        // A plain EXPLAIN runs nothing around it.
        let pg = Recorder::new(Dialect::Postgres, Some(false));
        explain_inner(&pg, "DELETE FROM t", ExplainMode::Plan, &slot)
            .await
            .unwrap();
        assert_eq!(
            pg.log(),
            ["EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) DELETE FROM t"]
        );
    }

    #[tokio::test]
    async fn explain_analyze_always_rolls_back() {
        // The EXPLAIN fails: rolled back all the same, and its own error is reported.
        let mut pg = Recorder::new(Dialect::Postgres, Some(false));
        pg.fail = Some("EXPLAIN");
        let err = explain_inner(
            &pg,
            "DELETE FROM t",
            ExplainMode::Analyze,
            &CancelSlot::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("EXPLAIN"), "{err}");
        assert_eq!(pg.log(), ["BEGIN", PG_ANALYZE, "ROLLBACK"]);

        // Cancelled while the EXPLAIN runs: the transaction is still rolled back.
        let mut pg = Recorder::new(Dialect::Postgres, Some(false));
        pg.cancel_during = Some("EXPLAIN");
        let slot = CancelSlot::default();
        let err = explain_inner(&pg, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");
        assert!(cancel_requested(&slot));
        assert_eq!(pg.log(), ["BEGIN", PG_ANALYZE, "ROLLBACK"]);

        // Cancelled before it started: nothing runs at all.
        let pg = Recorder::new(Dialect::Postgres, Some(false));
        let slot = CancelSlot::default();
        request_cancel(&slot);
        let err = explain_inner(&pg, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");
        assert!(pg.log().is_empty(), "{:?}", pg.log());

        // A failed rollback to the savepoint is logged, and the savepoint is not released
        // (that would keep the statement's changes); the plan is still shown.
        let mut pg = Recorder::new(Dialect::Postgres, Some(true));
        pg.fail = Some("ROLLBACK TO");
        explain_inner(
            &pg,
            "DELETE FROM t",
            ExplainMode::Analyze,
            &CancelSlot::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            pg.log(),
            [
                "SAVEPOINT tabler_explain",
                PG_ANALYZE,
                "ROLLBACK TO SAVEPOINT tabler_explain"
            ]
        );

        // The transaction could not be opened: nothing else runs.
        let mut pg = Recorder::new(Dialect::Postgres, Some(false));
        pg.fail = Some("BEGIN");
        let err = explain_inner(
            &pg,
            "DELETE FROM t",
            ExplainMode::Analyze,
            &CancelSlot::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("BEGIN"), "{err}");
        assert_eq!(pg.log(), ["BEGIN"]);
    }

    #[tokio::test]
    async fn explain_analyze_refuses_writes_in_an_unknown_transaction_state() {
        let slot = CancelSlot::default();
        let pg = Recorder::new(Dialect::Postgres, None);
        let err = explain_inner(&pg, "DELETE FROM t", ExplainMode::Analyze, &slot)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");
        assert!(pg.log().is_empty(), "{:?}", pg.log());
        // A read still runs (there is nothing safe to wrap it in).
        explain_inner(&pg, "SELECT 1", ExplainMode::Analyze, &slot)
            .await
            .unwrap();
        assert_eq!(
            pg.log(),
            ["EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) SELECT 1"]
        );
    }

    #[tokio::test]
    async fn run_with_explain_analyze_wraps_every_statement() {
        let my = Recorder::new(Dialect::Mysql, Some(false));
        let opts = RunOptions {
            explain: Some(ExplainMode::Analyze),
            stop_on_error: false,
            ..RunOptions::default()
        };
        let (results, _) = run(&my, "delete from a; update b set x = 1;", opts).await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert!(results.iter().all(|r| r.kind == StatementKind::Explain));
        assert_eq!(
            my.log(),
            [
                "SELECT @@autocommit",
                "START TRANSACTION",
                "EXPLAIN ANALYZE delete from a",
                "ROLLBACK",
                "SELECT @@autocommit",
                "START TRANSACTION",
                "EXPLAIN ANALYZE update b set x = 1",
                "ROLLBACK"
            ]
        );

        // Unknown state: the write is refused as that statement's error, nothing runs.
        let my = Recorder::new(Dialect::Mysql, None);
        let opts = RunOptions {
            explain: Some(ExplainMode::Analyze),
            ..RunOptions::default()
        };
        let (results, _) = run(&my, "delete from a", opts.clone()).await;
        assert_eq!(results[0].error.as_ref().unwrap().code, "UNSUPPORTED");
        assert_eq!(my.log(), ["SELECT @@autocommit"]);
        // …unless autocommit is off: then a transaction is open for sure.
        let mut my = Recorder::new(Dialect::Mysql, None);
        my.autocommit = 0;
        let (results, _) = run(&my, "delete from a", opts).await;
        assert!(results[0].error.is_none(), "{results:?}");
        assert_eq!(my.log()[1], "SAVEPOINT tabler_explain");
    }

    #[test]
    fn explain_needs_a_single_statement() {
        let block = "SELECT * FROM t; INSERT INTO t VALUES (42)";
        assert!(matches!(
            check_explainable(block, Dialect::Mysql),
            Err(AppError::InvalidInput(_))
        ));
        assert!(check_explainable("SELECT * FROM t", Dialect::Mysql).is_ok());
        assert!(
            check_explainable(
                "CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END",
                Dialect::Mysql
            )
            .is_ok()
        );
    }

    #[test]
    fn mysql_positions_from_messages() {
        let sql = "select *\nfrm users where id = 1";
        let msg = "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near 'users where id = 1' at line 2";
        assert_eq!(mysql_error_offset(msg, sql), Some(13));
        let msg = "You have an error in your SQL syntax; … near '' at line 1";
        assert_eq!(mysql_error_offset(msg, "select"), Some(6));
        assert_eq!(mysql_error_offset("Unknown column 'x'", sql), None);
        let msg = "… near 'it''s' x' at line 1";
        assert_eq!(mysql_error_offset(msg, "select 'it''s' x"), Some(8));

        // The near text also occurs earlier on the line: the error is at the end.
        let msg = "You have an error in your SQL syntax; … near '10' at line 1";
        assert_eq!(
            mysql_error_offset(msg, "select id from users limit 10 10"),
            Some(30)
        );
        let msg = "… near ')' at line 1";
        assert_eq!(
            mysql_error_offset(msg, "select 1 from dual where (1) and 1)"),
            Some(34)
        );
        // A near text spanning lines (the rest of the statement).
        let sql = "select 1 +\n  from x\nwhere 1";
        let msg = "… near 'from x\nwhere 1' at line 2";
        assert_eq!(mysql_error_offset(msg, sql), Some(13));
        // Cut at 80 chars: the last match starting on the reported line wins.
        let tail = "x".repeat(100);
        let sql = format!("select a a {tail}");
        let near: String = format!("a {tail}").chars().take(80).collect();
        let msg = format!("… near '{near}' at line 1");
        assert_eq!(mysql_error_offset(&msg, &sql), Some(9));
    }

    #[tokio::test]
    async fn results_query_time_out_cancels_the_statement() {
        let (session, _dir) = sqlite_session().await;
        let opts = RunOptions {
            timeout_secs: 1,
            ..RunOptions::default()
        };
        let endless = "with recursive c(x) as (select 1 union all select x + 1 from c \
                       where x < 2000000000) select count(*) from c; select 2";
        let started = Instant::now();
        let (results, events) = run(&session, endless, opts).await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it was cancelled"
        );
        assert_eq!(
            results.len(),
            1,
            "the run stops after a time out: {results:?}"
        );
        let err = results[0].error.as_ref().expect("timed out");
        assert_eq!(err.code, QUERY_TIMEOUT_CODE);
        assert_eq!(err.message, "Query timed out after 1 s");
        assert_eq!(err.statement_index, Some(0));
        assert_eq!(
            event_names(&events),
            ["started", "columns", "failed", "done"]
        );
        // The editor is usable again, and a statement within the limit just runs.
        let opts = RunOptions {
            timeout_secs: 5,
            ..RunOptions::default()
        };
        let (results, _) = run(&session, "select 1", opts).await;
        assert!(results[0].error.is_none(), "{results:?}");
        assert_eq!(
            timeout_payload(300, 2).message,
            "Query timed out after 300 s"
        );
    }

    const ENDLESS: &str = "with recursive c(x) as (select 1 union all select x + 1 from c \
                           where x < 2000000000) select count(*) from c";

    #[tokio::test]
    async fn query_limited_cancels_a_slow_read_and_reports_the_time_out() {
        let (session, _dir) = sqlite_session().await;
        let started = Instant::now();
        let err = query_limited(&session, ENDLESS, &[], Some(Duration::from_secs(1)))
            .await
            .unwrap_err();
        let took = started.elapsed();
        assert!(took >= Duration::from_secs(1), "{took:?}");
        assert!(took < Duration::from_secs(10), "it was cancelled: {took:?}");
        let payload = err.to_payload();
        assert_eq!(payload.code, QUERY_TIMEOUT_CODE);
        assert_eq!(payload.message, "Query timed out after 1 s");
        assert!(payload.hint.as_deref().unwrap().contains("Query time out"));
        // The worker connection is usable again, and the cancelled statement is in the trace.
        let rs = session.query("select 41 + 1", &[]).await.unwrap();
        assert_eq!(rs.rows, vec![vec![Value::Int(42)]]);
        let trace = session.drain_trace();
        let cancelled = trace
            .iter()
            .find(|t| t.sql.contains("with recursive"))
            .unwrap();
        assert!(cancelled.error.is_some(), "{trace:?}");
    }

    #[tokio::test]
    async fn query_limited_leaves_fast_and_unlimited_statements_alone() {
        let (session, _dir) = sqlite_session().await;
        let fast = query_limited(&session, "select 7", &[], Some(Duration::from_secs(30)))
            .await
            .unwrap();
        assert_eq!(fast.rows, vec![vec![Value::Int(7)]]);
        // No limit: a statement that outlasts what a limit would have been just completes.
        let slow = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 3000000) \
                    select count(*) from c";
        let started = Instant::now();
        let done = query_limited(&session, slow, &[], None).await.unwrap();
        assert_eq!(done.rows, vec![vec![Value::Int(3_000_000)]]);
        assert!(started.elapsed() > Duration::from_millis(50));
        // An ordinary failure is not turned into a time out.
        let err = query_limited(
            &session,
            "select * from no_such",
            &[],
            Some(Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "DB_ERROR");
    }

    #[test]
    fn the_query_time_out_is_read_from_the_settings() {
        use crate::appdb::AppDb;
        let db = AppDb::open_in_memory().unwrap();
        // Preference absent: the default 300 s.
        assert_eq!(query_time_limit(&db, None), Some(Duration::from_secs(300)));
        db.set_setting("prefs", &serde_json::json!({ "queryTimeout": 45 }))
            .unwrap();
        assert_eq!(query_time_limit(&db, None), Some(Duration::from_secs(45)));
        // 0 = no limit.
        db.set_setting("prefs", &serde_json::json!({ "queryTimeout": 0 }))
            .unwrap();
        assert_eq!(query_time_limit(&db, None), None);
        // An invalid value keeps the default.
        db.set_setting("prefs", &serde_json::json!({ "queryTimeout": "soon" }))
            .unwrap();
        assert_eq!(query_time_limit(&db, None), Some(Duration::from_secs(300)));
        // An explicit argument wins over the preference, 0 included.
        assert_eq!(query_time_limit(&db, Some(5)), Some(Duration::from_secs(5)));
        assert_eq!(query_time_limit(&db, Some(0)), None);
        assert_eq!(time_limit(0), None);
        assert_eq!(time_limit(1), Some(Duration::from_secs(1)));
    }

    #[test]
    fn the_timeout_error_has_its_own_code_and_a_hint() {
        let e = AppError::QueryTimeout { secs: 300 };
        assert_eq!(e.code(), QUERY_TIMEOUT_CODE);
        assert_ne!(e.code(), AppError::Timeout.code());
        assert_eq!(e.to_string(), "Query timed out after 300 s");
        let p = timeout_payload(300, 2);
        assert_eq!(
            (p.code.as_str(), p.statement_index),
            (QUERY_TIMEOUT_CODE, Some(2))
        );
        assert_eq!(p.hint, e.to_payload().hint);
        // Only a cancelled statement is reported as timed out, and only once the limit passed.
        let limit = Some(Duration::from_secs(3));
        assert!(matches!(
            timed_out_as_error::<()>(Err(AppError::Cancelled), true, limit),
            Err(AppError::QueryTimeout { secs: 3 })
        ));
        assert!(matches!(
            timed_out_as_error::<()>(Err(AppError::Cancelled), false, limit),
            Err(AppError::Cancelled)
        ));
        assert!(timed_out_as_error(Ok(1), true, limit).is_ok());
    }

    #[tokio::test]
    async fn explain_under_a_limit_still_explains() {
        let (session, _dir) = sqlite_session().await;
        let plan = explain_limited(
            &session,
            "select 1",
            ExplainMode::Plan,
            &CancelSlot::default(),
            Some(Duration::from_secs(30)),
        )
        .await
        .unwrap();
        assert!(!plan.columns.is_empty());
    }

    #[tokio::test]
    async fn results_columns_report_their_origin() {
        let (session, _dir) = sqlite_session().await;
        let (results, _) = run(
            &session,
            "create table users (id integer primary key, email text); \
             insert into users (email) values ('a@x');",
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let (results, events) = run(
            &session,
            "select u.id as uid, email, 1 as one from users u",
            RunOptions::default(),
        )
        .await;
        let columns = match &events[1] {
            QueryEvent::Columns { columns, .. } => columns.clone(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(columns, results[0].columns);
        let origin = |i: usize| {
            columns[i]
                .origin
                .as_ref()
                .map(|o| (o.table.name.as_str(), o.column.as_str()))
        };
        assert_eq!(origin(0), Some(("users", "id")));
        assert_eq!(columns[0].name, "uid");
        assert_eq!(origin(1), Some(("users", "email")));
        assert_eq!(origin(2), None);
    }

    #[test]
    fn sqlite_plan_tree() {
        let col = |n: &str| ResultColumn {
            name: n.into(),
            data_type: String::new(),
            kind: crate::types::schema::ValueKind::Other,
            nullable: None,
            table: None,
            origin: None,
        };
        let set = ResultSet {
            columns: vec![col("id"), col("parent"), col("notused"), col("detail")],
            rows: vec![
                vec![
                    Value::Int(2),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Text("SCAN t".into()),
                ],
                vec![
                    Value::Int(5),
                    Value::Int(2),
                    Value::Int(0),
                    Value::Text("USE INDEX".into()),
                ],
            ],
            truncated: false,
        };
        assert_eq!(
            plan_text(Dialect::Sqlite, &set).unwrap(),
            "SCAN t\n  USE INDEX"
        );
        assert_eq!(effective_max_rows(0), 10_000);
        assert_eq!(effective_max_rows(5), 5);
        assert_eq!(effective_max_rows(u32::MAX), 1_000_000);
        assert_eq!(history_sql("abc"), "abc");
    }
}
