//! An in-process Hrana server over rusqlite for the LibSQL tests: `GET v2|v3`, `POST
//! v2|v3/pipeline` and `POST v3/cursor`, streams with batons that change on every reply, a
//! connection per stream on one database file (so a transaction on one stream is invisible to
//! the others until it commits), batches with conditions, `get_autocommit`, `describe`, expiry,
//! a `base_url` to follow, and canned failures (401, 404, HTML 5xx…).
//!
//! It answers `Transport` calls directly; once `testkit::fake_http` takes handlers (a change
//! request), the same `handle` can sit behind a real socket.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use rusqlite::Connection;
use rusqlite::types::Value as Sv;
use serde_json::{Value as J, json};
use tokio::time::Instant;

use crate::drivers::sqlite_http::codes::code_name;
use crate::drivers::sqlite_http::testing::{Executed, run};
use crate::drivers::sqlite_http::transport::{
    HttpRequest, HttpResponse, Method, StreamingResponse, Transport,
};
use crate::drivers::sqlite_http::value::decode_base64;
use crate::error::AppError;

use super::hrana::HranaValue;

/// How long a stream may sit idle (Turso: about 10 s).
pub const IDLE: Duration = Duration::from_secs(10);

struct FakeStream {
    id: u64,
    conn: Connection,
    baton: String,
    last_used: Instant,
}

#[derive(Default)]
struct Inner {
    streams: HashMap<u64, FakeStream>,
    next_stream: u64,
    next_baton: u64,
    failures: VecDeque<HttpResponse>,
    status: Option<u16>,
    base_url: Option<String>,
    reply_delay: Option<Duration>,
}

pub struct FakeHrana {
    dir: tempfile::TempDir,
    v3: bool,
    inner: Mutex<Inner>,
    log: Mutex<Vec<HttpRequest>>,
}

impl FakeHrana {
    /// A server speaking Hrana 3 (and 2).
    pub fn new() -> FakeHrana {
        FakeHrana::with_version(true)
    }

    /// A server speaking only Hrana 2 (`GET v3` is a 404).
    pub fn v2_only() -> FakeHrana {
        FakeHrana::with_version(false)
    }

    fn with_version(v3: bool) -> FakeHrana {
        let dir = tempfile::tempdir().expect("a temp dir");
        let fake = FakeHrana {
            dir,
            v3,
            inner: Mutex::new(Inner::default()),
            log: Mutex::new(Vec::new()),
        };
        fake.connect()
            .pragma_update(None, "journal_mode", "WAL")
            .expect("WAL");
        fake
    }

    /// A connection to the server's database, for seeding and checking.
    pub fn connect(&self) -> Connection {
        let conn = Connection::open(self.dir.path().join("fake.db")).expect("the fake database");
        conn.busy_timeout(Duration::ZERO).expect("busy timeout");
        conn
    }

    pub fn seed(&self, sql: &str) {
        self.connect().execute_batch(sql).expect("seed");
    }

    /// The requests so far.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.log.lock().unwrap().clone()
    }

    pub fn request_bodies(&self) -> Vec<J> {
        self.requests()
            .iter()
            .map(|r| r.json().unwrap_or(J::Null))
            .collect()
    }

    /// The next replies, whatever is asked.
    pub fn fail_next(&self, reply: HttpResponse) {
        self.inner.lock().unwrap().failures.push_back(reply);
    }

    /// Every reply has this status (401: a wrong token).
    pub fn answer_status(&self, status: Option<u16>) {
        self.inner.lock().unwrap().status = status;
    }

    /// Replies carry this `base_url`.
    pub fn set_base_url(&self, url: Option<&str>) {
        self.inner.lock().unwrap().base_url = url.map(str::to_string);
    }

    /// Pipelines run, then the reply waits this long (a client that gives up never reads it).
    pub fn delay_replies(&self, delay: Option<Duration>) {
        self.inner.lock().unwrap().reply_delay = delay;
    }

    /// Drops every stream, as the server does after the idle timeout.
    pub fn expire_streams(&self) {
        self.inner.lock().unwrap().streams.clear();
    }

    pub fn open_streams(&self) -> usize {
        self.inner.lock().unwrap().streams.len()
    }

    /// Answers one request.
    pub fn handle(&self, req: &HttpRequest) -> HttpResponse {
        self.log.lock().unwrap().push(req.clone());
        let mut inner = self.inner.lock().unwrap();
        if let Some(reply) = inner.failures.pop_front() {
            return reply;
        }
        if let Some(status) = inner.status {
            return HttpResponse::text(status, "Unauthorized");
        }
        let path = req.path();
        let v3_path = path.ends_with("/v3/pipeline") || path.ends_with("/v3/cursor");
        match req.method {
            Method::Get if path.ends_with("/v2") => HttpResponse::text(200, ""),
            Method::Get if path.ends_with("/v3") && self.v3 => HttpResponse::text(200, ""),
            Method::Post if v3_path && !self.v3 => HttpResponse::text(404, "Not Found"),
            Method::Post if path.ends_with("/pipeline") => self.pipeline(&mut inner, req),
            Method::Post if path.ends_with("/v3/cursor") => self.cursor(&mut inner, req),
            _ => HttpResponse::text(404, &format!("no route for {path}")),
        }
    }

    fn bad_request(message: &str, code: &str) -> HttpResponse {
        HttpResponse::json(400, &json!({"message": message, "code": code}))
    }

    /// The stream a baton names (a new one for `null`), or the 400 a server answers.
    fn stream<'a>(
        &self,
        inner: &'a mut Inner,
        baton: &J,
    ) -> Result<&'a mut FakeStream, HttpResponse> {
        let now = Instant::now();
        inner
            .streams
            .retain(|_, s| now.duration_since(s.last_used) < IDLE);
        let id = match baton {
            J::Null => {
                inner.next_stream += 1;
                let id = inner.next_stream;
                inner.streams.insert(
                    id,
                    FakeStream {
                        id,
                        conn: self.connect(),
                        baton: String::new(),
                        last_used: now,
                    },
                );
                id
            }
            J::String(b) => {
                let id = b
                    .split('.')
                    .next()
                    .and_then(|s| s.strip_prefix('s'))
                    .and_then(|s| s.parse::<u64>().ok());
                match id.filter(|id| inner.streams.get(id).is_some_and(|s| &s.baton == b)) {
                    Some(id) => id,
                    None => {
                        return Err(FakeHrana::bad_request(
                            "The stream has expired due to inactivity",
                            "STREAM_EXPIRED",
                        ));
                    }
                }
            }
            _ => {
                return Err(FakeHrana::bad_request(
                    "baton must be a string",
                    "PROTO_ERROR",
                ));
            }
        };
        inner.next_baton += 1;
        let next = inner.next_baton;
        let stream = inner.streams.get_mut(&id).ok_or_else(|| {
            FakeHrana::bad_request("The stream has expired due to inactivity", "STREAM_EXPIRED")
        })?;
        stream.baton = format!("s{id}.{next}");
        stream.last_used = now;
        Ok(stream)
    }

    fn pipeline(&self, inner: &mut Inner, req: &HttpRequest) -> HttpResponse {
        let Ok(body) = req.json() else {
            return FakeHrana::bad_request("invalid JSON", "PROTO_ERROR");
        };
        let base_url = inner.base_url.clone();
        let stream = match self.stream(inner, &body["baton"]) {
            Ok(s) => s,
            Err(reply) => return reply,
        };
        let mut closed = false;
        let mut results = Vec::new();
        for request in body["requests"].as_array().cloned().unwrap_or_default() {
            if closed {
                results.push(json!({"type": "error", "error": {"message": "stream closed", "code": "STREAM_CLOSED"}}));
                continue;
            }
            let ty = request["type"].as_str().unwrap_or_default();
            let response = match ty {
                "close" => {
                    closed = true;
                    Ok(json!({"type": "close"}))
                }
                "execute" => execute(&stream.conn, &request["stmt"])
                    .map(|r| json!({"type": "execute", "result": r})),
                "batch" => {
                    Ok(json!({"type": "batch", "result": batch(&stream.conn, &request["batch"])}))
                }
                "sequence" => stream
                    .conn
                    .execute_batch(request["sql"].as_str().unwrap_or_default())
                    .map(|_| json!({"type": "sequence"}))
                    .map_err(|e| error_json(&e)),
                "describe" => describe(&stream.conn, request["sql"].as_str().unwrap_or_default())
                    .map(|r| json!({"type": "describe", "result": r})),
                "get_autocommit" if self.v3 => Ok(json!({
                    "type": "get_autocommit",
                    "is_autocommit": stream.conn.is_autocommit()
                })),
                other => Err(
                    json!({"message": format!("unknown request {other}"), "code": "PROTO_ERROR"}),
                ),
            };
            results.push(match response {
                Ok(r) => json!({"type": "ok", "response": r}),
                Err(e) => json!({"type": "error", "error": e}),
            });
        }
        let id = stream.id;
        let baton = (!closed).then(|| stream.baton.clone());
        if closed {
            inner.streams.remove(&id);
        }
        HttpResponse::json(
            200,
            &json!({"baton": baton, "base_url": base_url, "results": results}),
        )
    }

    fn cursor(&self, inner: &mut Inner, req: &HttpRequest) -> HttpResponse {
        let Ok(body) = req.json() else {
            return FakeHrana::bad_request("invalid JSON", "PROTO_ERROR");
        };
        let base_url = inner.base_url.clone();
        let stream = match self.stream(inner, &body["baton"]) {
            Ok(s) => s,
            Err(reply) => return reply,
        };
        let mut lines = vec![json!({"baton": stream.baton, "base_url": base_url})];
        let steps = body["batch"]["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut outcomes: Vec<Option<bool>> = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            if !condition(&step["condition"], &outcomes, &stream.conn) {
                outcomes.push(None);
                continue;
            }
            match execute(&stream.conn, &step["stmt"]) {
                Ok(r) => {
                    lines.push(json!({"type": "step_begin", "step": i, "cols": r["cols"]}));
                    for row in r["rows"].as_array().cloned().unwrap_or_default() {
                        lines.push(json!({"type": "row", "row": row}));
                    }
                    lines.push(json!({
                        "type": "step_end",
                        "affected_row_count": r["affected_row_count"],
                        "last_insert_rowid": r["last_insert_rowid"]
                    }));
                    outcomes.push(Some(true));
                }
                Err(e) => {
                    lines.push(json!({"type": "step_error", "step": i, "error": e}));
                    outcomes.push(Some(false));
                }
            }
        }
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        HttpResponse {
            status: 200,
            retry_after: None,
            content_type: Some("text/plain".into()),
            body: text.into_bytes(),
        }
    }
}

impl Default for FakeHrana {
    fn default() -> FakeHrana {
        FakeHrana::new()
    }
}

#[async_trait]
impl Transport for FakeHrana {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let reply = self.handle(&req);
        let delay = self.inner.lock().unwrap().reply_delay;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        Ok(reply)
    }

    /// The body in small pieces, so cursor lines arrive split.
    async fn send_streaming(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let reply = self.send(req).await?;
        let pieces: Vec<Result<Vec<u8>, AppError>> =
            reply.body.chunks(13).map(|c| Ok(c.to_vec())).collect();
        Ok(StreamingResponse {
            status: reply.status,
            retry_after: reply.retry_after,
            content_type: reply.content_type,
            body: Box::pin(futures::stream::iter(pieces)),
        })
    }
}

/// A Hrana value from a request, bound the way `sqld` binds it.
fn to_sqlite(v: &J) -> Result<Sv, J> {
    let value: HranaValue = serde_json::from_value(v.clone())
        .map_err(|e| json!({"message": format!("invalid value: {e}"), "code": "ARGS_INVALID"}))?;
    Ok(match value {
        HranaValue::Null | HranaValue::Float { value: None } => Sv::Null,
        HranaValue::Integer { value } => value
            .parse()
            .map(Sv::Integer)
            .map_err(|_| json!({"message": "invalid integer", "code": "ARGS_INVALID"}))?,
        HranaValue::Float { value: Some(f) } => Sv::Real(f),
        HranaValue::Text { value } => Sv::Text(value),
        HranaValue::Blob { base64 } => Sv::Blob(
            decode_base64(&base64)
                .ok_or_else(|| json!({"message": "invalid base64", "code": "ARGS_INVALID"}))?,
        ),
    })
}

fn from_sqlite(v: &Sv) -> J {
    match v {
        Sv::Null => json!({"type": "null"}),
        Sv::Integer(n) => json!({"type": "integer", "value": n.to_string()}),
        Sv::Real(f) => json!({"type": "float", "value": f}),
        Sv::Text(t) => json!({"type": "text", "value": t}),
        // The padded form; the client sends it unpadded.
        Sv::Blob(b) => {
            json!({"type": "blob", "base64": base64::engine::general_purpose::STANDARD.encode(b)})
        }
    }
}

fn error_json(e: &rusqlite::Error) -> J {
    match e {
        rusqlite::Error::SqliteFailure(ffi, msg) => json!({
            "message": format!("SQLite error: {}", msg.clone().unwrap_or_else(|| ffi.to_string())),
            "code": code_name(ffi.extended_code),
        }),
        rusqlite::Error::SqlInputError { msg, error, .. } => json!({
            "message": format!("SQLite error: {msg}"),
            "code": code_name(error.extended_code),
        }),
        rusqlite::Error::MultipleStatement => json!({
            "message": "SQL string contains more than one statement",
            "code": "SQL_MANY_STATEMENTS",
        }),
        other => json!({"message": other.to_string(), "code": "SQLITE_UNKNOWN"}),
    }
}

fn execute(conn: &Connection, stmt: &J) -> Result<J, J> {
    let sql = stmt["sql"].as_str().unwrap_or_default();
    let args: Vec<Sv> = stmt["args"]
        .as_array()
        .map(|a| a.iter().map(to_sqlite).collect::<Result<_, _>>())
        .transpose()?
        .unwrap_or_default();
    let named: Vec<(String, Sv)> = stmt["named_args"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| {
                    Ok((
                        n["name"].as_str().unwrap_or_default().to_string(),
                        to_sqlite(&n["value"])?,
                    ))
                })
                .collect::<Result<_, J>>()
        })
        .transpose()?
        .unwrap_or_default();
    let want_rows = stmt["want_rows"].as_bool().unwrap_or(true);
    let Executed {
        cols,
        rows,
        changes,
        last_insert_rowid,
        ..
    } = run(conn, sql, &args, &named).map_err(|e| error_json(&e))?;
    Ok(json!({
        "cols": cols.iter().map(|(n, d)| json!({"name": n, "decltype": d})).collect::<Vec<_>>(),
        "rows": if want_rows {
            rows.iter().map(|r| r.iter().map(from_sqlite).collect::<Vec<_>>()).collect::<Vec<_>>()
        } else {
            Vec::new()
        },
        "affected_row_count": changes,
        "last_insert_rowid": last_insert_rowid.to_string(),
        "rows_read": 0,
        "rows_written": changes,
        "query_duration_ms": 0.1,
    }))
}

/// Whether a step's condition holds, from the earlier steps' outcomes (`None`: skipped).
fn condition(cond: &J, outcomes: &[Option<bool>], conn: &Connection) -> bool {
    let step = |c: &J| {
        c["step"]
            .as_u64()
            .and_then(|s| outcomes.get(s as usize).copied().flatten())
    };
    match cond["type"].as_str() {
        None => true,
        Some("ok") => step(cond) == Some(true),
        Some("error") => step(cond) == Some(false),
        Some("not") => !condition(&cond["cond"], outcomes, conn),
        Some("and") => cond["conds"]
            .as_array()
            .is_some_and(|c| c.iter().all(|c| condition(c, outcomes, conn))),
        Some("or") => cond["conds"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| condition(c, outcomes, conn))),
        Some("is_autocommit") => conn.is_autocommit(),
        Some(_) => false,
    }
}

fn batch(conn: &Connection, batch: &J) -> J {
    let steps = batch["steps"].as_array().cloned().unwrap_or_default();
    let mut outcomes = Vec::new();
    let mut results = Vec::new();
    let mut errors = Vec::new();
    for step in &steps {
        if !condition(&step["condition"], &outcomes, conn) {
            outcomes.push(None);
            results.push(J::Null);
            errors.push(J::Null);
            continue;
        }
        match execute(conn, &step["stmt"]) {
            Ok(r) => {
                outcomes.push(Some(true));
                results.push(r);
                errors.push(J::Null);
            }
            Err(e) => {
                outcomes.push(Some(false));
                results.push(J::Null);
                errors.push(e);
            }
        }
    }
    json!({"step_results": results, "step_errors": errors})
}

fn describe(conn: &Connection, sql: &str) -> Result<J, J> {
    let stmt = conn.prepare(sql).map_err(|e| error_json(&e))?;
    let params: Vec<J> = (1..=stmt.parameter_count())
        .map(|i| json!({"name": stmt.parameter_name(i)}))
        .collect();
    let cols: Vec<J> = stmt
        .columns()
        .iter()
        .map(|c| json!({"name": c.name(), "decltype": c.decl_type()}))
        .collect();
    Ok(json!({
        "params": params,
        "cols": cols,
        "is_explain": sql.trim_start().to_ascii_uppercase().starts_with("EXPLAIN"),
        "is_readonly": stmt.readonly(),
    }))
}
