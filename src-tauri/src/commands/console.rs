//! The console of a store that speaks no SQL (Redis, MongoDB, Elasticsearch, DynamoDB): `run_sql`
//! hands a native connection's editor text here. It runs like the SQL editor's text: the store
//! splits it into commands, each streams its result through the same `QueryEvent`s (`started`,
//! `columns` / `rows`, `finished` / `failed`, then `done`), the run stops at the first error when
//! asked to, and a command still running past the query time out is cancelled through the
//! session and reported as timed out. `run_sql` records the history as for SQL.

use std::time::{Duration, Instant};

use crate::commands::query::{
    EventSink, effective_max_rows, explain_feature, now_rfc3339, timeout_payload, with_timeout_by,
};
use crate::commands::safety::refuse_read_only_console;
use crate::drivers::native::NativeSession;
use crate::drivers::{CancelSlot, lock_runs};
use crate::error::AppError;
use crate::state::Connection;
use crate::types::caps::Feature;
use crate::types::query::{QueryEvent, RunOptions, StatementKind, StatementResult, StatementSpan};

/// What a console run needs before anything is sent: the editor, no EXPLAIN (a console has
/// none), and on a read-only connection only commands the store says read.
pub fn check_console(
    conn: &Connection,
    store: &dyn NativeSession,
    text: &str,
    options: &RunOptions,
) -> Result<(), AppError> {
    let caps = store.capabilities();
    caps.require(Feature::Editor)?;
    if let Some(mode) = options.explain {
        return Err(AppError::Unsupported(format!(
            "{}: {} is not available",
            caps.engine(),
            explain_feature(mode).label()
        )));
    }
    refuse_read_only_console(conn, store, text)
}

/// Runs each command of `text` on `store`, emitting the events of `run_sql_inner`. The run keeps
/// the store's console for all its commands (`console_lock`), so another tab's commands cannot
/// land in between.
pub async fn run_console_inner(
    store: &dyn NativeSession,
    text: &str,
    options: &RunOptions,
    cancel: &CancelSlot,
    emit: &mut (dyn FnMut(QueryEvent) + Send),
) -> Vec<StatementResult> {
    let run_started = Instant::now();
    let time_limit =
        (options.timeout_secs > 0).then(|| Duration::from_secs(options.timeout_secs.into()));
    let max_rows = effective_max_rows(options.max_rows);
    let commands = store.split_console(text);
    let mut results = Vec::with_capacity(commands.len());
    // Err: cancelled while waiting for another run; the first command reports it.
    let console = lock_runs(store.console_lock(), cancel).await;

    for (index, command) in commands.iter().enumerate() {
        let span = StatementSpan {
            start: command.start as u32,
            end: command.end as u32,
            line: command.line,
        };
        emit(QueryEvent::Started {
            index,
            sql: command.sql.clone(),
            span,
        });
        let started_at = now_rfc3339();
        let started = Instant::now();
        let mut sink = EventSink::new(index, &mut *emit);
        let mut timed_out = false;
        let outcome = if console.is_err() {
            Err(AppError::Cancelled)
        } else {
            let run = store.run_console(&command.sql, max_rows, &mut sink, cancel.clone());
            let (outcome, expired) =
                with_timeout_by(|handle| store.cancel(handle), cancel, time_limit, run).await;
            timed_out = expired;
            outcome
        };
        let (first_columns, sets, delivered) = (sink.first_columns, sink.sets, sink.rows);

        let mut result = StatementResult {
            index,
            sql: command.sql.clone(),
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
                result.kind = outcome.kind;
                result.result_sets = outcome.result_sets;
                result.row_count = outcome.row_count;
                result.truncated = outcome.truncated;
                result.affected = outcome.affected;
                result.last_insert_id = outcome.last_insert_id;
                result.notices = outcome.notices;
                if timed_out {
                    result.notices.push(format!(
                        "The query time out ({} s) passed, but the command completed before it \
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
                    let mut payload = err.with_statement_index(index).to_payload();
                    payload.statement_index = Some(index);
                    payload
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
    drop(console);
    emit(QueryEvent::Done {
        total_ms: run_started.elapsed().as_millis() as u64,
    });
    results
}
