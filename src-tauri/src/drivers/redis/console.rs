//! The console: one redis-cli command at a time on the console's own connection, where
//! `SELECT`, `MULTI` and `WATCH` persist between commands and runs. The reply is a result grid
//! (`reply::shape`). A cancel stops waiting at once, kills the connection from another one and
//! drops it. A connection lost that way (or by the server) is opened again by the next command,
//! on the database the console had selected (also by a SELECT inside an EXEC), which its notice
//! names; an open MULTI or WATCH is gone with the old one.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::drivers::{
    CancelHandle, CancelSlot, RowChunker, RowSink, StatementOutcome, arm_cancel, cancel_requested,
};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::types::query::StatementKind;

use super::classify::{classify_argv, refused_in_console};
use super::cli::{command_name, tokenize};
use super::conn::{Abort, KillAction, Link, send_argv};
use super::info::MAX_DATABASES;
use super::reply::shape;
use super::session::{RedisSession, lock, read_only_error};
use super::values::{as_int, is_ok};

type Resp = ::redis::Value;

/// The console's connection, opened on the database it had selected when there is none.
async fn link(s: &RedisSession, notices: &mut Vec<String>) -> Result<Arc<Link>, AppError> {
    s.ensure_open()?;
    if let Some(link) = lock(&s.console).clone().filter(|l| l.alive()) {
        return Ok(link);
    }
    // What the last connection had set up (a MULTI, a WATCH) went with it.
    let in_multi = s.tx.get() == Some(true);
    s.tx.set(Some(false));
    lock(&s.console_queue).clear();
    let db = *lock(&s.console_db);
    // No reply time out of its own: the query time out cancels a command.
    let link = Arc::new(s.dial.open(db, None).await?);
    *lock(&s.console) = Some(link.clone());
    // Every time but the first the last connection was lost (an idle time out, a kill, a
    // cancel): say which database the next commands run in.
    if s.console_opened.swap(true, Ordering::SeqCst) {
        notices.push(if in_multi {
            format!(
                "The console's connection was opened again, on db{db}: its MULTI and WATCH were \
                 discarded."
            )
        } else {
            format!("The console's connection was opened again, on db{db}.")
        });
    }
    Ok(link)
}

/// Forgets the console's connection (`link`, if it is still the one).
fn drop_link(s: &RedisSession, link: &Arc<Link>) {
    let mut console = lock(&s.console);
    if console.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
        *console = None;
    }
    link.kill();
}

/// Runs one console command.
pub async fn run(
    s: &RedisSession,
    command: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let argv = tokenize(command).map_err(|e| AppError::InvalidInput(e.to_string()))?;
    if argv.is_empty() {
        return Err(AppError::InvalidInput("no command".into()));
    }
    if s.read_only && classify_argv(&argv, s.reads.as_ref()) == StatementClass::Write {
        return Err(read_only_error());
    }
    if let Some(why) = refused_in_console(&argv) {
        return Err(AppError::Unsupported(format!("Redis console: {why}")));
    }
    let name = command_name(&argv).unwrap_or_default();
    let mut notices = Vec::new();
    let link = link(s, &mut notices).await?;
    let abort = Arc::new(Abort::default());
    // The kill goes out on the grid connection of the profile's database.
    let kill = match link.id {
        Some(id) => s
            .reader(s.default_db)
            .await
            .ok()
            .map(|control| (control, id)),
        None => None,
    };
    let armed = arm_cancel(
        &cancel,
        CancelHandle::Custom(Arc::new(KillAction {
            abort: abort.clone(),
            kill,
        })),
    )?;
    let reply = abort.run(send_argv(&mut link.con(), &argv)).await;
    drop(armed);
    // A cancel's kill may be on its way to this connection (also when the reply made it first),
    // or the connection is lost: the next command opens a new one.
    let dropped = cancel_requested(&cancel)
        || matches!(
            reply,
            Err(AppError::Cancelled | AppError::NotConnected | AppError::Timeout)
        );
    if dropped {
        drop_link(s, &link);
        s.tx.set(Some(false));
    }
    let reply = reply?;
    if dropped {
        notices.push(
            "The command completed before it could be cancelled; the console's connection was \
             closed and the next command opens a new one."
                .into(),
        );
    }
    track_state(s, &name, &argv, &reply, dropped);
    let shaped = shape(&argv, &reply, max_rows)?;
    notices.extend(shaped.notices);
    let mut rows = RowChunker::new(sink);
    let mut flow = rows.begin_set(shaped.columns);
    for row in shaped.rows {
        if flow.is_break() {
            break;
        }
        flow = rows.push(row);
    }
    if flow == ControlFlow::Continue(()) {
        let _ = rows.flush();
    }
    Ok(StatementOutcome {
        kind: StatementKind::Rows,
        result_sets: 1,
        row_count: rows.total_rows,
        truncated: shaped.truncated,
        affected: None,
        last_insert_id: None,
        notices,
    })
}

/// What a command changed of the console's connection: its database (kept for the next
/// connection too, also when this one is `dropped`), its transaction (gone with a dropped one).
fn track_state(s: &RedisSession, name: &str, argv: &[Vec<u8>], reply: &Resp, dropped: bool) {
    let queued = matches!(reply, Resp::SimpleString(t) if t == "QUEUED");
    if queued && s.tx.get() == Some(true) {
        // Inside MULTI a SELECT only takes effect when EXEC runs it (DISCARD drops it).
        let db = (name == "SELECT").then(|| selected(argv)).flatten();
        lock(&s.console_queue).push(db);
        return;
    }
    match name {
        "SELECT" if is_ok(reply) => {
            if let Some(db) = selected(argv) {
                *lock(&s.console_db) = db;
            }
        }
        // EXEC answers for each queued command in order: the database is the last SELECT it
        // ran. EXEC and DISCARD end the MULTI, also when they fail (EXECABORT).
        "EXEC" => {
            let queue = std::mem::take(&mut *lock(&s.console_queue));
            if let Resp::Array(results) = reply {
                for (db, r) in queue.iter().zip(results) {
                    if let (Some(db), true) = (db, is_ok(r)) {
                        *lock(&s.console_db) = *db;
                    }
                }
            }
            s.tx.set(Some(false));
        }
        "DISCARD" => {
            lock(&s.console_queue).clear();
            s.tx.set(Some(false));
        }
        _ if dropped => {}
        "MULTI" if !matches!(reply, Resp::ServerError(_)) => {
            lock(&s.console_queue).clear();
            s.tx.set(Some(true));
        }
        _ => {}
    }
}

/// The database a `SELECT` names, when it is one.
fn selected(argv: &[Vec<u8>]) -> Option<u32> {
    argv.get(1)
        .and_then(|a| as_int(&Resp::BulkString(a.clone())))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n < MAX_DATABASES)
}
