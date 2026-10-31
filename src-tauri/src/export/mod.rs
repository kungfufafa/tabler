//! Streaming data export (M6): a table (with the grid's filter / sort / column choice) or one
//! SQL statement, written to CSV, JSON / NDJSON, SQL `INSERT` statements, HTML or Excel, in the
//! chosen text encoding (`encoding`).
//!
//! Rows are streamed from a worker connection (`DbSession::stream_query`) straight into a
//! format writer on a buffered temporary file next to the destination; nothing holds the whole
//! result. The temporary file is renamed over the destination only when every row was written,
//! and removed on error or cancellation. Progress is reported at most every
//! `PROGRESS_INTERVAL` or `PROGRESS_ROWS` rows, whichever comes first.
//!
//! A table export's SELECT is the session dialect's (`sql::export_select`). PostgreSQL table
//! exports select every column as text (as browsing does) and decode it here by the column's kind
//! (arrays by their element type), so `bytea` arrives in full. MySQL and SQLite drivers cap
//! binary cells at `BYTES_PREVIEW_CAP`; table exports therefore also select the hex text of
//! larger blobs and restore them (on SQLite for every column, since any column can hold a blob).
//! A query export cannot do that: a capped binary cell fails the export instead of silently
//! writing a truncated value. An extension dialect reads the columns it projects as text, decoded
//! here as the grid decodes them, and refuses one it would read cut. SQL scripts (`INSERT`
//! exports) are written in the core dialects only.
//!
//! A query export only runs statements that return rows (`sql_query`): the worker connection
//! it borrows goes back to the pool, so `BEGIN`, `SET`, `LOCK`… must never run there. It runs
//! on a core session only (`require_query_export`), whose server enforces a read-only connection.
//!
//! A rows export (`ExportSource::Rows`: a result the SQL editor shows) runs nothing at all: the
//! renderer's rows go through the same sink, in chunks, so progress, `max_rows`, cancellation
//! and the temporary file behave exactly as for a query.

pub mod encoding;
pub mod file;
pub mod results;
pub mod structure;
pub mod tables;
pub mod writer;
pub mod xlsx;

/// For the app's exit hook (`RunEvent::Exit`): deletes the temporary files of exports that are
/// still running, which would otherwise stay behind as hidden partial files.
pub use file::remove_partial_files;

use std::io::{BufWriter, Write};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio_util::sync::CancellationToken;

use crate::drivers::postgres::decode::{
    bytea_text_to_bytes, kind_from_name, parse_pg_array, text_to_value, text_to_value_with_elem,
};
use crate::drivers::{CancelSlot, DbSession, RowSink, contains_word, request_cancel};
use crate::error::AppError;
use crate::sqlgen::dialect::DialectRef;
use crate::sqlgen::split::{is_meta_command, leading_keywords};
use crate::sqlgen::sql::{self, ColumnForm, SelectQuery};
use crate::sqlgen::transfer::{default_time_zone, script_table};
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::browse::{FilterGroup, RowIdentity, SortDir, SortSpec};
use crate::types::caps::Feature;
use crate::types::export::{
    ExportFormat, ExportProgress, ExportRequest, ExportResult, ExportSource,
};
use crate::types::query::ResultColumn;
use crate::types::schema::{ColumnMeta, TableKind, TableMeta, ValueKind};
use crate::types::value::{BYTES_PREVIEW_CAP, BytesValue, Value};

use encoding::{Encoded, effective_encoding, html_charset};
use file::{BlockingWrite, Compressed, CountingWriter, TempTarget, validate_target};
use structure::{ScriptTable, closing_text, context_for, script_layout};
use writer::{FormatSpec, RowOut, RowWriter, SqlScript};

/// Progress is reported when this much time passed since the last report…
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// …or when this many rows were written since then.
pub const PROGRESS_ROWS: u64 = 5000;
/// After a cancel request the sink stops the stream at its next chunk, and the driver cancels a
/// read-only statement itself. A statement still running this long after the request (one
/// that delivers nothing, or one the driver would otherwise drain to its end) is cancelled
/// through its cancel handle.
const CANCEL_GRACE: Duration = Duration::from_millis(300);
const FILE_BUFFER: usize = 256 * 1024;

/// One column of the exported file and how a PostgreSQL text cell of it is decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputColumn {
    pub name: String,
    pub kind: ValueKind,
    pub data_type: String,
    /// PostgreSQL arrays: the element type name (`bool`, `int4`…). The table's `data_type`
    /// (`boolean[]`) does not name it in a form the text decoder understands.
    pub array_elem: Option<String>,
    /// The export's SELECT reads the column as text (`sql::column_form`): a text cell is decoded
    /// by the column's kind.
    pub as_text: bool,
}

impl From<&ColumnMeta> for OutputColumn {
    fn from(c: &ColumnMeta) -> Self {
        OutputColumn {
            name: c.name.clone(),
            kind: c.kind,
            data_type: c.data_type.clone(),
            array_elem: c.array_elem_type.clone(),
            as_text: false,
        }
    }
}

impl From<ResultColumn> for OutputColumn {
    fn from(c: ResultColumn) -> Self {
        OutputColumn {
            name: c.name,
            kind: c.kind,
            data_type: c.data_type,
            array_elem: None,
            as_text: false,
        }
    }
}

impl OutputColumn {
    /// Decodes a PostgreSQL text cell of this column. `bytea` (also inside arrays) is decoded
    /// in full, without the preview cap.
    fn decode_pg_text(&self, text: &str) -> Value {
        match (self.kind, self.array_elem.as_deref()) {
            (ValueKind::Bytes, _) => full_bytes(&bytea_text_to_bytes(text)),
            (ValueKind::Array, Some(elem)) => match kind_from_name(elem) {
                ValueKind::Bytes => match parse_pg_array(text) {
                    Some(items) => Value::Array(
                        items
                            .into_iter()
                            .map(|item| match item {
                                Some(s) => full_bytes(&bytea_text_to_bytes(&s)),
                                None => Value::Null,
                            })
                            .collect(),
                    ),
                    None => Value::other(&self.data_type, text),
                },
                elem_kind => text_to_value_with_elem(
                    self.kind,
                    &self.data_type,
                    Some((elem_kind, elem)),
                    text,
                ),
            },
            (kind, _) => text_to_value(kind, &self.data_type, text),
        }
    }

    /// A MySQL single-precision `FLOAT` column: its values are f32.
    fn is_mysql_float(&self) -> bool {
        self.kind == ValueKind::Float
            && self
                .data_type
                .trim_start()
                .get(..5)
                .is_some_and(|t| t.eq_ignore_ascii_case("float"))
    }
}

/// The shortest text of an f32 value, read back as f64. MySQL's binary protocol (used when the
/// statement has parameters, e.g. a filtered table export) delivers `FLOAT` as f32, which
/// widened to f64 prints as `1.100000023841858`; the text protocol sends `1.1`. Both come out
/// as `1.1` here, the same f32 value.
fn narrow_float(f: f64) -> f64 {
    let narrow = f as f32;
    if narrow.is_finite() {
        narrow.to_string().parse().unwrap_or(f)
    } else {
        f
    }
}

/// The statement an export streams.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportQuery {
    pub sql: String,
    pub params: Vec<Value>,
    /// Known output columns (table exports); `None` = take them from the result set.
    pub columns: Option<Vec<OutputColumn>>,
    /// For each extra column appended after the output columns: the output column whose large
    /// blob it carries as hex text (NULL when the blob is small enough to arrive in full).
    pub blob_hex: Vec<usize>,
    /// PostgreSQL: an exported column is an identity column `GENERATED ALWAYS`, so INSERT
    /// statements need `OVERRIDING SYSTEM VALUE` to load the rows.
    pub identity_always: bool,
}

/// The SELECT for a table export: the grid's columns, filter and sort, no paging (or
/// `LIMIT max_rows`, starting after `offset` rows: a table tab's page), in the session's dialect
/// (`sql::export_select`). `skip_generated` leaves generated columns out when no explicit column
/// list is given (INSERT exports cannot write them).
///
/// A page (`offset` or `max_rows`) is ordered deterministically, so consecutive pages neither
/// overlap nor miss rows: the sort, then the primary key; a table without a key by `rowid`
/// (SQLite) or by every selected column.
#[allow(clippy::too_many_arguments)]
pub fn table_query(
    dialect: impl Into<SqlDialect>,
    meta: &TableMeta,
    selected: Option<&[String]>,
    filter: Option<&FilterGroup>,
    sort: &[SortSpec],
    max_rows: Option<u64>,
    offset: u64,
    skip_generated: bool,
) -> Result<ExportQuery, AppError> {
    let dialect = dialect.into();
    let names: Vec<String> = match selected {
        Some([]) => {
            return Err(AppError::InvalidInput(
                "no columns are selected for the export".into(),
            ));
        }
        Some(names) => names.to_vec(),
        None => {
            let writable: Vec<String> = meta
                .columns
                .iter()
                .filter(|c| !(skip_generated && c.is_generated))
                .map(|c| c.name.clone())
                .collect();
            if writable.is_empty() {
                meta.columns.iter().map(|c| c.name.clone()).collect()
            } else {
                writable
            }
        }
    };
    let cols: Vec<&ColumnMeta> = names
        .iter()
        .map(|n| {
            meta.columns
                .iter()
                .find(|c| &c.name == n)
                .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{n}\"")))
        })
        .collect::<Result<_, _>>()?;
    let identity = if meta.primary_key.is_empty() {
        RowIdentity::None
    } else {
        RowIdentity::Pk
    };
    let limit = max_rows.map_or(u32::MAX, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let paged = offset > 0 || max_rows.is_some();
    // The key breaks the ties of the sort (without a sort the select orders by it already).
    let mut order: Vec<SortSpec> = sort.to_vec();
    if paged && !sort.is_empty() {
        for key in &meta.primary_key {
            if !order.iter().any(|s| &s.column == key) {
                order.push(SortSpec {
                    column: key.clone(),
                    dir: SortDir::Asc,
                    nulls: None,
                });
            }
        }
    }
    let built = sql::export_select(
        dialect,
        &SelectQuery {
            table: &meta.table,
            columns: &meta.columns,
            selected: Some(&names),
            filter,
            sort: &order,
            limit,
            offset,
            pk: &meta.primary_key,
            identity,
        },
        meta.kind,
    )?;

    let identity_always = matches!(dialect.resolve(), DialectRef::Core(Dialect::Postgres))
        && cols
            .iter()
            .any(|c| c.is_auto_increment && c.extra.as_deref() == Some("ALWAYS"));
    let columns = cols
        .into_iter()
        .map(|c| OutputColumn {
            as_text: sql::column_form(dialect, c) != ColumnForm::Native,
            ..OutputColumn::from(c)
        })
        .collect();
    Ok(ExportQuery {
        sql: built.sql,
        params: built.params,
        columns: Some(columns),
        blob_hex: built.blob_hex,
        identity_always,
    })
}

/// The statement of a query export: exactly one statement that returns rows, streamed as
/// written.
pub fn sql_query(sql: &str, dialect: impl Into<SqlDialect>) -> Result<ExportQuery, AppError> {
    let statements = sql::split_statements(dialect, sql);
    match statements.as_slice() {
        [] => Err(AppError::InvalidInput(
            "there is no SQL statement to export".into(),
        )),
        [one] if is_meta_command(&one.sql) => Err(AppError::InvalidInput(
            "psql meta-commands cannot be exported".into(),
        )),
        [one] => {
            check_returns_rows(&one.sql)?;
            Ok(ExportQuery {
                sql: one.sql.clone(),
                params: Vec::new(),
                columns: None,
                blob_hex: Vec::new(),
                identity_always: false,
            })
        }
        many => Err(AppError::InvalidInput(format!(
            "an export runs a single statement, but the SQL contains {}",
            many.len()
        ))),
    }
}

/// Refuses, before anything runs, a statement that cannot return rows. The export borrows a
/// pooled worker connection: `BEGIN`, `SET`, `LOCK`, `LISTEN`, `PRAGMA x = …`, `ATTACH`… would
/// stay in effect there for later browsing, and a `DELETE` would run in full only to report that
/// there was nothing to export. Allowed: queries (`SELECT`, `WITH`, `VALUES`, `TABLE`, `SHOW`,
/// `EXPLAIN`, `DESCRIBE`), `CALL` (MySQL procedures return result sets) and DML with
/// `RETURNING`. SQLite pragmas are read through their table-valued functions instead
/// (`SELECT * FROM pragma_table_info('t')`).
fn check_returns_rows(sql: &str) -> Result<(), AppError> {
    let first = leading_keywords(sql, 1)
        .into_iter()
        .next()
        .unwrap_or_default();
    let allowed = match first.as_str() {
        "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "EXPLAIN" | "DESCRIBE" | "DESC"
        | "CALL" => true,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" => {
            contains_word(&sql.to_uppercase(), "RETURNING")
        }
        _ => false,
    };
    if allowed {
        return Ok(());
    }
    let what = if first.is_empty() {
        "this statement".to_string()
    } else {
        format!("a {first} statement")
    };
    let hint = if first == "PRAGMA" {
        "; read a pragma with SELECT * FROM pragma_<name>(…) instead"
    } else {
        ""
    };
    Err(AppError::InvalidInput(format!(
        "an export needs a statement that returns rows (SELECT, WITH, VALUES, SHOW, EXPLAIN, \
         CALL or DML with RETURNING); {what} cannot be exported{hint}"
    )))
}

/// Checks the rows of a rows export before anything is written: every row has one value per
/// column, and no binary value is a capped preview (the editor holds at most
/// `BYTES_PREVIEW_CAP` bytes of each; writing that would silently cut the value). The error
/// names what can export the value in full: a query export decodes PostgreSQL `bytea` in full,
/// while MySQL and SQLite drivers cap blobs in queries too, so there only a table export can.
pub(crate) fn check_rows(
    columns: &[ResultColumn],
    rows: &[Vec<Value>],
    dialect: impl Into<SqlDialect>,
) -> Result<(), AppError> {
    let dialect = dialect.into();
    if columns.is_empty() {
        return Err(AppError::InvalidInput(
            "the result to export has no columns".into(),
        ));
    }
    for (i, row) in rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(AppError::InvalidInput(format!(
                "row {} of the result has {} values for {} columns",
                i + 1,
                row.len(),
                columns.len()
            )));
        }
        if row.iter().any(|v| truncated_len(v).is_some()) {
            let instead = match dialect.resolve() {
                DialectRef::Core(Dialect::Postgres) => "; export the query instead",
                DialectRef::Core(Dialect::Mysql | Dialect::Sqlite) => "; export its table instead",
                // Its driver decides how blobs arrive: nothing to point to.
                DialectRef::Ext(_) => "",
            };
            return Err(AppError::InvalidInput(format!(
                "a binary value in this result was truncated to {} KiB{instead}",
                BYTES_PREVIEW_CAP / 1024
            )));
        }
    }
    Ok(())
}

/// Hands a rows export's rows to the sink in `PROGRESS_ROWS` chunks, as a stream would. Yields
/// between chunks so a cancel request is seen; stops when the sink does.
async fn feed_rows(sink: &mut dyn RowSink, columns: &[ResultColumn], rows: &[Vec<Value>]) {
    sink.begin_set(columns.to_vec());
    for chunk in rows.chunks(PROGRESS_ROWS as usize) {
        if sink.rows(chunk.to_vec()).is_break() {
            return;
        }
        tokio::task::yield_now().await;
    }
}

/// Where a planned export reads its rows from.
enum Plan<'r> {
    Query(ExportQuery),
    Rows {
        columns: &'r [ResultColumn],
        rows: &'r [Vec<Value>],
    },
}

fn full_bytes(bytes: &[u8]) -> Value {
    Value::Bytes(BytesValue {
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        len: bytes.len() as u64,
        truncated: false,
    })
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, AppError> {
    let digits = hex.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(AppError::internal("odd-length hex text for a blob"));
    }
    digits
        .chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => Ok((h * 16 + l) as u8),
                _ => Err(AppError::internal("invalid hex text for a blob")),
            }
        })
        .collect()
}

/// Length of the first binary value capped at `BYTES_PREVIEW_CAP` (also inside arrays).
fn truncated_len(v: &Value) -> Option<u64> {
    match v {
        Value::Bytes(b) if b.truncated => Some(b.len),
        Value::Array(items) => items.iter().find_map(truncated_len),
        _ => None,
    }
}

/// Receives the streamed rows and writes them; decides when to stop.
struct ExportSink<'a, O: RowOut> {
    writer: O,
    dialect: SqlDialect,
    planned: Option<Vec<OutputColumn>>,
    columns: Vec<OutputColumn>,
    /// MySQL: indexes of single-precision `FLOAT` columns (see `narrow_float`).
    mysql_floats: Vec<usize>,
    blob_hex: Vec<usize>,
    /// PostgreSQL query and table exports deliver text cells, decoded here by column type; the
    /// rows of a rows export are values already.
    pg_text: bool,
    max_rows: Option<u64>,
    cancel: CancellationToken,
    /// Gets the rows written so far.
    progress: &'a mut (dyn FnMut(u64) + Send),
    rows: u64,
    sets: u32,
    cancelled: bool,
    limit_reached: bool,
    error: Option<AppError>,
    last_emit: Instant,
    rows_at_emit: u64,
}

impl<O: RowOut> ExportSink<'_, O> {
    fn prepare(&self, mut row: Vec<Value>) -> Result<Vec<Value>, AppError> {
        let n = self.columns.len();
        if !self.blob_hex.is_empty() && row.len() == n + self.blob_hex.len() {
            let extras = row.split_off(n);
            for (extra, &col) in extras.into_iter().zip(&self.blob_hex) {
                match extra {
                    Value::Null => {}
                    Value::Text(hex) => row[col] = full_bytes(&decode_hex(&hex)?),
                    other => {
                        return Err(AppError::internal(format!(
                            "unexpected blob hex cell {other:?}"
                        )));
                    }
                }
            }
        }
        if self.pg_text {
            // PostgreSQL table exports select text; decode it by the column's type.
            for (v, col) in row.iter_mut().zip(&self.columns) {
                if col.kind == ValueKind::Text {
                    continue;
                }
                if let Value::Text(text) = v {
                    *v = col.decode_pg_text(text);
                }
            }
        } else {
            // The columns an extension dialect projects as text, decoded as the grid does.
            for (v, col) in row.iter_mut().zip(&self.columns) {
                if col.as_text
                    && col.kind != ValueKind::Text
                    && let Value::Text(text) = v
                {
                    *v = text_to_value(col.kind, &col.data_type, text);
                }
            }
        }
        for &i in &self.mysql_floats {
            if let Some(Value::Float(f)) = row.get_mut(i) {
                *f = narrow_float(*f);
            }
        }
        for (v, col) in row.iter().zip(&self.columns) {
            if let Some(len) = truncated_len(v) {
                let (name, cap) = (&col.name, BYTES_PREVIEW_CAP);
                return Err(AppError::Unsupported(if self.planned.is_some() {
                    format!(
                        "column \"{name}\" holds a binary value of {len} bytes, more than the \
                         {cap} bytes the driver delivers in full"
                    )
                } else {
                    format!(
                        "column \"{name}\" holds a binary value of {len} bytes, more than the \
                         {cap} bytes a query export can read in full; export the table instead"
                    )
                }));
            }
        }
        Ok(row)
    }

    fn write(&mut self, row: Vec<Value>) -> Result<(), AppError> {
        let row = self.prepare(row)?;
        self.writer.write_row(&row)
    }

    fn emit_progress(&mut self) {
        (self.progress)(self.rows);
        self.last_emit = Instant::now();
        self.rows_at_emit = self.rows;
    }
}

impl<O: RowOut + Send> RowSink for ExportSink<'_, O> {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.sets += 1;
        // Only the first result set is exported (MySQL `CALL` can return several).
        if self.sets > 1 || self.error.is_some() {
            return;
        }
        let cols = match &self.planned {
            Some(planned) => {
                if columns.len() != planned.len() + self.blob_hex.len() {
                    self.error = Some(AppError::internal(format!(
                        "the export query returned {} columns, expected {}",
                        columns.len(),
                        planned.len() + self.blob_hex.len()
                    )));
                    return;
                }
                planned.clone()
            }
            None => columns.into_iter().map(OutputColumn::from).collect(),
        };
        let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
        if matches!(self.dialect.resolve(), DialectRef::Core(Dialect::Mysql)) {
            self.mysql_floats = cols
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_mysql_float())
                .map(|(i, _)| i)
                .collect();
        }
        self.columns = cols;
        if let Err(e) = self.writer.begin(&names) {
            self.error = Some(e);
        }
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        if self.error.is_some() || self.limit_reached || self.sets > 1 {
            return ControlFlow::Break(());
        }
        if self.cancel.is_cancelled() {
            self.cancelled = true;
            return ControlFlow::Break(());
        }
        for row in rows {
            if self.max_rows.is_some_and(|max| self.rows >= max) {
                self.limit_reached = true;
                break;
            }
            if let Err(e) = self.write(row) {
                self.error = Some(e);
                return ControlFlow::Break(());
            }
            self.rows += 1;
        }
        if self.max_rows.is_some_and(|max| self.rows >= max) {
            self.limit_reached = true;
        }
        if self.last_emit.elapsed() >= PROGRESS_INTERVAL
            || self.rows - self.rows_at_emit >= PROGRESS_ROWS
        {
            self.emit_progress();
        }
        if self.limit_reached {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// Streams an export's statement from a worker connection into `sink`. Once `cancel` fires, a
/// statement that keeps running is cancelled on the server.
async fn stream_export(
    session: &dyn DbSession,
    query: &ExportQuery,
    sink: &mut dyn RowSink,
    cancel: &CancellationToken,
    export_id: &str,
) -> Result<u64, AppError> {
    let slot = CancelSlot::default();
    let stream = session.stream_query(&query.sql, &query.params, sink, slot.clone());
    let watch = async {
        cancel.cancelled().await;
        // Usually the stream has ended by now: the sink stopped it and the driver cancelled a
        // read-only statement itself. A statement that is still running (it delivers nothing,
        // or the driver drains it because `cancel_safe_on_stop` refused it) is aborted on the
        // server: the user asked to stop, as in the SQL editor.
        tokio::time::sleep(CANCEL_GRACE).await;
        if let Some(handle) = request_cancel(&slot)
            && let Err(e) = session.cancel(handle).await
        {
            log::warn!("export {export_id}: cancel failed: {e}");
        }
        std::future::pending::<std::convert::Infallible>().await
    };
    tokio::select! {
        result = stream => result,
        never = watch => match never {},
    }
}

/// Streams a planned export into `out` (the statement's rows, or the rows held) and returns
/// the rows written. `progress` gets the rows written so far, throttled. Fails with `Cancelled`
/// once `cancel` fires.
#[allow(clippy::too_many_arguments)]
async fn stream_plan<O: RowOut + Send>(
    session: &dyn DbSession,
    dialect: SqlDialect,
    plan: &Plan<'_>,
    out: O,
    max_rows: Option<u64>,
    cancel: &CancellationToken,
    progress: &mut (dyn FnMut(u64) + Send),
    export_id: &str,
) -> Result<u64, AppError> {
    let (planned, blob_hex) = match plan {
        Plan::Query(query) => (query.columns.clone(), query.blob_hex.clone()),
        Plan::Rows { .. } => (None, Vec::new()),
    };
    let mut sink = ExportSink {
        writer: out,
        dialect,
        planned,
        columns: Vec::new(),
        mysql_floats: Vec::new(),
        blob_hex,
        pg_text: matches!(dialect.resolve(), DialectRef::Core(Dialect::Postgres))
            && matches!(plan, Plan::Query(_)),
        max_rows,
        cancel: cancel.clone(),
        progress,
        rows: 0,
        sets: 0,
        cancelled: false,
        limit_reached: max_rows == Some(0),
        error: None,
        last_emit: Instant::now(),
        rows_at_emit: 0,
    };
    let streamed = match plan {
        Plan::Rows { columns, rows } => {
            feed_rows(&mut sink, columns, rows).await;
            Ok(rows.len() as u64)
        }
        Plan::Query(query) => stream_export(session, query, &mut sink, cancel, export_id).await,
    };
    let rows = sink.rows;
    let sets = sink.sets;
    let cancelled = sink.cancelled;
    let limit_reached = sink.limit_reached;
    let error = sink.error.take();
    drop(sink);

    if cancelled || cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if let Some(e) = error {
        return Err(e);
    }
    match streamed {
        Ok(_) => {}
        // Stopping at `max_rows` may surface as a cancelled statement on some servers.
        Err(AppError::Cancelled) if limit_reached => {}
        Err(e) => return Err(e),
    }
    if sets == 0 {
        return Err(AppError::InvalidInput(
            "the statement returned no result set to export".into(),
        ));
    }
    Ok(rows)
}

/// The writer chain of an export file: counted, buffered, written off the async workers.
pub type FileOut = CountingWriter<BufWriter<BlockingWrite<std::fs::File>>>;

fn file_out(file: std::fs::File, bytes: Arc<AtomicU64>) -> FileOut {
    CountingWriter::new(
        BufWriter::with_capacity(FILE_BUFFER, BlockingWrite::new(file)),
        bytes,
    )
}

/// Flushes the chain and syncs the file to disk (blocking). The last flush and the fsync
/// (F_FULLFSYNC on macOS: the whole file plus the drive cache) take a while on a slow or network
/// disk: call it off the async workers.
fn sync_file(out: FileOut) -> Result<(), AppError> {
    let file = out
        .into_inner()
        .into_inner()
        .map_err(|e| AppError::Io(e.error().to_string()))?
        .into_inner();
    file.sync_all()?;
    Ok(())
}

async fn blocking<T: Send + 'static>(
    what: &str,
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("export {what} task failed: {e}")))?
}

/// Whether INSERTs read geographic WKT back with `axis-order=long-lat`: MySQL 8 and later (not
/// 5.7, not MariaDB, which take no such option) read a geographic SRS's WKT latitude first, while
/// the spatial decoder writes MySQL's storage order (longitude first). Asked once per export.
pub(crate) async fn mysql_long_lat(session: &dyn DbSession, dialect: Dialect) -> bool {
    if dialect != Dialect::Mysql {
        return false;
    }
    match session.server_info().await {
        Ok(info) => crate::backup::version::parse_server_version(&info.version).is_some_and(|v| {
            v.flavor == crate::types::backup::VersionFlavor::Mysql && v.major >= 8
        }),
        Err(e) => {
            log::debug!("export: server version unknown ({e}); spatial WKT without axis order");
            false
        }
    }
}

/// The session time zone the exported literals are read in, for the script's `SET TIME ZONE`
/// (see `time_zone_header`): MySQL's `@@session.time_zone`, as an offset (`+07:00`) when it is
/// `SYSTEM`; PostgreSQL's `TimeZone` setting. `None` for SQLite (no session zone) and when the
/// server does not say (the script then assumes the dialect's default zone).
pub(crate) async fn session_time_zone(session: &dyn DbSession, dialect: Dialect) -> Option<String> {
    let read = match dialect {
        Dialect::Sqlite => return None,
        Dialect::Postgres => read_pg_zone(session).await,
        Dialect::Mysql => read_mysql_zone(session).await,
    };
    match read {
        Ok(zone) => Some(zone),
        Err(e) => {
            log::debug!(
                "export: session time zone unknown ({e}); the script assumes {}",
                default_time_zone(dialect)
            );
            None
        }
    }
}

async fn read_pg_zone(session: &dyn DbSession) -> Result<String, AppError> {
    let rs = session
        .query("SELECT current_setting('TimeZone')::text", &[])
        .await?;
    rs.rows
        .first()
        .and_then(|r| r.first())
        .and_then(Value::as_text)
        .filter(|z| !z.trim().is_empty())
        .ok_or_else(|| AppError::internal("no TimeZone setting"))
}

async fn read_mysql_zone(session: &dyn DbSession) -> Result<String, AppError> {
    let rs = session
        .query(
            "SELECT @@session.time_zone, CAST(TIMEDIFF(NOW(), UTC_TIMESTAMP()) AS CHAR)",
            &[],
        )
        .await?;
    let row = rs
        .rows
        .first()
        .ok_or_else(|| AppError::internal("no time_zone row"))?;
    let zone = row
        .first()
        .and_then(Value::as_text)
        .filter(|z| !z.trim().is_empty())
        .ok_or_else(|| AppError::internal("no time_zone setting"))?;
    if zone.eq_ignore_ascii_case("SYSTEM") {
        // The server's own zone: the difference between its clock and UTC.
        return row
            .get(1)
            .and_then(Value::as_text)
            .and_then(|d| offset_from_timediff(&d))
            .ok_or_else(|| AppError::internal("unreadable system time zone offset"));
    }
    Ok(zone)
}

/// `TIMEDIFF` text (`07:00:00`, `-05:30:00`) as a MySQL zone offset (`+07:00`, `-05:30`).
pub(crate) fn offset_from_timediff(diff: &str) -> Option<String> {
    let diff = diff.trim();
    let (sign, rest) = match diff.strip_prefix('-') {
        Some(rest) => ('-', rest),
        None => ('+', diff.strip_prefix('+').unwrap_or(diff)),
    };
    let mut parts = rest.split(':');
    let hours: u32 = parts.next()?.parse().ok()?;
    let minutes: u32 = parts.next()?.parse().ok()?;
    (minutes < 60 && hours <= 14).then(|| format!("{sign}{hours:02}:{minutes:02}"))
}

/// The characters written as `?`, when there were any.
fn replaced_count(replaced: &AtomicU64) -> Option<u64> {
    Some(replaced.load(Ordering::Relaxed)).filter(|&n| n > 0)
}

/// Views have no table structure to write (TablePlus does not export view bodies either).
fn is_view(kind: TableKind) -> bool {
    matches!(
        kind,
        TableKind::View | TableKind::MaterializedView | TableKind::SystemView
    )
}

/// The dialect an export of `format` reads with: every export needs Export Tables (and reads
/// through the `sqlgen::sql` facade, so an extension dialect can), and a SQL script Export SQL
/// and a core dialect besides (the same for one table, several or a result).
pub(crate) fn export_dialect(
    session: &dyn DbSession,
    format: &ExportFormat,
) -> Result<SqlDialect, AppError> {
    match format {
        ExportFormat::SqlInsert { .. } => {
            session.require(Feature::ExportTables)?;
            session
                .core_dialect(Feature::ExportSql)
                .map(SqlDialect::from)
        }
        ExportFormat::Csv { .. }
        | ExportFormat::Json { .. }
        | ExportFormat::Xlsx { .. }
        | ExportFormat::Html => session.facade_dialect(Feature::ExportTables),
    }
}

/// A query export (`ExportSource::Sql`) runs the user's statement as written, with no classifier
/// in between, so it needs a core session besides `export_dialect`: there the server refuses
/// writes on a read-only connection itself. An extension dialect exports its tables and the rows
/// a result holds only (on Redshift, read-only by the classifier alone, `SELECT … INTO` or
/// `CALL` would otherwise write).
pub(crate) fn require_query_export(session: &dyn DbSession) -> Result<(), AppError> {
    let dialect = session.dialect();
    match dialect.resolve() {
        DialectRef::Core(_) => session.core_dialect(Feature::ExportTables).map(drop),
        DialectRef::Ext(_) => {
            session.require(Feature::ExportTables)?;
            Err(AppError::Unsupported(format!(
                "{}: query exports run in PostgreSQL, MySQL and SQLite only; export the result \
                 or the table instead",
                dialect.label()
            )))
        }
    }
}

/// The core dialect a SQL script is written in: `export_dialect` grants a script only there.
pub(crate) fn script_dialect(dialect: SqlDialect) -> Result<Dialect, AppError> {
    match dialect.resolve() {
        DialectRef::Core(core) => Ok(core),
        DialectRef::Ext(_) => Err(AppError::Unsupported(format!(
            "{}: SQL exports are written in PostgreSQL, MySQL and SQLite only",
            dialect.label()
        ))),
    }
}

/// Runs one export on `session`: plans the statement, streams it into the chosen format on a
/// temporary file, then moves the file into place. `meta` is the table's metadata for table
/// exports (fetched when `None`). Cancelling `cancel` stops the export with
/// `AppError::Cancelled` and leaves no file behind.
pub async fn run_export(
    session: &dyn DbSession,
    req: &ExportRequest,
    meta: Option<&TableMeta>,
    mut progress: impl FnMut(ExportProgress) + Send,
    cancel: CancellationToken,
) -> Result<ExportResult, AppError> {
    let started = Instant::now();
    let dialect = export_dialect(session, &req.format)?;
    if let ExportSource::Sql { .. } = req.source {
        require_query_export(session)?;
    }
    let dest = validate_target(&req.path)?;
    let fetched;
    let table_meta: Option<&TableMeta> = match (&req.source, meta) {
        (ExportSource::Table { .. }, Some(m)) => Some(m),
        (ExportSource::Table { table, .. }, None) => {
            fetched = session.table_meta(table).await?;
            Some(&fetched)
        }
        _ => None,
    };
    let (plan, default_table) = match &req.source {
        ExportSource::Table {
            table,
            filter,
            sort,
            columns,
            offset,
        } => {
            let meta = table_meta.ok_or_else(|| AppError::internal("table metadata missing"))?;
            let query = table_query(
                dialect,
                meta,
                columns.as_deref(),
                filter.as_ref(),
                sort,
                req.max_rows,
                offset.unwrap_or(0),
                matches!(req.format, ExportFormat::SqlInsert { .. }),
            )?;
            (Plan::Query(query), Some(table.name.as_str()))
        }
        ExportSource::Sql { sql } => (Plan::Query(sql_query(sql, dialect)?), None),
        ExportSource::Rows {
            columns,
            rows,
            table,
        } => {
            check_rows(columns, rows, dialect)?;
            (Plan::Rows { columns, rows }, table.as_deref())
        }
    };
    let mut spec = FormatSpec::from_request(&req.format, dialect, default_table)?;
    let text_encoding = effective_encoding(&req.format, req.encoding);
    if let FormatSpec::Html { charset, .. } = &mut spec {
        *charset = html_charset(text_encoding);
    }
    let script = SqlScript::of(&req.format);
    // A SQL script is written in the core dialect `export_dialect` granted it.
    let core = script.map(|_| script_dialect(dialect)).transpose()?;
    let content = script.is_none_or(|s| s.content);
    // The INSERTs' timestamps are written as the exporting session read them: the script sets
    // that session time zone first (and puts the previous one back at its end).
    let zone = match core {
        Some(core) if content => session_time_zone(session, core).await,
        _ => None,
    };
    // Structure and DROP exist for table sources only (not views): the script then has the
    // layout of a multi-table one (rows between the tables and their keys and triggers).
    let mut creates_table = false;
    let layout = match (script.zip(core), table_meta) {
        (Some((s, core)), Some(meta)) if (s.drop_table || s.structure) && !is_view(meta.kind) => {
            let context = context_for(session, core, meta, s.structure, false).await;
            creates_table = true;
            Some(script_layout(
                core,
                &[ScriptTable { meta, context }],
                s,
                true,
                zone.as_deref(),
            ))
        }
        // Rows only: just the time zone statements around them.
        (Some((s, core)), _) => Some(script_layout(core, &[], s, false, zone.as_deref())),
        _ => None,
    };
    let (preamble, closing, footer) = match layout {
        Some(l) => (l.opening, l.closing, l.footer),
        None => (String::new(), Vec::new(), Vec::new()),
    };
    if !content && preamble.is_empty() {
        return Err(AppError::InvalidInput(
            "nothing to export: include the table structure or its content".into(),
        ));
    }
    let long_lat = match core {
        Some(core) => mysql_long_lat(session, core).await,
        None => false,
    };
    if let FormatSpec::Insert {
        dialect: core,
        overriding_system_value,
        table,
        mysql_long_lat,
        ..
    } = &mut spec
    {
        *mysql_long_lat = long_lat;
        if let Plan::Query(query) = &plan {
            *overriding_system_value = query.identity_always;
        }
        // The INSERTs go into the table the script creates.
        if creates_table && let Some(meta) = table_meta {
            *table = script_table(*core, &meta.table);
        }
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    match &plan {
        Plan::Query(query) => log::debug!(
            "export {} to {}: {}",
            req.export_id,
            dest.display(),
            query.sql
        ),
        Plan::Rows { rows, .. } => log::debug!(
            "export {} to {}: {} rows held by the renderer",
            req.export_id,
            dest.display(),
            rows.len()
        ),
    }

    // Resolving links, checking the old file and sweeping leftovers touch the disk too.
    let (target, file) = {
        let dest = dest.clone();
        blocking("file", move || TempTarget::create(&dest)).await?
    };
    // Every early return below drops `target`, which deletes the temporary file.
    let bytes = Arc::new(AtomicU64::new(0));
    let replaced = Arc::new(AtomicU64::new(0));
    let mut out = Encoded::new(
        Compressed::new(
            file_out(file, bytes.clone()),
            script.is_some_and(|s| s.gzip),
        ),
        text_encoding,
        replaced.clone(),
    );
    out.write_all(preamble.as_bytes())?;
    let mut writer = RowWriter::new(spec, out);
    let rows = if content {
        let mut on_rows = |rows: u64| {
            progress(ExportProgress {
                rows,
                bytes: bytes.load(Ordering::Relaxed),
            })
        };
        stream_plan(
            session,
            dialect,
            &plan,
            &mut writer,
            req.max_rows,
            &cancel,
            &mut on_rows,
            &req.export_id,
        )
        .await?
    } else {
        0
    };
    blocking("finishing", move || -> Result<(), AppError> {
        let mut out = writer.finish()?;
        out.write_all(closing_text(&closing, rows > 0).as_bytes())?;
        for line in &footer {
            out.write_all(line.as_bytes())?;
            out.write_all(b"\n")?;
        }
        sync_file(out.finish()?.finish()?)?;
        target.commit()
    })
    .await?;

    let bytes = bytes.load(Ordering::Relaxed);
    progress(ExportProgress { rows, bytes });
    Ok(ExportResult {
        path: dest.to_string_lossy().into_owned(),
        rows,
        bytes,
        duration_ms: started.elapsed().as_millis() as u64,
        replaced: replaced_count(&replaced),
    })
}

#[cfg(test)]
mod tests;
