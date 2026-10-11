//! SQL editor commands: run a script statement by statement with streamed results, cancel it,
//! explain a statement. The orchestration lives in `run_sql_inner` / `explain_inner`, which
//! only need a `DbSession`, so they are tested without Tauri.

use std::ops::ControlFlow;
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use tauri::State;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::drivers::{
    CancelSlot, CollectSink, DbSession, RowSink, lock_editor_runs, request_cancel,
};
use crate::error::{AppError, ErrorPayload, ErrorPos};
use crate::sqlgen::Dialect;
use crate::sqlgen::explain::{explain_offset, wrap_explain};
use crate::sqlgen::split::{
    SplitStatement, is_meta_command, is_single_statement, split_statements,
};
use crate::state::{AppState, Connection};
use crate::types::history::NewHistoryEntry;
use crate::types::query::{
    DEFAULT_MAX_ROWS, ExplainMode, ExplainResult, QueryEvent, ResultColumn, ResultSet, RunOptions,
    StatementKind, StatementResult, StatementSpan,
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

/// Splits `sql` and runs each statement on the session's editor connection, emitting
/// `started`, `columns`/`rows`, `finished`/`failed` per statement and `done` at the end.
/// Stops at the first error when `stop_on_error` is set, and always after a cancellation
/// (`cancel` is the slot `cancel_query` uses for this run).
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
    let run_started = Instant::now();
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
        let outcome = if is_meta_command(&st.sql) {
            Err(meta_command_error())
        } else if editor.is_err() {
            Err(AppError::Cancelled)
        } else if options.explain.is_some()
            && let Err(e) = check_explainable(&st.sql, dialect)
        {
            Err(e)
        } else {
            session
                .run_statement(&exec_sql, max_rows, &mut sink, cancel.clone())
                .await
        };
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
                emit(QueryEvent::Finished {
                    index,
                    result: result.clone(),
                });
                false
            }
            Err(err) => {
                let cancelled = matches!(err, AppError::Cancelled);
                let payload = statement_error(err, index, st, sql, &exec_sql, shift, dialect);
                result.error = Some(payload.clone());
                emit(QueryEvent::Failed {
                    index,
                    error: payload,
                });
                cancelled || options.stop_on_error
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
    session
        .run_statement(&exec_sql, EXPLAIN_MAX_ROWS, &mut sink, cancel.clone())
        .await
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
) -> Result<ExplainResult, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
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
        explain_inner(session.as_ref(), &sql, mode, &slot).await
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

    #[test]
    fn sqlite_plan_tree() {
        let col = |n: &str| ResultColumn {
            name: n.into(),
            data_type: String::new(),
            kind: crate::types::schema::ValueKind::Other,
            nullable: None,
            table: None,
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
