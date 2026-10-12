//! SQL dump import (File ▸ Import ▸ From SQL Dump…): reads a `.sql` or gzip-compressed file,
//! splits it with the SQL editor's splitter and runs the statements one by one on one session
//! (the import's own connection, see `commands::import`), so `SET`, `BEGIN … COMMIT` and `USE`
//! in the dump apply to the statements after them, as when psql or mysql loads it.
//!
//! The file is read in chunks: each chunk is split, its statements except the last run (the
//! last may continue in the next chunk), and the rest is carried over. A MySQL `DELIMITER`
//! directive before the carried text is repeated in front of it, because the splitter starts
//! every chunk with `;`. Text is UTF-8; a byte sequence that is not is decoded as Windows-1252
//! (as TablePlus falls back), only that sequence, and counted. Inside a MySQL `_binary '…'`
//! literal (mysqldump without `--hex-blob`) such bytes are binary data: the literal is sent as a
//! hex literal with exactly the file's bytes.
//!
//! An import that ends inside the dump's own transaction (stopped at an error, cancelled, or a
//! dump without its `COMMIT`) rolls it back and reports the statements undone; so does a
//! `COMMIT` PostgreSQL turns into a rollback after an error in the transaction.
//!
//! psql meta-commands (`\restrict`…) are skipped and counted, `\connect` to another database
//! stops the import (everything after it belongs there); `COPY … FROM stdin` data blocks cannot
//! be sent this way and end the import with an error pointing to Restore.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use flate2::read::MultiGzDecoder;
use tokio_util::sync::CancellationToken;

use crate::drivers::{CancelSlot, DbSession, RowSink, contains_word, lock_editor_runs};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::split::{code_only, is_meta_command, leading_keywords, split_statements};
use crate::types::import::{
    SqlDumpPreview, SqlImportError, SqlImportProgress, SqlImportReport, SqlImportRequest,
    TextEncoding,
};
use crate::types::query::ResultColumn;
use crate::types::value::Value;

use super::text::{CountingReader, single_byte_char};

/// Statements the preview shows.
pub const PREVIEW_STATEMENTS: usize = 20;
/// Text read for the preview.
const PREVIEW_BYTES: usize = 256 * 1024;
/// Bytes read from the file at a time.
const READ_CHUNK: usize = 4 * 1024 * 1024;
/// Text split at a time (grows while one statement is longer).
const SPLIT_AT: usize = 4 * 1024 * 1024;
/// Errors kept in the report.
const MAX_ERRORS: usize = 100;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

type DumpRead = Box<dyn Read + Send>;

/// The dump's bytes (gunzipped when the file starts with the gzip magic), its size and whether
/// it was compressed. `bytes` counts the file's bytes read.
fn open_dump(path: &str, bytes: Arc<AtomicU64>) -> Result<(DumpRead, u64, bool), AppError> {
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
    let mut reader = BufReader::with_capacity(256 * 1024, CountingReader::new(file, bytes));
    let compressed = reader
        .fill_buf()
        .map_err(|e| AppError::Io(format!("cannot read \"{path}\": {e}")))?
        .starts_with(&GZIP_MAGIC);
    let read: DumpRead = if compressed {
        Box::new(MultiGzDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    Ok((read, meta.len(), compressed))
}

/// Bytes of the dump that are not UTF-8 travel through the text as these private-use characters
/// (U+10FF00 + the byte) until `restore_bytes` turns them into Windows-1252 text, or back into
/// the bytes inside a MySQL `_binary '…'` literal. (A dump holding these very characters, which
/// no text uses, would see them decoded the same way.)
const RAW_BYTE_BASE: u32 = 0x10FF00;

fn raw_byte_char(b: u8) -> char {
    char::from_u32(RAW_BYTE_BASE + u32::from(b)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

fn raw_byte(c: char) -> Option<u8> {
    u32::from(c)
        .checked_sub(RAW_BYTE_BASE)
        .and_then(|b| u8::try_from(b).ok())
}

/// Decodes the dump as UTF-8 (a byte order mark dropped). Each byte sequence that is not UTF-8
/// becomes raw-byte characters (`RAW_BYTE_BASE`) and is counted, with the line of the first.
pub struct DumpText<R: Read> {
    inner: R,
    pending: Vec<u8>,
    started: bool,
    eof: bool,
    /// Line of the next text (1-based).
    line: u64,
    invalid: u64,
    first_invalid_line: Option<u64>,
}

impl<R: Read> DumpText<R> {
    pub fn new(inner: R) -> DumpText<R> {
        DumpText {
            inner,
            pending: Vec::new(),
            started: false,
            eof: false,
            line: 1,
            invalid: 0,
            first_invalid_line: None,
        }
    }

    /// UTF-8, or Windows-1252 once some bytes were not UTF-8 (only those are decoded so).
    pub fn encoding(&self) -> TextEncoding {
        if self.invalid > 0 {
            TextEncoding::Windows1252
        } else {
            TextEncoding::Utf8
        }
    }

    /// Byte sequences that were not UTF-8, and the line of the first.
    pub fn invalid(&self) -> (u64, Option<u64>) {
        (self.invalid, self.first_invalid_line)
    }

    pub fn at_end(&self) -> bool {
        self.eof && self.pending.is_empty()
    }

    fn push_valid(&mut self, out: &mut String, text: &str) {
        self.line += text.bytes().filter(|&b| b == b'\n').count() as u64;
        out.push_str(text);
    }

    fn push_invalid(&mut self, out: &mut String, bytes: &[u8]) {
        self.invalid += 1;
        self.first_invalid_line.get_or_insert(self.line);
        out.extend(bytes.iter().map(|&b| raw_byte_char(b)));
    }

    /// The next piece of text, from about `want` bytes of the file; `None` at the end.
    pub fn next_text(&mut self, want: usize) -> Result<Option<String>, AppError> {
        if self.at_end() {
            return Ok(None);
        }
        let mut chunk = vec![0u8; want.max(4)];
        let mut filled = 0;
        while filled < chunk.len() {
            let n = self
                .inner
                .read(&mut chunk[filled..])
                .map_err(|e| AppError::Io(format!("cannot read the dump: {e}")))?;
            if n == 0 {
                self.eof = true;
                break;
            }
            filled += n;
        }
        self.pending.extend_from_slice(&chunk[..filled]);
        if !self.started && (self.pending.len() >= 3 || self.eof) {
            self.started = true;
            if self.pending.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.pending.drain(..3);
            }
        }
        let pending = std::mem::take(&mut self.pending);
        let mut out = String::with_capacity(pending.len());
        let mut rest: &[u8] = &pending;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.push_valid(&mut out, text);
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (valid, tail) = rest.split_at(e.valid_up_to());
                    self.push_valid(&mut out, std::str::from_utf8(valid).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            self.push_invalid(&mut out, &tail[..n]);
                            rest = &tail[n..];
                        }
                        // The piece ends inside a character: keep its start for the next one.
                        None if !self.eof => {
                            rest = tail;
                            break;
                        }
                        None => {
                            self.push_invalid(&mut out, tail);
                            rest = &[];
                            break;
                        }
                    }
                }
            }
        }
        self.pending = rest.to_vec();
        Ok(Some(out))
    }
}

/// A statement as it is sent: raw bytes (`RAW_BYTE_BASE`) of a MySQL `_binary '…'` literal
/// become a hex literal holding exactly those bytes, every other raw byte its Windows-1252
/// character.
pub fn restore_bytes(sql: &str, dialect: Dialect) -> Cow<'_, str> {
    // Raw-byte characters are encoded F4 8F BC 80 … F4 8F BF BF.
    if !sql.as_bytes().contains(&0xF4) || !sql.chars().any(|c| raw_byte(c).is_some()) {
        return Cow::Borrowed(sql);
    }
    let sql: Cow<'_, str> = if dialect == Dialect::Mysql {
        Cow::Owned(binary_literals_as_hex(sql))
    } else {
        Cow::Borrowed(sql)
    };
    Cow::Owned(
        sql.chars()
            .map(|c| raw_byte(c).map_or(c, |b| single_byte_char(TextEncoding::Windows1252, b)))
            .collect(),
    )
}

/// The end (exclusive) of the MySQL quoted text starting at `chars[start]` (`'`, `"` or a
/// backtick); `None` when it does not end. Quotes inside are doubled or, in strings,
/// backslash-escaped.
fn mysql_quoted_end(chars: &[char], start: usize) -> Option<usize> {
    let q = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if q != '`' => i += 2,
            c if c == q => {
                if chars.get(i + 1) == Some(&q) {
                    i += 2;
                } else {
                    return Some(i + 1);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// The bytes of a MySQL string literal's body: escapes resolved, raw-byte characters as their
/// byte, other characters as UTF-8.
fn mysql_string_bytes(body: &[char]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let push = |out: &mut Vec<u8>, c: char| match raw_byte(c) {
        Some(b) => out.push(b),
        None => out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes()),
    };
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        match (c, body.get(i + 1).copied()) {
            ('\\', Some(next)) => {
                match next {
                    '0' => out.push(0),
                    'b' => out.push(8),
                    'n' => out.push(b'\n'),
                    'r' => out.push(b'\r'),
                    't' => out.push(b'\t'),
                    'Z' => out.push(0x1A),
                    // Outside LIKE patterns `\%` and `\_` keep their backslash.
                    '%' | '_' => {
                        out.push(b'\\');
                        push(&mut out, next);
                    }
                    other => push(&mut out, other),
                }
                i += 2;
            }
            ('\'', Some('\'')) => {
                out.push(b'\'');
                i += 2;
            }
            _ => {
                push(&mut out, c);
                i += 1;
            }
        }
    }
    out
}

/// MySQL: every `_binary '…'` literal holding raw bytes rewritten as `_binary X'…'`.
fn binary_literals_as_hex(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let skip_to = match c {
            '\'' | '"' | '`' => Some(mysql_quoted_end(&chars, i).unwrap_or(n)),
            '#' => Some(
                chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(n, |p| i + p),
            ),
            '-' if next == Some('-') && chars.get(i + 2).is_none_or(|c| c.is_whitespace()) => Some(
                chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(n, |p| i + p),
            ),
            // Executable comments (`/*! … */`) and hints are code.
            '/' if next == Some('*') && !matches!(chars.get(i + 2), Some('!' | '+')) => Some(
                (i + 2..n.saturating_sub(1))
                    .find(|&j| chars[j] == '*' && chars[j + 1] == '/')
                    .map_or(n, |j| j + 2),
            ),
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '$')) {
                    i += 1;
                }
                out.extend(&chars[start..i]);
                let word: String = chars[start..i].iter().collect();
                if word.eq_ignore_ascii_case("_binary") {
                    let mut j = i;
                    while j < n && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'\'')
                        && let Some(end) = mysql_quoted_end(&chars, j)
                        && chars[j + 1..end - 1].iter().any(|&c| raw_byte(c).is_some())
                    {
                        out.extend(&chars[i..j]);
                        out.push_str("X'");
                        for b in mysql_string_bytes(&chars[j + 1..end - 1]) {
                            out.push_str(&format!("{b:02X}"));
                        }
                        out.push('\'');
                        i = end;
                    }
                }
                continue;
            }
            _ => None,
        };
        match skip_to {
            Some(end) => {
                out.extend(&chars[i..end]);
                i = end;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// One statement of the dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpStatement {
    pub sql: String,
    /// 0-based statement number in the file.
    pub index: u64,
    /// 1-based line it starts on.
    pub line: u64,
}

/// Splits the dump's text as it arrives (see the module comment).
pub struct DumpSplitter {
    dialect: Dialect,
    buf: String,
    /// File line of the buffer's first line.
    base_line: u64,
    /// Split once the buffer holds this much.
    split_at: usize,
    next_index: u64,
}

/// Byte offsets of the given (sorted) char offsets in `text`.
fn byte_offsets(text: &str, chars: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(chars.len());
    let mut it = text.char_indices().map(|(b, _)| b).enumerate();
    let mut current = it.next();
    for &c in chars {
        loop {
            match current {
                Some((i, b)) if i == c => {
                    out.push(b);
                    break;
                }
                Some((i, _)) if i < c => current = it.next(),
                _ => {
                    out.push(text.len());
                    break;
                }
            }
        }
    }
    out
}

/// The last MySQL `DELIMITER` directive in `gap` (text between statements).
fn delimiter_in(gap: &str) -> Option<String> {
    let mut found = None;
    for line in gap.lines() {
        let line = line.trim_start();
        if let Some(head) = line.get(..9)
            && head.eq_ignore_ascii_case("DELIMITER")
            && let Some(rest) = line.get(9..)
            && rest.starts_with([' ', '\t'])
            && let Some(tok) = rest.split_whitespace().next()
        {
            found = Some(tok.to_string());
        }
    }
    found
}

impl DumpSplitter {
    pub fn new(dialect: Dialect) -> DumpSplitter {
        DumpSplitter {
            dialect,
            buf: String::new(),
            base_line: 1,
            split_at: SPLIT_AT,
            next_index: 0,
        }
    }

    fn emit(&mut self, sql: String, line: u32) -> DumpStatement {
        let st = DumpStatement {
            sql,
            index: self.next_index,
            line: self.base_line + u64::from(line.max(1)) - 1,
        };
        self.next_index += 1;
        st
    }

    /// Adds `text`; returns the statements known to be complete (every one when `eof`).
    pub fn push(&mut self, text: &str, eof: bool) -> Vec<DumpStatement> {
        self.buf.push_str(text);
        if !eof && self.buf.len() < self.split_at {
            return Vec::new();
        }
        let mut statements = split_statements(&self.buf, self.dialect);
        if eof {
            self.buf.clear();
            return statements
                .into_iter()
                .map(|s| self.emit(s.sql, s.line))
                .collect();
        }
        if statements.len() < 2 {
            // One statement (or none) so far: it may go on. Read on before splitting again.
            self.split_at = (self.buf.len() * 2).max(SPLIT_AT);
            return Vec::new();
        }
        let last = statements.pop().expect("at least two statements");
        // Offsets: every statement's start and end (for the gaps), then the carried start.
        let mut chars: Vec<usize> = Vec::with_capacity(statements.len() * 2 + 1);
        for s in &statements {
            chars.push(s.start);
            chars.push(s.end);
        }
        chars.push(last.start);
        let bytes = byte_offsets(&self.buf, &chars);
        let carry_at = *bytes.last().unwrap_or(&self.buf.len());
        let mut delimiter = None;
        if self.dialect == Dialect::Mysql {
            let mut from = 0;
            for pair in bytes[..bytes.len() - 1].chunks(2) {
                if let Some(d) = delimiter_in(&self.buf[from..pair[0]]) {
                    delimiter = Some(d);
                }
                from = pair[1];
            }
            if let Some(d) = delimiter_in(&self.buf[from..carry_at]) {
                delimiter = Some(d);
            }
        }
        let out: Vec<DumpStatement> = statements
            .into_iter()
            .map(|s| self.emit(s.sql, s.line))
            .collect();
        let consumed_lines = self.buf[..carry_at].matches('\n').count() as u64;
        let mut carry = self.buf.split_off(carry_at);
        self.base_line += consumed_lines;
        if let Some(d) = delimiter.filter(|d| d != ";") {
            carry.insert_str(0, &format!("DELIMITER {d}\n"));
            self.base_line -= 1;
        }
        self.buf = carry;
        self.split_at = (self.buf.len() * 2).max(SPLIT_AT);
        out
    }
}

/// `COPY … FROM stdin`: the data follows the statement in the file, which only psql can send.
fn is_copy_from_stdin(sql: &str, dialect: Dialect) -> bool {
    dialect == Dialect::Postgres
        && leading_keywords(sql, 1).first().map(String::as_str) == Some("COPY")
        && contains_word(&code_only(sql, dialect).to_uppercase(), "STDIN")
}

/// The first lines of a statement, for the preview and error lists.
fn excerpt(sql: &str) -> String {
    const LINES: usize = 8;
    const CHARS: usize = 600;
    let mut out: String = sql.lines().take(LINES).collect::<Vec<_>>().join("\n");
    if out.chars().count() > CHARS {
        out = out.chars().take(CHARS).collect();
        out.push('…');
    } else if sql.lines().nth(LINES).is_some() {
        out.push_str("\n…");
    }
    out
}

/// Reads the preview (blocking: call it off the async workers).
pub fn preview_dump(path: &str, dialect: Dialect) -> Result<SqlDumpPreview, AppError> {
    let (read, size, compressed) = open_dump(path, Arc::new(AtomicU64::new(0)))?;
    let mut text = DumpText::new(read);
    let piece = text.next_text(PREVIEW_BYTES)?.unwrap_or_default();
    let eof = text.at_end();
    let mut statements = split_statements(&piece, dialect);
    // The last statement of a cut text may be partial: it is not shown.
    let cut = !eof && statements.len() > 1;
    if cut {
        statements.pop();
    }
    let more = cut || statements.len() > PREVIEW_STATEMENTS;
    let (invalid_utf8, first_invalid_line) = text.invalid();
    Ok(SqlDumpPreview {
        path: path.to_string(),
        file_name: Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size,
        compressed,
        encoding: text.encoding(),
        statements: statements
            .iter()
            .take(PREVIEW_STATEMENTS)
            .map(|s| excerpt(&restore_bytes(&s.sql, dialect)))
            .collect(),
        more,
        invalid_utf8,
        first_invalid_line,
    })
}

/// Drops whatever rows a statement of the dump returns.
struct Discard;

impl RowSink for Discard {
    fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}
    fn rows(&mut self, _rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

/// The database a psql `\connect` / `\c` meta-command switches to; `None` for another command or
/// one that keeps the database (`\c -`, only other connection settings).
fn psql_connect_target(meta: &str) -> Option<String> {
    let line = meta.trim_start().strip_prefix('\\')?;
    // psql's words: whitespace-separated, '…' and "…" quoted (doubled quotes inside).
    let mut words: Vec<String> = Vec::new();
    let mut chars = line.lines().next().unwrap_or("").chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            if c == '\'' || c == '"' {
                while let Some(d) = chars.next() {
                    if d == c {
                        if chars.peek() == Some(&c) {
                            chars.next();
                            word.push(c);
                        } else {
                            break;
                        }
                    } else {
                        word.push(d);
                    }
                }
            } else {
                word.push(c);
            }
        }
        words.push(word);
    }
    let mut words = words.into_iter();
    if !matches!(words.next().as_deref(), Some("c" | "connect")) {
        return None;
    }
    let arg = words.find(|w| !w.starts_with('-') || w == "-")?;
    if arg == "-" {
        return None;
    }
    if let Some(uri) = arg
        .strip_prefix("postgresql://")
        .or_else(|| arg.strip_prefix("postgres://"))
    {
        let path = uri.split_once('/')?.1;
        let db = path.split(['?', '#']).next().unwrap_or("");
        return (!db.is_empty()).then(|| db.to_string());
    }
    if arg.contains('=') {
        // A connection string: `dbname=x host=…`, values possibly '…' quoted.
        let mut rest = arg.as_str();
        while let Some(at) = rest.find("dbname") {
            let after = rest[at + 6..].trim_start();
            if let Some(value) = after.strip_prefix('=') {
                let value = value.trim_start();
                return Some(match value.strip_prefix('\'') {
                    Some(quoted) => quoted
                        .split_once('\'')
                        .map_or(quoted, |(v, _)| v)
                        .to_string(),
                    None => value.split_whitespace().next().unwrap_or("").to_string(),
                })
                .filter(|db| !db.is_empty());
            }
            rest = &rest[at + 6..];
        }
        return None;
    }
    Some(arg)
}

/// Whether the statement is a `COMMIT` / `END` (which PostgreSQL turns into a rollback in an
/// aborted transaction).
fn is_commit(sql: &str) -> bool {
    matches!(
        leading_keywords(sql, 1).first().map(String::as_str),
        Some("COMMIT" | "END")
    )
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The dump's own transaction while the import runs.
#[derive(Debug, Default)]
struct DumpTransaction {
    /// Statements that ran inside it (its `BEGIN` excluded).
    ran: u64,
    /// PostgreSQL: a statement failed inside it, so the server ignores the rest until its end.
    aborted: bool,
    /// Statements that failed only because of that.
    ignored: u64,
}

/// Runs a SQL dump on `session`'s editor session. `slot` is the cancel slot of the running
/// statement (`cancel_import` cancels through it); `cancel` stops between statements.
pub async fn run_sql_import(
    session: &dyn DbSession,
    req: &SqlImportRequest,
    slot: CancelSlot,
    mut progress: impl FnMut(SqlImportProgress) + Send,
    cancel: CancellationToken,
) -> Result<SqlImportReport, AppError> {
    let started = Instant::now();
    let dialect = session.dialect();
    let bytes = Arc::new(AtomicU64::new(0));
    let (read, total_bytes, _) = open_dump(&req.path, bytes.clone())?;
    let mut text = Some(DumpText::new(read));
    let mut splitter = DumpSplitter::new(dialect);
    let mut report = SqlImportReport {
        executed: 0,
        failed: 0,
        skipped: 0,
        errors: Vec::new(),
        stopped: false,
        cancelled: false,
        encoding: TextEncoding::Utf8,
        invalid_utf8: 0,
        first_invalid_line: None,
        rolled_back: None,
        notes: Vec::new(),
        duration_ms: 0,
    };
    // Other runs on this session (editor tabs) wait until the dump is done.
    let _editor = match lock_editor_runs(session, &slot).await {
        Ok(guard) => guard,
        Err(AppError::Cancelled) => {
            report.cancelled = true;
            return Ok(report);
        }
        Err(e) => return Err(e),
    };
    // psql's `\connect` to another database ends the import.
    let database = if dialect == Dialect::Postgres {
        session
            .server_info()
            .await
            .ok()
            .and_then(|i| i.current_database)
    } else {
        None
    };
    let mut last_emit = Instant::now();
    let mut emit = |report: &SqlImportReport, force: bool| {
        if force || last_emit.elapsed() >= PROGRESS_INTERVAL {
            progress(SqlImportProgress {
                statements: report.executed + report.failed,
                failed: report.failed,
                bytes: bytes.load(Ordering::Relaxed),
                total_bytes,
            });
            last_emit = Instant::now();
        }
    };
    let mut tx = DumpTransaction::default();
    let mut undone = 0u64;
    let mut rolled = false;
    let mut ignored = 0u64;

    'read: loop {
        let Some(mut t) = text.take() else { break };
        let (t, piece) = tokio::task::spawn_blocking(move || {
            let piece = t.next_text(READ_CHUNK);
            (t, piece)
        })
        .await
        .map_err(|e| AppError::internal(format!("dump reader failed: {e}")))?;
        let piece = piece?;
        let eof = piece.is_none() || t.at_end();
        report.encoding = t.encoding();
        (report.invalid_utf8, report.first_invalid_line) = t.invalid();
        let statements = splitter.push(piece.as_deref().unwrap_or_default(), eof);
        if !eof {
            text = Some(t);
        }
        for st in statements {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break 'read;
            }
            let sql = restore_bytes(&st.sql, dialect);
            if is_meta_command(&sql) {
                if dialect == Dialect::Postgres
                    && let Some(target) = psql_connect_target(&sql)
                    && database.as_deref() != Some(target.as_str())
                {
                    report.failed += 1;
                    report.errors.push(SqlImportError {
                        index: st.index,
                        line: st.line,
                        sql: excerpt(&sql),
                        message: format!(
                            "The dump switches to database \u{201c}{target}\u{201d} here; what \
                             follows belongs there, not in \u{201c}{}\u{201d}. Open a connection \
                             to \u{201c}{target}\u{201d} and import the rest there, or use File ▸ \
                             Restore….",
                            database.as_deref().unwrap_or("this database")
                        ),
                    });
                    report.stopped = true;
                    break 'read;
                }
                report.skipped += 1;
                continue;
            }
            let was_open = session.in_transaction() == Some(true);
            let outcome = if is_copy_from_stdin(&sql, dialect) {
                Err(AppError::Unsupported(
                    "COPY … FROM stdin data can only be loaded by psql: use File ▸ Restore…, or \
                     a dump made with --inserts"
                        .into(),
                ))
            } else {
                session
                    .run_statement(&sql, 1, &mut Discard, slot.clone())
                    .await
                    .map(|_| ())
            };
            let open = session.in_transaction() == Some(true);
            match outcome {
                Ok(()) => {
                    report.executed += 1;
                    if was_open && open {
                        tx.ran += 1;
                    } else if was_open {
                        // The transaction ended: a COMMIT after an error in it rolled back.
                        if tx.aborted && is_commit(&sql) {
                            undone += tx.ran;
                            rolled = true;
                            report.notes.push(format!(
                                "The COMMIT on line {} rolled the transaction back (a statement \
                                 in it had failed): {} undone.",
                                st.line,
                                plural(tx.ran, "statement was", "statements were")
                            ));
                        }
                        ignored += tx.ignored;
                        tx = DumpTransaction::default();
                    } else if open {
                        tx = DumpTransaction::default();
                    }
                }
                Err(AppError::Cancelled) => {
                    report.cancelled = true;
                    break 'read;
                }
                Err(e) => {
                    let fatal = matches!(e, AppError::Unsupported(_) | AppError::NotConnected);
                    report.failed += 1;
                    let in_aborted = was_open && tx.aborted;
                    if was_open && dialect == Dialect::Postgres {
                        tx.aborted = true;
                    }
                    if in_aborted {
                        // "current transaction is aborted": not an error of its own.
                        tx.ignored += 1;
                    } else if report.errors.len() < MAX_ERRORS {
                        report.errors.push(SqlImportError {
                            index: st.index,
                            line: st.line,
                            sql: excerpt(&sql),
                            message: e.to_string(),
                        });
                    }
                    if !open && was_open {
                        ignored += tx.ignored;
                        tx = DumpTransaction::default();
                    }
                    if req.stop_on_error || fatal {
                        report.stopped = true;
                        break 'read;
                    }
                }
            }
            emit(&report, false);
        }
        if eof {
            break;
        }
    }
    ignored += tx.ignored;
    // Never leave the dump's transaction open (on a session that may live on).
    if session.in_transaction() == Some(true) {
        match session
            .run_statement("ROLLBACK", 1, &mut Discard, CancelSlot::default())
            .await
        {
            Ok(_) => {
                undone += tx.ran;
                rolled = true;
                report.notes.push(format!(
                    "The import ended inside the dump's transaction: it was rolled back, and {} \
                     undone.",
                    plural(tx.ran, "statement was", "statements were")
                ));
            }
            Err(e) => {
                log::warn!("import: cannot roll back the dump's transaction: {e}");
                report.notes.push(format!(
                    "The import ended inside the dump's transaction, which could not be rolled \
                     back: {e}"
                ));
            }
        }
    }
    report.rolled_back = rolled.then_some(undone);
    if ignored > 0 {
        report.notes.push(format!(
            "{} failed because the transaction had already failed (PostgreSQL ignores the rest \
             of a failed transaction).",
            plural(ignored, "statement", "statements")
        ));
    }
    if report.invalid_utf8 > 0 {
        report.notes.push(format!(
            "{} not UTF-8 (the first on line {}): decoded as Windows-1252.{}",
            plural(
                report.invalid_utf8,
                "byte sequence was",
                "byte sequences were"
            ),
            report.first_invalid_line.unwrap_or(1),
            if dialect == Dialect::Mysql {
                " Values written as _binary '…' keep their bytes; other binary values may have \
                 changed: dump them with mysqldump --hex-blob."
            } else {
                ""
            }
        ));
    }
    emit(&report, true);
    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn statements(text: &str, dialect: Dialect, piece: usize) -> Vec<DumpStatement> {
        let mut s = DumpSplitter::new(dialect);
        s.split_at = 1; // split after every piece
        let mut out = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let end = (i + piece).min(chars.len());
            let part: String = chars[i..end].iter().collect();
            i = end;
            out.extend(s.push(&part, false));
            s.split_at = 1;
        }
        out.extend(s.push("", true));
        out
    }

    fn sqls(st: &[DumpStatement]) -> Vec<&str> {
        st.iter().map(|s| s.sql.as_str()).collect()
    }

    #[test]
    fn splits_across_pieces_with_lines() {
        let dump = "-- header\nCREATE TABLE t (a int, b text);\nINSERT INTO t VALUES (1, 'x;\ny');\n\nINSERT INTO t VALUES (2, 'z');\n";
        for piece in [1, 3, 7, 1000] {
            let st = statements(dump, Dialect::Postgres, piece);
            assert_eq!(
                sqls(&st),
                [
                    "CREATE TABLE t (a int, b text)",
                    "INSERT INTO t VALUES (1, 'x;\ny')",
                    "INSERT INTO t VALUES (2, 'z')"
                ],
                "piece {piece}"
            );
            assert_eq!(st.iter().map(|s| s.line).collect::<Vec<_>>(), [2, 3, 6]);
            assert_eq!(st.iter().map(|s| s.index).collect::<Vec<_>>(), [0, 1, 2]);
        }
    }

    #[test]
    fn keeps_mysql_delimiters_across_pieces() {
        let dump = "SET NAMES utf8mb4;\nDELIMITER $$\nCREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND$$\nCREATE PROCEDURE q()\nBEGIN\n  SELECT 3;\nEND$$\nDELIMITER ;\nCALL p();\nCALL q();\n";
        for piece in [2, 5, 11, 1000] {
            let st = statements(dump, Dialect::Mysql, piece);
            assert_eq!(
                sqls(&st),
                [
                    "SET NAMES utf8mb4",
                    "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND",
                    "CREATE PROCEDURE q()\nBEGIN\n  SELECT 3;\nEND",
                    "CALL p()",
                    "CALL q()"
                ],
                "piece {piece}"
            );
            assert_eq!(
                st.iter().map(|s| s.line).collect::<Vec<_>>(),
                [1, 3, 8, 13, 14],
                "piece {piece}"
            );
        }
    }

    #[test]
    fn psql_meta_commands_are_statements_of_their_own() {
        let st = statements(
            "\\restrict abc\nSELECT 1;\n\\connect db\nSELECT 2;\n",
            Dialect::Postgres,
            4,
        );
        let flags: Vec<bool> = st.iter().map(|s| is_meta_command(&s.sql)).collect();
        assert_eq!(flags, [true, false, true, false]);
        assert_eq!(st[3].line, 4);
    }

    #[test]
    fn decodes_utf8_with_windows_1252_for_invalid_bytes_only() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("select 'é';\n".as_bytes());
        let mut t = DumpText::new(bytes.as_slice());
        assert_eq!(t.next_text(1024).unwrap().unwrap(), "select 'é';\n");
        assert_eq!(t.encoding(), TextEncoding::Utf8);
        assert_eq!(t.invalid(), (0, None));
        assert!(t.next_text(1024).unwrap().is_none());

        // A character cut between reads stays whole.
        let text = "aaaé b";
        let mut t = DumpText::new(text.as_bytes());
        let mut out = String::new();
        while let Some(p) = t.next_text(3).unwrap() {
            out.push_str(&p);
        }
        assert_eq!(out, text);
        assert_eq!(t.encoding(), TextEncoding::Utf8);

        // Only the invalid bytes are Windows-1252: UTF-8 after them stays UTF-8.
        let mixed: &[u8] = b"-- x\ninsert into t values ('caf\xE9 \x80');\ninsert into t values ('\xC3\xA9t\xC3\xA9');";
        let mut t = DumpText::new(mixed);
        let mut out = String::new();
        while let Some(p) = t.next_text(7).unwrap() {
            out.push_str(&p);
        }
        assert_eq!(
            restore_bytes(&out, Dialect::Postgres),
            "-- x\ninsert into t values ('café €');\ninsert into t values ('été');"
        );
        assert_eq!(t.encoding(), TextEncoding::Windows1252);
        assert_eq!(t.invalid(), (2, Some(2)));
        // A sequence cut by the end of the file is invalid too.
        let mut t = DumpText::new(&b"a\xE2\x82"[..]);
        let out = t.next_text(1024).unwrap().unwrap();
        assert_eq!(t.invalid(), (1, Some(1)));
        assert_eq!(restore_bytes(&out, Dialect::Sqlite), "aâ\u{201A}");
    }

    #[test]
    fn mysql_binary_literals_keep_their_bytes() {
        // mysqldump without --hex-blob: raw bytes, escaped quote / backslash / NUL / \Z.
        let raw: &[u8] = b"INSERT INTO `b` VALUES (1,_binary '\xFF\x00\\'\\\\\\0\\Z\xC3\xA9','caf\xE9'),(2,_binary 'ok','x');";
        let mut t = DumpText::new(raw);
        let text = t.next_text(1024).unwrap().unwrap();
        let sql = restore_bytes(&text, Dialect::Mysql);
        assert_eq!(
            sql,
            "INSERT INTO `b` VALUES (1,_binary X'FF00275C001AC3A9','café'),(2,_binary 'ok','x');"
        );
        // Not in a string or comment; `_BINARY` in any case; doubled quotes.
        let text = format!(
            "SELECT '_binary ''{}''' /* _binary '{}' */, _BINARY  'a''{}'",
            raw_byte_char(0x80),
            raw_byte_char(0x81),
            raw_byte_char(0x82)
        );
        assert_eq!(
            restore_bytes(&text, Dialect::Mysql),
            "SELECT '_binary ''€''' /* _binary '\u{81}' */, _BINARY  X'612782'"
        );
        // PostgreSQL has no _binary: Windows-1252 text.
        assert_eq!(
            restore_bytes(
                &format!("_binary '{}'", raw_byte_char(0xE9)),
                Dialect::Postgres
            ),
            "_binary 'é'"
        );
        assert!(matches!(
            restore_bytes("SELECT 1", Dialect::Mysql),
            Cow::Borrowed(_)
        ));
        assert_eq!(
            mysql_string_bytes(&"a\\%\\n\\x".chars().collect::<Vec<_>>()),
            b"a\\%\nx"
        );
    }

    #[test]
    fn psql_connect_targets() {
        assert_eq!(psql_connect_target("\\connect shop"), Some("shop".into()));
        assert_eq!(psql_connect_target("\\c \"My Db\""), Some("My Db".into()));
        assert_eq!(
            psql_connect_target("\\connect -reuse-previous=on \"dbname='sales db'\""),
            Some("sales db".into())
        );
        assert_eq!(
            psql_connect_target("\\c postgresql://u@h:5432/app?sslmode=require"),
            Some("app".into())
        );
        assert_eq!(psql_connect_target("\\c - otheruser"), None);
        assert_eq!(psql_connect_target("\\restrict abc"), None);
        assert_eq!(psql_connect_target("\\copyright"), None);
        assert!(is_commit("commit") && is_commit("END;") && !is_commit("ROLLBACK"));
    }

    #[test]
    fn previews_plain_and_gzip_files() {
        let dir = tempfile::tempdir().unwrap();
        let sql: String = (0..30)
            .map(|i| format!("INSERT INTO t VALUES ({i});\n"))
            .collect();
        let plain = dir.path().join("d.sql");
        std::fs::write(&plain, &sql).unwrap();
        let p = preview_dump(&plain.to_string_lossy(), Dialect::Sqlite).unwrap();
        assert_eq!(p.statements.len(), PREVIEW_STATEMENTS);
        assert_eq!(p.statements[0], "INSERT INTO t VALUES (0)");
        assert!(p.more);
        assert!(!p.compressed);
        assert_eq!(p.size, sql.len() as u64);

        let gz = dir.path().join("d.sql.gz");
        let mut enc =
            flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::fast());
        enc.write_all(b"CREATE TABLE x (a);\nSELECT 1;").unwrap();
        enc.finish().unwrap();
        let p = preview_dump(&gz.to_string_lossy(), Dialect::Sqlite).unwrap();
        assert!(p.compressed);
        assert_eq!(p.statements, ["CREATE TABLE x (a)", "SELECT 1"]);
        assert!(!p.more);
    }

    #[test]
    fn excerpts_and_copy_detection() {
        let long: String = (0..20).map(|i| format!("line {i}\n")).collect();
        assert!(excerpt(&long).ends_with("\n…"));
        assert_eq!(excerpt("select 1"), "select 1");
        assert!(is_copy_from_stdin(
            "COPY public.t (a, b) FROM stdin",
            Dialect::Postgres
        ));
        assert!(!is_copy_from_stdin(
            "COPY t FROM '/tmp/x.csv'",
            Dialect::Postgres
        ));
        assert!(!is_copy_from_stdin(
            "SELECT 'COPY FROM stdin'",
            Dialect::Postgres
        ));
        assert_eq!(delimiter_in("  delimiter //\n"), Some("//".into()));
        assert_eq!(delimiter_in("-- DELIMITER $$\n"), None);
        assert_eq!(byte_offsets("aé b", &[0, 2, 3, 9]), vec![0, 3, 4, 5]);
    }
}
