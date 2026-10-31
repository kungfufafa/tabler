//! What the LibSQL and D1 sessions share: the read-only refusal, a request stopped through the
//! run's cancel slot (neither server has a cancel of its own, so the request is dropped, which
//! closes its connection), the console log of what they send, and the end of a closed session.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::catalog::{CatalogQuery, Rows};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, SqlTrace, TraceCounts, TxStatement, arm_cancel,
    classify_statement, trace_sql, trace_text,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::browse::TraceCategory;
use crate::types::query::ExecOutcome;

/// Refuses on a read-only session what the classifier does not know is a read, and text where
/// SQLite and the classifier could read different statements (`hides_code_in_a_variable`). Both
/// sessions call it before sending a statement: neither D1 nor `sqld` refuses a write for a
/// session, and `PRAGMA query_only` (where a LibSQL server takes it) stops no PRAGMA, which SQLite
/// applies while preparing it, under `EXPLAIN QUERY PLAN` too.
pub fn refuse_write(read_only: bool, sql: &str) -> Result<(), AppError> {
    if read_only
        && (crate::sqlgen::sql::writes(Dialect::Sqlite, sql) || hides_code_in_a_variable(sql))
    {
        Err(AppError::InvalidInput(READ_ONLY_MESSAGE.into()))
    } else {
        Ok(())
    }
}

/// Whether `sql` holds what SQLite reads as one variable with a TCL-style suffix (`$a(…)`,
/// `:a::b(…)`, up to a space or `)`) whose suffix opens a comment, a string or a quoted name for
/// the shared classifier, which then misses the code after it (`SELECT $a(--) ; DELETE FROM t`).
/// Read over the raw text, strings and comments included, so it errs towards refusing; the
/// servers would refuse the unbound variable or the second statement anyway, but the read-only
/// guarantee is the session's.
pub fn hides_code_in_a_variable(sql: &str) -> bool {
    let b = sql.as_bytes();
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    let mut start = 0;
    while start < b.len() {
        if !matches!(at(start), b'$' | b'@' | b':' | b'#') {
            start += 1;
            continue;
        }
        let (mut i, mut named) = (start + 1, false);
        loop {
            if ident(at(i)) {
                named = true;
                i += 1;
            } else if at(i) == b':' && at(i + 1) == b':' {
                i += 2;
            } else {
                break;
            }
        }
        if named && at(i) == b'(' {
            let end = (i + 1..b.len())
                .find(|&j| matches!(at(j), b')' | b' ' | b'\t'..=b'\r'))
                .unwrap_or(b.len());
            let suffix = &b[i + 1..end];
            if suffix.windows(2).any(|w| w == b"--" || w == b"/*")
                || suffix
                    .iter()
                    .any(|c| matches!(c, b'\'' | b'"' | b'[' | b'`'))
            {
                return true;
            }
            // A variable inside the suffix ends where it does, with less in its suffix.
            i = end;
        }
        // Every start inside the name ends where this one did: one pass over the text.
        start = i.max(start + 1);
    }
    false
}

/// Drops the request a statement waits for.
struct DropRequest(CancellationToken);

impl CancelAction for DropRequest {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.cancel();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "HTTP request drop"
    }
}

/// Runs `work` with `slot` armed: a cancel drops it, and the request it waits for, and it fails
/// with `Cancelled`. A run cancelled before it starts sends nothing. The server finishes a
/// statement it already has.
pub async fn cancellable<T>(
    slot: &CancelSlot,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    let token = CancellationToken::new();
    let _armed = arm_cancel(
        slot,
        CancelHandle::Custom(Arc::new(DropRequest(token.clone()))),
    )?;
    tokio::select! {
        biased;
        () = token.cancelled() => Err(AppError::Cancelled),
        result = work => result,
    }
}

/// `work` under `slot` when the caller can cancel it (see [`cancellable`]).
pub async fn cancellable_by<T>(
    slot: Option<&CancelSlot>,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    match slot {
        Some(slot) => cancellable(slot, work).await,
        None => work.await,
    }
}

/// A catalog round trip as one entry of the console log: its statements, one per line.
pub async fn traced_catalog(
    trace: &SqlTrace,
    queries: &[CatalogQuery],
    run: impl Future<Output = Result<Vec<Rows>, AppError>>,
) -> Result<Vec<Rows>, AppError> {
    let text = queries
        .iter()
        .map(|q| trace_text(Dialect::Sqlite, q.sql, &q.params))
        .collect::<Vec<_>>()
        .join(";\n");
    trace
        .run_async(
            TraceCategory::Structure,
            text,
            |sets: &Vec<Rows>| TraceCounts::rows(sets.iter().map(Vec::len).sum()),
            run,
        )
        .await
}

/// One statement the app runs, recorded in the console log.
pub async fn traced<T>(
    trace: &SqlTrace,
    sql: &str,
    params: &[crate::types::value::Value],
    counts: impl FnOnce(&T) -> TraceCounts,
    run: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    trace
        .run_async(
            classify_statement(sql, Dialect::Sqlite),
            trace_sql(Dialect::Sqlite, sql, params),
            counts,
            run,
        )
        .await
}

/// A staged commit, which the server ran as one batch, in the console log: each statement up to
/// the one that failed (with its error); a failure the server did not place is one entry for the
/// whole batch.
pub fn trace_transaction(
    trace: &SqlTrace,
    statements: &[TxStatement],
    started: Instant,
    result: &Result<Vec<ExecOutcome>, AppError>,
) {
    let entry = |st: &TxStatement, one: &Result<ExecOutcome, AppError>| {
        trace.finish(
            classify_statement(&st.sql, Dialect::Sqlite),
            trace_sql(Dialect::Sqlite, &st.sql, &st.params),
            started,
            one,
            TraceCounts::of_exec,
        );
    };
    match result {
        Ok(outcomes) => {
            for (st, outcome) in statements.iter().zip(outcomes) {
                entry(st, &Ok(outcome.clone()));
            }
        }
        Err(e) => match e {
            AppError::Db {
                statement_index: Some(failed),
                ..
            } if *failed < statements.len() => {
                for (i, st) in statements.iter().enumerate().take(failed + 1) {
                    let one = if i == *failed {
                        Err(e.clone())
                    } else {
                        Ok(ExecOutcome::default())
                    };
                    entry(st, &one);
                }
            }
            _ => {
                let text = statements
                    .iter()
                    .map(|st| trace_sql(Dialect::Sqlite, &st.sql, &st.params).text)
                    .collect::<Vec<_>>()
                    .join(";\n");
                trace.finish(TraceCategory::Data, text, started, result, |_| {
                    TraceCounts::NONE
                });
            }
        },
    }
}

/// Whether the session was closed: later calls fail as on a lost connection instead of opening
/// new streams or requests.
#[derive(Debug, Default)]
pub struct Closed(AtomicBool);

impl Closed {
    pub fn close(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn check(&self) -> Result<(), AppError> {
        if self.0.load(Ordering::SeqCst) {
            Err(AppError::NotConnected)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::{cancel_portable, request_cancel};
    use std::time::Duration;

    #[tokio::test]
    async fn a_cancel_drops_the_work_and_a_cancelled_slot_starts_nothing() {
        let slot = CancelSlot::default();
        let run = cancellable(&slot, async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<_, AppError>(1)
        });
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let handle = request_cancel(&slot).expect("armed while it runs");
            cancel_portable(handle).await.expect("a custom handle")
        };
        let (result, fired) = tokio::join!(run, cancel);
        assert!(matches!(result, Err(AppError::Cancelled)));
        fired.unwrap();
        assert!(!slot.is_armed());
        let mut started = false;
        let again = cancellable(&slot, async {
            started = true;
            Ok::<_, AppError>(2)
        })
        .await;
        assert!(matches!(again, Err(AppError::Cancelled)));
        assert!(!started);
        // Without a cancel the work's own result comes back.
        assert_eq!(
            cancellable(&CancelSlot::default(), async { Ok::<_, AppError>(3) })
                .await
                .unwrap(),
            3
        );
    }

    #[test]
    fn staged_commits_are_logged_up_to_the_failure() {
        let trace = SqlTrace::default();
        let stmts = [
            TxStatement::new(
                "UPDATE t SET a = ?1",
                vec![crate::types::value::Value::Int(1)],
            ),
            TxStatement::new("DELETE FROM t", vec![]),
            TxStatement::new("INSERT INTO t VALUES (1)", vec![]),
        ];
        let started = Instant::now();
        let outcomes = vec![ExecOutcome::default(); 3];
        trace_transaction(&trace, &stmts, started, &Ok(outcomes));
        let logged = trace.drain();
        assert_eq!(logged.len(), 3);
        assert_eq!(logged[0].sql, "UPDATE t SET a = 1");
        let failed = AppError::db("constraint").with_statement_index(1);
        trace_transaction(&trace, &stmts, started, &Err(failed));
        let logged = trace.drain();
        assert_eq!(logged.len(), 2);
        assert_eq!(logged[0].error, None);
        assert_eq!(logged[1].error.as_deref(), Some("constraint"));
        trace_transaction(&trace, &stmts, started, &Err(AppError::db("rolled back")));
        let logged = trace.drain();
        assert_eq!(logged.len(), 1);
        assert!(
            logged[0].sql.contains("DELETE FROM t;\nINSERT"),
            "{}",
            logged[0].sql
        );
        assert_eq!(logged[0].error.as_deref(), Some("rolled back"));
    }

    #[test]
    fn a_closed_session_refuses_calls() {
        let closed = Closed::default();
        closed.check().unwrap();
        closed.close();
        assert!(matches!(closed.check(), Err(AppError::NotConnected)));
    }
}
