//! Streaming data export (M6): a table (with the grid's filter / sort / column choice) or one
//! SQL statement, written to CSV, JSON / NDJSON or SQL `INSERT` statements.
//!
//! Rows are streamed from a worker connection (`DbSession::stream_query`) straight into a
//! format writer on a buffered temporary file next to the destination; nothing holds the whole
//! result. The temporary file is renamed over the destination only when every row was written,
//! and removed on error or cancellation. Progress is reported at most every
//! `PROGRESS_INTERVAL` or `PROGRESS_ROWS` rows, whichever comes first.
//!
//! PostgreSQL table exports select every column as text (as browsing does) and decode it here
//! by the column's kind (arrays by their element type), so `bytea` arrives in full. MySQL and
//! SQLite drivers cap binary cells at `BYTES_PREVIEW_CAP`; table exports therefore also select
//! the hex text of larger blobs and restore them (on SQLite for every column, since any column
//! can hold a blob). A query export cannot do that: a capped binary cell fails the export
//! instead of silently writing a truncated value.
//!
//! A query export only runs statements that return rows (`sql_query`): the worker connection
//! it borrows goes back to the pool, so `BEGIN`, `SET`, `LOCK`… must never run there.
//!
//! A rows export (`ExportSource::Rows`: a result the SQL editor shows) runs nothing at all: the
//! renderer's rows go through the same sink, in chunks, so progress, `max_rows`, cancellation
//! and the temporary file behave exactly as for a query.

pub mod file;
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
use crate::sqlgen::Dialect;
use crate::sqlgen::select::{SelectSpec, build_select};
use crate::sqlgen::split::{is_meta_command, leading_keywords, split_statements};
use crate::sqlgen::transfer::script_table;
use crate::types::browse::{FilterGroup, RowIdentity, SortSpec};
use crate::types::export::{
    ExportFormat, ExportProgress, ExportRequest, ExportResult, ExportSource,
};
use crate::types::query::ResultColumn;
use crate::types::schema::{ColumnMeta, TableKind, TableMeta, ValueKind};
use crate::types::value::{BYTES_PREVIEW_CAP, BytesValue, Value};

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
}

impl From<&ColumnMeta> for OutputColumn {
    fn from(c: &ColumnMeta) -> Self {
        OutputColumn {
            name: c.name.clone(),
            kind: c.kind,
            data_type: c.data_type.clone(),
            array_elem: c.array_elem_type.clone(),
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
/// `LIMIT max_rows`). `skip_generated` leaves generated columns out when no explicit column
/// list is given (INSERT exports cannot write them).
pub fn table_query(
    dialect: Dialect,
    meta: &TableMeta,
    selected: Option<&[String]>,
    filter: Option<&FilterGroup>,
    sort: &[SortSpec],
    max_rows: Option<u64>,
    skip_generated: bool,
) -> Result<ExportQuery, AppError> {
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
    let built = build_select(&SelectSpec {
        dialect,
        table: &meta.table,
        columns: &meta.columns,
        selected: Some(&names),
        filter,
        sort,
        limit,
        offset: 0,
        pk: &meta.primary_key,
        identity,
    })?;
    let mut sql = built.sql;
    if limit == u32::MAX {
        // Unbounded: drop the paging clause (the sink enforces `max_rows` anyway).
        let paging = format!("\nLIMIT {} OFFSET 0", u32::MAX);
        if let Some(stripped) = sql.strip_suffix(&paging) {
            sql = stripped.to_string();
        }
    }

    let mut blob_hex = Vec::new();
    if dialect == Dialect::Postgres {
        // `"c"::text AS "c"` makes `ORDER BY "c"` bind to the text alias (PostgreSQL resolves
        // output names first), which sorts numbers and dates as strings. The file's column
        // names come from the metadata, so neutral aliases keep the sort on the real columns.
        let list = cols
            .iter()
            .map(|c| {
                let ident = dialect.quote_ident(&c.name);
                format!("{ident}::text AS {ident}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = format!("SELECT {list}\nFROM ");
        if let Some(rest) = sql.strip_prefix(&prefix) {
            let aliased = cols
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "{}::text AS {}",
                        dialect.quote_ident(&c.name),
                        dialect.quote_ident(&format!("_tabler_c{i}"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            sql = format!("SELECT {aliased}\nFROM {rest}");
        }
    } else {
        // MySQL types are strict: only binary columns hold blobs. In SQLite any column can
        // (whatever its declared type, or none); the `typeof` guard keeps the extra NULL
        // for everything else.
        let blobs: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| dialect == Dialect::Sqlite || c.kind == ValueKind::Bytes)
            .map(|(i, _)| i)
            .collect();
        let list = cols
            .iter()
            .map(|c| dialect.quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = format!("SELECT {list}\nFROM ");
        if !blobs.is_empty()
            && let Some(rest) = sql.strip_prefix(&prefix)
        {
            let extras: Vec<String> = blobs
                .iter()
                .enumerate()
                .map(|(j, &i)| large_blob_hex(dialect, &cols[i].name, j))
                .collect();
            sql = format!("SELECT {list}, {}\nFROM {rest}", extras.join(", "));
            blob_hex = blobs;
        }
    }

    let identity_always = dialect == Dialect::Postgres
        && cols
            .iter()
            .any(|c| c.is_auto_increment && c.extra.as_deref() == Some("ALWAYS"));

    Ok(ExportQuery {
        sql,
        params: built.params,
        columns: Some(cols.into_iter().map(OutputColumn::from).collect()),
        blob_hex,
        identity_always,
    })
}

/// Extra select-list item: the hex text of a blob the driver would deliver truncated.
fn large_blob_hex(dialect: Dialect, column: &str, n: usize) -> String {
    let c = dialect.quote_ident(column);
    let alias = dialect.quote_ident(&format!("_tabler_hex_{n}"));
    let cap = BYTES_PREVIEW_CAP;
    match dialect {
        Dialect::Mysql => format!("CASE WHEN LENGTH({c}) > {cap} THEN HEX({c}) END AS {alias}"),
        _ => format!(
            "CASE WHEN typeof({c}) = 'blob' AND length({c}) > {cap} THEN hex({c}) END AS {alias}"
        ),
    }
}

/// The statement of a query export: exactly one statement that returns rows, streamed as
/// written.
pub fn sql_query(sql: &str, dialect: Dialect) -> Result<ExportQuery, AppError> {
    let statements = split_statements(sql, dialect);
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
fn check_rows(
    columns: &[ResultColumn],
    rows: &[Vec<Value>],
    dialect: Dialect,
) -> Result<(), AppError> {
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
            let instead = match dialect {
                Dialect::Postgres => "the query",
                Dialect::Mysql | Dialect::Sqlite => "its table",
            };
            return Err(AppError::InvalidInput(format!(
                "a binary value in this result was truncated to {} KiB; export {instead} instead",
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
    dialect: Dialect,
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
        if self.dialect == Dialect::Mysql {
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
async fn stream_plan<O: RowOut + Send>(
    session: &dyn DbSession,
    plan: &Plan<'_>,
    out: O,
    max_rows: Option<u64>,
    cancel: &CancellationToken,
    progress: &mut (dyn FnMut(u64) + Send),
    export_id: &str,
) -> Result<u64, AppError> {
    let dialect = session.dialect();
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
        pg_text: dialect == Dialect::Postgres && matches!(plan, Plan::Query(_)),
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

/// Views have no table structure to write (TablePlus does not export view bodies either).
fn is_view(kind: TableKind) -> bool {
    matches!(
        kind,
        TableKind::View | TableKind::MaterializedView | TableKind::SystemView
    )
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
    let dialect = session.dialect();
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
        } => {
            let meta = table_meta.ok_or_else(|| AppError::internal("table metadata missing"))?;
            let query = table_query(
                dialect,
                meta,
                columns.as_deref(),
                filter.as_ref(),
                sort,
                req.max_rows,
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
    let script = SqlScript::of(&req.format);
    // Structure and DROP exist for table sources only (not views): the script then has the
    // layout of a multi-table one (rows between the tables and their keys and triggers).
    let layout = match (script, table_meta) {
        (Some(s), Some(meta)) if (s.drop_table || s.structure) && !is_view(meta.kind) => {
            let context = context_for(session, meta, s.structure, false).await;
            Some(script_layout(
                dialect,
                &[ScriptTable { meta, context }],
                s,
                true,
            ))
        }
        _ => None,
    };
    let (preamble, closing, footer) = match layout {
        Some(l) => (l.opening, l.closing, l.footer),
        None => (String::new(), Vec::new(), Vec::new()),
    };
    let content = script.is_none_or(|s| s.content);
    if !content && preamble.is_empty() {
        return Err(AppError::InvalidInput(
            "nothing to export: include the table structure or its content".into(),
        ));
    }
    if let FormatSpec::Insert {
        overriding_system_value,
        table,
        ..
    } = &mut spec
    {
        if let Plan::Query(query) = &plan {
            *overriding_system_value = query.identity_always;
        }
        // The INSERTs go into the table the script creates.
        if !preamble.is_empty()
            && let Some(meta) = table_meta
        {
            *table = script_table(dialect, &meta.table);
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
    let mut out = Compressed::new(
        file_out(file, bytes.clone()),
        script.is_some_and(|s| s.gzip),
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
        sync_file(out.finish()?)?;
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
    })
}

#[cfg(test)]
mod tests;
