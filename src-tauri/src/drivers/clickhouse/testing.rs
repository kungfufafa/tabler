//! A scripted ClickHouse HTTP server for the client's tests, answering `Transport` calls
//! directly: replies per statement (rows in `JSONCompactEachRowWithNamesAndTypes` with their
//! summary, an error before or after the rows, a query that runs until it is killed), the
//! server's own rules (credentials, `readonly=2` refusing writes and changes of `readonly`, a
//! `readonly=1` profile refusing every setting, settings a constraint fixes, in the URL or in
//! a statement's SETTINGS clause, `getSetting('readonly')`, `KILL QUERY`, `/ping`), bodies cut
//! into small pieces, and every request recorded. `ReqwestTransport` sends the same requests over a socket to `testkit::fake_http`
//! for the wire-level tests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;

use crate::error::AppError;

use super::client::{READONLY_PROBE, SETTINGS_PROBE};
use super::transport::{
    ByteStream, HttpRequest, Method, ResponseHead, StreamingResponse, Transport, header,
};

/// A canned reply.
#[derive(Debug, Clone)]
pub enum FakeReply {
    /// A result: names, types and rows (each a JSON array's text).
    Rows {
        names: Vec<String>,
        types: Vec<String>,
        rows: Vec<String>,
    },
    /// A statement without output that wrote `written_rows` rows.
    Done { written_rows: u64 },
    /// An error before any output (HTTP 500 with the exception text and code header).
    Error { code: i64, message: String },
    /// Rows, then an error in the tagged block.
    FailAfter {
        names: Vec<String>,
        types: Vec<String>,
        rows: Vec<String>,
        code: i64,
        message: String,
    },
    /// The header lines, then nothing until the query is killed (an error 394 follows) or the
    /// client goes away.
    Hang {
        names: Vec<String>,
        types: Vec<String>,
    },
    /// A body as it is, with status 200 and the given `X-ClickHouse-Format`.
    Raw { format: String, body: String },
}

impl FakeReply {
    pub fn rows(names: &[&str], types: &[&str], rows: &[&str]) -> FakeReply {
        FakeReply::Rows {
            names: names.iter().map(|s| s.to_string()).collect(),
            types: types.iter().map(|s| s.to_string()).collect(),
            rows: rows.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn error(code: i64, message: &str) -> FakeReply {
        FakeReply::Error {
            code,
            message: message.to_string(),
        }
    }
}

/// The text the server writes for an error.
pub fn exception_text(code: i64, message: &str) -> String {
    format!("Code: {code}. DB::Exception: {message}. (FAKE_{code}) (version 25.8.1.1)")
}

fn header_lines(names: &[String], types: &[String]) -> String {
    let json = |v: &[String]| serde_json::to_string(v).unwrap_or_default();
    format!("{}\n{}\n", json(names), json(types))
}

/// The tagged block of an error after the status was sent.
pub fn exception_block(tag: &str, text: &str) -> String {
    format!(
        "\r\n__exception__\r\n{tag}\r\n{text}\r\n{} {tag}\r\n__exception__\r\n",
        text.len()
    )
}

const TAG: &str = "faketagfaketag12";

struct Rule {
    /// A piece of the statement's text.
    contains: String,
    reply: FakeReply,
    /// Used once, then removed.
    once: bool,
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    log: Vec<HttpRequest>,
    killed: Vec<String>,
    /// Query ids whose body the client dropped before its end.
    dropped: Vec<String>,
}

pub struct FakeClickhouse {
    state: Arc<Mutex<State>>,
    /// `(user, password)` the server accepts; None accepts anyone.
    pub credentials: Option<(String, String)>,
    /// The user's profile is `readonly=1`: every setting is refused.
    pub profile_readonly: bool,
    /// Settings a `const` constraint of the profile fixes: changing one is refused (452).
    pub fixed_settings: Vec<String>,
    /// Bodies go out in pieces of this many bytes.
    pub piece: usize,
}

impl Default for FakeClickhouse {
    fn default() -> Self {
        FakeClickhouse::new()
    }
}

/// URL parameters that are no settings.
const NOT_SETTINGS: &[&str] = &[
    "database",
    "query_id",
    "session_id",
    "session_timeout",
    "default_format",
    "role",
];

/// First words the fake refuses under `readonly`.
const WRITES: &[&str] = &[
    "INSERT", "ALTER", "CREATE", "DROP", "DELETE", "UPDATE", "TRUNCATE", "RENAME", "OPTIMIZE",
    "SYSTEM", "GRANT", "REVOKE",
];

impl FakeClickhouse {
    pub fn new() -> FakeClickhouse {
        FakeClickhouse {
            state: Arc::new(Mutex::new(State::default())),
            credentials: None,
            profile_readonly: false,
            fixed_settings: Vec::new(),
            piece: 7,
        }
    }

    /// Replies `reply` to every statement holding `contains` (the first rule that matches).
    pub fn on(&self, contains: &str, reply: FakeReply) {
        self.state.lock().unwrap().rules.push(Rule {
            contains: contains.to_string(),
            reply,
            once: false,
        });
    }

    /// Like `on`, for the next matching statement only.
    pub fn once(&self, contains: &str, reply: FakeReply) {
        self.state.lock().unwrap().rules.push(Rule {
            contains: contains.to_string(),
            reply,
            once: true,
        });
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.state.lock().unwrap().log.clone()
    }

    /// The statements sent, in order.
    pub fn statements(&self) -> Vec<String> {
        self.requests().iter().map(HttpRequest::sql).collect()
    }

    pub fn killed(&self) -> Vec<String> {
        self.state.lock().unwrap().killed.clone()
    }

    pub fn dropped(&self) -> Vec<String> {
        self.state.lock().unwrap().dropped.clone()
    }

    fn reply(&self, req: &HttpRequest) -> FakeReply {
        let fail = |code: i64, message: String| FakeReply::Error { code, message };
        if let Some((user, password)) = &self.credentials {
            let ok = req.header(header::USER) == Some(user.as_str())
                && req.header(header::KEY) == Some(password.as_str());
            if !ok {
                return fail(
                    516,
                    format!(
                        "{}: Authentication failed: password is incorrect, or there is no user \
                         with such name",
                        req.header(header::USER).unwrap_or("default")
                    ),
                );
            }
        }
        let pairs = req.query_pairs();
        let url_settings: Vec<&String> = pairs
            .iter()
            .map(|(k, _)| k)
            .filter(|k| !NOT_SETTINGS.contains(&k.as_str()) && !k.starts_with("param_"))
            .collect();
        let url_readonly = req.param("readonly").is_some_and(|v| v != "0");
        let sql = req.sql();
        // The URL's settings are applied before the statement is read, then its own.
        for (k, from_url) in url_settings
            .iter()
            .map(|k| (k.to_string(), true))
            .chain(statement_settings(&sql).into_iter().map(|k| (k, false)))
        {
            if self.profile_readonly || (k == "readonly" && !from_url && url_readonly) {
                return fail(164, format!("Cannot modify '{k}' setting in readonly mode"));
            }
            if self.fixed_settings.contains(&k) {
                return fail(452, format!("Setting {k} should not be changed"));
            }
        }
        if sql == SETTINGS_PROBE {
            return FakeReply::rows(&["settings_probe"], &["UInt8"], &["[1]"]);
        }
        if sql == READONLY_PROBE {
            let level = if self.profile_readonly {
                "1".to_string()
            } else {
                req.param("readonly").unwrap_or_else(|| "0".into())
            };
            let row = format!("[{level}]");
            return FakeReply::rows(&["readonly"], &["UInt8"], &[row.as_str()]);
        }
        let first = sql
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let readonly = self.profile_readonly || url_readonly;
        if readonly && WRITES.contains(&first.as_str()) {
            return fail(164, "default: Cannot execute query in readonly mode".into());
        }
        if first == "KILL" {
            let id = req.param("param_id").unwrap_or_default();
            self.state.lock().unwrap().killed.push(id);
            return FakeReply::rows(&["kill_status", "query_id"], &["String", "String"], &[]);
        }
        let mut state = self.state.lock().unwrap();
        let Some(i) = state.rules.iter().position(|r| sql.contains(&r.contains)) else {
            return fail(62, format!("Syntax error: no reply scripted for {sql:?}"));
        };
        let reply = state.rules[i].reply.clone();
        if state.rules[i].once {
            state.rules.remove(i);
        }
        reply
    }
}

/// The names a statement's trailing `SETTINGS a = 1, b = 2` clause sets.
fn statement_settings(sql: &str) -> Vec<String> {
    let upper = sql.to_ascii_uppercase();
    let Some(at) = upper.rfind(" SETTINGS ") else {
        return Vec::new();
    };
    sql[at + " SETTINGS ".len()..]
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, _)| k.trim().to_string())
        .collect()
}

fn head(status: u16, headers: &[(&str, String)]) -> ResponseHead {
    ResponseHead {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    }
}

fn summary(written_rows: u64, result_rows: u64) -> String {
    format!(
        "{{\"read_rows\":\"0\",\"read_bytes\":\"0\",\"written_rows\":\"{written_rows}\",\
         \"written_bytes\":\"0\",\"total_rows_to_read\":\"0\",\"result_rows\":\"{result_rows}\",\
         \"result_bytes\":\"0\",\"elapsed_ns\":\"1000\"}}"
    )
}

/// Notes a body the client dropped before its end.
struct DropGuard {
    state: Arc<Mutex<State>>,
    query_id: String,
    done: bool,
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        if !self.done
            && let Ok(mut s) = self.state.lock()
        {
            s.dropped.push(self.query_id.clone());
        }
    }
}

fn pieces(body: String, size: usize, guard: DropGuard) -> ByteStream {
    let chunks: Vec<Vec<u8>> = body
        .into_bytes()
        .chunks(size.max(1))
        .map(<[u8]>::to_vec)
        .collect();
    let n = chunks.len();
    let mut guard = guard;
    guard.done = n == 0;
    let mut guard = Some(guard);
    Box::pin(
        futures::stream::iter(chunks.into_iter().enumerate()).map(move |(i, c)| {
            if i + 1 == n
                && let Some(g) = guard.as_mut()
            {
                g.done = true;
            }
            Ok(c)
        }),
    )
}

#[async_trait]
impl Transport for FakeClickhouse {
    async fn send(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        self.state.lock().unwrap().log.push(req.clone());
        if req.method == Method::Get {
            let body = if req.path() == "/ping" { "Ok.\n" } else { "" };
            let status = if body.is_empty() { 404 } else { 200 };
            return Ok(StreamingResponse {
                head: head(status, &[]),
                body: Box::pin(futures::stream::iter([Ok(body.as_bytes().to_vec())])),
            });
        }
        let query_id = req.param("query_id").unwrap_or_default();
        let guard = DropGuard {
            state: self.state.clone(),
            query_id: query_id.clone(),
            done: false,
        };
        let common = |extra: Vec<(&'static str, String)>| {
            let mut h = vec![
                (header::QUERY_ID, query_id.clone()),
                (header::TIMEZONE, "UTC".to_string()),
                (
                    header::FORMAT,
                    "JSONCompactEachRowWithNamesAndTypes".to_string(),
                ),
                (header::EXCEPTION_TAG, TAG.to_string()),
            ];
            h.extend(extra);
            h
        };
        Ok(match self.reply(&req) {
            FakeReply::Rows { names, types, rows } => {
                let mut body = header_lines(&names, &types);
                for r in &rows {
                    body.push_str(r);
                    body.push('\n');
                }
                StreamingResponse {
                    head: head(
                        200,
                        &common(vec![(header::SUMMARY, summary(0, rows.len() as u64))]),
                    ),
                    body: pieces(body, self.piece, guard),
                }
            }
            FakeReply::Done { written_rows } => StreamingResponse {
                head: head(
                    200,
                    &common(vec![(header::SUMMARY, summary(written_rows, 0))]),
                ),
                body: pieces(String::new(), self.piece, guard),
            },
            FakeReply::Error { code, message } => StreamingResponse {
                head: head(500, &[(header::EXCEPTION_CODE, code.to_string())]),
                body: pieces(exception_text(code, &message), self.piece, guard),
            },
            FakeReply::FailAfter {
                names,
                types,
                rows,
                code,
                message,
            } => {
                let mut body = header_lines(&names, &types);
                for r in &rows {
                    body.push_str(r);
                    body.push('\n');
                }
                body.push_str(&exception_block(TAG, &exception_text(code, &message)));
                StreamingResponse {
                    head: head(200, &common(Vec::new())),
                    body: pieces(body, self.piece, guard),
                }
            }
            FakeReply::Raw { format, body } => StreamingResponse {
                head: head(200, &[(header::FORMAT, format)]),
                body: pieces(body, self.piece, guard),
            },
            FakeReply::Hang { names, types } => {
                let first = header_lines(&names, &types).into_bytes();
                let state = self.state.clone();
                let id = query_id.clone();
                let mut guard = Some(guard);
                let wait = futures::stream::once(async move {
                    loop {
                        if state.lock().map(|s| s.killed.contains(&id)).unwrap_or(true) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    if let Some(g) = guard.as_mut() {
                        g.done = true;
                    }
                    Ok(
                        exception_block(TAG, &exception_text(394, "Query was cancelled"))
                            .into_bytes(),
                    )
                });
                StreamingResponse {
                    head: head(200, &common(Vec::new())),
                    body: Box::pin(futures::stream::iter([Ok(first)]).chain(wait)),
                }
            }
        })
    }
}

/// reqwest without proxies (the requests must reach 127.0.0.1).
pub struct ReqwestTransport {
    http: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> ReqwestTransport {
        crate::testkit::install_crypto();
        ReqwestTransport {
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("a reqwest client"),
        }
    }
}

fn map_reqwest(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::Timeout
    } else if e.is_connect() {
        AppError::Connect(e.to_string())
    } else {
        AppError::NotConnected
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let mut builder = match req.method {
            Method::Get => self.http.get(&req.url),
            Method::Post => self.http.post(&req.url),
        };
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body);
        }
        let resp = builder.send().await.map_err(map_reqwest)?;
        let head = ResponseHead {
            status: resp.status().as_u16(),
            headers: resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_ascii_lowercase(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect(),
        };
        let body = resp
            .bytes_stream()
            .map(|piece| piece.map(|b| b.to_vec()).map_err(map_reqwest));
        Ok(StreamingResponse {
            head,
            body: Box::pin(body),
        })
    }
}
