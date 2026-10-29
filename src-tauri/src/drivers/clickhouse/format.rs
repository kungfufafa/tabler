//! Reading a ClickHouse HTTP reply: the `JSONCompactEachRowWithNamesAndTypes` body as it arrives
//! (a line of names, a line of types, then one JSON array per row), the errors, and the summary.
//!
//! HTTP 200 does not mean success: the server sends the status before it runs the query, so an
//! error during the run is written into the body after the rows already sent. Three forms are
//! recognized, from newest to oldest server:
//!
//! - the tagged block, `\r\n__exception__\r\n<TAG>\r\n<message>\r\n<length> <TAG>\r\n__exception__\r\n`,
//!   whose tag the `X-ClickHouse-Exception-Tag` header announced;
//! - a JSON object `{"exception": "Code: …"}` on a line of its own;
//! - the bare text `Code: N. DB::Exception: …` after the last row, possibly glued to a row cut
//!   short.
//!
//! A line that is no row switches the reader to the end of the body, which must then hold one of
//! those; anything else is a protocol error. Either way a result is never taken for complete
//! when it is not.

use std::collections::VecDeque;

use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::value::Value;

use super::decode::{decode, result_column};
use super::json::{self, JsonVal};
use super::transport::{ResponseHead, header};
use super::types::ChType;

/// The text after a bad line that is looked through for an error before giving up; also how
/// much of its start and of its end is kept.
const TRAILER_MAX: usize = 64 * 1024;
/// How far a trailer that names an error is read before giving up: an exception block is
/// small, and output in another format may mention `Code: ` and go on without end.
const TRAILER_LIMIT: usize = 1024 * 1024;
/// The longest line of names or of types: output in another format may hold no line break.
const HEADER_LINE_MAX: usize = 16 * 1024 * 1024;
/// What marks an error in a trailer.
const MARKERS: &[&[u8]] = &[b"__exception__", b"DB::Exception", b"Code: "];

/// The bad line and what follows it, kept bounded: its start, its end, and whether an error
/// marker was seen anywhere in between.
#[derive(Default)]
struct Trailer {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    /// Bytes between `head` and `tail` were dropped.
    cut: bool,
    len: usize,
    marked: bool,
    /// The last bytes seen, so a marker split across pieces is found.
    carry: Vec<u8>,
}

impl Trailer {
    fn push(&mut self, bytes: &[u8]) {
        self.len += bytes.len();
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(bytes);
        self.marked |= MARKERS.iter().any(|m| contains(&window, m));
        let keep = MARKERS.iter().map(|m| m.len()).max().unwrap_or(1) - 1;
        self.carry = window[window.len().saturating_sub(keep)..].to_vec();
        let room = TRAILER_MAX.saturating_sub(self.head.len());
        let (to_head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(to_head);
        self.tail.extend(rest);
        if self.tail.len() > TRAILER_MAX {
            self.tail.drain(..self.tail.len() - TRAILER_MAX);
            self.cut = true;
        }
    }

    /// The kept text, with a mark where the middle was dropped.
    fn text(&self) -> Vec<u8> {
        let mut out = self.head.clone();
        if self.cut {
            out.extend_from_slice(b"\n\xE2\x80\xA6\n");
        }
        out.extend(&self.tail);
        out
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// What the reader found in a piece of the body.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The result's columns (once, before any row).
    Columns(Vec<ResultColumn>),
    Row(Vec<Value>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Names,
    Types,
    Rows,
    /// After a line that is no row: everything else is collected for `finish`.
    Trailer,
}

pub struct ResultReader {
    state: State,
    /// Bytes of the current, unfinished line.
    line: Vec<u8>,
    names: Vec<String>,
    types: Vec<ChType>,
    /// The bad line and what follows it.
    trailer: Trailer,
    tag: Option<String>,
}

impl ResultReader {
    /// `head`: the reply's headers (for the exception tag).
    pub fn new(head: &ResponseHead) -> ResultReader {
        ResultReader {
            state: State::Names,
            line: Vec::new(),
            names: Vec::new(),
            types: Vec::new(),
            trailer: Trailer::default(),
            tag: head
                .header(header::EXCEPTION_TAG)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        }
    }

    /// Reads a piece of the body; `out` gets the columns and rows it completes.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) -> Result<(), AppError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            if self.state == State::Trailer {
                self.trailer.push(rest);
                if self.trailer.len > TRAILER_MAX && !self.trailer.marked {
                    return Err(unreadable(&self.trailer.text()));
                }
                if self.trailer.len > TRAILER_LIMIT {
                    return Err(trailer_error(&self.trailer.text(), self.tag.as_deref()));
                }
                return Ok(());
            }
            match rest.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    self.line.extend_from_slice(&rest[..i]);
                    rest = &rest[i + 1..];
                    let line = std::mem::take(&mut self.line);
                    self.take_line(line, out)?;
                    if self.state == State::Trailer {
                        // What follows the bad line belongs to the trailer.
                        self.trailer.push(b"\n");
                    }
                }
                None => {
                    self.line.extend_from_slice(rest);
                    rest = &[];
                    if matches!(self.state, State::Names | State::Types)
                        && self.line.len() > HEADER_LINE_MAX
                    {
                        return Err(unreadable(&self.line[..TRAILER_MAX]));
                    }
                }
            }
        }
        Ok(())
    }

    /// The end of the body: an error found in it, or a body cut short. `Ok(true)` when a result
    /// set was announced (a statement without output sends an empty body).
    pub fn finish(mut self, out: &mut Vec<Event>) -> Result<bool, AppError> {
        if self.state != State::Trailer && !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.take_line(line, out)?;
        }
        if self.state == State::Trailer {
            return Err(trailer_error(&self.trailer.text(), self.tag.as_deref()));
        }
        match self.state {
            State::Names => Ok(false),
            State::Types => Err(AppError::db(
                "ClickHouse ended the result after the column names",
            )),
            _ => Ok(true),
        }
    }

    fn take_line(&mut self, mut line: Vec<u8>, out: &mut Vec<Event>) -> Result<(), AppError> {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8_lossy(&line);
        let parsed = (!text.trim().is_empty())
            .then(|| json::parse(&text).ok())
            .flatten();
        match (self.state, parsed) {
            (State::Names, Some(JsonVal::Arr(items))) => match strings(&items) {
                Some(names) => {
                    self.names = names;
                    self.state = State::Types;
                }
                None => self.start_trailer(line),
            },
            (State::Types, Some(JsonVal::Arr(items))) => match strings(&items) {
                Some(types) if types.len() == self.names.len() => {
                    let mut columns = Vec::with_capacity(types.len());
                    for (name, ty) in self.names.iter().zip(&types) {
                        let (column, parsed) = result_column(name, ty);
                        columns.push(column);
                        self.types.push(parsed);
                    }
                    out.push(Event::Columns(columns));
                    self.state = State::Rows;
                }
                _ => self.start_trailer(line),
            },
            (State::Rows, Some(JsonVal::Arr(cells))) if cells.len() == self.types.len() => {
                let row = cells
                    .iter()
                    .zip(&self.types)
                    .map(|(cell, ty)| decode(ty, cell))
                    .collect();
                out.push(Event::Row(row));
            }
            _ => self.start_trailer(line),
        }
        Ok(())
    }

    fn start_trailer(&mut self, line: Vec<u8>) {
        self.state = State::Trailer;
        self.trailer = Trailer::default();
        self.trailer.push(&line);
    }
}

fn strings(items: &[JsonVal]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|i| match i {
            JsonVal::Str(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn unreadable(trailer: &[u8]) -> AppError {
    let text = String::from_utf8_lossy(trailer);
    let excerpt: String = text.chars().take(200).collect();
    AppError::db(format!(
        "ClickHouse sent a result Tabler cannot read (a FORMAT clause in the statement changes \
         it): {}",
        excerpt.trim()
    ))
}

/// The error a trailer holds, or a protocol error when it holds none.
fn trailer_error(trailer: &[u8], tag: Option<&str>) -> AppError {
    let text = String::from_utf8_lossy(trailer);
    if let Some(message) = tagged_message(&text, tag) {
        return exception_error(&message, None);
    }
    for line in text.lines() {
        if let Ok(v) = json::parse(line)
            && let Some(JsonVal::Str(message)) = v.get("exception")
        {
            return exception_error(message, None);
        }
    }
    if let Some(at) = text.find("Code: ") {
        return exception_error(&text[at..], None);
    }
    if text.contains("DB::Exception") {
        return exception_error(&text, None);
    }
    unreadable(trailer)
}

/// The message of a tagged exception block (`tag` the header's, when the reply had one).
fn tagged_message(text: &str, tag: Option<&str>) -> Option<String> {
    const MARK: &str = "__exception__";
    // The first mark followed by this reply's tag: another one is the text of a value that
    // merely says `__exception__`.
    let (found_tag, rest) = text.match_indices(MARK).find_map(|(at, _)| {
        let body = text[at + MARK.len()..].trim_start_matches(['\r', '\n']);
        // The tag line, then the message, then `<length> <tag>` and the closing mark.
        let (tag_line, rest) = body.split_once('\n')?;
        let found_tag = tag_line.trim();
        (!tag.is_some_and(|t| t != found_tag)).then_some((found_tag, rest))
    })?;
    let end = rest.rfind(MARK).unwrap_or(rest.len());
    let inner = &rest[..end];
    // `<message>\r\n<length> <tag>\r\n`: use the length when it holds.
    let trimmed = inner.trim_end_matches(['\r', '\n']);
    let message = match trimmed.rsplit_once('\n') {
        Some((msg, last)) if last.trim().ends_with(found_tag) => {
            let msg = msg.trim_end_matches('\r');
            let len = last
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok());
            match len {
                Some(n) if n <= msg.len() && msg.is_char_boundary(n) => msg[..n].to_string(),
                _ => msg.to_string(),
            }
        }
        _ => trimmed.to_string(),
    };
    Some(message)
}

/// `Code: 60. DB::Exception: Table default.x does not exist. (UNKNOWN_TABLE) (version 24.3.1)`
/// split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionText {
    pub code: Option<i64>,
    /// `UNKNOWN_TABLE`
    pub name: Option<String>,
    /// The message without the code, the name and the version.
    pub message: String,
}

pub fn parse_exception(text: &str) -> ExceptionText {
    let text = text.trim();
    let mut rest = text;
    let mut code = None;
    if let Some(after) = rest.strip_prefix("Code: ") {
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        code = digits.parse().ok();
        rest = after[digits.len()..].trim_start_matches(['.', ' ']);
    }
    for prefix in ["DB::Exception: ", "DB::NetException: ", "Poco::Exception. "] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r;
            break;
        }
    }
    let mut message = rest.trim();
    // `… (version 24.3.1.2672 (official build))` at the end.
    if let Some(at) = message.rfind("(version ") {
        message = message[..at].trim_end();
    }
    // `… (UNKNOWN_TABLE)` at the end.
    let mut name = None;
    if message.ends_with(')')
        && let Some(open) = message.rfind(" (")
    {
        let candidate = &message[open + 2..message.len() - 1];
        if !candidate.is_empty()
            && candidate
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            name = Some(candidate.to_string());
            message = message[..open].trim_end();
        }
    }
    ExceptionText {
        code,
        name,
        message: if message.is_empty() {
            text.to_string()
        } else {
            message.to_string()
        },
    }
}

/// Error codes of the server (`src/Common/ErrorCodes.cpp`) the driver treats specially.
pub mod code {
    pub const READONLY: i64 = 164;
    pub const UNKNOWN_USER: i64 = 192;
    pub const WRONG_PASSWORD: i64 = 193;
    pub const REQUIRED_PASSWORD: i64 = 194;
    pub const IP_ADDRESS_NOT_ALLOWED: i64 = 195;
    pub const UNKNOWN_SETTING: i64 = 115;
    pub const QUERY_WAS_CANCELLED: i64 = 394;
    pub const SESSION_NOT_FOUND: i64 = 372;
    pub const SESSION_IS_LOCKED: i64 = 373;
    pub const AUTHENTICATION_FAILED: i64 = 516;
    pub const SETTING_CONSTRAINT_VIOLATION: i64 = 452;
}

/// The `AppError` of an exception text (`header_code`: `X-ClickHouse-Exception-Code`, when the
/// reply had one).
pub fn exception_error(text: &str, header_code: Option<i64>) -> AppError {
    let parsed = parse_exception(text);
    let vendor_code = parsed.code.or(header_code);
    let message = match &parsed.name {
        Some(name) => format!("{} ({name})", parsed.message),
        None => parsed.message.clone(),
    };
    match vendor_code {
        Some(
            code::UNKNOWN_USER
            | code::WRONG_PASSWORD
            | code::REQUIRED_PASSWORD
            | code::AUTHENTICATION_FAILED
            | code::IP_ADDRESS_NOT_ALLOWED,
        ) => AppError::Auth(message),
        Some(code::QUERY_WAS_CANCELLED) => AppError::Cancelled,
        _ => AppError::Db {
            message,
            detail: None,
            hint: hint(vendor_code, &parsed.message),
            sqlstate: None,
            vendor_code,
            position: None,
            statement_index: None,
        },
    }
}

fn hint(code: Option<i64>, message: &str) -> Option<String> {
    match code {
        Some(code::READONLY) if message.to_ascii_lowercase().contains("cannot modify") => Some(
            "Read-only mode forbids changing this setting: a read-only connection keeps \
             readonly=2, and a readonly=1 profile forbids every setting."
                .into(),
        ),
        Some(code::READONLY) => Some("The connection is read-only.".into()),
        Some(code::SESSION_IS_LOCKED) => {
            Some("Another statement still runs in this session; wait for it or stop it.".into())
        }
        _ => None,
    }
}

/// A setting the server would not let a request change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRefusal {
    /// Read-only mode refused it (`Cannot modify 'x' setting in readonly mode`); otherwise a
    /// constraint of the profile did (`Setting x should not be changed`, `Setting x shouldn't
    /// be less than 1`, `Setting x is not allowed to be set by query`).
    pub readonly_mode: bool,
    /// The setting named, when the message names one.
    pub setting: Option<String>,
}

/// What `e` says when it refuses a setting. Whose setting it was (the request's URL, the
/// statement's own SETTINGS clause, a `SET` earlier in the session) it does not say.
pub fn settings_refusal(e: &AppError) -> Option<SettingsRefusal> {
    let AppError::Db {
        vendor_code: Some(code),
        message,
        ..
    } = e
    else {
        return None;
    };
    let lower = message.to_ascii_lowercase();
    let readonly_mode = match *code {
        code::READONLY if lower.contains("cannot modify") => true,
        code::READONLY | code::SETTING_CONSTRAINT_VIOLATION if lower.contains("setting ") => false,
        _ => return None,
    };
    Some(SettingsRefusal {
        readonly_mode,
        setting: refused_setting(message),
    })
}

/// The setting a refusal names: `Cannot modify 'x' setting …` or `Setting x …`, in any case.
fn refused_setting(message: &str) -> Option<String> {
    // ASCII lower-casing keeps every byte offset.
    let lower = message.to_ascii_lowercase();
    let name: String = if let Some(at) = lower.find("cannot modify '") {
        message[at + "cannot modify '".len()..]
            .split('\'')
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        let at = lower.find("setting ")?;
        message[at + "setting ".len()..]
            .trim_start_matches(['\'', '`', '"'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect()
    };
    (!name.is_empty()).then_some(name)
}

/// The error of a reply whose status is not 200.
pub fn status_error(head: &ResponseHead, body: &[u8]) -> AppError {
    let text = String::from_utf8_lossy(body);
    let header_code = head
        .header(header::EXCEPTION_CODE)
        .and_then(|c| c.trim().parse::<i64>().ok());
    let looks_like_clickhouse = header_code.is_some() || text.contains("DB::Exception");
    if looks_like_clickhouse {
        return exception_error(&text, header_code);
    }
    let excerpt: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt: String = excerpt.chars().take(300).collect();
    let said = if excerpt.is_empty() {
        String::new()
    } else {
        format!(": {excerpt}")
    };
    match head.status {
        401 | 403 => AppError::Auth(format!(
            "ClickHouse refused the credentials (HTTP {}){said}",
            head.status
        )),
        404 => AppError::NotFound(format!("ClickHouse HTTP endpoint (HTTP 404){said}")),
        408 | 504 => AppError::Timeout,
        502 | 503 => AppError::Connect(format!(
            "ClickHouse is unavailable (HTTP {}){said}",
            head.status
        )),
        s => AppError::db(format!("ClickHouse answered HTTP {s}{said}")),
    }
}

/// `X-ClickHouse-Summary`: what the statement read and wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub read_rows: u64,
    pub read_bytes: u64,
    pub written_rows: u64,
    pub written_bytes: u64,
    pub total_rows_to_read: u64,
    pub result_rows: u64,
    pub result_bytes: u64,
    pub elapsed_ns: u64,
}

impl Summary {
    /// None without the header or when it is not JSON. Values come as strings (or numbers).
    pub fn from_head(head: &ResponseHead) -> Option<Summary> {
        let v = json::parse(head.header(header::SUMMARY)?).ok()?;
        let n = |k: &str| {
            v.get(k)
                .and_then(JsonVal::as_scalar_text)
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(0)
        };
        Some(Summary {
            read_rows: n("read_rows"),
            read_bytes: n("read_bytes"),
            written_rows: n("written_rows"),
            written_bytes: n("written_bytes"),
            total_rows_to_read: n("total_rows_to_read"),
            result_rows: n("result_rows"),
            result_bytes: n("result_bytes"),
            elapsed_ns: n("elapsed_ns"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(headers: &[(&str, &str)]) -> ResponseHead {
        ResponseHead {
            status: 200,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// Feeds `body` in pieces of `step` bytes.
    fn read(h: &ResponseHead, body: &[u8], step: usize) -> (Vec<Event>, Result<bool, AppError>) {
        let mut r = ResultReader::new(h);
        let mut out = Vec::new();
        for piece in body.chunks(step.max(1)) {
            if let Err(e) = r.feed(piece, &mut out) {
                return (out, Err(e));
            }
        }
        let end = r.finish(&mut out);
        (out, end)
    }

    const ROWS: &str = "[\"id\",\"name\",\"ts\"]\n\
        [\"UInt64\",\"LowCardinality(Nullable(String))\",\"DateTime64(3, 'Asia/Jakarta')\"]\n\
        [\"9007199254740993\",\"a\",\"2024-01-01 00:00:00.000\"]\n\
        [\"2\",null,\"2024-01-02 00:00:00.500\"]\n";

    #[test]
    fn rows_arrive_whole_in_any_pieces() {
        for step in [1, 2, 7, 64, 4096] {
            let (events, end) = read(&head(&[]), ROWS.as_bytes(), step);
            assert!(end.unwrap(), "step {step}");
            assert_eq!(events.len(), 3);
            let Event::Columns(cols) = &events[0] else {
                panic!("{events:?}")
            };
            assert_eq!(cols[2].data_type, "DateTime64(3, 'Asia/Jakarta')");
            assert_eq!(
                events[1],
                Event::Row(vec![
                    Value::BigInt("9007199254740993".into()),
                    Value::Text("a".into()),
                    Value::Timestamp("2024-01-01 00:00:00.000".into()),
                ])
            );
            assert_eq!(
                events[2],
                Event::Row(vec![
                    Value::Int(2),
                    Value::Null,
                    Value::Timestamp("2024-01-02 00:00:00.500".into()),
                ])
            );
        }
        // CRLF line ends and a last line without one.
        let crlf = ROWS.replace('\n', "\r\n");
        let (events, end) = read(&head(&[]), crlf.trim_end().as_bytes(), 5);
        assert!(end.unwrap());
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn empty_bodies_and_empty_results() {
        let (events, end) = read(&head(&[]), b"", 1);
        assert!(!end.unwrap());
        assert!(events.is_empty());
        let (events, end) = read(&head(&[]), b"[\"a\"]\n[\"UInt8\"]\n", 3);
        assert!(end.unwrap());
        assert_eq!(events.len(), 1);
        let (_, end) = read(&head(&[]), b"[\"a\"]\n", 3);
        assert!(end.is_err());
    }

    #[test]
    fn a_tagged_exception_after_some_rows() {
        let tag = "aBcDeFgHiJkLmNoP";
        let msg = "Code: 395. DB::Exception: Value passed to 'throwIf' function is non-zero: \
                   while executing 'FUNCTION throwIf(x)'. (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) \
                   (version 25.11.1.1)";
        // The block cuts a row short.
        let body = format!(
            "[\"x\"]\n[\"UInt8\"]\n[1]\n[2]\n[3\r\n__exception__\r\n{tag}\r\n{msg}\r\n{} {tag}\r\n__exception__\r\n",
            msg.len()
        );
        for step in [1, 3, 50, 10_000] {
            let (events, end) = read(
                &head(&[(header::EXCEPTION_TAG, tag)]),
                body.as_bytes(),
                step,
            );
            assert_eq!(
                events.len(),
                3,
                "step {step}: the rows before the error stay"
            );
            match end.unwrap_err() {
                AppError::Db {
                    message,
                    vendor_code,
                    ..
                } => {
                    assert_eq!(vendor_code, Some(395));
                    assert!(
                        message.starts_with("Value passed to 'throwIf'"),
                        "{message}"
                    );
                    assert!(message.ends_with("(FUNCTION_THROW_IF_VALUE_IS_NON_ZERO)"));
                    assert!(!message.contains("version"));
                }
                other => panic!("{other:?}"),
            }
        }
        // A message holding line breaks keeps them.
        let multi = "Code: 1. DB::Exception: line one\nline two. (X)";
        let body = format!(
            "[\"x\"]\n[\"UInt8\"]\n\r\n__exception__\r\n{tag}\r\n{multi}\r\n{} {tag}\r\n__exception__\r\n",
            multi.len()
        );
        let (_, end) = read(&head(&[(header::EXCEPTION_TAG, tag)]), body.as_bytes(), 4);
        assert!(end.unwrap_err().to_string().contains("line one\nline two"));
    }

    #[test]
    fn older_exception_forms() {
        // Plain text after the rows (servers before the tagged block).
        let body = "[\"x\"]\n[\"UInt8\"]\n[1]\nCode: 241. DB::Exception: Memory limit (total) \
                    exceeded. (MEMORY_LIMIT_EXCEEDED) (version 23.3.1.1)\n";
        let (events, end) = read(&head(&[]), body.as_bytes(), 9);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            end.unwrap_err(),
            AppError::Db {
                vendor_code: Some(241),
                ..
            }
        ));
        // Glued to a row cut short.
        let body = "[\"x\"]\n[\"String\"]\n[\"abCode: 159. DB::Exception: Timeout exceeded\n";
        let (_, end) = read(&head(&[]), body.as_bytes(), 2);
        assert!(matches!(
            end.unwrap_err(),
            AppError::Db {
                vendor_code: Some(159),
                ..
            }
        ));
        // As a JSON object.
        let body = "[\"x\"]\n[\"UInt8\"]\n{\"exception\": \"Code: 60. DB::Exception: Table x does not exist. (UNKNOWN_TABLE)\"}\n";
        let (_, end) = read(&head(&[]), body.as_bytes(), 6);
        assert_eq!(
            end.unwrap_err().to_string(),
            "Table x does not exist. (UNKNOWN_TABLE)"
        );
        // Cancelled on the server.
        let body = "[\"x\"]\n[\"UInt8\"]\nCode: 394. DB::Exception: Query was cancelled. (QUERY_WAS_CANCELLED)";
        let (_, end) = read(&head(&[]), body.as_bytes(), 100);
        assert!(matches!(end.unwrap_err(), AppError::Cancelled));
    }

    #[test]
    fn a_value_that_mentions_an_exception_is_still_a_value() {
        let body = "[\"x\"]\n[\"String\"]\n[\"__exception__\"]\n[\"Code: 1. DB::Exception: no\"]\n";
        let (events, end) = read(&head(&[(header::EXCEPTION_TAG, "T")]), body.as_bytes(), 3);
        assert!(end.unwrap());
        assert_eq!(events.len(), 3);
        // A block with another tag is not this reply's.
        let body = "[\"x\"]\n[\"UInt8\"]\n\r\n__exception__\r\nOTHER\r\nboom\r\n";
        let (_, end) = read(&head(&[(header::EXCEPTION_TAG, "T")]), body.as_bytes(), 3);
        let e = end.unwrap_err().to_string();
        assert!(e.contains("cannot read"), "{e}");
    }

    #[test]
    fn output_in_another_format_is_refused() {
        let (_, end) = read(&head(&[]), b"1,\"a\"\n2,\"b\"\n", 4);
        let e = end.unwrap_err().to_string();
        assert!(e.contains("FORMAT clause"), "{e}");
        // A huge body in another format stops early instead of being kept.
        let big = "x,y\n".repeat(50_000);
        let mut r = ResultReader::new(&head(&[]));
        let mut out = Vec::new();
        let mut failed = false;
        for piece in big.as_bytes().chunks(1000) {
            if r.feed(piece, &mut out).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
        // A row of the wrong width is no row.
        let (_, end) = read(&head(&[]), b"[\"a\"]\n[\"UInt8\"]\n[1,2]\n", 4);
        assert!(end.is_err());
    }

    #[test]
    fn exception_texts() {
        let e = parse_exception(
            "Code: 516. DB::Exception: alice: Authentication failed: password is incorrect, or \
             there is no user with such name. (AUTHENTICATION_FAILED) (version 24.8.4.13 (official build))\n",
        );
        assert_eq!(e.code, Some(516));
        assert_eq!(e.name.as_deref(), Some("AUTHENTICATION_FAILED"));
        assert!(e.message.starts_with("alice: Authentication failed"));
        assert!(matches!(
            exception_error("Code: 516. DB::Exception: x", None),
            AppError::Auth(_)
        ));
        let plain = parse_exception("something odd");
        assert_eq!(plain.code, None);
        assert_eq!(plain.message, "something odd");
        // A message ending in parentheses that are no error name keeps them.
        assert_eq!(
            parse_exception("Code: 62. DB::Exception: Syntax error at (end)").message,
            "Syntax error at (end)"
        );
        let ro = exception_error(
            "Code: 164. DB::Exception: Cannot modify 'output_format_json_quote_decimals' setting \
             in readonly mode. (READONLY)",
            None,
        );
        assert_eq!(
            settings_refusal(&ro),
            Some(SettingsRefusal {
                readonly_mode: true,
                setting: Some("output_format_json_quote_decimals".into()),
            })
        );
        assert_eq!(
            settings_refusal(&exception_error(
                "Code: 164. DB::Exception: cannot modify 'readonly' setting in readonly mode",
                None
            ))
            .and_then(|r| r.setting),
            Some("readonly".into())
        );
        assert_eq!(
            settings_refusal(&exception_error(
                "Code: 164. DB::Exception: alice: Cannot execute query in readonly mode. (READONLY)",
                None
            )),
            None
        );
        // Constraints: the server writes "Setting", capitalized.
        for (text, name) in [
            (
                "Setting output_format_json_quote_decimals should not be changed",
                "output_format_json_quote_decimals",
            ),
            (
                "Setting max_threads shouldn't be less than 1",
                "max_threads",
            ),
            (
                "Setting max_threads shouldn't be greater than 8",
                "max_threads",
            ),
            (
                "Setting max_threads should not be changed: its constraint leaves out the                  minimum 1, which would lift it",
                "max_threads",
            ),
        ] {
            let e = exception_error(
                &format!("Code: 452. DB::Exception: {text}. (SETTING_CONSTRAINT_VIOLATION)"),
                None,
            );
            assert_eq!(
                settings_refusal(&e),
                Some(SettingsRefusal {
                    readonly_mode: false,
                    setting: Some(name.into()),
                }),
                "{text}"
            );
        }
        assert_eq!(
            settings_refusal(&exception_error(
                "Code: 60. DB::Exception: Table default.setting x does not exist",
                None
            )),
            None
        );
    }

    #[test]
    fn a_trailer_is_kept_bounded() {
        // Output in another format whose first line mentions `Code: ` and goes on: the reader
        // keeps only the start and the end, and gives up after TRAILER_LIMIT.
        let mut r = ResultReader::new(&head(&[]));
        let mut out = Vec::new();
        let mut failed = None;
        let line = "Code: 1,\"x\"
"
        .repeat(1000);
        for _ in 0..1000 {
            if let Err(e) = r.feed(line.as_bytes(), &mut out) {
                failed = Some(e);
                break;
            }
            assert!(r.trailer.head.len() + r.trailer.tail.len() <= 2 * TRAILER_MAX);
        }
        assert!(failed.is_some());
        assert!(r.trailer.len <= TRAILER_LIMIT + line.len());
        // A marker split across pieces counts.
        let mut r = ResultReader::new(&head(&[]));
        r.feed(
            b"not a row
DB::Exc",
            &mut out,
        )
        .unwrap();
        assert!(!r.trailer.marked);
        r.feed(
            b"eption: boom
",
            &mut out,
        )
        .unwrap();
        assert!(r.trailer.marked);
        // The real block at the end of a long trailer is still found, past a value that
        // mentions the mark.
        let block = "Code: 241. DB::Exception: Memory limit exceeded. (MEMORY_LIMIT_EXCEEDED)";
        let body = format!(
            "[\"x\"]
[\"String\"]
__exception__ in a bad line
{}
__exception__
T
             {block}
{} T
__exception__
",
            "y".repeat(3 * TRAILER_MAX),
            block.len()
        );
        let (_, end) = read(
            &head(&[(header::EXCEPTION_TAG, "T")]),
            body.as_bytes(),
            4096,
        );
        assert!(
            matches!(
                end,
                Err(AppError::Db {
                    vendor_code: Some(241),
                    ..
                })
            ),
            "{end:?}"
        );
    }

    #[test]
    fn statuses_and_summaries() {
        let mut h = head(&[(header::EXCEPTION_CODE, "60")]);
        h.status = 404;
        let e = status_error(
            &h,
            b"Code: 60. DB::Exception: Table default.nope does not exist. (UNKNOWN_TABLE)",
        );
        assert!(matches!(
            e,
            AppError::Db {
                vendor_code: Some(60),
                ..
            }
        ));
        let mut h = head(&[]);
        h.status = 502;
        assert!(matches!(
            status_error(&h, b"<html><body>Bad Gateway</body></html>"),
            AppError::Connect(_)
        ));
        h.status = 401;
        assert!(matches!(status_error(&h, b""), AppError::Auth(_)));
        h.status = 504;
        assert!(matches!(status_error(&h, b""), AppError::Timeout));
        h.status = 500;
        assert!(
            status_error(&h, b"oops")
                .to_string()
                .contains("HTTP 500: oops")
        );
        let h = head(&[(
            header::SUMMARY,
            r#"{"read_rows":"5","read_bytes":"40","written_rows":"1","written_bytes":"8","total_rows_to_read":"0","result_rows":"1","result_bytes":"8","elapsed_ns":"1234"}"#,
        )]);
        let s = Summary::from_head(&h).unwrap();
        assert_eq!((s.read_rows, s.written_rows, s.elapsed_ns), (5, 1, 1234));
        assert_eq!(
            Summary::from_head(&head(&[(header::SUMMARY, "garbage")])),
            None
        );
        assert_eq!(Summary::from_head(&head(&[])), None);
    }

    /// A small xorshift generator: the same garbage on every machine.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    #[test]
    fn garbage_never_panics() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let body_atoms: &[&[u8]] = &[
            b"[",
            b"]",
            b"\"",
            b",",
            b"\n",
            b"\r\n",
            b"null",
            b"1",
            b"-",
            b"1e999",
            b"\\u00",
            b"\\ud800",
            b"{",
            b"}",
            b":",
            b"true",
            b"__exception__",
            b"T",
            b"Code: 1. DB::Exception: x",
            b"\xff",
            b"\xe2\x82",
            b"\"nan\"",
            b"\"x\"",
        ];
        let type_atoms = [
            "Nullable(",
            "LowCardinality(",
            "Array(",
            "Map(",
            "Tuple(",
            "Nested(",
            "Variant(",
            "SimpleAggregateFunction(sum, ",
            "AggregateFunction(",
            ")",
            ",",
            "'",
            "\\",
            "\"",
            "`",
            "String",
            "Int256",
            "UInt8",
            "Enum8(",
            "'a' = ",
            "1",
            "-",
            "DateTime64(",
            "Decimal(",
            "FixedString(",
            "JSON(",
            "x y",
            "=",
            "Bool",
            "Float64",
            "IPv6",
        ];
        let h = head(&[(header::EXCEPTION_TAG, "T")]);
        for _ in 0..2000 {
            let mut ty = String::new();
            for _ in 0..rng.below(12) {
                ty.push_str(type_atoms[rng.below(type_atoms.len())]);
            }
            let ty2 = type_atoms[rng.below(type_atoms.len())].trim_end_matches('(');
            let mut body =
                format!("[\"a\",\"b\"]\n{}\n", serde_json::json!([ty, ty2])).into_bytes();
            for _ in 0..rng.below(40) {
                body.extend_from_slice(body_atoms[rng.below(body_atoms.len())]);
            }
            let step = rng.below(9) + 1;
            let _ = read(&h, &body, step);
            let cut = rng.below(body.len() + 1);
            let _ = read(&h, &body[..cut], step);
            let _ = parse_exception(&String::from_utf8_lossy(&body));
        }
    }
}
