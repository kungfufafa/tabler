//! A staged commit on D1: one `/raw` batch, which D1 runs as one transaction (it refuses
//! `BEGIN`), with the `changes()` guard after each statement that expects a row count. D1
//! reports `meta.changes` only once the batch has committed, so the guard is what keeps a
//! commit from going through with the wrong rows; and since an error in a batch does not say
//! which statement failed, the guard's message names it.
//!
//! `meta.changes` is the statement's `sqlite3_total_changes()` delta, so it also counts the rows
//! triggers and foreign-key cascades changed (D1 always enforces foreign keys): it is never
//! compared with the expected count after the commit.

use crate::drivers::TxStatement;
use crate::drivers::sqlite_http::guard::{
    GUARD_SQL, guard_error, guard_params, parse_guard_failure,
};
use crate::error::AppError;
use crate::types::query::ExecOutcome;

use super::api::{RawQuery, RawResult, one_statement};
use super::decode::{ParamEncoding, encode_params, exec_outcome};

/// What each statement of the batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRole {
    Statement(usize),
    Guard(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TxBatch {
    pub queries: Vec<RawQuery>,
    pub roles: Vec<StepRole>,
}

pub fn tx_batch(statements: &[TxStatement], encoding: ParamEncoding) -> Result<TxBatch, AppError> {
    let mut queries = Vec::new();
    let mut roles = Vec::new();
    for (i, st) in statements.iter().enumerate() {
        one_statement(&st.sql).map_err(|e| e.with_statement_index(i))?;
        queries.push(RawQuery {
            sql: st.sql.clone(),
            params: encode_params(&st.sql, &st.params, encoding)
                .map_err(|e| e.with_statement_index(i))?,
        });
        roles.push(StepRole::Statement(i));
        if let Some(n) = st.expect_affected {
            queries.push(RawQuery {
                sql: GUARD_SQL.to_string(),
                params: encode_params(GUARD_SQL, &guard_params(n, i), encoding)?,
            });
            roles.push(StepRole::Guard(i));
        }
    }
    Ok(TxBatch { queries, roles })
}

/// Each statement's outcome from the results of a batch that committed. Never an error: the
/// changes are applied, and an error would leave them staged for a retry that runs the batch's
/// inserts again. A guarded statement changed exactly its expected count (the guard proved
/// `changes()` before the commit); the others get `meta.changes` as a rough count. A reply whose
/// results do not line up with the batch gives the others no count rather than wrong ones.
pub fn tx_outcome(
    statements: &[TxStatement],
    batch: &TxBatch,
    results: &[RawResult],
) -> Vec<ExecOutcome> {
    let mut outcomes: Vec<ExecOutcome> = statements
        .iter()
        .map(|st| ExecOutcome {
            affected: st.expect_affected.unwrap_or(0),
            ..ExecOutcome::default()
        })
        .collect();
    if results.len() != batch.roles.len() {
        return outcomes;
    }
    for (role, result) in batch.roles.iter().zip(results) {
        let StepRole::Statement(i) = role else {
            continue;
        };
        let (Some(st), Some(slot)) = (statements.get(*i), outcomes.get_mut(*i)) else {
            continue;
        };
        let mut outcome = exec_outcome(&st.sql, result);
        if let Some(n) = st.expect_affected {
            outcome.affected = n;
            if n == 0 {
                outcome.last_insert_id = None;
            }
        }
        *slot = outcome;
    }
    outcomes
}

/// The error of a batch that failed (and was rolled back): a failed guard reads like
/// `check_affected`; anything else names no statement, because D1 does not say which failed.
pub fn tx_error(statements: &[TxStatement], error: AppError) -> AppError {
    let failure = match &error {
        AppError::Db { message, .. } => parse_guard_failure(message),
        _ => None,
    };
    match failure {
        Some(f) => guard_error(statements, f),
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::Value;
    use serde_json::json;

    fn stmts() -> Vec<TxStatement> {
        vec![
            TxStatement::new(
                "UPDATE t SET v = ?1 WHERE id = ?2",
                vec![Value::Text("x".into()), Value::Int(1)],
            )
            .expecting(1),
            TxStatement::new("INSERT INTO t (v) VALUES ('y')", vec![]),
        ]
    }

    #[test]
    fn guards_follow_the_statements_that_expect_a_count() {
        let b = tx_batch(&stmts(), ParamEncoding::Native).unwrap();
        assert_eq!(
            b.roles,
            [
                StepRole::Statement(0),
                StepRole::Guard(0),
                StepRole::Statement(1)
            ]
        );
        assert_eq!(b.queries[1].sql, GUARD_SQL);
        assert_eq!(b.queries[1].params, [json!(1), json!(0)]);
        assert_eq!(b.queries[0].params, [json!("x"), json!(1)]);
        let strings = tx_batch(&stmts(), ParamEncoding::Strings).unwrap();
        assert_eq!(strings.queries[1].params, [json!("1"), json!("0")]);
        let too_many = vec![TxStatement::new("SELECT 1", vec![Value::Int(1); 101])];
        assert!(matches!(
            tx_batch(&too_many, ParamEncoding::Native),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn outcomes_and_errors() {
        let s = stmts();
        let b = tx_batch(&s, ParamEncoding::Native).unwrap();
        let result = |changes: u64, last: i64| -> RawResult {
            serde_json::from_value(json!({
                "results": {"columns": [], "rows": []},
                "meta": {"changes": changes, "last_row_id": last}
            }))
            .unwrap()
        };
        let out = tx_outcome(&s, &b, &[result(1, 0), result(1, 0), result(1, 4)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].affected, 1);
        assert_eq!(out[1].last_insert_id.as_deref(), Some("4"));
        // `meta.changes` counts what a cascade or trigger changed too: the committed batch is
        // still a success, with the count the guard proved.
        let cascaded = tx_outcome(&s, &b, &[result(3, 0), result(0, 0), result(2, 4)]);
        assert_eq!(
            cascaded.iter().map(|o| o.affected).collect::<Vec<_>>(),
            [1, 2]
        );
        // A reply that does not line up: still one outcome per statement, no guessed counts.
        let short = tx_outcome(&s, &b, &[result(1, 0)]);
        assert_eq!(
            short
                .iter()
                .map(|o| (o.affected, o.last_insert_id.clone()))
                .collect::<Vec<_>>(),
            [(1, None), (0, None)]
        );

        let guard = tx_error(
            &s,
            AppError::db("bad JSON path: 'tabler: statement 0 expected 1 changed 0'"),
        );
        assert!(
            guard
                .to_string()
                .starts_with("expected to affect 1 row(s) but matched 0")
        );
        assert!(matches!(
            guard,
            AppError::Db {
                statement_index: Some(0),
                ..
            }
        ));
        let other = tx_error(&s, AppError::db("UNIQUE constraint failed: t.v"));
        assert!(matches!(
            other,
            AppError::Db {
                statement_index: None,
                ..
            }
        ));
        assert!(matches!(tx_error(&s, AppError::Timeout), AppError::Timeout));
    }
}
