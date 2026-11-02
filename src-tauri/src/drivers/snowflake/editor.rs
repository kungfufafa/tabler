//! The editor's runs, on the session's own Snowflake session. The editor's splitter cuts the
//! text into statements (`$$` bodies and Snowflake Scripting blocks kept whole), and each is one
//! request (the login sets `MULTI_STATEMENT_COUNT = 1`, so a `;` the splitter missed makes
//! Snowflake refuse the request rather than run a second statement).
//!
//! Snowflake answers once a statement has finished, so the row cap only leaves partitions
//! undownloaded: nothing on the server needs stopping. DML answers with its counts as a row
//! (`number of rows inserted`), which the editor reports as rows affected. `USE` and `ALTER
//! SESSION` statements are kept, to set up a new session the same way after Snowflake expired
//! the old one.
//!
//! Anything not known to only read may move the session to another database (`USE`, `CREATE
//! DATABASE`, a procedure) or leave a transaction open where Tabler cannot see it: the session's
//! database and transaction are read again after it (`SnowflakeSession::probe_editor`).

use std::ops::ControlFlow;
use std::sync::Arc;

use super::conn::{Outcome, Part, SnowflakeConn};
use super::errors::StatementError;
use super::session::{DIALECT, SnowflakeSession, cancel_handle, lock, may_have_run, may_resend};
use crate::drivers::{
    CancelSlot, LATE_CANCEL_NOTICE, RowChunker, RowSink, StatementOutcome, arm_cancel,
    cancel_requested, cancel_safe_on_stop, contains_word, is_dml, tx_effect,
};
use crate::error::AppError;
use crate::sqlgen::split::leading_keywords;
use crate::sqlgen::sql::writes;
use crate::types::query::StatementKind;

/// What the editor says after it signed in again in place of an expired session.
pub const EXPIRED_NOTICE: &str = "Snowflake had ended the session: Tabler signed in again and ran \
     its USE and ALTER SESSION statements again; session variables and temporary tables are gone.";

/// What the editor says when the expired session held a transaction.
const EXPIRED_IN_TRANSACTION: &str = "Snowflake ended the session, and its open transaction with \
     it (rolled back). Tabler signed in again: run the transaction again.";

/// Whether a statement may change the session's AUTOCOMMIT: anything not known to only read that
/// names it (coarse: comments and strings count), except `ALTER USER` and `ALTER ACCOUNT`, which
/// set the default of new sessions (Tabler's sign-in sets its own). Tabler's commits and its
/// transaction tracking need every statement outside BEGIN … COMMIT committed on its own.
pub(super) fn sets_autocommit(sql: &str) -> bool {
    let kw = leading_keywords(sql, 2);
    let defaults = kw.first().is_some_and(|w| w == "ALTER")
        && matches!(kw.get(1).map(String::as_str), Some("USER" | "ACCOUNT"));
    !defaults && writes(DIALECT, sql) && contains_word(&sql.to_uppercase(), "AUTOCOMMIT")
}

pub(super) fn autocommit_error() -> AppError {
    AppError::InvalidInput(
        "Snowflake: Tabler keeps AUTOCOMMIT on (its commits and its transaction tracking rely on \
         it): open a transaction with BEGIN and end it with COMMIT or ROLLBACK instead"
            .into(),
    )
}

/// Whether a statement sets the session up in a way a new session must repeat.
fn sets_up_session(sql: &str) -> bool {
    let kw = leading_keywords(sql, 2);
    match kw.first().map(String::as_str) {
        Some("USE") => true,
        Some("ALTER") => kw.get(1).is_some_and(|w| w == "SESSION"),
        _ => false,
    }
}

/// Whether a statement opened (`Some(true)`) or ended (`Some(false)`) a transaction. `BEGIN`
/// followed by statements is a Snowflake Scripting block, not a transaction.
fn transaction_effect(sql: &str, error: Option<&AppError>) -> Option<bool> {
    let kw = leading_keywords(sql, 2);
    if kw.first().is_some_and(|w| w == "BEGIN")
        && !matches!(
            kw.get(1).map(String::as_str),
            None | Some("TRANSACTION" | "WORK" | "NAME")
        )
    {
        return None;
    }
    tx_effect(sql, DIALECT, error)
}

/// Whether a statement ends an open transaction by committing it (COMMIT, or DDL's implicit
/// commit); a ROLLBACK ends it with nothing applied.
fn commits(sql: &str) -> bool {
    transaction_effect(sql, None) == Some(false)
        && !matches!(
            leading_keywords(sql, 1).first().map(String::as_str),
            Some("ROLLBACK" | "ABORT")
        )
}

/// Runs one statement of the editor.
pub(super) async fn run(
    session: &SnowflakeSession,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    session.refuse_write(sql)?;
    if sets_autocommit(sql) {
        return Err(autocommit_error());
    }
    let may_move = writes(DIALECT, sql);
    // On a shared session, the app's reads in flight end before the session may move, and none
    // starts until it has been read back.
    let moving = session.moving(may_move, &cancel).await?;
    if may_move {
        session.forget_database();
    }
    let live = &session.editor;
    let conn = live.current().await;
    // The app signed the shared session in again under the editor's transaction: sent now, the
    // statement would autocommit on the new session (and a ROLLBACK would end nothing). Read once
    // the session is taken, not before the wait on `moving`: the flag is set under the slot's
    // lock before the swap, so a new session here always comes with it.
    if session.take_lost_transaction() {
        session.tx.set(Some(false));
        return Err(AppError::db(EXPIRED_IN_TRANSACTION));
    }
    // The session the statement ran on, read back below: the app may replace the shared one
    // meanwhile.
    let mut ran_on = Arc::clone(&conn);
    let first = statement(session, &conn, sql, max_rows, sink, &cancel).await;
    let result = match first {
        Err(e) if e.expired => {
            let in_transaction = session.tx.get() != Some(false);
            // Reported here, when the app's sign-in got there first.
            session.take_lost_transaction();
            let fresh = live.renew(&session.signin, &conn).await?;
            ran_on = Arc::clone(&fresh);
            session.forget_database();
            session.tx.set(Some(false));
            // A COMMIT (or DDL, which commits) Snowflake had accepted may have committed the
            // transaction: running it again could apply its writes twice.
            if in_transaction && e.query_id.is_some() && commits(sql) {
                return Err(may_have_run(&e));
            }
            if in_transaction {
                return Err(AppError::db(EXPIRED_IN_TRANSACTION));
            }
            if !may_resend(&e, sql) {
                return Err(may_have_run(&e));
            }
            statement(session, &fresh, sql, max_rows, sink, &cancel)
                .await
                .map(|mut o| {
                    o.notices.push(EXPIRED_NOTICE.into());
                    o
                })
                .map_err(|e| e.error)
        }
        other => other.map_err(|e| e.error),
    };
    let probed = if may_move {
        Some(session.probe_editor(&ran_on).await)
    } else {
        None
    };
    let said = transaction_effect(sql, result.as_ref().err());
    let before = session.tx.get();
    let tx = match probed {
        // What the session says beats what the text suggests: a statement the text takes for a
        // commit may have left the transaction open.
        Some(Ok(true)) => Some(true),
        Some(Ok(false)) => match said {
            Some(open) => Some(open),
            // A BEGIN Tabler saw stays open until a COMMIT or ROLLBACK it sees.
            None if before == Some(true) => Some(true),
            None => Some(false),
        },
        Some(Err(e)) => {
            log::warn!("Snowflake: reading the session's transaction failed: {e}");
            // Only a statement that succeeded lets the text decide: a COMMIT Snowflake refused
            // may have left the transaction open (unlike PostgreSQL's), so a failed statement
            // keeps an open transaction open and leaves anything else not known (read again
            // before the app's next statement).
            match &result {
                Ok(_) => said,
                Err(_) if before == Some(true) => Some(true),
                Err(_) => None,
            }
        }
        None => said.or(before),
    };
    session.tx.set(tx);
    session.editor_ran_on(&ran_on, result.is_ok()).await;
    // Set before the app's statements may run again, so none of them sees the old state.
    drop(moving);
    let mut outcome = result?;
    if sets_up_session(sql) {
        live.remember(sql);
    }
    if cancel_requested(&cancel) {
        // The statement ended before the cancel reached it: a read reports what the user asked
        // for, anything else its real outcome, since its changes stand.
        if outcome.kind == StatementKind::Rows && cancel_safe_on_stop(sql, DIALECT) {
            return Err(AppError::Cancelled);
        }
        outcome.notices.push(LATE_CANCEL_NOTICE.to_string());
    }
    Ok(outcome)
}

/// One request on `conn`, its rows streamed into `sink` up to `max_rows`; DML's counts become
/// rows affected.
async fn statement(
    session: &SnowflakeSession,
    conn: &SnowflakeConn,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, StatementError> {
    let (handle, stop) = conn.prepare(sql, &[])?;
    let _armed = arm_cancel(cancel, cancel_handle(stop.clone()))?;
    *lock(&session.editor_stop) = Some(stop);
    let dml = is_dml(sql);
    let mut chunker = RowChunker::new(sink);
    // DML's one row of counts is kept, not shown.
    let mut counts = Outcome::default();
    let mut capped = false;
    let result = SnowflakeConn::run(handle, sql, |part| match part {
        Part::Columns(columns) if dml => {
            counts.columns = columns;
            ControlFlow::Continue(())
        }
        Part::Rows(rows) if dml => {
            counts.rows.extend(rows);
            ControlFlow::Continue(())
        }
        Part::Columns(columns) => chunker.begin_set(columns),
        Part::Rows(rows) => {
            for row in rows {
                if chunker.rows_in_set >= max_rows as u64 {
                    capped = true;
                    return ControlFlow::Break(());
                }
                if chunker.push(row).is_break() {
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        }
    })
    .await;
    lock(&session.editor_stop).take();
    result?;
    let _ = chunker.flush();
    if dml && let Some(affected) = counts.affected() {
        return Ok(StatementOutcome {
            kind: StatementKind::Affected,
            affected: Some(affected),
            ..StatementOutcome::default()
        });
    }
    if dml {
        // A DML statement without its counts (a different answer than Snowflake's): its rows.
        let _ = chunker.begin_set(counts.columns);
        for row in counts.rows {
            let _ = chunker.push(row);
        }
        let _ = chunker.flush();
    }
    Ok(StatementOutcome {
        kind: if chunker.sets > 0 {
            StatementKind::Rows
        } else {
            StatementKind::Empty
        },
        result_sets: chunker.sets,
        row_count: chunker.total_rows,
        truncated: capped,
        affected: None,
        last_insert_id: None,
        notices: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripting_blocks_are_no_transactions() {
        assert_eq!(transaction_effect("BEGIN", None), Some(true));
        assert_eq!(transaction_effect("begin transaction", None), Some(true));
        assert_eq!(transaction_effect("BEGIN WORK", None), Some(true));
        assert_eq!(transaction_effect("COMMIT", None), Some(false));
        assert_eq!(transaction_effect("ROLLBACK", None), Some(false));
        // DDL commits the open transaction.
        assert_eq!(
            transaction_effect("CREATE TABLE t (a INT)", None),
            Some(false)
        );
        assert_eq!(
            transaction_effect("BEGIN LET x := 1; RETURN x; END", None),
            None
        );
        assert_eq!(
            transaction_effect("BEGIN\n  INSERT INTO t VALUES (1);\nEND", None),
            None
        );
        assert_eq!(transaction_effect("SELECT 1", None), None);
    }

    #[test]
    fn autocommit_changes_are_found() {
        for sql in [
            "ALTER SESSION SET AUTOCOMMIT = FALSE",
            "alter session set week_start = 1, autocommit = false",
            "ALTER SESSION UNSET autocommit",
            "EXECUTE IMMEDIATE 'alter session set autocommit = false'",
            "BEGIN ALTER SESSION SET AUTOCOMMIT = FALSE; END",
        ] {
            assert!(sets_autocommit(sql), "{sql}");
        }
        for sql in [
            "SHOW PARAMETERS LIKE 'AUTOCOMMIT' IN SESSION",
            "ALTER USER u SET AUTOCOMMIT = FALSE",
            "ALTER ACCOUNT SET AUTOCOMMIT = TRUE",
            "ALTER SESSION SET AUTOCOMMIT_X = 1",
            "ALTER SESSION SET TIMEZONE = 'UTC'",
        ] {
            assert!(!sets_autocommit(sql), "{sql}");
        }
    }

    #[test]
    fn use_and_alter_session_are_kept() {
        for sql in [
            "USE WAREHOUSE w",
            "use schema db.s",
            "ALTER SESSION SET TIMEZONE = 'UTC'",
            "/* c */ alter session unset query_tag",
        ] {
            assert!(sets_up_session(sql), "{sql}");
        }
        for sql in [
            "ALTER TABLE t ADD COLUMN c INT",
            "SELECT 1",
            "SET x = 1",
            "",
        ] {
            assert!(!sets_up_session(sql), "{sql}");
        }
    }
}
