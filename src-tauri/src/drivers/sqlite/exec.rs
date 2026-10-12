//! SQL-editor execution and row streaming for SQLite.
//!
//! Statements run on an actor thread; rows travel back to the async caller through a small
//! bounded channel (backpressure) and are handed to the caller's `RowSink` there.

use std::ops::ControlFlow;
use std::sync::Arc;

use rusqlite::Connection;
use rusqlite::types::ToSqlOutput;
use tokio::sync::mpsc;

use super::actor::Actor;
use super::decode::{cell_value, columns_of, to_sql};
use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelHandle, CancelSlot, RowChunker, RowSink, StatementOutcome, TxState,
    arm_cancel, cancel_requested, is_dml, settle_cancels_blocking,
};
use crate::error::AppError;
use crate::sqlgen::split::leading_keywords;
use crate::types::query::{ResultColumn, StatementKind};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// How often (in rows) the actor looks for a pending cancel request between steps; a single long
/// step is stopped by `sqlite3_interrupt` instead.
const CANCEL_CHECK_EVERY: u64 = 256;

/// Chunks in flight between the actor thread and the async side.
const CHANNEL_DEPTH: usize = 4;

pub(crate) enum LiteMsg {
    Columns(Vec<ResultColumn>),
    Rows(Vec<Vec<Value>>),
}

/// Actor-side sink: forwards to the async side, `Break` once the receiver is gone.
struct ChannelSink {
    tx: mpsc::Sender<LiteMsg>,
}

impl RowSink for ChannelSink {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        let _ = self.tx.blocking_send(LiteMsg::Columns(columns));
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        match self.tx.blocking_send(LiteMsg::Rows(rows)) {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        }
    }
}

/// rusqlite reports syntax-error positions as byte offsets; the editor needs char offsets.
fn with_char_offset(mut err: AppError, sql: &str) -> AppError {
    if let AppError::Db { position, .. } = &mut err
        && let Some(pos) = position
    {
        let byte = pos.offset as usize;
        if byte > sql.len() {
            *position = None;
        } else {
            let mut b = byte;
            while !sql.is_char_boundary(b) {
                b -= 1;
            }
            pos.offset = sql[..b].chars().count() as u32;
        }
    }
    err
}

/// Runs one statement on `conn`, streaming rows into `sink`. Runs on the actor thread.
pub(crate) fn execute_streaming(
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
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| with_char_offset(e.into(), sql))?;
    let bound: Vec<ToSqlOutput<'static>> = params.iter().map(to_sql).collect::<Result<_, _>>()?;
    let mut outcome = StatementOutcome::default();
    let changes_before = conn.total_changes();

    if stmt.column_count() > 0 {
        let columns = columns_of(conn, &stmt, sql);
        let kinds: Vec<ValueKind> = columns.iter().map(|c| c.kind).collect();
        let mut chunker = RowChunker::new(sink);
        let _ = chunker.begin_set(columns);
        outcome.kind = StatementKind::Rows;
        outcome.result_sets = 1;
        {
            let mut rows = stmt
                .query(rusqlite::params_from_iter(bound.iter()))
                .map_err(|e| with_char_offset(e.into(), sql))?;
            loop {
                let row = match rows.next() {
                    Ok(Some(row)) => row,
                    Ok(None) => break,
                    Err(e) => {
                        let _ = chunker.flush();
                        return Err(e.into());
                    }
                };
                if let Some(cap) = max_rows
                    && chunker.rows_in_set as usize >= cap
                {
                    outcome.truncated = true;
                    break;
                }
                let mut cells = Vec::with_capacity(kinds.len());
                for (i, kind) in kinds.iter().enumerate() {
                    cells.push(cell_value(row.get_ref(i)?, *kind));
                }
                if chunker.push(cells).is_break() {
                    outcome.truncated = true;
                    break;
                }
                if chunker.total_rows.is_multiple_of(CANCEL_CHECK_EVERY) && cancel_requested(cancel)
                {
                    let _ = chunker.flush();
                    return Err(AppError::Cancelled);
                }
            }
        }
        let _ = chunker.flush();
        outcome.row_count = chunker.total_rows;
        // DML with RETURNING applies its changes on the first step.
        if conn.total_changes() > changes_before {
            outcome.affected = Some(conn.changes());
        }
    } else {
        stmt.execute(rusqlite::params_from_iter(bound.iter()))
            .map_err(|e| with_char_offset(e.into(), sql))?;
        // `changes()` is stale after DDL; only trust it when the total moved.
        let changed = conn.total_changes() > changes_before;
        let affected = if changed { conn.changes() } else { 0 };
        if changed || is_dml(sql) {
            outcome.kind = StatementKind::Affected;
            outcome.affected = Some(affected);
        }
        let first = leading_keywords(sql, 1);
        if changed
            && matches!(
                first.first().map(String::as_str),
                Some("INSERT" | "REPLACE")
            )
        {
            outcome.last_insert_id = Some(conn.last_insert_rowid().to_string());
        }
    }
    Ok(outcome)
}

/// Runs `sql` on `actor`, streaming rows into the async-side `sink`. `tx_state` (editor only)
/// records whether a transaction is open afterwards.
///
/// The actor's interrupt handle stops whatever statement its connection runs, so it goes into
/// `cancel` only while this job's statement executes on the actor thread, not while the job
/// waits behind other jobs (a cancel then just marks the slot, and the job refuses to start).
/// Before the job hands the connection to the next one it waits for any cancel request already
/// given out, so a late `interrupt()` cannot hit the next job's statement.
pub(crate) async fn run_on_actor(
    actor: &Actor,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
    tx_state: Option<Arc<TxState>>,
) -> Result<StatementOutcome, AppError> {
    let (tx, mut rx) = mpsc::channel::<LiteMsg>(CHANNEL_DEPTH);
    let sql = sql.to_string();
    let params = params.to_vec();
    let interrupt = actor.interrupt_handle();
    let job = actor.run(move |conn| {
        let mut chan = ChannelSink { tx };
        let result = match arm_cancel(&cancel, CancelHandle::Sqlite(interrupt)) {
            Ok(armed) => {
                let result = execute_streaming(conn, &sql, &params, max_rows, &mut chan, &cancel);
                drop(armed);
                if !settle_cancels_blocking(&cancel, CANCEL_SETTLE_LIMIT) {
                    log::warn!("sqlite: a cancel request did not finish in time");
                }
                result
            }
            Err(e) => Err(e),
        };
        if let Some(state) = tx_state {
            state.set(Some(!conn.is_autocommit()));
        }
        result
    });
    tokio::pin!(job);

    let mut stopped = false;
    let mut deliver = |msg: LiteMsg, rx: &mut mpsc::Receiver<LiteMsg>| {
        if stopped {
            return;
        }
        match msg {
            LiteMsg::Columns(columns) => sink.begin_set(columns),
            LiteMsg::Rows(rows) => {
                if sink.rows(rows).is_break() {
                    // The actor sees the closed channel and stops stepping.
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
    // The job is done, so its sender is gone: whatever it sent is still buffered.
    while let Ok(msg) = rx.try_recv() {
        deliver(msg, &mut rx);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::CollectSink;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO t (name) VALUES ('a'), ('b'), ('c');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn rows_affected_and_empty() {
        let conn = conn();
        let slot = CancelSlot::default();
        let mut sink = CollectSink::default();
        let out = execute_streaming(&conn, "select * from t", &[], None, &mut sink, &slot).unwrap();
        assert_eq!(out.kind, StatementKind::Rows);
        assert_eq!(out.row_count, 3);
        assert_eq!(sink.sets[0].rows.len(), 3);

        let mut sink = CollectSink::default();
        let out =
            execute_streaming(&conn, "select * from t", &[], Some(2), &mut sink, &slot).unwrap();
        assert!(out.truncated);
        assert_eq!(out.row_count, 2);

        let mut sink = CollectSink::default();
        let out = execute_streaming(
            &conn,
            "update t set name = name where id > 1",
            &[],
            None,
            &mut sink,
            &slot,
        )
        .unwrap();
        assert_eq!(out.kind, StatementKind::Affected);
        assert_eq!(out.affected, Some(2));

        // DDL after DML: stale `changes()` must not leak into the result.
        let out = execute_streaming(&conn, "create table u (a int)", &[], None, &mut sink, &slot)
            .unwrap();
        assert_eq!(out.kind, StatementKind::Empty);
        assert_eq!(out.affected, None);

        let out = execute_streaming(
            &conn,
            "insert into t (name) values ('d')",
            &[],
            None,
            &mut sink,
            &slot,
        )
        .unwrap();
        assert_eq!(out.affected, Some(1));
        assert_eq!(out.last_insert_id.as_deref(), Some("4"));

        let out = execute_streaming(
            &conn,
            "delete from t where id = 4 returning id",
            &[],
            None,
            &mut sink,
            &slot,
        )
        .unwrap();
        assert_eq!(out.kind, StatementKind::Rows);
        assert_eq!(out.affected, Some(1));
    }

    #[test]
    fn syntax_error_offsets_are_chars() {
        let conn = conn();
        let slot = CancelSlot::default();
        let mut sink = CollectSink::default();
        let err =
            execute_streaming(&conn, "select 'é' frm t", &[], None, &mut sink, &slot).unwrap_err();
        match err {
            AppError::Db { position, .. } => {
                let pos = position.expect("position");
                // `t` is at char 15 (byte 16).
                assert_eq!(pos.offset, 15);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cancelled_slot_refuses_to_start() {
        let conn = conn();
        let slot = CancelSlot::default();
        crate::drivers::request_cancel(&slot);
        let mut sink = CollectSink::default();
        assert!(matches!(
            execute_streaming(&conn, "select 1", &[], None, &mut sink, &slot),
            Err(AppError::Cancelled)
        ));
    }
}
