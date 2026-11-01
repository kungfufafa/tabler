//! The editor's runs, on the session's own connection, as simple queries (one statement each:
//! the editor splits its text first). INSERT, UPDATE, DELETE and MERGE answer with one `OUTPUT`
//! row holding their count, which the editor reports as the rows affected instead of a result.
//!
//! At the row cap a read is stopped on the server with a CancelRequest; anything else runs to its
//! end without its rows being kept, so what it writes is not cut in half. The run waits until the
//! server has the cancel request (it hangs up once it does), so a late request can never reach
//! the next statement. The transaction state comes from the server's ReadyForQuery. A run whose
//! future was dropped leaves the connection owing replies: the next run (or a ping) finds it
//! broken and the editor lost, as after a dropped connection.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::task::JoinHandle;

use super::conn::{Flow, Item, Summary};
use super::connect::{Conn, Dial};
use super::decode;
use super::session::{DIALECT, VerticaSession, cancel_handle, refuse_several, settle_user_cancels};
use super::wire::BackendKey;
use crate::drivers::{
    CancelSlot, LATE_CANCEL_NOTICE, RowChunker, RowSink, StatementOutcome, arm_cancel,
    cancel_requested, cancel_safe_on_stop, cap_cancel_notice, is_dml,
};
use crate::error::AppError;
use crate::sqlgen::sql::calls_side_effect_function;
use crate::types::query::StatementKind;
use crate::types::value::Value;

type Pending = Option<JoinHandle<Result<(), AppError>>>;

/// Starts a cancel request for `key` (none without a key) without waiting for it.
fn stop_on_server(dial: &Arc<Dial>, key: Option<BackendKey>, pending: &mut Pending) {
    if pending.is_some() {
        return;
    }
    if let Some(key) = key {
        let dial = Arc::clone(dial);
        *pending = Some(tokio::spawn(async move { dial.cancel(key).await }));
    }
}

/// Waits until the server has the cancel request a run started.
async fn settle(pending: Pending) {
    if let Some(handle) = pending {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("Vertica: stopping a read at its row cap failed: {e}"),
            Err(e) => log::warn!("Vertica: the cancel request's task failed: {e}"),
        }
    }
}

/// The count of an `OUTPUT` row.
fn count_of(row: &[Value]) -> Option<u64> {
    match row.first()? {
        Value::Int(n) => u64::try_from(*n).ok(),
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The count of a tag that carries one (`COPY 3`).
fn tag_count(summary: &Summary) -> Option<u64> {
    let tag = summary.tags.last()?;
    let (_, n) = tag.rsplit_once(' ')?;
    n.parse().ok()
}

/// Runs one statement on the editor.
pub(super) async fn run(
    session: &VerticaSession,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    refuse_several(sql)?;
    session.refuse_write(sql)?;
    let mut editor = session.editor.lock().await;
    let conn = match editor.as_mut() {
        Some(conn) if !conn.is_broken() => conn,
        _ => {
            lose_editor(session, &mut editor);
            return Err(AppError::NotConnected);
        }
    };
    let key = conn.key();
    let armed = arm_cancel(&cancel, cancel_handle(&session.dial, key))?;
    session.editor_busy.store(true, Ordering::SeqCst);

    let dml = is_dml(sql);
    let stop_safe = cancel_safe_on_stop(sql, DIALECT);
    let mut chunker = RowChunker::new(sink);
    let mut truncated = false;
    let mut counting = false;
    let mut affected: Option<u64> = None;
    let mut pending: Pending = None;
    let result = {
        let mut on_item = |item: Item<'_>| -> Flow {
            let flow = match item {
                // A DML statement's count, not a result.
                Item::Columns([f]) if dml && f.name.eq_ignore_ascii_case("OUTPUT") => {
                    counting = true;
                    return Flow::Continue;
                }
                Item::Columns(fields) => {
                    counting = false;
                    chunker.begin_set(fields.iter().map(decode::result_column).collect())
                }
                Item::Row(row) if counting => {
                    affected = count_of(&row);
                    return Flow::Continue;
                }
                Item::Row(_) if chunker.total_rows >= max_rows as u64 => {
                    truncated = true;
                    ControlFlow::Break(())
                }
                Item::Row(row) => chunker.push(row),
                Item::Done(_) => return Flow::Continue,
            };
            if flow.is_break() {
                if stop_safe {
                    stop_on_server(&session.dial, key, &mut pending);
                }
                return Flow::Stop;
            }
            Flow::Continue
        };
        conn.inner.simple(sql, &mut on_item).await
    };
    let _ = chunker.flush();
    let (sets, rows) = (chunker.sets, chunker.total_rows);
    let stopped = pending.is_some();
    settle(pending).await;
    drop(armed);
    // The user's cancel too, before the editor's next statement (a COMMIT, say) can be hit.
    settle_user_cancels(&cancel).await;
    session.editor_busy.store(false, Ordering::SeqCst);
    let notices: Vec<String> = conn
        .inner
        .take_notices()
        .into_iter()
        .map(|n| n.message)
        .collect();
    if conn.is_broken() {
        lose_editor(session, &mut editor);
    } else {
        session.tx.set(Some(conn.inner.in_transaction()));
    }
    let mut outcome = StatementOutcome {
        kind: StatementKind::Empty,
        result_sets: sets,
        row_count: rows,
        truncated,
        affected: None,
        last_insert_id: None,
        notices,
    };
    match result {
        Ok(summary) => {
            if counting || (dml && sets == 0) {
                outcome.kind = StatementKind::Affected;
                outcome.affected = affected.or_else(|| tag_count(&summary));
            } else if sets > 0 {
                outcome.kind = StatementKind::Rows;
            }
        }
        // The read stopped at the row cap.
        Err(AppError::Cancelled) if stopped && !cancel_requested(&cancel) => {
            outcome.kind = StatementKind::Rows;
            if calls_side_effect_function(DIALECT, sql) {
                outcome.notices.push(cap_cancel_notice(rows, DIALECT));
            }
        }
        Err(e) => return Err(e),
    }
    if cancel_requested(&cancel) {
        // The statement ended before the cancel reached it: a read reports what the user asked
        // for, anything else its real outcome, since its changes stand.
        if outcome.kind == StatementKind::Rows && stop_safe {
            return Err(AppError::Cancelled);
        }
        outcome.notices.push(LATE_CANCEL_NOTICE.to_string());
    }
    Ok(outcome)
}

/// The editor's connection broke: its session (the `SET`s and transaction of the user's) went
/// with it, so the editor is not reopened behind the user's back; `ping` reports it.
pub(super) fn lose_editor(session: &VerticaSession, editor: &mut Option<Conn>) {
    editor.take();
    session.editor_lost.store(true, Ordering::SeqCst);
    session.tx.set(None);
}

/// Streams every row of a query on a worker (`stream_query`): no row cap. When the sink has
/// enough, a read is stopped on the server and anything else drained.
pub(super) async fn stream_rows(
    dial: &Arc<Dial>,
    key: Option<BackendKey>,
    conn: &mut Conn,
    sql: &str,
    params: &[Value],
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
) -> Result<u64, AppError> {
    let stop_safe = cancel_safe_on_stop(sql, DIALECT);
    let mut chunker = RowChunker::new(sink);
    let mut stopped = false;
    let mut pending: Pending = None;
    let result = {
        let mut on_item = |item: Item<'_>| -> Flow {
            if stopped {
                return Flow::Stop;
            }
            let flow = match item {
                Item::Columns(fields) => {
                    chunker.begin_set(fields.iter().map(decode::result_column).collect())
                }
                Item::Row(row) => chunker.push(row),
                Item::Done(_) => ControlFlow::Continue(()),
            };
            if flow.is_break() {
                stopped = true;
                if stop_safe {
                    stop_on_server(dial, key, &mut pending);
                }
                return Flow::Stop;
            }
            Flow::Continue
        };
        conn.inner.run(sql, params, &mut on_item).await
    };
    if !stopped {
        let _ = chunker.flush();
    }
    let rows = chunker.total_rows;
    settle(pending).await;
    match result {
        Ok(_) => Ok(rows),
        Err(AppError::Cancelled) if stopped && !cancel_requested(cancel) => Ok(rows),
        Err(e) => Err(e),
    }
}
