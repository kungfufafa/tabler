//! Applying planned edits on the edit connection: per change, `WATCH` its guarded key and send
//! the guard's checks (the first that answers otherwise is the change's error, and nothing of it
//! is sent), then `MULTI` and, once it is answered OK, its commands and `EXEC` (a nil EXEC means
//! someone changed the key in between). A refused WATCH or MULTI (an ACL without
//! `@transaction`, a proxy) refuses the change with nothing of it sent: without them the commands
//! would run at once, unguarded. Redis has no rollback: each change applies or not on its own,
//! with a result each, so the renderer unstages the applied ones and a retry never repeats them.

use std::sync::Arc;
use std::time::Instant;

use crate::error::AppError;
use crate::sqlgen::native::redis::{self as cmd, Command, Expect, Guard, KeyEdit};
use crate::types::browse::TraceCategory;
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult};

use super::conn::{Link, REPLY_TIMEOUT, trace};
use super::session::{RedisSession, lock, read_only_error};
use super::values::{as_bytes, error_text, is_ok};

type Resp = ::redis::Value;

/// A plan of `plan_changes`: the database and the edit of each change.
pub struct Plan {
    pub db: u32,
    pub edits: Vec<KeyEdit>,
}

/// Whether `reply` is what `c` expects (`Err`: the change's error).
pub fn check(c: &Command, reply: &Resp) -> Result<(), String> {
    match (&c.expect, reply) {
        (_, Resp::ServerError(e)) => Err(error_text(e)),
        (Expect::Any, _) => Ok(()),
        (Expect::Ok(_), Resp::Okay) => Ok(()),
        (Expect::Ok(_), Resp::SimpleString(s)) if s == "OK" => Ok(()),
        (Expect::Int(n, _), Resp::Int(m)) if n == m => Ok(()),
        (Expect::Positive(_), Resp::Int(m)) if *m > 0 => Ok(()),
        (Expect::Bytes(want, _), r) if as_bytes(r).as_ref() == Some(want) => Ok(()),
        (
            Expect::Ok(msg) | Expect::Int(_, msg) | Expect::Positive(msg) | Expect::Bytes(_, msg),
            _,
        ) => Err(msg.to_string()),
    }
}

/// What became of one edit.
#[derive(Debug, Clone)]
pub enum Outcome {
    Applied,
    /// Refused by its guard or by the server: nothing (or, inside EXEC, not all) of it applied.
    Refused(String),
    /// The connection failed: whether it applied is unknown.
    Lost(AppError),
    /// Nothing of it was sent: there was no edit connection on its database.
    NotSent(AppError),
}

/// The edit connection on `db`, opened (or SELECTed) for the caller, who holds `writer_lock`.
async fn writer(s: &RedisSession, db: u32) -> Result<Arc<Link>, AppError> {
    s.ensure_open()?;
    let current = lock(&s.writer).clone();
    let link = match current {
        Some((link, at)) if link.alive() => {
            if at == db {
                return Ok(link);
            }
            link
        }
        _ => {
            let link = Arc::new(s.dial.open(db, Some(REPLY_TIMEOUT)).await?);
            *lock(&s.writer) = Some((link.clone(), db));
            return Ok(link);
        }
    };
    let select = cmd::select_db(db);
    let started = Instant::now();
    let out = link.send(&select).await;
    trace(
        &s.trace,
        TraceCategory::Data,
        std::slice::from_ref(&select),
        started,
        &out,
        |_| None,
    );
    match out {
        // Refused: the connection stays on the database it had.
        Ok(Resp::ServerError(e)) => Err(super::errors::server_error(&e)),
        Ok(reply) if check(&select, &reply).is_ok() => {
            *lock(&s.writer) = Some((link.clone(), db));
            Ok(link)
        }
        // No answer in time (the server may still run the SELECT) or an odd one: which database
        // the connection is on is unknown, so it is never used again.
        out => {
            forget(s, &link);
            Err(match out {
                Err(e) => e,
                Ok(reply) => {
                    AppError::Internal(format!("Redis: unexpected SELECT reply {reply:?}"))
                }
            })
        }
    }
}

/// Drops the edit connection `link`: its database or transaction state is unknown, so the next
/// apply opens a new one.
fn forget(s: &RedisSession, link: &Arc<Link>) {
    let mut writer = lock(&s.writer);
    if writer.as_ref().is_some_and(|(l, _)| Arc::ptr_eq(l, link)) {
        *writer = None;
    }
    link.kill();
}

/// A change refused because the server refused `what` (`WATCH`, `MULTI`) before any of its
/// commands was sent.
fn refused_by(what: &str, reply: Option<&Resp>) -> String {
    match reply {
        Some(Resp::ServerError(e)) => format!(
            "the server refused {what}, which guards the change, so none of it was sent: {}",
            error_text(e)
        ),
        other => format!("unexpected {what} reply {other:?}: none of the change was sent"),
    }
}

/// Applies `edits` in order on database `db`, an outcome each. A lost connection stops the
/// rest, which are reported as not applied.
pub async fn apply_edits(s: &RedisSession, db: u32, edits: &[KeyEdit]) -> Vec<Outcome> {
    if s.read_only {
        let e = read_only_error().to_string();
        return edits.iter().map(|_| Outcome::Refused(e.clone())).collect();
    }
    let _one = s.writer_lock.lock().await;
    let mut out = Vec::with_capacity(edits.len());
    for edit in edits {
        // Per change: one whose transaction state turned out unknown dropped the connection.
        let link = match writer(s, db).await {
            Ok(link) => link,
            Err(e) => {
                while out.len() < edits.len() {
                    out.push(Outcome::NotSent(e.clone()));
                }
                break;
            }
        };
        match apply_one(s, &link, edit).await {
            Ok(Ok(())) => out.push(Outcome::Applied),
            Ok(Err(msg)) => out.push(Outcome::Refused(msg)),
            Err(e) => {
                // Its WATCH and MULTI state is unknown: the next apply opens a new connection.
                forget(s, &link);
                out.push(Outcome::Lost(e));
                while out.len() < edits.len() {
                    out.push(Outcome::Refused(
                        "not applied: an earlier change lost the connection".into(),
                    ));
                }
                break;
            }
        }
    }
    out
}

/// One edit: `Ok(Err(msg))` when its guard or the server refused it, or nothing of it was sent;
/// `Err` when its commands were sent and the connection failed (whether it applied is unknown).
async fn apply_one(
    s: &RedisSession,
    link: &Arc<Link>,
    edit: &KeyEdit,
) -> Result<Result<(), String>, AppError> {
    let con = link.con();
    let send_traced = |cmds: Vec<Command>| {
        let mut con = con.clone();
        async move {
            let started = Instant::now();
            let out = super::conn::pipeline(&mut con, &cmds).await;
            trace(&s.trace, TraceCategory::Data, &cmds, started, &out, |_| {
                None
            });
            out
        }
    };
    // Before its commands are sent, a failed round trip leaves the change unapplied; the
    // connection's state is unknown, so the next change opens a new one.
    let not_sent = |e: AppError| -> Result<Result<(), String>, AppError> {
        forget(s, link);
        Ok(Err(format!("not applied: {e}")))
    };
    // Ends a guarded change that is not sent: its key stays watched otherwise, and a later EXEC
    // on this connection would answer for it.
    let unwatch = || async {
        let done = send_traced(vec![cmd::unwatch()]).await;
        if !done.is_ok_and(|r| r.first().is_some_and(is_ok)) {
            forget(s, link);
        }
    };
    if let Some(key) = edit.guard.key() {
        let checks = edit.guard.checks();
        let mut first = vec![cmd::watch(&[key])];
        first.extend(checks.iter().cloned());
        let replies = match send_traced(first).await {
            Ok(replies) => replies,
            Err(e) => return not_sent(e),
        };
        if !replies.first().is_some_and(is_ok) {
            // Unguarded, the change could land on a key someone else changed in between (the
            // checks only read).
            forget(s, link);
            return Ok(Err(refused_by("WATCH", replies.first())));
        }
        let refused = checks
            .iter()
            .zip(replies.iter().skip(1))
            .find_map(|(c, r)| check(c, r).err());
        if let Some(e) = refused {
            unwatch().await;
            return Ok(Err(e));
        }
    }
    let wrap = edit.commands.len() > 1 || edit.guard != Guard::None;
    if !wrap {
        // One command that refuses on its own (`SET … NX`): as the preview shows it.
        let replies = send_traced(edit.commands.clone()).await?;
        return Ok(edit
            .commands
            .iter()
            .zip(&replies)
            .find_map(|(c, r)| check(c, r).err())
            .map_or(Ok(()), Err));
    }
    // MULTI alone first: pipelined with the commands, a refused MULTI would let them run at once,
    // outside the transaction and its WATCH.
    let begun = match send_traced(vec![cmd::multi()]).await {
        Ok(replies) => replies,
        Err(e) => return not_sent(e),
    };
    if !begun.first().is_some_and(is_ok) {
        if edit.guard.key().is_some() {
            unwatch().await;
        }
        return Ok(Err(refused_by("MULTI", begun.first())));
    }
    let mut cmds = edit.commands.clone();
    cmds.push(cmd::exec());
    let replies = send_traced(cmds).await?;
    Ok(match replies.last() {
        Some(Resp::Array(results)) => edit
            .commands
            .iter()
            .zip(results)
            .find_map(|(c, r)| check(c, r).err())
            .map_or(Ok(()), Err),
        // A watched key changed between WATCH and EXEC.
        Some(Resp::Nil) => Err(cmd::CHANGED.to_string()),
        // Nothing ran: the commands were only queued. EXECABORT (a command refused while queued)
        // ends the transaction; a refused EXEC (an ACL) leaves it open, so the connection is not
        // used again.
        Some(Resp::ServerError(e)) => {
            if e.code() != "EXECABORT" {
                forget(s, link);
            }
            Err(error_text(e))
        }
        other => {
            forget(s, link);
            Err(format!("unexpected EXEC reply: {other:?}"))
        }
    })
}

/// `apply_changes` of the grid: the plan's edits applied, a `RowResult` per change.
pub async fn apply_changes(
    s: &RedisSession,
    req: &ApplyRequest,
    plan: crate::drivers::native::NativePlan,
) -> ApplyResult {
    let started = Instant::now();
    let sql: Vec<String> = plan.preview.iter().map(|(_, t)| t.clone()).collect();
    let failed = |e: AppError| ApplyResult {
        success: false,
        results: Vec::new(),
        sql: sql.clone(),
        error: Some(e.to_payload()),
        duration_ms: started.elapsed().as_millis() as u64,
    };
    if s.read_only {
        return failed(read_only_error());
    }
    let Ok(plan) = plan.ops.downcast::<Plan>() else {
        return failed(AppError::Internal(
            "Redis: the plan is not one of this session's".into(),
        ));
    };
    let outcomes = apply_edits(s, plan.db, &plan.edits).await;
    let results: Vec<RowResult> = req
        .changes
        .iter()
        .enumerate()
        .map(|(i, change)| {
            let outcome = plan
                .edits
                .iter()
                .position(|e| e.change == i)
                .and_then(|at| outcomes.get(at));
            let error = match outcome {
                Some(Outcome::Applied) => None,
                Some(Outcome::Refused(msg)) => Some(AppError::db(msg.clone())),
                Some(Outcome::Lost(e)) => Some(lost(e)),
                Some(Outcome::NotSent(e)) => Some(AppError::db(format!("not applied: {e}"))),
                None => Some(AppError::Internal("the change was not planned".into())),
            };
            RowResult {
                row_id: change.row_id().to_string(),
                op: change.op_name().to_string(),
                affected: u64::from(error.is_none()),
                returned: None,
                error: error.map(|e| e.to_payload()),
            }
        })
        .collect();
    let error = results.iter().find_map(|r| r.error.clone());
    ApplyResult {
        success: error.is_none(),
        results,
        sql,
        error,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// A change whose connection failed: it may have applied, so it must not be retried blindly.
fn lost(e: &AppError) -> AppError {
    AppError::db(format!(
        "{e}: the change may or may not have been applied; reload before trying again"
    ))
}
