//! The CSV wizard's backend: the preview (resolved delimiter / encoding, first rows, detected
//! types) and the import itself.
//!
//! The import streams the file and inserts batched multi-row INSERTs with bind parameters
//! (`sqlgen::transfer`). Rows go in through `DbSession::transaction`, which takes its statements
//! at once: a file of up to `CHUNK_CELLS` values is imported in one transaction, so a failure
//! (STOP ON ERROR) rolls every row back. A larger file is committed in chunks of that size, as
//! TablePlus does: a failure rolls back its chunk, the chunks before it stay. A cancel stops
//! between chunks. "Create new table" creates the table in the first transaction (PostgreSQL,
//! SQLite) or just before it (MySQL, whose DDL commits; the table is dropped again when nothing
//! got imported).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::drivers::{DbSession, TxStatement};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::transfer::{
    InsertTarget, create_import_table, import_type_sql, mysql_warning_guard, mysql_warning_text,
    pg_sync_sequence,
};
use crate::types::import::{
    CsvColumn, CsvFormat, CsvImportFailure, CsvImportProgress, CsvImportReport, CsvImportRequest,
    CsvPreview, CsvSkippedRow, ImportPhase, ImportTarget, ImportType, InsertMode, TextEncoding,
};
use crate::types::schema::{TableMeta, TableRef, ValueKind};
use crate::types::value::Value;

use super::csvread::{CsvReader, Record};
use super::detect::{
    TypeGuess, created_value_problem, detect_delimiter, parse_bool, timestamp_in_utc,
};
use super::text::{CountingReader, Utf8Reader, detect_encoding, is_utf8_prefix};

/// Data rows the preview shows.
pub const PREVIEW_ROWS: usize = 100;
/// Data rows the column types are detected from.
pub const SAMPLE_ROWS: usize = 1000;
/// Bytes read to detect the encoding and the delimiter.
const SNIFF_BYTES: usize = 64 * 1024;
/// Values per transaction: a file with at most this many goes in as one transaction.
pub const CHUNK_CELLS: usize = 2_000_000;
/// Text bytes per INSERT statement (well below MySQL's default 64 MB packet).
const STATEMENT_BYTES: usize = 4 * 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const FILE_BUFFER: usize = 256 * 1024;

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn open_file(path: &str) -> Result<(File, u64), AppError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(AppError::InvalidInput(
            "the file path must be absolute".into(),
        ));
    }
    let file = File::open(p).map_err(|e| AppError::Io(format!("cannot open \"{path}\": {e}")))?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(AppError::InvalidInput(format!("\"{path}\" is a folder")));
    }
    Ok((file, meta.len()))
}

fn quote_byte(quote: &str) -> Result<u8, AppError> {
    match quote {
        "\"" => Ok(b'"'),
        "'" => Ok(b'\''),
        _ => Err(AppError::InvalidInput(
            "the CSV quote must be \" or '".into(),
        )),
    }
}

fn delimiter_byte(delimiter: &str, quote: u8) -> Result<u8, AppError> {
    let mut chars = delimiter.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii() && c as u8 != quote && !matches!(c, '\r' | '\n') => {
            Ok(c as u8)
        }
        _ => Err(AppError::InvalidInput(
            "the CSV delimiter must be one ASCII character other than the quote or a line break"
                .into(),
        )),
    }
}

/// The start of the file, for detection.
fn sniff(path: &str) -> Result<Vec<u8>, AppError> {
    let (file, _) = open_file(path)?;
    let mut sample = Vec::with_capacity(SNIFF_BYTES);
    file.take(SNIFF_BYTES as u64).read_to_end(&mut sample)?;
    Ok(sample)
}

/// Resolves the format's open choices (delimiter, encoding) from the start of the file.
pub fn resolve_format(path: &str, format: &CsvFormat) -> Result<(u8, u8, TextEncoding), AppError> {
    let quote = quote_byte(&format.quote)?;
    let sample = if format.encoding.is_none() || format.delimiter.is_none() {
        Some(sniff(path)?)
    } else {
        None
    };
    let encoding = match format.encoding {
        Some(e) => e,
        None => detect_encoding(sample.as_deref().unwrap_or_default()),
    };
    let delimiter = match &format.delimiter {
        Some(d) => delimiter_byte(d, quote)?,
        None => {
            let raw = sample.unwrap_or_default();
            let cut = raw.len() >= SNIFF_BYTES;
            let mut text = String::new();
            Utf8Reader::new(raw.as_slice(), encoding)
                .read_to_string(&mut text)
                .or_else(|_| {
                    // A cut or invalid UTF-8 sample: detect on what decodes.
                    text = String::from_utf8_lossy(&raw).into_owned();
                    Ok::<usize, std::io::Error>(0)
                })?;
            detect_delimiter(&text, quote, cut)
        }
    };
    Ok((delimiter, quote, encoding))
}

type FileReader = CsvReader<BufReader<Utf8Reader<CountingReader<File>>>>;

fn open_reader(
    path: &str,
    delimiter: u8,
    quote: u8,
    encoding: TextEncoding,
    bytes: Arc<AtomicU64>,
) -> Result<(FileReader, u64), AppError> {
    let (file, size) = open_file(path)?;
    let decoded = Utf8Reader::new(CountingReader::new(file, bytes), encoding);
    Ok((
        CsvReader::new(
            BufReader::with_capacity(FILE_BUFFER, decoded),
            delimiter,
            quote,
        ),
        size,
    ))
}

fn column_name(header: Option<&Record>, i: usize) -> String {
    header
        .and_then(|h| h.fields.get(i))
        .map(|f| f.text.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("column_{}", i + 1))
}

/// Reads the preview (blocking: call it off the async workers).
pub fn preview_csv(path: &str, format: &CsvFormat) -> Result<CsvPreview, AppError> {
    let (delimiter, quote, encoding) = resolve_format(path, format)?;
    if encoding == TextEncoding::Utf8 && format.encoding.is_some() {
        // An explicit UTF-8 on a file that is not: say so now rather than at some row.
        let sample = sniff(path)?;
        if !is_utf8_prefix(&sample) {
            return Err(AppError::InvalidInput(
                "the file is not valid UTF-8: choose its encoding".into(),
            ));
        }
    }
    let (mut reader, size) = open_reader(
        path,
        delimiter,
        quote,
        encoding,
        Arc::new(AtomicU64::new(0)),
    )?;
    let header = if format.header {
        reader.next_record()?
    } else {
        None
    };
    let mut sample: Vec<Record> = Vec::new();
    while sample.len() < SAMPLE_ROWS {
        match reader.next_record()? {
            Some(rec) => sample.push(rec),
            None => break,
        }
    }
    let more = sample.len() > PREVIEW_ROWS || reader.next_record()?.is_some();
    let width = sample
        .iter()
        .map(|r| r.fields.len())
        .chain(header.iter().map(|h| h.fields.len()))
        .max()
        .unwrap_or(0);
    let mut guesses = vec![TypeGuess::default(); width];
    for rec in &sample {
        for (g, f) in guesses.iter_mut().zip(&rec.fields) {
            g.add(&f.text);
        }
    }
    let columns = guesses
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let name = column_name(header.as_ref(), i);
            CsvColumn {
                detected: g.column(&name),
                name,
            }
        })
        .collect();
    let rows = sample
        .iter()
        .take(PREVIEW_ROWS)
        .map(|r| {
            r.fields
                .iter()
                .map(|f| {
                    if f.text.is_empty() && !f.quoted {
                        None
                    } else {
                        Some(f.text.clone())
                    }
                })
                .collect()
        })
        .collect();
    Ok(CsvPreview {
        path: path.to_string(),
        file_name: file_name(Path::new(path)),
        size,
        delimiter: (delimiter as char).to_string(),
        encoding,
        columns,
        rows,
        sampled_rows: sample.len() as u32,
        more,
    })
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

/// A table column the import fills.
#[derive(Debug, Clone, PartialEq)]
struct TargetColumn {
    name: String,
    /// PostgreSQL type of the column (its parameters are cast to the base type).
    pg_type: String,
    /// The column takes booleans (CSV spells them `yes`, `1`, `t`…; drivers bind real booleans).
    boolean: bool,
    /// "Create new table": the type the import creates the column with.
    created: Option<ImportType>,
}

/// Where each CSV column goes.
#[derive(Debug, Clone)]
struct Plan {
    dialect: Dialect,
    /// Per CSV column: target index.
    by_csv: Vec<Option<usize>>,
    columns: Vec<TargetColumn>,
    /// CREATE TABLE of "Create new table".
    create: Option<String>,
    table: TableRef,
    primary_key: Vec<String>,
    /// STOP ON ERROR: a row that cannot be imported stops the import (other modes skip it).
    stop_on_error: bool,
}

fn plan(
    dialect: Dialect,
    req: &CsvImportRequest,
    meta: Option<&TableMeta>,
) -> Result<Plan, AppError> {
    let (table, available, primary_key, create): (TableRef, Vec<TargetColumn>, _, _) =
        match &req.target {
            ImportTarget::Existing { table } => {
                let meta = meta.ok_or_else(|| AppError::NotFound(table.name.clone()))?;
                let cols = meta
                    .columns
                    .iter()
                    .filter(|c| !c.is_generated)
                    .map(|c| TargetColumn {
                        name: c.name.clone(),
                        pg_type: c.cast_type().to_string(),
                        boolean: c.kind == ValueKind::Bool,
                        created: None,
                    })
                    .collect();
                (meta.table.clone(), cols, meta.primary_key.clone(), None)
            }
            ImportTarget::Create { table, columns } => {
                let sql = create_import_table(dialect, table, columns)?;
                let cols = columns
                    .iter()
                    .map(|c| TargetColumn {
                        name: c.name.trim().to_string(),
                        pg_type: import_type_sql(Dialect::Postgres, c),
                        boolean: c.ty == ImportType::Boolean,
                        created: Some(c.ty),
                    })
                    .collect();
                (table.clone(), cols, Vec::new(), Some(sql))
            }
        };
    let mut by_csv = Vec::with_capacity(req.mapping.len());
    let mut columns: Vec<TargetColumn> = Vec::new();
    for target in &req.mapping {
        let Some(name) = target else {
            by_csv.push(None);
            continue;
        };
        if columns.iter().any(|c| &c.name == name) {
            return Err(AppError::InvalidInput(format!(
                "two CSV columns are imported into \"{name}\""
            )));
        }
        let col = available.iter().find(|c| &c.name == name).ok_or_else(|| {
            AppError::InvalidInput(format!("\"{name}\" is not a column the import can fill"))
        })?;
        by_csv.push(Some(columns.len()));
        columns.push(col.clone());
    }
    Ok(Plan {
        dialect,
        by_csv,
        columns,
        create,
        table,
        primary_key,
        stop_on_error: req.mode == InsertMode::StopOnError,
    })
}

/// The values of one record for the target columns, or why the record cannot be imported as it
/// is. `utc` counts the times converted to UTC (MySQL, new `timestamptz` columns).
fn row_values(
    rec: &Record,
    plan: &Plan,
    empty_as_null: bool,
    utc: &mut u64,
) -> Result<Vec<Value>, String> {
    let mut row = vec![Value::Null; plan.columns.len()];
    for (i, target) in plan.by_csv.iter().enumerate() {
        let Some(t) = *target else { continue };
        let Some(field) = rec.fields.get(i) else {
            continue; // past the end of every record of the header's width: NULL
        };
        let col = &plan.columns[t];
        row[t] = if field.text.is_empty() && !field.quoted && empty_as_null {
            Value::Null
        } else if col.boolean
            && let Some(b) = parse_bool(&field.text)
        {
            Value::Bool(b)
        } else {
            let mut text = field.text.clone();
            if let Some(ty) = col.created {
                if let Some(problem) = created_value_problem(ty, plan.dialect, &text) {
                    return Err(format!("column \"{}\": {problem}", col.name));
                }
                if ty == ImportType::TimestampTz
                    && plan.dialect == Dialect::Mysql
                    && let Some(in_utc) = timestamp_in_utc(&text)
                {
                    text = in_utc;
                    *utc += 1;
                }
            }
            Value::Text(text)
        };
    }
    Ok(row)
}

fn text_len(v: &Value) -> usize {
    match v {
        Value::Text(s) => s.len() + 8,
        _ => 8,
    }
}

/// `text` with its first letter in upper case (a reason as a message of its own).
fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Skipped rows a report lists.
const MAX_SKIPPED: usize = 100;

/// The file being imported, with what reading it has learned so far.
struct Source {
    reader: FileReader,
    /// The header line is still to be read.
    header_pending: bool,
    /// Fields every record has: the header's, else the first record's.
    width: Option<(usize, &'static str)>,
    /// Number of the next data row (1-based).
    next_row: u64,
}

/// A chunk of rows read from the file.
#[derive(Debug, Default)]
struct Chunk {
    rows: Vec<Vec<Value>>,
    /// Per row: its data row number (1-based) and the line it starts on.
    at: Vec<(u64, u64)>,
    eof: bool,
    /// Rows left out ("Replace" / "Update on duplicate"), the first `MAX_SKIPPED`.
    skipped: Vec<CsvSkippedRow>,
    skipped_count: u64,
    /// Times converted to UTC.
    utc: u64,
    /// STOP ON ERROR: the row that stops the import (the chunk is not imported).
    bad: Option<CsvSkippedRow>,
}

impl Source {
    /// Reads up to `chunk_cells` values (blocking).
    fn read_chunk(
        &mut self,
        plan: &Plan,
        empty_as_null: bool,
        chunk_cells: usize,
        cancel: &CancellationToken,
        read_count: &AtomicU64,
    ) -> Result<Chunk, AppError> {
        if std::mem::take(&mut self.header_pending)
            && let Some(header) = self.reader.next_record()?
        {
            self.width = Some((header.fields.len(), "the header has"));
        }
        let width = plan.columns.len().max(1);
        let budget = (chunk_cells / width).max(1);
        let mut chunk = Chunk::default();
        let mut records = 0usize;
        while chunk.rows.len() < budget {
            if records.is_multiple_of(1024) && cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            records += 1;
            let Some(rec) = self.reader.next_record()? else {
                chunk.eof = true;
                break;
            };
            let row = self.next_row;
            self.next_row += 1;
            read_count.fetch_add(1, Ordering::Relaxed);
            let (expected, whose) = *self
                .width
                .get_or_insert((rec.fields.len(), "the first row has"));
            let values = if rec.fields.len() == expected {
                row_values(&rec, plan, empty_as_null, &mut chunk.utc)
            } else {
                Err(format!(
                    "the row has {} field{}, {whose} {expected}",
                    rec.fields.len(),
                    if rec.fields.len() == 1 { "" } else { "s" }
                ))
            };
            match values {
                Ok(values) => {
                    chunk.rows.push(values);
                    chunk.at.push((row, rec.line));
                }
                Err(reason) => {
                    let skipped = CsvSkippedRow {
                        row,
                        line: rec.line,
                        reason,
                    };
                    if plan.stop_on_error {
                        chunk.bad = Some(skipped);
                        break;
                    }
                    chunk.skipped_count += 1;
                    if chunk.skipped.len() < MAX_SKIPPED {
                        chunk.skipped.push(skipped);
                    }
                }
            }
        }
        Ok(chunk)
    }
}

/// The rows of a statement: `start..end` in its chunk.
type RowRange = (usize, usize);

/// The chunk's statements and the rows each covers (`None`: no rows, e.g. CREATE TABLE). On
/// MySQL each INSERT runs between the statements of `mysql_warning_guard`, which cover its rows.
fn chunk_statements(
    target: &InsertTarget<'_>,
    rows: Vec<Vec<Value>>,
) -> Result<(Vec<TxStatement>, Vec<Option<RowRange>>), AppError> {
    let per = target.rows_per_statement();
    let guard = (target.dialect == Dialect::Mysql).then(mysql_warning_guard);
    let mut statements = Vec::new();
    let mut ranges = Vec::new();
    let mut batch: Vec<Vec<Value>> = Vec::new();
    let mut bytes = 0usize;
    let mut batch_start = 0usize;
    let total = rows.len();
    for (i, row) in rows.into_iter().enumerate() {
        bytes += row.iter().map(text_len).sum::<usize>();
        batch.push(row);
        if batch.len() >= per || bytes >= STATEMENT_BYTES || i + 1 == total {
            let built = target.build(std::mem::take(&mut batch))?;
            let range = Some((batch_start, i + 1));
            if let Some((before, _)) = &guard {
                statements.push(TxStatement::new(before.clone(), Vec::new()));
                ranges.push(range);
            }
            statements.push(TxStatement::new(built.sql, built.params));
            ranges.push(range);
            if let Some((_, after)) = &guard {
                for sql in after {
                    statements.push(TxStatement::new(sql.clone(), Vec::new()));
                    ranges.push(range);
                }
            }
            batch_start = i + 1;
            bytes = 0;
        }
    }
    Ok((statements, ranges))
}

/// SQLite reports no statement index: find the failing statement by replaying prefixes of the
/// transaction followed by a statement that always fails (every probe rolls back).
async fn sqlite_failing_statement(
    session: &dyn DbSession,
    statements: &[TxStatement],
) -> Option<usize> {
    let sentinel = format!(
        "SELECT * FROM \"_tabler_probe_{}\"",
        uuid::Uuid::new_v4().simple()
    );
    // Invariant: statements[..ok] run; statements[..bad] fail.
    let (mut ok, mut bad) = (0usize, statements.len());
    while bad - ok > 1 {
        let mid = (ok + bad) / 2;
        let mut probe: Vec<TxStatement> = statements[..mid].to_vec();
        probe.push(TxStatement::new(sentinel.clone(), Vec::new()));
        match session.transaction(probe).await {
            Err(e) if e.to_string().contains("_tabler_probe_") => ok = mid,
            Err(_) => bad = mid,
            // The sentinel cannot succeed; give up rather than guess.
            Ok(_) => return None,
        }
    }
    Some(ok)
}

/// Why a transaction failed, with the rows of the failing statement (`at`: the chunk's rows).
/// A MySQL value change caught by `mysql_warning_guard` names its own row.
fn failure(e: &AppError, range: Option<RowRange>, at: &[(u64, u64)]) -> CsvImportFailure {
    let detail = match e {
        AppError::Db { detail, .. } => detail.clone(),
        _ => None,
    };
    let span = |start: usize, end: usize| {
        let first = at.get(start)?;
        let last = at.get(end.checked_sub(1)?)?;
        Some((first.0, last.0, first.1))
    };
    let mut message = e.to_string();
    let mut rows = range.and_then(|(s, end)| span(s, end));
    if let Some((text, row)) = mysql_warning_text(&message) {
        let text = match text.rsplit_once(" at row ") {
            Some((head, _)) if row.is_some() => head.to_string(),
            _ => text,
        };
        message = format!("{text}: MySQL would store a changed value");
        if let (Some((start, _)), Some(n)) = (range, row)
            && let Some(one) = span(start + n as usize - 1, start + n as usize)
        {
            rows = Some(one);
        }
    }
    CsvImportFailure {
        message,
        detail,
        first_row: rows.map(|s| s.0),
        last_row: rows.map(|s| s.1),
        line: rows.map(|s| s.2),
    }
}

/// PostgreSQL: moves the sequences of the serial / identity columns the import wrote past the
/// values it wrote (`pg_sync_sequence`). Problems become notes: the rows are in.
async fn sync_sequences(
    session: &dyn DbSession,
    plan: &Plan,
    meta: Option<&TableMeta>,
    notes: &mut Vec<String>,
) {
    let Some(meta) = meta else { return };
    for col in meta.columns.iter().filter(|c| {
        c.is_auto_increment
            && matches!(c.kind, ValueKind::Int | ValueKind::BigInt)
            && plan.columns.iter().any(|t| t.name == c.name)
    }) {
        let sync = pg_sync_sequence(&plan.table, &col.name);
        if let Err(e) = session.execute(&sync.sql, &sync.params).await {
            log::warn!("import: cannot move the sequence of {}: {e}", col.name);
            notes.push(format!(
                "The sequence of \"{}\" could not be moved past the imported values ({e}): new \
                 rows may get values that already exist.",
                col.name
            ));
        }
    }
}

/// Runs a CSV import on `session`. `meta` is the target table's metadata (existing tables).
/// Errors before anything is read (bad request, unreadable file) are `Err`; failures and cancels
/// while importing are reported in the result, with what was committed.
pub async fn run_csv_import(
    session: &dyn DbSession,
    req: &CsvImportRequest,
    meta: Option<&TableMeta>,
    progress: impl FnMut(CsvImportProgress) + Send,
    cancel: CancellationToken,
) -> Result<CsvImportReport, AppError> {
    import_in_chunks(session, req, meta, progress, cancel, CHUNK_CELLS).await
}

/// `run_csv_import` with `chunk_cells` values per transaction.
async fn import_in_chunks(
    session: &dyn DbSession,
    req: &CsvImportRequest,
    meta: Option<&TableMeta>,
    mut progress: impl FnMut(CsvImportProgress) + Send,
    cancel: CancellationToken,
    chunk_cells: usize,
) -> Result<CsvImportReport, AppError> {
    let started = Instant::now();
    let dialect = session.dialect();
    let plan = plan(dialect, req, meta)?;
    let insert_columns: Vec<(String, String)> = plan
        .columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                if dialect == Dialect::Postgres {
                    c.pg_type.clone()
                } else {
                    String::new()
                },
            )
        })
        .collect();
    let target = InsertTarget {
        dialect,
        table: &plan.table,
        columns: &insert_columns,
        mode: req.mode,
        primary_key: &plan.primary_key,
    };
    target.validate()?;
    let (delimiter, quote, encoding) = resolve_format(&req.path, &req.format)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let read_count = Arc::new(AtomicU64::new(0));
    let (reader, total_bytes) = open_reader(&req.path, delimiter, quote, encoding, bytes.clone())?;
    let mut source = Some(Source {
        reader,
        header_pending: req.format.header,
        width: None,
        next_row: 1,
    });

    let mut report = CsvImportReport {
        rows_imported: 0,
        rows_read: 0,
        single_transaction: true,
        created_table: false,
        cancelled: false,
        failure: None,
        rows_skipped: 0,
        skipped: Vec::new(),
        notes: Vec::new(),
        duration_ms: 0,
    };
    let mut utc = 0u64;
    let emit = |progress: &mut dyn FnMut(CsvImportProgress), phase, imported| {
        progress(CsvImportProgress {
            phase,
            rows_read: read_count.load(Ordering::Relaxed),
            rows_imported: imported,
            bytes: bytes.load(Ordering::Relaxed),
            total_bytes,
        })
    };

    // MySQL commits DDL at once: create the table up front, drop it again if nothing lands.
    let mut create_in_tx = plan.create.clone();
    if dialect == Dialect::Mysql
        && let Some(sql) = create_in_tx.take()
    {
        session.execute(&sql, &[]).await?;
        report.created_table = true;
    }

    let mut first_chunk = true;
    loop {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let Some(mut src) = source.take() else { break };
        let task_plan = plan.clone();
        let task_cancel = cancel.clone();
        let task_count = read_count.clone();
        let empty_as_null = req.empty_as_null;
        let task = tokio::task::spawn_blocking(move || {
            let result = src.read_chunk(
                &task_plan,
                empty_as_null,
                chunk_cells,
                &task_cancel,
                &task_count,
            );
            (src, result)
        });
        tokio::pin!(task);
        let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
        let (src, chunk) = loop {
            tokio::select! {
                joined = &mut task => {
                    break joined.map_err(|e| AppError::internal(format!("import reader failed: {e}")))?;
                }
                _ = tick.tick() => emit(&mut progress, ImportPhase::Reading, report.rows_imported),
            }
        };
        let chunk = match chunk {
            Ok(c) => c,
            Err(AppError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(e) => {
                report.failure = Some(CsvImportFailure {
                    message: e.to_string(),
                    detail: None,
                    first_row: None,
                    last_row: None,
                    line: None,
                });
                break;
            }
        };
        source = Some(src);
        report.rows_skipped += chunk.skipped_count;
        let room = MAX_SKIPPED.saturating_sub(report.skipped.len());
        report.skipped.extend(chunk.skipped.into_iter().take(room));
        if let Some(bad) = chunk.bad {
            // STOP ON ERROR: nothing of this chunk goes in.
            report.failure = Some(CsvImportFailure {
                message: upper_first(&bad.reason),
                detail: None,
                first_row: Some(bad.row),
                last_row: Some(bad.row),
                line: Some(bad.line),
            });
            if !first_chunk {
                report.single_transaction = false;
            }
            break;
        }
        let eof = chunk.eof;
        if !(first_chunk && eof) {
            report.single_transaction = false;
        }
        let rows = chunk.rows.len() as u64;
        if rows == 0 && create_in_tx.is_none() {
            if eof {
                break;
            }
            first_chunk = false;
            continue;
        }
        let at = chunk.at;
        let (mut statements, mut ranges) = if rows > 0 {
            chunk_statements(&target, chunk.rows)?
        } else {
            (Vec::new(), Vec::new())
        };
        let created_here = create_in_tx.is_some();
        if let Some(sql) = create_in_tx.take() {
            statements.insert(0, TxStatement::new(sql, Vec::new()));
            ranges.insert(0, None);
        }
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        emit(&mut progress, ImportPhase::Writing, report.rows_imported);
        // SQLite reports no statement index: keep the statements to find the failing one.
        let kept = (dialect == Dialect::Sqlite).then(|| statements.clone());
        let attempt = session.transaction(statements).await;
        match attempt {
            Ok(_) => {
                report.rows_imported += rows;
                utc += chunk.utc;
                if created_here {
                    report.created_table = true;
                }
                emit(&mut progress, ImportPhase::Writing, report.rows_imported);
            }
            Err(e) => {
                let index = match &e {
                    AppError::Db {
                        statement_index: Some(i),
                        ..
                    } => Some(*i),
                    _ if dialect == Dialect::Sqlite => {
                        sqlite_failing_statement(session, kept.as_deref().unwrap_or_default()).await
                    }
                    _ => None,
                };
                let range = match index {
                    Some(i) => ranges.get(i).copied().flatten(),
                    // Unknown statement: every row of the chunk.
                    None if rows > 0 => Some((0, at.len())),
                    None => None,
                };
                report.failure = Some(failure(&e, range, &at));
                break;
            }
        }
        first_chunk = false;
        if eof {
            break;
        }
    }

    report.rows_read = read_count.load(Ordering::Relaxed);
    if report.rows_imported == 0
        && report.created_table
        && dialect == Dialect::Mysql
        && (report.failure.is_some() || report.cancelled)
    {
        let drop = format!(
            "DROP TABLE {}",
            dialect.qualify(plan.table.schema.as_deref(), &plan.table.name)
        );
        match session.execute(&drop, &[]).await {
            Ok(_) => report.created_table = false,
            Err(e) => log::warn!("import: cannot drop the table it created: {e}"),
        }
    }
    if dialect == Dialect::Postgres && report.rows_imported > 0 {
        sync_sequences(session, &plan, meta, &mut report.notes).await;
    }
    if utc > 0 {
        report.notes.push(format!(
            "{utc} time{} with a UTC offset {} converted to UTC: MySQL DATETIME keeps no offset.",
            if utc == 1 { "" } else { "s" },
            if utc == 1 { "was" } else { "were" }
        ));
    }
    emit(&mut progress, ImportPhase::Writing, report.rows_imported);
    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

#[cfg(test)]
pub(crate) async fn import_with_chunks(
    session: &dyn DbSession,
    req: &CsvImportRequest,
    meta: Option<&TableMeta>,
    chunk_cells: usize,
) -> Result<CsvImportReport, AppError> {
    import_in_chunks(
        session,
        req,
        meta,
        |_| {},
        CancellationToken::new(),
        chunk_cells,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::import::{ImportColumnDef, InsertMode};
    use std::io::Write;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn format(header: bool) -> CsvFormat {
        CsvFormat {
            delimiter: None,
            quote: "\"".into(),
            encoding: None,
            header,
        }
    }

    #[test]
    fn preview_detects_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("id;name;price;active\n");
        for i in 1..=150 {
            text.push_str(&format!("{i};\"Name {i}\";{i}.25;{}\n", i % 2 == 0));
        }
        text.push_str("151;;;\n");
        let path = write(dir.path(), "items.csv", text.as_bytes());
        let p = preview_csv(&path, &format(true)).unwrap();
        assert_eq!(p.file_name, "items.csv");
        assert_eq!(p.size, text.len() as u64);
        assert_eq!(p.delimiter, ";");
        assert_eq!(p.encoding, TextEncoding::Utf8);
        let names: Vec<&str> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "price", "active"]);
        let types: Vec<ImportType> = p.columns.iter().map(|c| c.detected.ty).collect();
        assert_eq!(
            types,
            [
                ImportType::Integer,
                ImportType::Text,
                ImportType::Numeric,
                ImportType::Boolean
            ]
        );
        assert_eq!(p.rows.len(), PREVIEW_ROWS);
        assert_eq!(p.sampled_rows, 151);
        assert!(p.more);
        assert_eq!(
            p.rows[0],
            vec![
                Some("1".into()),
                Some("Name 1".into()),
                Some("1.25".into()),
                Some("false".into())
            ]
        );

        // Without a header: generated names, the first line is data.
        let p = preview_csv(&path, &format(false)).unwrap();
        assert_eq!(p.columns[0].name, "column_1");
        assert_eq!(p.columns[0].detected.ty, ImportType::Text);
        assert_eq!(p.rows[0][0].as_deref(), Some("id"));

        // Windows-1252, detected; tab-delimited.
        let path = write(dir.path(), "latin.tsv", b"name\tcity\nZo\xEB\tM\xFCnchen\n");
        let p = preview_csv(&path, &format(true)).unwrap();
        assert_eq!(p.encoding, TextEncoding::Windows1252);
        assert_eq!(p.delimiter, "\t");
        assert_eq!(
            p.rows,
            vec![vec![Some("Zoë".into()), Some("München".into())]]
        );
        assert!(!p.more);
        // UTF-8 chosen for a file that is not.
        let mut utf8 = format(true);
        utf8.encoding = Some(TextEncoding::Utf8);
        assert!(preview_csv(&path, &utf8).is_err());

        // Empty unquoted fields are None, quoted ones Some("").
        let path = write(dir.path(), "e.csv", b"a,b\n,\"\"\n");
        let p = preview_csv(&path, &format(true)).unwrap();
        assert_eq!(p.rows, vec![vec![None, Some(String::new())]]);
        assert!(preview_csv("relative.csv", &format(true)).is_err());
    }

    fn request(path: &str, target: ImportTarget, mapping: &[Option<&str>]) -> CsvImportRequest {
        CsvImportRequest {
            import_id: "t".into(),
            path: path.into(),
            format: CsvFormat {
                delimiter: Some(",".into()),
                quote: "\"".into(),
                encoding: Some(TextEncoding::Utf8),
                header: true,
            },
            target,
            mapping: mapping.iter().map(|m| m.map(str::to_string)).collect(),
            empty_as_null: true,
            mode: InsertMode::StopOnError,
        }
    }

    #[test]
    fn plans_cast_domain_columns_to_their_base_type() {
        // `code` is a domain over varchar(3): cast to the domain, 'abcdef' would be cut to
        // 'abc'; cast to its base type, storing it fails with "value too long".
        let meta: TableMeta = serde_json::from_value(serde_json::json!({
            "table": { "schema": "public", "name": "t" },
            "kind": "table",
            "columns": [
                { "name": "code", "ordinal": 1, "dataType": "short_code", "kind": "text",
                  "nullable": true, "isPk": false, "isAutoIncrement": false,
                  "isGenerated": false, "isUnsigned": false,
                  "baseType": "character varying" },
                { "name": "n", "ordinal": 2, "dataType": "numeric(10,2)", "kind": "decimal",
                  "nullable": true, "isPk": false, "isAutoIncrement": false,
                  "isGenerated": false, "isUnsigned": false }
            ],
            "primaryKey": [], "indexes": [], "foreignKeys": [], "triggers": []
        }))
        .unwrap();
        let req = request(
            "/x.csv",
            ImportTarget::Existing {
                table: meta.table.clone(),
            },
            &[Some("code"), Some("n")],
        );
        let p = plan(Dialect::Postgres, &req, Some(&meta)).unwrap();
        assert_eq!(p.columns[0].pg_type, "character varying");
        assert_eq!(p.columns[1].pg_type, "numeric(10,2)");
    }

    #[test]
    fn plans_check_the_mapping() {
        let target = ImportTarget::Create {
            table: TableRef::new(None, "t"),
            columns: vec![
                ImportColumnDef {
                    name: "a".into(),
                    ty: ImportType::Integer,
                    precision: None,
                    scale: None,
                },
                ImportColumnDef {
                    name: "b".into(),
                    ty: ImportType::Boolean,
                    precision: None,
                    scale: None,
                },
            ],
        };
        let req = request("/x.csv", target.clone(), &[Some("b"), None, Some("a")]);
        let p = plan(Dialect::Postgres, &req, None).unwrap();
        assert_eq!(p.by_csv, vec![Some(0), None, Some(1)]);
        assert_eq!(
            p.columns[0],
            TargetColumn {
                name: "b".into(),
                pg_type: "boolean".into(),
                boolean: true,
                created: Some(ImportType::Boolean),
            }
        );
        assert!(p.stop_on_error);
        assert!(p.create.unwrap().starts_with("CREATE TABLE \"t\""));
        let dup = request("/x.csv", target.clone(), &[Some("a"), Some("a")]);
        assert!(plan(Dialect::Postgres, &dup, None).is_err());
        let unknown = request("/x.csv", target, &[Some("zzz")]);
        assert!(plan(Dialect::Postgres, &unknown, None).is_err());
        let existing = request(
            "/x.csv",
            ImportTarget::Existing {
                table: TableRef::new(None, "t"),
            },
            &[Some("a")],
        );
        assert!(plan(Dialect::Postgres, &existing, None).is_err());

        let rec = Record {
            fields: vec![
                crate::import::csvread::Field {
                    text: "yes".into(),
                    quoted: false,
                },
                crate::import::csvread::Field {
                    text: "skip".into(),
                    quoted: false,
                },
            ],
            line: 2,
        };
        let req = request(
            "/x.csv",
            ImportTarget::Create {
                table: TableRef::new(None, "t"),
                columns: vec![
                    ImportColumnDef {
                        name: "a".into(),
                        ty: ImportType::Integer,
                        precision: None,
                        scale: None,
                    },
                    ImportColumnDef {
                        name: "b".into(),
                        ty: ImportType::Boolean,
                        precision: None,
                        scale: None,
                    },
                ],
            },
            &[Some("b"), None, Some("a")],
        );
        let p = plan(Dialect::Sqlite, &req, None).unwrap();
        // A CSV column past the record's end leaves `a` NULL; `yes` becomes a boolean.
        assert_eq!(
            row_values(&rec, &p, true, &mut 0),
            Ok(vec![Value::Bool(true), Value::Null])
        );
    }

    fn created(name: &str, ty: ImportType) -> ImportColumnDef {
        ImportColumnDef {
            name: name.into(),
            ty,
            precision: None,
            scale: None,
        }
    }

    fn field(text: &str) -> crate::import::csvread::Field {
        crate::import::csvread::Field {
            text: text.into(),
            quoted: false,
        }
    }

    #[test]
    fn new_columns_refuse_values_they_would_change() {
        let req = request(
            "/x.csv",
            ImportTarget::Create {
                table: TableRef::new(None, "t"),
                columns: vec![
                    created("d", ImportType::Date),
                    created("ts", ImportType::Timestamp),
                    created("tz", ImportType::TimestampTz),
                ],
            },
            &[Some("d"), Some("ts"), Some("tz")],
        );
        let rec = |d: &str, ts: &str, tz: &str| Record {
            fields: vec![field(d), field(ts), field(tz)],
            line: 5,
        };
        let pg = plan(Dialect::Postgres, &req, None).unwrap();
        let mut utc = 0;
        assert!(
            row_values(
                &rec("2026-01-02", "2026-01-02 10:00", "x"),
                &pg,
                true,
                &mut utc
            )
            .is_ok()
        );
        // A time of day in a date column; an offset in a timestamp column (PostgreSQL drops it).
        let err = row_values(&rec("2026-01-02 10:20", "", ""), &pg, true, &mut utc).unwrap_err();
        assert!(
            err.contains("\"d\"") && err.contains("time of day"),
            "{err}"
        );
        let err = row_values(&rec("", "2026-01-02 10:20Z", ""), &pg, true, &mut utc).unwrap_err();
        assert!(err.contains("timestamptz"), "{err}");
        // Midnight is no time of day.
        assert!(row_values(&rec("2026-01-02 00:00:00", "", ""), &pg, true, &mut utc).is_ok());
        // PostgreSQL keeps offsets itself; MySQL gets the moment in UTC.
        assert_eq!(
            row_values(
                &rec("", "", "2026-01-02 03:20:30.5+07:00"),
                &pg,
                true,
                &mut utc
            )
            .unwrap()[2],
            Value::Text("2026-01-02 03:20:30.5+07:00".into())
        );
        assert_eq!(utc, 0);
        let my = plan(Dialect::Mysql, &req, None).unwrap();
        assert_eq!(
            row_values(
                &rec("", "", "2026-01-02 03:20:30.5+07:00"),
                &my,
                true,
                &mut utc
            )
            .unwrap()[2],
            Value::Text("2026-01-01 20:20:30.5".into())
        );
        assert_eq!(utc, 1);
        // MySQL converts an offset into a DATETIME itself; SQLite keeps the text.
        assert!(row_values(&rec("", "2026-01-02 10:20Z", ""), &my, true, &mut utc).is_ok());
        let lite = plan(Dialect::Sqlite, &req, None).unwrap();
        assert!(row_values(&rec("2026-01-02 10:20", "", ""), &lite, true, &mut utc).is_ok());
    }

    fn reader_over(text: &str) -> CsvReader<BufReader<Utf8Reader<CountingReader<File>>>> {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "r.csv", text.as_bytes());
        let (reader, _) = open_reader(
            &path,
            b',',
            b'"',
            TextEncoding::Utf8,
            Arc::new(AtomicU64::new(0)),
        )
        .unwrap();
        reader
    }

    #[test]
    fn rows_with_another_number_of_fields_are_row_errors() {
        let text = "a,b\n1,2\n3\n4,5,6\n7,8\n";
        let target = ImportTarget::Create {
            table: TableRef::new(None, "t"),
            columns: vec![
                created("a", ImportType::Text),
                created("b", ImportType::Text),
            ],
        };
        let mut req = request("/x.csv", target, &[Some("a"), Some("b"), None]);
        let count = AtomicU64::new(0);
        let read = |req: &CsvImportRequest, header: bool| {
            let mut src = Source {
                reader: reader_over(text),
                header_pending: header,
                width: None,
                next_row: 1,
            };
            let p = plan(Dialect::Sqlite, req, None).unwrap();
            src.read_chunk(&p, true, 1000, &CancellationToken::new(), &count)
                .unwrap()
        };
        // STOP ON ERROR stops at the first short row, naming it.
        let chunk = read(&req, true);
        let bad = chunk.bad.expect("a row error");
        assert_eq!((bad.row, bad.line), (2, 3));
        assert_eq!(bad.reason, "the row has 1 field, the header has 2");
        // Other modes skip and count them.
        req.mode = InsertMode::Replace;
        let chunk = read(&req, true);
        assert!(chunk.bad.is_none() && chunk.eof);
        assert_eq!(chunk.rows.len(), 2);
        assert_eq!(chunk.at, vec![(1, 2), (4, 5)]);
        assert_eq!(chunk.skipped_count, 2);
        assert_eq!(
            chunk
                .skipped
                .iter()
                .map(|s| (s.row, s.line))
                .collect::<Vec<_>>(),
            [(2, 3), (3, 4)]
        );
        assert!(chunk.skipped[1].reason.contains("3 fields"));
        // Without a header the first row sets the width.
        let chunk = read(&req, false);
        assert_eq!(chunk.rows.len(), 3);
        assert!(chunk.skipped[0].reason.ends_with("the first row has 2"));
    }
}
