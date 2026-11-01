//! A scripted ClickHouse HTTP server for the client's tests, answering `Transport` calls
//! directly: replies per statement (rows in `JSONCompactEachRowWithNamesAndTypes` with their
//! summary, an error before or after the rows, a query that runs until it is killed), the
//! server's own rules (credentials, `readonly=2` refusing writes and changes of `readonly`, a
//! `readonly=1` profile refusing every setting, settings a constraint fixes, in the URL or in
//! a statement's SETTINGS clause, `getSetting('readonly')`, `KILL QUERY`, `/ping`), bodies cut
//! into small pieces, and every request recorded. `serve` puts the same fake on a socket
//! (`testkit::fake_http`) for the shared HTTP client, and `app_connection` holds a session in an
//! app state for the command bodies.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;
use tokio::sync::{RwLock, watch};

use crate::appdb::AppDb;
use crate::drivers::Session;
use crate::drivers::common::http::{Credential, HttpClient, HttpEndpoint, HttpOptions};
use crate::error::AppError;
use crate::secrets::{Backend, Secrets};
use crate::state::{AppState, Connection as AppConnection};
use crate::testkit::{FakeServer, Reply, Route};
use crate::types::profile::{ConnStatus, ConnectionInfo, ConnectionProfile};

use super::client::{READONLY_PROBE, SETTINGS_PROBE};
use super::transport::{
    ByteStream, HttpRequest, HttpTransport, Method, ResponseHead, StreamingResponse, Transport,
    header,
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
    /// An error of a started query before any output (HTTP 500, or 403 for 192, 193 and 516
    /// raised while it runs, with the exception text, the code header and the query id).
    Error { code: i64, message: String },
    /// A sign-in refused before the query starts: a 403 without `X-ClickHouse-Query-Id`.
    Refused { code: i64, message: String },
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
                return FakeReply::Refused {
                    code: 516,
                    message: format!(
                        "{}: Authentication failed: password is incorrect, or there is no user \
                         with such name",
                        req.header(header::USER).unwrap_or("default")
                    ),
                };
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
            // As the server: 192, 193 and 516 are a 403 whatever raised them
            // (`exceptionCodeToHTTPStatus`); the query id is set once the query has started.
            FakeReply::Error { code, message } => StreamingResponse {
                head: head(
                    if matches!(code, 192 | 193 | 516) {
                        403
                    } else {
                        500
                    },
                    &common(vec![(header::EXCEPTION_CODE, code.to_string())]),
                ),
                body: pieces(exception_text(code, &message), self.piece, guard),
            },
            FakeReply::Refused { code, message } => StreamingResponse {
                head: head(403, &[(header::EXCEPTION_CODE, code.to_string())]),
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

/// The shared HTTP client's transport to `server`, sending `credential` (no proxy, no retries).
pub fn http_transport(server: &FakeServer, credential: Credential) -> HttpTransport {
    crate::testkit::install_crypto();
    let options = HttpOptions {
        retry: crate::drivers::common::http::RetryPolicy::NONE,
        ..HttpOptions::default()
    };
    let endpoint = HttpEndpoint::new(&server.url("/"), None).expect("an endpoint");
    HttpTransport::new(HttpClient::new(endpoint, options, credential).expect("a client"))
}

/// `fake` on a socket: each request goes to it as the client sent it, and its reply (headers,
/// status and the whole body) goes back. Replies that wait for a kill (`FakeReply::Hang`) need
/// the in-process fake.
pub async fn serve(fake: Arc<FakeClickhouse>) -> FakeServer {
    crate::testkit::install_crypto();
    let route = Route::new("*", "/*").handler(move |req| {
        let url = match &req.query {
            Some(q) => format!("http://fake{}?{q}", req.path),
            None => format!("http://fake{}", req.path),
        };
        let method = if req.method.eq_ignore_ascii_case("GET") {
            Method::Get
        } else {
            Method::Post
        };
        let sent = HttpRequest {
            method,
            url,
            headers: req.headers.clone(),
            body: req.body.clone(),
        };
        let replied = futures::executor::block_on(async {
            let resp = fake.send(sent).await?;
            resp.collect(usize::MAX).await
        });
        match replied {
            Ok((head, body)) => {
                let mut out = Reply::new(head.status).body(body);
                for (k, v) in &head.headers {
                    out = out.header(k, v);
                }
                out
            }
            Err(e) => Reply::text(500, e.to_string()),
        }
    });
    FakeServer::start(vec![route]).await
}

/// A ClickHouse profile of `fields` over the defaults (plain HTTP, no host).
pub fn profile(fields: serde_json::Value) -> ConnectionProfile {
    let mut json = serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "ch", "driver": "clickhouse",
        "ssl": "disable", "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
    });
    if let (Some(base), Some(extra)) = (json.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(json).expect("a profile")
}

/// A new app state with one open connection of `profile` holding `session`, for the command
/// bodies (`browse_rows`, `apply_changes_on`…).
pub async fn app_connection(
    profile: ConnectionProfile,
    session: Session,
) -> (Arc<AppState>, Arc<AppConnection>) {
    let appdb = AppDb::open_in_memory().expect("an app database");
    let profile = appdb.upsert_profile(&profile).expect("a saved profile");
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let id = uuid::Uuid::new_v4();
    let conn = Arc::new(AppConnection {
        id,
        profile: profile.clone(),
        info: RwLock::new(ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: None,
            current_user: None,
            tunnel: None,
            read_only: profile.options.read_only,
            capabilities: Some(session.capabilities()),
        }),
        session: RwLock::new(session),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: None,
    });
    state.insert_connection(conn.clone(), None).await;
    (state, conn)
}
