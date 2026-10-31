//! Format writers: turn rows of `Value`s into CSV, JSON / NDJSON, SQL `INSERT` text, an HTML
//! table or an Excel sheet, incrementally, into any `io::Write`. Nothing here buffers more than
//! the current row.

use std::io::{self, Write};
use std::sync::LazyLock;

use regex::Regex;

use base64::Engine as _;

use crate::error::AppError;
use crate::sqlgen::select::literal;
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::export::{CsvQuote, ExportFormat};
use crate::types::value::Value;

use super::xlsx::XlsxWriter;

/// A validated `ExportFormat`, ready to build a writer from.
#[derive(Debug, Clone, PartialEq)]
pub enum FormatSpec {
    Csv(CsvSpec),
    Json {
        ndjson: bool,
        pretty: bool,
    },
    Insert {
        dialect: Dialect,
        /// Already quoted for `dialect`.
        table: String,
        batch_size: u32,
        /// PostgreSQL: write `OVERRIDING SYSTEM VALUE` (the rows carry values for an identity
        /// column declared `GENERATED ALWAYS`, which a plain INSERT may not set).
        overriding_system_value: bool,
        /// MySQL 8 or later (not MariaDB, not 5.7): geographic WKT is read back with
        /// `axis-order=long-lat`, the order the spatial decoder writes (MySQL's storage order).
        mysql_long_lat: bool,
    },
    Xlsx {
        header: bool,
        /// Name of the (single) sheet.
        sheet: String,
    },
    Html {
        /// The page's `<title>`.
        title: String,
        /// The `charset` the page declares (see `encoding::html_charset`).
        charset: &'static str,
    },
}

/// The CSV options, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvSpec {
    pub delimiter: u8,
    pub header: bool,
    /// `"`, `'` or a space.
    pub quote: u8,
    /// Quote every value, not only the ones that need it.
    pub always_quote: bool,
    pub null_as: String,
    /// `\n`, `\r\n` or `\r`.
    pub line_break: &'static [u8],
    /// The decimal separator of numbers (`.` writes them as they are).
    pub decimal: u8,
    /// Line breaks inside values become one space.
    pub line_break_to_space: bool,
}

impl CsvSpec {
    /// The defaults of the export dialog (and of TablePlus).
    pub fn standard(delimiter: u8) -> CsvSpec {
        CsvSpec {
            delimiter,
            header: true,
            quote: b'"',
            always_quote: false,
            null_as: String::new(),
            line_break: b"\n",
            decimal: b'.',
            line_break_to_space: false,
        }
    }
}

/// The SQL script options of an `SqlInsert` format: what precedes / makes up the INSERTs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SqlScript {
    pub drop_table: bool,
    pub structure: bool,
    pub content: bool,
    pub gzip: bool,
}

impl SqlScript {
    pub fn of(format: &ExportFormat) -> Option<SqlScript> {
        match format {
            ExportFormat::SqlInsert {
                drop_table,
                structure,
                content,
                gzip,
                ..
            } => Some(SqlScript {
                drop_table: *drop_table,
                structure: *structure,
                content: *content,
                gzip: *gzip,
            }),
            _ => None,
        }
    }
}

/// The INSERT target of a result with no known table.
pub const UNTITLED_TABLE: &str = "untitled";

/// A SQL script's INSERT statements end once they reach this many bytes (a row never splits, so a
/// statement can pass it by one row): a statement beyond the server's packet limit (MySQL's
/// `max_allowed_packet`) cannot be loaded back. The row-count batch stays the other bound.
pub const INSERT_CHUNK_BYTES: usize = 250 * 1024;

fn single_ascii(text: &str, what: &str, forbidden: &[char]) -> Result<u8, AppError> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii() && !forbidden.contains(&c) => Ok(c as u8),
        _ => Err(AppError::InvalidInput(format!(
            "the CSV {what} must be a single ASCII character other than a quote or a line break"
        ))),
    }
}

/// The CSV decimal separator: `.`, `,` or one custom ASCII character that is not a digit, a quote,
/// a line break or another control character, and differs from the delimiter (`,` is the one
/// exception: its numbers are quoted wherever the delimiter is a comma too).
fn decimal_byte(decimal: &str, delimiter: u8) -> Result<u8, AppError> {
    let mut chars = decimal.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None)
            if c.is_ascii()
                && !c.is_ascii_digit()
                && !c.is_ascii_control()
                && !matches!(c, '"' | '\'')
                && (c as u8 != delimiter || c == ',') =>
        {
            Ok(c as u8)
        }
        _ => Err(AppError::InvalidInput(
            "the CSV decimal separator must be a single ASCII character other than a digit, \
             the delimiter, a quote or a line break"
                .into(),
        )),
    }
}

impl FormatSpec {
    /// Validates the renderer's format options. `default_table` names the INSERT target when
    /// `table_name` is empty (table exports) and the Excel sheet; INSERTs are written in
    /// `dialect`, a core one.
    pub fn from_request(
        format: &ExportFormat,
        dialect: impl Into<SqlDialect>,
        default_table: Option<&str>,
    ) -> Result<FormatSpec, AppError> {
        Ok(match format {
            ExportFormat::Csv {
                delimiter,
                header,
                quote,
                null_as,
                line_break,
                decimal,
                line_break_to_space,
            } => {
                let delimiter = single_ascii(delimiter, "delimiter", &['"', '\'', '\r', '\n'])?;
                let (quote, always_quote) = match quote {
                    CsvQuote::Double => (b'"', true),
                    CsvQuote::Single => (b'\'', true),
                    CsvQuote::Space => (b' ', true),
                    CsvQuote::IfNeeded => (b'"', false),
                };
                if quote == b' ' && delimiter == b' ' {
                    return Err(AppError::InvalidInput(
                        "the CSV delimiter cannot be a space when values are quoted with spaces"
                            .into(),
                    ));
                }
                let line_break: &'static [u8] = match line_break.as_str() {
                    "\n" => b"\n",
                    "\r\n" => b"\r\n",
                    "\r" => b"\r",
                    _ => {
                        return Err(AppError::InvalidInput(
                            "the CSV line break must be \\n, \\r\\n or \\r".into(),
                        ));
                    }
                };
                let decimal = decimal_byte(decimal, delimiter)?;
                // As PostgreSQL's COPY requires: the NULL marker is written unquoted, so it must
                // not contain anything that would need quoting.
                if null_as
                    .bytes()
                    .any(|b| b == delimiter || b == quote || matches!(b, b'\r' | b'\n'))
                {
                    return Err(AppError::InvalidInput(
                        "the CSV NULL text must not contain the delimiter, a quote or a line break"
                            .into(),
                    ));
                }
                FormatSpec::Csv(CsvSpec {
                    delimiter,
                    header: *header,
                    quote,
                    always_quote,
                    null_as: null_as.clone(),
                    line_break,
                    decimal,
                    line_break_to_space: *line_break_to_space,
                })
            }
            ExportFormat::Json { ndjson, pretty } => FormatSpec::Json {
                ndjson: *ndjson,
                pretty: *pretty,
            },
            ExportFormat::SqlInsert {
                table_name,
                batch_size,
                ..
            } => {
                if *batch_size == 0 {
                    return Err(AppError::InvalidInput(
                        "the INSERT batch size must be at least 1".into(),
                    ));
                }
                // A result whose table is unknown (a join, a computed column, a query) is
                // written as `untitled`, as TablePlus does.
                let name = match table_name.trim() {
                    "" => default_table.unwrap_or(UNTITLED_TABLE),
                    _ => table_name.as_str(),
                };
                let dialect = super::script_dialect(dialect.into())?;
                FormatSpec::Insert {
                    dialect,
                    table: dialect.quote_ident(name),
                    batch_size: *batch_size,
                    overriding_system_value: false,
                    mysql_long_lat: false,
                }
            }
            ExportFormat::Xlsx { header } => FormatSpec::Xlsx {
                header: *header,
                sheet: default_table.unwrap_or("Result").to_string(),
            },
            ExportFormat::Html => FormatSpec::Html {
                title: default_table.unwrap_or("Result").to_string(),
                charset: "utf-8",
            },
        })
    }
}

fn io_err(e: impl std::fmt::Display) -> AppError {
    AppError::Io(e.to_string())
}

/// What the export sink writes rows into: a `RowWriter`, or a sheet of a multi-sheet workbook.
pub trait RowOut {
    /// Called once per result set with its column names, before its rows.
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError>;
    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError>;
}

impl<T: RowOut + ?Sized> RowOut for &mut T {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        (**self).begin(columns)
    }
    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        (**self).write_row(row)
    }
}

impl<W: Write> RowOut for XlsxWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        XlsxWriter::begin(self, columns)
    }
    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        XlsxWriter::write_row(self, row)
    }
}

/// Writes rows in one format. Call `begin` once with the column names, `write_row` per row and
/// `finish` at the end (also when no row or no result set arrived: JSON then writes `[]`).
pub enum RowWriter<W: Write> {
    Csv(Box<CsvWriter<W>>),
    Json(JsonWriter<W>),
    Insert(InsertWriter<W>),
    Xlsx(Box<XlsxWriter<W>>),
    Html(Box<HtmlWriter<W>>),
}

impl<W: Write> RowWriter<W> {
    pub fn new(spec: FormatSpec, out: W) -> RowWriter<W> {
        match spec {
            FormatSpec::Csv(spec) => RowWriter::Csv(Box::new(CsvWriter {
                w: out,
                spec,
                line: Vec::new(),
            })),
            FormatSpec::Json { ndjson, pretty } => RowWriter::Json(JsonWriter {
                w: out,
                ndjson,
                pretty: pretty && !ndjson,
                keys: Vec::new(),
                rows: 0,
            }),
            FormatSpec::Insert {
                dialect,
                table,
                batch_size,
                overriding_system_value,
                mysql_long_lat,
            } => RowWriter::Insert(InsertWriter {
                w: out,
                dialect,
                table,
                overriding_system_value,
                mysql_long_lat,
                prefix: String::new(),
                batch_size,
                in_batch: 0,
                stmt_bytes: 0,
            }),
            FormatSpec::Xlsx { header, sheet } => {
                let mut w = XlsxWriter::new(out, header);
                w.next_sheet(&sheet);
                RowWriter::Xlsx(Box::new(w))
            }
            FormatSpec::Html { title, charset } => {
                RowWriter::Html(Box::new(HtmlWriter::new(out, &title, charset)))
            }
        }
    }

    pub fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        match self {
            RowWriter::Csv(w) => w.begin(columns),
            RowWriter::Json(w) => w.begin(columns),
            RowWriter::Insert(w) => w.begin(columns),
            RowWriter::Xlsx(w) => w.begin(columns),
            RowWriter::Html(w) => w.begin(columns),
        }
    }

    pub fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        match self {
            RowWriter::Csv(w) => w.write_row(row).map_err(io_err),
            RowWriter::Json(w) => w.write_row(row).map_err(io_err),
            RowWriter::Insert(w) => w.write_row(row),
            RowWriter::Xlsx(w) => w.write_row(row),
            RowWriter::Html(w) => w.write_row(row).map_err(io_err),
        }
    }

    /// Writes the closing text, flushes and returns the output.
    pub fn finish(self) -> Result<W, AppError> {
        match self {
            RowWriter::Csv(w) => w.finish().map_err(io_err),
            RowWriter::Json(w) => w.finish().map_err(io_err),
            RowWriter::Insert(w) => w.finish().map_err(io_err),
            RowWriter::Xlsx(w) => w.finish(),
            RowWriter::Html(w) => w.finish().map_err(io_err),
        }
    }
}

impl<W: Write> RowOut for RowWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        RowWriter::begin(self, columns)
    }
    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        RowWriter::write_row(self, row)
    }
}

// ---------------------------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------------------------

/// CSV written by hand because the NULL marker and the values need different quoting rules (the
/// csv crate quotes every field the same way). As in PostgreSQL's CSV output, NULL is always the
/// unquoted marker, even when every value is quoted, and a value whose text equals the marker
/// (including `''` when the marker is empty) is always quoted, so a loader such as `COPY … CSV`
/// tells the two apart.
pub struct CsvWriter<W: Write> {
    w: W,
    spec: CsvSpec,
    /// The record being built (reused between rows).
    line: Vec<u8>,
}

impl<W: Write> CsvWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        if self.spec.header {
            self.line.clear();
            for (i, name) in columns.iter().enumerate() {
                self.push_field(i, name, false);
            }
            self.line.extend_from_slice(self.spec.line_break);
            self.w.write_all(&self.line).map_err(io_err)?;
        }
        Ok(())
    }

    fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        self.line.clear();
        for (i, v) in row.iter().enumerate() {
            match csv_text(v, &self.spec) {
                Some(text) => {
                    let clashes = text == self.spec.null_as;
                    self.push_field(i, &text, clashes);
                }
                None => {
                    if i > 0 {
                        self.line.push(self.spec.delimiter);
                    }
                    self.line.extend_from_slice(self.spec.null_as.as_bytes());
                }
            }
        }
        self.line.extend_from_slice(self.spec.line_break);
        self.w.write_all(&self.line)
    }

    fn push_field(&mut self, i: usize, text: &str, force_quotes: bool) {
        let (delimiter, quote) = (self.spec.delimiter, self.spec.quote);
        if i > 0 {
            self.line.push(delimiter);
        }
        if force_quotes || self.spec.always_quote || csv_needs_quotes(text, delimiter, quote) {
            self.line.push(quote);
            // The quote is ASCII, so working on the UTF-8 bytes is safe.
            for &b in text.as_bytes() {
                if b == quote {
                    self.line.push(quote);
                }
                self.line.push(b);
            }
            self.line.push(quote);
        } else {
            self.line.extend_from_slice(text.as_bytes());
        }
    }

    fn finish(mut self) -> io::Result<W> {
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Whether a value must be quoted: it contains the delimiter, the quote character or a line
/// break, or it is `\.` (PostgreSQL's end-of-data marker when it stands alone on a line).
fn csv_needs_quotes(text: &str, delimiter: u8, quote: u8) -> bool {
    text == "\\."
        || text
            .bytes()
            .any(|b| b == delimiter || b == quote || matches!(b, b'\r' | b'\n'))
}

/// Text of a CSV cell: `None` for NULL, base64 for bytes, raw text for JSON, a PostgreSQL array
/// literal for arrays; numbers with the chosen decimal separator; line breaks as spaces when
/// asked.
fn csv_text(v: &Value, spec: &CsvSpec) -> Option<String> {
    let text = match v {
        Value::Float(f) => float_text(*f),
        other => other.as_text()?,
    };
    let text = if spec.decimal != b'.' && matches!(v, Value::Float(_) | Value::Decimal(_)) {
        // The separator is one ASCII character.
        text.replace('.', char::from(spec.decimal).encode_utf8(&mut [0; 4]))
    } else {
        text
    };
    if spec.line_break_to_space && text.contains(['\r', '\n']) {
        Some(text.replace("\r\n", " ").replace(['\r', '\n'], " "))
    } else {
        Some(text)
    }
}

/// Shortest round-trip text of a float; exponent notation for very large or small magnitudes
/// (plain `Display` would spell 1e300 with 301 digits).
pub fn float_text(f: f64) -> String {
    let abs = f.abs();
    if abs != 0.0 && abs.is_finite() && !(1e-5..1e16).contains(&abs) {
        format!("{f:e}")
    } else {
        f.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// JSON / NDJSON
// ---------------------------------------------------------------------------------------------

pub struct JsonWriter<W: Write> {
    w: W,
    ndjson: bool,
    pretty: bool,
    /// Column names, already encoded as JSON strings.
    keys: Vec<String>,
    rows: u64,
}

impl<W: Write> JsonWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        self.keys = columns
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()?;
        Ok(())
    }

    fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        if !self.ndjson {
            self.w
                .write_all(if self.rows == 0 { b"[\n" } else { b",\n" })?;
        }
        if self.pretty {
            self.w.write_all(b"  {")?;
            for (i, (key, v)) in self.keys.iter().zip(row).enumerate() {
                self.w
                    .write_all(if i == 0 { b"\n    " } else { b",\n    " })?;
                self.w.write_all(key.as_bytes())?;
                self.w.write_all(b": ")?;
                write_json_value(&mut self.w, v)?;
            }
            self.w
                .write_all(if self.keys.is_empty() { b"}" } else { b"\n  }" })?;
        } else {
            self.w.write_all(b"{")?;
            for (i, (key, v)) in self.keys.iter().zip(row).enumerate() {
                if i > 0 {
                    self.w.write_all(b",")?;
                }
                self.w.write_all(key.as_bytes())?;
                self.w.write_all(b":")?;
                write_json_value(&mut self.w, v)?;
            }
            self.w.write_all(b"}")?;
        }
        if self.ndjson {
            self.w.write_all(b"\n")?;
        }
        self.rows += 1;
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        if !self.ndjson {
            self.w
                .write_all(if self.rows == 0 { b"[]\n" } else { b"\n]\n" })?;
        }
        self.w.flush()?;
        Ok(self.w)
    }
}

fn write_json_str<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    serde_json::to_writer(&mut *w, s).map_err(io::Error::other)
}

/// One cell as JSON: integers that are safe in JavaScript as numbers, big integers and decimals
/// as strings, JSON cells embedded (minified) when they are valid JSON, bytes as base64.
pub fn write_json_value<W: Write>(w: &mut W, v: &Value) -> io::Result<()> {
    match v {
        Value::Null | Value::Default => w.write_all(b"null"),
        Value::Bool(b) => w.write_all(if *b { b"true" } else { b"false" }),
        Value::Int(n) => write!(w, "{n}"),
        Value::Float(f) if f.is_finite() => {
            serde_json::to_writer(&mut *w, f).map_err(io::Error::other)
        }
        Value::Float(f) => write_json_str(w, &f.to_string()),
        Value::Json(raw) => {
            if serde_json::from_str::<serde::de::IgnoredAny>(raw).is_ok() {
                write_minified_json(w, raw)
            } else {
                write_json_str(w, raw)
            }
        }
        Value::Array(items) => {
            w.write_all(b"[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    w.write_all(b",")?;
                }
                write_json_value(w, item)?;
            }
            w.write_all(b"]")
        }
        Value::Bytes(b) => write_json_str(w, &b.base64),
        Value::Other(o) => write_json_str(w, &o.text),
        Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Uuid(s)
        | Value::Enum(s) => write_json_str(w, s),
    }
}

/// Copies valid JSON text without the whitespace between tokens (so NDJSON stays one line).
pub(crate) fn write_minified_json<W: Write>(w: &mut W, raw: &str) -> io::Result<()> {
    let bytes = raw.as_bytes();
    let mut in_str = false;
    let mut escaped = false;
    let mut start = 0;
    for (i, &c) in bytes.iter().enumerate() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            w.write_all(&bytes[start..i])?;
            start = i + 1;
        }
    }
    w.write_all(&bytes[start..])
}

// ---------------------------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------------------------

/// The page's inline style: compact, readable in light and dark viewers.
const HTML_STYLE: &str = "\
:root{color-scheme:light dark}
body{font:13px -apple-system,BlinkMacSystemFont,\"Segoe UI\",Helvetica,Arial,sans-serif;margin:16px}
h2{font-size:15px;margin:20px 0 8px}
table{border-collapse:collapse;margin:0 0 16px}
th,td{border:1px solid #c8c8cc;padding:3px 8px;text-align:left;vertical-align:top;white-space:pre-wrap}
th{background:rgba(127,127,127,.14);font-weight:600}
td.num{text-align:right;font-variant-numeric:tabular-nums}
td.null{color:#8e8e93;font-style:italic}
";

/// `text` with `&`, `<`, `>`, `"` and `'` escaped for HTML text and attribute values.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// An HTML page with one `<table>` per result set: `begin` opens a table (closing the previous
/// one), `next_table` names the following one with a heading (multi-table files).
pub struct HtmlWriter<W: Write> {
    w: W,
    title: String,
    charset: &'static str,
    /// Heading written before the next table.
    heading: Option<String>,
    started: bool,
    in_table: bool,
}

impl<W: Write> HtmlWriter<W> {
    pub fn new(w: W, title: &str, charset: &'static str) -> HtmlWriter<W> {
        HtmlWriter {
            w,
            title: title.to_string(),
            charset,
            heading: None,
            started: false,
            in_table: false,
        }
    }

    fn start(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.started, true) {
            write!(
                self.w,
                "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"{}\">\n<title>{}</title>\n<style>\n{HTML_STYLE}</style>\n</head>\n<body>\n",
                self.charset,
                html_escape(&self.title)
            )?;
        }
        Ok(())
    }

    /// Closes the open table; the next one gets `heading` above it.
    pub fn next_table(&mut self, heading: &str) -> Result<(), AppError> {
        self.end_table()?;
        self.heading = Some(heading.to_string());
        Ok(())
    }

    /// Closes the open table, if any.
    pub fn end_table(&mut self) -> Result<(), AppError> {
        if std::mem::take(&mut self.in_table) {
            self.w.write_all(b"</tbody>\n</table>\n").map_err(io_err)?;
        }
        Ok(())
    }

    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        self.start().map_err(io_err)?;
        self.end_table()?;
        let mut head = String::new();
        if let Some(h) = self.heading.take() {
            head.push_str(&format!("<h2>{}</h2>\n", html_escape(&h)));
        }
        head.push_str("<table>\n<thead>\n<tr>");
        for c in columns {
            head.push_str("<th>");
            head.push_str(&html_escape(c));
            head.push_str("</th>");
        }
        head.push_str("</tr>\n</thead>\n<tbody>\n");
        self.w.write_all(head.as_bytes()).map_err(io_err)?;
        self.in_table = true;
        Ok(())
    }

    fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        let mut line = String::from("<tr>");
        for v in row {
            match v {
                Value::Null | Value::Default => line.push_str("<td class=\"null\">NULL</td>"),
                Value::Int(_) | Value::BigInt(_) | Value::Decimal(_) | Value::Float(_) => {
                    let text = match v {
                        Value::Float(f) => float_text(*f),
                        other => other.as_text().unwrap_or_default(),
                    };
                    line.push_str("<td class=\"num\">");
                    line.push_str(&html_escape(&text));
                    line.push_str("</td>");
                }
                other => {
                    line.push_str("<td>");
                    line.push_str(&html_escape(&other.as_text().unwrap_or_default()));
                    line.push_str("</td>");
                }
            }
        }
        line.push_str("</tr>\n");
        self.w.write_all(line.as_bytes())
    }

    /// Closes the open table and the page (a page without any table is still complete).
    pub fn finish(mut self) -> io::Result<W> {
        self.start()?;
        if std::mem::take(&mut self.in_table) {
            self.w.write_all(b"</tbody>\n</table>\n")?;
        }
        self.w.write_all(b"</body>\n</html>\n")?;
        self.w.flush()?;
        Ok(self.w)
    }
}

impl<W: Write> RowOut for HtmlWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        HtmlWriter::begin(self, columns)
    }
    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        HtmlWriter::write_row(self, row).map_err(io_err)
    }
}

// ---------------------------------------------------------------------------------------------
// SQL INSERT
// ---------------------------------------------------------------------------------------------

pub struct InsertWriter<W: Write> {
    w: W,
    dialect: Dialect,
    table: String,
    overriding_system_value: bool,
    mysql_long_lat: bool,
    /// `INSERT INTO t (a, b) VALUES\n`, built by `begin`.
    prefix: String,
    batch_size: u32,
    /// Rows written into the open statement (0 = no statement open).
    in_batch: u32,
    /// Bytes of the open statement so far (see `INSERT_CHUNK_BYTES`).
    stmt_bytes: usize,
}

impl<W: Write> InsertWriter<W> {
    fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        if columns.is_empty() {
            return Err(AppError::InvalidInput(
                "the result has no columns to write INSERT statements for".into(),
            ));
        }
        let cols: Vec<String> = columns
            .iter()
            .map(|c| self.dialect.quote_ident(c))
            .collect();
        let overriding = if self.overriding_system_value && self.dialect == Dialect::Postgres {
            " OVERRIDING SYSTEM VALUE"
        } else {
            ""
        };
        self.prefix = format!(
            "INSERT INTO {} ({}){overriding} VALUES\n",
            self.table,
            cols.join(", ")
        );
        Ok(())
    }

    fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        let mut line = String::with_capacity(64);
        line.push('(');
        for (i, v) in row.iter().enumerate() {
            if i > 0 {
                line.push_str(", ");
            }
            line.push_str(&insert_literal(self.dialect, v, self.mysql_long_lat));
        }
        line.push(')');
        let sep: &[u8] = if self.in_batch == 0 {
            self.prefix.as_bytes()
        } else {
            b",\n"
        };
        self.w.write_all(sep).map_err(io_err)?;
        self.w.write_all(line.as_bytes()).map_err(io_err)?;
        self.stmt_bytes += sep.len() + line.len();
        self.in_batch += 1;
        // The statement ends at the row batch or at ~250 KB, whichever comes first (checked
        // after the row, so one huge row still goes out whole).
        if self.in_batch >= self.batch_size || self.stmt_bytes >= INSERT_CHUNK_BYTES {
            self.w.write_all(b";\n").map_err(io_err)?;
            self.in_batch = 0;
            self.stmt_bytes = 0;
        }
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        if self.in_batch > 0 {
            self.w.write_all(b";\n")?;
        }
        self.w.flush()?;
        Ok(self.w)
    }
}

static NUMERIC_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$").expect("valid numeric regex")
});
static BIT_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^b'[01]*'$").expect("valid bit regex"));
static HEX_DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[0-9A-Fa-f]{2})*$").expect("valid hex regex"));
/// The WKT the spatial decoder writes: a type keyword, then numbers, parentheses, commas, `EMPTY`
/// and dimension tags (`Z`, `M`, `ZM`).
static WKT_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9 ,.()+\-]*$").expect("valid WKT regex"));

/// `SRID=4326;POINT(1 2)` → (4326, `POINT(1 2)`); plain WKT has SRID 0. None for anything that
/// is not (E)WKT.
pub fn split_ewkt(text: &str) -> Option<(u32, &str)> {
    let t = text.trim();
    let (srid, wkt) = match t.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("SRID=") => {
            let (n, wkt) = t[5..].split_once(';')?;
            (n.trim().parse::<u32>().ok()?, wkt.trim())
        }
        _ => (0, t),
    };
    WKT_TEXT.is_match(wkt).then_some((srid, wkt))
}

/// A MySQL spatial value given as (E)WKT: `ST_GeomFromText('…', srid)`. MySQL 8 reads the
/// coordinates of a geographic SRS latitude first unless told `axis-order=long-lat`; the decoder
/// writes them in MySQL's storage order (longitude first), so 8+ gets that option for an SRID.
fn mysql_geometry(text: &str, long_lat: bool) -> Option<String> {
    let (srid, wkt) = split_ewkt(text)?;
    let wkt = string_literal(Dialect::Mysql, wkt);
    Some(if long_lat && srid != 0 {
        format!("ST_GeomFromText({wkt}, {srid}, 'axis-order=long-lat')")
    } else {
        format!("ST_GeomFromText({wkt}, {srid})")
    })
}

/// The PostGIS type a column type names (`geometry`, `geometry(Point,4326)`,
/// `public.geography`), if it is one.
fn postgis_type(ty: &str) -> Option<&'static str> {
    let lower = ty.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or_default();
    let base = base
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('"');
    match base {
        "geometry" => Some("geometry"),
        "geography" => Some("geography"),
        _ => None,
    }
}

/// A value as a SQL literal of `dialect`, for the INSERT export. Unlike the preview `literal`,
/// the text never depends on the loading session's settings: string literals contain no
/// backslash escapes unless they are PostgreSQL `E'…'` strings, which read the same with
/// `standard_conforming_strings` on or off, and MySQL strings with a backslash (or NUL) are
/// written as hex, so `NO_BACKSLASH_ESCAPES` cannot turn data into SQL. Database text is never
/// emitted unquoted unless it is a well-formed number; MySQL BIT / geometry values stay loadable.
pub fn sql_literal(dialect: Dialect, v: &Value) -> String {
    insert_literal(dialect, v, false)
}

/// `sql_literal`, with spatial values loadable on the server they came from: MySQL (E)WKT through
/// `ST_GeomFromText` (see `mysql_geometry`; `mysql_long_lat` for MySQL 8+), PostGIS EWKT or hex
/// cast to its type (`'SRID=4326;POINT(1 2)'::geometry`).
pub fn insert_literal(dialect: Dialect, v: &Value, mysql_long_lat: bool) -> String {
    match v {
        Value::Null | Value::Default | Value::Bool(_) | Value::Int(_) => literal(dialect, v),
        Value::Float(f) if f.is_finite() => float_literal(*f),
        Value::BigInt(s) | Value::Decimal(s) => {
            if NUMERIC_LITERAL.is_match(s) {
                s.clone()
            } else {
                // `NaN`, `Infinity`, or free text in a SQLite NUMERIC column.
                string_literal(dialect, s)
            }
        }
        Value::Bytes(b) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .unwrap_or_default();
            let hex = hex_upper(&bytes);
            match dialect {
                // `'\x…'::bytea` would change meaning with standard_conforming_strings off.
                Dialect::Postgres => format!("decode('{hex}', 'hex')"),
                Dialect::Mysql | Dialect::Sqlite => format!("X'{hex}'"),
            }
        }
        Value::Other(o) if dialect == Dialect::Mysql => {
            if o.ty == "bit" && BIT_LITERAL.is_match(&o.text) {
                o.text.clone()
            } else if o.ty == "geometry" && HEX_DIGITS.is_match(&o.text) {
                // The internal format (SRID + WKB), as mysqldump --hex-blob writes it.
                format!("X'{}'", o.text)
            } else if o.ty == "geometry"
                && let Some(geometry) = mysql_geometry(&o.text, mysql_long_lat)
            {
                geometry
            } else {
                string_literal(dialect, &o.text)
            }
        }
        Value::Other(o) if dialect == Dialect::Postgres => match postgis_type(&o.ty) {
            Some(ty) => format!("{}::{ty}", string_literal(dialect, &o.text)),
            None => string_literal(dialect, &o.text),
        },
        other => string_literal(dialect, &other.as_text().unwrap_or_default()),
    }
}

/// A float literal that stays a floating-point literal: `5.0`, not `5`, which SQLite would
/// store as an INTEGER in a column without REAL affinity.
fn float_literal(f: f64) -> String {
    let text = float_text(f);
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// A string literal that reads the same whatever the loading session's escape settings are.
fn string_literal(dialect: Dialect, s: &str) -> String {
    match dialect {
        Dialect::Postgres if s.contains('\\') => {
            // An escape string: backslashes are escapes whatever standard_conforming_strings is.
            format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
        }
        // In MySQL a backslash is an escape unless NO_BACKSLASH_ESCAPES is set; hex text with a
        // character set introducer has no escapes at all (and keeps NUL bytes intact).
        Dialect::Mysql if s.contains(['\\', '\0']) => {
            format!("_utf8mb4 X'{}'", hex_upper(s.as_bytes()))
        }
        // SQLite has no backslash escapes; NUL cannot appear inside a quoted literal.
        Dialect::Sqlite if s.contains('\0') => {
            format!("CAST(X'{}' AS TEXT)", hex_upper(s.as_bytes()))
        }
        _ => format!("'{}'", s.replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::{JS_SAFE_INT, OtherValue};

    fn run(spec: FormatSpec, columns: &[&str], rows: &[Vec<Value>]) -> String {
        let mut w = RowWriter::new(spec, Vec::new());
        let cols: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        w.begin(&cols).unwrap();
        for row in rows {
            w.write_row(row).unwrap();
        }
        String::from_utf8(w.finish().unwrap()).unwrap()
    }

    /// The CSV of the M6 tests: CRLF line ends, `"` quotes.
    fn csv(delimiter: u8, header: bool, quote_all: bool, null_as: &str) -> FormatSpec {
        FormatSpec::Csv(CsvSpec {
            delimiter,
            header,
            quote: b'"',
            always_quote: quote_all,
            null_as: null_as.into(),
            line_break: b"\r\n",
            decimal: b'.',
            line_break_to_space: false,
        })
    }

    fn csv_request(format: serde_json::Value) -> Result<FormatSpec, AppError> {
        let format: ExportFormat = serde_json::from_value(format).unwrap();
        FormatSpec::from_request(&format, Dialect::Postgres, None)
    }

    fn sample_rows() -> Vec<Vec<Value>> {
        vec![
            vec![
                Value::Int(1),
                Value::Text("plain".into()),
                Value::Null,
                Value::BigInt("9007199254740993".into()),
            ],
            vec![
                Value::Int(-2),
                Value::Text("say \"hi\", ok".into()),
                Value::Text(String::new()),
                Value::Decimal("12.50".into()),
            ],
            vec![
                Value::Int(3),
                Value::Text("line1\r\nline2".into()),
                Value::Text("héllo 世界 😀".into()),
                Value::Float(1.5),
            ],
        ]
    }

    #[test]
    fn csv_quotes_escapes_and_nulls() {
        let out = run(
            csv(b',', true, false, ""),
            &["id", "name", "note", "n"],
            &sample_rows(),
        );
        assert_eq!(
            out,
            "id,name,note,n\r\n\
             1,plain,,9007199254740993\r\n\
             -2,\"say \"\"hi\"\", ok\",\"\",12.50\r\n\
             3,\"line1\r\nline2\",héllo 世界 😀,1.5\r\n"
        );
    }

    #[test]
    fn csv_null_marker_quote_all_and_semicolon() {
        let rows = vec![vec![
            Value::Null,
            Value::Text("a;b".into()),
            Value::Text("a,b".into()),
            Value::Bool(true),
        ]];
        let out = run(
            csv(b';', false, false, "NULL"),
            &["a", "b", "c", "d"],
            &rows,
        );
        assert_eq!(out, "NULL;\"a;b\";a,b;true\r\n");
        let out = run(csv(b';', true, true, "\\N"), &["a", "b", "c", "d"], &rows);
        // The NULL marker stays unquoted even when every value is quoted.
        assert_eq!(
            out,
            "\"a\";\"b\";\"c\";\"d\"\r\n\\N;\"a;b\";\"a,b\";\"true\"\r\n"
        );
        let out = run(
            csv(b'\t', false, false, ""),
            &["a", "b"],
            &[vec![Value::Text("x\ty".into()), Value::Int(1)]],
        );
        assert_eq!(out, "\"x\ty\"\t1\r\n");
    }

    #[test]
    fn csv_null_and_values_equal_to_the_marker_stay_distinct() {
        let rows = vec![
            vec![Value::Int(1), Value::Null, Value::Text(String::new())],
            vec![Value::Int(2), Value::Text("NULL".into()), Value::Null],
            vec![
                Value::Int(3),
                Value::Text("\\N".into()),
                Value::Text("\\.".into()),
            ],
        ];
        let cols = ["id", "a", "b"];
        // Default options: NULL is an empty unquoted field, '' an empty quoted one.
        assert_eq!(
            run(csv(b',', false, false, ""), &cols, &rows),
            "1,,\"\"\r\n2,NULL,\r\n3,\\N,\"\\.\"\r\n"
        );
        // quoteAll: every value is quoted, the NULL marker never is.
        assert_eq!(
            run(csv(b',', false, true, ""), &cols, &rows),
            "\"1\",,\"\"\r\n\"2\",\"NULL\",\r\n\"3\",\"\\N\",\"\\.\"\r\n"
        );
        // A value spelled like a non-empty marker is quoted; '' is not ambiguous then.
        assert_eq!(
            run(csv(b',', false, false, "NULL"), &cols, &rows),
            "1,NULL,\r\n2,\"NULL\",NULL\r\n3,\\N,\"\\.\"\r\n"
        );
        assert_eq!(
            run(csv(b';', false, true, "\\N"), &cols, &rows),
            "\"1\";\\N;\"\"\r\n\"2\";\"NULL\";\\N\r\n\"3\";\"\\N\";\"\\.\"\r\n"
        );
        // One column: a NULL row is an empty line (as PostgreSQL writes it), '' is `""`.
        assert_eq!(
            run(
                csv(b',', true, false, ""),
                &["v"],
                &[vec![Value::Null], vec![Value::Text(String::new())]]
            ),
            "v\r\n\r\n\"\"\r\n"
        );
    }

    #[test]
    fn csv_bytes_json_and_arrays_as_text() {
        let rows = vec![vec![
            Value::from_bytes(&[0, 1, 2, 255]),
            Value::Json("{\"a\": [1, 2]}".into()),
            Value::Array(vec![Value::Int(1), Value::Null, Value::Text("x y".into())]),
            Value::other("inet", "10.0.0.1/32"),
        ]];
        let out = run(csv(b',', false, false, ""), &["b", "j", "arr", "o"], &rows);
        assert_eq!(
            out,
            "AAEC/w==,\"{\"\"a\"\": [1, 2]}\",\"{\"\"1\"\",NULL,\"\"x y\"\"}\",10.0.0.1/32\r\n"
        );
        // The output parses back with a CSV reader.
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(out.as_bytes());
        let rec = rdr.records().next().unwrap().unwrap();
        assert_eq!(&rec[1], "{\"a\": [1, 2]}");
        assert_eq!(&rec[2], "{\"1\",NULL,\"x y\"}");
    }

    #[test]
    fn csv_round_trips_crlf_and_unicode() {
        let rows = sample_rows();
        let out = run(
            csv(b',', true, false, ""),
            &["id", "name", "note", "n"],
            &rows,
        );
        let mut rdr = csv::Reader::from_reader(out.as_bytes());
        let recs: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();
        assert_eq!(recs.len(), 3);
        assert_eq!(&recs[1][1], "say \"hi\", ok");
        assert_eq!(&recs[2][1], "line1\r\nline2");
        assert_eq!(&recs[2][2], "héllo 世界 😀");
    }

    #[test]
    fn json_array_types_and_embedding() {
        let rows = vec![
            vec![
                Value::Int(JS_SAFE_INT),
                Value::BigInt("9007199254740993".into()),
                Value::Decimal("0.10".into()),
                Value::Float(2.5),
                Value::Bool(false),
                Value::Null,
                Value::Json(
                    "{\n  \"k\": [1, 2.50, \"a b\"],\n  \"n\": 12345678901234567890\n}".into(),
                ),
                Value::from_bytes(b"hi"),
                Value::Text("line\r\n\"q\" 😀".into()),
            ],
            vec![
                Value::Int(-1),
                Value::BigInt("-9007199254740993".into()),
                Value::Decimal("NaN".into()),
                Value::Float(-0.25),
                Value::Bool(true),
                Value::Array(vec![Value::Int(1), Value::Null, Value::Text("x".into())]),
                Value::Json("not json {".into()),
                Value::Null,
                Value::Timestamp("2026-01-02 03:04:05".into()),
            ],
        ];
        let cols = ["i", "big", "dec", "f", "b", "n", "j", "bytes", "t"];
        let out = run(
            FormatSpec::Json {
                ndjson: false,
                pretty: false,
            },
            &cols,
            &rows,
        );
        assert_eq!(
            out,
            "[\n\
             {\"i\":9007199254740991,\"big\":\"9007199254740993\",\"dec\":\"0.10\",\"f\":2.5,\"b\":false,\"n\":null,\"j\":{\"k\":[1,2.50,\"a b\"],\"n\":12345678901234567890},\"bytes\":\"aGk=\",\"t\":\"line\\r\\n\\\"q\\\" 😀\"},\n\
             {\"i\":-1,\"big\":\"-9007199254740993\",\"dec\":\"NaN\",\"f\":-0.25,\"b\":true,\"n\":[1,null,\"x\"],\"j\":\"not json {\",\"bytes\":null,\"t\":\"2026-01-02 03:04:05\"}\n\
             ]\n"
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 2);
        assert_eq!(parsed[0]["j"]["k"][2], "a b");
        assert_eq!(parsed[0]["t"], "line\r\n\"q\" 😀");
        assert_eq!(parsed[1]["n"][1], serde_json::Value::Null);
    }

    #[test]
    fn json_pretty_ndjson_and_empty() {
        let rows = vec![
            vec![Value::Int(1), Value::Json("{\"a\" : 1}".into())],
            vec![Value::Int(2), Value::Json("[ ]".into())],
        ];
        let pretty = run(
            FormatSpec::Json {
                ndjson: false,
                pretty: true,
            },
            &["id", "doc"],
            &rows,
        );
        assert_eq!(
            pretty,
            "[\n  {\n    \"id\": 1,\n    \"doc\": {\"a\":1}\n  },\n  {\n    \"id\": 2,\n    \"doc\": []\n  }\n]\n"
        );
        let parsed: serde_json::Value = serde_json::from_str(&pretty).unwrap();
        assert_eq!(parsed[0]["doc"]["a"], 1);

        // NDJSON: one compact object per line, `pretty` ignored, no brackets.
        let nd = run(
            FormatSpec::Json {
                ndjson: true,
                pretty: true,
            },
            &["id", "doc"],
            &rows,
        );
        assert_eq!(nd, "{\"id\":1,\"doc\":{\"a\":1}}\n{\"id\":2,\"doc\":[]}\n");
        for line in nd.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }

        let empty = run(
            FormatSpec::Json {
                ndjson: false,
                pretty: false,
            },
            &["id"],
            &[],
        );
        assert_eq!(empty, "[]\n");
        let parsed: serde_json::Value = serde_json::from_str(&empty).unwrap();
        assert!(parsed.as_array().unwrap().is_empty());
        let empty_nd = run(
            FormatSpec::Json {
                ndjson: true,
                pretty: false,
            },
            &["id"],
            &[],
        );
        assert_eq!(empty_nd, "");
    }

    #[test]
    fn minify_keeps_string_whitespace() {
        let mut out = Vec::new();
        write_minified_json(&mut out, "{ \"a b\" : \"x \\\" y\" ,\n\t\"c\": [ 1 , 2 ] }").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"a b\":\"x \\\" y\",\"c\":[1,2]}"
        );
    }

    fn insert(dialect: Dialect, batch: u32) -> FormatSpec {
        FormatSpec::from_request(
            &ExportFormat::SqlInsert {
                table_name: String::new(),
                batch_size: batch,
                drop_table: false,
                structure: false,
                content: true,
                gzip: false,
            },
            dialect,
            Some("my table"),
        )
        .unwrap()
    }

    fn insert_rows() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Int(1), Value::Text("it's".into()), Value::Bool(true)],
            vec![Value::Int(2), Value::Null, Value::Bool(false)],
            vec![
                Value::Int(3),
                Value::Text("a\\b\r\nc".into()),
                Value::Bool(true),
            ],
            vec![Value::Int(4), Value::Text("😀".into()), Value::Null],
            vec![
                Value::Int(5),
                Value::Text(String::new()),
                Value::Bool(false),
            ],
        ]
    }

    #[test]
    fn insert_batches_per_dialect() {
        let out = run(
            insert(Dialect::Postgres, 2),
            &["id", "na\"me", "ok"],
            &insert_rows(),
        );
        assert_eq!(
            out,
            "INSERT INTO \"my table\" (\"id\", \"na\"\"me\", \"ok\") VALUES\n\
             (1, 'it''s', TRUE),\n\
             (2, NULL, FALSE);\n\
             INSERT INTO \"my table\" (\"id\", \"na\"\"me\", \"ok\") VALUES\n\
             (3, E'a\\\\b\r\nc', TRUE),\n\
             (4, '😀', NULL);\n\
             INSERT INTO \"my table\" (\"id\", \"na\"\"me\", \"ok\") VALUES\n\
             (5, '', FALSE);\n"
        );
        let out = run(
            insert(Dialect::Mysql, 3),
            &["id", "name", "ok"],
            &insert_rows(),
        );
        assert_eq!(
            out,
            "INSERT INTO `my table` (`id`, `name`, `ok`) VALUES\n\
             (1, 'it''s', 1),\n\
             (2, NULL, 0),\n\
             (3, _utf8mb4 X'615C620D0A63', 1);\n\
             INSERT INTO `my table` (`id`, `name`, `ok`) VALUES\n\
             (4, '😀', NULL),\n\
             (5, '', 0);\n"
        );
        // Exactly one full batch: no empty trailing statement.
        let out = run(
            insert(Dialect::Sqlite, 5),
            &["id", "name", "ok"],
            &insert_rows(),
        );
        assert_eq!(out.matches("INSERT INTO").count(), 1);
        assert!(out.ends_with("(5, '', 0);\n"));
        let out = run(
            insert(Dialect::Sqlite, 1),
            &["id", "name", "ok"],
            &insert_rows(),
        );
        assert_eq!(out.matches("INSERT INTO").count(), 5);
        // No rows: nothing at all.
        assert_eq!(run(insert(Dialect::Sqlite, 10), &["id"], &[]), "");
    }

    #[test]
    fn insert_output_loads_into_sqlite() {
        let rows = vec![
            vec![
                Value::Int(1),
                Value::Text("it's \"q\"\r\n😀".into()),
                Value::from_bytes(&[0, 1, 254, 255]),
                Value::Decimal("12.50".into()),
                Value::BigInt("9007199254740993".into()),
                Value::Float(0.1),
                Value::Float(5.0),
            ],
            vec![
                Value::Int(2),
                Value::Null,
                Value::Null,
                Value::Decimal("not a number".into()),
                Value::Null,
                Value::Float(-1e-7),
                Value::Float(-0.0),
            ],
            vec![
                Value::Int(3),
                Value::Text("x'); DROP TABLE t; --".into()),
                Value::from_bytes(&[]),
                Value::Decimal("-.5e3".into()),
                Value::BigInt("-9223372036854775808".into()),
                Value::Float(1e300),
                Value::Float(1e20),
            ],
            vec![
                Value::Int(4),
                Value::Text("nul \0 inside \\ ok".into()),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Float(2.5),
            ],
        ];
        let out = run(
            insert(Dialect::Sqlite, 2),
            &["id", "t", "b", "d", "big", "f", "v"],
            &rows,
        );
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE \"my table\" (id INTEGER PRIMARY KEY, t TEXT, b BLOB, d, big INTEGER, f REAL, v);",
        )
        .unwrap();
        db.execute_batch(&out).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM \"my table\"", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 4);
        // Integral floats stay REAL in a column without type affinity.
        let kinds: String = db
            .query_row(
                "SELECT group_concat(typeof(v), ',') FROM (SELECT v FROM \"my table\" ORDER BY id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kinds, "real,real,real,real");
        let t: String = db
            .query_row("SELECT t FROM \"my table\" WHERE id = 4", [], |r| r.get(0))
            .unwrap();
        assert_eq!(t, "nul \0 inside \\ ok");
        let (t, b, big): (String, Vec<u8>, i64) = db
            .query_row("SELECT t, b, big FROM \"my table\" WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(t, "it's \"q\"\r\n😀");
        assert_eq!(b, vec![0, 1, 254, 255]);
        assert_eq!(big, 9007199254740993);
        let d: String = db
            .query_row("SELECT d FROM \"my table\" WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(d, "not a number");
        let (t, big): (String, i64) = db
            .query_row("SELECT t, big FROM \"my table\" WHERE id = 3", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(t, "x'); DROP TABLE t; --");
        assert_eq!(big, i64::MIN);
    }

    #[test]
    fn floats_use_short_forms() {
        assert_eq!(float_text(1.5), "1.5");
        assert_eq!(float_text(12345678.0), "12345678");
        assert_eq!(float_text(0.0), "0");
        assert_eq!(float_text(1e300), "1e300");
        assert_eq!(float_text(-1e-7), "-1e-7");
        assert_eq!(float_text(f64::MAX), "1.7976931348623157e308");
        assert_eq!(f64::MAX.to_string().len(), 309);
        assert_eq!(sql_literal(Dialect::Mysql, &Value::Float(-1e300)), "-1e300");
        // INSERT literals of integral floats keep a fraction, so they stay REAL literals.
        assert_eq!(sql_literal(Dialect::Sqlite, &Value::Float(5.0)), "5.0");
        assert_eq!(sql_literal(Dialect::Sqlite, &Value::Float(-0.0)), "-0.0");
        assert_eq!(
            sql_literal(Dialect::Postgres, &Value::Float(12345678.0)),
            "12345678.0"
        );
        assert_eq!(sql_literal(Dialect::Sqlite, &Value::Float(2.5)), "2.5");
        assert_eq!(sql_literal(Dialect::Sqlite, &Value::Float(1e20)), "1e20");
        let out = run(
            csv(b',', false, false, ""),
            &["f"],
            &[vec![Value::Float(1e20)]],
        );
        assert_eq!(out, "1e20\r\n");
    }

    #[test]
    fn literals_guard_numbers_and_mysql_specials() {
        assert_eq!(
            sql_literal(Dialect::Postgres, &Value::Decimal("NaN".into())),
            "'NaN'"
        );
        assert_eq!(
            sql_literal(Dialect::Postgres, &Value::Decimal("-1.5E+3".into())),
            "-1.5E+3"
        );
        assert_eq!(
            sql_literal(Dialect::Sqlite, &Value::BigInt("1; DROP".into())),
            "'1; DROP'"
        );
        assert_eq!(
            sql_literal(
                Dialect::Mysql,
                &Value::Other(OtherValue {
                    ty: "bit".into(),
                    text: "b'101'".into()
                })
            ),
            "b'101'"
        );
        assert_eq!(
            sql_literal(Dialect::Mysql, &Value::other("geometry", "000000001")),
            "'000000001'"
        );
        assert_eq!(
            sql_literal(Dialect::Mysql, &Value::other("geometry", "00000000AB")),
            "X'00000000AB'"
        );
        assert_eq!(
            sql_literal(Dialect::Mysql, &Value::other("bit", "b'1'); x")),
            "'b''1''); x'"
        );
        assert_eq!(
            sql_literal(Dialect::Postgres, &Value::other("inet", "10.0.0.1")),
            "'10.0.0.1'"
        );
        assert_eq!(
            sql_literal(Dialect::Postgres, &Value::from_bytes(&[0xde, 0xad])),
            "decode('DEAD', 'hex')"
        );
        assert_eq!(
            sql_literal(Dialect::Mysql, &Value::from_bytes(&[0xde, 0xad])),
            "X'DEAD'"
        );
    }

    #[test]
    fn spatial_values_load_back_as_geometries() {
        let my = |text: &str, long_lat: bool| {
            insert_literal(Dialect::Mysql, &Value::other("geometry", text), long_lat)
        };
        // Hex (the decoder off): the internal format, byte for byte.
        assert_eq!(my("E6100000010100000000", false), "X'E6100000010100000000'");
        // WKT: ST_GeomFromText with the SRID; MySQL 8 reads geographic WKT longitude first.
        assert_eq!(
            my("LINESTRING(0 0,1 2)", true),
            "ST_GeomFromText('LINESTRING(0 0,1 2)', 0)"
        );
        assert_eq!(
            my("SRID=4326;POINT(2 1)", true),
            "ST_GeomFromText('POINT(2 1)', 4326, 'axis-order=long-lat')"
        );
        assert_eq!(
            my("SRID=4326;POINT(2 1)", false),
            "ST_GeomFromText('POINT(2 1)', 4326)"
        );
        assert_eq!(
            my("srid=3857; MULTIPOINT((1 2),(3 -4.5e3)) ", false),
            "ST_GeomFromText('MULTIPOINT((1 2),(3 -4.5e3))', 3857)"
        );
        assert_eq!(
            my("GEOMETRYCOLLECTION EMPTY", false),
            "ST_GeomFromText('GEOMETRYCOLLECTION EMPTY', 0)"
        );
        // Anything else stays a string (never SQL).
        assert_eq!(my("POINT(1 2)'); DROP", false), "'POINT(1 2)''); DROP'");
        assert_eq!(my("SRID=x;POINT(1 2)", false), "'SRID=x;POINT(1 2)'");
        assert_eq!(
            insert_literal(Dialect::Mysql, &Value::other("set", "POINT(1 2)"), true),
            "'POINT(1 2)'"
        );
        // PostGIS: EWKT (or hex EWKB) cast to the column's type.
        let pg = |ty: &str, text: &str| {
            insert_literal(Dialect::Postgres, &Value::other(ty, text), false)
        };
        assert_eq!(
            pg("geometry", "SRID=4326;POINT(1 2)"),
            "'SRID=4326;POINT(1 2)'::geometry"
        );
        assert_eq!(
            pg("geometry(Point,4326)", "0101000020E6100000"),
            "'0101000020E6100000'::geometry"
        );
        assert_eq!(
            pg("public.geography", "POINT(1 2)"),
            "'POINT(1 2)'::geography"
        );
        assert_eq!(pg("inet", "10.0.0.1"), "'10.0.0.1'");
        assert_eq!(
            split_ewkt("SRID=4326;POINT(1 2)"),
            Some((4326, "POINT(1 2)"))
        );
        assert_eq!(split_ewkt("not wkt!"), None);
    }

    #[test]
    fn string_literals_do_not_depend_on_escape_settings() {
        let text = |d: Dialect, s: &str| sql_literal(d, &Value::Text(s.into()));
        // No backslash: plain literals with doubled quotes in every dialect.
        assert_eq!(text(Dialect::Postgres, "it's"), "'it''s'");
        assert_eq!(text(Dialect::Mysql, "it's"), "'it''s'");
        assert_eq!(text(Dialect::Sqlite, "it's"), "'it''s'");
        // PostgreSQL: an E'' string reads the same with standard_conforming_strings off.
        assert_eq!(text(Dialect::Postgres, "a\\'b\\"), "E'a\\\\''b\\\\'");
        // MySQL: hex text, so NO_BACKSLASH_ESCAPES cannot end the literal early.
        let attack = "'); DROP TABLE v; -- \\";
        let lit = text(Dialect::Mysql, attack);
        assert!(lit.starts_with("_utf8mb4 X'"), "{lit}");
        let hex = lit.trim_start_matches("_utf8mb4 X'").trim_end_matches('\'');
        assert_eq!(hex, hex_upper(attack.as_bytes()));
        assert_eq!(text(Dialect::Mysql, "a\0b"), "_utf8mb4 X'610062'");
        assert_eq!(text(Dialect::Sqlite, "a\\b"), "'a\\b'");
        assert_eq!(text(Dialect::Sqlite, "a\0b"), "CAST(X'610062' AS TEXT)");
        // JSON and other textual kinds take the same path.
        assert_eq!(
            sql_literal(Dialect::Mysql, &Value::Json("{\"p\": \"a\\\\b\"}".into())),
            format!("_utf8mb4 X'{}'", hex_upper(b"{\"p\": \"a\\\\b\"}"))
        );
    }

    #[test]
    fn insert_overriding_system_value_on_postgres_only() {
        let spec = |dialect| FormatSpec::Insert {
            dialect,
            table: "\"t\"".into(),
            batch_size: 10,
            overriding_system_value: true,
            mysql_long_lat: false,
        };
        let rows = [vec![Value::Int(1), Value::Text("a".into())]];
        assert_eq!(
            run(spec(Dialect::Postgres), &["id", "note"], &rows),
            "INSERT INTO \"t\" (\"id\", \"note\") OVERRIDING SYSTEM VALUE VALUES\n(1, 'a');\n"
        );
        assert_eq!(
            run(spec(Dialect::Sqlite), &["id", "note"], &rows),
            "INSERT INTO \"t\" (\"id\", \"note\") VALUES\n(1, 'a');\n"
        );
    }

    #[test]
    fn validates_format_options() {
        let bad = |delimiter: &str| {
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": delimiter, "header": true, "nullAs": ""
            }))
            .is_err()
        };
        assert!(bad(""));
        assert!(bad(",;"));
        assert!(bad("\""));
        assert!(bad("\n"));
        assert!(bad("€"));
        assert!(!bad(";"));
        assert!(!bad("\t"));
        assert!(FormatSpec::from_request(&sql_insert("t", 0), Dialect::Postgres, None).is_err());
        // No table name and no source table: the INSERTs go into `untitled`.
        assert_eq!(
            FormatSpec::from_request(&sql_insert(" ", 10), Dialect::Postgres, None).unwrap(),
            FormatSpec::Insert {
                dialect: Dialect::Postgres,
                table: "\"untitled\"".into(),
                batch_size: 10,
                overriding_system_value: false,
                mysql_long_lat: false,
            }
        );
        assert_eq!(
            FormatSpec::from_request(&sql_insert("Dest`x", 10), Dialect::Mysql, Some("src"))
                .unwrap(),
            FormatSpec::Insert {
                dialect: Dialect::Mysql,
                table: "`Dest``x`".into(),
                batch_size: 10,
                overriding_system_value: false,
                mysql_long_lat: false,
            }
        );
        // The NULL text is written unquoted, so it cannot hold the delimiter, a quote or a
        // line break.
        let null_as = |delimiter: &str, null_as: &str| {
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": delimiter, "header": true, "nullAs": null_as
            }))
        };
        assert!(null_as(",", "a,b").is_err());
        assert!(null_as(";", "\"").is_err());
        assert!(null_as(",", "x\ny").is_err());
        assert!(null_as(",", "a;b").is_ok());
        assert!(null_as(",", "\\N").is_ok());
    }

    fn sql_insert(table_name: &str, batch_size: u32) -> ExportFormat {
        ExportFormat::SqlInsert {
            table_name: table_name.into(),
            batch_size,
            drop_table: false,
            structure: false,
            content: true,
            gzip: false,
        }
    }

    #[test]
    fn csv_options_of_the_export_dialog() {
        let rows = vec![
            vec![
                Value::Int(1),
                Value::Text("it's, \"x\"".into()),
                Value::Decimal("12.50".into()),
                Value::Float(0.25),
                Value::Null,
            ],
            vec![
                Value::Int(2),
                Value::Text("a\r\nb\nc\rd".into()),
                Value::BigInt("9007199254740993".into()),
                Value::Float(-1.5e300),
                Value::Text(String::new()),
            ],
        ];
        let cols = ["id", "t", "d", "f", "n"];
        let spec = |patch: serde_json::Value| {
            let mut base = serde_json::json!({
                "format": "csv", "delimiter": ",", "header": true, "nullAs": ""
            });
            for (k, v) in patch.as_object().unwrap() {
                base[k] = v.clone();
            }
            csv_request(base).unwrap()
        };
        // Defaults: \n line ends, quotes only when needed, NULL empty.
        assert_eq!(
            run(spec(serde_json::json!({})), &cols, &rows),
            "id,t,d,f,n\n1,\"it's, \"\"x\"\"\",12.50,0.25,\n2,\"a\r\nb\nc\rd\",9007199254740993,-1.5e300,\"\"\n"
        );
        // `'` around every value (doubled inside), NULL as `NULL` unquoted, CRLF, no header.
        assert_eq!(
            run(
                spec(serde_json::json!({
                    "quote": "single", "nullAs": "NULL", "lineBreak": "\r\n", "header": false
                })),
                &cols,
                &rows[..1]
            ),
            "'1','it''s, \"x\"','12.50','0.25',NULL\r\n"
        );
        // `"` around every value; \r records; line breaks inside values become spaces.
        assert_eq!(
            run(
                spec(serde_json::json!({
                    "quote": "double", "lineBreak": "\r", "lineBreakToSpace": true,
                    "header": false
                })),
                &cols,
                &rows[1..]
            ),
            "\"2\",\"a b c d\",\"9007199254740993\",\"-1.5e300\",\"\"\r"
        );
        // Decimal comma with a `;` delimiter: numbers keep their comma unquoted.
        assert_eq!(
            run(
                spec(serde_json::json!({"delimiter": ";", "decimal": ",", "header": false})),
                &cols,
                &rows[..1]
            ),
            "1;\"it's, \"\"x\"\"\";12,50;0,25;\n"
        );
        // …while with a `,` delimiter they are quoted, and the output reads back.
        let out = run(
            spec(serde_json::json!({"decimal": ",", "header": false})),
            &cols,
            &rows[..1],
        );
        assert_eq!(out, "1,\"it's, \"\"x\"\"\",\"12,50\",\"0,25\",\n");
        let rec = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(out.as_bytes())
            .records()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(&rec[2], "12,50");
        // Bad values are refused.
        assert!(
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": ",", "header": true, "nullAs": "", "lineBreak": "x"
            }))
            .is_err()
        );
        // A decimal separator is one character that is no digit: `;` is a valid custom one.
        assert!(
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": ",", "header": true, "nullAs": "", "decimal": "5"
            }))
            .is_err()
        );
        assert!(
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": ",", "header": true, "nullAs": "", "decimal": ";"
            }))
            .is_ok()
        );
        assert!(
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": "'", "header": true, "nullAs": ""
            }))
            .is_err()
        );
        assert!(csv_request(serde_json::json!({
            "format": "csv", "delimiter": ",", "header": true, "nullAs": "a'b", "quote": "single"
        }))
        .is_err());
    }

    #[test]
    fn html_escapes_and_marks_nulls_and_numbers() {
        let spec =
            FormatSpec::from_request(&ExportFormat::Html, Dialect::Sqlite, Some("a<b")).unwrap();
        let out = run(
            spec,
            &["id", "note & \"x\""],
            &[
                vec![
                    Value::Int(1),
                    Value::Text("<script>alert('x')</script>".into()),
                ],
                vec![Value::Float(2.5), Value::Null],
                vec![
                    Value::Decimal("12.50".into()),
                    Value::Text("a\nb 😀".into()),
                ],
            ],
        );
        assert!(out.starts_with("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">"));
        assert!(out.contains("<title>a&lt;b</title>"));
        assert!(out.contains("<th>id</th><th>note &amp; &quot;x&quot;</th>"));
        assert!(out.contains(
            "<tr><td class=\"num\">1</td><td>&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;</td></tr>"
        ));
        assert!(out.contains("<tr><td class=\"num\">2.5</td><td class=\"null\">NULL</td></tr>"));
        assert!(out.contains("<td>a\nb 😀</td>"));
        assert!(!out.contains("<script>"));
        assert!(out.ends_with("</tbody>\n</table>\n</body>\n</html>\n"));
        assert_eq!(out.matches("<table>").count(), 1);
    }

    #[test]
    fn html_pages_hold_one_table_per_table() {
        let mut page = HtmlWriter::new(Vec::new(), "export", "windows-1252");
        page.next_table("public.users").unwrap();
        RowOut::begin(&mut page, &["id".into()]).unwrap();
        RowOut::write_row(&mut page, &[Value::Int(1)]).unwrap();
        page.end_table().unwrap();
        page.next_table("orders").unwrap();
        RowOut::begin(&mut page, &["n".into()]).unwrap();
        let out = String::from_utf8(page.finish().unwrap()).unwrap();
        assert!(out.contains("<meta charset=\"windows-1252\">"));
        assert_eq!(out.matches("<table>").count(), 2);
        assert_eq!(out.matches("</table>").count(), 2);
        let users = out.find("<h2>public.users</h2>").unwrap();
        let orders = out.find("<h2>orders</h2>").unwrap();
        assert!(users < orders);
        // No table at all: still a complete page.
        let empty =
            String::from_utf8(HtmlWriter::new(Vec::new(), "t", "utf-8").finish().unwrap()).unwrap();
        assert!(empty.ends_with("<body>\n</body>\n</html>\n"));
    }

    #[test]
    fn csv_space_quote_wraps_every_value_in_spaces() {
        let rows = vec![
            vec![
                Value::Int(1),
                Value::Text("ab".into()),
                Value::Text("a b  c".into()),
                Value::Null,
            ],
            vec![
                Value::Int(2),
                Value::Text("x,y".into()),
                Value::Text(String::new()),
                Value::Text("it's \"q\"".into()),
            ],
        ];
        let cols = ["id", "t", "s", "n"];
        let spec = |patch: serde_json::Value| {
            let mut base = serde_json::json!({
                "format": "csv", "delimiter": ",", "header": true, "nullAs": "",
                "quote": "space"
            });
            for (k, v) in patch.as_object().unwrap() {
                base[k] = v.clone();
            }
            csv_request(base)
        };
        let out = run(spec(serde_json::json!({})).unwrap(), &cols, &rows);
        // Every value between spaces (the header too), a space inside a value doubled, NULL empty
        // and unquoted.
        assert_eq!(
            out,
            [
                " id , t , s , n \n",
                " 1 , ab , a  b    c ,\n",
                " 2 , x,y ,  , it's  \"q\" \n",
            ]
            .concat()
        );
        // The quote is a single space: another reader taking a space as its quote reads it back.
        let mut reader = crate::import::csvread::CsvReader::new(out.as_bytes(), b',', b' ');
        let header = reader.next_record().unwrap().unwrap();
        assert_eq!(
            header
                .fields
                .iter()
                .map(|f| f.text.as_str())
                .collect::<Vec<_>>(),
            ["id", "t", "s", "n"]
        );
        let first = reader.next_record().unwrap().unwrap();
        assert_eq!(first.fields[2].text, "a b  c");
        assert!(first.fields[2].quoted);
        assert_eq!(first.fields[3].text, "");
        assert!(!first.fields[3].quoted);
        let second = reader.next_record().unwrap().unwrap();
        assert_eq!(second.fields[1].text, "x,y");
        assert_eq!(second.fields[2].text, "");
        assert!(second.fields[2].quoted);
        // A space delimiter would be a quote: refused.
        assert!(spec(serde_json::json!({"delimiter": " "})).is_err());
        // Without the space quote a space delimiter stays what it was: only ASCII is required.
        assert!(spec(serde_json::json!({"delimiter": " ", "quote": "ifNeeded"})).is_ok());
        // The NULL text is written unquoted, so it cannot hold the quote.
        assert!(spec(serde_json::json!({"nullAs": "N/A "})).is_err());
        assert!(spec(serde_json::json!({"nullAs": "NULL"})).is_ok());
    }

    #[test]
    fn csv_custom_decimal_separator() {
        let rows = vec![vec![
            Value::Float(3.25),
            Value::Decimal("12.50".into()),
            Value::Int(7),
            Value::Text("1.5".into()),
        ]];
        let cols = ["f", "d", "i", "t"];
        let spec = |delimiter: &str, decimal: &str| {
            csv_request(serde_json::json!({
                "format": "csv", "delimiter": delimiter, "header": false, "nullAs": "",
                "decimal": decimal
            }))
        };
        // A custom character replaces the point of numbers, never of text.
        assert_eq!(
            run(spec(";", "#").unwrap(), &cols, &rows),
            "3#25;12#50;7;1.5\n"
        );
        // The two classic choices keep working (a comma delimiter then quotes the numbers).
        assert_eq!(
            run(spec(";", ",").unwrap(), &cols, &rows),
            "3,25;12,50;7;1.5\n"
        );
        assert_eq!(
            run(spec(",", ",").unwrap(), &cols, &rows),
            "\"3,25\",\"12,50\",7,1.5\n"
        );
        assert_eq!(
            run(spec(";", ".").unwrap(), &cols, &rows),
            "3.25;12.50;7;1.5\n"
        );
        // Exponent notation keeps its point replaced too: 1.5e20 → 1#5e20.
        assert_eq!(
            run(
                spec(";", "#").unwrap(),
                &["f"],
                &[vec![Value::Float(1.5e20)]]
            ),
            "1#5e20\n"
        );
        // Not a decimal separator: empty, several characters, a digit, the delimiter, a quote, a
        // line break or another control character, a non-ASCII character.
        for bad in ["", "..", "5", "\"", "'", "\n", "\r", "\t", "€"] {
            assert!(spec(";", bad).is_err(), "{bad:?} must be refused");
        }
        assert!(spec(";", ";").is_err());
        assert!(spec("\t", "\t").is_err());
        assert!(spec(";", "#").is_ok());
        assert!(spec(";", " ").is_ok());
    }

    /// Splits an INSERT script into its statements (the `;` that ends each is dropped).
    fn statements(script: &str) -> Vec<String> {
        crate::sqlgen::split::split_statements(script, Dialect::Sqlite)
            .into_iter()
            .map(|s| s.sql)
            .collect()
    }

    #[test]
    fn insert_statements_end_at_250_kb() {
        // 30 rows of 20 KB text with a batch size far above 30: the bytes decide.
        let big = "x".repeat(20 * 1024);
        let rows: Vec<Vec<Value>> = (0..30)
            .map(|i| vec![Value::Int(i), Value::Text(format!("{i:02}{big}"))])
            .collect();
        let out = run(insert(Dialect::Postgres, 1000), &["id", "t"], &rows);
        let stmts = statements(&out);
        assert!(stmts.len() >= 3, "{} statements", stmts.len());
        let row = 20 * 1024 + 40;
        for stmt in &stmts {
            // Closed as soon as it passed 250 KB: below the limit plus the row that crossed it.
            assert!(
                stmt.len() < INSERT_CHUNK_BYTES + row,
                "{} bytes",
                stmt.len()
            );
            assert!(stmt.starts_with("INSERT INTO \"my table\" (\"id\", \"t\") VALUES\n("));
        }
        // Every row once and in order (a row is one line starting with its id).
        let ids: Vec<usize> = out
            .lines()
            .filter_map(|l| l.strip_prefix('('))
            .map(|l| l.split(',').next().unwrap().parse().unwrap())
            .collect();
        assert_eq!(ids, (0..30).collect::<Vec<_>>());
        assert_eq!(out.matches("INSERT INTO").count(), stmts.len());
        // The statements the splitter finds hold every row once.
        let rows_in_statements: usize = stmts.iter().map(|s| s.matches("\n(").count()).sum();
        assert_eq!(rows_in_statements, 30);
        assert!(out.ends_with(");\n"));
    }

    #[test]
    fn insert_chunking_keeps_small_rows_in_row_batches() {
        // Small rows: the row-count batch decides, as before.
        let rows: Vec<Vec<Value>> = (0..25).map(|i| vec![Value::Int(i)]).collect();
        let out = run(insert(Dialect::Sqlite, 10), &["id"], &rows);
        assert_eq!(out.matches("INSERT INTO").count(), 3);
        // A single row bigger than the limit still goes out whole, in a statement of its own.
        let huge = "y".repeat(INSERT_CHUNK_BYTES + 10);
        let rows = vec![
            vec![Value::Int(1), Value::Text("small".into())],
            vec![Value::Int(2), Value::Text(huge.clone())],
            vec![Value::Int(3), Value::Text("small".into())],
        ];
        let out = run(insert(Dialect::Mysql, 100), &["id", "t"], &rows);
        let stmts = statements(&out);
        assert_eq!(stmts.len(), 2, "{}", out.len());
        assert!(stmts[0].contains("small") && stmts[0].contains(&huge));
        assert!(stmts[1].contains("(3, 'small')"));
        // A statement with the huge row first closes right after it.
        let rows = vec![
            vec![Value::Int(2), Value::Text(huge)],
            vec![Value::Int(3), Value::Text("small".into())],
        ];
        let out = run(insert(Dialect::Mysql, 100), &["id", "t"], &rows);
        assert_eq!(statements(&out).len(), 2);
    }

    #[test]
    fn xlsx_through_the_row_writer() {
        let spec = FormatSpec::from_request(
            &ExportFormat::Xlsx { header: true },
            Dialect::Sqlite,
            Some("people"),
        )
        .unwrap();
        let mut w = RowWriter::new(spec, Vec::new());
        w.begin(&["id".into(), "name".into()]).unwrap();
        w.write_row(&[Value::Int(7), Value::Text("Ann".into())])
            .unwrap();
        let bytes = w.finish().unwrap();
        let files = crate::export::xlsx::tests::unzip(&bytes);
        let wb = String::from_utf8(files["xl/workbook.xml"].clone()).unwrap();
        assert!(wb.contains("name=\"people\""));
        let sheet = String::from_utf8(files["xl/worksheets/sheet1.xml"].clone()).unwrap();
        assert!(sheet.contains("<c r=\"A2\"><v>7</v></c>"));
        assert!(sheet.contains(">Ann</t>"));
    }
}
