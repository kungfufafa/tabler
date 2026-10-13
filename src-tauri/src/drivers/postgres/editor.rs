//! SQL-editor execution for PostgreSQL: describe with `prepare`, run with the simple query
//! protocol (text rows, any type), stream rows in chunks, cancel server-side.
//!
//! Why both protocols: the simple protocol returns every value as text, which `text_to_value`
//! decodes losslessly for any type (PostGIS, ranges, enums…), but tokio-postgres only exposes
//! column *names* for it. `prepare` (Parse + Describe, nothing executes) supplies the column
//! types. Statements that cannot return rows skip the describe round trip.

use std::collections::{HashMap, VecDeque};
use std::ops::ControlFlow;
use std::sync::Arc;
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
use super::decode::{
    RelationCache, RelationNames, cache_relations, cached_relation, column_source, kind_from_type,
    origins_from, text_to_value_with_elem,
};
use crate::drivers::{
    RowChunker, RowSink, StatementOutcome, cancel_safe_on_stop, cap_cancel_notice, contains_word,
    is_dml,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::split::{code_only, leading_keywords};
use crate::types::query::{ColumnOrigin, ResultColumn, ServerNotice, StatementKind};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

const CANCEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the server to confirm (by closing the socket) that it delivered a cancel.
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Notices the hub keeps until they are taken; older ones are dropped.
const MAX_QUEUED_NOTICES: usize = 1000;
/// Notices one statement hands to its sink as they arrive; later ones are counted, not sent.
const MAX_LIVE_NOTICES: usize = 1000;

/// A notice as the server sent it (`RAISE NOTICE`, warnings…).
pub(crate) fn notice_from(n: &tokio_postgres::error::DbError) -> ServerNotice {
    ServerNotice {
        severity: n.severity().to_string(),
        message: n.message().to_string(),
        detail: n.detail().map(str::to_string),
        hint: n.hint().map(str::to_string),
        context: n.where_().map(str::to_string),
    }
}

/// Server notices received on the editor connection, oldest first. The connection task `push`es
/// them as they arrive; the statement being streamed hands them to its sink right away
/// (`stream_simple`), and the rest is taken before (`hand_over_earlier`) or after a statement.
#[derive(Debug, Default)]
pub(crate) struct NoticeHub {
    queue: std::sync::Mutex<VecDeque<ServerNotice>>,
    /// Signalled on every push (a permit is stored when nobody waits).
    arrived: tokio::sync::Notify,
}

impl NoticeHub {
    fn queue(&self) -> std::sync::MutexGuard<'_, VecDeque<ServerNotice>> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Called by the connection task for every `NoticeResponse`.
    pub(crate) fn push(&self, notice: &tokio_postgres::error::DbError) {
        self.push_notice(notice_from(notice));
    }

    pub(crate) fn push_notice(&self, notice: ServerNotice) {
        {
            let mut queue = self.queue();
            if queue.len() >= MAX_QUEUED_NOTICES {
                queue.pop_front();
            }
            queue.push_back(notice);
        }
        self.arrived.notify_one();
    }

    /// Every queued notice, oldest first.
    pub(crate) fn take(&self) -> Vec<ServerNotice> {
        self.queue().drain(..).collect()
    }

    /// `take`, as text (`ServerNotice::to_text`).
    pub(crate) fn take_text(&self) -> Vec<String> {
        self.take().iter().map(ServerNotice::to_text).collect()
    }

    /// Before a statement: hands the notices raised since the last one (bootstrap SQL, the
    /// session's activity between runs) to `sink` as raised outside it; a sink that does not
    /// take them gets them logged.
    pub(crate) fn hand_over_earlier(&self, sink: &mut dyn RowSink) {
        for notice in self.take() {
            if !sink.notice(&notice, false) {
                log::info!("postgres notice outside a statement: {}", notice.to_text());
            }
        }
    }
}

/// Hands a statement's notices to its sink as they arrive, up to `MAX_LIVE_NOTICES`. Notices the
/// sink does not take stay in the outcome as text.
struct NoticeForwarder<'h> {
    hub: Option<&'h NoticeHub>,
    sent: usize,
    dropped: usize,
}

impl NoticeForwarder<'_> {
    fn forward(&mut self, chunker: &mut RowChunker<'_>, outcome: &mut StatementOutcome) {
        let Some(hub) = self.hub else {
            return;
        };
        for notice in hub.take() {
            if self.sent >= MAX_LIVE_NOTICES {
                self.dropped += 1;
            } else if chunker.notice(&notice, true) {
                self.sent += 1;
            } else {
                outcome.notices.push(notice.to_text());
            }
        }
    }

    /// The last notices, and a note on how many were not sent.
    fn finish(mut self, chunker: &mut RowChunker<'_>, outcome: &mut StatementOutcome) {
        self.forward(chunker, outcome);
        if self.dropped > 0 {
            outcome.notices.push(format!(
                "{} more notices were raised and not shown (only the first {MAX_LIVE_NOTICES} \
                 are).",
                self.dropped
            ));
        }
    }
}

/// The next message of a simple-query response; meanwhile, notices that arrive go to the sink.
async fn next_message<S>(
    stream: &mut std::pin::Pin<&mut S>,
    notices: &mut NoticeForwarder<'_>,
    chunker: &mut RowChunker<'_>,
    outcome: &mut StatementOutcome,
) -> Result<Option<SimpleQueryMessage>, tokio_postgres::Error>
where
    S: futures::Stream<Item = Result<SimpleQueryMessage, tokio_postgres::Error>>,
{
    let Some(hub) = notices.hub else {
        return stream.try_next().await;
    };
    loop {
        tokio::select! {
            // A notice is sent before the messages that follow it: hand it over first.
            biased;
            () = hub.arrived.notified() => notices.forward(chunker, outcome),
            msg = stream.try_next() => return msg,
        }
    }
}

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
    /// Column types and origins; `None`: every column is decoded as text.
    pub types: Option<Box<DescribedColumns>>,
    /// Why the types are unknown, when that is worth telling the user.
    pub notice: Option<String>,
}

/// The described result columns: their types, and the base-table column each one reads.
#[derive(Debug, Default)]
pub(crate) struct DescribedColumns {
    pub types: Vec<Type>,
    pub origins: Vec<Option<ColumnOrigin>>,
}

/// Every non-dropped column of the relations `$1` (an `oid[]`): what result column origins are
/// resolved with.
const RELATION_NAMES_SQL: &str = "SELECT c.oid, n.nspname::text, c.relname::text, a.attnum, \
    a.attname::text FROM pg_catalog.pg_class c \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 \
    AND NOT a.attisdropped WHERE c.oid = ANY($1)";

/// Resolves the table OID / attribute number of each RowDescription field to schema, table and
/// column names (`origins_from`). Relations already looked up in this run come from the run's
/// cache; the rest is one catalog query. A failed lookup only costs the origins (the result is
/// then read-only): it is logged, never reported.
async fn column_origins(
    client: &Client,
    columns: &[tokio_postgres::Column],
) -> Vec<Option<ColumnOrigin>> {
    let sources: Vec<Option<(u32, i16)>> = columns.iter().map(column_source).collect();
    let mut relations = RelationCache::new();
    let mut missing: Vec<u32> = Vec::new();
    for (oid, _) in sources.iter().flatten() {
        if relations.contains_key(oid) || missing.contains(oid) {
            continue;
        }
        match cached_relation(*oid) {
            Some(names) => {
                relations.insert(*oid, names);
            }
            None => missing.push(*oid),
        }
    }
    if !missing.is_empty() {
        match client.query(RELATION_NAMES_SQL, &[&missing]).await {
            Ok(rows) => {
                let mut found: HashMap<u32, RelationNames> = HashMap::new();
                for row in rows {
                    let (Ok(oid), Ok(schema), Ok(name)) = (
                        row.try_get::<_, u32>(0),
                        row.try_get::<_, String>(1),
                        row.try_get::<_, String>(2),
                    ) else {
                        continue;
                    };
                    let rel = found.entry(oid).or_insert_with(|| RelationNames {
                        schema,
                        name,
                        columns: HashMap::new(),
                    });
                    if let (Ok(Some(attnum)), Ok(Some(column))) = (
                        row.try_get::<_, Option<i16>>(3),
                        row.try_get::<_, Option<String>>(4),
                    ) {
                        rel.columns.insert(attnum, column);
                    }
                }
                let looked_up: RelationCache = missing
                    .iter()
                    .map(|oid| (*oid, found.remove(oid).map(Arc::new)))
                    .collect();
                cache_relations(&looked_up);
                relations.extend(looked_up);
            }
            Err(e) => log::debug!("postgres: looking up result column origins failed: {e}"),
        }
    }
    origins_from(&sources, &relations)
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
            let origins = column_origins(client, stmt.columns()).await;
            return Ok(Described {
                types: Some(Box::new(DescribedColumns {
                    types: stmt.columns().iter().map(|c| c.type_().clone()).collect(),
                    origins,
                })),
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
    described: Option<&DescribedColumns>,
) -> (Vec<TextColumn>, Vec<ResultColumn>) {
    let described = described.filter(|d| d.types.len() == names.len());
    let types = described.map(|d| d.types.as_slice());
    let mut decoders = Vec::with_capacity(names.len());
    let mut columns = Vec::with_capacity(names.len());
    for (i, col) in names.iter().enumerate() {
        let origin = described.and_then(|d| d.origins.get(i).cloned().flatten());
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
            table: origin.as_ref().map(|o| o.table.name.clone()),
            origin,
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
///
/// `notices`: the editor connection's notices, handed to `sink` while the statement runs
/// (`RowSink::notice`).
pub(crate) async fn stream_simple(
    client: &Client,
    sql: &str,
    types: Option<&DescribedColumns>,
    max_rows: Option<usize>,
    cancel_on_stop: Option<Canceller<'_>>,
    notices: Option<&NoticeHub>,
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
    let mut forwarder = NoticeForwarder {
        hub: notices,
        sent: 0,
        dropped: 0,
    };
    let mut decoders: Vec<TextColumn> = Vec::new();
    let mut command_has_rows = false;
    let mut affected: Option<u64> = None;
    let dml = is_dml(sql);
    // Rows past the cap of the current result set are read and dropped.
    let mut skipping = false;
    // Reading stopped altogether: `Some(true)` at the row cap, `Some(false)` on a sink `Break`.
    let mut stopped: Option<bool> = None;

    loop {
        let msg = match next_message(&mut stream, &mut forwarder, &mut chunker, &mut outcome).await
        {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(e) => {
                let _ = chunker.flush();
                // Notices raised before the error (`RAISE NOTICE` … `RAISE EXCEPTION`).
                forwarder.forward(&mut chunker, &mut outcome);
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
            Some(e) if at_cap => {
                forwarder.forward(&mut chunker, &mut outcome);
                return Err(e.into());
            }
            // The caller stopped reading (export done or cancelled): only the rows matter.
            Some(e) => log::debug!("postgres: error after the reader stopped: {e}"),
        }
    }
    forwarder.finish(&mut chunker, &mut outcome);

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
///
/// The request goes to the address `config` is pinned to (`hostaddr`: the one the editor
/// connected to, so a host name with several servers behind it reaches the right one), else, or
/// when that address cannot be reached, to the host name.
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

    let dial = async |addr: (&str, u16)| {
        tokio::time::timeout(CANCEL_CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(|e| AppError::Connect(format!("cancel request: {e}")))
    };
    let pinned = config.get_hostaddrs().first().map(ToString::to_string);
    let tcp = match pinned {
        Some(ip) => match dial((ip.as_str(), port)).await {
            Ok(tcp) => tcp,
            // Nothing was sent: the host name may lead to the server now.
            Err(e) => {
                log::debug!("postgres: cancel request to {ip}: {e}; trying {host}");
                dial((host.as_str(), port)).await?
            }
        },
        None => dial((host.as_str(), port)).await?,
    };
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

    fn notice(message: &str) -> ServerNotice {
        ServerNotice {
            severity: "NOTICE".into(),
            message: message.into(),
            ..ServerNotice::default()
        }
    }

    /// Keeps notices when `accept` is set (as the editor's event sink does).
    #[derive(Default)]
    struct NoticeSink {
        accept: bool,
        got: Vec<(String, bool)>,
    }

    impl RowSink for NoticeSink {
        fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}
        fn rows(&mut self, _rows: Vec<Vec<Value>>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
        fn notice(&mut self, notice: &ServerNotice, during: bool) -> bool {
            if self.accept {
                self.got.push((notice.message.clone(), during));
            }
            self.accept
        }
    }

    #[test]
    fn notice_hub_queues_and_caps() {
        let hub = NoticeHub::default();
        for i in 0..=MAX_QUEUED_NOTICES {
            hub.push_notice(notice(&i.to_string()));
        }
        let all = hub.take();
        assert_eq!(all.len(), MAX_QUEUED_NOTICES);
        assert_eq!(all[0].message, "1", "the oldest one is dropped");
        assert!(hub.take().is_empty());
        hub.push_notice(ServerNotice {
            hint: Some("h".into()),
            ..notice("x")
        });
        assert_eq!(hub.take_text(), ["NOTICE: x\nHINT: h"]);
    }

    #[test]
    fn earlier_notices_are_handed_over_as_outside_the_statement() {
        let hub = NoticeHub::default();
        hub.push_notice(notice("bootstrap"));
        let mut sink = NoticeSink {
            accept: true,
            ..NoticeSink::default()
        };
        hub.hand_over_earlier(&mut sink);
        assert_eq!(sink.got, [("bootstrap".to_string(), false)]);
        // A sink that does not take them: they are logged, and gone either way.
        hub.push_notice(notice("again"));
        hub.hand_over_earlier(&mut NoticeSink::default());
        assert!(hub.take().is_empty());
    }

    #[test]
    fn forwarder_sends_live_or_keeps_text() {
        let hub = NoticeHub::default();
        let mut sink = NoticeSink {
            accept: true,
            ..NoticeSink::default()
        };
        let mut outcome = StatementOutcome::default();
        {
            let mut chunker = RowChunker::new(&mut sink);
            let fwd = NoticeForwarder {
                hub: Some(&hub),
                sent: MAX_LIVE_NOTICES - 1,
                dropped: 0,
            };
            hub.push_notice(notice("a"));
            hub.push_notice(notice("b"));
            hub.push_notice(notice("c"));
            fwd.finish(&mut chunker, &mut outcome);
        }
        assert_eq!(sink.got, [("a".to_string(), true)]);
        assert_eq!(outcome.notices.len(), 1);
        assert!(outcome.notices[0].starts_with("2 more notices"));

        // Sinks that take no notices (CollectSink, exports) keep them in the outcome.
        let mut plain = NoticeSink::default();
        let mut outcome = StatementOutcome::default();
        let mut chunker = RowChunker::new(&mut plain);
        let fwd = NoticeForwarder {
            hub: Some(&hub),
            sent: 0,
            dropped: 0,
        };
        hub.push_notice(notice("kept"));
        fwd.finish(&mut chunker, &mut outcome);
        assert_eq!(outcome.notices, ["NOTICE: kept"]);
    }

    #[tokio::test]
    async fn notices_are_handed_over_before_the_next_message() {
        let hub = NoticeHub::default();
        let mut sink = NoticeSink {
            accept: true,
            ..NoticeSink::default()
        };
        let mut outcome = StatementOutcome::default();
        let mut chunker = RowChunker::new(&mut sink);
        let mut fwd = NoticeForwarder {
            hub: Some(&hub),
            sent: 0,
            dropped: 0,
        };
        hub.push_notice(notice("first"));
        let stream = futures::stream::iter(vec![Ok::<_, tokio_postgres::Error>(
            SimpleQueryMessage::CommandComplete(0),
        )]);
        futures::pin_mut!(stream);
        let msg = next_message(&mut stream, &mut fwd, &mut chunker, &mut outcome)
            .await
            .unwrap();
        assert!(matches!(msg, Some(SimpleQueryMessage::CommandComplete(0))));
        assert_eq!(fwd.sent, 1);
        let end = next_message(&mut stream, &mut fwd, &mut chunker, &mut outcome)
            .await
            .unwrap();
        assert!(end.is_none());
        drop(chunker);
        assert_eq!(sink.got, [("first".to_string(), true)]);
    }

    #[test]
    fn whole_word_matching() {
        assert!(contains_word("A RETURNING B", "RETURNING"));
        assert!(contains_word("RETURNING", "RETURNING"));
        assert!(!contains_word("XRETURNING", "RETURNING"));
        assert!(!contains_word("RETURNING_X", "RETURNING"));
    }
}
