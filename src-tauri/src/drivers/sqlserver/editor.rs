//! The editor's runs, on the session's own connection. The editor's splitter cuts its text at
//! `GO` lines (`sqlgen::portable::split`), and each batch is sent whole as a SQL batch: CREATE
//! PROCEDURE and its kind must start their batch, and `USE`, `SET` and temporary tables last for
//! the session, which `sp_executesql` would undo when it ends.
//!
//! At the row cap a batch that is one read is stopped on the server (an Attention); any other
//! runs to its end without its rows being read, so what it writes is not cut in half. After each
//! batch `tsql::AFTER_BATCH` reads `@@ROWCOUNT` (the rows of a lone DML statement) and
//! `@@TRANCOUNT` (whether a transaction is open), since tiberius reports neither.
//!
//! The driver's own statements around a batch (that probe, plan output on and off) answer at
//! once: one that does not within `OWN_WITHIN` gives the editor's connection up, so a stalled
//! network ends the run instead of holding it until TCP gives up.

use std::ops::ControlFlow;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ::tiberius::ColumnData;

use super::conn::{Conn, Item, Mode, Stop};
use super::decode::Bytes;
use super::introspect::{AfterBatch, parse_after_batch};
use super::session::{DIALECT, MssqlSession, cancel_handle, lock};
use crate::drivers::{
    CancelSlot, LATE_CANCEL_NOTICE, RowChunker, RowSink, StatementOutcome, arm_cancel,
    cancel_requested, cancel_safe_on_stop, cap_cancel_notice,
};
use crate::error::AppError;
use crate::sqlgen::dialect::tsql;
use crate::sqlgen::sql::calls_side_effect_function;
use crate::types::query::{ExplainMode, ResultColumn, ResultSet, StatementKind};

/// How long one of the driver's own statements on the editor may take.
const OWN_WITHIN: Duration = Duration::from_secs(10);

/// Runs one batch (or the Explain of one statement, `tsql::explain_of`) on the editor.
pub(super) async fn run(
    session: &MssqlSession,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let explain = tsql::explain_of(sql);
    match explain {
        // A plan does not run its statement, so the plan of one INSERT, UPDATE, DELETE or MERGE
        // that reaches no other server is let through; anything else must read, since a `SET`
        // (or a `USE`) among the statements could still take effect while plans are on.
        Some((ExplainMode::Plan, statement)) if tsql::plan_writes_nothing(statement) => {}
        Some((ExplainMode::Plan, statement)) => session.refuse_write(statement)?,
        Some((ExplainMode::Analyze, statement)) => session.refuse_write(statement)?,
        // What an Explain Analyze runs in reads nothing and writes nothing.
        None if is_analyze_wrapper(sql) => {}
        None => session.refuse_write(sql)?,
    }
    let mut editor = session.editor.lock().await;
    let conn = match editor.as_ref() {
        Some(conn) if !conn.is_broken() => conn.clone(),
        _ => {
            lose_editor(session, &mut editor);
            return Err(AppError::NotConnected);
        }
    };
    let result = {
        let stop = Stop::new();
        let _armed = arm_cancel(&cancel, cancel_handle(&stop))?;
        *lock(&session.editor_stop) = Some(stop.clone());
        let result = match explain {
            None => batch(&conn, sql, max_rows, sink, &stop, &cancel, false).await,
            Some((mode, statement)) => {
                explained(&conn, mode, statement, max_rows, sink, &stop, &cancel).await
            }
        };
        *lock(&session.editor_stop) = None;
        result
    };
    if conn.is_broken() {
        lose_editor(session, &mut editor);
        return result;
    }
    let after = after_batch(&conn, &cancel).await;
    if conn.is_broken() {
        // The probe went unanswered: the batch's outcome stands, the session is lost.
        lose_editor(session, &mut editor);
    } else {
        session.tx.set(after.in_transaction);
    }
    let mut outcome = result?;
    if outcome.result_sets == 0 && explain.is_none() && tsql::is_lone_dml(sql) {
        outcome.kind = StatementKind::Affected;
        outcome.affected = after.rows;
    }
    if cancel_requested(&cancel) {
        // The batch ended before the cancel reached it: a read reports what the user asked for,
        // anything else its real outcome, since its changes stand.
        if outcome.kind == StatementKind::Rows && cancel_safe_on_stop(sql, DIALECT) {
            return Err(AppError::Cancelled);
        }
        outcome.notices.push(LATE_CANCEL_NOTICE.to_string());
    }
    Ok(outcome)
}

/// The editor's connection broke: its session (the `USE`, options and transaction of the user's)
/// went with it, so the editor is not reopened behind the user's back; `ping` reports it.
fn lose_editor(session: &MssqlSession, editor: &mut Option<Conn>) {
    editor.take();
    session.editor_lost.store(true, Ordering::SeqCst);
    session.tx.set(None);
}

/// `BEGIN TRANSACTION` / `ROLLBACK TRANSACTION`, which an Explain Analyze runs around the
/// statement (`SqlKnobs::analyze_tx`).
fn is_analyze_wrapper(sql: &str) -> bool {
    tsql::SPEC
        .knobs
        .analyze_tx
        .is_some_and(|(open, close)| [open, close].contains(&sql.trim()))
}

/// Runs one of the driver's own statements on the editor within `OWN_WITHIN`; `cancel` stops it
/// like the user's statement. One that does not answer in time is stopped (the actor drops the
/// connection when even the Attention goes unanswered), and its connection is given up.
async fn own(
    conn: &Conn,
    sql: &str,
    cancel: Option<&CancelSlot>,
) -> Result<Vec<ResultSet>, AppError> {
    let stop = Stop::new();
    // A run cancelled already still reads its state, bounded by the time alone.
    let _armed = cancel.and_then(|c| arm_cancel(c, cancel_handle(&stop)).ok());
    let query = conn.query(sql, Vec::new(), Mode::Batch, Bytes::Preview, stop.clone());
    match tokio::time::timeout(OWN_WITHIN, query).await {
        Ok(result) => result,
        Err(_) => {
            stop.fire();
            conn.abandon();
            Err(AppError::Timeout)
        }
    }
}

/// The editor's `@@ROWCOUNT` and `@@TRANCOUNT` after a batch. A cancel or time limit that comes
/// now stops the probe, and the transaction is then unknown.
async fn after_batch(conn: &Conn, cancel: &CancelSlot) -> AfterBatch {
    match own(conn, tsql::AFTER_BATCH, Some(cancel)).await {
        Ok(sets) => parse_after_batch(&sets.into_iter().next().unwrap_or_default().rows),
        Err(e) => {
            log::warn!("SQL Server: reading the editor's row count and transaction failed: {e}");
            AfterBatch {
                rows: None,
                in_transaction: None,
            }
        }
    }
}

/// The plan of one statement: plan output on in a batch of its own, the statement, plan output
/// off again. An Explain Analyze forwards only the plan, not the statement's own rows. A cancel
/// stops the statement only: one that stopped a switch could leave plan output on.
///
/// A plan does not run its statement only while the server honours `SHOWPLAN_XML`, which one
/// may accept and ignore (Babelfish does by default): `tsql::PLAN_PROBE` must come back as a
/// plan before the statement is sent, and a Plan that brings no plan back fails.
async fn explained(
    conn: &Conn,
    mode: ExplainMode,
    statement: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    stop: &Stop,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let switch = |on: bool| own(conn, tsql::showplan(mode, on), None);
    switch(true).await?;
    if mode == ExplainMode::Plan
        && let Err(e) = plan_output_is_on(conn).await
    {
        if !conn.is_broken()
            && let Err(off) = switch(false).await
        {
            log::warn!("SQL Server: turning plan output off failed: {off}");
            conn.abandon();
        }
        return Err(e);
    }
    let result = batch(
        conn,
        statement,
        max_rows,
        sink,
        stop,
        cancel,
        mode == ExplainMode::Analyze,
    )
    .await
    .and_then(|outcome| {
        if mode == ExplainMode::Plan && outcome.result_sets == 0 {
            Err(AppError::Unsupported(NO_PLAN.into()))
        } else {
            Ok(outcome)
        }
    });
    if !conn.is_broken()
        && let Err(e) = switch(false).await
    {
        // Left on, every later batch would only be planned: the editor's connection goes.
        log::warn!("SQL Server: turning plan output off failed: {e}");
        conn.abandon();
    }
    // The server places errors in the statement, which follows the prefix and a space.
    let shift = tsql::explain_prefix(mode).chars().count() + 1;
    result.map_err(|mut e| {
        if let AppError::Db {
            position: Some(position),
            ..
        } = &mut e
        {
            position.offset = position.offset.saturating_add(shift as u32);
        }
        e
    })
}

const NO_PLAN: &str = "SQL Server returned no plan for this statement";

const PLAN_IGNORED: &str = "This server accepted SET SHOWPLAN_XML ON but still runs statements, \
                            so Explain did not send the statement";

/// Whether the server plans instead of running: `tsql::PLAN_PROBE` must come back as one plan.
async fn plan_output_is_on(conn: &Conn) -> Result<(), AppError> {
    let sets = own(conn, tsql::PLAN_PROBE, None).await?;
    match sets.as_slice() {
        [set] if is_plan(&set.columns) => Ok(()),
        _ => Err(AppError::Unsupported(PLAN_IGNORED.into())),
    }
}

fn is_plan(columns: &[ResultColumn]) -> bool {
    matches!(columns, [c] if c.name == tsql::SHOWPLAN_COLUMN)
}

/// Streams one batch into `sink`, up to `max_rows` rows. `plans_only`: forward only the result
/// sets that hold a plan.
async fn batch(
    conn: &Conn,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    stop: &Stop,
    cancel: &CancelSlot,
    plans_only: bool,
) -> Result<StatementOutcome, AppError> {
    let mut running = conn
        .stream(sql, Vec::new(), Mode::Batch, Bytes::Preview, stop.clone())
        .await?;
    let mut chunker = RowChunker::new(sink);
    let mut truncated = false;
    let mut stopped = false;
    let mut skipping = false;
    while let Some(item) = running.items.recv().await {
        let flow = match item {
            Item::Columns(columns) => {
                skipping = plans_only && !is_plan(&columns);
                if skipping {
                    continue;
                }
                chunker.begin_set(columns)
            }
            Item::Row(_) if skipping => continue,
            Item::Row(_) if chunker.total_rows >= max_rows as u64 => {
                truncated = true;
                ControlFlow::Break(())
            }
            Item::Row(row) => chunker.push(row),
        };
        if flow.is_break() {
            if cancel_safe_on_stop(sql, DIALECT) {
                stopped = true;
                stop.fire();
            }
            break;
        }
    }
    let _ = chunker.flush();
    let (sets, rows) = (chunker.sets, chunker.total_rows);
    let mut notices = Vec::new();
    match running.finish().await {
        Ok(_) => {}
        // The read stopped at the row cap.
        Err(AppError::Cancelled) if stopped && !cancel_requested(cancel) => {
            if calls_side_effect_function(DIALECT, sql) {
                notices.push(cap_cancel_notice(rows, DIALECT));
            }
        }
        Err(e) => return Err(e),
    }
    Ok(StatementOutcome {
        kind: if sets > 0 {
            StatementKind::Rows
        } else {
            StatementKind::Empty
        },
        result_sets: sets,
        row_count: rows,
        truncated,
        affected: None,
        last_insert_id: None,
        notices,
    })
}

/// Streams every row of a query on a worker (`stream_query`): whole values, no row cap. When the
/// sink has enough, a single read is stopped on the server and anything else drained.
pub(super) async fn stream_rows(
    conn: &Conn,
    sql: &str,
    params: Vec<ColumnData<'static>>,
    sink: &mut dyn RowSink,
    stop: &Stop,
    cancel: &CancelSlot,
) -> Result<u64, AppError> {
    let mut running = conn
        .stream(sql, params, Mode::Rpc, Bytes::Whole, stop.clone())
        .await?;
    let mut chunker = RowChunker::new(sink);
    let mut stopped = false;
    while let Some(item) = running.items.recv().await {
        let flow = match item {
            Item::Columns(columns) => chunker.begin_set(columns),
            Item::Row(row) => chunker.push(row),
        };
        if flow.is_break() {
            stopped = true;
            if cancel_safe_on_stop(sql, DIALECT) {
                stop.fire();
            }
            break;
        }
    }
    if !stopped {
        let _ = chunker.flush();
    }
    let rows = chunker.total_rows;
    match running.finish().await {
        Ok(_) => Ok(rows),
        Err(AppError::Cancelled) if stopped && !cancel_requested(cancel) => Ok(rows),
        Err(e) => Err(e),
    }
}
