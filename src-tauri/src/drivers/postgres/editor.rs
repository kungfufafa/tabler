//! SQL-editor execution for PostgreSQL: describe with `prepare`, run with the simple query
//! protocol (text rows, any type), stream rows in chunks, cancel server-side.
//!
//! Why both protocols: the simple protocol returns every value as text, which `text_to_value`
//! decodes losslessly for any type (PostGIS, ranges, enums…), but tokio-postgres only exposes
//! column *names* for it. `prepare` (Parse + Describe, nothing executes) supplies the column
//! types. Statements that cannot return rows skip the describe round trip.

use std::ops::ControlFlow;
use std::time::Duration;

use futures::TryStreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio::net::TcpStream;
use tokio_postgres::config::Host;
use tokio_postgres::error::SqlState;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::types::{Kind, Type};
use tokio_postgres::{CancelToken, Client, Config, SimpleQueryMessage};

use super::PgTls;
use super::decode::{kind_from_type, text_to_value_with_elem};
use crate::drivers::{
    RowChunker, RowSink, StatementOutcome, cancel_safe_on_stop, cap_cancel_notice, contains_word,
    is_dml,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::split::{code_only, leading_keywords};
use crate::types::query::{ResultColumn, StatementKind};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

const CANCEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the server to confirm (by closing the socket) that it delivered a cancel.
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Upper-cased code of `sql` (literals, quoted identifiers and comments removed) for word checks.
fn code_upper(sql: &str) -> String {
    code_only(sql, Dialect::Postgres).to_uppercase()
}

/// Statements the simple protocol cannot run here: `COPY … FROM STDIN / TO STDOUT` would leave
/// the connection waiting for client-side copy data.
pub(crate) fn reject_unsupported(sql: &str) -> Result<(), AppError> {
    if leading_keywords(sql, 1).first().map(String::as_str) == Some("COPY") {
        let upper = code_upper(sql);
        if contains_word(&upper, "STDIN") || contains_word(&upper, "STDOUT") {
            return Err(AppError::Unsupported(
                "COPY … FROM STDIN / TO STDOUT needs a client-side data stream; use a server-side \
                 file or the export feature instead"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// Whether a statement can produce a result set (and is worth describing first).
pub(crate) fn may_return_rows(sql: &str) -> bool {
    let kw = leading_keywords(sql, 1);
    match kw.first().map(String::as_str) {
        None => true,
        Some("INSERT" | "UPDATE" | "DELETE" | "MERGE") => {
            contains_word(&code_upper(sql), "RETURNING")
        }
        Some(
            "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "BEGIN" | "START"
            | "COMMIT" | "ROLLBACK" | "END" | "ABORT" | "SAVEPOINT" | "RELEASE" | "SET" | "RESET"
            | "VACUUM" | "ANALYZE" | "COMMENT" | "DO" | "LOCK" | "LISTEN" | "UNLISTEN" | "NOTIFY"
            | "DISCARD" | "CHECKPOINT" | "CLUSTER" | "REINDEX" | "REFRESH" | "PREPARE"
            | "DEALLOCATE" | "DECLARE" | "CLOSE" | "COPY" | "SECURITY" | "LOAD" | "IMPORT"
            | "REASSIGN" | "MOVE",
        ) => false,
        _ => true,
    }
}

/// What `describe` learned about a statement's result columns.
#[derive(Debug, Default)]
pub(crate) struct Described {
    /// Column types; `None`: every column is decoded as text.
    pub types: Option<Vec<Type>>,
    /// Why the types are unknown, when that is worth telling the user.
    pub notice: Option<String>,
}

fn types_unknown_notice(reason: &str) -> String {
    format!(
        "The result's column types could not be determined ({reason}); values are shown as text."
    )
}

/// Column types of `sql` via Parse + Describe. Without types (several commands, unresolved `$n`
/// parameters…) the columns are decoded as text and the statement still runs.
///
/// Errors of the statement itself (syntax, unknown table…) are returned as they are, so a failed
/// describe inside a transaction reports the real cause instead of "current transaction is
/// aborted". So is a failure of the type lookup itself (tokio-postgres keeps its catalog
/// queries prepared on the connection, and `DEALLOCATE ALL` / `DISCARD ALL` run where the editor
/// does not see it, inside a function, drop them: 26000) or an internal server error, but only
/// inside a transaction block (`in_tx`), which that error has aborted already; anywhere else the
/// statement runs with text columns and a notice.
pub(crate) async fn describe(
    client: &Client,
    sql: &str,
    in_tx: bool,
) -> Result<Described, AppError> {
    let e = match client.prepare(sql).await {
        Ok(stmt) => {
            return Ok(Described {
                types: Some(stmt.columns().iter().map(|c| c.type_().clone()).collect()),
                notice: None,
            });
        }
        Err(e) => e,
    };
    if e.is_closed() {
        return Err(AppError::NotConnected);
    }
    let Some(db) = e.as_db_error() else {
        // Failed on the client (decoding the type catalog…): nothing went wrong on the server.
        log::warn!("postgres: describing a statement failed: {e}");
        return Ok(Described {
            types: None,
            notice: Some(types_unknown_notice(&e.to_string())),
        });
    };
    let code = db.code().code().to_string();
    if matches!(code.as_str(), "42P18" | "42P02" | "0A000" | "08P01")
        || (code == "42601" && db.message().contains("multiple commands"))
    {
        return Ok(Described::default());
    }
    let lookup_failed = code == "26000" || code.starts_with("XX");
    if lookup_failed && !in_tx {
        log::warn!("postgres: describing a statement failed ({code}): {e}");
        return Ok(Described {
            types: None,
            notice: Some(types_unknown_notice(db.message())),
        });
    }
    let mut err = AppError::from(e);
    if code == "26000"
        && let AppError::Db { hint, .. } = &mut err
    {
        *hint = Some(
            "Looking up the result's column types failed because prepared statements of this \
             connection were dropped (DEALLOCATE ALL / DISCARD ALL), which aborted the \
             transaction. Reconnect to restore them."
                .into(),
        );
    }
    Err(err)
}

/// Whether `sql` can drop prepared statements of the session.
pub(crate) fn drops_prepared_statements(sql: &str) -> bool {
    let kw = leading_keywords(sql, 2);
    match kw.first().map(String::as_str) {
        Some("DEALLOCATE") => true,
        Some("DISCARD") => kw.get(1).map(String::as_str) == Some("ALL"),
        _ => false,
    }
}

/// The protocol-level prepared statements of the session (by name) as `PREPARE` statements that
/// re-create them. On the editor these are tokio-postgres's type-lookup queries, which it
/// prepares once and keeps using for the life of the connection. Empty when the query fails
/// (aborted transaction…).
pub(crate) async fn protocol_statements(client: &Client) -> Vec<(String, String)> {
    const SQL: &str = "SELECT name, CASE WHEN coalesce(array_length(parameter_types, 1), 0) = 0 \
        THEN format('PREPARE %I AS %s', name, statement) \
        ELSE format('PREPARE %I (%s) AS %s', name, \
          array_to_string(parameter_types::pg_catalog.text[], ', '), statement) END \
        FROM pg_catalog.pg_prepared_statements WHERE NOT from_sql";
    match client.simple_query(SQL).await {
        Ok(messages) => messages
            .into_iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some((
                    row.get(0).unwrap_or_default().to_string(),
                    row.get(1).unwrap_or_default().to_string(),
                )),
                _ => None,
            })
            .filter(|(name, prepare)| !name.is_empty() && !prepare.is_empty())
            .collect(),
        Err(e) => {
            log::debug!("postgres: listing prepared statements failed: {e}");
            Vec::new()
        }
    }
}

/// Re-creates the statements of `before` (`protocol_statements`) that no longer exist, so
/// tokio-postgres's cached type lookups keep working after `DEALLOCATE` / `DISCARD ALL`.
/// Prepared statements are not transactional, and only missing names are prepared, so this
/// cannot fail (and abort) a transaction the user has open.
pub(crate) async fn restore_protocol_statements(client: &Client, before: &[(String, String)]) {
    if before.is_empty() {
        return;
    }
    let existing: std::collections::HashSet<String> = match client
        .simple_query("SELECT name FROM pg_catalog.pg_prepared_statements")
        .await
    {
        Ok(messages) => messages
            .into_iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
                _ => None,
            })
            .collect(),
        Err(e) => {
            log::warn!("postgres: cannot check the prepared statements to restore: {e}");
            return;
        }
    };
    let missing: Vec<&str> = before
        .iter()
        .filter(|(name, _)| !existing.contains(name))
        .map(|(_, prepare)| prepare.as_str())
        .collect();
    if missing.is_empty() {
        return;
    }
    if let Err(e) = client.batch_execute(&missing.join(";\n")).await {
        log::warn!("postgres: restoring the connection's prepared statements failed: {e}");
    }
}

/// How one text-protocol column is decoded.
struct TextColumn {
    kind: ValueKind,
    type_name: String,
    elem: Option<(ValueKind, String)>,
}

fn base_type(ty: &Type) -> &Type {
    match ty.kind() {
        Kind::Domain(inner) => base_type(inner),
        _ => ty,
    }
}

fn text_columns(
    names: &[tokio_postgres::SimpleColumn],
    types: Option<&[Type]>,
) -> (Vec<TextColumn>, Vec<ResultColumn>) {
    let types = types.filter(|t| t.len() == names.len());
    let mut decoders = Vec::with_capacity(names.len());
    let mut columns = Vec::with_capacity(names.len());
    for (i, col) in names.iter().enumerate() {
        let (kind, type_name, elem) = match types.map(|t| &t[i]) {
            Some(ty) => {
                let elem = match base_type(ty).kind() {
                    Kind::Array(e) => Some((kind_from_type(e), e.name().to_string())),
                    _ => None,
                };
                (kind_from_type(ty), ty.name().to_string(), elem)
            }
            None => (ValueKind::Text, "text".to_string(), None),
        };
        columns.push(ResultColumn {
            name: col.name().to_string(),
            data_type: type_name.clone(),
            kind,
            nullable: None,
            table: None,
        });
        decoders.push(TextColumn {
            kind,
            type_name,
            elem,
        });
    }
    (decoders, columns)
}

fn decode_text_row(
    row: &tokio_postgres::SimpleQueryRow,
    columns: &[TextColumn],
) -> Result<Vec<Value>, AppError> {
    let mut out = Vec::with_capacity(columns.len());
    for (i, col) in columns.iter().enumerate() {
        let cell: Option<&str> = row.try_get(i)?;
        out.push(match cell {
            None => Value::Null,
            Some(text) => text_to_value_with_elem(
                col.kind,
                &col.type_name,
                col.elem.as_ref().map(|(k, n)| (*k, n.as_str())),
                text,
            ),
        });
    }
    Ok(out)
}

/// Where to send a cancel request for the connection a statement runs on.
pub(crate) struct Canceller<'a> {
    pub token: CancelToken,
    pub tls: &'a PgTls,
    pub config: &'a Config,
}

impl Canceller<'_> {
    /// Sends a cancel request and waits until the server acted on it; `false` when it failed.
    pub(crate) async fn cancel_now(&self) -> bool {
        match cancel_and_wait(&self.token, self.tls, self.config).await {
            Ok(()) => true,
            Err(e) => {
                log::warn!("postgres: cancel request failed: {e}");
                false
            }
        }
    }
}

/// What `stream_simple` reports.
#[derive(Debug)]
pub(crate) struct Streamed {
    pub outcome: StatementOutcome,
    /// Reading stopped early, the statement was cancelled on the server to stop it, and the
    /// server confirmed that: the statement did not finish.
    pub cancelled: bool,
}

/// Runs `sql` with the simple query protocol and streams its rows into `sink`.
///
/// When a result set reaches `max_rows`, the rest of its rows are not decoded. A single
/// read-only query (`cancel_safe_on_stop`) with a `cancel_on_stop` canceller (inside a
/// transaction block only when the caller set a savepoint to roll the cancel back to) is then
/// cancelled on the server and the stream drained to `ReadyForQuery`; if the server confirms the
/// cancel (`Streamed::cancelled`), a notice says the statement did not finish. Every other statement keeps running to completion with its
/// surplus rows discarded, so a `CALL`, `FETCH` or `… RETURNING` still does all its work and
/// reports its real outcome (including a late error). A sink that returns `Break` (exports)
/// stops reading the same way, without the notice.
pub(crate) async fn stream_simple(
    client: &Client,
    sql: &str,
    types: Option<&[Type]>,
    max_rows: Option<usize>,
    cancel_on_stop: Option<Canceller<'_>>,
    sink: &mut dyn RowSink,
) -> Result<Streamed, AppError> {
    let stream = client.simple_query_raw(sql).await?;
    futures::pin_mut!(stream);
    // Whether to cancel is decided once, when the statement first has to stop early.
    let mut safe_to_cancel: Option<bool> = None;
    let mut cancel_safe =
        || *safe_to_cancel.get_or_insert_with(|| cancel_safe_on_stop(sql, Dialect::Postgres));
    let mut chunker = RowChunker::new(sink);
    let mut outcome = StatementOutcome::default();
    let mut decoders: Vec<TextColumn> = Vec::new();
    let mut command_has_rows = false;
    let mut affected: Option<u64> = None;
    let dml = is_dml(sql);
    // Rows past the cap of the current result set are read and dropped.
    let mut skipping = false;
    // Reading stopped altogether: `Some(true)` at the row cap, `Some(false)` on a sink `Break`.
    let mut stopped: Option<bool> = None;

    loop {
        let msg = match stream.try_next().await {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(e) => {
                let _ = chunker.flush();
                return Err(e.into());
            }
        };
        match msg {
            SimpleQueryMessage::RowDescription(names) => {
                let (dec, columns) = text_columns(&names, types);
                decoders = dec;
                command_has_rows = true;
                skipping = false;
                outcome.kind = StatementKind::Rows;
                outcome.result_sets += 1;
                if chunker.begin_set(columns).is_break() {
                    stopped = Some(false);
                    break;
                }
            }
            SimpleQueryMessage::Row(row) => {
                if skipping {
                    continue;
                }
                if let Some(cap) = max_rows
                    && chunker.rows_in_set as usize >= cap
                {
                    outcome.truncated = true;
                    if cancel_on_stop.is_some() && cancel_safe() {
                        stopped = Some(true);
                        break;
                    }
                    skipping = true;
                    continue;
                }
                let values = decode_text_row(&row, &decoders)?;
                if let ControlFlow::Break(()) = chunker.push(values) {
                    outcome.truncated = true;
                    stopped = Some(false);
                    break;
                }
            }
            SimpleQueryMessage::CommandComplete(n) => {
                // SELECT reports its row count here; DML (with or without RETURNING) its
                // affected rows; most other commands 0.
                if !command_has_rows || dml {
                    affected = Some(affected.unwrap_or(0) + n);
                }
                command_has_rows = false;
            }
            _ => {}
        }
    }
    let _ = chunker.flush();
    outcome.row_count = chunker.total_rows;
    let mut confirmed_cancel = false;

    if let Some(at_cap) = stopped {
        let cancelled = match &cancel_on_stop {
            Some(canceller) if cancel_safe() => canceller.cancel_now().await,
            _ => false,
        };
        let end = drain(stream.as_mut(), dml).await?;
        if let Some(n) = end.affected {
            affected = Some(affected.unwrap_or(0) + n);
        }
        match end.error {
            None => {}
            Some(e) if cancelled && e.code() == Some(&SqlState::QUERY_CANCELED) => {
                confirmed_cancel = true;
                if at_cap {
                    outcome
                        .notices
                        .push(cap_cancel_notice(outcome.row_count, Dialect::Postgres));
                }
            }
            // The statement failed on its own while we were no longer reading its rows.
            Some(e) if at_cap => return Err(e.into()),
            // The caller stopped reading (export done or cancelled): only the rows matter.
            Some(e) => log::debug!("postgres: error after the reader stopped: {e}"),
        }
    }

    match outcome.kind {
        StatementKind::Rows => {
            if dml {
                outcome.affected = affected;
            }
        }
        _ => {
            let n = affected.unwrap_or(0);
            if dml || n > 0 {
                outcome.kind = StatementKind::Affected;
                outcome.affected = Some(n);
            }
        }
    }
    Ok(Streamed {
        outcome,
        cancelled: confirmed_cancel,
    })
}

/// How a drained response ended.
struct DrainEnd {
    /// Rows reported by a DML statement's `CommandComplete` while draining.
    affected: Option<u64>,
    /// The first server error (the `57014` of our own cancel, or the statement's own error).
    error: Option<tokio_postgres::Error>,
}

/// Reads and discards the rest of a response up to `ReadyForQuery`, keeping the counters and the
/// first server error.
async fn drain<S>(mut stream: std::pin::Pin<&mut S>, dml: bool) -> Result<DrainEnd, AppError>
where
    S: futures::Stream<Item = Result<SimpleQueryMessage, tokio_postgres::Error>>,
{
    let mut end = DrainEnd {
        affected: None,
        error: None,
    };
    let mut errors = 0;
    loop {
        match stream.try_next().await {
            Ok(Some(SimpleQueryMessage::CommandComplete(n))) if dml => {
                end.affected = Some(end.affected.unwrap_or(0) + n);
            }
            Ok(Some(_)) => {}
            Ok(None) => return Ok(end),
            Err(e) if e.is_closed() => return Err(AppError::NotConnected),
            Err(e) => {
                log::debug!("postgres: error while draining a stopped statement: {e}");
                errors += 1;
                if end.error.is_none() {
                    end.error = Some(e);
                }
                if errors > 2 {
                    return Ok(end);
                }
            }
        }
    }
}

/// Drains a binary row stream (worker `stream_query` with parameters).
pub(crate) async fn drain_rows(
    mut stream: std::pin::Pin<&mut tokio_postgres::RowStream>,
) -> Result<(), AppError> {
    loop {
        match stream.try_next().await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(()),
            Err(e) if e.is_closed() => return Err(AppError::NotConnected),
            Err(_) => return Ok(()),
        }
    }
}

/// Sends a cancel request and waits until the server has acted on it.
///
/// `CancelToken::cancel_query` returns as soon as the request is written, so the signal can
/// still be in flight when the next statement starts, which it would then cancel instead. The
/// server closes the cancel connection only after delivering the signal, so this routes the
/// request through an in-memory pipe and waits for that close. Falls back to the plain request
/// for non-TCP (Unix socket) hosts.
pub(crate) async fn cancel_and_wait(
    token: &CancelToken,
    tls: &PgTls,
    config: &Config,
) -> Result<(), AppError> {
    let target = match (config.get_hosts().first(), config.get_ports().first()) {
        (Some(Host::Tcp(host)), Some(port)) => Some((host.clone(), *port)),
        _ => None,
    };
    let Some((host, port)) = target else {
        return tokio::time::timeout(CANCEL_CONNECT_TIMEOUT, token.cancel_query(tls.clone()))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(AppError::from);
    };

    let tcp = tokio::time::timeout(
        CANCEL_CONNECT_TIMEOUT,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Connect(format!("cancel request: {e}")))?;
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let (client_side, proxy) = tokio::io::duplex(16 * 1024);
    let (mut proxy_r, mut proxy_w) = tokio::io::split(proxy);
    let upstream = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut proxy_r, &mut tcp_w).await;
        let _ = tcp_w.shutdown().await;
    });
    let downstream = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                // The client half may be gone already; only the server's close matters.
                Ok(n) => {
                    let _ = proxy_w.write_all(&buf[..n]).await;
                }
            }
        }
    });

    let mut make = tls.clone();
    let connector = <PgTls as MakeTlsConnect<DuplexStream>>::make_tls_connect(&mut make, &host)
        .map_err(|e| AppError::Connect(format!("cancel request: {e}")))?;
    let sent = tokio::time::timeout(
        CANCEL_CONNECT_TIMEOUT,
        token.cancel_query_raw(client_side, connector),
    )
    .await;
    let result = match sent {
        Err(_) => Err(AppError::Timeout),
        Ok(Err(e)) => Err(AppError::from(e)),
        Ok(Ok(())) => {
            let _ = tokio::time::timeout(CANCEL_SETTLE_TIMEOUT, downstream).await;
            Ok(())
        }
    };
    upstream.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_statements() {
        assert!(may_return_rows("select 1"));
        assert!(may_return_rows("  (select 1) union (select 2)"));
        assert!(may_return_rows("with x as (select 1) select * from x"));
        assert!(may_return_rows("insert into t values (1) returning id"));
        assert!(!may_return_rows("insert into t values ('returning_x')"));
        assert!(!may_return_rows("update t set a = 1"));
        assert!(!may_return_rows("BEGIN"));
        assert!(!may_return_rows("create table t (a int)"));
        assert!(may_return_rows("show search_path"));
        assert!(may_return_rows("explain select 1"));
        assert!(may_return_rows("fetch 10 from c"));
    }

    #[test]
    fn rejects_client_side_copy() {
        assert!(reject_unsupported("copy t from stdin").is_err());
        assert!(reject_unsupported("COPY (select 1) TO STDOUT WITH CSV").is_err());
        assert!(reject_unsupported("copy t from '/tmp/x.csv'").is_ok());
        assert!(reject_unsupported("select 'copy from stdin'").is_ok());
        assert!(reject_unsupported("copy stdin_log from '/tmp/x'").is_ok());
    }

    #[test]
    fn whole_word_matching() {
        assert!(contains_word("A RETURNING B", "RETURNING"));
        assert!(contains_word("RETURNING", "RETURNING"));
        assert!(!contains_word("XRETURNING", "RETURNING"));
        assert!(!contains_word("RETURNING_X", "RETURNING"));
    }
}
