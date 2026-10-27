//! The affected-rows guard of a staged commit over HTTP. Neither engine lets the client look at a
//! statement's row count before the commit (a LibSQL batch and a D1 batch run whole on the
//! server), so a guard statement after each guarded write checks `changes()` there and fails the
//! batch, which rolls it back, when the count is wrong.
//!
//! The failure is a JSON path error whose path carries the statement index and both counts: the
//! error message names them on any SQLite. The path depends on `changes()`, so SQLite cannot
//! evaluate it once ahead of the CASE (constant factoring) and fail a guard that holds.

use crate::drivers::{TxStatement, check_affected};
use crate::error::AppError;
use crate::types::query::ExecOutcome;
use crate::types::value::Value;

/// `?1` the expected count, `?2` the statement's index. `CAST` because D1 may take parameters
/// as text only.
pub const GUARD_SQL: &str = "SELECT CASE WHEN changes() = CAST(?1 AS INTEGER) THEN 1 ELSE \
     json_extract('{}', 'tabler: statement ' || ?2 || ' expected ' || ?1 || ' changed ' || changes()) END";

const MARK: &str = "tabler: statement ";

/// The parameters of the guard after statement `index`, which must change `expected` rows.
pub fn guard_params(expected: u64, index: usize) -> Vec<Value> {
    vec![Value::from_u64(expected), Value::from_u64(index as u64)]
}

/// A guard that failed: statement `index` changed `changed` rows instead of `expected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardFailure {
    pub index: usize,
    pub expected: u64,
    pub changed: u64,
}

/// The guard failure an error message reports, if it is one.
pub fn parse_guard_failure(message: &str) -> Option<GuardFailure> {
    let rest = &message[message.find(MARK)? + MARK.len()..];
    let mut words = rest.split_whitespace();
    let number = |w: Option<&str>| -> Option<u64> {
        let digits: String = w?.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let index = number(words.next())?;
    (words.next()? == "expected").then_some(())?;
    let expected = number(words.next())?;
    (words.next()? == "changed").then_some(())?;
    let changed = number(words.next())?;
    Some(GuardFailure {
        index: usize::try_from(index).ok()?,
        expected,
        changed,
    })
}

/// The error a failed guard reports: the same message and statement index as every other
/// driver's `check_affected`.
pub fn guard_error(statements: &[TxStatement], failure: GuardFailure) -> AppError {
    let outcome = ExecOutcome {
        affected: failure.changed,
        ..ExecOutcome::default()
    };
    let fallback = TxStatement::new("", Vec::new()).expecting(failure.expected);
    let st = statements.get(failure.index).unwrap_or(&fallback);
    match check_affected(st, &outcome, failure.index) {
        Err(e) => e,
        // The statement carried no expectation (or the same count): still a failed guard.
        Ok(()) => check_affected(&fallback, &outcome, failure.index)
            .err()
            .unwrap_or_else(|| AppError::db("the affected-rows check failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_guard(conn: &rusqlite::Connection, expected: u64, index: usize) -> Result<i64, String> {
        let params: Vec<i64> = guard_params(expected, index)
            .iter()
            .map(|v| match v {
                Value::Int(n) => *n,
                other => panic!("{other:?}"),
            })
            .collect();
        conn.query_row(GUARD_SQL, rusqlite::params_from_iter(params), |r| r.get(0))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_guard_passes_on_the_count_and_names_the_mismatch_otherwise() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t (v) VALUES ('a'), ('b');",
        )
        .unwrap();
        conn.execute("UPDATE t SET v = 'x' WHERE id = 1", [])
            .unwrap();
        assert_eq!(run_guard(&conn, 1, 0), Ok(1));
        let err = run_guard(&conn, 2, 3).unwrap_err();
        assert_eq!(
            parse_guard_failure(&err),
            Some(GuardFailure {
                index: 3,
                expected: 2,
                changed: 1
            }),
            "{err}"
        );
        conn.execute("DELETE FROM t WHERE id = 99", []).unwrap();
        let err = run_guard(&conn, 1, 0).unwrap_err();
        assert_eq!(
            parse_guard_failure(&err).map(|f| f.changed),
            Some(0),
            "{err}"
        );
        // Text parameters, as D1 may bind them.
        conn.execute("UPDATE t SET v = 'y'", []).unwrap();
        let ok: i64 = conn.query_row(GUARD_SQL, ["2", "0"], |r| r.get(0)).unwrap();
        assert_eq!(ok, 1);
    }

    #[test]
    fn messages_of_old_and_new_sqlite_parse() {
        let want = Some(GuardFailure {
            index: 0,
            expected: 1,
            changed: 0,
        });
        assert_eq!(
            parse_guard_failure("bad JSON path: 'tabler: statement 0 expected 1 changed 0'"),
            want
        );
        assert_eq!(
            parse_guard_failure(
                "JSON path error near 'tabler: statement 0 expected 1 changed 0': SQLITE_ERROR"
            ),
            want
        );
        assert_eq!(parse_guard_failure("no such table: t"), None);
        assert_eq!(parse_guard_failure("tabler: statement x expected"), None);
    }

    #[test]
    fn guard_errors_read_like_check_affected() {
        let stmts = vec![
            TxStatement::new("UPDATE t SET v = 1 WHERE id = 1", vec![]),
            TxStatement::new("DELETE FROM t WHERE id = 2", vec![]).expecting(1),
        ];
        let e = guard_error(
            &stmts,
            GuardFailure {
                index: 1,
                expected: 1,
                changed: 0,
            },
        );
        let payload = e.to_string();
        assert!(
            payload.starts_with("expected to affect 1 row(s) but matched 0"),
            "{payload}"
        );
        assert!(matches!(
            e,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ));
        // An index the batch does not have still reports the failure.
        let e = guard_error(
            &stmts,
            GuardFailure {
                index: 9,
                expected: 2,
                changed: 3,
            },
        );
        assert!(e.to_string().contains("expected to affect 2"), "{e}");
    }
}
