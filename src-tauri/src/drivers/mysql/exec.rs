//! Generic query/execute path shared by pooled connections and transactions.
//!
//! Statements without parameters go through the text protocol (`query_iter`, multi-statement
//! capable); parameterised ones through the binary protocol (`exec_iter`). Only the first result
//! set is returned; the rest is drained so the connection can be reused.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use mysql_async::prelude::Queryable;
use mysql_async::{BinaryProtocol, Column, Params, QueryResult, Row, TextProtocol};

use super::decode::{result_column, value_from_mysql};
use super::params::bind;
use crate::error::AppError;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// Common view over the text and binary `QueryResult`s (`mysql_async::Protocol` is private).
trait ResultLike: Send {
    fn is_empty(&self) -> bool;
    fn columns(&self) -> Option<Arc<[Column]>>;
    fn affected_rows(&self) -> u64;
    fn last_insert_id(&self) -> Option<u64>;
    /// Collects the current result set (stops at the result-set boundary).
    fn collect_rows(&mut self) -> BoxFuture<'_, mysql_async::Result<Vec<Row>>>;
}

macro_rules! impl_result_like {
    ($proto:ty) => {
        impl ResultLike for QueryResult<'_, '_, $proto> {
            fn is_empty(&self) -> bool {
                QueryResult::is_empty(self)
            }
            fn columns(&self) -> Option<Arc<[Column]>> {
                QueryResult::columns(self)
            }
            fn affected_rows(&self) -> u64 {
                QueryResult::affected_rows(self)
            }
            fn last_insert_id(&self) -> Option<u64> {
                QueryResult::last_insert_id(self)
            }
            fn collect_rows(&mut self) -> BoxFuture<'_, mysql_async::Result<Vec<Row>>> {
                self.collect::<Row>().boxed()
            }
        }
    };
}

impl_result_like!(TextProtocol);
impl_result_like!(BinaryProtocol);

struct Collected {
    first: Option<ResultSet>,
    affected: u64,
    last_insert_id: Option<u64>,
}

/// Decodes a result set's rows into app values.
fn decode_rows(columns: &[Column], rows: Vec<Row>) -> ResultSet {
    let result_columns: Vec<_> = columns.iter().map(result_column).collect();
    let kinds: Vec<ValueKind> = result_columns.iter().map(|c| c.kind).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = row
            .unwrap()
            .into_iter()
            .zip(columns.iter())
            .zip(kinds.iter())
            .map(|((v, col), kind)| value_from_mysql(*kind, col, v))
            .collect();
        out.push(cells);
    }
    ResultSet {
        columns: result_columns,
        rows: out,
        truncated: false,
    }
}

async fn collect_first<R: ResultLike>(result: &mut R) -> Result<Collected, AppError> {
    let columns = result.columns().unwrap_or_else(|| Arc::from(Vec::new()));
    let collected = if columns.is_empty() {
        // OK packet already read: counters are final. Consume the empty set to advance.
        let affected = result.affected_rows();
        let last_insert_id = result.last_insert_id();
        result.collect_rows().await?;
        Collected {
            first: None,
            affected,
            last_insert_id,
        }
    } else {
        let rows = result.collect_rows().await?;
        // Counters arrive in the terminator packet after the rows (MariaDB `RETURNING`).
        let affected = result.affected_rows();
        let last_insert_id = result.last_insert_id();
        Collected {
            first: Some(decode_rows(&columns, rows)),
            affected,
            last_insert_id,
        }
    };
    // Drain remaining result sets (multi-statement, CALL) so the connection is reusable.
    while !result.is_empty() {
        result.collect_rows().await?;
    }
    Ok(collected)
}

async fn run<Q: Queryable>(q: &mut Q, sql: &str, params: &[Value]) -> Result<Collected, AppError> {
    if params.is_empty() {
        let mut result = q.query_iter(sql).await?;
        collect_first(&mut result).await
    } else {
        let bound = bind(params)?;
        let mut result = q.exec_iter(sql, Params::Positional(bound)).await?;
        collect_first(&mut result).await
    }
}

/// Runs a row-returning statement and decodes its first result set.
pub async fn run_query<Q: Queryable>(
    q: &mut Q,
    sql: &str,
    params: &[Value],
) -> Result<ResultSet, AppError> {
    let collected = run(q, sql, params).await?;
    Ok(collected.first.unwrap_or_default())
}

/// Runs a statement for its side effects; rows are kept when the server produced any.
pub async fn run_execute<Q: Queryable>(
    q: &mut Q,
    sql: &str,
    params: &[Value],
) -> Result<ExecOutcome, AppError> {
    let collected = run(q, sql, params).await?;
    Ok(ExecOutcome {
        affected: collected.affected,
        last_insert_id: collected
            .last_insert_id
            .filter(|id| *id != 0)
            .map(|id| id.to_string()),
        rows: collected.first,
    })
}
