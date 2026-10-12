//! EXPLAIN wrapping per dialect. Every form produces a textual plan the editor can show as-is.

use super::Dialect;
use crate::error::AppError;
use crate::types::query::ExplainMode;

/// Savepoint that keeps an Explain Analyze inside the user's own open transaction.
pub const EXPLAIN_SAVEPOINT: &str = "tabler_explain";

/// Statements an executing Explain Analyze runs in, so that whatever it does is undone:
/// `(open, close)` around the EXPLAIN, on the same editor connection. `Ok(None)`: run it as is.
///
/// - SQLite: `EXPLAIN QUERY PLAN` never executes the statement.
/// - No open transaction (`in_tx == Some(false)`): a transaction of its own, rolled back.
/// - Inside the user's transaction (`Some(true)`): a savepoint, rolled back to and released, so
///   the user's transaction stays open with its earlier work. Never `START TRANSACTION` there:
///   MySQL would implicitly commit the user's transaction first.
/// - Unknown state (`None`): a statement that `writes` is refused (neither form is safe without
///   knowing whether a transaction is open); anything else runs as is.
///
/// Callers wrap every PostgreSQL / MySQL Analyze, whether `writes` or not: a SELECT can call a
/// function that writes. MySQL tables without transactions (MyISAM…) keep their changes anyway.
pub fn analyze_wrapper(
    dialect: Dialect,
    in_tx: Option<bool>,
    writes: bool,
) -> Result<Option<(String, Vec<String>)>, AppError> {
    if dialect == Dialect::Sqlite {
        return Ok(None);
    }
    match in_tx {
        Some(false) => {
            let open = match dialect {
                Dialect::Mysql => "START TRANSACTION",
                _ => "BEGIN",
            };
            Ok(Some((open.to_string(), vec!["ROLLBACK".to_string()])))
        }
        Some(true) => Ok(Some((
            format!("SAVEPOINT {EXPLAIN_SAVEPOINT}"),
            vec![
                format!("ROLLBACK TO SAVEPOINT {EXPLAIN_SAVEPOINT}"),
                format!("RELEASE SAVEPOINT {EXPLAIN_SAVEPOINT}"),
            ],
        ))),
        None if writes => Err(AppError::Unsupported(
            "Explain Analyze would really run this statement and its transaction state is \
             unknown; run it in an explicit transaction instead"
                .into(),
        )),
        None => Ok(None),
    }
}

/// Wraps one statement in the dialect's EXPLAIN. A trailing `;` (and whitespace) is dropped.
/// `Analyze` executes the statement on PostgreSQL / MySQL / MariaDB.
pub fn wrap_explain(dialect: Dialect, sql: &str, mode: ExplainMode, mariadb: bool) -> String {
    format!(
        "{} {}",
        explain_prefix(dialect, mode, mariadb),
        strip_terminator(sql)
    )
}

/// The text `wrap_explain` puts in front of the statement (without the separating space).
pub fn explain_prefix(dialect: Dialect, mode: ExplainMode, mariadb: bool) -> &'static str {
    match (dialect, mode) {
        (Dialect::Postgres, ExplainMode::Plan) => "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT)",
        (Dialect::Postgres, ExplainMode::Analyze) => {
            "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT)"
        }
        (Dialect::Mysql, ExplainMode::Plan) if mariadb => "EXPLAIN FORMAT=JSON",
        (Dialect::Mysql, ExplainMode::Analyze) if mariadb => "ANALYZE FORMAT=JSON",
        (Dialect::Mysql, ExplainMode::Plan) => "EXPLAIN FORMAT=TREE",
        (Dialect::Mysql, ExplainMode::Analyze) => "EXPLAIN ANALYZE",
        // SQLite has no executing EXPLAIN that reports a plan; both modes show the query plan.
        (Dialect::Sqlite, _) => "EXPLAIN QUERY PLAN",
    }
}

/// Number of characters `wrap_explain` adds before the (unchanged) statement text, so error
/// positions reported for the wrapped SQL can be mapped back onto the statement.
pub fn explain_offset(dialect: Dialect, mode: ExplainMode, mariadb: bool) -> usize {
    explain_prefix(dialect, mode, mariadb).chars().count() + 1
}

fn strip_terminator(sql: &str) -> &str {
    let mut s = sql.trim_end();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_per_dialect() {
        let sql = "select * from users where id = 1;";
        assert_eq!(
            wrap_explain(Dialect::Postgres, sql, ExplainMode::Plan, false),
            "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Postgres, sql, ExplainMode::Analyze, false),
            "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Plan, false),
            "EXPLAIN FORMAT=TREE select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Analyze, false),
            "EXPLAIN ANALYZE select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Plan, true),
            "EXPLAIN FORMAT=JSON select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Mysql, sql, ExplainMode::Analyze, true),
            "ANALYZE FORMAT=JSON select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Sqlite, sql, ExplainMode::Plan, false),
            "EXPLAIN QUERY PLAN select * from users where id = 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Sqlite, sql, ExplainMode::Analyze, false),
            "EXPLAIN QUERY PLAN select * from users where id = 1"
        );
    }

    #[test]
    fn strips_terminators_and_whitespace() {
        assert_eq!(
            wrap_explain(Dialect::Sqlite, "select 1 ;  ;\n", ExplainMode::Plan, false),
            "EXPLAIN QUERY PLAN select 1"
        );
        assert_eq!(
            wrap_explain(Dialect::Postgres, "select ';'", ExplainMode::Plan, false),
            "EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) select ';'"
        );
    }

    #[test]
    fn analyze_runs_in_a_rolled_back_transaction() {
        let pair = |open: &str, close: &[&str]| {
            Some((
                open.to_string(),
                close.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
        };
        // SQLite never executes the statement: nothing to wrap, whatever the state.
        for in_tx in [None, Some(false), Some(true)] {
            for writes in [false, true] {
                assert_eq!(
                    analyze_wrapper(Dialect::Sqlite, in_tx, writes).unwrap(),
                    None
                );
            }
        }
        // No transaction open: one of its own, rolled back (also for reads: functions write).
        for writes in [false, true] {
            assert_eq!(
                analyze_wrapper(Dialect::Postgres, Some(false), writes).unwrap(),
                pair("BEGIN", &["ROLLBACK"])
            );
            assert_eq!(
                analyze_wrapper(Dialect::Mysql, Some(false), writes).unwrap(),
                pair("START TRANSACTION", &["ROLLBACK"])
            );
        }
        // Inside the user's transaction: a savepoint, never a new transaction.
        for d in [Dialect::Postgres, Dialect::Mysql] {
            for writes in [false, true] {
                let (open, close) = analyze_wrapper(d, Some(true), writes).unwrap().unwrap();
                assert_eq!(open, "SAVEPOINT tabler_explain");
                assert_eq!(
                    close,
                    [
                        "ROLLBACK TO SAVEPOINT tabler_explain",
                        "RELEASE SAVEPOINT tabler_explain"
                    ]
                );
                assert!(!open.contains("TRANSACTION") && !open.starts_with("BEGIN"));
            }
        }
        // Unknown state: a write is refused, a read runs as is.
        for d in [Dialect::Postgres, Dialect::Mysql] {
            match analyze_wrapper(d, None, true) {
                Err(AppError::Unsupported(msg)) => {
                    assert!(msg.contains("explicit transaction"), "{msg}")
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(analyze_wrapper(d, None, false).unwrap(), None);
        }
    }

    #[test]
    fn offset_points_at_the_statement() {
        for (d, m, maria) in [
            (Dialect::Postgres, ExplainMode::Plan, false),
            (Dialect::Postgres, ExplainMode::Analyze, false),
            (Dialect::Mysql, ExplainMode::Plan, false),
            (Dialect::Mysql, ExplainMode::Analyze, true),
            (Dialect::Sqlite, ExplainMode::Plan, false),
        ] {
            let wrapped = wrap_explain(d, "select x", m, maria);
            let off = explain_offset(d, m, maria);
            assert_eq!(wrapped.chars().skip(off).collect::<String>(), "select x");
        }
    }
}
